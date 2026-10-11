//! Vehicle state, decomposed into cohesive subsystems.
//!
//! [`Vehicle`] is a thin aggregator: each subsystem owns its slice of state and
//! its `observe_*` fold, and `handle_message` dispatches each decoded frame to
//! the right subsystem. Identity lives in [`identity`], attitude in
//! [`attitude`], position and its trail in [`position`], battery in [`battery`],
//! and status texts in [`status`].

pub mod attitude;
pub mod battery;
pub mod identity;
pub mod position;
pub mod status;

pub use self::status::StatusText;

use self::{
    attitude::AttitudeState, battery::Battery, identity::Identity, position::PositionTrack,
    status::StatusLog,
};

use std::time::Instant;

use mav_param::Ident;
use mavio::{
    DefaultDialect,
    default_dialect::{enums::MavAutopilot, messages},
};

use crate::{
    domain::firmware::FlightMode,
    domain::mission::Mission,
    domain::parameter::{self, Parameters},
    foundation::{MavlinkId, SystemId},
};

/// How recently a vehicle has been heard from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Liveness {
    #[default]
    Active,
    Stale,
    Lost,
}

#[derive(Debug, Clone, Default)]
pub struct Vehicle {
    /// The system this vehicle represents: its identity in the fleet.
    pub system: SystemId,
    /// The component resolved as this system's autopilot, if a heartbeat has
    /// named one. Used to address autopilot transactions via [`Self::target`].
    autopilot: Option<u8>,
    pub identity: Identity,
    pub attitude: AttitudeState,
    pub position: PositionTrack,
    pub battery: Battery,
    pub status: StatusLog,
    pub mission: Mission,
    parameters: Parameters,
    pub last_seen: Option<Instant>,
    pub liveness: Liveness,
}

impl Vehicle {
    pub fn new(system: SystemId) -> Self {
        Self {
            system,
            ..Vehicle::default()
        }
    }

    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    pub fn parameters_mut(&mut self) -> &mut Parameters {
        &mut self.parameters
    }

    /// The MAVLink address to target this vehicle's autopilot transactions at.
    pub fn target(&self) -> MavlinkId {
        MavlinkId {
            system: self.system.0,
            component: self.autopilot.unwrap_or(1),
        }
    }

    /// The latest reported flight mode, resolved for the vehicle's firmware.
    pub fn flight_mode(&self) -> FlightMode {
        self.identity.flight_mode()
    }

    /// Whether the vehicle reports itself as armed, if it has sent a heartbeat.
    pub fn armed(&self) -> Option<bool> {
        self.identity.armed()
    }

    pub fn on_param_value(&mut self, message: &messages::ParamValue) {
        let cap = self.identity.capabilities.unwrap_or_default();

        let maybe_value = parameter::mavio_into_value(cap, message.param_value, message.param_type);

        let Some(new_value) = maybe_value else {
            log::error!("Unsupported type of parameter: {:?}", message.param_type);
            return;
        };

        let Ok(ident) = Ident::try_from(&message.param_id) else {
            log::error!("Invalid parameter identifier: {:?}", message.param_id);
            return;
        };

        // Upload results are handled by `parameter::Message::ValueUploaded`: a
        // reported value only refreshes the stored one, never the edit state.
        self.parameters.observe_value(ident, new_value);

        if message.param_index == u16::MAX {
            return;
        }

        // Keep track of how many we expect
        self.parameters
            .note_loaded_index(message.param_index, message.param_count);
    }

    pub fn pretty_name(&self) -> String {
        format!(
            "{} {} (mav {})",
            self.identity.vendor_name.as_deref().unwrap_or("Unknown"),
            self.identity.model_name.as_deref().unwrap_or("Unknown"),
            self.system.0
        )
    }

    pub fn handle_message(&mut self, time: Instant, component: u8, message: DefaultDialect) {
        match &message {
            DefaultDialect::Heartbeat(message) => {
                if message.autopilot != MavAutopilot::Invalid {
                    self.autopilot = Some(component);
                }
                self.identity.observe_heartbeat(self.system, message);
            }
            DefaultDialect::AutopilotVersion(message) => {
                self.identity
                    .observe_autopilot_version(self.system, message);
            }
            DefaultDialect::ComponentInformationBasic(message) => {
                self.identity
                    .observe_component_information_basic(self.system, message);
            }
            DefaultDialect::ParamValue(message) => {
                self.on_param_value(message);
            }
            DefaultDialect::SysStatus(message) => {
                self.battery.observe_sys_status(message);
            }
            DefaultDialect::BatteryStatus(message) => {
                self.battery.observe_battery_status(message);
            }
            DefaultDialect::Attitude(message) => {
                self.attitude.observe_attitude(message);
            }
            DefaultDialect::AttitudeQuaternion(message) => {
                self.attitude.observe_attitude_quaternion(message);
            }
            DefaultDialect::GlobalPositionInt(message) => {
                self.position.observe_global_position_int(message, time)
            }
            DefaultDialect::GlobalPositionIntCov(message) => {
                self.position.observe_global_position_int_cov(message, time)
            }
            DefaultDialect::MissionCurrent(message) => self.mission.observe_current(message),
            DefaultDialect::MissionItemReached(message) => {
                self.mission.observe_reached(message.seq)
            }
            DefaultDialect::Statustext(message) => {
                if let Err(error) = self.status.observe_statustext(message, time) {
                    log::warn!(
                        "Statustext from '{}' not valid utf8: {}",
                        self.pretty_name(),
                        error
                    );
                }
            }
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::enums::MissionState;

    fn vehicle() -> Vehicle {
        Vehicle::new(SystemId(1))
    }

    #[test]
    fn mission_current_updates_progress() {
        let mut vehicle = vehicle();

        vehicle.handle_message(
            Instant::now(),
            1,
            DefaultDialect::MissionCurrent(messages::MissionCurrent {
                seq: 2,
                total: 8,
                mission_state: MissionState::Active,
                ..Default::default()
            }),
        );

        assert_eq!(vehicle.mission.progress.current_seq, Some(2));
        assert_eq!(vehicle.mission.progress.total, Some(8));
        assert_eq!(vehicle.mission.progress.state, MissionState::Active);
    }

    #[test]
    fn mission_item_reached_updates_current_seq() {
        let mut vehicle = vehicle();

        vehicle.handle_message(
            Instant::now(),
            1,
            DefaultDialect::MissionItemReached(messages::MissionItemReached { seq: 5 }),
        );

        assert_eq!(vehicle.mission.progress.current_seq, Some(5));
    }
}
