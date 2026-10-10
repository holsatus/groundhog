//! Firmware-specific knowledge: how to turn a heartbeat's autopilot, vehicle
//! type, and `custom_mode` into a human-readable flight mode.

pub mod ardupilot;

use std::fmt;

use mavio::dialects::minimal::enums::{MavAutopilot, MavType};

use ardupilot::flight_mode;

/// A resolved flight mode, independent of which firmware reported it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlightMode {
    ArduPilotPlane(flight_mode::Plane),
    ArduPilotCopter(flight_mode::Copter),
    Unknown,
}

impl fmt::Display for FlightMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlightMode::ArduPilotPlane(mode) => write!(f, "{mode:?}"),
            FlightMode::ArduPilotCopter(mode) => write!(f, "{mode:?}"),
            FlightMode::Unknown => f.write_str("Unknown"),
        }
    }
}

/// Resolve the flight mode reported by a `HEARTBEAT`.
pub fn flight_mode(autopilot: MavAutopilot, mav_type: MavType, custom_mode: u32) -> FlightMode {
    use num_enum::FromPrimitive;

    match autopilot {
        MavAutopilot::Ardupilotmega => match mav_type {
            MavType::FixedWing | MavType::VtolTiltrotor => {
                FlightMode::ArduPilotPlane(flight_mode::Plane::from_primitive(custom_mode as u8))
            }
            MavType::Quadrotor | MavType::Helicopter => {
                FlightMode::ArduPilotCopter(flight_mode::Copter::from_primitive(custom_mode as u8))
            }
            _ => FlightMode::Unknown,
        },
        _ => FlightMode::Unknown,
    }
}
