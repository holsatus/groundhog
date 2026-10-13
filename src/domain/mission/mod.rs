//! MAVLink mission model.
//!
//! This module holds the typed command/item representation and its lossless
//! conversion to and from the wire, plus the editable [`MissionPlan`] and the
//! [`MissionSync`] metadata that tracks whether that plan matches the vehicle.
//! Execution progress reported by the vehicle lives with the vehicle, in
//! `domain::vehicle::mission`. Transport and protocol handling live elsewhere.

pub mod command;

pub use command::*;

/// Scale between WGS-84 degrees and the integer coordinates of `MISSION_ITEM_INT`.
///
/// Defined in [`crate::domain::geo`] so the mission and vehicle layers share one
/// source; re-exported here for the existing `domain::mission::LATLON_SCALE`
/// path.
pub(crate) use crate::domain::geo::LATLON_SCALE;

/// The editable flight plan: an ordered list of mission items.
///
/// This owns only the plan itself. Whether the plan matches the vehicle is
/// tracked separately in [`MissionSync`], and execution progress lives with the
/// vehicle, so a plan edit and a telemetry update never share a struct.
#[derive(Debug, Clone, Default)]
pub struct MissionPlan {
    items: Vec<MissionItem>,
}

impl MissionPlan {
    pub fn items(&self) -> &[MissionItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[allow(dead_code)] // Part of the plan API; exercised by tests.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&MissionItem> {
        self.items.get(index)
    }

    pub fn push(&mut self, item: MissionItem) {
        self.items.push(item);
    }

    pub fn remove(&mut self, index: usize) -> Option<MissionItem> {
        if index >= self.items.len() {
            return None;
        }

        Some(self.items.remove(index))
    }

    /// Swap the item at `index` with its predecessor. `false` at the boundary.
    pub fn move_up(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.items.len() {
            return false;
        }

        self.items.swap(index, index - 1);
        true
    }

    /// Swap the item at `index` with its successor. `false` at the boundary.
    pub fn move_down(&mut self, index: usize) -> bool {
        if index + 1 >= self.items.len() {
            return false;
        }

        self.items.swap(index, index + 1);
        true
    }

    /// Apply an edit to the item at `index`. `false` if the index is invalid.
    pub fn update(&mut self, index: usize, f: impl FnOnce(&mut MissionItem)) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };

        f(item);
        true
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Replace the contents with a freshly downloaded plan.
    pub fn set_items(&mut self, items: Vec<MissionItem>) {
        self.items = items;
    }

    /// Project the plan's route: each positioned item paired with its list
    /// index. A command is positioned iff [`MissionCommand::position`] is
    /// `Some`, so non-positional items (delays, jumps, RTL, ...) are skipped
    /// while retaining the true index of the ones that remain.
    pub fn route_points(&self) -> impl Iterator<Item = (usize, Position)> + '_ {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| item.command.position().map(|pos| (index, pos)))
    }
}

/// Whether the working plan matches the last plan synced with the vehicle.
#[derive(Debug, Clone, Default)]
pub struct MissionSync {
    /// Opaque id of the plan last synced with the vehicle (upload/download).
    pub vehicle_id: Option<u32>,
    /// Whether the working plan differs from the last synced plan.
    pub dirty: bool,
}

impl MissionSync {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Record the plan as matching the vehicle, tagged with `vehicle_id`.
    pub fn mark_synced(&mut self, vehicle_id: Option<u32>) {
        self.vehicle_id = vehicle_id;
        self.dirty = false;
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
    fn mutations_update_order() {
        let a = waypoint(1.0);
        let b = waypoint(2.0);

        let mut plan = MissionPlan::default();
        assert!(plan.is_empty());

        plan.push(a.clone());
        plan.push(b.clone());
        assert_eq!(plan.len(), 2);
        assert_eq!(plan.get(0), Some(&a));

        let removed = plan.remove(0);
        assert_eq!(removed, Some(a));
        assert_eq!(plan.items(), [b.clone()].as_slice());
        assert_eq!(plan.remove(9), None);

        assert!(plan.update(0, |item| item.autocontinue = false));
        assert!(!plan.get(0).unwrap().autocontinue);
        assert!(!plan.update(9, |item| item.autocontinue = false));

        plan.clear();
        assert!(plan.is_empty());
    }

    #[test]
    fn move_up_and_down_reorder_and_guard_boundaries() {
        let mut plan = MissionPlan::default();
        plan.push(waypoint(1.0));
        plan.push(waypoint(2.0));
        plan.push(waypoint(3.0));

        assert!(plan.move_up(2));
        assert_eq!(
            plan.items(),
            [waypoint(1.0), waypoint(3.0), waypoint(2.0)].as_slice()
        );

        assert!(plan.move_down(0));
        assert_eq!(
            plan.items(),
            [waypoint(3.0), waypoint(1.0), waypoint(2.0)].as_slice()
        );

        assert!(!plan.move_up(0));
        assert!(!plan.move_down(2));
        assert!(!plan.move_up(9));
        assert!(!plan.move_down(9));

        let mut single = MissionPlan::default();
        single.push(waypoint(1.0));
        assert!(!single.move_up(0));
        assert!(!single.move_down(0));
    }

    #[test]
    fn mark_synced_sets_vehicle_id_and_clears_dirty() {
        let mut sync = MissionSync::default();
        assert!(!sync.dirty);

        sync.mark_dirty();
        assert!(sync.dirty);

        sync.mark_synced(Some(42));
        assert_eq!(sync.vehicle_id, Some(42));
        assert!(!sync.dirty);

        sync.mark_dirty();
        assert!(sync.dirty);

        sync.mark_synced(None);
        assert_eq!(sync.vehicle_id, None);
        assert!(!sync.dirty);
    }

    #[test]
    fn set_items_replaces_the_plan() {
        let mut plan = MissionPlan::default();
        plan.push(waypoint(1.0));

        plan.set_items(vec![waypoint(4.0), waypoint(5.0)]);

        assert_eq!(plan.len(), 2);
        assert_eq!(plan.items(), [waypoint(4.0), waypoint(5.0)].as_slice());
    }

    #[test]
    fn route_points_yields_positioned_items_with_their_index() {
        let mut plan = MissionPlan::default();
        plan.push(waypoint(1.0));
        plan.push(MissionItem {
            frame: MavFrame::Mission,
            autocontinue: true,
            command: MissionCommand::ReturnToLaunch,
        });
        plan.push(waypoint(3.0));

        let points: Vec<(usize, Position)> = plan.route_points().collect();

        assert_eq!(
            points,
            [
                (
                    0,
                    Position {
                        lat: 1.0,
                        lon: 8.5,
                        alt_m: 50.0,
                    },
                ),
                (
                    2,
                    Position {
                        lat: 3.0,
                        lon: 8.5,
                        alt_m: 50.0,
                    },
                ),
            ]
            .as_slice()
        );
    }
}
