//! The fleet: every vehicle the GCS has seen, and which one is primary.
//!
//! This is the single owner of vehicle state. The UI layer feeds inbound
//! messages in through [`Fleet::observe`] and turns the resulting
//! [`FleetEvent`]s into effects (initialising a vehicle, recentering the map).

use std::{
    collections::{BTreeMap, btree_map::Entry},
    time::{Duration, Instant},
};

use mavio::DefaultDialect;
use slippery::Mercator;

use crate::{
    domain::vehicle::{Liveness, Vehicle},
    foundation::MavlinkId,
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
    Registered(MavlinkId),
    /// A vehicle's liveness changed, as of a [`Fleet::tick`] or a fresh frame.
    LivenessChanged(MavlinkId, Liveness),
    /// The primary vehicle's latest position, after this frame was applied.
    /// Whether to recenter is the application's policy, not the fleet's.
    PrimaryMoved(Mercator),
}

/// Every vehicle the GCS has seen, and which one is primary.
///
/// `Fleet` is the single owner of [`Vehicle`] state. The vehicle map is private
/// and the parameter/edit state is encapsulated behind methods; all mutation of
/// vehicle state flows through `Fleet` and `Vehicle`. Vehicles still expose
/// many read-only fields for the view layer to render.
#[derive(Default)]
pub struct Fleet {
    vehicles: BTreeMap<MavlinkId, Vehicle>,
    primary: Option<MavlinkId>,
}

impl Fleet {
    pub fn vehicles(&self) -> &BTreeMap<MavlinkId, Vehicle> {
        &self.vehicles
    }

    pub fn vehicle(&self, id: MavlinkId) -> Option<&Vehicle> {
        self.vehicles.get(&id)
    }

    pub fn vehicle_mut(&mut self, id: MavlinkId) -> Option<&mut Vehicle> {
        self.vehicles.get_mut(&id)
    }

    pub fn primary(&self) -> Option<MavlinkId> {
        self.primary
    }

    /// Make `id` the primary vehicle, if it is known. Returns whether it changed.
    pub fn set_primary(&mut self, id: MavlinkId) -> bool {
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
    pub fn observe(
        &mut self,
        source: MavlinkId,
        received_at: Instant,
        message: Option<DefaultDialect>,
    ) -> Vec<FleetEvent> {
        let mut events = Vec::new();

        if let Entry::Vacant(entry) = self.vehicles.entry(source) {
            entry.insert(Vehicle::new(source));
            events.push(FleetEvent::Registered(source));

            if self.primary.is_none() {
                self.primary = Some(source);
            }
        }

        if let Some(vehicle) = self.vehicles.get_mut(&source) {
            vehicle.last_seen = Some(received_at);

            // A previously stale or lost vehicle is back as soon as any frame
            // arrives; a brand-new vehicle is already `Active`, so it does not
            // emit a recovery here.
            if vehicle.liveness != Liveness::Active {
                vehicle.liveness = Liveness::Active;
                events.push(FleetEvent::LivenessChanged(source, Liveness::Active));
            }

            if let Some(message) = message {
                vehicle.handle_message(received_at, message);
            }
        }

        if self.primary == Some(source)
            && let Some(position) = self
                .vehicles
                .get(&source)
                .and_then(|v| v.global_position.as_ref())
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
    use mavio::default_dialect::messages::Heartbeat;

    const A: MavlinkId = MavlinkId {
        system: 1,
        component: 1,
    };
    const B: MavlinkId = MavlinkId {
        system: 2,
        component: 1,
    };

    #[test]
    fn first_vehicle_becomes_primary() {
        let mut fleet = Fleet::default();
        let events = fleet.observe(A, Instant::now(), None);

        assert!(fleet.vehicles().contains_key(&A));
        assert_eq!(fleet.primary(), Some(A));
        assert!(matches!(events.as_slice(), [FleetEvent::Registered(A)]));
    }

    #[test]
    fn unknown_message_still_registers() {
        let mut fleet = Fleet::default();
        let events = fleet.observe(B, Instant::now(), None);

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
        fleet.observe(A, Instant::now(), None);
        let events = fleet.observe(B, Instant::now(), None);

        assert_eq!(fleet.primary(), Some(A));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], FleetEvent::Registered(B)));
    }

    #[test]
    fn set_primary_switches_once() {
        let mut fleet = Fleet::default();
        fleet.observe(A, Instant::now(), None);
        fleet.observe(B, Instant::now(), None);

        assert!(fleet.set_primary(B));
        assert_eq!(fleet.primary(), Some(B));
        // Setting the same primary again is a no-op.
        assert!(!fleet.set_primary(B));
    }

    #[test]
    fn set_primary_ignores_unknown_vehicle() {
        let mut fleet = Fleet::default();
        fleet.observe(A, Instant::now(), None);

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

        fleet.observe(A, Instant::now(), Some(heartbeat));
        assert_eq!(fleet.vehicles()[&A].custom_mode, 7);
    }

    #[test]
    fn newly_seen_vehicle_is_active() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        let events = fleet.observe(A, t0, None);

        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Active);
        assert_eq!(fleet.vehicles()[&A].last_seen, Some(t0));
        assert!(matches!(events.as_slice(), [FleetEvent::Registered(A)]));
    }

    #[test]
    fn tick_transitions_stale_then_lost_edge_triggered() {
        let mut fleet = Fleet::default();
        let t0 = Instant::now();
        fleet.observe(A, t0, None);

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
        fleet.observe(A, t0, None);
        fleet.tick(t0 + Duration::from_secs(5));
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Stale);

        let events = fleet.observe(A, t0 + Duration::from_secs(5), None);

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
        fleet.observe(A, t0, None);
        fleet.tick(t0 + Duration::from_secs(11));
        assert_eq!(fleet.vehicles()[&A].liveness, Liveness::Lost);

        let events = fleet.observe(A, t0 + Duration::from_secs(11), None);

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

        let events = fleet.observe(A, t0, None);
        let fleet_events = fleet.tick(t0);

        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FleetEvent::LivenessChanged(..)))
        );
        assert!(fleet_events.is_empty());
    }
}
