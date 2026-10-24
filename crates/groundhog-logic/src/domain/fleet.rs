//! The fleet: every vehicle the GCS has seen.
//!
//! [`Fleet`] is the single owner of vehicle state. A runtime feeds inbound
//! messages in through [`Fleet::observe`] and turns the resulting
//! [`FleetEvent`]s into effects; readers get an immutable [`FleetSnapshot`],
//! which shares all vehicle data with the fleet until the fleet next changes
//! it.
//!
//! Identity is per MAVLink *system* ([`SystemId`]), not per address: a system
//! is a vehicle, and the several components that may heartbeat under it
//! (autopilot, gimbal, companion computer, ...) are endpoints of that one
//! vehicle rather than vehicles of their own.
//!
//! A system only becomes a vehicle when one of its components heartbeats as an
//! autopilot of a vehicle kind. Other ground stations, telemetry radios, ADS-B
//! receivers and our own echoed frames are not vehicles, and must not be
//! offered to the operator or bootstrapped with parameter downloads.

use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use mavio::{DefaultDialect, default_dialect::enums::MavAutopilot};

use crate::{
    MavlinkId, SystemId,
    domain::{
        calibration::{CalibrationKind, CalibrationUpdate, Outcome, Session},
        firmware::FlightMode,
        mission::OnboardMission,
        parameter::{DownloadState, Parameters},
        vehicle::{Liveness, StatusText, Vehicle, class::VehicleClass},
    },
};

/// A vehicle still counts as active while it has been heard from within this
/// window. MAVLink heartbeats are nominally 1 Hz, so this tolerates a couple of
/// missed beats before flagging the link.
pub const STALE_AFTER: Duration = Duration::from_secs(3);

/// Silence this long means the vehicle is presumed lost.
pub const LOST_AFTER: Duration = Duration::from_secs(10);

/// Something the fleet observed that the application may need to react to.
#[derive(Debug, Clone, PartialEq)]
pub enum FleetEvent {
    /// A vehicle was seen for the first time.
    Registered(SystemId),
    /// A vehicle's liveness changed, as the fleet ages vehicles or a fresh frame
    /// arrives. A vehicle recovers straight to active, so `from` tells a
    /// recovery after a loss from one after a stale spell.
    LivenessChanged {
        system: SystemId,
        from: Liveness,
        to: Liveness,
    },
    /// The vehicle armed (`true`) or disarmed.
    ArmedChanged(SystemId, bool),
    /// The vehicle changed flight mode (`None`: a mode the firmware adapter
    /// does not know).
    ModeChanged(SystemId, Option<FlightMode>),
    /// A complete status text arrived.
    StatusText(SystemId, StatusText),
    /// The vehicle now reports holding a different mission than the one last
    /// transferred.
    OnboardMissionStale(SystemId),
    /// A calibration ended: succeeded, failed or cancelled.
    CalibrationFinished(SystemId, CalibrationKind, Outcome),
}

/// An immutable view of the fleet at one moment. Cheap to clone and to keep:
/// vehicles are shared with the fleet, copied only when the fleet changes them.
#[derive(Debug, Clone, Default)]
pub struct FleetSnapshot {
    vehicles: BTreeMap<SystemId, Arc<Vehicle>>,
}

impl FleetSnapshot {
    pub fn vehicle(&self, id: SystemId) -> Option<&Vehicle> {
        self.vehicles.get(&id).map(Arc::as_ref)
    }

    /// The parameters of the component at `address`, if it has been heard
    /// from.
    pub fn parameters(&self, address: MavlinkId) -> Option<&Parameters> {
        self.vehicle(address.system_id())?
            .parameters(address.component)
    }

    /// How to name a vehicle to the operator: its [`Vehicle::display_name`],
    /// or its system id when the snapshot does not hold it (yet, or any more).
    pub fn vehicle_name(&self, id: SystemId) -> String {
        self.vehicle(id)
            .map_or_else(|| format!("vehicle {}", id.0), Vehicle::display_name)
    }

    /// Every vehicle, in system id order.
    pub fn vehicles(&self) -> impl Iterator<Item = &Vehicle> {
        self.vehicles.values().map(Arc::as_ref)
    }

    pub fn ids(&self) -> impl Iterator<Item = SystemId> + '_ {
        self.vehicles.keys().copied()
    }

    pub fn contains(&self, id: SystemId) -> bool {
        self.vehicles.contains_key(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.vehicles.is_empty()
    }
}

/// Every vehicle the GCS has seen.
#[derive(Default)]
pub struct Fleet {
    vehicles: BTreeMap<SystemId, Arc<Vehicle>>,
}

impl Fleet {
    #[cfg(test)]
    pub fn vehicle(&self, id: SystemId) -> Option<&Vehicle> {
        self.vehicles.get(&id).map(Arc::as_ref)
    }

    /// Mutable access to one vehicle. Copies it first if a snapshot still
    /// shares it.
    pub(crate) fn vehicle_mut(&mut self, id: SystemId) -> Option<&mut Vehicle> {
        let vehicle = self.vehicles.get_mut(&id)?;
        Some(Arc::make_mut(vehicle))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.vehicles.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.vehicles.is_empty()
    }

    /// The fleet as of now.
    pub fn snapshot(&self) -> FleetSnapshot {
        FleetSnapshot {
            vehicles: self.vehicles.clone(),
        }
    }

    /// Whether a heartbeat makes its system a vehicle.
    fn registers(message: &DefaultDialect) -> bool {
        matches!(
            message,
            DefaultDialect::Heartbeat(heartbeat)
                if heartbeat.autopilot != MavAutopilot::Invalid
                    && VehicleClass::of(heartbeat.type_).is_vehicle()
        )
    }

    /// Fold one inbound frame into vehicle state, returning what changed.
    ///
    /// `message` is `None` for a frame that did not decode: it still refreshes
    /// an already registered vehicle's liveness. Frames from systems that are
    /// not (yet) vehicles are ignored.
    pub fn observe(
        &mut self,
        source: MavlinkId,
        received_at: Instant,
        message: Option<&DefaultDialect>,
    ) -> Vec<FleetEvent> {
        let mut events = Vec::new();
        let id = source.system_id();

        if let std::collections::btree_map::Entry::Vacant(entry) = self.vehicles.entry(id) {
            if !message.is_some_and(Self::registers) {
                return events;
            }

            log::info!("Vehicle {id} registered");
            entry.insert(Arc::new(Vehicle::new(id)));
            events.push(FleetEvent::Registered(id));
        }

        let Some(vehicle) = self.vehicle_mut(id) else {
            return events;
        };

        // Liveness follows the command path: commands go to the resolved
        // autopilot, so once it is resolved only its frames keep the vehicle
        // alive. The gate is evaluated *before* `handle_message` below: a
        // frame from a component that establishes the autopilot (autopilot
        // still `None` here) still refreshes — the frame that names the
        // authority is itself on the command path.
        if vehicle.is_liveness_source(source.component) {
            vehicle.last_seen = Some(received_at);

            // A previously stale or lost vehicle is back as soon as a frame
            // from the liveness source arrives; a brand-new vehicle is already
            // `Active`, so it does not emit a recovery here.
            if vehicle.liveness != Liveness::Active {
                events.push(FleetEvent::LivenessChanged {
                    system: id,
                    from: vehicle.liveness,
                    to: Liveness::Active,
                });
                vehicle.liveness = Liveness::Active;
            }
        }

        let Some(message) = message else {
            return events;
        };

        let calibrating_before = is_calibrating(vehicle);
        let armed_before = vehicle.armed();
        let mode_before = vehicle.flight_mode();
        let had_heartbeat = vehicle.identity.base_mode.is_some();
        let stale_before = vehicle.mission.onboard_is_stale();

        if let Some(text) = vehicle.handle_message(received_at, source.component, message) {
            events.push(FleetEvent::StatusText(id, text));
        }

        if had_heartbeat {
            if let (Some(before), Some(after)) = (armed_before, vehicle.armed())
                && before != after
            {
                events.push(FleetEvent::ArmedChanged(id, after));
            }

            let mode_after = vehicle.flight_mode();
            if mode_after != mode_before {
                events.push(FleetEvent::ModeChanged(id, mode_after));
            }
        }

        if !stale_before && vehicle.mission.onboard_is_stale() {
            events.push(FleetEvent::OnboardMissionStale(id));
        }

        if calibrating_before {
            events.extend(calibration_finished(id, vehicle));
        }

        events
    }

    /// Age every vehicle and report liveness transitions, edge-triggered.
    ///
    /// Vehicles are never removed: that is a policy decision left to the
    /// application.
    pub(crate) fn tick(&mut self, now: Instant) -> Vec<FleetEvent> {
        let mut events = Vec::new();

        for (id, vehicle) in &mut self.vehicles {
            let age = now.saturating_duration_since(vehicle.last_seen.unwrap_or(now));
            let liveness = if age > LOST_AFTER {
                Liveness::Lost
            } else if age > STALE_AFTER {
                Liveness::Stale
            } else {
                Liveness::Active
            };

            if liveness != vehicle.liveness {
                let vehicle = Arc::make_mut(vehicle);
                events.push(FleetEvent::LivenessChanged {
                    system: *id,
                    from: vehicle.liveness,
                    to: liveness,
                });
                vehicle.liveness = liveness;

                if liveness == Liveness::Lost
                    && let Some(session) = vehicle.calibration.as_mut()
                    && session.is_running()
                {
                    session.fail("contact with the vehicle was lost");
                    events.extend(calibration_finished(*id, vehicle));
                }
            }
        }

        events
    }

    /// Record the mission a vehicle holds after a transfer. Returns whether
    /// the vehicle is known.
    pub fn set_onboard_mission(&mut self, id: SystemId, mission: OnboardMission) -> bool {
        match self.vehicle_mut(id) {
            Some(vehicle) => {
                vehicle.set_onboard_mission(mission);
                true
            }
            None => false,
        }
    }

    /// Apply what a calibration operation reports. Returns the events it
    /// caused; none for an unknown vehicle.
    pub(crate) fn update_calibration(
        &mut self,
        id: SystemId,
        update: CalibrationUpdate,
    ) -> Vec<FleetEvent> {
        let Some(vehicle) = self.vehicle_mut(id) else {
            return Vec::new();
        };

        // A run replaced by a new one is not reported as finished.
        let calibrating_before =
            is_calibrating(vehicle) && !matches!(update, CalibrationUpdate::Begin(_));
        if !vehicle.update_calibration(update) || !calibrating_before {
            return Vec::new();
        }

        calibration_finished(id, vehicle).into_iter().collect()
    }

    /// Record a parameter list download's progress. Returns whether the
    /// vehicle is known.
    pub(crate) fn set_param_download(&mut self, address: MavlinkId, state: DownloadState) -> bool {
        match self.vehicle_mut(address.system_id()) {
            Some(vehicle) => {
                vehicle.set_param_download(address.component, state);
                true
            }
            None => false,
        }
    }
}

fn is_calibrating(vehicle: &Vehicle) -> bool {
    vehicle
        .calibration
        .as_ref()
        .is_some_and(Session::is_running)
}

/// The event for a vehicle's calibration having finished, if it has.
fn calibration_finished(id: SystemId, vehicle: &Vehicle) -> Option<FleetEvent> {
    vehicle.calibration.as_ref().and_then(|session| {
        let outcome = session.phase.outcome()?;
        Some(FleetEvent::CalibrationFinished(id, session.kind, outcome))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::{
        enums::{MavModeFlag, MavState, MavType},
        messages::{self, Heartbeat},
    };

    const A: SystemId = SystemId(1);
    const B: SystemId = SystemId(2);

    /// The address of `id`'s autopilot endpoint, for feeding `observe`.
    fn source(id: SystemId) -> MavlinkId {
        MavlinkId {
            system: id.0,
            component: 1,
        }
    }

    fn heartbeat(autopilot: MavAutopilot) -> DefaultDialect {
        DefaultDialect::Heartbeat(Heartbeat {
            type_: MavType::Quadrotor,
            autopilot,
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
            ..Default::default()
        })
    }

    fn autopilot_heartbeat() -> DefaultDialect {
        heartbeat(MavAutopilot::Ardupilotmega)
    }

    /// A fleet with `id` registered by an autopilot heartbeat at `at`.
    fn register(fleet: &mut Fleet, id: SystemId, at: Instant) -> Vec<FleetEvent> {
        fleet.observe(source(id), at, Some(&autopilot_heartbeat()))
    }

    #[test]
    fn an_autopilot_heartbeat_registers_a_vehicle() {
        let mut fleet = Fleet::default();
        let events = register(&mut fleet, A, Instant::now());

        assert!(fleet.vehicle(A).is_some());
        assert!(matches!(events.as_slice(), [FleetEvent::Registered(A)]));
    }

    #[test]
    fn non_vehicles_never_register() {
        let mut fleet = Fleet::default();
        let now = Instant::now();

        // Another ground station.
        let gcs = DefaultDialect::Heartbeat(Heartbeat {
            type_: MavType::Gcs,
            autopilot: MavAutopilot::Invalid,
            ..Default::default()
        });
        assert!(
            fleet
                .observe(source(SystemId(255)), now, Some(&gcs))
                .is_empty()
        );

        // A telemetry radio reporting link quality, never a heartbeat.
        let radio = DefaultDialect::RadioStatus(messages::RadioStatus::default());
        assert!(
            fleet
                .observe(source(SystemId(51)), now, Some(&radio))
                .is_empty()
        );

        // A companion computer heartbeating before its autopilot.
        let companion = DefaultDialect::Heartbeat(Heartbeat {
            type_: MavType::OnboardController,
            autopilot: MavAutopilot::Invalid,
            ..Default::default()
        });
        assert!(fleet.observe(source(A), now, Some(&companion)).is_empty());

        // Undecodable frames.
        assert!(fleet.observe(source(B), now, None).is_empty());

        assert!(fleet.is_empty());
    }

    #[test]
    fn heartbeat_updates_vehicle_state() {
        let mut fleet = Fleet::default();
        let heartbeat = DefaultDialect::Heartbeat(Heartbeat {
            custom_mode: 7,
            type_: MavType::Quadrotor,
            autopilot: MavAutopilot::Ardupilotmega,
            ..Default::default()
        });

        fleet.observe(source(A), Instant::now(), Some(&heartbeat));
        assert_eq!(fleet.vehicle(A).unwrap().identity.custom_mode, 7);
    }

    #[test]
    fn components_of_one_system_share_a_vehicle() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        register(&mut fleet, A, t0);

        fleet.observe(
            MavlinkId {
                system: 1,
                component: 191,
            },
            t0,
            None,
        );

        assert_eq!(fleet.len(), 1);
    }

    #[test]
    fn component_one_takes_precedence_over_another_autopilot_candidate() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        let at = |component| MavlinkId {
            system: 1,
            component,
        };

        // A non-1 component may claim the role while nothing else has.
        fleet.observe(at(10), t0, Some(&autopilot_heartbeat()));
        assert_eq!(fleet.vehicle(A).unwrap().autopilot_component(), Some(10));

        // A valid component-1 heartbeat displaces it...
        fleet.observe(at(1), t0, Some(&autopilot_heartbeat()));
        assert_eq!(fleet.vehicle(A).unwrap().autopilot_component(), Some(1));

        // ...and a later non-1 candidate cannot reclaim it.
        fleet.observe(at(10), t0, Some(&autopilot_heartbeat()));
        assert_eq!(fleet.vehicle(A).unwrap().autopilot_component(), Some(1));
    }

    #[test]
    fn peripheral_heartbeat_does_not_overwrite_identity() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();

        let autopilot = DefaultDialect::Heartbeat(Heartbeat {
            type_: MavType::Quadrotor,
            autopilot: MavAutopilot::Ardupilotmega,
            system_status: MavState::Active,
            ..Default::default()
        });
        fleet.observe(source(A), t0, Some(&autopilot));

        // A gimbal heartbeat must not clobber the vehicle's identity.
        let gimbal = DefaultDialect::Heartbeat(Heartbeat {
            type_: MavType::Gimbal,
            autopilot: MavAutopilot::Invalid,
            system_status: MavState::Standby,
            ..Default::default()
        });
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 154,
            },
            t0,
            Some(&gimbal),
        );

        let vehicle = fleet.vehicle(A).unwrap();
        assert_eq!(vehicle.identity.mav_type, Some(MavType::Quadrotor));
        assert_eq!(vehicle.identity.mav_state, Some(MavState::Active));
        // The peripheral still registers as a component of the system.
        assert!(vehicle.component_ids().contains(&154));
    }

    #[test]
    fn vehicles_are_named_even_when_unknown() {
        let mut fleet = Fleet::default();
        register(&mut fleet, A, Instant::now());
        let snapshot = fleet.snapshot();

        let known = snapshot.vehicle(A).unwrap().display_name();
        assert_eq!(snapshot.vehicle_name(A), known);
        assert_eq!(snapshot.vehicle_name(SystemId(9)), "vehicle 9");
    }

    #[test]
    fn tick_transitions_stale_then_lost_edge_triggered() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        register(&mut fleet, A, t0);

        // Within the active window: nothing changes.
        assert!(fleet.tick(t0 + Duration::from_secs(1)).is_empty());

        // Past the stale threshold: exactly one transition.
        let stale = fleet.tick(t0 + Duration::from_secs(5));
        assert!(matches!(
            stale.as_slice(),
            [FleetEvent::LivenessChanged {
                system: A,
                from: Liveness::Active,
                to: Liveness::Stale
            }]
        ));

        // Still stale: no repeated event.
        assert!(fleet.tick(t0 + Duration::from_secs(6)).is_empty());

        // Past the lost threshold: exactly one transition.
        let lost = fleet.tick(t0 + Duration::from_secs(11));
        assert!(matches!(
            lost.as_slice(),
            [FleetEvent::LivenessChanged {
                system: A,
                from: Liveness::Stale,
                to: Liveness::Lost
            }]
        ));

        // And it stays lost without further events.
        assert!(fleet.tick(t0 + Duration::from_secs(20)).is_empty());
    }

    #[test]
    fn observing_a_lost_vehicle_recovers_it() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        register(&mut fleet, A, t0);
        fleet.tick(t0 + Duration::from_secs(11));
        assert_eq!(fleet.vehicle(A).unwrap().liveness, Liveness::Lost);

        let events = fleet.observe(source(A), t0 + Duration::from_secs(11), None);

        assert_eq!(fleet.vehicle(A).unwrap().liveness, Liveness::Active);
        assert!(events.contains(&FleetEvent::LivenessChanged {
            system: A,
            from: Liveness::Lost,
            to: Liveness::Active
        }));
    }

    #[test]
    fn companion_frames_do_not_restore_a_resolved_vehicle() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        register(&mut fleet, A, t0);
        fleet.tick(t0 + Duration::from_secs(5));

        let companion = MavlinkId {
            system: 1,
            component: 191,
        };
        let events = fleet.observe(companion, t0 + Duration::from_secs(6), None);
        assert_eq!(fleet.vehicle(A).unwrap().liveness, Liveness::Stale);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FleetEvent::LivenessChanged { .. }))
        );

        let events = fleet.observe(source(A), t0 + Duration::from_secs(7), None);
        assert!(events.contains(&FleetEvent::LivenessChanged {
            system: A,
            from: Liveness::Stale,
            to: Liveness::Active
        }));
    }

    #[test]
    fn arming_and_mode_changes_are_reported() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        register(&mut fleet, A, t0);

        let armed_auto = DefaultDialect::Heartbeat(Heartbeat {
            type_: MavType::Quadrotor,
            autopilot: MavAutopilot::Ardupilotmega,
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED | MavModeFlag::SAFETY_ARMED,
            custom_mode: 3,
            ..Default::default()
        });
        let events = fleet.observe(source(A), t0, Some(&armed_auto));

        assert!(events.contains(&FleetEvent::ArmedChanged(A, true)));
        assert!(events.iter().any(|event| matches!(
            event,
            FleetEvent::ModeChanged(A, Some(mode)) if mode.name == "Auto"
        )));

        // The same heartbeat again changes nothing.
        assert!(fleet.observe(source(A), t0, Some(&armed_auto)).is_empty());
    }

    #[test]
    fn status_texts_are_reported() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        register(&mut fleet, A, t0);

        let mut text = [0u8; 50];
        text[..6].copy_from_slice(b"PreArm");
        let events = fleet.observe(
            source(A),
            t0,
            Some(&DefaultDialect::Statustext(messages::Statustext {
                text,
                ..Default::default()
            })),
        );

        assert!(matches!(
            events.as_slice(),
            [FleetEvent::StatusText(A, status)] if status.text == "PreArm"
        ));
    }

    #[test]
    fn snapshots_are_isolated_from_later_changes() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        register(&mut fleet, A, t0);

        let snapshot = fleet.snapshot();
        register(&mut fleet, B, t0);

        assert_eq!(snapshot.vehicles().count(), 1);
        assert_eq!(fleet.snapshot().vehicles().count(), 2);
    }

    #[test]
    fn mission_folds_ignore_unknown_vehicles() {
        let mut fleet = Fleet::default();
        assert!(!fleet.set_onboard_mission(A, OnboardMission::empty()));

        register(&mut fleet, A, Instant::now());
        assert!(fleet.set_onboard_mission(A, OnboardMission::empty()));
        assert!(fleet.vehicle(A).unwrap().mission.onboard.is_some());
    }
}
