//! Vehicle identity: type, autopilot, mode, firmware capabilities, and names.

use mavio::default_dialect::{
    enums::{MavAutopilot, MavModeFlag, MavProtocolCapability, MavState, MavType},
    messages,
};

use crate::{
    domain::firmware::{self, FlightMode},
    foundation::MavlinkId,
};

#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub mav_type: Option<MavType>,
    pub autopilot: Option<MavAutopilot>,
    pub custom_mode: u32,
    pub base_mode: Option<MavModeFlag>,
    pub mav_state: Option<MavState>,
    pub capabilities: Option<MavProtocolCapability>,
    pub model_name: Option<String>,
    pub vendor_name: Option<String>,
}

impl Identity {
    pub fn observe_heartbeat(&mut self, mav_id: MavlinkId, message: &messages::Heartbeat) {
        if self.mav_type != Some(message.type_) {
            log::debug!(
                "Received new MAV type for {:?}: {:?}",
                mav_id,
                message.type_
            );
            self.mav_type = Some(message.type_);
        }

        if self.mav_state != Some(message.system_status) {
            log::debug!(
                "Received new MAV state for {:?}: {:?}",
                mav_id,
                message.system_status
            );
            self.mav_state = Some(message.system_status);
        }

        self.autopilot = Some(message.autopilot);
        self.custom_mode = message.custom_mode;
        self.base_mode = Some(message.base_mode);
    }

    pub fn observe_autopilot_version(
        &mut self,
        mav_id: MavlinkId,
        message: &messages::AutopilotVersion,
    ) {
        self.set_capability(mav_id, message.capabilities);
    }

    pub fn observe_component_information_basic(
        &mut self,
        mav_id: MavlinkId,
        message: &messages::ComponentInformationBasic,
    ) {
        if let Some(model_name) = string_from_cstring(&message.model_name)
            && !model_name.is_empty()
        {
            self.model_name = Some(model_name)
        }
        if let Some(vendor_name) = string_from_cstring(&message.vendor_name)
            && !vendor_name.is_empty()
        {
            self.vendor_name = Some(vendor_name)
        }
        self.set_capability(mav_id, message.capabilities);
    }

    pub fn set_capability(&mut self, mav_id: MavlinkId, capab: MavProtocolCapability) {
        if self.capabilities != Some(capab) {
            log::debug!("Received capabilities for {:?}: {:b}", mav_id, capab);
            self.capabilities = Some(capab);
        }
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

fn string_from_cstring(cstring: &[u8]) -> Option<String> {
    let null = cstring
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(cstring.len());

    str::from_utf8(&cstring[..null])
        .map(|str| str.to_string())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: MavlinkId = MavlinkId {
        system: 1,
        component: 1,
    };

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
