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
pub mod class;
pub mod health;
pub mod home;
pub mod identity;
pub mod mission;
pub mod position;
pub mod status;

pub use self::status::StatusText;

use self::{
    attitude::AttitudeState, battery::Battery, class::VehicleClass, health::Health, home::Home,
    identity::Identity, mission::VehicleMission, position::PositionTrack, status::StatusLog,
};

use std::{collections::BTreeMap, sync::Arc, time::Instant};

use mav_param::Ident;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavAutopilot, MavProtocolCapability, MissionState},
        messages,
    },
};

use crate::{
    MavlinkId, SystemId,
    domain::{
        calibration::{self, CalibrationControls, CalibrationKind, CalibrationUpdate, Feedback},
        firmware::{self, Firmware, FlightMode, MissionLayout, ModeKind},
        geo::GeoPosition,
        mission::OnboardMission,
        parameter::{DownloadState, Parameters},
    },
    wire::{self, param::ParamEncoding},
};

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
    pub model_name: Option<String>,
    pub vendor_name: Option<String>,
    parameters: Parameters,
}

impl ComponentState {
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
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
    pub status: Arc<StatusLog>,
    home_position: Option<Home>,
    pub mission: VehicleMission,
    /// Per-component state (parameters, capabilities, names), keyed by the
    /// MAVLink component id.
    components: BTreeMap<u8, Arc<ComponentState>>,
    /// The latest calibration run, until dismissed.
    pub calibration: Option<calibration::Session>,
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

    fn component_mut(&mut self, component: u8) -> &mut ComponentState {
        Arc::make_mut(self.components.entry(component).or_default())
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
        self.mission.onboard = Some(Arc::new(mission));
    }

    /// Apply what the operation running a calibration reports. Returns
    /// whether it was applied: a vehicle whose firmware has no calibration
    /// protocol cannot begin one.
    pub(crate) fn update_calibration(&mut self, update: CalibrationUpdate) -> bool {
        match update {
            CalibrationUpdate::Begin(kind) => {
                let Some(protocol) = self.firmware().calibration() else {
                    return false;
                };
                self.calibration = Some(calibration::Session::new(
                    kind,
                    protocol.completes_on_ack(kind),
                ));
            }
            CalibrationUpdate::Acknowledged(result) => match &mut self.calibration {
                Some(session) => session.acknowledged(result),
                None => return false,
            },
            CalibrationUpdate::Cancel => match &mut self.calibration {
                Some(session) => session.cancel(),
                None => return false,
            },
            CalibrationUpdate::Dismiss => {
                if self
                    .calibration
                    .as_ref()
                    .is_some_and(calibration::Session::is_running)
                {
                    return false;
                }
                self.calibration = None;
            }
        }
        true
    }

    /// The calibrations the vehicle's firmware can run, in display order;
    /// none when there is no calibration protocol for it.
    pub fn calibration_kinds(&self) -> Vec<CalibrationKind> {
        self.firmware()
            .calibration()
            .map_or_else(Vec::new, |protocol| protocol.kinds())
    }

    /// What the operator can do about the running calibration.
    pub fn calibration_controls(&self) -> CalibrationControls {
        match (self.firmware().calibration(), &self.calibration) {
            (Some(protocol), Some(session)) if session.is_running() => CalibrationControls {
                confirm: protocol.confirm(session).is_some(),
                cancel: protocol.cancel(session).is_some(),
            },
            _ => CalibrationControls::default(),
        }
    }

    /// Fold the autopilot's feedback into a running calibration.
    fn observe_calibration(&mut self, feedback: Feedback<'_>) {
        let protocol = self.firmware().calibration();
        if let (Some(protocol), Some(session)) = (protocol, &mut self.calibration)
            && session.is_running()
        {
            protocol.observe(session, feedback);
        }
    }

    /// Record a parameter list download's progress for `component`.
    pub(crate) fn set_param_download(&mut self, component: u8, state: DownloadState) {
        Arc::make_mut(self.components.entry(component).or_default())
            .parameters
            .set_download(state);
    }

    fn on_param_value(&mut self, component: u8, message: &messages::ParamValue) {
        let Ok(ident) = Ident::try_from(&message.param_id) else {
            log::warn!("Invalid parameter identifier: {:?}", message.param_id);
            return;
        };

        let state = self.component_mut(component);
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

    /// Fold one decoded message from `component` into the vehicle. Returns a
    /// status text the message completed, if any.
    pub(crate) fn handle_message(
        &mut self,
        time: Instant,
        component: u8,
        message: &DefaultDialect,
    ) -> Option<StatusText> {
        match message {
            DefaultDialect::Heartbeat(message) => {
                let autopilot = Some(message.autopilot);
                if let Some(state) =
                    self.component_mut_if(component, |state| state.autopilot != autopilot)
                {
                    state.autopilot = autopilot;
                    state.refresh_encoding();
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
            DefaultDialect::Attitude(message) if self.is_authoritative(component) => {
                self.attitude.observe_attitude(message);
            }
            DefaultDialect::AttitudeQuaternion(message) if self.is_authoritative(component) => {
                self.attitude.observe_attitude_quaternion(message);
            }
            DefaultDialect::GlobalPositionInt(message) if self.is_authoritative(component) => {
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
                self.mission.progress.observe_current(message);
            }
            DefaultDialect::Statustext(message) => {
                let text =
                    Arc::make_mut(&mut self.status).observe_statustext(component, message, time);
                if let Some(text) = &text
                    && self.is_authoritative(component)
                {
                    self.observe_calibration(Feedback::Text(text));
                }
                return text;
            }
            _ => (),
        }

        if self.is_authoritative(component) {
            self.observe_calibration(Feedback::Message(message));
        }

        None
    }
}

/// Decode a fixed-size, NUL-terminated MAVLink string, if it is valid,
/// non-empty UTF-8.
fn string_from_cstring(cstring: &[u8]) -> Option<String> {
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

    /// The controls a frontend offers come from the firmware's protocol and
    /// the step the run is at, not from the frontend.
    #[test]
    fn calibration_controls_follow_the_firmware_and_the_step() {
        let t = Instant::now();
        let mut ardupilot = vehicle();
        assert!(ardupilot.calibration_kinds().is_empty(), "unknown firmware");
        ardupilot.handle_message(t, 1, &heartbeat(MavAutopilot::Ardupilotmega));
        assert!(
            ardupilot
                .calibration_kinds()
                .contains(&CalibrationKind::Airspeed)
        );

        ardupilot.update_calibration(CalibrationUpdate::Begin(CalibrationKind::Accelerometer));
        ardupilot.update_calibration(CalibrationUpdate::Acknowledged(Ok(())));
        assert_eq!(
            ardupilot.calibration_controls(),
            CalibrationControls::default(),
            "nothing to confirm before the vehicle asks"
        );
        ardupilot.handle_message(
            t,
            1,
            &DefaultDialect::CommandLong(messages::CommandLong {
                command: mavio::default_dialect::enums::MavCmd::AccelcalVehiclePos,
                param1: 1.0,
                ..Default::default()
            }),
        );
        assert_eq!(
            ardupilot.calibration_controls(),
            CalibrationControls {
                confirm: true,
                cancel: false
            }
        );

        let mut px4 = vehicle();
        px4.handle_message(t, 1, &heartbeat(MavAutopilot::Px4));
        px4.update_calibration(CalibrationUpdate::Begin(CalibrationKind::Gyroscope));
        assert_eq!(
            px4.calibration_controls(),
            CalibrationControls {
                confirm: false,
                cancel: true
            }
        );
        px4.update_calibration(CalibrationUpdate::Cancel);
        assert_eq!(
            px4.calibration_controls(),
            CalibrationControls::default(),
            "a finished run has no controls"
        );
    }
}
