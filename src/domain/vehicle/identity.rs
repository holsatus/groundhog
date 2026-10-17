//! Vehicle identity: type, autopilot, mode, and armed state from the autopilot's
//! heartbeat.

use mavio::default_dialect::{
    enums::{MavAutopilot, MavModeFlag, MavState, MavType},
    messages,
};

use super::class::VehicleClass;
use crate::domain::firmware::{self, Firmware, FlightMode};

#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub mav_type: Option<MavType>,
    pub autopilot: Option<MavAutopilot>,
    pub custom_mode: u32,
    pub base_mode: Option<MavModeFlag>,
    pub mav_state: Option<MavState>,
}

impl Identity {
    pub fn observe_heartbeat(&mut self, message: &messages::Heartbeat) {
        self.mav_type = Some(message.type_);
        self.mav_state = Some(message.system_status);
        self.autopilot = Some(message.autopilot);
        self.custom_mode = message.custom_mode;
        self.base_mode = Some(message.base_mode);
    }

    /// The firmware adapter for this vehicle's autopilot.
    pub fn firmware(&self) -> &'static dyn Firmware {
        firmware::for_autopilot(self.autopilot.unwrap_or(MavAutopilot::Generic))
    }

    pub fn class(&self) -> VehicleClass {
        self.mav_type.map(VehicleClass::of).unwrap_or_default()
    }

    /// The latest reported flight mode, if the firmware knows it.
    pub fn flight_mode(&self) -> Option<FlightMode> {
        self.firmware()
            .decode_mode(self.class(), self.base_mode?, self.custom_mode)
    }

    /// Whether the vehicle reports itself as armed, if it has sent a heartbeat.
    pub fn is_armed(&self) -> Option<bool> {
        self.base_mode
            .map(|mode| mode.intersects(MavModeFlag::SAFETY_ARMED))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_sets_armed_and_resolves_flight_mode() {
        let mut identity = Identity::default();

        let heartbeat = messages::Heartbeat {
            type_: MavType::Hexarotor,
            autopilot: MavAutopilot::Ardupilotmega,
            base_mode: MavModeFlag::SAFETY_ARMED | MavModeFlag::CUSTOM_MODE_ENABLED,
            // ArduPilot copter custom mode 5 is `Loiter`.
            custom_mode: 5,
            system_status: MavState::Active,
            mavlink_version: 3,
        };

        identity.observe_heartbeat(&heartbeat);

        assert_eq!(identity.is_armed(), Some(true));
        assert_eq!(identity.class(), VehicleClass::Copter);
        assert_eq!(identity.firmware().name(), "ArduPilot");
        assert_eq!(identity.flight_mode().map(|mode| mode.name), Some("Loiter"));
    }

    #[test]
    fn nothing_is_known_before_a_heartbeat() {
        let identity = Identity::default();
        assert_eq!(identity.is_armed(), None);
        assert_eq!(identity.flight_mode(), None);
    }
}
