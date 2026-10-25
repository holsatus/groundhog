//! Import and export of missions in the "QGC WPL 110" plain-text format.
//!
//! By convention (QGroundControl, Mission Planner) the first row of a WPL file
//! is the home position, and the plan starts at row 1 — the same layout as
//! ArduPilot's home slot. Files are therefore read and written through the
//! mission codec with a home slot, which also keeps jump targets pointing at
//! the right rows.
//!
//! WPL predates the `_INT` MAVLink frames: it stores latitude/longitude as
//! degrees and uses the float global frames. Positional frames are normalized
//! to their `_INT` counterparts on the way in (`0/3/10 -> 5/6/11`) and back out.

use mavio::default_dialect::enums::{MavCmd, MavFrame, MavMissionType};

use crate::{
    domain::{
        firmware::MissionLayout,
        mission::{MissionItem, PlanTooLong, codec},
    },
    wire::{degrees, degrees_e7, mission_item::RawMissionItem},
};

/// The layout of a WPL file: home in row 0.
const WPL_LAYOUT: MissionLayout = MissionLayout { home_slot: true };

/// A mission read from a WPL file.
#[derive(Debug, Clone, PartialEq)]
pub struct WplMission {
    /// The home row, if the file had one besides the plan.
    pub home: Option<MissionItem>,
    pub items: Vec<MissionItem>,
}

const HEADER: &str = "QGC WPL 110";

/// A failure while parsing a WPL file, tagged with the 1-based line it occurred
/// on.
#[derive(Debug, Clone, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct WplError {
    pub line: usize,
    pub message: String,
}

impl WplError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        WplError {
            line,
            message: message.into(),
        }
    }
}

/// Serialize a plan as a "QGC WPL 110" document, with `home` (or a zero
/// placeholder) in row 0. The format requires the home row, so an empty plan
/// is written as that row alone.
///
/// # Errors
///
/// [`PlanTooLong`] when the plan has more items than a file can number.
pub fn write_wpl(items: &[MissionItem], home: Option<&MissionItem>) -> Result<String, PlanTooLong> {
    let wire = if items.is_empty() {
        codec::home_row(home, WPL_LAYOUT, MavMissionType::Mission)
            .into_iter()
            .collect()
    } else {
        codec::encode(items, home, WPL_LAYOUT, MavMissionType::Mission)?
    };

    let mut out = String::from(HEADER);
    out.push('\n');

    for wire in wire {
        let frame = export_frame(wire.frame, wire.command);

        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            wire.seq,
            if wire.seq == 0 { 1 } else { 0 },
            u16::from(frame),
            wire.command,
            wire.params[0],
            wire.params[1],
            wire.params[2],
            wire.params[3],
            degrees(wire.x),
            degrees(wire.y),
            wire.z,
            wire.autocontinue,
        ));
    }

    Ok(out)
}

/// Parse a "QGC WPL 110" document. Blank lines are ignored; the header must be
/// the first non-blank line; the first row is home.
///
/// # Errors
///
/// A [`WplError`] naming the line: a missing or unknown header, a row whose
/// fields are not the eleven a WPL row has, or a field that is not a number.
pub fn parse_wpl(text: &str) -> Result<WplMission, WplError> {
    // A UTF-8 BOM is not part of the header, but editors add it.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    let mut rows = Vec::new();
    let mut header_seen = false;

    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim();

        if line.is_empty() {
            continue;
        }

        if !header_seen {
            validate_header(line).map_err(|message| WplError::at(line_number, message))?;
            header_seen = true;
            continue;
        }

        let mut row = parse_row(line, line_number)?;
        // The index column is advisory; rows are taken in file order.
        row.seq = rows.len() as u16;
        rows.push(row);
    }

    if !header_seen {
        return Err(WplError::at(1, "missing 'QGC WPL <version>' header"));
    }

    let mission = codec::decode(&rows, 0, WPL_LAYOUT);

    // The format requires a home row; a zero one is its placeholder, not a
    // real home (the same rule as other "unknown" sentinels).
    let home = mission
        .home
        .filter(|home| home.x != 0 || home.y != 0 || home.z != 0.0);

    Ok(WplMission {
        home,
        items: mission.items,
    })
}

fn validate_header(line: &str) -> Result<(), String> {
    let mut parts = line.split_whitespace();

    match (parts.next(), parts.next(), parts.next()) {
        (Some("QGC"), Some("WPL"), Some(version)) if version.parse::<u32>().is_ok() => Ok(()),
        _ => Err(format!(
            "expected a 'QGC WPL <version>' header, got '{line}'"
        )),
    }
}

fn parse_row(line: &str, line_number: usize) -> Result<RawMissionItem, WplError> {
    let fields: Vec<&str> = line.split('\t').collect();

    if fields.len() != 12 {
        return Err(WplError::at(
            line_number,
            format!("expected 12 tab-separated fields, got {}", fields.len()),
        ));
    }

    let integer = |index: usize, name: &str| -> Result<i64, WplError> {
        fields[index].trim().parse::<i64>().map_err(|_| {
            WplError::at(
                line_number,
                format!("{name} is not an integer: '{}'", fields[index]),
            )
        })
    };

    let float = |index: usize, name: &str| -> Result<f64, WplError> {
        fields[index].trim().parse::<f64>().map_err(|_| {
            WplError::at(
                line_number,
                format!("{name} is not a number: '{}'", fields[index]),
            )
        })
    };

    // The index and current columns are not part of the model: the sequence is
    // positional and the current item comes from MISSION_CURRENT.
    integer(0, "index")?;
    integer(1, "current")?;

    let frame_raw = integer(2, "frame")?;
    let frame = u8::try_from(frame_raw)
        .map_err(|_| WplError::at(line_number, format!("frame out of range: {frame_raw}")))?;

    let command_raw = integer(3, "command")?;
    let command = u16::try_from(command_raw)
        .map_err(|_| WplError::at(line_number, format!("command out of range: {command_raw}")))?;

    let param1 = float(4, "param1")? as f32;
    let param2 = float(5, "param2")? as f32;
    let param3 = float(6, "param3")? as f32;
    let param4 = float(7, "param4")? as f32;
    let lat = float(8, "latitude")?;
    let lon = float(9, "longitude")?;
    let alt = float(10, "altitude")?;
    let autocontinue = integer(11, "autocontinue")? != 0;

    if !(-90.0..=90.0).contains(&lat) {
        return Err(WplError::at(
            line_number,
            format!("latitude out of range: {lat}"),
        ));
    }
    if !(-180.0..=180.0).contains(&lon) {
        return Err(WplError::at(
            line_number,
            format!("longitude out of range: {lon}"),
        ));
    }
    if !alt.is_finite() {
        return Err(WplError::at(
            line_number,
            format!("altitude is not a finite number: {alt}"),
        ));
    }
    let alt = alt as f32;

    Ok(RawMissionItem {
        frame: import_frame(frame, command),
        command,
        autocontinue: u8::from(autocontinue),
        params: [param1, param2, param3, param4],
        x: degrees_e7(lat),
        y: degrees_e7(lon),
        z: alt,
        mission_type: MavMissionType::Mission as u8,
        ..Default::default()
    })
}

/// Whether `command` carries a location, per the command table. Unknown
/// commands (newer than the table) are not positional: their raw frame is
/// left as it is.
fn is_positional_command(command: u16) -> bool {
    MavCmd::try_from(command)
        .ok()
        .and_then(crate::domain::mission::command::spec_of)
        .is_some_and(|spec| spec.is_positional())
}

/// Map our global-INT frame back to the float global frame WPL uses.
fn export_frame(frame: u8, command: u16) -> u8 {
    if !is_positional_command(command) {
        return frame;
    }

    match MavFrame::try_from(frame) {
        Ok(MavFrame::GlobalInt) => MavFrame::Global as u8,
        Ok(MavFrame::GlobalRelativeAltInt) => MavFrame::GlobalRelativeAlt as u8,
        Ok(MavFrame::GlobalTerrainAltInt) => MavFrame::GlobalTerrainAlt as u8,
        _ => frame,
    }
}

/// Map WPL's float global frame to the global-INT frame our model expects.
fn import_frame(frame: u8, command: u16) -> u8 {
    if !is_positional_command(command) {
        return frame;
    }

    match MavFrame::try_from(frame) {
        Ok(MavFrame::Global) => MavFrame::GlobalInt as u8,
        Ok(MavFrame::GlobalRelativeAlt) => MavFrame::GlobalRelativeAltInt as u8,
        Ok(MavFrame::GlobalTerrainAlt) => MavFrame::GlobalTerrainAltInt as u8,
        _ => frame,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mission::Position;

    fn waypoint(lat: f64, lon: f64) -> MissionItem {
        let mut item = MissionItem::waypoint(Position {
            lat,
            lon,
            alt_m: 120.0,
        });
        item.frame = MavFrame::GlobalInt as u8;
        item.params = [3.0, 1.5, 2.5, 90.0];
        item
    }

    #[test]
    fn mixed_list_round_trips_with_its_home() {
        let mut delay = MissionItem::new(MavCmd::ConditionDelay, None);
        delay.params[0] = 4.0;

        let mut servo = MissionItem::new(MavCmd::DoSetServo, None);
        servo.autocontinue = false;
        servo.params = [1.0, 2.0, 3.0, 4.0];
        servo.x = 123;
        servo.y = -456;
        servo.z = 78.0;

        let mut jump = MissionItem::new(MavCmd::DoJump, None);
        jump.params[0] = 0.0;

        let items = vec![waypoint(47.4, 8.5), delay, servo, jump];
        let home = MissionItem::home(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 400.0,
        });

        let text = write_wpl(&items, Some(&home)).expect("the plan fits in a file");
        let parsed = parse_wpl(&text).expect("round trip should parse");

        assert_eq!(parsed.items, items);
        assert_eq!(parsed.home, Some(home));
    }

    #[test]
    fn the_jump_column_is_a_file_row_number() {
        let mut jump = MissionItem::new(MavCmd::DoJump, None);
        jump.params[0] = 0.0; // the first plan item
        let text = write_wpl(&[waypoint(1.0, 1.0), jump], None).expect("the plan fits in a file");

        let jump_row = text.lines().nth(3).unwrap();
        let target: f32 = jump_row.split('\t').nth(4).unwrap().parse().unwrap();
        assert_eq!(target, 1.0, "row 1, after home");
    }

    #[test]
    fn row_zero_is_home_and_frames_import_as_int() {
        let text = "QGC WPL 110\n\
                    0\t1\t0\t16\t0\t0\t0\t0\t47.0\t8.0\t400\t1\n\
                    1\t0\t3\t16\t0\t0\t0\t0\t47.4\t8.5\t120\t1\n";

        let mission = parse_wpl(text).expect("valid file");
        assert!(mission.home.is_some());
        assert_eq!(mission.items.len(), 1);

        let item = &mission.items[0];
        assert_eq!(item.known_frame(), Some(MavFrame::GlobalRelativeAltInt));
        assert!(item.position().is_some());
    }

    #[test]
    fn unknown_commands_and_frames_round_trip_in_a_file() {
        let text = "QGC WPL 110\n\
                    0\t1\t0\t16\t0\t0\t0\t0\t47.0\t8.0\t400\t1\n\
                    1\t0\t255\t31050\t1\t2\t3\t4\t12\t-34\t56\t1\n";

        let mission = parse_wpl(text).expect("a newer command still loads");
        let item = &mission.items[0];
        assert_eq!(item.command, 31_050);
        assert_eq!(item.frame, 255);

        let written = write_wpl(&mission.items, mission.home.as_ref()).expect("fits in a file");
        let reparsed = parse_wpl(&written).expect("the file round-trips");
        assert_eq!(reparsed.items, mission.items);
    }

    /// The format needs a home row, but its zero placeholder is not a real
    /// home: a round trip keeps a real one and invents none.
    #[test]
    fn a_saved_home_survives_and_a_placeholder_does_not() {
        let items = vec![waypoint(47.4, 8.5)];
        let home = MissionItem::home(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 400.0,
        });

        let text = write_wpl(&items, Some(&home)).expect("fits in a file");
        assert_eq!(
            parse_wpl(&text).expect("valid file").home,
            Some(home),
            "a real home comes back"
        );

        let text = write_wpl(&items, None).expect("fits in a file");
        assert_eq!(
            parse_wpl(&text).expect("valid file").home,
            None,
            "the zero placeholder is not taken for a home"
        );
    }

    #[test]
    fn a_utf8_bom_is_stripped() {
        let text = "\u{feff}QGC WPL 110\n\
                    0\t1\t0\t16\t0\t0\t0\t0\t47.0\t8.0\t400\t1\n";
        assert!(parse_wpl(text).is_ok());
    }

    #[test]
    fn coordinates_outside_their_range_are_refused() {
        for (lat, lon) in [(95.0, 8.0), (-91.0, 8.0), (47.0, 181.0), (47.0, -181.0)] {
            let text = format!("QGC WPL 110\n0\t1\t0\t16\t0\t0\t0\t0\t{lat}\t{lon}\t400\t1\n");
            assert!(
                parse_wpl(&text).is_err(),
                "({lat}, {lon}) is not a coordinate"
            );
        }
    }

    #[test]
    fn missing_header_is_an_error() {
        let error = parse_wpl("").expect_err("empty file has no header");
        assert_eq!(error.line, 1);
    }

    #[test]
    fn garbage_header_reports_its_line() {
        let error = parse_wpl("\nnot a header\n").expect_err("garbage header");
        assert_eq!(error.line, 2);
    }

    #[test]
    fn malformed_row_reports_its_line() {
        let text = "QGC WPL 110\n\
                    0\t1\t0\t16\t0\t0\t0\t0\t47.4\t8.5\t120\n";

        let error = parse_wpl(text).expect_err("row has 11 fields");
        assert_eq!(error.line, 2);
    }

    #[test]
    fn header_only_yields_empty_mission() {
        let mission = parse_wpl("QGC WPL 110\n").expect("valid header");
        assert!(mission.items.is_empty());
        assert!(mission.home.is_none());
    }

    #[test]
    fn an_empty_plan_is_written_with_its_home_row() {
        let text = write_wpl(&[], None).expect("an empty plan fits in a file");

        assert_eq!(text.lines().count(), 2, "the header and the home row");
        assert!(
            parse_wpl(&text).expect("valid file").items.is_empty(),
            "the placeholder home row is not a plan item"
        );
    }
}
