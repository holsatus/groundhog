//! Vehicle identity: type, autopilot, mode, and armed state from the autopilot's
//! heartbeat.

use mavio::default_dialect::{
    enums::{MavAutopilot, MavModeFlag, MavState, MavType},
    messages,
};

use crate::{
    domain::firmware::{self, FlightMode},
    foundation::SystemId,
};

#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub mav_type: Option<MavType>,
    pub autopilot: Option<MavAutopilot>,
    pub custom_mode: u32,
    pub base_mode: Option<MavModeFlag>,
    pub mav_state: Option<MavState>,
    /// The most recent `HEARTBEAT`, kept for display.
    pub last_heartbeat: Option<messages::Heartbeat>,
}

impl Identity {
    pub fn observe_heartbeat(&mut self, system: SystemId, message: &messages::Heartbeat) {
        self.last_heartbeat = Some(message.clone());

        if self.mav_type != Some(message.type_) {
            log::debug!(
                "Received new MAV type for {:?}: {:?}",
                system,
                message.type_
            );
            self.mav_type = Some(message.type_);
        }

        if self.mav_state != Some(message.system_status) {
            log::debug!(
                "Received new MAV state for {:?}: {:?}",
                system,
                message.system_status
            );
            self.mav_state = Some(message.system_status);
        }

        self.autopilot = Some(message.autopilot);
        self.custom_mode = message.custom_mode;
        self.base_mode = Some(message.base_mode);
    }

    /// The latest reported flight mode, resolved for the vehicle's firmware.
    pub fn flight_mode(&self) -> FlightMode {
        match (self.autopilot, self.mav_type) {
            (Some(autopilot), Some(mav_type)) => {
                firmware::flight_mode(autopilot, mav_type, self.custom_mode)
            }
            _ => FlightMode::Unknown,
        }
    }

    /// Whether the vehicle reports itself as armed, if it has sent a heartbeat.
    pub fn armed(&self) -> Option<bool> {
        self.base_mode
            .map(|mode| mode.intersects(MavModeFlag::SAFETY_ARMED))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SystemId = SystemId(1);

    #[test]
    fn heartbeat_sets_armed_and_resolves_flight_mode() {
        let mut identity = Identity::default();

        let heartbeat = messages::Heartbeat {
            type_: MavType::Quadrotor,
            autopilot: MavAutopilot::Ardupilotmega,
            base_mode: MavModeFlag::SAFETY_ARMED,
            // ArduPilot copter custom mode 5 is `Loiter`.
            custom_mode: 5,
            system_status: MavState::Active,
            mavlink_version: 3,
        };

        identity.observe_heartbeat(A, &heartbeat);

        assert_eq!(identity.armed(), Some(true));
        assert_eq!(
            identity.flight_mode(),
            FlightMode::ArduPilotCopter(firmware::ardupilot::flight_mode::Copter::Loiter)
        );
    }
}
