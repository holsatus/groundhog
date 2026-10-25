//! The operator's draft of a vehicle's mission, kept against what the vehicle
//! holds.
//!
//! A [`MissionDraft`] is operator state: it changes only when the operator
//! edits it, when a transfer the operator started finishes, or when the
//! vehicle's onboard mission changes while the draft has no edits of its own
//! (a download elsewhere, or another GCS uploading). A modified draft is never
//! overwritten behind the operator's back.
//!
//! Whether a draft matches the vehicle is derived, never tracked: the plan is
//! compared with the onboard mission in the snapshot ([`MissionDraft::status`]).
//! What follows from that for the operator's actions is decided here too
//! ([`MissionDraft::check`]): which ask first, and which are refused.
//! What a frontend shows *around* a draft (the selected item, half-typed field
//! text, a placing mode) is its own.

use std::sync::Arc;

use crate::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        geo::GeoPosition,
        mission::{ItemId, MissionItem, MissionPlan, OnboardMission, Position},
    },
};

/// How a draft relates to its vehicle's onboard mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    /// The offline plan: there is no vehicle to compare with.
    Offline,
    /// The vehicle's mission is not known (never transferred).
    Unknown,
    Synced,
    Modified,
    /// The vehicle reports holding a mission other than the one last
    /// transferred: someone else changed it.
    VehicleChanged,
}

/// What the operator asks of a vehicle's mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionAction {
    /// Replace the plan with the vehicle's mission.
    Download,
    /// Replace the vehicle's mission with the plan.
    Upload,
    /// Delete the vehicle's mission.
    Clear,
    /// Fly the vehicle's mission from its first item.
    Start,
    /// Hold the mission. A safety stop: it never asks.
    Pause,
    /// Continue the paused mission.
    Resume,
    /// Continue the mission at this plan item.
    Jump(ItemId),
}

/// Why an action needs the operator's word first. A frontend words each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caution {
    /// The plan has edits the action replaces.
    DiscardsEdits,
    /// The vehicle is armed, or its armed state is unknown: a change to its
    /// mission may take effect in flight.
    Armed,
    /// The vehicle's mission is deleted: by clearing it, or by uploading an
    /// empty plan.
    DeletesMission,
    /// The vehicle's mission was changed elsewhere since the last transfer,
    /// and the action replaces that change.
    ReplacesChangeElsewhere,
    /// The vehicle flies its stored mission, which is not the plan shown.
    FliesAnotherMission,
    /// The vehicle flies its stored mission, which was never transferred
    /// here.
    FliesUnseenMission,
}

/// Why an action cannot run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The offline plan has no vehicle to act on.
    NoVehicle,
    /// The plan is not what the vehicle holds, so its item numbers mean
    /// something else on the vehicle.
    NotSynced,
    /// The item is not in the plan.
    NoSuchItem,
}

/// Whether an action may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Go,
    /// Ask the operator first. Some actions always ask, with no caution.
    Confirm(Vec<Caution>),
    Refuse(Refusal),
}

/// A plan for one vehicle, or the offline plan, and what it was last synced
/// with.
#[derive(Debug, Clone)]
pub struct MissionDraft {
    vehicle: Option<SystemId>,
    plan: MissionPlan,
    /// The items the draft was last synced with.
    base: Option<Vec<MissionItem>>,
    /// The onboard mission last seen in a snapshot, to notice new ones.
    seen: Option<Arc<OnboardMission>>,
}

impl MissionDraft {
    /// An empty draft for `vehicle`, or the offline plan (`None`).
    pub fn new(vehicle: Option<SystemId>) -> Self {
        Self {
            vehicle,
            plan: MissionPlan::default(),
            base: None,
            seen: None,
        }
    }

    /// The vehicle the draft is for; `None` for the offline plan.
    pub fn vehicle(&self) -> Option<SystemId> {
        self.vehicle
    }

    pub fn plan(&self) -> &MissionPlan {
        &self.plan
    }

    /// The plan, to edit. Whether the draft is modified is derived from it, so
    /// any edit counts.
    pub fn plan_mut(&mut self) -> &mut MissionPlan {
        &mut self.plan
    }

    /// Whether the operator changed the plan since it was last synced (for a
    /// draft never synced: whether it holds anything).
    pub fn is_modified(&self) -> bool {
        match &self.base {
            Some(base) => !self.plan.matches(base),
            None => !self.plan.is_empty(),
        }
    }

    /// How the plan relates to the mission the vehicle holds.
    pub fn status(&self, fleet: &FleetSnapshot) -> SyncStatus {
        let Some(system) = self.vehicle else {
            return SyncStatus::Offline;
        };
        let Some(vehicle) = fleet.vehicle(system) else {
            return SyncStatus::Unknown;
        };
        let Some(onboard) = &vehicle.mission.onboard else {
            return SyncStatus::Unknown;
        };

        if vehicle.mission.onboard_is_stale() {
            SyncStatus::VehicleChanged
        } else if self.plan.matches(&onboard.items) {
            SyncStatus::Synced
        } else {
            SyncStatus::Modified
        }
    }

    /// Whether `action` may run on the draft's vehicle now, should ask first,
    /// or cannot run. One rule set, so no frontend decides one on its own:
    ///
    /// | Action | Asks | Cautions |
    /// |---|---|---|
    /// | Download | when cautioned | edits are discarded |
    /// | Upload | when cautioned | armed (or unknown); empty plan deletes; changed elsewhere |
    /// | Clear | always | deletes; armed (or unknown) |
    /// | Start, Resume | always | the stored mission is not the plan shown, or unseen |
    /// | Pause | never | none |
    /// | Jump | always | none; refused unless synced |
    ///
    /// The offline plan refuses every action.
    pub fn check(&self, action: MissionAction, fleet: &FleetSnapshot) -> Verdict {
        let Some(system) = self.vehicle else {
            return Verdict::Refuse(Refusal::NoVehicle);
        };
        let status = self.status(fleet);
        let armed = fleet
            .vehicle(system)
            .and_then(|vehicle| vehicle.armed())
            .unwrap_or(true);

        // What the vehicle flies on Start or Resume, when it is not the plan.
        let flies_stored = match status {
            SyncStatus::Modified | SyncStatus::VehicleChanged => Some(Caution::FliesAnotherMission),
            SyncStatus::Unknown => Some(Caution::FliesUnseenMission),
            SyncStatus::Synced | SyncStatus::Offline => None,
        };

        let mut cautions = Vec::new();
        let always_asks = match action {
            MissionAction::Download => {
                if self.is_modified() {
                    cautions.push(Caution::DiscardsEdits);
                }
                false
            }
            MissionAction::Upload => {
                if armed {
                    cautions.push(Caution::Armed);
                }
                if self.plan.is_empty() {
                    cautions.push(Caution::DeletesMission);
                }
                if status == SyncStatus::VehicleChanged {
                    cautions.push(Caution::ReplacesChangeElsewhere);
                }
                false
            }
            MissionAction::Clear => {
                cautions.push(Caution::DeletesMission);
                if armed {
                    cautions.push(Caution::Armed);
                }
                true
            }
            MissionAction::Start | MissionAction::Resume => {
                cautions.extend(flies_stored);
                true
            }
            MissionAction::Pause => false,
            MissionAction::Jump(item) => {
                if status != SyncStatus::Synced {
                    return Verdict::Refuse(Refusal::NotSynced);
                }
                if self.plan.index_of(item).is_none() {
                    return Verdict::Refuse(Refusal::NoSuchItem);
                }
                true
            }
        };

        if always_asks || !cautions.is_empty() {
            Verdict::Confirm(cautions)
        } else {
            Verdict::Go
        }
    }

    /// Fold a snapshot in: a draft without edits of its own follows a new
    /// onboard mission. Returns whether the plan's items were replaced, after
    /// which their old [`ItemId`]s resolve to nothing.
    pub fn follow(&mut self, fleet: &FleetSnapshot) -> bool {
        let Some(onboard) = self
            .vehicle
            .and_then(|system| fleet.vehicle(system))
            .and_then(|vehicle| vehicle.mission.onboard.as_ref())
        else {
            return false;
        };

        if self
            .seen
            .as_ref()
            .is_some_and(|seen| Arc::ptr_eq(seen, onboard))
        {
            return false;
        }

        self.seen = Some(onboard.clone());
        !self.is_modified() && self.sync_to(&onboard.items)
    }

    /// A download or upload finished: the vehicle holds `mission`, and the
    /// draft becomes it, edits included (a frontend asks before downloading
    /// over edits). Returns whether the plan's items were replaced; a plan
    /// that already matches keeps its item ids.
    pub fn transferred(&mut self, mission: &OnboardMission) -> bool {
        self.sync_to(&mission.items)
    }

    /// The vehicle's mission was cleared. The plan is kept, now as an edit
    /// the vehicle does not hold.
    pub fn cleared(&mut self) {
        self.base = Some(Vec::new());
    }

    /// Append a waypoint: at the last positioned item (to be dragged from
    /// there), else where the vehicle is or its home. Its altitude is the
    /// plan's next one ([`MissionPlan::next_altitude`]), never the vehicle's,
    /// which is zero on the ground. `None`, adding nothing, when none of
    /// these is known: the operator has to say where it goes
    /// ([`MissionDraft::place_waypoint`]).
    pub fn add_waypoint(
        &mut self,
        fleet: &FleetSnapshot,
        default_altitude_m: f32,
    ) -> Option<ItemId> {
        let at = self
            .plan
            .route()
            .last()
            .map(|(_, _, position)| GeoPosition::new(position.lat, position.lon))
            .or_else(|| {
                let vehicle = fleet.vehicle(self.vehicle?)?;
                vehicle
                    .position
                    .current
                    .as_ref()
                    .map(|position| position.pos)
                    .or_else(|| vehicle.home().map(|home| home.position))
            })?;
        Some(self.place_waypoint(at, default_altitude_m))
    }

    /// Append a waypoint at `at`, at the plan's next altitude.
    pub fn place_waypoint(&mut self, at: GeoPosition, default_altitude_m: f32) -> ItemId {
        let alt_m = self.plan.next_altitude(default_altitude_m);
        self.plan.push(MissionItem::waypoint(Position {
            lat: at.lat,
            lon: at.lon,
            alt_m,
        }))
    }

    /// Adopt `items` as both the plan and its base, keeping the plan's ids
    /// when it already holds them. Returns whether the plan was replaced.
    fn sync_to(&mut self, items: &[MissionItem]) -> bool {
        let replaced = !self.plan.matches(items);
        if replaced {
            self.plan.set_items(items.iter().cloned());
        }
        self.base = Some(items.to_vec());
        replaced
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use mavio::default_dialect::{
        enums::{MavAutopilot, MavModeFlag, MavType},
        messages,
    };

    use super::*;
    use crate::{
        MavlinkId,
        domain::{fleet::Fleet, mission::Field},
    };

    const SYSTEM: SystemId = SystemId(1);
    const ADDRESS: MavlinkId = MavlinkId {
        system: 1,
        component: 1,
    };

    /// A fleet with one ArduPilot vehicle.
    fn fleet() -> Fleet {
        let mut fleet = Fleet::default();
        let heartbeat = mavio::DefaultDialect::Heartbeat(messages::Heartbeat {
            type_: MavType::Quadrotor,
            autopilot: MavAutopilot::Ardupilotmega,
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
            ..Default::default()
        });
        fleet.observe(ADDRESS, Instant::now(), Some(&heartbeat));
        fleet
    }

    fn waypoint(lat: f64) -> MissionItem {
        MissionItem::waypoint(Position {
            lat,
            lon: 10.0,
            alt_m: 30.0,
        })
    }

    fn onboard(items: Vec<MissionItem>, opaque_id: u32) -> OnboardMission {
        OnboardMission {
            items,
            home: None,
            opaque_id,
        }
    }

    fn report(fleet: &mut Fleet, mission: OnboardMission) {
        assert!(fleet.set_onboard_mission(SYSTEM, mission));
    }

    #[test]
    fn an_unmodified_draft_follows_the_onboard_mission() {
        let mut fleet = fleet();
        let mut draft = MissionDraft::new(Some(SYSTEM));
        assert_eq!(draft.status(&fleet.snapshot()), SyncStatus::Unknown);

        report(&mut fleet, onboard(vec![waypoint(1.0)], 1));
        assert!(draft.follow(&fleet.snapshot()));
        assert_eq!(draft.plan().mission_items(), vec![waypoint(1.0)]);
        assert_eq!(draft.status(&fleet.snapshot()), SyncStatus::Synced);
        assert!(!draft.follow(&fleet.snapshot()), "the same mission again");

        // Another GCS uploads a different mission: still unmodified here, so
        // the draft follows.
        report(&mut fleet, onboard(vec![waypoint(2.0)], 2));
        assert!(draft.follow(&fleet.snapshot()));
        assert_eq!(draft.plan().mission_items(), vec![waypoint(2.0)]);
    }

    #[test]
    fn a_modified_draft_is_never_overwritten() {
        let mut fleet = fleet();
        let mut draft = MissionDraft::new(Some(SYSTEM));
        report(&mut fleet, onboard(vec![waypoint(1.0)], 1));
        draft.follow(&fleet.snapshot());

        draft.plan_mut().push(waypoint(5.0));
        assert!(draft.is_modified());
        assert_eq!(draft.status(&fleet.snapshot()), SyncStatus::Modified);

        report(&mut fleet, onboard(vec![waypoint(2.0)], 2));
        assert!(!draft.follow(&fleet.snapshot()));
        assert_eq!(
            draft.plan().mission_items(),
            vec![waypoint(1.0), waypoint(5.0)]
        );
    }

    #[test]
    fn transfers_sync_the_draft_and_keep_matching_ids() {
        let mut draft = MissionDraft::new(Some(SYSTEM));
        let first = draft.plan_mut().push(waypoint(1.0));
        assert!(draft.is_modified(), "an unsynced draft with items");

        // Uploading what the plan holds keeps its ids.
        assert!(!draft.transferred(&onboard(vec![waypoint(1.0)], 1)));
        assert!(!draft.is_modified());
        assert!(draft.plan().get(first).is_some());

        // Downloading something else replaces the plan, edits and all.
        draft.plan_mut().push(waypoint(3.0));
        assert!(draft.transferred(&onboard(vec![waypoint(2.0)], 2)));
        assert_eq!(draft.plan().mission_items(), vec![waypoint(2.0)]);
        assert!(
            draft.plan().get(first).is_none(),
            "old ids resolve to nothing"
        );
        assert!(!draft.is_modified());
    }

    #[test]
    fn clearing_the_vehicle_keeps_the_plan_as_an_edit() {
        let mut draft = MissionDraft::new(Some(SYSTEM));
        draft.transferred(&onboard(vec![waypoint(1.0)], 1));

        draft.cleared();
        assert!(draft.is_modified());
        assert_eq!(draft.plan().len(), 1);
    }

    #[test]
    fn a_vehicle_changed_elsewhere_is_reported() {
        let mut fleet = fleet();
        let mut draft = MissionDraft::new(Some(SYSTEM));
        report(&mut fleet, onboard(vec![waypoint(1.0)], 7));
        draft.follow(&fleet.snapshot());

        // The vehicle now reports holding another mission id.
        let current = mavio::DefaultDialect::MissionCurrent(messages::MissionCurrent {
            seq: 0,
            total: 2,
            mission_id: 8,
            ..Default::default()
        });
        fleet.observe(ADDRESS, Instant::now(), Some(&current));
        assert_eq!(draft.status(&fleet.snapshot()), SyncStatus::VehicleChanged);
    }

    /// A fleet whose vehicle is armed, or not.
    fn armed_fleet(armed: bool) -> Fleet {
        let mut fleet = Fleet::default();
        let mut base_mode = MavModeFlag::CUSTOM_MODE_ENABLED;
        if armed {
            base_mode |= MavModeFlag::SAFETY_ARMED;
        }
        let heartbeat = mavio::DefaultDialect::Heartbeat(messages::Heartbeat {
            type_: MavType::Quadrotor,
            autopilot: MavAutopilot::Ardupilotmega,
            base_mode,
            ..Default::default()
        });
        fleet.observe(ADDRESS, Instant::now(), Some(&heartbeat));
        fleet
    }

    /// A draft synced with a two-waypoint mission on a disarmed vehicle.
    fn synced() -> (Fleet, MissionDraft) {
        let mut fleet = armed_fleet(false);
        let mut draft = MissionDraft::new(Some(SYSTEM));
        report(&mut fleet, onboard(vec![waypoint(1.0), waypoint(2.0)], 1));
        draft.follow(&fleet.snapshot());
        (fleet, draft)
    }

    #[test]
    fn an_empty_upload_asks_because_it_deletes_the_mission() {
        let (fleet, mut draft) = synced();
        draft.plan_mut().clear();
        assert_eq!(
            draft.check(MissionAction::Upload, &fleet.snapshot()),
            Verdict::Confirm(vec![Caution::DeletesMission])
        );
    }

    #[test]
    fn an_upload_over_a_change_elsewhere_asks() {
        let (mut fleet, mut draft) = synced();
        draft.plan_mut().push(waypoint(3.0));
        let current = mavio::DefaultDialect::MissionCurrent(messages::MissionCurrent {
            total: 2,
            mission_id: 8,
            ..Default::default()
        });
        fleet.observe(ADDRESS, Instant::now(), Some(&current));

        assert_eq!(
            draft.check(MissionAction::Upload, &fleet.snapshot()),
            Verdict::Confirm(vec![Caution::ReplacesChangeElsewhere])
        );
    }

    #[test]
    fn an_upload_asks_while_the_vehicle_is_armed_or_may_be() {
        let mut fleet = armed_fleet(true);
        let mut draft = MissionDraft::new(Some(SYSTEM));
        draft.plan_mut().push(waypoint(1.0));
        assert!(matches!(
            draft.check(MissionAction::Upload, &fleet.snapshot()),
            Verdict::Confirm(cautions) if cautions.contains(&Caution::Armed)
        ));

        // A vehicle the fleet knows nothing of may be armed too.
        fleet = Fleet::default();
        assert!(matches!(
            draft.check(MissionAction::Upload, &fleet.snapshot()),
            Verdict::Confirm(cautions) if cautions.contains(&Caution::Armed)
        ));

        // Disarmed, with nothing else to say: no question.
        let (fleet, mut draft) = synced();
        draft.plan_mut().push(waypoint(3.0));
        assert_eq!(
            draft.check(MissionAction::Upload, &fleet.snapshot()),
            Verdict::Go
        );
    }

    #[test]
    fn start_says_when_the_vehicle_flies_another_mission() {
        let (fleet, mut draft) = synced();
        assert_eq!(
            draft.check(MissionAction::Start, &fleet.snapshot()),
            Verdict::Confirm(vec![])
        );

        draft.plan_mut().push(waypoint(3.0));
        for action in [MissionAction::Start, MissionAction::Resume] {
            assert_eq!(
                draft.check(action, &fleet.snapshot()),
                Verdict::Confirm(vec![Caution::FliesAnotherMission])
            );
        }

        let never_downloaded = MissionDraft::new(Some(SYSTEM));
        assert_eq!(
            never_downloaded.check(MissionAction::Start, &armed_fleet(false).snapshot()),
            Verdict::Confirm(vec![Caution::FliesUnseenMission])
        );
    }

    #[test]
    fn a_jump_needs_a_synced_plan() {
        let (fleet, mut draft) = synced();
        let second = draft.plan().items()[1].id();
        assert_eq!(
            draft.check(MissionAction::Jump(second), &fleet.snapshot()),
            Verdict::Confirm(vec![])
        );

        draft.plan_mut().push(waypoint(3.0));
        assert_eq!(
            draft.check(MissionAction::Jump(second), &fleet.snapshot()),
            Verdict::Refuse(Refusal::NotSynced)
        );
    }

    #[test]
    fn pause_never_asks_and_the_offline_plan_refuses() {
        let fleet = armed_fleet(true);
        let mut draft = MissionDraft::new(Some(SYSTEM));
        draft.plan_mut().push(waypoint(1.0));
        assert_eq!(
            draft.check(MissionAction::Pause, &fleet.snapshot()),
            Verdict::Go
        );
        assert_eq!(
            MissionDraft::new(None).check(MissionAction::Upload, &fleet.snapshot()),
            Verdict::Refuse(Refusal::NoVehicle)
        );
    }

    #[test]
    fn the_offline_plan_has_no_vehicle_to_follow() {
        let mut fleet = fleet();
        let mut draft = MissionDraft::new(None);
        report(&mut fleet, onboard(vec![waypoint(1.0)], 1));

        assert!(!draft.follow(&fleet.snapshot()));
        assert!(draft.plan().is_empty());
        assert_eq!(draft.status(&fleet.snapshot()), SyncStatus::Offline);
    }

    #[test]
    fn a_waypoint_with_nowhere_to_go_is_not_added() {
        let fleet = fleet();
        let mut offline = MissionDraft::new(None);
        assert_eq!(offline.add_waypoint(&fleet.snapshot(), 50.0), None);
        assert!(offline.plan().is_empty());

        // A vehicle with neither a position nor a home knows no better.
        let mut draft = MissionDraft::new(Some(SYSTEM));
        assert_eq!(draft.add_waypoint(&fleet.snapshot(), 50.0), None);
    }

    #[test]
    fn new_waypoints_never_take_the_vehicles_altitude() {
        let mut fleet = fleet();
        let position = mavio::DefaultDialect::GlobalPositionInt(messages::GlobalPositionInt {
            lat: 550_000_000,
            lon: 120_000_000,
            alt: 0,
            relative_alt: 0,
            ..Default::default()
        });
        fleet.observe(ADDRESS, Instant::now(), Some(&position));
        let snapshot = fleet.snapshot();

        // The first waypoint goes where the vehicle is, at the default
        // altitude, not at the vehicle's (zero, on the ground).
        let mut draft = MissionDraft::new(Some(SYSTEM));
        let first = draft.add_waypoint(&snapshot, 50.0).unwrap();
        let item = draft.plan().get(first).unwrap().position().unwrap();
        assert!((item.lat - 55.0).abs() < 1e-9 && (item.lon - 12.0).abs() < 1e-9);
        assert_eq!(item.alt_m, 50.0);

        // Later ones go where the plan ends, at its altitude.
        draft.plan_mut().update(first, |item| {
            item.set_lat_lon(56.0, 13.0);
            item.set_field_from_str(Field::Altitude, "80");
        });
        let second = draft.add_waypoint(&snapshot, 50.0).unwrap();
        let item = draft.plan().get(second).unwrap().position().unwrap();
        assert_eq!((item.lat, item.lon, item.alt_m), (56.0, 13.0, 80.0));
    }
}
