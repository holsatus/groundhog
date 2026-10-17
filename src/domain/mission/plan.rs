//! An editable flight plan with stable item identities.
//!
//! Every item gets an [`ItemId`] when it enters the plan and keeps it through
//! reordering and edits. Asynchronous inputs (a map drag in progress, an editor
//! field) address items by id, so a plan replaced or reordered underneath them
//! can never redirect an edit to a different item: a stale id simply no longer
//! resolves.
//!
//! Jump targets are plan indices. The plan keeps them pointing at the same
//! item when items are inserted, removed or moved.

use super::item::{MissionItem, Position};

/// A plan-unique, stable item identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(u32);

impl ItemId {
    /// The id as a number, for embedding in other identifiers (map handles).
    pub fn raw(self) -> u32 {
        self.0
    }

    /// The id a [`raw`](ItemId::raw) value came from. An id that does not
    /// belong to a plan simply resolves to nothing.
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

/// One item of a plan, with its identity.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanItem {
    id: ItemId,
    pub item: MissionItem,
}

impl PlanItem {
    pub fn id(&self) -> ItemId {
        self.id
    }
}

/// An ordered, editable list of mission items.
#[derive(Debug, Clone, Default)]
pub struct MissionPlan {
    items: Vec<PlanItem>,
    next_id: u32,
    revision: u64,
}

impl MissionPlan {
    /// A plan holding `items`, each with a fresh id.
    #[cfg(test)]
    pub fn from_items(items: impl IntoIterator<Item = MissionItem>) -> Self {
        let mut plan = MissionPlan::default();
        plan.set_items(items);
        plan
    }

    pub fn items(&self) -> &[PlanItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Bumped on every change, for caches keyed on plan content.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn index_of(&self, id: ItemId) -> Option<usize> {
        self.items.iter().position(|item| item.id == id)
    }

    pub fn get(&self, id: ItemId) -> Option<&MissionItem> {
        self.items
            .iter()
            .find(|item| item.id == id)
            .map(|item| &item.item)
    }

    /// The items without their ids, as they would be uploaded.
    pub fn mission_items(&self) -> Vec<MissionItem> {
        self.items.iter().map(|item| item.item.clone()).collect()
    }

    /// Whether the plan holds exactly `items`, in order.
    pub fn matches(&self, items: &[MissionItem]) -> bool {
        self.items.len() == items.len()
            && self
                .items
                .iter()
                .zip(items)
                .all(|(ours, theirs)| ours.item == *theirs)
    }

    fn fresh_id(&mut self) -> ItemId {
        let id = ItemId(self.next_id);
        self.next_id += 1;
        id
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Append an item, returning its id.
    pub fn push(&mut self, item: MissionItem) -> ItemId {
        let index = self.items.len();
        self.insert(index, item)
    }

    /// Insert an item before `index` (clamped to the end), returning its id.
    pub fn insert(&mut self, index: usize, item: MissionItem) -> ItemId {
        let index = index.min(self.items.len());
        let id = self.fresh_id();

        // Targets at or after the insertion point move down by one.
        self.remap_targets(|target| {
            if target >= index {
                Some(target + 1)
            } else {
                Some(target)
            }
        });
        self.items.insert(index, PlanItem { id, item });
        self.touch();
        id
    }

    /// Remove an item. Jump targets pointing at it now point at its successor.
    pub fn remove(&mut self, id: ItemId) -> Option<MissionItem> {
        let index = self.index_of(id)?;
        let removed = self.items.remove(index);

        self.remap_targets(|target| {
            if target > index {
                Some(target - 1)
            } else {
                Some(target)
            }
        });
        self.touch();
        Some(removed.item)
    }

    /// Swap an item with its predecessor. `false` at the boundary.
    pub fn move_up(&mut self, id: ItemId) -> bool {
        match self.index_of(id) {
            Some(index) if index > 0 => {
                self.swap(index - 1, index);
                true
            }
            _ => false,
        }
    }

    /// Swap an item with its successor. `false` at the boundary.
    pub fn move_down(&mut self, id: ItemId) -> bool {
        match self.index_of(id) {
            Some(index) if index + 1 < self.items.len() => {
                self.swap(index, index + 1);
                true
            }
            _ => false,
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.items.swap(a, b);
        self.remap_targets(|target| {
            Some(if target == a {
                b
            } else if target == b {
                a
            } else {
                target
            })
        });
        self.touch();
    }

    /// Apply an edit to an item. Returns whether the item exists and changed.
    pub fn update(&mut self, id: ItemId, edit: impl FnOnce(&mut MissionItem)) -> bool {
        let Some(slot) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };

        let before = slot.item.clone();
        edit(&mut slot.item);
        let changed = slot.item != before;

        if changed {
            self.touch();
        }
        changed
    }

    /// Replace the whole plan; every item gets a fresh id.
    pub fn set_items(&mut self, items: impl IntoIterator<Item = MissionItem>) {
        let items: Vec<_> = items.into_iter().collect();
        self.items = items
            .into_iter()
            .map(|item| PlanItem {
                id: self.fresh_id(),
                item,
            })
            .collect();
        self.touch();
    }

    pub fn clear(&mut self) {
        self.set_items([]);
    }

    /// Each positioned item with its id and index, in plan order.
    pub fn route(&self) -> impl Iterator<Item = (usize, ItemId, Position)> + '_ {
        self.items.iter().enumerate().filter_map(|(index, item)| {
            item.item
                .position()
                .map(|position| (index, item.id, position))
        })
    }

    /// The altitude a new waypoint should get: that of the last positioned
    /// item, else `fallback`. Never the vehicle's current altitude, which is
    /// near zero on the ground.
    pub fn next_altitude(&self, fallback: f32) -> f32 {
        self.items
            .iter()
            .rev()
            .find_map(|item| item.item.position())
            .map(|position| position.alt_m)
            .unwrap_or(fallback)
    }

    /// Rewrite every jump target through `map`, leaving targets outside the
    /// plan untouched.
    fn remap_targets(&mut self, map: impl Fn(usize) -> Option<usize>) {
        for slot in &mut self.items {
            let slots: Vec<usize> = slot.item.item_index_slots().collect();
            for param in slots {
                let target = slot.item.params[param];
                if target < 0.0 || !target.is_finite() {
                    continue;
                }
                if let Some(mapped) = map(target as usize) {
                    slot.item.params[param] = mapped as f32;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::enums::MavCmd;

    fn waypoint(lat: f64) -> MissionItem {
        MissionItem::waypoint(Position {
            lat,
            lon: 8.5,
            alt_m: 50.0,
        })
    }

    fn jump_to(index: usize) -> MissionItem {
        let mut jump = MissionItem::new(MavCmd::DoJump, None);
        jump.params[0] = index as f32;
        jump
    }

    #[test]
    fn ids_are_stable_across_reordering() {
        let mut plan = MissionPlan::default();
        let a = plan.push(waypoint(1.0));
        let b = plan.push(waypoint(2.0));

        assert!(plan.move_down(a));
        assert_eq!(plan.index_of(a), Some(1));
        assert_eq!(plan.index_of(b), Some(0));
        assert!(!plan.move_down(a), "already last");
    }

    #[test]
    fn a_replaced_plan_invalidates_old_ids() {
        let mut plan = MissionPlan::default();
        let old = plan.push(waypoint(1.0));
        plan.set_items([waypoint(5.0)]);

        assert!(plan.get(old).is_none());
        assert!(!plan.update(old, |item| item.z = 99.0));
    }

    #[test]
    fn jump_targets_follow_their_item() {
        let mut plan = MissionPlan::default();
        let first = plan.push(waypoint(1.0));
        let second = plan.push(waypoint(2.0));
        let jump = plan.push(jump_to(1)); // -> second

        plan.insert(0, waypoint(0.0));
        assert_eq!(
            plan.get(jump).unwrap().params[0],
            2.0,
            "shifted by the insert"
        );

        plan.remove(first);
        assert_eq!(plan.get(jump).unwrap().params[0], 1.0, "shifted back");

        assert!(plan.move_up(second));
        assert_eq!(plan.get(jump).unwrap().params[0], 0.0, "followed the move");
        assert_eq!(plan.index_of(second), Some(0));
    }

    #[test]
    fn updates_report_real_changes_only() {
        let mut plan = MissionPlan::default();
        let id = plan.push(waypoint(1.0));
        let revision = plan.revision();

        assert!(!plan.update(id, |_| {}));
        assert_eq!(plan.revision(), revision);

        assert!(plan.update(id, |item| item.z = 70.0));
        assert_ne!(plan.revision(), revision);
    }

    #[test]
    fn new_waypoints_take_the_last_planned_altitude() {
        let mut plan = MissionPlan::default();
        assert_eq!(plan.next_altitude(50.0), 50.0);

        plan.push(MissionItem::waypoint(Position {
            lat: 1.0,
            lon: 1.0,
            alt_m: 120.0,
        }));
        plan.push(MissionItem::new(MavCmd::ConditionDelay, None));
        assert_eq!(plan.next_altitude(50.0), 120.0);
    }

    #[test]
    fn matching_compares_wire_content() {
        let plan = MissionPlan::from_items([waypoint(1.0), waypoint(2.0)]);
        assert!(plan.matches(&[waypoint(1.0), waypoint(2.0)]));
        assert!(!plan.matches(&[waypoint(1.0)]));
        assert!(!plan.matches(&[waypoint(1.0), waypoint(3.0)]));
    }
}
