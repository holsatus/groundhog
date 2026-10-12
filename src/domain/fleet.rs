//! The fleet: every vehicle the GCS has seen, and which one is primary.
//!
//! This is the single owner of vehicle state. The UI layer feeds inbound
//! messages in through [`Fleet::observe`] and turns the resulting
//! [`FleetEvent`]s into effects (initialising a vehicle, recentering the map).
//!
//! Identity is per MAVLink *system* ([`SystemId`]), not per address: a system
//! is a vehicle, and the several components that may heartbeat under it
//! (autopilot, gimbal, companion computer, ...) are endpoints of that one
//! vehicle rather than vehicles of their own. Inbound frames still arrive
//! addressed to a `(system, component)` pair, so [`Fleet::observe`] keys on the
//! system while preserving each frame's source component for targeting.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    time::{Duration, Instant},
};

use mavio::DefaultDialect;

use crate::{
    domain::geo::GeoPosition,
    domain::mission::MissionPlan,
    domain::vehicle::{Liveness, Vehicle},
    foundation::{MavlinkId, SystemId},
};

/// A vehicle still counts as active while it has been heard from within this
/// window. MAVLink heartbeats are nominally 1 Hz, so this tolerates a couple of
/// missed beats before flagging the link.
const STALE_AFTER: Duration = Duration::from_secs(3);

/// Silence this long means the vehicle is presumed lost.
const LOST_AFTER: Duration = Duration::from_secs(10);

/// Something the fleet observed that the application may need to react to.
#[derive(Debug, Clone)]
pub enum FleetEvent {
    /// A vehicle was seen for the first time.
    Registered(SystemId),
    /// A vehicle's liveness changed, as of a [`Fleet::tick`] or a fresh frame.
    LivenessChanged(SystemId, Liveness),
    /// The primary vehicle's latest position, after this frame was applied.
    /// Whether to recenter is the application's policy, not the fleet's.
    PrimaryMoved(GeoPosition),
}

/// Every vehicle the GCS has seen, and which one is primary.
///
/// `Fleet` is the single owner of [`Vehicle`] state. The vehicle map is private
/// and the parameter/edit state is encapsulated behind methods; all mutation of
/// vehicle state flows through `Fleet` and `Vehicle`. Vehicles still expose
/// many read-only fields for the view layer to render.
#[derive(Default)]
pub struct Fleet {
    vehicles: BTreeMap<SystemId, Vehicle>,
    primary: Option<SystemId>,
}

impl Fleet {
    pub fn vehicles(&self) -> &BTreeMap<SystemId, Vehicle> {
        &self.vehicles
    }

    pub fn vehicle(&self, id: SystemId) -> Option<&Vehicle> {
        self.vehicles.get(&id)
    }

    pub fn vehicle_mut(&mut self, id: SystemId) -> Option<&mut Vehicle> {
        self.vehicles.get_mut(&id)
    }

    pub fn primary(&self) -> Option<SystemId> {
        self.primary
    }

    /// Make `id` the primary vehicle, if it is known. Returns whether it changed.
    pub fn set_primary(&mut self, id: SystemId) -> bool {
        if self.vehicles.contains_key(&id) && self.primary != Some(id) {
            self.primary = Some(id);
            true
        } else {
            false
        }
    }

    /// Fold one inbound frame into vehicle state, returning what changed.
    ///
    /// A vehicle is registered as soon as any frame is seen from it, even if
    /// the frame does not decode (`message` is `None`). The first vehicle seen
    /// becomes the primary.
    ///
    /// `source.component` is the endpoint that sent this frame; it is passed
    /// through so the vehicle can resolve which component is its autopilot.
    pub fn observe(
        &mut self,
        source: MavlinkId,
        received_at: Instant,
        message: Option<DefaultDialect>,
    ) -> Vec<FleetEvent> {
        let mut events = Vec::new();

        // A vehicle is a system: frames from any of its components fold into
        // one entry keyed by the system.
        let id = SystemId::from(source.system);

        // Only a fresh position frame can move the primary vehicle; other
        // frames (heartbeats, status, ...) leave the stored position untouched.
        let is_position_frame = matches!(
            message.as_ref(),
            Some(DefaultDialect::GlobalPositionInt(_) | DefaultDialect::GlobalPositionIntCov(_))
        );

        if let Entry::Vacant(entry) = self.vehicles.entry(id) {
            entry.insert(Vehicle::new(id));
            events.push(FleetEvent::Registered(id));

            if self.primary.is_none() {
                self.primary = Some(id);
            }
        }

        if let Some(vehicle) = self.vehicles.get_mut(&id) {
            // Liveness follows the command path: commands go to the resolved
            // autopilot, so once it is resolved only its frames keep the
            // vehicle alive. The gate is evaluated *before* `handle_message`
            // below: a frame from a component that establishes the autopilot
            // (autopilot still `None` here) still refreshes — the frame that
            // names the authority is itself on the command path.
            if vehicle.is_liveness_source(source.component) {
                vehicle.last_seen = Some(received_at);

                // A previously stale or lost vehicle is back as soon as a
                // frame from the liveness source arrives; a brand-new vehicle
                // is already `Active`, so it does not emit a recovery here.
                if vehicle.liveness != Liveness::Active {
                    vehicle.liveness = Liveness::Active;
                    events.push(FleetEvent::LivenessChanged(id, Liveness::Active));
                }
            }

            if let Some(message) = message {
                vehicle.handle_message(received_at, source.component, message);
            }
        }

        if is_position_frame
            && self.primary == Some(id)
            && self
                .vehicles
                .get(&id)
                .is_some_and(|v| v.is_authoritative(source.component))
            && let Some(position) = self
                .vehicles
                .get(&id)
                .and_then(|v| v.position.current.as_ref())
        {
            events.push(FleetEvent::PrimaryMoved(position.pos));
        }

        events
    }

    /// Age every vehicle and report liveness transitions, edge-triggered.
    ///
    /// Vehicles are never removed and the primary is never changed: those are
    /// policy decisions left to the application.
    pub fn tick(&mut self, now: Instant) -> Vec<FleetEvent> {
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
                vehicle.liveness = liveness;
                events.push(FleetEvent::LivenessChanged(*id, liveness));
            }
        }

        events
    }

    /// Install a mission the vehicle handed over during a download.
    ///
    /// The adapter (`services::vehicle`) decodes the wire items into a plan;
    /// what the vehicle handed over is, by definition, what the vehicle has, so
    /// this fold also records the plan as synced under `vehicle_id`. Returns
    /// whether the fold landed; an unknown vehicle leaves everything untouched.
    pub fn apply_mission_download(
        &mut self,
        id: SystemId,
        plan: MissionPlan,
        vehicle_id: u32,
    ) -> bool {
        let Some(vehicle) = self.vehicles.get_mut(&id) else {
            return false;
        };

        vehicle.mission = plan;
        vehicle.mission_sync.mark_synced(Some(vehicle_id));
        true
    }

    /// The vehicle accepted the uploaded plan and reports it under the opaque
    /// `vehicle_id`: record the working plan as synced with the vehicle under
    /// that id. Returns whether the fold landed.
    pub fn apply_mission_upload(&mut self, id: SystemId, vehicle_id: u32) -> bool {
        let Some(vehicle) = self.vehicles.get_mut(&id) else {
            return false;
        };

        vehicle.mission_sync.mark_synced(Some(vehicle_id));
        true
    }

    /// The vehicle cleared its plan: empty the working plan and record it as
    /// unsynced. Returns whether the fold landed.
    pub fn apply_mission_clear(&mut self, id: SystemId) -> bool {
        let Some(vehicle) = self.vehicles.get_mut(&id) else {
            return false;
        };

        vehicle.mission.clear();
        vehicle.mission_sync.mark_synced(None);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::{
        enums::{MavAutopilot, MavFrame, MavState, MavType},
        messages::{self, Heartbeat},
    };

    use crate::domain::mission::{
        MissionCommand, MissionCommandKind, MissionItem, MissionPlan, Position,
    };

    const A: SystemId = SystemId(1);
    const B: SystemId = SystemId(2);

    /// Push one waypoint onto `id`'s working mission and mark it dirty, as a
    /// user edit would.
    fn push_waypoint(fleet: &mut Fleet, id: SystemId) {
        let vehicle = fleet.vehicle_mut(id).unwrap();
        vehicle.mission.push(MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command: MissionCommand::default_for(
                MissionCommandKind::Waypoint,
                Position {
                    lat: 47.0,
                    lon: 8.0,
                    alt_m: 50.0,
                },
            ),
        });
        vehicle.mission_sync.mark_dirty();
    }

    /// A plan shaped like one an in-flight download hands back, with the opaque
    /// id the vehicle reports for it.
    fn synced_mission(vehicle_id: u32) -> (MissionPlan, u32) {
        let mut plan = MissionPlan::default();
        plan.push(MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command: MissionCommand::default_for(
                MissionCommandKind::Waypoint,
                Position {
                    lat: 47.0,
                    lon: 8.0,
                    alt_m: 100.0,
                },
            ),
        });
        (plan, vehicle_id)
    }

    /// The address of `id`'s autopilot endpoint, for feeding `observe`.
    fn source(id: SystemId) -> MavlinkId {
        MavlinkId {
            system: id.0,
            component: 1,
        }
    }

    fn heartbeat(autopilot: MavAutopilot) -> DefaultDialect {
        DefaultDialect::Heartbeat(Heartbeat {
            autopilot,
            ..Default::default()
        })
    }

    fn global_position_int(lat: i32, lon: i32) -> DefaultDialect {
        DefaultDialect::GlobalPositionInt(messages::GlobalPositionInt {
            time_boot_ms: 0,
            lat,
            lon,
            alt: 0,
            relative_alt: 0,
            vx: 0,
            vy: 0,
            vz: 0,
            hdg: 0,
        })
    }

    #[test]
    fn first_vehicle_becomes_primary() {
        let mut fleet = Fleet::default();
        let events = fleet.observe(source(A), Instant::now(), None);

        assert!(fleet.vehicles().contains_key(&A));
        assert_eq!(fleet.primary(), Some(A));
        assert!(matches!(events.as_slice(), [FleetEvent::Registered(A)]));
    }

    #[test]
    fn unknown_message_still_registers() {
        let mut fleet = Fleet::default();
        let events = fleet.observe(source(B), Instant::now(), None);

        assert!(fleet.vehicles().contains_key(&B));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, FleetEvent::Registered(B)))
        );
    }

    #[test]
    fn second_vehicle_does_not_become_primary() {
        let mut fleet = Fleet::default();
        fleet.observe(source(A), Instant::now(), None);
        let events = fleet.observe(source(B), Instant::now(), None);

        assert_eq!(fleet.primary(), Some(A));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], FleetEvent::Registered(B)));
    }

    #[test]
    fn set_primary_switches_once() {
        let mut fleet = Fleet::default();
        fleet.observe(source(A), Instant::now(), None);
        fleet.observe(source(B), Instant::now(), None);

        assert!(fleet.set_primary(B));
        assert_eq!(fleet.primary(), Some(B));
        // Setting the same primary again is a no-op.
        assert!(!fleet.set_primary(B));
    }

    #[test]
    fn set_primary_ignores_unknown_vehicle() {
        let mut fleet = Fleet::default();
        fleet.observe(source(A), Instant::now(), None);

        assert!(!fleet.set_primary(B));
        assert_eq!(fleet.primary(), Some(A));
    }

    #[test]
    fn heartbeat_updates_vehicle_state() {
        let mut fleet = Fleet::default();
        let heartbeat = DefaultDialect::Heartbeat(Heartbeat {
            custom_mode: 7,
            ..Default::default()
        });

        fleet.observe(source(A), Instant::now(), Some(heartbeat));
        assert_eq!(fleet.vehicles()[&A].identity.custom_mode, 7);
    }

    #[test]
    fn components_of_one_system_share_a_vehicle() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();

        // Autopilot and companion computer heartbeat under the same system.
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 1,
            },
            t0,
            None,
        );
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 191,
            },
            t0,
            None,
        );

        assert_eq!(fleet.vehicles().len(), 1);
        assert!(fleet.vehicles().contains_key(&SystemId(1)));
    }

    #[test]
    fn heartbeat_resolves_the_autopilot_component_for_targeting() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();

        // A companion endpoint that reports no valid autopilot must not become
        // the target; the fallback autopilot component is used instead.
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 191,
            },
            t0,
            Some(heartbeat(MavAutopilot::Invalid)),
        );
        assert_eq!(fleet.vehicle(SystemId(1)).unwrap().target().component, 1);

        // The autopilot's own heartbeat resolves the target.
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 1,
            },
            t0,
            Some(heartbeat(MavAutopilot::Ardupilotmega)),
        );
        assert_eq!(fleet.vehicle(SystemId(1)).unwrap().target().component, 1);
    }

    #[test]
    fn component_one_takes_precedence_over_another_autopilot_candidate() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();

        // A non-1 component may claim the role while nothing else has.
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 10,
            },
            t0,
            Some(heartbeat(MavAutopilot::Ardupilotmega)),
        );
        assert_eq!(
            fleet.vehicle(SystemId(1)).unwrap().autopilot_component(),
            Some(10)
        );

        // A valid component-1 heartbeat displaces it...
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 1,
            },
            t0,
            Some(heartbeat(MavAutopilot::Ardupilotmega)),
        );
        assert_eq!(
            fleet.vehicle(SystemId(1)).unwrap().autopilot_component(),
            Some(1)
        );

        // ...and a later non-1 candidate cannot reclaim it.
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 10,
            },
            t0,
            Some(heartbeat(MavAutopilot::Ardupilotmega)),
        );
        assert_eq!(
            fleet.vehicle(SystemId(1)).unwrap().autopilot_component(),
            Some(1)
        );
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
        fleet.observe(
            MavlinkId {
                system: 1,
                component: 1,
            },
            t0,
            Some(autopilot),
        );

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
            Some(gimbal),
        );

        let vehicle = fleet.vehicle(SystemId(1)).unwrap();
        assert_eq!(vehicle.identity.mav_type, Some(MavType::Quadrotor));
        assert_eq!(vehicle.identity.mav_state, Some(MavState::Active));
        // The peripheral still registers as a component of the system.
        assert!(vehicle.component_ids().contains(&154));
    }

    #[test]
    fn newly_seen_vehicle_is_active() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        let events = fleet.observe(source(A), t0, None);

        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Active);
        assert_eq!(fleet.vehicles()[&A].last_seen, Some(t0));
        assert!(matches!(events.as_slice(), [FleetEvent::Registered(A)]));
    }

    #[test]
    fn tick_transitions_stale_then_lost_edge_triggered() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        fleet.observe(source(A), t0, None);

        // Within the active window: nothing changes.
        assert!(fleet.tick(t0 + Duration::from_secs(1)).is_empty());

        // Past the stale threshold: exactly one transition.
        let stale = fleet.tick(t0 + Duration::from_secs(5));
        assert!(matches!(
            stale.as_slice(),
            [FleetEvent::LivenessChanged(A, Liveness::Stale)]
        ));
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Stale);

        // Still stale: no repeated event.
        assert!(fleet.tick(t0 + Duration::from_secs(6)).is_empty());

        // Past the lost threshold: exactly one transition.
        let lost = fleet.tick(t0 + Duration::from_secs(11));
        assert!(matches!(
            lost.as_slice(),
            [FleetEvent::LivenessChanged(A, Liveness::Lost)]
        ));
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Lost);

        // And it stays lost without further events.
        assert!(fleet.tick(t0 + Duration::from_secs(20)).is_empty());
    }

    #[test]
    fn observing_stale_vehicle_recovers_to_active() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        fleet.observe(source(A), t0, None);
        fleet.tick(t0 + Duration::from_secs(5));
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Stale);

        let events = fleet.observe(source(A), t0 + Duration::from_secs(5), None);

        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Active);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FleetEvent::LivenessChanged(A, Liveness::Active)))
        );
    }

    #[test]
    fn observing_lost_vehicle_recovers_to_active() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        fleet.observe(source(A), t0, None);
        fleet.tick(t0 + Duration::from_secs(11));
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Lost);

        let events = fleet.observe(source(A), t0 + Duration::from_secs(11), None);

        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Active);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FleetEvent::LivenessChanged(A, Liveness::Active)))
        );
    }

    #[test]
    fn companion_frames_do_not_restore_a_resolved_vehicle() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();

        // Resolve the autopilot with a heartbeat naming a real autopilot.
        fleet.observe(source(A), t0, Some(heartbeat(MavAutopilot::Ardupilotmega)));
        assert_eq!(fleet.vehicle(A).unwrap().autopilot_component(), Some(1));
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Active);

        fleet.tick(t0 + Duration::from_secs(5));
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Stale);

        // A companion's frames cannot restore it: commands would go to the
        // autopilot, so its silence still means lost contact.
        let companion = MavlinkId {
            system: 1,
            component: 191,
        };
        let events = fleet.observe(companion, t0 + Duration::from_secs(6), None);
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Stale);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FleetEvent::LivenessChanged(A, Liveness::Active)))
        );

        // The autopilot's own frame recovers it.
        let events = fleet.observe(source(A), t0 + Duration::from_secs(7), None);
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Active);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FleetEvent::LivenessChanged(A, Liveness::Active)))
        );
    }

    #[test]
    fn companion_frames_keep_an_unresolved_system_alive() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();

        // Only a companion has been heard, naming no valid autopilot: the
        // system is not resolved yet, so any component keeps it alive.
        let companion = MavlinkId {
            system: 1,
            component: 191,
        };
        fleet.observe(companion, t0, Some(heartbeat(MavAutopilot::Invalid)));
        assert_eq!(fleet.vehicle(A).unwrap().autopilot_component(), None);

        let stale = fleet.tick(t0 + Duration::from_secs(5));
        assert!(matches!(
            stale.as_slice(),
            [FleetEvent::LivenessChanged(A, Liveness::Stale)]
        ));

        let events = fleet.observe(companion, t0 + Duration::from_secs(6), None);
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Active);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FleetEvent::LivenessChanged(A, Liveness::Active)))
        );
    }

    #[test]
    fn new_vehicle_does_not_emit_liveness_event() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();

        let events = fleet.observe(source(A), t0, None);
        let fleet_events = fleet.tick(t0);

        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FleetEvent::LivenessChanged(..)))
        );
        assert!(fleet_events.is_empty());
    }

    #[test]
    fn non_position_frame_does_not_emit_primary_moved() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        // Give the primary vehicle a recorded position.
        fleet.observe(
            source(A),
            t0,
            Some(global_position_int(474_000_000, 85_000_000)),
        );

        let events = fleet.observe(
            source(A),
            t0,
            Some(DefaultDialect::Heartbeat(Heartbeat {
                custom_mode: 0,
                ..Default::default()
            })),
        );

        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FleetEvent::PrimaryMoved(..)))
        );
    }

    #[test]
    fn position_frame_emits_primary_moved() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        // Register without a message so no position exists yet.
        fleet.observe(source(A), t0, None);

        let events = fleet.observe(
            source(A),
            t0,
            Some(global_position_int(474_000_000, 85_000_000)),
        );

        let moved = events
            .iter()
            .find_map(|event| match event {
                FleetEvent::PrimaryMoved(position) => Some(*position),
                _ => None,
            })
            .expect("a position frame for the primary should emit PrimaryMoved");

        assert!((moved.lat - 47.4).abs() < 1e-7, "lat was {}", moved.lat);
        assert!((moved.lon - 8.5).abs() < 1e-7, "lon was {}", moved.lon);
    }

    #[test]
    fn peripheral_position_frame_is_ignored() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        // Give the autopilot (component 1) a recorded position.
        fleet.observe(
            source(A),
            t0,
            Some(global_position_int(474_000_000, 85_000_000)),
        );

        let before = {
            let pos = fleet
                .vehicle(A)
                .unwrap()
                .position
                .current
                .as_ref()
                .unwrap()
                .pos;
            (pos.lat, pos.lon)
        };

        // A peripheral position frame must be ignored for vehicle state...
        let events = fleet.observe(
            MavlinkId {
                system: 1,
                component: 191,
            },
            t0,
            Some(global_position_int(0, 0)),
        );

        let after = {
            let pos = fleet
                .vehicle(A)
                .unwrap()
                .position
                .current
                .as_ref()
                .unwrap()
                .pos;
            (pos.lat, pos.lon)
        };
        assert!(
            (after.0 - before.0).abs() < 1e-12,
            "lat changed to {}",
            after.0
        );
        assert!(
            (after.1 - before.1).abs() < 1e-12,
            "lon changed to {}",
            after.1
        );

        // ...and must not emit PrimaryMoved.
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FleetEvent::PrimaryMoved(..)))
        );
    }

    #[test]
    fn apply_mission_download_installs_the_adapter_plan() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        fleet.observe(source(A), t0, None);

        // Setup: a dirty working plan awaiting the download.
        push_waypoint(&mut fleet, A);
        assert!(fleet.vehicle(A).unwrap().mission_sync.dirty);

        // The adapter hands over what the vehicle already has, under id `5`.
        let (plan, vehicle_id) = synced_mission(5);

        assert!(fleet.apply_mission_download(A, plan, vehicle_id));

        let vehicle = fleet.vehicle(A).unwrap();
        assert_eq!(vehicle.mission.len(), 1);
        assert!(!vehicle.mission_sync.dirty);
        assert_eq!(vehicle.mission_sync.vehicle_id, Some(5));
    }

    #[test]
    fn apply_mission_upload_marks_the_working_plan_synced() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        fleet.observe(source(A), t0, None);

        push_waypoint(&mut fleet, A);
        assert!(fleet.vehicle(A).unwrap().mission_sync.dirty);

        // The vehicle accepted the plan and reports it under `7`.
        assert!(fleet.apply_mission_upload(A, 7));

        let vehicle = fleet.vehicle(A).unwrap();
        assert_eq!(vehicle.mission.len(), 1, "the working plan is kept");
        assert!(!vehicle.mission_sync.dirty);
        assert_eq!(vehicle.mission_sync.vehicle_id, Some(7));
    }

    #[test]
    fn apply_mission_clear_empties_and_unsyncs() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        fleet.observe(source(A), t0, None);

        push_waypoint(&mut fleet, A);
        assert!(fleet.apply_mission_upload(A, 3));
        assert_eq!(fleet.vehicle(A).unwrap().mission_sync.vehicle_id, Some(3));

        assert!(fleet.apply_mission_clear(A));

        let vehicle = fleet.vehicle(A).unwrap();
        assert_eq!(vehicle.mission.len(), 0);
        assert!(!vehicle.mission_sync.dirty);
        assert_eq!(vehicle.mission_sync.vehicle_id, None);
    }

    #[test]
    fn mission_folds_ignore_unknown_vehicles() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        fleet.observe(source(A), t0, None);

        push_waypoint(&mut fleet, A);
        assert!(fleet.apply_mission_upload(A, 3));

        let (plan, vehicle_id) = synced_mission(9);
        assert!(!fleet.apply_mission_download(B, plan, vehicle_id));
        assert!(!fleet.apply_mission_upload(B, 9));
        assert!(!fleet.apply_mission_clear(B));

        // Every fold on an unknown vehicle leaves the known one untouched.
        let vehicle = fleet.vehicle(A).unwrap();
        assert_eq!(vehicle.mission.len(), 1);
        assert!(!vehicle.mission_sync.dirty);
        assert_eq!(vehicle.mission_sync.vehicle_id, Some(3));
    }
}
