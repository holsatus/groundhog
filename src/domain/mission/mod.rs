//! MAVLink mission model.
//!
//! This module holds the typed command/item representation and its lossless
//! conversion to and from the wire, plus the editable [`Mission`] list and the
//! execution progress reported by the vehicle. Transport and protocol handling
//! live elsewhere.

use mavio::default_dialect::{enums::MissionState, messages};

pub mod command;

pub use command::*;

/// Scale between WGS-84 degrees and the integer coordinates of `MISSION_ITEM_INT`.
pub(crate) const LATLON_SCALE: f64 = 1e-7;

/// A vehicle's mission: the working plan the user edits, plus execution state.
#[derive(Debug, Clone, Default)]
pub struct Mission {
    items: Vec<MissionItem>,
    pub progress: MissionProgress,
    /// Opaque id of the plan last synced with the vehicle (upload/download).
    pub vehicle_id: Option<u32>,
    /// Whether the working plan differs from the last synced plan.
    pub dirty: bool,
}

/// Execution progress as reported by the vehicle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MissionProgress {
    pub current_seq: Option<u16>,
    pub total: Option<u16>,
    pub state: MissionState,
}

impl Mission {
    pub fn items(&self) -> &[MissionItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[allow(dead_code)] // Part of the mission list API; exercised by tests.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&MissionItem> {
        self.items.get(index)
    }

    pub fn push(&mut self, item: MissionItem) {
        self.items.push(item);
        self.dirty = true;
    }

    #[allow(dead_code)] // Part of the mission list API; exercised by tests.
    pub fn insert(&mut self, index: usize, item: MissionItem) {
        self.items.insert(index, item);
        self.dirty = true;
    }

    pub fn remove(&mut self, index: usize) -> Option<MissionItem> {
        if index >= self.items.len() {
            return None;
        }

        self.dirty = true;
        Some(self.items.remove(index))
    }

    /// Swap the item at `index` with its predecessor. `false` at the boundary.
    pub fn move_up(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.items.len() {
            return false;
        }

        self.items.swap(index, index - 1);
        self.dirty = true;
        true
    }

    /// Swap the item at `index` with its successor. `false` at the boundary.
    pub fn move_down(&mut self, index: usize) -> bool {
        if index + 1 >= self.items.len() {
            return false;
        }

        self.items.swap(index, index + 1);
        self.dirty = true;
        true
    }

    /// Apply an edit to the item at `index`. `false` if the index is invalid.
    pub fn update(&mut self, index: usize, f: impl FnOnce(&mut MissionItem)) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };

        f(item);
        self.dirty = true;
        true
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.dirty = true;
    }

    /// Replace the contents with a freshly downloaded plan. Deliberately leaves
    /// `dirty` and `vehicle_id` alone; call [`Mission::mark_synced`] afterwards.
    pub fn set_items(&mut self, items: Vec<MissionItem>) {
        self.items = items;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Record the plan as matching the vehicle, tagged with `vehicle_id`.
    pub fn mark_synced(&mut self, vehicle_id: Option<u32>) {
        self.vehicle_id = vehicle_id;
        self.dirty = false;
    }

    pub fn observe_current(&mut self, msg: &messages::MissionCurrent) {
        self.progress.current_seq = Some(msg.seq);
        self.progress.total = Some(msg.total);
        self.progress.state = msg.mission_state;
    }

    pub fn observe_reached(&mut self, seq: u16) {
        self.progress.current_seq = Some(seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::enums::MavFrame;

    fn waypoint(lat: f64) -> MissionItem {
        MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command: MissionCommand::Waypoint {
                pos: Position {
                    lat,
                    lon: 8.5,
                    alt_m: 50.0,
                },
                hold_s: 0.0,
                accept_radius_m: 0.0,
                pass_radius_m: 0.0,
                yaw_deg: None,
            },
        }
    }

    #[test]
    fn mutations_update_order_and_dirty() {
        let a = waypoint(1.0);
        let b = waypoint(2.0);
        let c = waypoint(3.0);

        let mut mission = Mission::default();
        assert!(mission.is_empty());
        assert!(!mission.dirty);

        mission.push(a.clone());
        mission.push(b.clone());
        assert_eq!(mission.len(), 2);
        assert_eq!(mission.get(0), Some(&a));
        assert!(mission.dirty);

        mission.insert(1, c.clone());
        assert_eq!(
            mission.items(),
            [a.clone(), c.clone(), b.clone()].as_slice()
        );

        let removed = mission.remove(0);
        assert_eq!(removed, Some(a));
        assert_eq!(mission.items(), [c.clone(), b.clone()].as_slice());
        assert_eq!(mission.remove(9), None);

        assert!(mission.update(0, |item| item.autocontinue = false));
        assert!(!mission.get(0).unwrap().autocontinue);
        assert!(!mission.update(9, |item| item.autocontinue = false));
        assert!(mission.dirty);

        mission.clear();
        assert!(mission.is_empty());
        assert!(mission.dirty);
    }

    #[test]
    fn move_up_and_down_reorder_and_guard_boundaries() {
        let mut mission = Mission::default();
        mission.push(waypoint(1.0));
        mission.push(waypoint(2.0));
        mission.push(waypoint(3.0));
        mission.mark_synced(None);
        assert!(!mission.dirty);

        assert!(mission.move_up(2));
        assert!(mission.dirty);
        assert_eq!(
            mission.items(),
            [waypoint(1.0), waypoint(3.0), waypoint(2.0)].as_slice()
        );

        assert!(mission.move_down(0));
        assert_eq!(
            mission.items(),
            [waypoint(3.0), waypoint(1.0), waypoint(2.0)].as_slice()
        );

        assert!(!mission.move_up(0));
        assert!(!mission.move_down(2));
        assert!(!mission.move_up(9));
        assert!(!mission.move_down(9));

        let mut single = Mission::default();
        single.push(waypoint(1.0));
        single.mark_synced(None);
        assert!(!single.move_up(0));
        assert!(!single.move_down(0));
        assert!(!single.dirty, "a rejected move must not dirty the plan");
    }

    #[test]
    fn mark_synced_sets_vehicle_id_and_clears_dirty() {
        let mut mission = Mission::default();
        mission.push(waypoint(1.0));

        mission.mark_synced(Some(42));
        assert_eq!(mission.vehicle_id, Some(42));
        assert!(!mission.dirty);

        mission.mark_dirty();
        assert!(mission.dirty);

        mission.mark_synced(None);
        assert_eq!(mission.vehicle_id, None);
        assert!(!mission.dirty);
    }

    #[test]
    fn set_items_preserves_sync_state() {
        let mut mission = Mission::default();
        mission.push(waypoint(1.0));
        mission.mark_synced(Some(7));

        mission.set_items(vec![waypoint(4.0), waypoint(5.0)]);

        assert_eq!(mission.len(), 2);
        assert_eq!(mission.vehicle_id, Some(7));
        assert!(!mission.dirty);
    }

    #[test]
    fn observe_current_and_reached_update_progress() {
        let mut mission = Mission::default();

        mission.observe_current(&messages::MissionCurrent {
            seq: 3,
            total: 10,
            mission_state: MissionState::Active,
            ..Default::default()
        });

        assert_eq!(mission.progress.current_seq, Some(3));
        assert_eq!(mission.progress.total, Some(10));
        assert_eq!(mission.progress.state, MissionState::Active);

        mission.observe_reached(5);
        assert_eq!(mission.progress.current_seq, Some(5));
        assert_eq!(mission.progress.total, Some(10));
        assert_eq!(mission.progress.state, MissionState::Active);

        mission.observe_current(&messages::MissionCurrent {
            seq: 0,
            total: 0,
            mission_state: MissionState::Unknown,
            ..Default::default()
        });
        assert_eq!(
            mission.progress,
            MissionProgress {
                current_seq: Some(0),
                total: Some(0),
                state: MissionState::Unknown,
            }
        );
    }
}
