//! Vehicle state, decomposed into cohesive subsystems.
//!
//! [`Vehicle`] is a thin aggregator: each subsystem owns its slice of state and
//! its `observe_*` fold, and `handle_message` dispatches each decoded frame to
//! the right subsystem.
//!
//! This is *vehicle* state only — what the vehicle reported. The operator's
//! drafts (mission plans, parameter edits) live with the features that edit
//! them, so the fleet can be owned by a runtime off the UI thread and handed to
//! the UI as cheap immutable snapshots. The heavier subsystems (trail, status
//! log, per-component parameters) sit behind [`Arc`]s: cloning a vehicle for a
//! snapshot shares them, and a mutation copies only the subsystem it touches.
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
pub mod camera;
pub mod class;
pub mod health;
pub mod home;
pub mod hud;
pub mod identity;
pub mod mission;
pub mod position;
pub mod status;

pub use self::status::StatusText;

use self::{
    attitude::AttitudeState,
    battery::Battery,
    camera::Cameras,
    class::VehicleClass,
    health::{Health, Sensor},
    home::Home,
    hud::Hud,
    identity::Identity,
    mission::VehicleMission,
    position::PositionTrack,
    status::StatusLog,
};

use std::{collections::BTreeMap, sync::Arc, time::Instant};

use mav_param::Ident;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavAutopilot, MavProtocolCapability, MavType, MissionState},
        messages,
    },
};

use crate::{
    MavlinkId, SystemId,
    domain::{
        calibration::{
            Calibration, CalibrationControls, CalibrationKind, CalibrationUpdate, Ended, Feedback,
            Session,
        },
        firmware::{self, Firmware, FlightMode, MissionLayout, ModeKind},
        geo::GeoPosition,
        mission::OnboardMission,
        parameter::{DownloadUpdate, Parameters},
    },
    wire::{self, param::ParamEncoding},
};

/// How far an autopilot's `time_boot_ms` must step backwards to mean a
/// reboot, rather than frames arriving out of order.
const BOOT_REWIND_MS: u32 = 2_000;

/// A winged vehicle's speed through the air.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Airspeed {
    /// Metres per second.
    pub speed: f32,
    /// Whether an airspeed sensor in use measured it; otherwise the
    /// autopilot estimated it from its ground speed and wind.
    pub measured: bool,
}

/// How recently a vehicle has been heard from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Liveness {
    #[default]
    Active,
    Stale,
    Lost,
}

/// A component's own state: its identity, parameters and the capabilities
/// that govern how those parameters are encoded on the wire.
///
/// Parameters are per component: a gimbal or companion computer has its own set
/// under the same MAVLink system, decoded with its own encoding.
#[derive(Debug, Clone, Default)]
pub struct ComponentState {
    capabilities: Option<MavProtocolCapability>,
    autopilot: Option<MavAutopilot>,
    /// What the component's heartbeat says it is.
    kind: Option<MavType>,
    pub model_name: Option<String>,
    pub vendor_name: Option<String>,
    parameters: Parameters,
}

impl ComponentState {
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    /// Whether the component's heartbeat says it is a camera.
    pub fn is_camera(&self) -> bool {
        self.kind == Some(MavType::Camera)
    }

    /// How this component encodes parameter values: as advertised in its
    /// capabilities, else as its firmware is known to. `None` when neither
    /// is known, in which case parameter writes must not be attempted.
    pub fn param_encoding(&self) -> Option<ParamEncoding> {
        self.capabilities
            .and_then(ParamEncoding::advertised)
            .or_else(|| {
                self.autopilot
                    .and_then(|autopilot| firmware::for_autopilot(autopilot).param_encoding())
            })
    }

    /// Re-decode parameters if what is known about the encoding changed.
    fn refresh_encoding(&mut self) {
        let encoding = self.param_encoding();
        self.parameters.set_encoding(encoding);
    }
}

/// Where a place is from the vehicle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offset {
    pub distance_m: f64,
    /// Degrees clockwise from north.
    pub bearing_deg: f64,
}

impl Offset {
    fn between(from: GeoPosition, to: GeoPosition) -> Self {
        Self {
            distance_m: from.distance_to(to),
            bearing_deg: from.bearing_to(to),
        }
    }
}

/// How far a vehicle has come through its mission.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MissionStatus {
    /// The plan index of the item being executed.
    pub current: usize,
    /// How many items the mission has, if known.
    pub count: Option<usize>,
    pub state: MissionState,
    /// Where the current item is from the vehicle, for items with a
    /// position.
    pub to_current: Option<Offset>,
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
    pub position: Arc<PositionTrack>,
    pub battery: Battery,
    pub health: Health,
    pub hud: Hud,
    pub status: Arc<StatusLog>,
    home_position: Option<Home>,
    pub mission: VehicleMission,
    /// Per-component state (parameters, capabilities, names), keyed by the
    /// MAVLink component id.
    components: BTreeMap<u8, Arc<ComponentState>>,
    /// The cameras its components have described.
    pub cameras: Cameras,
    /// The latest calibration run, until dismissed.
    calibration: Calibration,
    /// The last `time_boot_ms` the autopilot reported.
    last_boot_time: Option<u32>,
    /// How many times the autopilot's boot time has jumped backwards.
    boots: u32,
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

    /// `component`'s state to change, if `changes` says the change would
    /// alter it. A component's state holds its whole parameter set, so it is
    /// copied out of the last snapshot only for a real change, not for every
    /// heartbeat.
    fn component_mut_if(
        &mut self,
        component: u8,
        changes: impl FnOnce(&ComponentState) -> bool,
    ) -> Option<&mut ComponentState> {
        let state = self.components.entry(component).or_default();
        changes(state).then(|| Arc::make_mut(state))
    }

    /// The state of `component`, if it has been heard from.
    pub fn component_state(&self, component: u8) -> Option<&ComponentState> {
        self.components.get(&component).map(Arc::as_ref)
    }

    /// The component's parameter set, if that component has been heard from.
    pub fn parameters(&self, component: u8) -> Option<&Parameters> {
        self.component_state(component)
            .map(ComponentState::parameters)
    }

    /// How `component` encodes parameter values, if known.
    pub fn param_encoding(&self, component: u8) -> Option<ParamEncoding> {
        self.component_state(component)
            .and_then(ComponentState::param_encoding)
    }

    /// Every component heard from, in ascending id order.
    pub fn component_ids(&self) -> Vec<u8> {
        self.components.keys().copied().collect()
    }

    /// The component resolved as the autopilot, if any.
    pub fn autopilot_component(&self) -> Option<u8> {
        self.autopilot
    }

    /// The component whose parameters a panel shows by default: the resolved
    /// autopilot, or the conventional component 1 before one is known.
    pub fn default_parameter_component(&self) -> u8 {
        self.autopilot_component().unwrap_or(1)
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

    /// The firmware adapter for this vehicle.
    pub fn firmware(&self) -> &'static dyn Firmware {
        self.identity.firmware()
    }

    pub fn class(&self) -> VehicleClass {
        self.identity.class()
    }

    /// The latest reported flight mode, if the firmware knows it.
    pub fn flight_mode(&self) -> Option<FlightMode> {
        self.identity.flight_mode()
    }

    /// The modes an operator may select on this vehicle: its firmware's
    /// modes for its class, less those its autopilot's parameters rule out.
    pub fn modes(&self) -> impl Iterator<Item = FlightMode> + '_ {
        let firmware = self.firmware();
        let class = self.class();
        let parameters = self.parameters(self.default_parameter_component());
        firmware
            .modes(class)
            .iter()
            .filter(move |mode| firmware.offers(mode, class, parameters))
            .copied()
    }

    /// The selectable mode of `kind` (return, land, ...), if this vehicle
    /// has one.
    pub fn mode_of_kind(&self, kind: ModeKind) -> Option<FlightMode> {
        self.modes().find(|mode| mode.kind == kind)
    }

    /// Whether the vehicle reports itself as armed, if it has sent a heartbeat.
    /// Whether the vehicle may be told to reboot: only while it is known to
    /// be disarmed, as a reboot stops everything it is doing.
    pub fn can_reboot(&self) -> bool {
        self.armed() == Some(false)
    }

    /// The airspeed of a vehicle that flies on wings, and whether a sensor
    /// measured it. `None` for other vehicles: theirs is only ever estimated
    /// from the ground speed, which tells the operator nothing more.
    pub fn airspeed(&self) -> Option<Airspeed> {
        if !self.class().has_wings() {
            return None;
        }
        let speed = self.hud.airspeed?;
        let measured = self
            .health
            .sensors()
            .iter()
            .any(|status| status.sensor == Sensor::Airspeed && status.enabled);
        Some(Airspeed { speed, measured })
    }

    pub fn armed(&self) -> Option<bool> {
        self.identity.is_armed()
    }

    /// How this vehicle's firmware lays out missions.
    pub fn mission_layout(&self) -> MissionLayout {
        self.firmware().mission_layout()
    }

    /// The home position: as reported by `HOME_POSITION`, else as held in the
    /// home slot of the last downloaded mission.
    ///
    /// A home slot at latitude and longitude zero means no home: ArduPilot
    /// fills the slot with its home, which is all zeros until it has one
    /// (as [`wire::latlon`] reads reported positions).
    pub fn home(&self) -> Option<Home> {
        self.home_position.or_else(|| {
            let home = self.mission.onboard.as_ref()?.home.as_ref()?;
            let position = home.position()?;
            if position.lat == 0.0 && position.lon == 0.0 {
                return None;
            }
            Some(Home {
                position: GeoPosition::new(position.lat, position.lon),
                altitude_msl: position.alt_m,
            })
        })
    }

    /// The plan index the vehicle is executing, if known.
    pub fn current_mission_index(&self) -> Option<usize> {
        self.mission.progress.current_index(self.mission_layout())
    }

    /// Where the vehicle is, if it has reported a position.
    pub fn location(&self) -> Option<GeoPosition> {
        self.position.current.as_ref().map(|position| position.pos)
    }

    /// How far, and in which direction, home is from the vehicle.
    pub fn to_home(&self) -> Option<Offset> {
        Some(Offset::between(self.location()?, self.home()?.position))
    }

    /// How far the vehicle has come through its mission, while it reports a
    /// current item.
    pub fn mission_status(&self) -> Option<MissionStatus> {
        let current = self.current_mission_index()?;
        let onboard = self.mission.onboard.as_deref();
        let count = onboard
            .map(|mission| mission.items.len())
            .or_else(|| self.mission.progress.total.map(usize::from));
        let to_current = onboard
            .and_then(|mission| mission.items.get(current)?.position())
            .zip(self.location())
            .map(|(item, location)| {
                Offset::between(location, GeoPosition::new(item.lat, item.lon))
            });

        Some(MissionStatus {
            current,
            count,
            state: self.mission.progress.state,
            to_current,
        })
    }

    /// A display name: vendor and model when known, else the class and id.
    pub fn display_name(&self) -> String {
        let state = self.component_state(self.default_parameter_component());
        let vendor = state.and_then(|state| state.vendor_name.as_deref());
        let model = state.and_then(|state| state.model_name.as_deref());

        match (vendor, model) {
            (Some(vendor), Some(model)) => format!("{vendor} {model} ({})", self.system.0),
            (None, Some(model)) => format!("{model} ({})", self.system.0),
            _ => format!("{} {}", self.class().name(), self.system.0),
        }
    }

    /// Record the mission the vehicle holds after a transfer.
    pub(crate) fn set_onboard_mission(&mut self, mission: OnboardMission) {
        self.mission.set_onboard(mission);
    }

    /// Apply what the operation running a calibration reports. Returns the
    /// run's end, if the update finished it.
    pub(crate) fn update_calibration(
        &mut self,
        now: Instant,
        update: CalibrationUpdate,
    ) -> Option<Ended> {
        let protocol = self.firmware().calibration();
        self.calibration.apply(now, update, protocol)
    }

    /// The latest calibration run, until dismissed.
    pub fn calibration(&self) -> Option<&Session> {
        self.calibration.session()
    }

    /// The calibration, for the operations that run one and the fleet that
    /// ages it.
    pub(crate) fn calibration_state(&self) -> &Calibration {
        &self.calibration
    }

    pub(crate) fn calibration_mut(&mut self) -> &mut Calibration {
        &mut self.calibration
    }

    /// The calibrations the vehicle's firmware can run, in display order;
    /// none when there is no calibration protocol for it.
    pub fn calibration_kinds(&self) -> Vec<CalibrationKind> {
        self.firmware()
            .calibration()
            .map_or_else(Vec::new, |protocol| protocol.kinds(self.class()))
    }

    /// What the operator can do about calibrating the vehicle: start a run,
    /// or confirm or cancel the running one.
    pub fn calibration_controls(&self) -> CalibrationControls {
        self.calibration.controls(self.armed(), self.liveness)
    }

    /// How many times the autopilot has rebooted, as far as this build can
    /// tell from its boot time.
    pub(crate) fn boot_count(&self) -> u32 {
        self.boots
    }

    /// Follow the autopilot's boot time: a large step backwards means it
    /// rebooted between two messages.
    fn observe_boot_time(&mut self, time_boot_ms: u32) {
        if let Some(last) = self.last_boot_time
            && last.saturating_sub(time_boot_ms) > BOOT_REWIND_MS
        {
            self.boots = self.boots.saturating_add(1);
            log::info!(
                "Vehicle {} rebooted (boot time {last} ms -> {time_boot_ms} ms)",
                self.system
            );
        }
        self.last_boot_time = Some(time_boot_ms);
    }

    /// Fold a parameter list download's start or outcome in for `component`.
    pub(crate) fn set_param_download(&mut self, component: u8, update: DownloadUpdate) {
        Arc::make_mut(self.components.entry(component).or_default())
            .parameters
            .set_download(update);
    }

    fn on_param_value(&mut self, component: u8, message: &messages::ParamValue) {
        let Ok(ident) = Ident::try_from(&message.param_id) else {
            log::warn!("Invalid parameter identifier: {:?}", message.param_id);
            return;
        };

        // A repeat of a value already stored (PX4 echoes on every link) leaves
        // the component's whole parameter set shared with the last snapshot.
        let Some(state) = self.component_mut_if(component, |state| {
            !state.parameters.reports_nothing_new(
                &ident,
                message.param_value,
                message.param_type,
                message.param_index,
                state.param_encoding(),
            )
        }) else {
            return;
        };

        let encoding = state.param_encoding();
        if let Err(error) = state.parameters.observe(
            ident,
            message.param_value,
            message.param_type,
            message.param_index,
            message.param_count,
            encoding,
        ) {
            log::warn!("Parameter {:?} skipped: {error}", message.param_id);
        }
    }

    /// Fold one decoded message from `component` into the vehicle. Returns
    /// what the fleet announces of it: a status text the message completed,
    /// and the end of a calibration run it finished.
    pub(crate) fn handle_message(
        &mut self,
        time: Instant,
        component: u8,
        message: &DefaultDialect,
    ) -> Folded {
        match message {
            DefaultDialect::Heartbeat(message) => {
                let autopilot = Some(message.autopilot);
                let kind = Some(message.type_);
                if let Some(state) = self.component_mut_if(component, |state| {
                    state.autopilot != autopilot || state.kind != kind
                }) {
                    state.kind = kind;
                    if state.autopilot != autopilot {
                        state.autopilot = autopilot;
                        state.refresh_encoding();
                    }
                }

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
                // Nor may a heartbeat naming no autopilot that shares the
                // autopilot's address: it would make the firmware generic.
                if self.autopilot == Some(component) && message.autopilot != MavAutopilot::Invalid {
                    self.identity.observe_heartbeat(message);
                }
            }
            DefaultDialect::AutopilotVersion(message) => {
                let system = self.system;
                let capabilities = Some(message.capabilities);
                if let Some(state) =
                    self.component_mut_if(component, |state| state.capabilities != capabilities)
                {
                    log::debug!(
                        "Capabilities of {system:?} component {component}: {:?}",
                        message.capabilities
                    );
                    state.capabilities = capabilities;
                    state.refresh_encoding();
                }
            }
            DefaultDialect::ComponentInformationBasic(message) => {
                let model_name = string_from_cstring(&message.model_name);
                let vendor_name = string_from_cstring(&message.vendor_name);
                let capabilities = Some(message.capabilities);
                let changes = |state: &ComponentState| {
                    (model_name.is_some() && state.model_name != model_name)
                        || (vendor_name.is_some() && state.vendor_name != vendor_name)
                        || state.capabilities != capabilities
                };
                if let Some(state) = self.component_mut_if(component, changes) {
                    if model_name.is_some() {
                        state.model_name = model_name;
                    }
                    if vendor_name.is_some() {
                        state.vendor_name = vendor_name;
                    }
                    if state.capabilities != capabilities {
                        state.capabilities = capabilities;
                        state.refresh_encoding();
                    }
                }
            }
            DefaultDialect::ParamValue(message) => {
                self.on_param_value(component, message);
            }
            DefaultDialect::CameraInformation(message) => {
                self.cameras.observe_information(component, message);
            }
            DefaultDialect::CameraTrackingImageStatus(message) => {
                self.cameras.observe_tracking(time, component, message);
            }
            DefaultDialect::SysStatus(message) if self.is_authoritative(component) => {
                self.battery.observe_sys_status(message);
                self.health.observe_sys_status(message);
            }
            DefaultDialect::GpsRawInt(message) if self.is_authoritative(component) => {
                self.health.observe_gps_raw_int(message);
            }
            DefaultDialect::BatteryStatus(message) if self.is_authoritative(component) => {
                self.battery.observe_battery_status(message);
            }
            DefaultDialect::VfrHud(message) if self.is_authoritative(component) => {
                self.hud.observe_vfr_hud(message);
            }
            DefaultDialect::Attitude(message) if self.is_authoritative(component) => {
                self.observe_boot_time(message.time_boot_ms);
                self.attitude.observe_attitude(message);
            }
            DefaultDialect::AttitudeQuaternion(message) if self.is_authoritative(component) => {
                self.attitude.observe_attitude_quaternion(message);
            }
            DefaultDialect::GlobalPositionInt(message) if self.is_authoritative(component) => {
                self.observe_boot_time(message.time_boot_ms);
                if let Some((lat, lon)) = wire::latlon(message.lat, message.lon) {
                    let pos = GeoPosition::new(lat, lon);
                    Arc::make_mut(&mut self.position)
                        .observe_global_position_int(message, pos, time);
                }
            }
            DefaultDialect::GlobalPositionIntCov(message) if self.is_authoritative(component) => {
                if let Some((lat, lon)) = wire::latlon(message.lat, message.lon) {
                    let pos = GeoPosition::new(lat, lon);
                    Arc::make_mut(&mut self.position)
                        .observe_global_position_int_cov(message, pos, time);
                }
            }
            DefaultDialect::HomePosition(message) if self.is_authoritative(component) => {
                if let Some(home) = Home::from_message(message) {
                    self.home_position = Some(home);
                }
            }
            DefaultDialect::MissionCurrent(message) if self.is_authoritative(component) => {
                self.mission.observe_current(message);
            }
            DefaultDialect::Statustext(message) => {
                let text =
                    Arc::make_mut(&mut self.status).observe_statustext(component, message, time);
                let calibration = match &text {
                    Some(text) if self.is_authoritative(component) => {
                        self.calibration.observe(time, Feedback::Text(text))
                    }
                    _ => None,
                };
                return Folded { text, calibration };
            }
            _ => (),
        }

        let calibration = if self.is_authoritative(component) {
            self.calibration.observe(time, Feedback::Message(message))
        } else {
            None
        };

        Folded {
            text: None,
            calibration,
        }
    }
}

/// What folding one message into a vehicle produced, for the fleet to
/// announce.
#[derive(Debug, Default)]
pub(crate) struct Folded {
    /// A status text the message completed.
    pub(crate) text: Option<StatusText>,
    /// The end of the calibration run the message finished.
    pub(crate) calibration: Option<Ended>,
}

/// Decode a fixed-size, NUL-terminated MAVLink string, if it is valid,
/// non-empty UTF-8.
pub(super) fn string_from_cstring(cstring: &[u8]) -> Option<String> {
    let null = cstring
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(cstring.len());

    str::from_utf8(&cstring[..null])
        .ok()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mav_param::Value;
    use mavio::default_dialect::enums::{MavModeFlag, MavParamType, MavType, MissionState};

    fn vehicle() -> Vehicle {
        Vehicle::new(SystemId(1))
    }

    fn heartbeat(autopilot: MavAutopilot) -> DefaultDialect {
        DefaultDialect::Heartbeat(messages::Heartbeat {
            type_: MavType::Quadrotor,
            autopilot,
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
            ..Default::default()
        })
    }

    fn param_value(name: &str, value: f32, param_type: MavParamType) -> DefaultDialect {
        let mut param_id = [0u8; 16];
        param_id[..name.len()].copy_from_slice(name.as_bytes());

        DefaultDialect::ParamValue(messages::ParamValue {
            param_id,
            param_value: value,
            param_type,
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
    fn health_is_taken_from_the_autopilot_only() {
        use mavio::default_dialect::enums::{GpsFixType, MavSysStatusSensor};

        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));

        let gps = |fix_type| {
            DefaultDialect::GpsRawInt(messages::GpsRawInt {
                fix_type,
                satellites_visible: 12,
                eph: 90,
                ..Default::default()
            })
        };
        vehicle.handle_message(t, 1, &gps(GpsFixType::_3dFix));
        // A peripheral's own GPS does not describe the vehicle.
        vehicle.handle_message(t, 154, &gps(GpsFixType::NoFix));
        assert_eq!(vehicle.health.gps().unwrap().fix, GpsFixType::_3dFix);

        let compass = MavSysStatusSensor::_3D_MAG;
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::SysStatus(messages::SysStatus {
                onboard_control_sensors_present: compass,
                onboard_control_sensors_enabled: compass,
                onboard_control_sensors_health: MavSysStatusSensor::empty(),
                ..Default::default()
            }),
        );
        assert_eq!(
            vehicle.health.failing().collect::<Vec<_>>(),
            [health::Sensor::Compass]
        );
    }

    #[test]
    fn home_and_the_current_item_are_measured_from_the_vehicle() {
        use crate::domain::mission::{MissionItem, Position};

        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Px4));
        assert_eq!(vehicle.to_home(), None, "no position yet");

        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::GlobalPositionInt(messages::GlobalPositionInt {
                lat: 550_000_000,
                lon: 100_000_000,
                ..Default::default()
            }),
        );
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::HomePosition(messages::HomePosition {
                latitude: 550_100_000,
                longitude: 100_000_000,
                ..Default::default()
            }),
        );
        let home = vehicle.to_home().unwrap();
        assert!((home.distance_m - 1112.0).abs() < 5.0, "{home:?}");
        assert!(home.bearing_deg.abs() < 0.01, "due north");

        let waypoint = |lat| {
            MissionItem::waypoint(Position {
                lat,
                lon: 10.0,
                alt_m: 30.0,
            })
        };
        vehicle.set_onboard_mission(OnboardMission {
            items: vec![waypoint(55.0), waypoint(54.99)],
            home: None,
            opaque_id: 0,
        });
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::MissionCurrent(messages::MissionCurrent {
                seq: 1,
                total: 2,
                mission_state: MissionState::Active,
                ..Default::default()
            }),
        );
        let status = vehicle.mission_status().unwrap();
        assert_eq!((status.current, status.count), (1, Some(2)));
        assert_eq!(status.state, MissionState::Active);
        let to_current = status.to_current.unwrap();
        assert!((to_current.bearing_deg - 180.0).abs() < 0.01, "due south");
    }

    #[test]
    fn the_latest_alert_skips_lesser_texts() {
        use mavio::default_dialect::enums::MavSeverity;

        let mut vehicle = vehicle();
        let t = Instant::now();
        let text = |severity, text: &str| {
            let mut bytes = [0u8; 50];
            bytes[..text.len()].copy_from_slice(text.as_bytes());
            DefaultDialect::Statustext(messages::Statustext {
                severity,
                text: bytes,
                ..Default::default()
            })
        };
        vehicle.handle_message(t, 1, &text(MavSeverity::Info, "Ready"));
        assert_eq!(vehicle.status.latest_alert(), None);

        vehicle.handle_message(t, 1, &text(MavSeverity::Warning, "PreArm: Compass"));
        vehicle.handle_message(t, 1, &text(MavSeverity::Info, "Mode changed"));
        assert_eq!(
            vehicle
                .status
                .latest_alert()
                .map(|alert| alert.text.as_str()),
            Some("PreArm: Compass")
        );
    }

    #[test]
    fn mission_current_updates_progress() {
        let mut vehicle = vehicle();

        vehicle.handle_message(
            Instant::now(),
            1,
            &DefaultDialect::MissionCurrent(messages::MissionCurrent {
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
    fn mission_totals_of_zero_and_max_are_no_count() {
        for total in [0, u16::MAX] {
            let mut vehicle = vehicle();
            vehicle.handle_message(
                Instant::now(),
                1,
                &DefaultDialect::MissionCurrent(messages::MissionCurrent {
                    seq: 1,
                    total,
                    ..Default::default()
                }),
            );
            assert_eq!(vehicle.mission.progress.total, None, "total {total}");
        }
    }

    /// `MISSION_CURRENT.total` already leaves out ArduPilot's home slot.
    #[test]
    fn the_reported_total_counts_plan_items() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::MissionCurrent(messages::MissionCurrent {
                seq: 1,
                total: 3,
                ..Default::default()
            }),
        );

        assert_eq!(vehicle.mission_status().unwrap().count, Some(3));
    }

    #[test]
    fn the_current_index_follows_the_firmware_layout() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::MissionCurrent(messages::MissionCurrent {
                seq: 3,
                ..Default::default()
            }),
        );

        assert_eq!(
            vehicle.current_mission_index(),
            Some(2),
            "ArduPilot seq 3 is plan item 2"
        );
    }

    #[test]
    fn a_heartbeat_naming_no_autopilot_keeps_the_firmware() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        let mode = vehicle.flight_mode();
        assert!(mode.is_some());

        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Invalid));

        assert_eq!(vehicle.firmware().name(), "ArduPilot");
        assert!(vehicle.mission_layout().home_slot);
        assert_eq!(vehicle.flight_mode(), mode);
    }

    /// An ArduPlane, which reports itself as a fixed wing even when it is a
    /// QuadPlane, with `Q_ENABLE` as given (or not yet downloaded).
    fn ardupilot_plane(q_enable: Option<f32>) -> Vehicle {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::Heartbeat(messages::Heartbeat {
                type_: MavType::FixedWing,
                autopilot: MavAutopilot::Ardupilotmega,
                base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
                ..Default::default()
            }),
        );
        if let Some(value) = q_enable {
            vehicle.handle_message(t, 1, &param_value("Q_ENABLE", value, MavParamType::Int8));
        }
        vehicle
    }

    #[test]
    fn a_plane_without_a_quadplane_offers_no_q_modes() {
        let vehicle = ardupilot_plane(Some(0.0));

        assert!(vehicle.modes().all(|mode| !mode.name.starts_with('Q')));
        assert!(vehicle.modes().any(|mode| mode.name == "Autoland"));
        assert_eq!(vehicle.mode_of_kind(ModeKind::Land), None);
        assert!(vehicle.mode_of_kind(ModeKind::ReturnToLaunch).is_some());
    }

    #[test]
    fn a_quadplane_lands_in_qland_and_an_unknown_one_is_offered_it() {
        for q_enable in [Some(1.0), None] {
            let vehicle = ardupilot_plane(q_enable);
            let land = vehicle.mode_of_kind(ModeKind::Land);
            assert_eq!(land.map(|mode| mode.name), Some("QLand"), "{q_enable:?}");
        }
    }

    #[test]
    fn a_copter_keeps_its_modes_whatever_q_enable_says() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        vehicle.handle_message(t, 1, &param_value("Q_ENABLE", 0.0, MavParamType::Int8));

        assert_eq!(
            vehicle.mode_of_kind(ModeKind::Land).map(|mode| mode.name),
            Some("Land")
        );
        let selectable = firmware::ardupilot::COPTER
            .iter()
            .filter(|mode| mode.selectable);
        assert_eq!(vehicle.modes().count(), selectable.count());
    }

    #[test]
    fn ardupilot_parameters_decode_with_a_c_cast_before_capabilities_arrive() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        vehicle.handle_message(t, 1, &param_value("ARMING_CHECK", 1.0, MavParamType::Int32));

        let value = vehicle
            .parameters(1)
            .unwrap()
            .value(&Ident::from_str_truncated("ARMING_CHECK"));
        assert_eq!(value, Some(Value::I32(1)));
        assert_eq!(vehicle.param_encoding(1), Some(ParamEncoding::CCast));
    }

    #[test]
    fn an_unknown_encoding_is_reported_as_unknown() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Generic));
        vehicle.handle_message(t, 1, &param_value("P", 1.0, MavParamType::Real32));

        assert_eq!(vehicle.param_encoding(1), None);

        // Advertised capabilities settle it and re-decode the stored values.
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::AutopilotVersion(messages::AutopilotVersion {
                capabilities: MavProtocolCapability::PARAM_ENCODE_BYTEWISE,
                ..Default::default()
            }),
        );
        assert_eq!(vehicle.param_encoding(1), Some(ParamEncoding::Bytewise));
    }

    #[test]
    fn components_keep_separate_parameter_sets() {
        let mut vehicle = vehicle();
        let t = Instant::now();

        vehicle.handle_message(t, 1, &param_value("PARAM_A", 1.0, MavParamType::Real32));
        vehicle.handle_message(t, 154, &param_value("PARAM_B", 2.0, MavParamType::Real32));

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

        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::Attitude(messages::Attitude {
                roll: 1.0,
                ..Default::default()
            }),
        );
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::SysStatus(messages::SysStatus {
                voltage_battery: 12_000,
                ..Default::default()
            }),
        );

        // A peripheral's frames must not change any vehicle-scoped state.
        vehicle.handle_message(
            t,
            154,
            &DefaultDialect::Attitude(messages::Attitude {
                roll: 2.0,
                ..Default::default()
            }),
        );
        vehicle.handle_message(
            t,
            154,
            &DefaultDialect::SysStatus(messages::SysStatus {
                voltage_battery: 9_000,
                ..Default::default()
            }),
        );

        assert_eq!(vehicle.battery.primary().unwrap().voltage, Some(12.0));
        let roll = vehicle
            .attitude
            .current
            .as_ref()
            .unwrap()
            .attitude
            .euler_angles()
            .0;
        assert!((roll - 1.0).abs() < 1e-6);
    }

    #[test]
    fn status_text_records_its_component() {
        let mut vehicle = vehicle();
        let t = Instant::now();

        vehicle.handle_message(t, 1, &statustext("auto"));
        vehicle.handle_message(t, 154, &statustext("gimbal"));

        let texts = &vehicle.status.texts;
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0].component, 1);
        assert_eq!(texts[0].text, "auto");
        assert_eq!(texts[1].component, 154);
        assert_eq!(texts[1].text, "gimbal");
    }

    /// A component's state holds its parameters; a heartbeat that changes
    /// nothing must not copy them.
    #[test]
    fn an_unchanged_heartbeat_shares_the_component() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        vehicle.handle_message(t, 1, &param_value("GAIN", 1.0, MavParamType::Real32));

        let snapshot = vehicle.clone();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        assert!(Arc::ptr_eq(
            &snapshot.components[&1],
            &vehicle.components[&1]
        ));

        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Px4));
        assert!(!Arc::ptr_eq(
            &snapshot.components[&1],
            &vehicle.components[&1]
        ));
    }

    #[test]
    fn snapshots_share_untouched_subsystems() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &statustext("before"));

        let snapshot = vehicle.clone();
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::GlobalPositionInt(messages::GlobalPositionInt {
                lat: 550_000_000,
                lon: 100_000_000,
                ..Default::default()
            }),
        );

        assert!(
            Arc::ptr_eq(&snapshot.status, &vehicle.status),
            "untouched, shared"
        );
        assert!(
            !Arc::ptr_eq(&snapshot.position, &vehicle.position),
            "copied on write"
        );
        assert!(
            snapshot.position.current.is_none(),
            "the snapshot is unaffected"
        );
    }

    #[test]
    fn home_falls_back_to_the_mission_home_slot() {
        let mut vehicle = vehicle();
        assert!(vehicle.home().is_none());

        vehicle.set_onboard_mission(OnboardMission {
            items: Vec::new(),
            home: Some(crate::domain::mission::MissionItem::home(
                crate::domain::mission::Position {
                    lat: 47.0,
                    lon: 8.0,
                    alt_m: 420.0,
                },
            )),
            opaque_id: 0,
        });

        let home = vehicle.home().unwrap();
        assert!((home.position.lat - 47.0).abs() < 1e-6);
        assert_eq!(home.altitude_msl, 420.0);
    }

    /// Before it has a location, ArduPilot reports one of all zeros.
    #[test]
    fn a_reported_position_at_zero_is_no_position() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        let report = |lat, lon| {
            DefaultDialect::GlobalPositionInt(messages::GlobalPositionInt {
                lat,
                lon,
                ..Default::default()
            })
        };
        vehicle.handle_message(t, 1, &report(0, 0));
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::HomePosition(messages::HomePosition::default()),
        );
        assert!(vehicle.position.current.is_none());
        assert!(vehicle.position.trail.is_empty());
        assert_eq!(vehicle.home(), None);

        // One coordinate at zero is a place (the equator, or Greenwich).
        vehicle.handle_message(t, 1, &report(0, 100_000_000));
        assert_eq!(vehicle.location(), Some(GeoPosition::new(0.0, 10.0)));
        assert_eq!(vehicle.position.trail.len(), 1);
    }

    /// A vehicle of `type_`, heartbeating `base_mode`.
    fn vehicle_of(type_: MavType, base_mode: MavModeFlag) -> Vehicle {
        let mut vehicle = vehicle();
        vehicle.handle_message(
            Instant::now(),
            1,
            &DefaultDialect::Heartbeat(messages::Heartbeat {
                type_,
                autopilot: MavAutopilot::Ardupilotmega,
                base_mode,
                ..Default::default()
            }),
        );
        vehicle
    }

    fn vfr_hud(airspeed: f32) -> DefaultDialect {
        DefaultDialect::VfrHud(messages::VfrHud {
            airspeed,
            throttle: 40,
            ..Default::default()
        })
    }

    /// `SYS_STATUS` with an airspeed sensor present, and enabled or not.
    fn airspeed_sensor(enabled: bool) -> DefaultDialect {
        use mavio::default_dialect::enums::MavSysStatusSensor as F;
        let present = F::DIFFERENTIAL_PRESSURE;
        DefaultDialect::SysStatus(messages::SysStatus {
            onboard_control_sensors_present: present,
            onboard_control_sensors_enabled: if enabled { present } else { F::empty() },
            onboard_control_sensors_health: present,
            ..Default::default()
        })
    }

    #[test]
    fn a_plane_reports_airspeed_and_says_whether_it_was_measured() {
        let t = Instant::now();
        let mut plane = vehicle_of(MavType::FixedWing, MavModeFlag::empty());
        assert_eq!(plane.airspeed(), None, "nothing reported yet");

        plane.handle_message(t, 1, &vfr_hud(17.0));
        assert_eq!(
            plane.airspeed(),
            Some(Airspeed {
                speed: 17.0,
                measured: false
            }),
            "estimated while no sensor is reported"
        );

        plane.handle_message(t, 1, &airspeed_sensor(false));
        assert_eq!(
            plane.airspeed().map(|airspeed| airspeed.measured),
            Some(false),
            "a sensor that is not in use measures nothing"
        );

        plane.handle_message(t, 1, &airspeed_sensor(true));
        assert_eq!(
            plane.airspeed().map(|airspeed| airspeed.measured),
            Some(true)
        );
        assert_eq!(plane.hud.throttle, Some(40));
    }

    #[test]
    fn only_winged_vehicles_report_airspeed() {
        let t = Instant::now();
        for (type_, winged) in [
            (MavType::FixedWing, true),
            (MavType::VtolTiltrotor, true),
            (MavType::Quadrotor, false),
            (MavType::GroundRover, false),
        ] {
            let mut vehicle = vehicle_of(type_, MavModeFlag::empty());
            vehicle.handle_message(t, 1, &vfr_hud(12.0));
            assert_eq!(vehicle.airspeed().is_some(), winged, "{type_:?}");
            assert_eq!(vehicle.hud.throttle, Some(40), "{type_:?} has a throttle");
        }
    }

    #[test]
    fn a_peripheral_does_not_report_the_vehicles_airspeed() {
        let mut plane = vehicle_of(MavType::FixedWing, MavModeFlag::empty());
        plane.handle_message(Instant::now(), 154, &vfr_hud(30.0));
        assert_eq!(plane.hud, Hud::default());
    }

    #[test]
    fn only_a_vehicle_known_to_be_disarmed_may_reboot() {
        assert!(!vehicle().can_reboot(), "arming unknown");

        let disarmed = vehicle_of(MavType::Quadrotor, MavModeFlag::CUSTOM_MODE_ENABLED);
        assert!(disarmed.can_reboot());

        let armed = vehicle_of(
            MavType::Quadrotor,
            MavModeFlag::CUSTOM_MODE_ENABLED | MavModeFlag::SAFETY_ARMED,
        );
        assert!(!armed.can_reboot());
    }

    /// Before ArduPilot has a fix, its home slot reads back as all zeros.
    #[test]
    fn a_home_slot_at_zero_is_no_home() {
        let mut vehicle = vehicle();
        vehicle.set_onboard_mission(OnboardMission {
            items: Vec::new(),
            home: Some(crate::domain::mission::MissionItem::home(
                crate::domain::mission::Position {
                    lat: 0.0,
                    lon: 0.0,
                    alt_m: 0.0,
                },
            )),
            opaque_id: 0,
        });

        assert_eq!(vehicle.home(), None);
    }

    /// A quaternion report reaches the attitude, not just the observer.
    #[test]
    fn a_quaternion_folds_into_attitude() {
        let mut vehicle = vehicle();
        vehicle.handle_message(
            Instant::now(),
            1,
            &DefaultDialect::AttitudeQuaternion(messages::AttitudeQuaternion {
                q1: 1.0,
                ..Default::default()
            }),
        );

        let attitude = vehicle
            .attitude
            .current
            .as_ref()
            .expect("attitude recorded");
        let (roll, pitch, yaw) = attitude.attitude.euler_angles();
        assert!(roll.abs() < 1e-6 && pitch.abs() < 1e-6 && yaw.abs() < 1e-6);
    }

    /// A battery report reaches the summary, not just the observer.
    #[test]
    fn a_battery_status_folds_into_the_summary() {
        let mut vehicle = vehicle();
        let mut voltages = [u16::MAX; 10];
        voltages[..3].copy_from_slice(&[4_000, 4_000, 4_000]);
        vehicle.handle_message(
            Instant::now(),
            1,
            &DefaultDialect::BatteryStatus(messages::BatteryStatus {
                id: 0,
                voltages,
                current_battery: 250,
                battery_remaining: 80,
                ..Default::default()
            }),
        );

        let state = vehicle.battery.primary().expect("battery recorded");
        assert_eq!(state.current, Some(2.5));
        assert_eq!(state.remaining, Some(80.0));
    }

    /// A covariance position carries the velocity, which the UI shows as
    /// climb and ground speed.
    #[test]
    fn a_cov_position_records_its_velocity() {
        let mut vehicle = vehicle();
        vehicle.handle_message(
            Instant::now(),
            1,
            &DefaultDialect::GlobalPositionIntCov(messages::GlobalPositionIntCov {
                lat: 474_000_000,
                lon: 85_000_000,
                relative_alt: 30_000,
                vx: 3.0,
                vy: 4.0,
                vz: -2.0,
                ..Default::default()
            }),
        );

        assert_eq!(vehicle.location(), Some(GeoPosition::new(47.4, 8.5)));
        assert_eq!(
            vehicle.position.climb_rate(),
            Some(2.0),
            "negative z climbs"
        );
        assert_eq!(vehicle.position.ground_speed(), Some(5.0));
        assert_eq!(
            vehicle
                .position
                .current
                .as_ref()
                .expect("position recorded")
                .alt_agl,
            30.0
        );
    }

    /// The component information names the vehicle for the operator.
    #[test]
    fn component_information_names_the_vehicle() {
        let mut vehicle = vehicle();
        let t = Instant::now();
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));

        let mut vendor = [0u8; 32];
        vendor[..4].copy_from_slice(b"Acme");
        let mut model = [0u8; 32];
        model[..5].copy_from_slice(b"Plane");
        vehicle.handle_message(
            t,
            1,
            &DefaultDialect::ComponentInformationBasic(messages::ComponentInformationBasic {
                vendor_name: vendor,
                model_name: model,
                ..Default::default()
            }),
        );

        assert_eq!(vehicle.display_name(), "Acme Plane (1)");
    }

    /// The vehicle hands its calibration what only it knows: the firmware's
    /// protocol, its armed state and liveness, and the autopilot's feedback.
    #[test]
    fn the_calibration_follows_the_firmware_and_the_autopilot() {
        let t = Instant::now();
        let mut vehicle = vehicle();
        assert!(vehicle.calibration_kinds().is_empty(), "unknown firmware");
        vehicle.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        let kinds = vehicle.calibration_kinds();
        assert!(kinds.contains(&CalibrationKind::Accelerometer));
        assert!(
            !kinds.contains(&CalibrationKind::Airspeed),
            "a copter has no airspeed sensor"
        );
        assert_eq!(
            vehicle.calibration_controls().start_blocked,
            None,
            "the heartbeat says it is disarmed"
        );

        let run = crate::domain::calibration::RunId::next();
        let kind = CalibrationKind::Accelerometer;
        vehicle.update_calibration(t, CalibrationUpdate::Begin { kind, run });
        vehicle.update_calibration(
            t,
            CalibrationUpdate::Acknowledged {
                run,
                result: Ok(()),
            },
        );
        let position = DefaultDialect::CommandLong(messages::CommandLong {
            command: mavio::default_dialect::enums::MavCmd::AccelcalVehiclePos,
            param1: 1.0,
            ..Default::default()
        });

        vehicle.handle_message(t, 2, &position);
        assert!(
            !vehicle.calibration_controls().confirm,
            "a peripheral does not speak for the autopilot"
        );
        vehicle.handle_message(t, 1, &position);
        assert!(vehicle.calibration_controls().confirm);
    }
}
