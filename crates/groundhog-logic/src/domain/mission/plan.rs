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

use mavio::default_dialect::enums::MavCmd;

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
    /// The home a file planned, when the plan is not tied to a vehicle's own.
    home: Option<Position>,
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

    /// The home the plan was loaded with, if any.
    pub fn home(&self) -> Option<Position> {
        self.home
    }

    /// Set the planned home. `None` forgets it, so saving writes no real home.
    pub fn set_home(&mut self, home: Option<Position>) {
        if self.home != home {
            self.home = home;
            self.touch();
        }
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

        // A target at the removed item follows the item that takes its place;
        // removing the last item sends it to the new last one, never past the
        // plan.
        let last = self.items.len();
        self.remap_targets(|target| {
            if target > index {
                Some(target - 1)
            } else if target == index {
                Some(index.min(last.saturating_sub(1)))
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
        self.set_home(None);
    }

    /// Each positioned item with its id and index, in plan order.
    pub fn route(&self) -> impl Iterator<Item = (usize, ItemId, Position)> + '_ {
        self.items.iter().enumerate().filter_map(|(index, item)| {
            item.item
                .position()
                .map(|position| (index, item.id, position))
        })
    }

    /// The altitude a new waypoint should get: the last flight altitude in
    /// the plan ([`MissionItem::flight_altitude`]), else `fallback`. Never the
    /// vehicle's current altitude, which is near zero on the ground, nor a
    /// landing's or a region of interest's.
    pub fn next_altitude(&self, fallback: f32) -> f32 {
        self.altitude_before(self.items.len(), fallback)
    }

    /// The last flight altitude among the items before `index`, else
    /// `fallback`.
    fn altitude_before(&self, index: usize, fallback: f32) -> f32 {
        self.items[..index]
            .iter()
            .rev()
            .find_map(|item| item.item.flight_altitude())
            .unwrap_or(fallback)
    }

    /// Change the command of `id`, keeping what carries over
    /// ([`MissionItem::with_command`]). An item that has no location, turned
    /// into a command that needs one, takes the nearest location in the plan
    /// (before it, else after it) at the flight altitude that applies there,
    /// else `default_altitude_m`. Returns `false`, changing nothing, when the
    /// item does not exist or the plan has no location to give.
    pub fn change_command(&mut self, id: ItemId, command: MavCmd, default_altitude_m: f32) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let item = &self.items[index].item;

        let alt_m = item
            .flight_altitude()
            .unwrap_or_else(|| self.altitude_before(index, default_altitude_m));
        let (before, after) = (&self.items[..index], &self.items[index + 1..]);
        let fallback = before
            .iter()
            .rev()
            .chain(after)
            .find_map(|slot| slot.item.position())
            .map(|position| Position { alt_m, ..position });

        let Some(converted) = item.with_command(command, fallback) else {
            return false;
        };
        self.update(id, |item| *item = converted);
        true
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

    fn waypoint(lat: f64) -> MissionItem {
        MissionItem::waypoint(Position {
            lat,
            lon: 8.5,
            alt_m: 50.0,
        })
    }

    #[test]
    fn landings_and_regions_of_interest_lend_no_altitude() {
        let mut plan = MissionPlan::default();
        plan.push(waypoint(1.0));
        plan.push(MissionItem::new(
            MavCmd::NavLand,
            Some(Position {
                lat: 1.0,
                lon: 8.5,
                alt_m: 0.0,
            }),
        ));
        assert_eq!(plan.next_altitude(30.0), 50.0);

        let mut roi_only = MissionPlan::default();
        roi_only.push(MissionItem::new(
            MavCmd::DoSetRoiLocation,
            Some(Position {
                lat: 1.0,
                lon: 8.5,
                alt_m: 0.0,
            }),
        ));
        assert_eq!(roi_only.next_altitude(30.0), 30.0);
    }

    #[test]
    fn a_command_change_takes_its_location_from_the_plan() {
        let mut plan = MissionPlan::default();
        plan.push(waypoint(10.0));
        let delay = plan.push(MissionItem::new(MavCmd::NavDelay, None));

        assert!(plan.change_command(delay, MavCmd::NavWaypoint, 30.0));
        let position = plan.get(delay).unwrap().position().unwrap();
        assert_eq!(
            (position.lat, position.lon, position.alt_m),
            (10.0, 8.5, 50.0)
        );
    }

    #[test]
    fn a_command_change_without_a_location_to_give_is_refused() {
        let mut plan = MissionPlan::default();
        let delay = plan.push(MissionItem::new(MavCmd::NavDelay, None));
        let before = plan.get(delay).cloned();

        assert!(!plan.change_command(delay, MavCmd::NavWaypoint, 30.0));
        assert_eq!(plan.get(delay).cloned(), before);

        // A takeoff needs no location: "from here".
        assert!(plan.change_command(delay, MavCmd::NavTakeoff, 30.0));
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

    /// A removed jump target follows its successor, and never points past
    /// the plan.
    #[test]
    fn removing_a_jump_target_keeps_it_in_the_plan() {
        let mut plan = MissionPlan::default();
        plan.push(waypoint(1.0));
        let second = plan.push(waypoint(2.0));
        let third = plan.push(waypoint(3.0));
        let jump = plan.push(jump_to(1)); // -> second

        plan.remove(second);
        let target = plan.get(jump).unwrap().params[0];
        assert_eq!(target, 1.0, "the item that took its place");
        assert_eq!(plan.index_of(third), Some(1));

        // Removing the last item leaves the target inside the plan.
        plan.remove(third);
        let target = plan.get(jump).unwrap().params[0];
        assert!(
            target < plan.len() as f32,
            "a target never points past the plan"
        );
    }

    #[test]
    fn a_planned_home_is_kept_until_the_plan_is_cleared() {
        let mut plan = MissionPlan::default();
        let home = Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 400.0,
        };

        assert_eq!(plan.home(), None);
        plan.set_home(Some(home));
        assert_eq!(plan.home(), Some(home));

        plan.push(waypoint(1.0));
        assert_eq!(plan.home(), Some(home), "editing keeps the planned home");

        plan.clear();
        assert_eq!(plan.home(), None);
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
