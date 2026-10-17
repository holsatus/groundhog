//! One mission item, stored exactly as it travels on the wire.
//!
//! Storing the wire fields (rather than a typed decoding of them) makes every
//! item round-trip byte for byte — including commands and parameter values the
//! editor does not understand — and keeps equality meaningful: two items are
//! equal when they would encode identically. Meaning is layered on top by the
//! [command table](super::command): positions, labelled fields, defaults.

use std::borrow::Cow;

use mavio::default_dialect::{
    enums::{MavCmd, MavFrame, MavMissionType},
    messages::MissionItemInt,
};

use super::command::{CommandSpec, Location, ParamKind, spec_of};
use crate::domain::geo::LATLON_SCALE;

/// A WGS-84 position with altitude, in degrees and metres. The altitude's
/// reference (home, sea level, terrain) is the item's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f32,
}

/// A mission item in wire form: command, frame, `param1..4`, `x`/`y`/`z`.
#[derive(Debug, Clone)]
pub struct MissionItem {
    pub command: MavCmd,
    pub frame: MavFrame,
    pub params: [f32; 4],
    /// Latitude in degrees × 1e7 for global frames, otherwise raw.
    pub x: i32,
    /// Longitude in degrees × 1e7 for global frames, otherwise raw.
    pub y: i32,
    /// Altitude in metres for global frames, otherwise raw.
    pub z: f32,
    pub autocontinue: bool,
}

/// Wire identity: floats compare by bit pattern, so a `NaN` yaw ("vehicle
/// default") equals itself and an item equals its own round trip.
impl PartialEq for MissionItem {
    fn eq(&self, other: &Self) -> bool {
        self.command == other.command
            && self.frame == other.frame
            && self
                .params
                .iter()
                .zip(other.params)
                .all(|(a, b)| a.to_bits() == b.to_bits())
            && self.x == other.x
            && self.y == other.y
            && self.z.to_bits() == other.z.to_bits()
            && self.autocontinue == other.autocontinue
    }
}

/// The frame freshly placed positional items use: altitude above home.
pub const DEFAULT_FRAME: MavFrame = MavFrame::GlobalRelativeAltInt;

/// The frame of items without a location.
pub const NON_POSITIONAL_FRAME: MavFrame = MavFrame::Mission;

/// One editable field of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    Latitude,
    Longitude,
    Altitude,
    /// `param1..4`, zero-based.
    Param(usize),
    /// Raw `x`, `y` and `z`, for commands without metadata.
    X,
    Y,
    Z,
}

/// How a field is edited.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldKind {
    Number,
    Integer,
    Bool,
    /// `NaN` (shown as empty) means "vehicle default".
    OptionalNumber,
    Choice(&'static [(f32, &'static str)]),
    /// A 1-based item number.
    ItemNumber,
}

/// A field as the editor presents it: label, units, widget and current value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldView {
    pub field: Field,
    pub label: &'static str,
    pub units: &'static str,
    pub kind: FieldKind,
    /// The value in display terms (item numbers are 1-based).
    pub value: f64,
}

impl MissionItem {
    /// A fresh item of `command` with its default parameters, placed at
    /// `position` if the command has a location.
    pub fn new(command: MavCmd, position: Option<Position>) -> MissionItem {
        let spec = spec_of(command);
        let positional = spec.is_some_and(CommandSpec::is_positional);

        let mut item = MissionItem {
            command,
            frame: if positional {
                DEFAULT_FRAME
            } else {
                NON_POSITIONAL_FRAME
            },
            params: spec.map(CommandSpec::default_params).unwrap_or_default(),
            x: 0,
            y: 0,
            z: 0.0,
            autocontinue: true,
        };

        if positional && let Some(position) = position {
            item.set_position(position);
        }

        item
    }

    /// A waypoint at `position`, as the editor places one.
    pub fn waypoint(position: Position) -> MissionItem {
        MissionItem::new(MavCmd::NavWaypoint, Some(position))
    }

    /// The item that occupies a firmware's home slot: a waypoint at the home
    /// position, altitude above mean sea level.
    pub fn home(position: Position) -> MissionItem {
        MissionItem {
            frame: MavFrame::GlobalInt,
            ..MissionItem::waypoint(position)
        }
    }

    /// The same item converted to `command`: the location and altitude carry
    /// over when both commands have one, parameters reset to the defaults.
    pub fn with_command(&self, command: MavCmd) -> MissionItem {
        let mut item = MissionItem::new(command, self.position());

        if item.spec().is_some_and(CommandSpec::is_positional) && self.position().is_none() {
            // No position to keep, but a freshly chosen altitude still is one.
            item.z = self.z;
        }

        item.autocontinue = self.autocontinue;
        item
    }

    /// The metadata of this item's command, if the editor knows it.
    pub fn spec(&self) -> Option<&'static CommandSpec> {
        spec_of(self.command)
    }

    /// The command's display name.
    pub fn name(&self) -> Cow<'static, str> {
        match self.spec() {
            Some(spec) => Cow::Borrowed(spec.name),
            None => Cow::Owned(format!("{:?} ({})", self.command, self.command as u16)),
        }
    }

    /// Whether x/y are latitude/longitude in this item's frame.
    pub fn has_global_frame(&self) -> bool {
        is_global_frame(self.frame)
    }

    /// Where the item is, for commands with a location in a global frame.
    /// `None` for commands without a location, for an optional location left
    /// at "current position" (zero lat/lon), and for local frames.
    pub fn position(&self) -> Option<Position> {
        let location = self.spec()?.location;

        if location == Location::None || !self.has_global_frame() {
            return None;
        }

        if location == Location::Optional && self.x == 0 && self.y == 0 {
            return None;
        }

        Some(Position {
            lat: self.x as f64 * LATLON_SCALE,
            lon: self.y as f64 * LATLON_SCALE,
            alt_m: self.z,
        })
    }

    /// Move the item to `position` (latitude, longitude and altitude).
    pub fn set_position(&mut self, position: Position) {
        self.set_lat_lon(position.lat, position.lon);
        self.z = position.alt_m;
    }

    /// Move the item horizontally, keeping its altitude.
    pub fn set_lat_lon(&mut self, lat: f64, lon: f64) {
        self.x = (lat / LATLON_SCALE).round() as i32;
        self.y = (lon / LATLON_SCALE).round() as i32;
    }

    /// The fields the editor should present, in order.
    pub fn fields(&self) -> Vec<FieldView> {
        let mut fields = Vec::with_capacity(7);

        let Some(spec) = self.spec() else {
            // Unknown command: every wire field, raw.
            for (index, label) in ["Param 1", "Param 2", "Param 3", "Param 4"]
                .into_iter()
                .enumerate()
            {
                fields.push(FieldView {
                    field: Field::Param(index),
                    label,
                    units: "",
                    kind: FieldKind::Number,
                    value: self.params[index] as f64,
                });
            }
            for (field, label, value) in [
                (Field::X, "X", self.x as f64),
                (Field::Y, "Y", self.y as f64),
                (Field::Z, "Z", self.z as f64),
            ] {
                fields.push(FieldView {
                    field,
                    label,
                    units: "",
                    kind: if field == Field::Z {
                        FieldKind::Number
                    } else {
                        FieldKind::Integer
                    },
                    value,
                });
            }
            return fields;
        };

        if spec.is_positional() {
            fields.push(FieldView {
                field: Field::Latitude,
                label: "Latitude",
                units: "°",
                kind: FieldKind::Number,
                value: self.x as f64 * LATLON_SCALE,
            });
            fields.push(FieldView {
                field: Field::Longitude,
                label: "Longitude",
                units: "°",
                kind: FieldKind::Number,
                value: self.y as f64 * LATLON_SCALE,
            });
            fields.push(FieldView {
                field: Field::Altitude,
                label: "Altitude",
                units: "m",
                kind: FieldKind::Number,
                value: self.z as f64,
            });
        }

        for (index, param) in spec.params.iter().enumerate() {
            let Some(param) = param else { continue };

            let (kind, value) = match param.kind {
                ParamKind::Number => (FieldKind::Number, self.params[index] as f64),
                ParamKind::Integer => (FieldKind::Integer, self.params[index] as f64),
                ParamKind::Bool => (FieldKind::Bool, self.params[index] as f64),
                ParamKind::OptionalNumber => (FieldKind::OptionalNumber, self.params[index] as f64),
                ParamKind::Choice(choices) => {
                    (FieldKind::Choice(choices), self.params[index] as f64)
                }
                ParamKind::ItemIndex => (FieldKind::ItemNumber, self.params[index] as f64 + 1.0),
            };

            fields.push(FieldView {
                field: Field::Param(index),
                label: param.label,
                units: param.units,
                kind,
                value,
            });
        }

        fields
    }

    /// Set one field from its display value (see [`FieldView::value`]).
    /// Returns whether the item changed.
    pub fn set_field(&mut self, field: Field, value: f64) -> bool {
        let before = self.clone();

        match field {
            Field::Latitude => self.x = (value / LATLON_SCALE).round() as i32,
            Field::Longitude => self.y = (value / LATLON_SCALE).round() as i32,
            Field::Altitude | Field::Z => self.z = value as f32,
            Field::X => self.x = value as i32,
            Field::Y => self.y = value as i32,
            Field::Param(index) if index < 4 => {
                let kind = self
                    .spec()
                    .and_then(|spec| spec.params[index])
                    .map(|param| param.kind);

                self.params[index] = match kind {
                    Some(ParamKind::ItemIndex) => (value - 1.0).max(0.0).round() as f32,
                    Some(ParamKind::Integer) => value.round() as f32,
                    Some(ParamKind::Bool) => f32::from(value != 0.0),
                    _ => value as f32,
                };
            }
            Field::Param(_) => {}
        }

        *self != before
    }

    /// Set one field from editor text. Blank text sets an optional number to
    /// "vehicle default" (`NaN`); other malformed text changes nothing.
    /// Returns whether the item changed.
    pub fn set_field_from_str(&mut self, field: Field, text: &str) -> bool {
        let trimmed = text.trim();

        let optional = self
            .fields()
            .iter()
            .any(|view| view.field == field && view.kind == FieldKind::OptionalNumber);

        let value = if (trimmed.is_empty() && optional) || trimmed.eq_ignore_ascii_case("nan") {
            f64::NAN
        } else {
            match trimmed.parse::<f64>() {
                Ok(value) => value,
                Err(_) => return false,
            }
        };

        self.set_field(field, value)
    }

    /// The `param1..4` slots that hold item indices (jump targets).
    pub(super) fn item_index_slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.spec().into_iter().flat_map(|spec| {
            spec.params
                .iter()
                .enumerate()
                .filter(|(_, param)| param.is_some_and(|param| param.kind == ParamKind::ItemIndex))
                .map(|(index, _)| index)
        })
    }

    /// Encode as a `MISSION_ITEM_INT` at wire sequence `seq`.
    pub fn to_wire(&self, seq: u16, mission_type: MavMissionType) -> MissionItemInt {
        MissionItemInt {
            target_system: 0,
            target_component: 0,
            seq,
            frame: self.frame,
            command: self.command,
            current: 0,
            autocontinue: u8::from(self.autocontinue),
            param1: self.params[0],
            param2: self.params[1],
            param3: self.params[2],
            param4: self.params[3],
            x: self.x,
            y: self.y,
            z: self.z,
            mission_type,
        }
    }

    /// Decode a `MISSION_ITEM_INT`, losslessly.
    pub fn from_wire(message: &MissionItemInt) -> MissionItem {
        MissionItem {
            command: message.command,
            frame: message.frame,
            params: [
                message.param1,
                message.param2,
                message.param3,
                message.param4,
            ],
            x: message.x,
            y: message.y,
            z: message.z,
            autocontinue: message.autocontinue != 0,
        }
    }
}

/// Whether x/y are latitude/longitude in `frame`.
///
/// Both the float (`Global`, ...) and `_INT` variants count: in a
/// `MISSION_ITEM_INT` the x/y are scaled integers regardless, and autopilots
/// (ArduPilot included) commonly report the float frames.
fn is_global_frame(frame: MavFrame) -> bool {
    matches!(
        frame,
        MavFrame::Global
            | MavFrame::GlobalRelativeAlt
            | MavFrame::GlobalInt
            | MavFrame::GlobalRelativeAltInt
            | MavFrame::GlobalTerrainAlt
            | MavFrame::GlobalTerrainAltInt
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(lat: f64, lon: f64, alt_m: f32) -> Position {
        Position { lat, lon, alt_m }
    }

    #[test]
    fn wire_round_trip_is_lossless_for_unknown_commands() {
        let wire = MissionItemInt {
            seq: 3,
            frame: MavFrame::Mission,
            command: MavCmd::DoMotorTest,
            autocontinue: 1,
            param1: 1.0,
            param2: f32::NAN,
            param3: -0.0,
            param4: 4.5,
            x: 123,
            y: -456,
            z: 7.5,
            mission_type: MavMissionType::Mission,
            ..Default::default()
        };

        let item = MissionItem::from_wire(&wire);
        let back = item.to_wire(3, MavMissionType::Mission);
        assert_eq!(MissionItem::from_wire(&back), item);
        assert_eq!(back.param3.to_bits(), (-0.0f32).to_bits());
        assert!(item.spec().is_none());
        assert_eq!(item.fields().len(), 7, "raw params plus x/y/z");
    }

    /// The old typed model rewrote `CONDITION_YAW` direction 0 as clockwise;
    /// wire storage keeps every value.
    #[test]
    fn known_commands_keep_values_outside_the_editor_choices() {
        let mut item = MissionItem::new(MavCmd::ConditionYaw, None);
        item.params[2] = 0.0;
        let wire = item.to_wire(0, MavMissionType::Mission);
        assert_eq!(MissionItem::from_wire(&wire).params[2], 0.0);
    }

    #[test]
    fn nan_yaw_items_equal_themselves() {
        let item = MissionItem::waypoint(at(47.0, 8.0, 50.0));
        assert!(item.params[3].is_nan());
        assert_eq!(item, item.clone());
    }

    #[test]
    fn positions_need_a_global_frame_and_a_location() {
        let mut item = MissionItem::waypoint(at(47.4, 8.5, 30.0));
        let position = item.position().unwrap();
        assert!((position.lat - 47.4).abs() < 1e-7);
        assert!((position.lon - 8.5).abs() < 1e-7);

        item.frame = MavFrame::LocalNed;
        assert!(item.position().is_none());

        assert!(MissionItem::new(MavCmd::DoJump, None).position().is_none());
    }

    #[test]
    fn an_optional_location_at_zero_means_current_position() {
        let takeoff = MissionItem::new(MavCmd::NavTakeoff, None);
        assert!(takeoff.position().is_none());

        let placed = MissionItem::new(MavCmd::NavTakeoff, Some(at(1.0, 2.0, 10.0)));
        assert!(placed.position().is_some());
    }

    #[test]
    fn jump_targets_are_edited_as_item_numbers() {
        let mut jump = MissionItem::new(MavCmd::DoJump, None);
        assert!(jump.set_field(Field::Param(0), 3.0), "item 3");
        assert_eq!(jump.params[0], 2.0, "is plan index 2");

        let view = jump
            .fields()
            .into_iter()
            .find(|view| view.field == Field::Param(0))
            .unwrap();
        assert_eq!(view.kind, FieldKind::ItemNumber);
        assert_eq!(view.value, 3.0);
    }

    #[test]
    fn blank_text_resets_an_optional_yaw() {
        let mut item = MissionItem::waypoint(at(0.0, 0.0, 50.0));
        assert!(item.set_field_from_str(Field::Param(3), "90"));
        assert_eq!(item.params[3], 90.0);
        assert!(item.set_field_from_str(Field::Param(3), ""));
        assert!(item.params[3].is_nan());
        assert!(!item.set_field_from_str(Field::Param(0), "abc"));
    }

    #[test]
    fn changing_the_command_keeps_the_location() {
        let waypoint = MissionItem::waypoint(at(47.0, 8.0, 80.0));
        let loiter = waypoint.with_command(MavCmd::NavLoiterUnlim);
        assert_eq!(loiter.position(), waypoint.position());

        let delay = waypoint.with_command(MavCmd::NavDelay);
        assert_eq!(delay.frame, NON_POSITIONAL_FRAME);
        assert!(delay.position().is_none());
    }
}
