//! Vehicle state, decomposed into cohesive subsystems.
//!
//! [`Vehicle`] is a thin aggregator: each subsystem owns its slice of state and
//! its `observe_*` fold, and `handle_message` dispatches each decoded frame to
//! the right subsystem. Identity lives in [`identity`], attitude in
//! [`attitude`], position and its trail in [`position`], battery in [`battery`],
//! and status texts in [`status`].
//!
//! Scoping follows MAVLink's component model: vehicle-scoped flight and system
//! state (identity, attitude, position, battery, mission progress) is folded
//! only from the vehicle's authoritative autopilot component, so a peripheral
//! (gimbal, companion, camera) cannot overwrite the autopilot's state. State
//! that is genuinely per component (parameters, capabilities, names) is stored
//! per component, and `STATUSTEXT` is accepted from any component but
//! attributed to its source.

pub mod attitude;
pub mod battery;
pub mod identity;
pub mod mission;
pub mod position;
pub mod status;

pub use self::status::StatusText;

use self::{
    attitude::AttitudeState, battery::Battery, identity::Identity, mission::MissionProgress,
    position::PositionTrack, status::StatusLog,
};

use std::{collections::BTreeMap, time::Instant};

use mav_param::{Ident, Value};
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavAutopilot, MavProtocolCapability},
        messages,
    },
};

use crate::{
    domain::firmware::FlightMode,
    domain::mission::{MissionPlan, MissionSync},
    domain::parameter::{self, Parameters},
    foundation::{BoxError, MavlinkId, SystemId},
};

/// How recently a vehicle has been heard from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Liveness {
    #[default]
    Active,
    Stale,
    Lost,
}

/// A component's own state: its parameters and the capabilities that govern
/// how those parameters are encoded on the wire.
///
/// Parameters are per component: a gimbal or companion computer has its own set
/// under the same MAVLink system, decoded with its own
/// [`MavProtocolCapability`].
#[derive(Debug, Clone, Default)]
pub struct ComponentState {
    capabilities: Option<MavProtocolCapability>,
    pub model_name: Option<String>,
    pub vendor_name: Option<String>,
    parameters: Parameters,
}

impl ComponentState {
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    pub fn parameters_mut(&mut self) -> &mut Parameters {
        &mut self.parameters
    }

    pub fn capabilities(&self) -> Option<MavProtocolCapability> {
        self.capabilities
    }
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
    /// The editable flight plan.
    pub mission: MissionPlan,
    /// Whether the plan matches the vehicle, and under which id.
    pub mission_sync: MissionSync,
    /// The vehicle's mission execution progress.
    pub mission_progress: MissionProgress,
    /// Per-component state (parameters, capabilities, names), keyed by the
    /// MAVLink component id.
    components: BTreeMap<u8, ComponentState>,
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

    fn component(&self, component: u8) -> Option<&ComponentState> {
        self.components.get(&component)
    }

    fn component_mut(&mut self, component: u8) -> &mut ComponentState {
        self.components.entry(component).or_default()
    }

    /// The component's parameter set, if that component has been heard from.
    pub fn parameters(&self, component: u8) -> Option<&Parameters> {
        self.component(component).map(ComponentState::parameters)
    }

    /// The component's parameter set, created on demand.
    pub fn parameters_mut(&mut self, component: u8) -> &mut Parameters {
        self.component_mut(component).parameters_mut()
    }

    /// Fold an upload result onto the component's in-flight upload of `ident`.
    ///
    /// A result only updates parameter state the component already has: if the
    /// component was never heard from, the outcome is
    /// [`UploadOutcome::Ignored`] and nothing is touched. The result is only
    /// read; the caller keeps ownership for diagnostics.
    pub fn apply_parameter_upload(
        &mut self,
        component: u8,
        ident: Ident,
        requested: Value,
        result: &Result<Value, BoxError>,
    ) -> parameter::UploadOutcome {
        if self.parameters(component).is_none() {
            return parameter::UploadOutcome::Ignored;
        }

        self.parameters_mut(component)
            .apply_upload_result(&ident, requested, result)
    }

    pub fn capabilities(&self, component: u8) -> Option<MavProtocolCapability> {
        self.component(component)
            .and_then(ComponentState::capabilities)
    }

    /// Every component heard from, in ascending id order.
    pub fn component_ids(&self) -> Vec<u8> {
        self.components.keys().copied().collect()
    }

    /// The component resolved as the autopilot, if any.
    pub fn autopilot_component(&self) -> Option<u8> {
        self.autopilot
    }

    /// The MAVLink address to target this vehicle's autopilot transactions at.
    pub fn target(&self) -> MavlinkId {
        self.target_for(self.autopilot.unwrap_or(1))
    }

    /// The MAVLink address to target `component`'s transactions at.
    pub fn target_for(&self, component: u8) -> MavlinkId {
        MavlinkId {
            system: self.system.0,
            component,
        }
    }

    /// Whether frames from `component` may refresh the vehicle's liveness:
    /// any component until the autopilot resolves, then the authoritative
    /// autopilot's frames only.
    pub(crate) fn is_liveness_source(&self, component: u8) -> bool {
        self.autopilot
            .is_none_or(|autopilot| autopilot == component)
    }

    /// Whether `component` is the vehicle's authoritative (autopilot) source.
    ///
    /// Vehicle-scoped telemetry (attitude, position, battery, mission progress)
    /// is only folded from this component; before the autopilot is resolved the
    /// conventional component 1 is presumed.
    pub fn is_authoritative(&self, component: u8) -> bool {
        self.autopilot.unwrap_or(1) == component
    }

    /// The latest reported flight mode, resolved for the vehicle's firmware.
    pub fn flight_mode(&self) -> FlightMode {
        self.identity.flight_mode()
    }

    /// Whether the vehicle reports itself as armed, if it has sent a heartbeat.
    pub fn armed(&self) -> Option<bool> {
        self.identity.armed()
    }

    fn on_param_value(&mut self, component: u8, message: &messages::ParamValue) {
        let cap = self.capabilities(component).unwrap_or_default();

        let maybe_value = parameter::mavio_into_value(cap, message.param_value, message.param_type);

        let Some(new_value) = maybe_value else {
            log::error!("Unsupported type of parameter: {:?}", message.param_type);
            return;
        };

        let Ok(ident) = Ident::try_from(&message.param_id) else {
            log::error!("Invalid parameter identifier: {:?}", message.param_id);
            return;
        };

        let parameters = self.parameters_mut(component);

        // Upload results are handled by `parameter::Message::ValueUploaded`: a
        // reported value only refreshes the stored one, never the edit state.
        parameters.observe_value(ident, new_value);

        if message.param_index == u16::MAX {
            return;
        }

        // Keep track of how many we expect
        parameters.note_loaded_index(message.param_index, message.param_count);
    }

    pub fn pretty_name(&self) -> String {
        let state = self.component(self.autopilot.unwrap_or(1));

        format!(
            "{} {} (mav {})",
            state
                .and_then(|state| state.vendor_name.as_deref())
                .unwrap_or("Unknown"),
            state
                .and_then(|state| state.model_name.as_deref())
                .unwrap_or("Unknown"),
            self.system.0
        )
    }

    pub fn handle_message(&mut self, time: Instant, component: u8, message: DefaultDialect) {
        match &message {
            DefaultDialect::Heartbeat(message) => {
                // Any heartbeat marks the component as present.
                self.component_mut(component);

                if message.autopilot != MavAutopilot::Invalid {
                    // The conventional autopilot is component 1, and it takes
                    // precedence: another component may claim the role only
                    // while no autopilot is known yet.
                    if component == 1 || self.autopilot.is_none() {
                        self.autopilot = Some(component);
                    }
                }

                // Only the autopilot's heartbeat describes the vehicle as a
                // whole; a peripheral's must not overwrite type, mode or armed.
                if self.autopilot == Some(component) {
                    self.identity.observe_heartbeat(self.system, message);
                }
            }
            DefaultDialect::AutopilotVersion(message) => {
                let system = self.system;
                let state = self.component_mut(component);
                if state.capabilities != Some(message.capabilities) {
                    log::debug!(
                        "Received capabilities for {system:?} component {component}: {:b}",
                        message.capabilities
                    );
                    state.capabilities = Some(message.capabilities);
                }
            }
            DefaultDialect::ComponentInformationBasic(message) => {
                let system = self.system;
                let state = self.component_mut(component);

                if let Some(model_name) = string_from_cstring(&message.model_name)
                    && !model_name.is_empty()
                {
                    state.model_name = Some(model_name)
                }
                if let Some(vendor_name) = string_from_cstring(&message.vendor_name)
                    && !vendor_name.is_empty()
                {
                    state.vendor_name = Some(vendor_name)
                }

                if state.capabilities != Some(message.capabilities) {
                    log::debug!(
                        "Received capabilities for {system:?} component {component}: {:b}",
                        message.capabilities
                    );
                    state.capabilities = Some(message.capabilities);
                }
            }
            DefaultDialect::ParamValue(message) => {
                self.on_param_value(component, message);
            }
            DefaultDialect::SysStatus(message) => {
                if self.is_authoritative(component) {
                    self.battery.observe_sys_status(message);
                }
            }
            DefaultDialect::BatteryStatus(message) => {
                if self.is_authoritative(component) {
                    self.battery.observe_battery_status(message);
                }
            }
            DefaultDialect::Attitude(message) => {
                if self.is_authoritative(component) {
                    self.attitude.observe_attitude(message);
                }
            }
            DefaultDialect::AttitudeQuaternion(message) => {
                if self.is_authoritative(component) {
                    self.attitude.observe_attitude_quaternion(message);
                }
            }
            DefaultDialect::GlobalPositionInt(message) => {
                if self.is_authoritative(component) {
                    self.position.observe_global_position_int(message, time);
                }
            }
            DefaultDialect::GlobalPositionIntCov(message) => {
                if self.is_authoritative(component) {
                    self.position.observe_global_position_int_cov(message, time);
                }
            }
            DefaultDialect::MissionCurrent(message) => {
                if self.is_authoritative(component) {
                    self.mission_progress.observe_current(message);
                }
            }
            DefaultDialect::MissionItemReached(message) => {
                if self.is_authoritative(component) {
                    self.mission_progress.observe_reached(message.seq);
                }
            }
            DefaultDialect::Statustext(message) => {
                if let Err(error) = self.status.observe_statustext(component, message, time) {
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

/// Decode a fixed-size, NUL-terminated MAVLink string, if it is valid UTF-8.
fn string_from_cstring(cstring: &[u8]) -> Option<String> {
    let null = cstring
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(cstring.len());

    str::from_utf8(&cstring[..null]).map(str::to_string).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::enums::{MavAutopilot, MavParamType, MissionState};

    fn vehicle() -> Vehicle {
        Vehicle::new(SystemId(1))
    }

    fn param_value(name: &str, value: f32) -> DefaultDialect {
        let mut param_id = [0u8; 16];
        param_id[..name.len()].copy_from_slice(name.as_bytes());

        DefaultDialect::ParamValue(messages::ParamValue {
            param_id,
            param_value: value,
            param_type: MavParamType::Real32,
            param_count: 2,
            param_index: 0,
        })
    }

    fn statustext(text: &str) -> DefaultDialect {
        let mut bytes = [0u8; 50];
        bytes[..text.len()].copy_from_slice(text.as_bytes());

        DefaultDialect::Statustext(messages::Statustext {
            text: bytes,
            ..Default::default()
        })
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

        assert_eq!(vehicle.mission_progress.current_seq, Some(2));
        assert_eq!(vehicle.mission_progress.total, Some(8));
        assert_eq!(vehicle.mission_progress.state, MissionState::Active);
    }

    #[test]
    fn parameter_upload_result_for_an_unheard_component_is_ignored() {
        let mut vehicle = vehicle();

        let ident = Ident::from_str_truncated("PARAM_ONE");
        let outcome =
            vehicle.apply_parameter_upload(1, ident, Value::F32(2.0), &Ok(Value::F32(2.0)));

        assert_eq!(outcome, parameter::UploadOutcome::Ignored);
        assert!(vehicle.parameters(1).is_none(), "no state was created");
    }

    #[test]
    fn mission_item_reached_updates_current_seq() {
        let mut vehicle = vehicle();

        vehicle.handle_message(
            Instant::now(),
            1,
            DefaultDialect::MissionItemReached(messages::MissionItemReached { seq: 5 }),
        );

        assert_eq!(vehicle.mission_progress.current_seq, Some(5));
    }

    #[test]
    fn components_keep_separate_parameter_sets() {
        let mut vehicle = vehicle();
        let t = Instant::now();

        vehicle.handle_message(t, 1, param_value("PARAM_A", 1.0));
        vehicle.handle_message(t, 154, param_value("PARAM_B", 2.0));

        let autopilot = vehicle.parameters(1).expect("component 1 has parameters");
        assert!(
            autopilot
                .get(&Ident::from_str_truncated("PARAM_A"))
                .is_some()
        );
        assert!(
            autopilot
                .get(&Ident::from_str_truncated("PARAM_B"))
                .is_none()
        );

        let gimbal = vehicle
            .parameters(154)
            .expect("component 154 has parameters");
        assert!(gimbal.get(&Ident::from_str_truncated("PARAM_A")).is_none());
        assert!(gimbal.get(&Ident::from_str_truncated("PARAM_B")).is_some());
    }

    #[test]
    fn peripheral_telemetry_does_not_overwrite_vehicle_state() {
        let mut vehicle = vehicle();
        let t = Instant::now();

        // The autopilot (component 1) establishes authoritative state.
        vehicle.handle_message(
            t,
            1,
            DefaultDialect::Heartbeat(messages::Heartbeat {
                autopilot: MavAutopilot::Ardupilotmega,
                ..Default::default()
            }),
        );
        vehicle.handle_message(
            t,
            1,
            DefaultDialect::Attitude(messages::Attitude {
                roll: 1.0,
                ..Default::default()
            }),
        );
        vehicle.handle_message(
            t,
            1,
            DefaultDialect::MissionCurrent(messages::MissionCurrent {
                seq: 3,
                ..Default::default()
            }),
        );
        vehicle.handle_message(
            t,
            1,
            DefaultDialect::SysStatus(messages::SysStatus {
                voltage_battery: 12_000,
                ..Default::default()
            }),
        );

        let autopilot_roll = vehicle
            .attitude
            .current
            .as_ref()
            .unwrap()
            .attitude
            .euler_angles()
            .0;
        let autopilot_voltage = vehicle.battery.state.as_ref().unwrap().voltage;

        // A peripheral's frames must not change any vehicle-scoped state.
        vehicle.handle_message(
            t,
            154,
            DefaultDialect::Attitude(messages::Attitude {
                roll: 2.0,
                ..Default::default()
            }),
        );
        vehicle.handle_message(
            t,
            154,
            DefaultDialect::MissionCurrent(messages::MissionCurrent {
                seq: 9,
                ..Default::default()
            }),
        );
        vehicle.handle_message(
            t,
            154,
            DefaultDialect::SysStatus(messages::SysStatus {
                voltage_battery: 9_000,
                ..Default::default()
            }),
        );

        assert_eq!(vehicle.mission_progress.current_seq, Some(3));
        assert_eq!(
            vehicle.battery.state.as_ref().unwrap().voltage,
            autopilot_voltage
        );
        let roll = vehicle
            .attitude
            .current
            .as_ref()
            .unwrap()
            .attitude
            .euler_angles()
            .0;
        assert!((roll - autopilot_roll).abs() < 1e-9);
    }

    #[test]
    fn status_text_records_its_component() {
        let mut vehicle = vehicle();
        let t = Instant::now();

        vehicle.handle_message(t, 1, statustext("auto"));
        vehicle.handle_message(t, 154, statustext("gimbal"));

        let texts = &vehicle.status.texts;
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0].component, 1);
        assert_eq!(texts[0].text, "auto");
        assert_eq!(texts[1].component, 154);
        assert_eq!(texts[1].text, "gimbal");
    }
}
