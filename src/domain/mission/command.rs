//! Typed mission commands and their lossless MAVLink wire form.
//!
//! A [`MissionItem`] pairs a [`MissionCommand`] with the frame and autocontinue
//! flag that travel alongside it on the wire. Commands are decoded to a typed
//! variant only when both the command id and the frame are recognised; anything
//! else is kept as [`MissionCommand::Unknown`] so a downloaded mission
//! round-trips byte-for-byte.
//!
//! For a typed (non-`Unknown`) item the encoded frame always equals
//! [`MissionItem::frame`]; non-positional commands decode regardless of frame so
//! that missions from other planners (which often tag `DO_*`/`CONDITION_*` items
//! with `Global` or `GlobalRelativeAlt`) keep their typed meaning. Positional
//! commands are stricter: they decode only for the global-INT frames, and a
//! positional command in any other frame is kept as [`MissionCommand::Unknown`].

use mavio::default_dialect::{
    enums::{MavCmd, MavFrame, MavMissionType},
    messages::MissionItemInt,
};

/// Scale between WGS-84 degrees and the integer coordinates of `MISSION_ITEM_INT`.
const LATLON_SCALE: f64 = 1e-7;

/// A WGS-84 position with altitude, in degrees and metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f32,
}

/// Turn direction for CONDITION_YAW.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnDirection {
    Clockwise,
    CounterClockwise,
}

/// Speed reference for DO_CHANGE_SPEED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedType {
    Airspeed,
    Groundspeed,
}

/// A mission command typed by purpose, carrying semantic values rather than raw
/// parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum MissionCommand {
    Waypoint {
        pos: Position,
        hold_s: f32,
        accept_radius_m: f32,
        pass_radius_m: f32,
        yaw_deg: Option<f32>,
    },
    LoiterUnlim {
        pos: Position,
        radius_m: f32,
        yaw_deg: Option<f32>,
    },
    LoiterTurns {
        pos: Position,
        turns: f32,
        radius_m: f32,
    },
    LoiterTime {
        pos: Position,
        time_s: f32,
        radius_m: f32,
    },
    ReturnToLaunch,
    Land {
        pos: Position,
        abort_alt_m: f32,
        precision_land_mode: u8,
        yaw_deg: Option<f32>,
    },
    Takeoff {
        pos: Position,
        pitch_deg: f32,
        yaw_deg: Option<f32>,
    },
    VtolTakeoff {
        pos: Position,
        yaw_deg: Option<f32>,
    },
    NavDelay {
        seconds: f32,
    },
    ConditionDelay {
        seconds: f32,
    },
    ConditionDistance {
        distance_m: f32,
    },
    ConditionYaw {
        angle_deg: f32,
        rate_deg_s: f32,
        direction: TurnDirection,
        relative: bool,
    },
    ChangeSpeed {
        speed_type: SpeedType,
        speed_m_s: f32,
        throttle_pct: f32,
    },
    Jump {
        target_seq: u16,
        repeat: u16,
    },
    SetRoiLocation {
        pos: Position,
    },
    /// Any command outside the supported set, or a supported command in an
    /// unexpected frame — kept raw so a downloaded mission round-trips exactly.
    Unknown {
        command: MavCmd,
        params: [f32; 4],
        x: i32,
        y: i32,
        z: f32,
        frame: MavFrame,
    },
}

/// A wire-faithful mission item: a command plus its frame and autocontinue flag.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionItem {
    pub frame: MavFrame,
    pub autocontinue: bool,
    pub command: MissionCommand,
}

/// The wire fields shared by every encoded command.
struct Wire {
    command: MavCmd,
    frame: MavFrame,
    params: [f32; 4],
    x: i32,
    y: i32,
    z: f32,
}

impl MissionItem {
    /// Decode a wire item, preserving anything the typed model does not cover.
    pub fn from_wire(msg: &MissionItemInt) -> MissionItem {
        MissionItem {
            frame: msg.frame,
            autocontinue: msg.autocontinue != 0,
            command: decode_command(msg),
        }
    }

    /// Encode to the wire. `seq` and `mission_type` are supplied by the caller
    /// because they describe the item's place in a mission, not the command.
    pub fn to_wire(&self, seq: u16, mission_type: MavMissionType) -> MissionItemInt {
        let wire = self.wire();

        MissionItemInt {
            target_system: 0,
            target_component: 0,
            seq,
            frame: wire.frame,
            command: wire.command,
            current: 0,
            autocontinue: u8::from(self.autocontinue),
            param1: wire.params[0],
            param2: wire.params[1],
            param3: wire.params[2],
            param4: wire.params[3],
            x: wire.x,
            y: wire.y,
            z: wire.z,
            mission_type,
        }
    }

    fn wire(&self) -> Wire {
        let (x, y, z) = match self.command.position() {
            Some(pos) => encode_position(&pos),
            None => (0, 0, 0.0),
        };

        let params = match &self.command {
            MissionCommand::Waypoint {
                hold_s,
                accept_radius_m,
                pass_radius_m,
                yaw_deg,
                ..
            } => [
                *hold_s,
                *accept_radius_m,
                *pass_radius_m,
                encode_yaw(*yaw_deg),
            ],
            MissionCommand::LoiterUnlim {
                radius_m, yaw_deg, ..
            } => [0.0, 0.0, *radius_m, encode_yaw(*yaw_deg)],
            MissionCommand::LoiterTurns {
                turns, radius_m, ..
            } => [*turns, 0.0, *radius_m, 0.0],
            MissionCommand::LoiterTime {
                time_s, radius_m, ..
            } => [*time_s, 0.0, *radius_m, 0.0],
            MissionCommand::Land {
                abort_alt_m,
                precision_land_mode,
                yaw_deg,
                ..
            } => [
                *abort_alt_m,
                *precision_land_mode as f32,
                0.0,
                encode_yaw(*yaw_deg),
            ],
            MissionCommand::Takeoff {
                pitch_deg, yaw_deg, ..
            } => [*pitch_deg, 0.0, 0.0, encode_yaw(*yaw_deg)],
            MissionCommand::VtolTakeoff { yaw_deg, .. } => [0.0, 0.0, 0.0, encode_yaw(*yaw_deg)],
            MissionCommand::NavDelay { seconds } | MissionCommand::ConditionDelay { seconds } => {
                [*seconds, 0.0, 0.0, 0.0]
            }
            MissionCommand::ConditionDistance { distance_m } => [*distance_m, 0.0, 0.0, 0.0],
            MissionCommand::ConditionYaw {
                angle_deg,
                rate_deg_s,
                direction,
                relative,
            } => [
                *angle_deg,
                *rate_deg_s,
                encode_direction(*direction),
                f32::from(*relative as u8),
            ],
            MissionCommand::ChangeSpeed {
                speed_type,
                speed_m_s,
                throttle_pct,
            } => [
                encode_speed_type(*speed_type),
                *speed_m_s,
                *throttle_pct,
                0.0,
            ],
            MissionCommand::Jump { target_seq, repeat } => {
                [*target_seq as f32, *repeat as f32, 0.0, 0.0]
            }
            MissionCommand::Unknown { params, .. } => *params,
            _ => [0.0; 4],
        };

        let command = self.command.wire_command();
        // Invariant: for a typed (non-`Unknown`) item the encoded frame is
        // exactly `MissionItem::frame`, preserved verbatim in both directions.
        // Only `Unknown` carries its frame internally, so it wins here.
        let frame = match &self.command {
            MissionCommand::Unknown { frame, .. } => *frame,
            _ => self.frame,
        };

        let (x, y, z) = match &self.command {
            MissionCommand::Unknown { x, y, z, .. } => (*x, *y, *z),
            _ => (x, y, z),
        };

        Wire {
            command,
            frame,
            params,
            x,
            y,
            z,
        }
    }
}

impl MissionCommand {
    fn wire_command(&self) -> MavCmd {
        match self {
            MissionCommand::Waypoint { .. } => MavCmd::NavWaypoint,
            MissionCommand::LoiterUnlim { .. } => MavCmd::NavLoiterUnlim,
            MissionCommand::LoiterTurns { .. } => MavCmd::NavLoiterTurns,
            MissionCommand::LoiterTime { .. } => MavCmd::NavLoiterTime,
            MissionCommand::ReturnToLaunch => MavCmd::NavReturnToLaunch,
            MissionCommand::Land { .. } => MavCmd::NavLand,
            MissionCommand::Takeoff { .. } => MavCmd::NavTakeoff,
            MissionCommand::VtolTakeoff { .. } => MavCmd::NavVtolTakeoff,
            MissionCommand::NavDelay { .. } => MavCmd::NavDelay,
            MissionCommand::ConditionDelay { .. } => MavCmd::ConditionDelay,
            MissionCommand::ConditionDistance { .. } => MavCmd::ConditionDistance,
            MissionCommand::ConditionYaw { .. } => MavCmd::ConditionYaw,
            MissionCommand::ChangeSpeed { .. } => MavCmd::DoChangeSpeed,
            MissionCommand::Jump { .. } => MavCmd::DoJump,
            MissionCommand::SetRoiLocation { .. } => MavCmd::DoSetRoiLocation,
            MissionCommand::Unknown { command, .. } => *command,
        }
    }

    /// The ordered fields the editor should present, empty for commands with no
    /// adjustable parameters.
    pub fn fields(&self) -> Vec<FieldSpec> {
        match self {
            MissionCommand::Waypoint {
                pos,
                hold_s,
                accept_radius_m,
                pass_radius_m,
                yaw_deg,
            } => {
                let mut fields = position_fields(pos);
                fields.push(spec(MissionField::Hold, "Hold", "s", *hold_s as f64));
                fields.push(spec(
                    MissionField::AcceptRadius,
                    "Accept radius",
                    "m",
                    *accept_radius_m as f64,
                ));
                fields.push(spec(
                    MissionField::PassRadius,
                    "Pass radius",
                    "m",
                    *pass_radius_m as f64,
                ));
                fields.push(spec(MissionField::Yaw, "Yaw", "°", yaw_value(*yaw_deg)));
                fields
            }
            MissionCommand::LoiterUnlim {
                pos,
                radius_m,
                yaw_deg,
            } => {
                let mut fields = position_fields(pos);
                fields.push(spec(MissionField::Radius, "Radius", "m", *radius_m as f64));
                fields.push(spec(MissionField::Yaw, "Yaw", "°", yaw_value(*yaw_deg)));
                fields
            }
            MissionCommand::LoiterTurns {
                pos,
                turns,
                radius_m,
            } => {
                let mut fields = position_fields(pos);
                fields.push(spec(MissionField::Turns, "Turns", "", *turns as f64));
                fields.push(spec(MissionField::Radius, "Radius", "m", *radius_m as f64));
                fields
            }
            MissionCommand::LoiterTime {
                pos,
                time_s,
                radius_m,
            } => {
                let mut fields = position_fields(pos);
                fields.push(spec(MissionField::Time, "Time", "s", *time_s as f64));
                fields.push(spec(MissionField::Radius, "Radius", "m", *radius_m as f64));
                fields
            }
            MissionCommand::ReturnToLaunch => Vec::new(),
            MissionCommand::Land {
                pos,
                abort_alt_m,
                precision_land_mode,
                yaw_deg,
            } => {
                let mut fields = position_fields(pos);
                fields.push(spec(
                    MissionField::AbortAltitude,
                    "Abort altitude",
                    "m",
                    *abort_alt_m as f64,
                ));
                fields.push(spec(
                    MissionField::PrecisionLandMode,
                    "Precision land mode",
                    "",
                    *precision_land_mode as f64,
                ));
                fields.push(spec(MissionField::Yaw, "Yaw", "°", yaw_value(*yaw_deg)));
                fields
            }
            MissionCommand::Takeoff {
                pos,
                pitch_deg,
                yaw_deg,
            } => {
                let mut fields = position_fields(pos);
                fields.push(spec(MissionField::Pitch, "Pitch", "°", *pitch_deg as f64));
                fields.push(spec(MissionField::Yaw, "Yaw", "°", yaw_value(*yaw_deg)));
                fields
            }
            MissionCommand::VtolTakeoff { pos, yaw_deg } => {
                let mut fields = position_fields(pos);
                fields.push(spec(MissionField::Yaw, "Yaw", "°", yaw_value(*yaw_deg)));
                fields
            }
            MissionCommand::NavDelay { seconds } | MissionCommand::ConditionDelay { seconds } => {
                vec![spec(MissionField::Seconds, "Seconds", "s", *seconds as f64)]
            }
            MissionCommand::ConditionDistance { distance_m } => {
                vec![spec(
                    MissionField::Distance,
                    "Distance",
                    "m",
                    *distance_m as f64,
                )]
            }
            MissionCommand::ConditionYaw {
                angle_deg,
                rate_deg_s,
                direction,
                relative,
            } => vec![
                spec(MissionField::Angle, "Angle", "°", *angle_deg as f64),
                spec(MissionField::Rate, "Rate", "°/s", *rate_deg_s as f64),
                spec(
                    MissionField::Direction,
                    "Direction",
                    "",
                    direction_value(*direction),
                ),
                spec(
                    MissionField::Relative,
                    "Relative",
                    "",
                    f64::from(*relative as u8),
                ),
            ],
            MissionCommand::ChangeSpeed {
                speed_type,
                speed_m_s,
                throttle_pct,
            } => vec![
                spec(
                    MissionField::SpeedType,
                    "Speed type",
                    "",
                    speed_type_value(*speed_type),
                ),
                spec(MissionField::Speed, "Speed", "m/s", *speed_m_s as f64),
                spec(
                    MissionField::Throttle,
                    "Throttle",
                    "%",
                    *throttle_pct as f64,
                ),
            ],
            MissionCommand::Jump { target_seq, repeat } => vec![
                spec(
                    MissionField::TargetSeq,
                    "Target seq",
                    "",
                    *target_seq as f64,
                ),
                spec(MissionField::Repeat, "Repeat", "", *repeat as f64),
            ],
            MissionCommand::SetRoiLocation { pos } => position_fields(pos),
            MissionCommand::Unknown { .. } => Vec::new(),
        }
    }

    /// Update a single field, a no-op when the field does not apply to this
    /// variant. A `NaN` yaw means "leave unchanged" and maps to `None`.
    pub fn set_field(&mut self, field: MissionField, value: f64) {
        match self {
            MissionCommand::Waypoint {
                pos,
                hold_s,
                accept_radius_m,
                pass_radius_m,
                yaw_deg,
            } => match field {
                MissionField::Latitude => pos.lat = value,
                MissionField::Longitude => pos.lon = value,
                MissionField::Altitude => pos.alt_m = value as f32,
                MissionField::Hold => *hold_s = value as f32,
                MissionField::AcceptRadius => *accept_radius_m = value as f32,
                MissionField::PassRadius => *pass_radius_m = value as f32,
                MissionField::Yaw => set_yaw(yaw_deg, value),
                _ => (),
            },
            MissionCommand::LoiterUnlim {
                pos,
                radius_m,
                yaw_deg,
            } => match field {
                MissionField::Latitude => pos.lat = value,
                MissionField::Longitude => pos.lon = value,
                MissionField::Altitude => pos.alt_m = value as f32,
                MissionField::Radius => *radius_m = value as f32,
                MissionField::Yaw => set_yaw(yaw_deg, value),
                _ => (),
            },
            MissionCommand::LoiterTurns {
                pos,
                turns,
                radius_m,
            } => match field {
                MissionField::Latitude => pos.lat = value,
                MissionField::Longitude => pos.lon = value,
                MissionField::Altitude => pos.alt_m = value as f32,
                MissionField::Turns => *turns = value as f32,
                MissionField::Radius => *radius_m = value as f32,
                _ => (),
            },
            MissionCommand::LoiterTime {
                pos,
                time_s,
                radius_m,
            } => match field {
                MissionField::Latitude => pos.lat = value,
                MissionField::Longitude => pos.lon = value,
                MissionField::Altitude => pos.alt_m = value as f32,
                MissionField::Time => *time_s = value as f32,
                MissionField::Radius => *radius_m = value as f32,
                _ => (),
            },
            MissionCommand::ReturnToLaunch => (),
            MissionCommand::Land {
                pos,
                abort_alt_m,
                precision_land_mode,
                yaw_deg,
            } => match field {
                MissionField::Latitude => pos.lat = value,
                MissionField::Longitude => pos.lon = value,
                MissionField::Altitude => pos.alt_m = value as f32,
                MissionField::AbortAltitude => *abort_alt_m = value as f32,
                MissionField::PrecisionLandMode => *precision_land_mode = value as u8,
                MissionField::Yaw => set_yaw(yaw_deg, value),
                _ => (),
            },
            MissionCommand::Takeoff {
                pos,
                pitch_deg,
                yaw_deg,
            } => match field {
                MissionField::Latitude => pos.lat = value,
                MissionField::Longitude => pos.lon = value,
                MissionField::Altitude => pos.alt_m = value as f32,
                MissionField::Pitch => *pitch_deg = value as f32,
                MissionField::Yaw => set_yaw(yaw_deg, value),
                _ => (),
            },
            MissionCommand::VtolTakeoff { pos, yaw_deg } => match field {
                MissionField::Latitude => pos.lat = value,
                MissionField::Longitude => pos.lon = value,
                MissionField::Altitude => pos.alt_m = value as f32,
                MissionField::Yaw => set_yaw(yaw_deg, value),
                _ => (),
            },
            MissionCommand::NavDelay { seconds } | MissionCommand::ConditionDelay { seconds } => {
                if field == MissionField::Seconds {
                    *seconds = value as f32;
                }
            }
            MissionCommand::ConditionDistance { distance_m } => {
                if field == MissionField::Distance {
                    *distance_m = value as f32;
                }
            }
            MissionCommand::ConditionYaw {
                angle_deg,
                rate_deg_s,
                direction,
                relative,
            } => match field {
                MissionField::Angle => *angle_deg = value as f32,
                MissionField::Rate => *rate_deg_s = value as f32,
                MissionField::Direction => {
                    *direction = if value < 0.0 {
                        TurnDirection::CounterClockwise
                    } else {
                        TurnDirection::Clockwise
                    };
                }
                MissionField::Relative => *relative = value != 0.0,
                _ => (),
            },
            MissionCommand::ChangeSpeed {
                speed_type,
                speed_m_s,
                throttle_pct,
            } => match field {
                MissionField::SpeedType => {
                    *speed_type = if value == 1.0 {
                        SpeedType::Groundspeed
                    } else {
                        SpeedType::Airspeed
                    };
                }
                MissionField::Speed => *speed_m_s = value as f32,
                MissionField::Throttle => *throttle_pct = value as f32,
                _ => (),
            },
            MissionCommand::Jump { target_seq, repeat } => match field {
                MissionField::TargetSeq => *target_seq = value as u16,
                MissionField::Repeat => *repeat = value as u16,
                _ => (),
            },
            MissionCommand::SetRoiLocation { pos } => match field {
                MissionField::Latitude => pos.lat = value,
                MissionField::Longitude => pos.lon = value,
                MissionField::Altitude => pos.alt_m = value as f32,
                _ => (),
            },
            MissionCommand::Unknown { .. } => (),
        }
    }
}

/// The kind of a [`MissionCommand`], without its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionCommandKind {
    Waypoint,
    LoiterUnlim,
    LoiterTurns,
    LoiterTime,
    ReturnToLaunch,
    Land,
    Takeoff,
    VtolTakeoff,
    NavDelay,
    ConditionDelay,
    ConditionDistance,
    ConditionYaw,
    ChangeSpeed,
    Jump,
    SetRoiLocation,
}

impl MissionCommandKind {
    /// Every supported kind, in a stable display order.
    pub const ALL: &'static [Self] = &[
        Self::Waypoint,
        Self::LoiterUnlim,
        Self::LoiterTurns,
        Self::LoiterTime,
        Self::ReturnToLaunch,
        Self::Land,
        Self::Takeoff,
        Self::VtolTakeoff,
        Self::NavDelay,
        Self::ConditionDelay,
        Self::ConditionDistance,
        Self::ConditionYaw,
        Self::ChangeSpeed,
        Self::Jump,
        Self::SetRoiLocation,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Waypoint => "Waypoint",
            Self::LoiterUnlim => "LoiterUnlim",
            Self::LoiterTurns => "LoiterTurns",
            Self::LoiterTime => "LoiterTime",
            Self::ReturnToLaunch => "ReturnToLaunch",
            Self::Land => "Land",
            Self::Takeoff => "Takeoff",
            Self::VtolTakeoff => "VtolTakeoff",
            Self::NavDelay => "NavDelay",
            Self::ConditionDelay => "ConditionDelay",
            Self::ConditionDistance => "ConditionDistance",
            Self::ConditionYaw => "ConditionYaw",
            Self::ChangeSpeed => "ChangeSpeed",
            Self::Jump => "Jump",
            Self::SetRoiLocation => "SetRoiLocation",
        }
    }
}

impl MissionCommand {
    /// The command's kind, or `None` for [`MissionCommand::Unknown`].
    pub fn kind(&self) -> Option<MissionCommandKind> {
        let kind = match self {
            MissionCommand::Waypoint { .. } => MissionCommandKind::Waypoint,
            MissionCommand::LoiterUnlim { .. } => MissionCommandKind::LoiterUnlim,
            MissionCommand::LoiterTurns { .. } => MissionCommandKind::LoiterTurns,
            MissionCommand::LoiterTime { .. } => MissionCommandKind::LoiterTime,
            MissionCommand::ReturnToLaunch => MissionCommandKind::ReturnToLaunch,
            MissionCommand::Land { .. } => MissionCommandKind::Land,
            MissionCommand::Takeoff { .. } => MissionCommandKind::Takeoff,
            MissionCommand::VtolTakeoff { .. } => MissionCommandKind::VtolTakeoff,
            MissionCommand::NavDelay { .. } => MissionCommandKind::NavDelay,
            MissionCommand::ConditionDelay { .. } => MissionCommandKind::ConditionDelay,
            MissionCommand::ConditionDistance { .. } => MissionCommandKind::ConditionDistance,
            MissionCommand::ConditionYaw { .. } => MissionCommandKind::ConditionYaw,
            MissionCommand::ChangeSpeed { .. } => MissionCommandKind::ChangeSpeed,
            MissionCommand::Jump { .. } => MissionCommandKind::Jump,
            MissionCommand::SetRoiLocation { .. } => MissionCommandKind::SetRoiLocation,
            MissionCommand::Unknown { .. } => return None,
        };
        Some(kind)
    }

    pub fn name(&self) -> &'static str {
        self.kind()
            .map(MissionCommandKind::name)
            .unwrap_or("Unknown")
    }

    /// Whether the command carries a WGS-84 position.
    pub fn is_positional(&self) -> bool {
        self.position().is_some()
    }

    pub fn position(&self) -> Option<Position> {
        match self {
            MissionCommand::Waypoint { pos, .. }
            | MissionCommand::LoiterUnlim { pos, .. }
            | MissionCommand::LoiterTurns { pos, .. }
            | MissionCommand::LoiterTime { pos, .. }
            | MissionCommand::Land { pos, .. }
            | MissionCommand::Takeoff { pos, .. }
            | MissionCommand::VtolTakeoff { pos, .. }
            | MissionCommand::SetRoiLocation { pos } => Some(*pos),
            _ => None,
        }
    }

    /// A new command of `kind`, positioned at `pos`.
    pub fn default_for(kind: MissionCommandKind, pos: Position) -> MissionCommand {
        match kind {
            MissionCommandKind::Waypoint => MissionCommand::Waypoint {
                pos,
                hold_s: 0.0,
                accept_radius_m: 0.0,
                pass_radius_m: 0.0,
                yaw_deg: None,
            },
            MissionCommandKind::LoiterUnlim => MissionCommand::LoiterUnlim {
                pos,
                radius_m: 0.0,
                yaw_deg: None,
            },
            MissionCommandKind::LoiterTurns => MissionCommand::LoiterTurns {
                pos,
                turns: 1.0,
                radius_m: 0.0,
            },
            MissionCommandKind::LoiterTime => MissionCommand::LoiterTime {
                pos,
                time_s: 0.0,
                radius_m: 0.0,
            },
            MissionCommandKind::ReturnToLaunch => MissionCommand::ReturnToLaunch,
            MissionCommandKind::Land => MissionCommand::Land {
                pos,
                abort_alt_m: 0.0,
                precision_land_mode: 0,
                yaw_deg: None,
            },
            MissionCommandKind::Takeoff => MissionCommand::Takeoff {
                pos,
                pitch_deg: 0.0,
                yaw_deg: None,
            },
            MissionCommandKind::VtolTakeoff => MissionCommand::VtolTakeoff { pos, yaw_deg: None },
            MissionCommandKind::NavDelay => MissionCommand::NavDelay { seconds: 0.0 },
            MissionCommandKind::ConditionDelay => MissionCommand::ConditionDelay { seconds: 0.0 },
            MissionCommandKind::ConditionDistance => {
                MissionCommand::ConditionDistance { distance_m: 0.0 }
            }
            MissionCommandKind::ConditionYaw => MissionCommand::ConditionYaw {
                angle_deg: 0.0,
                rate_deg_s: 0.0,
                direction: TurnDirection::Clockwise,
                relative: false,
            },
            MissionCommandKind::ChangeSpeed => MissionCommand::ChangeSpeed {
                speed_type: SpeedType::Airspeed,
                speed_m_s: 0.0,
                throttle_pct: 0.0,
            },
            MissionCommandKind::Jump => MissionCommand::Jump {
                target_seq: 0,
                repeat: 0,
            },
            MissionCommandKind::SetRoiLocation => MissionCommand::SetRoiLocation { pos },
        }
    }
}

/// A single editable field, ready to be rendered generically by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MissionField {
    Latitude,
    Longitude,
    Altitude,
    Hold,
    AcceptRadius,
    PassRadius,
    Yaw,
    Radius,
    Turns,
    Time,
    Pitch,
    AbortAltitude,
    PrecisionLandMode,
    Seconds,
    Distance,
    Angle,
    Rate,
    Direction,
    Relative,
    SpeedType,
    Speed,
    Throttle,
    TargetSeq,
    Repeat,
}

pub struct FieldSpec {
    pub field: MissionField,
    pub label: &'static str,
    pub units: &'static str,
    pub value: f64,
}

fn spec(field: MissionField, label: &'static str, units: &'static str, value: f64) -> FieldSpec {
    FieldSpec {
        field,
        label,
        units,
        value,
    }
}

fn position_fields(pos: &Position) -> Vec<FieldSpec> {
    vec![
        spec(MissionField::Latitude, "Latitude", "°", pos.lat),
        spec(MissionField::Longitude, "Longitude", "°", pos.lon),
        spec(MissionField::Altitude, "Altitude", "m", pos.alt_m as f64),
    ]
}

fn decode_command(msg: &MissionItemInt) -> MissionCommand {
    let p = [msg.param1, msg.param2, msg.param3, msg.param4];

    match msg.command {
        MavCmd::NavWaypoint if is_global_int(msg.frame) => MissionCommand::Waypoint {
            pos: decode_position(msg),
            hold_s: p[0],
            accept_radius_m: p[1],
            pass_radius_m: p[2],
            yaw_deg: decode_yaw(p[3]),
        },
        MavCmd::NavLoiterUnlim if is_global_int(msg.frame) => MissionCommand::LoiterUnlim {
            pos: decode_position(msg),
            radius_m: p[2],
            yaw_deg: decode_yaw(p[3]),
        },
        MavCmd::NavLoiterTurns if is_global_int(msg.frame) => MissionCommand::LoiterTurns {
            pos: decode_position(msg),
            turns: p[0],
            radius_m: p[2],
        },
        MavCmd::NavLoiterTime if is_global_int(msg.frame) => MissionCommand::LoiterTime {
            pos: decode_position(msg),
            time_s: p[0],
            radius_m: p[2],
        },
        MavCmd::NavReturnToLaunch => MissionCommand::ReturnToLaunch,
        MavCmd::NavLand if is_global_int(msg.frame) => MissionCommand::Land {
            pos: decode_position(msg),
            abort_alt_m: p[0],
            precision_land_mode: p[1] as u8,
            yaw_deg: decode_yaw(p[3]),
        },
        MavCmd::NavTakeoff if is_global_int(msg.frame) => MissionCommand::Takeoff {
            pos: decode_position(msg),
            pitch_deg: p[0],
            yaw_deg: decode_yaw(p[3]),
        },
        MavCmd::NavVtolTakeoff if is_global_int(msg.frame) => MissionCommand::VtolTakeoff {
            pos: decode_position(msg),
            yaw_deg: decode_yaw(p[3]),
        },
        MavCmd::NavDelay => MissionCommand::NavDelay { seconds: p[0] },
        MavCmd::ConditionDelay => MissionCommand::ConditionDelay { seconds: p[0] },
        MavCmd::ConditionDistance => MissionCommand::ConditionDistance { distance_m: p[0] },
        MavCmd::ConditionYaw => MissionCommand::ConditionYaw {
            angle_deg: p[0],
            rate_deg_s: p[1],
            direction: if p[2] < 0.0 {
                TurnDirection::CounterClockwise
            } else {
                TurnDirection::Clockwise
            },
            relative: p[3] != 0.0,
        },
        MavCmd::DoChangeSpeed => MissionCommand::ChangeSpeed {
            speed_type: if p[0] == 1.0 {
                SpeedType::Groundspeed
            } else {
                SpeedType::Airspeed
            },
            speed_m_s: p[1],
            throttle_pct: p[2],
        },
        MavCmd::DoJump => MissionCommand::Jump {
            target_seq: p[0] as u16,
            repeat: p[1] as u16,
        },
        MavCmd::DoSetRoiLocation if is_global_int(msg.frame) => MissionCommand::SetRoiLocation {
            pos: decode_position(msg),
        },
        other => MissionCommand::Unknown {
            command: other,
            params: p,
            x: msg.x,
            y: msg.y,
            z: msg.z,
            frame: msg.frame,
        },
    }
}

fn decode_position(msg: &MissionItemInt) -> Position {
    Position {
        lat: decode_lat(msg.x),
        lon: decode_lat(msg.y),
        alt_m: msg.z,
    }
}

fn is_global_int(frame: MavFrame) -> bool {
    matches!(
        frame,
        MavFrame::GlobalInt | MavFrame::GlobalRelativeAltInt | MavFrame::GlobalTerrainAltInt
    )
}

/// Whether `command` positions the vehicle (its x/y are latitude/longitude).
pub fn is_positional_command(command: MavCmd) -> bool {
    matches!(
        command,
        MavCmd::NavWaypoint
            | MavCmd::NavLoiterUnlim
            | MavCmd::NavLoiterTurns
            | MavCmd::NavLoiterTime
            | MavCmd::NavLand
            | MavCmd::NavTakeoff
            | MavCmd::NavVtolTakeoff
            | MavCmd::DoSetRoiLocation
    )
}

fn decode_lat(scaled: i32) -> f64 {
    scaled as f64 * LATLON_SCALE
}

fn encode_position(pos: &Position) -> (i32, i32, f32) {
    (
        (pos.lat / LATLON_SCALE).round() as i32,
        (pos.lon / LATLON_SCALE).round() as i32,
        pos.alt_m,
    )
}

fn decode_yaw(param: f32) -> Option<f32> {
    if param.is_nan() { None } else { Some(param) }
}

fn encode_yaw(yaw: Option<f32>) -> f32 {
    yaw.unwrap_or(f32::NAN)
}

fn set_yaw(slot: &mut Option<f32>, value: f64) {
    *slot = if value.is_nan() {
        None
    } else {
        Some(value as f32)
    };
}

fn yaw_value(yaw: Option<f32>) -> f64 {
    yaw.map(f64::from).unwrap_or(f64::NAN)
}

fn encode_direction(direction: TurnDirection) -> f32 {
    match direction {
        TurnDirection::Clockwise => 1.0,
        TurnDirection::CounterClockwise => -1.0,
    }
}

fn direction_value(direction: TurnDirection) -> f64 {
    match direction {
        TurnDirection::Clockwise => 1.0,
        TurnDirection::CounterClockwise => -1.0,
    }
}

fn encode_speed_type(speed_type: SpeedType) -> f32 {
    match speed_type {
        SpeedType::Airspeed => 0.0,
        SpeedType::Groundspeed => 1.0,
    }
}

fn speed_type_value(speed_type: SpeedType) -> f64 {
    match speed_type {
        SpeedType::Airspeed => 0.0,
        SpeedType::Groundspeed => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(lat: f64, lon: f64, alt_m: f32) -> Position {
        Position { lat, lon, alt_m }
    }

    fn wire_item(
        frame: MavFrame,
        command: MavCmd,
        params: [f32; 4],
        x: i32,
        y: i32,
        z: f32,
    ) -> MissionItemInt {
        MissionItemInt {
            target_system: 0,
            target_component: 0,
            seq: 0,
            frame,
            command,
            current: 0,
            autocontinue: 1,
            param1: params[0],
            param2: params[1],
            param3: params[2],
            param4: params[3],
            x,
            y,
            z,
            mission_type: MavMissionType::Mission,
        }
    }

    fn sample_items() -> Vec<MissionItem> {
        let p = pos(47.4, 8.5, 512.0);

        vec![
            MissionItem {
                frame: MavFrame::GlobalInt,
                autocontinue: true,
                command: MissionCommand::Waypoint {
                    pos: p,
                    hold_s: 5.0,
                    accept_radius_m: 1.5,
                    pass_radius_m: 2.5,
                    yaw_deg: Some(90.0),
                },
            },
            MissionItem {
                frame: MavFrame::GlobalRelativeAltInt,
                autocontinue: false,
                command: MissionCommand::Waypoint {
                    pos: p,
                    hold_s: 0.0,
                    accept_radius_m: 3.0,
                    pass_radius_m: 0.0,
                    yaw_deg: None,
                },
            },
            MissionItem {
                frame: MavFrame::GlobalInt,
                autocontinue: true,
                command: MissionCommand::LoiterUnlim {
                    pos: p,
                    radius_m: 30.0,
                    yaw_deg: None,
                },
            },
            MissionItem {
                frame: MavFrame::GlobalTerrainAltInt,
                autocontinue: true,
                command: MissionCommand::LoiterTurns {
                    pos: p,
                    turns: 2.0,
                    radius_m: 45.0,
                },
            },
            MissionItem {
                frame: MavFrame::GlobalInt,
                autocontinue: false,
                command: MissionCommand::LoiterTime {
                    pos: p,
                    time_s: 12.0,
                    radius_m: 20.0,
                },
            },
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: true,
                command: MissionCommand::ReturnToLaunch,
            },
            MissionItem {
                frame: MavFrame::GlobalInt,
                autocontinue: false,
                command: MissionCommand::Land {
                    pos: p,
                    abort_alt_m: 15.0,
                    precision_land_mode: 2,
                    yaw_deg: Some(-45.0),
                },
            },
            MissionItem {
                frame: MavFrame::GlobalInt,
                autocontinue: true,
                command: MissionCommand::Takeoff {
                    pos: p,
                    pitch_deg: 10.0,
                    yaw_deg: None,
                },
            },
            MissionItem {
                frame: MavFrame::GlobalInt,
                autocontinue: true,
                command: MissionCommand::VtolTakeoff {
                    pos: p,
                    yaw_deg: Some(180.0),
                },
            },
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: true,
                command: MissionCommand::NavDelay { seconds: 3.5 },
            },
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: true,
                command: MissionCommand::ConditionDelay { seconds: 7.0 },
            },
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: true,
                command: MissionCommand::ConditionDistance { distance_m: 150.0 },
            },
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: true,
                command: MissionCommand::ConditionYaw {
                    angle_deg: 90.0,
                    rate_deg_s: 30.0,
                    direction: TurnDirection::CounterClockwise,
                    relative: true,
                },
            },
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: true,
                command: MissionCommand::ChangeSpeed {
                    speed_type: SpeedType::Groundspeed,
                    speed_m_s: 15.0,
                    throttle_pct: 80.0,
                },
            },
            MissionItem {
                frame: MavFrame::Mission,
                autocontinue: false,
                command: MissionCommand::Jump {
                    target_seq: 4,
                    repeat: 3,
                },
            },
            MissionItem {
                frame: MavFrame::GlobalInt,
                autocontinue: true,
                command: MissionCommand::SetRoiLocation { pos: p },
            },
            MissionItem {
                frame: MavFrame::Global,
                autocontinue: false,
                command: MissionCommand::Unknown {
                    command: MavCmd::NavWaypoint,
                    params: [1.0, 2.0, 3.0, 4.0],
                    x: 474_000_000,
                    y: 85_000_000,
                    z: 78.0,
                    frame: MavFrame::Global,
                },
            },
        ]
    }

    #[test]
    fn every_variant_round_trips() {
        for item in sample_items() {
            let wire = item.to_wire(7, MavMissionType::Mission);
            let back = MissionItem::from_wire(&wire);
            assert_eq!(item, back, "round trip failed for {}", item.command.name());
        }
    }

    #[test]
    fn unknown_round_trips_verbatim() {
        let wire = wire_item(
            MavFrame::Global,
            MavCmd::NavWaypoint,
            [11.0, 22.0, 33.0, 44.0],
            474_000_000,
            85_000_000,
            99.0,
        );

        let item = MissionItem::from_wire(&wire);

        assert_eq!(item.frame, MavFrame::Global);
        assert_eq!(
            item.command,
            MissionCommand::Unknown {
                command: MavCmd::NavWaypoint,
                params: [11.0, 22.0, 33.0, 44.0],
                x: 474_000_000,
                y: 85_000_000,
                z: 99.0,
                frame: MavFrame::Global,
            }
        );

        let encoded = item.to_wire(9, MavMissionType::Mission);
        assert_eq!(encoded.command, MavCmd::NavWaypoint);
        assert_eq!(encoded.frame, MavFrame::Global);
        assert_eq!(encoded.param1, 11.0);
        assert_eq!(encoded.param2, 22.0);
        assert_eq!(encoded.param3, 33.0);
        assert_eq!(encoded.param4, 44.0);
        assert_eq!(encoded.x, 474_000_000);
        assert_eq!(encoded.y, 85_000_000);
        assert_eq!(encoded.z, 99.0);
    }

    #[test]
    fn unsupported_command_decodes_to_unknown() {
        let wire = wire_item(
            MavFrame::Mission,
            MavCmd::DoSetServo,
            [1.0, 2.0, 3.0, 4.0],
            0,
            0,
            0.0,
        );

        let item = MissionItem::from_wire(&wire);

        assert!(matches!(
            item.command,
            MissionCommand::Unknown {
                command: MavCmd::DoSetServo,
                ..
            }
        ));
    }

    #[test]
    fn non_positional_command_decodes_in_any_frame() {
        let wire = wire_item(
            MavFrame::GlobalRelativeAlt,
            MavCmd::NavReturnToLaunch,
            [0.0; 4],
            0,
            0,
            0.0,
        );

        let item = MissionItem::from_wire(&wire);
        assert_eq!(item.command, MissionCommand::ReturnToLaunch);
    }

    #[test]
    fn condition_delay_travels_in_global_frame() {
        let wire = wire_item(
            MavFrame::Global,
            MavCmd::ConditionDelay,
            [7.0, 0.0, 0.0, 0.0],
            0,
            0,
            0.0,
        );

        let item = MissionItem::from_wire(&wire);
        assert_eq!(item.frame, MavFrame::Global);
        assert_eq!(
            item.command,
            MissionCommand::ConditionDelay { seconds: 7.0 }
        );

        let encoded = item.to_wire(3, MavMissionType::Mission);
        assert_eq!(encoded.frame, MavFrame::Global);
        assert_eq!(MissionItem::from_wire(&encoded), item);
    }

    #[test]
    fn lat_lon_scale_round_trips() {
        let wire = wire_item(
            MavFrame::GlobalInt,
            MavCmd::NavWaypoint,
            [0.0; 4],
            474_000_000,
            85_000_000,
            100.0,
        );

        let item = MissionItem::from_wire(&wire);
        let MissionCommand::Waypoint { pos: decoded, .. } = item.command else {
            panic!("expected a waypoint");
        };
        assert!((decoded.lat - 47.4).abs() < 1e-6, "lat was {}", decoded.lat);
        assert!((decoded.lon - 8.5).abs() < 1e-6, "lon was {}", decoded.lon);

        let item = MissionItem {
            frame: MavFrame::GlobalInt,
            autocontinue: true,
            command: MissionCommand::Waypoint {
                pos: pos(47.4, 8.5, 100.0),
                hold_s: 0.0,
                accept_radius_m: 0.0,
                pass_radius_m: 0.0,
                yaw_deg: None,
            },
        };

        let encoded = item.to_wire(0, MavMissionType::Mission);
        assert_eq!(encoded.x, 474_000_000);
        assert_eq!(encoded.y, 85_000_000);
        assert_eq!(encoded.z, 100.0);
    }

    #[test]
    fn set_field_mutates_only_the_named_field() {
        let mut command = MissionCommand::Waypoint {
            pos: pos(47.4, 8.5, 100.0),
            hold_s: 5.0,
            accept_radius_m: 1.0,
            pass_radius_m: 2.0,
            yaw_deg: None,
        };

        command.set_field(MissionField::Altitude, 250.0);
        command.set_field(MissionField::Hold, 9.0);

        let MissionCommand::Waypoint {
            pos,
            hold_s,
            accept_radius_m,
            pass_radius_m,
            yaw_deg,
        } = &command
        else {
            panic!("expected a waypoint");
        };

        assert_eq!(pos.alt_m, 250.0);
        assert_eq!(*hold_s, 9.0);
        assert_eq!(*accept_radius_m, 1.0);
        assert_eq!(*pass_radius_m, 2.0);
        assert_eq!(pos.lat, 47.4);
        assert_eq!(pos.lon, 8.5);
        assert_eq!(*yaw_deg, None);
    }

    #[test]
    fn set_field_yaw_round_trips_the_none_sentinel() {
        let mut command = MissionCommand::Takeoff {
            pos: pos(47.4, 8.5, 30.0),
            pitch_deg: 0.0,
            yaw_deg: Some(10.0),
        };

        command.set_field(MissionField::Yaw, 33.0);
        assert!(matches!(
            command,
            MissionCommand::Takeoff {
                yaw_deg: Some(v),
                ..
            } if v == 33.0
        ));

        command.set_field(MissionField::Yaw, f64::NAN);
        assert!(matches!(
            command,
            MissionCommand::Takeoff { yaw_deg: None, .. }
        ));
    }

    #[test]
    fn set_field_is_a_no_op_for_inapplicable_fields() {
        let mut command = MissionCommand::ReturnToLaunch;
        command.set_field(MissionField::Altitude, 100.0);
        assert_eq!(command, MissionCommand::ReturnToLaunch);
    }

    #[test]
    fn return_to_launch_has_no_fields() {
        assert!(MissionCommand::ReturnToLaunch.fields().is_empty());
        assert!(
            MissionCommand::Unknown {
                command: MavCmd::DoSetServo,
                params: [0.0; 4],
                x: 0,
                y: 0,
                z: 0.0,
                frame: MavFrame::Global,
            }
            .fields()
            .is_empty()
        );
    }

    #[test]
    fn default_for_yields_matching_kind_and_position() {
        let p = pos(47.4, 8.5, 30.0);

        for &kind in MissionCommandKind::ALL {
            let command = MissionCommand::default_for(kind, p);
            assert_eq!(command.kind(), Some(kind));
            assert_eq!(command.name(), kind.name());

            let positional = matches!(
                kind,
                MissionCommandKind::Waypoint
                    | MissionCommandKind::LoiterUnlim
                    | MissionCommandKind::LoiterTurns
                    | MissionCommandKind::LoiterTime
                    | MissionCommandKind::Land
                    | MissionCommandKind::Takeoff
                    | MissionCommandKind::VtolTakeoff
                    | MissionCommandKind::SetRoiLocation
            );
            assert_eq!(command.is_positional(), positional);
            assert_eq!(command.position(), positional.then_some(p));
        }
    }

    #[test]
    fn names_and_positionality() {
        let waypoint =
            MissionCommand::default_for(MissionCommandKind::Waypoint, pos(1.0, 2.0, 3.0));
        assert_eq!(waypoint.name(), "Waypoint");
        assert!(waypoint.is_positional());

        assert_eq!(MissionCommand::ReturnToLaunch.name(), "ReturnToLaunch");
        assert!(!MissionCommand::ReturnToLaunch.is_positional());
        assert_eq!(MissionCommand::ReturnToLaunch.position(), None);

        let unknown = MissionCommand::Unknown {
            command: MavCmd::DoSetServo,
            params: [0.0; 4],
            x: 0,
            y: 0,
            z: 0.0,
            frame: MavFrame::Global,
        };
        assert_eq!(unknown.name(), "Unknown");
        assert_eq!(unknown.kind(), None);
        assert!(!unknown.is_positional());
    }
}
