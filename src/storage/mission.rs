//! Import and export of missions in the "QGC WPL 110" plain-text format.
//!
//! WPL predates the `_INT` MAVLink frames: it stores latitude/longitude as
//! degrees and uses the float global frames. Our typed model requires the
//! global-INT frames for positional commands, so the frame is normalized on the
//! way in (`0/3/10 -> 5/6/11`) and back out.

use mavio::default_dialect::{
    enums::{MavCmd, MavFrame, MavMissionType},
    messages::MissionItemInt,
};

use crate::domain::mission::{is_positional_command, MissionItem, LATLON_SCALE};

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

/// Serialize `items` as a "QGC WPL 110" document.
pub fn write_wpl(items: &[MissionItem]) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');

    for (seq, item) in items.iter().enumerate() {
        let wire = item.to_wire(seq as u16, MavMissionType::Mission);
        let frame = export_frame(wire.frame, wire.command);

        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            seq,
            if seq == 0 { 1 } else { 0 },
            frame as u16,
            wire.command as u16,
            wire.param1,
            wire.param2,
            wire.param3,
            wire.param4,
            wire.x as f64 * LATLON_SCALE,
            wire.y as f64 * LATLON_SCALE,
            wire.z,
            u8::from(wire.autocontinue != 0),
        ));
    }

    out
}

/// Parse a "QGC WPL 110" document. Blank lines are ignored; the header must be
/// the first non-blank line.
pub fn parse_wpl(text: &str) -> Result<Vec<MissionItem>, WplError> {
    let mut items = Vec::new();
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

        items.push(parse_row(line, line_number)?);
    }

    if !header_seen {
        return Err(WplError::at(1, "missing 'QGC WPL <version>' header"));
    }

    Ok(items)
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

fn parse_row(line: &str, line_number: usize) -> Result<MissionItem, WplError> {
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
    let frame = MavFrame::try_from(
        u8::try_from(frame_raw)
            .map_err(|_| WplError::at(line_number, format!("frame out of range: {frame_raw}")))?,
    )
    .map_err(|_| WplError::at(line_number, format!("unknown frame: {frame_raw}")))?;

    let command_raw = integer(3, "command")?;
    let command =
        MavCmd::try_from(u16::try_from(command_raw).map_err(|_| {
            WplError::at(line_number, format!("command out of range: {command_raw}"))
        })?)
        .map_err(|_| WplError::at(line_number, format!("unknown command: {command_raw}")))?;

    let param1 = float(4, "param1")? as f32;
    let param2 = float(5, "param2")? as f32;
    let param3 = float(6, "param3")? as f32;
    let param4 = float(7, "param4")? as f32;
    let lat = float(8, "latitude")?;
    let lon = float(9, "longitude")?;
    let alt = float(10, "altitude")? as f32;
    let autocontinue = integer(11, "autocontinue")? != 0;

    let wire = MissionItemInt {
        frame: import_frame(frame, command),
        command,
        autocontinue: u8::from(autocontinue),
        param1,
        param2,
        param3,
        param4,
        x: (lat / LATLON_SCALE).round() as i32,
        y: (lon / LATLON_SCALE).round() as i32,
        z: alt,
        ..Default::default()
    };

    Ok(MissionItem::from_wire(&wire))
}

/// Map our global-INT frame back to the float global frame WPL uses.
fn export_frame(frame: MavFrame, command: MavCmd) -> MavFrame {
    if !is_positional_command(command) {
        return frame;
    }

    match frame {
        MavFrame::GlobalInt => MavFrame::Global,
        MavFrame::GlobalRelativeAltInt => MavFrame::GlobalRelativeAlt,
        MavFrame::GlobalTerrainAltInt => MavFrame::GlobalTerrainAlt,
        other => other,
    }
}

/// Map WPL's float global frame to the global-INT frame our model expects.
fn import_frame(frame: MavFrame, command: MavCmd) -> MavFrame {
    if !is_positional_command(command) {
        return frame;
    }

    match frame {
        MavFrame::Global => MavFrame::GlobalInt,
        MavFrame::GlobalRelativeAlt => MavFrame::GlobalRelativeAltInt,
        MavFrame::GlobalTerrainAlt => MavFrame::GlobalTerrainAltInt,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mission::{MissionCommand, Position};

    fn waypoint(lat: f64, lon: f64) -> MissionItem {
        MissionItem {
            frame: MavFrame::GlobalInt,
            autocontinue: true,
            command: MissionCommand::Waypoint {
                pos: Position {
                    lat,
                    lon,
                    alt_m: 120.0,
                },
                hold_s: 3.0,
                accept_radius_m: 1.5,
                pass_radius_m: 2.5,
                yaw_deg: Some(90.0),
            },
        }
    }

    #[test]
    fn mixed_list_round_trips() {
        let items = vec![
            waypoint(47.4, 8.5),
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: true,
                command: MissionCommand::ConditionDelay { seconds: 4.0 },
            },
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: false,
                command: MissionCommand::Unknown {
                    command: MavCmd::DoSetServo,
                    params: [1.0, 2.0, 3.0, 4.0],
                    x: 123,
                    y: -456,
                    z: 78.0,
                    frame: MavFrame::Mission,
                },
            },
        ];

        let text = write_wpl(&items);
        let parsed = parse_wpl(&text).expect("round trip should parse");

        assert_eq!(parsed, items);
    }

    #[test]
    fn frame_zero_waypoint_imports_as_typed_global_int() {
        let text = "QGC WPL 110\n\
                    0\t1\t0\t16\t0\t0\t0\t0\t47.4\t8.5\t120\t1\n";

        let items = parse_wpl(text).expect("valid file");
        assert_eq!(items.len(), 1);

        let item = &items[0];
        assert_eq!(item.frame, MavFrame::GlobalInt);
        assert!(matches!(item.command, MissionCommand::Waypoint { .. }));
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
        let items = parse_wpl("QGC WPL 110\n").expect("valid header");
        assert!(items.is_empty());
    }
}
