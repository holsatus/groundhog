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
            vehicle.last_seen = Some(received_at);

            // A previously stale or lost vehicle is back as soon as any frame
            // arrives; a brand-new vehicle is already `Active`, so it does not
            // emit a recovery here.
            if vehicle.liveness != Liveness::Active {
                vehicle.liveness = Liveness::Active;
                events.push(FleetEvent::LivenessChanged(id, Liveness::Active));
            }

            if let Some(message) = message {
                vehicle.handle_message(received_at, source.component, message);
            }
        }

        if is_position_frame
            && self.primary == Some(id)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::{
        enums::MavAutopilot,
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
}
