//! The fleet: every vehicle the GCS has seen, and which one is primary.
//!
//! This is the single owner of vehicle state. The UI layer feeds inbound
//! messages in through [`Fleet::observe`] and turns the resulting
//! [`FleetEvent`]s into effects (initialising a vehicle, recentering the map).

use std::{
    collections::{BTreeMap, btree_map::Entry},
    time::Instant,
};

use mavio::DefaultDialect;
use slippery::Mercator;

use crate::{domain::vehicle::Vehicle, foundation::MavlinkId};

/// Something the fleet observed that the application may need to react to.
#[derive(Debug, Clone)]
pub enum FleetEvent {
    /// A vehicle was seen for the first time.
    Registered(MavlinkId),
    /// The primary vehicle's latest position, after this frame was applied.
    /// Whether to recenter is the application's policy, not the fleet's.
    PrimaryMoved(Mercator),
}

#[derive(Default)]
pub struct Fleet {
    vehicles: BTreeMap<MavlinkId, Vehicle>,
    primary: Option<MavlinkId>,
}

impl Fleet {
    pub fn vehicles(&self) -> &BTreeMap<MavlinkId, Vehicle> {
        &self.vehicles
    }

    pub fn vehicles_mut(&mut self) -> &mut BTreeMap<MavlinkId, Vehicle> {
        &mut self.vehicles
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

        if let Some(message) = message
            && let Some(vehicle) = self.vehicles.get_mut(&source)
        {
            vehicle.handle_message(received_at, message);
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
}
