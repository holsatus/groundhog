//! The mission feature: planning flight plans and transferring them.
//!
//! The editor keeps one [`MissionDraft`] per vehicle, plus an offline draft
//! for planning with no vehicle selected; the draft decides when it follows
//! the vehicle and how it relates to it. Around each draft the editor keeps
//! what only the UI needs: the selected item, half-typed field text, and
//! whether map clicks place waypoints. Items are addressed by [`ItemId`], so
//! map drags and editor input stay attached to the item they started on
//! whatever happens to the list meanwhile.

use std::{collections::BTreeMap, path::PathBuf};

use groundhog_logic::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        geo::GeoPosition,
        mission::{
            Field, ItemId, MissionDraft, MissionItem, MissionPlan, OnboardMission, SyncStatus,
        },
    },
    services::{Result, TransferProgress},
    storage::mission::{WplMission, parse_wpl, write_wpl},
};
use mavio::default_dialect::enums::MavCmd;
use tokio::sync::mpsc;

use super::{
    Context, Effect, file_dialog,
    notifications::Notice,
    operations::{Operations, Ticket},
};
use crate::style;

/// Which draft: a vehicle's, or the offline plan (`None`).
pub type PlanKey = Option<SystemId>;

/// The vehicle operations the editor runs, one of each per vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MissionOp {
    Download,
    Upload,
    Clear,
    Start,
    Pause,
    Resume,
    SetCurrent,
}

impl MissionOp {
    pub fn describe(self) -> &'static str {
        match self {
            MissionOp::Download => "Mission download",
            MissionOp::Upload => "Mission upload",
            MissionOp::Clear => "Clearing the mission",
            MissionOp::Start => "Starting the mission",
            MissionOp::Pause => "Pausing the mission",
            MissionOp::Resume => "Resuming the mission",
            MissionOp::SetCurrent => "Jumping to the item",
        }
    }

    /// Whether the operation transfers the mission (and blocks other
    /// transfers and edits that would race it).
    fn is_transfer(self) -> bool {
        matches!(
            self,
            MissionOp::Download | MissionOp::Upload | MissionOp::Clear
        )
    }
}

/// How a draft relates to its vehicle's onboard mission.
type Key = (SystemId, MissionOp);

/// What the editor's operations and file tasks report back.
enum MissionEvent {
    /// Progress of a transfer, per run.
    Progress(SystemId, MissionOp, Ticket, TransferProgress),
    /// The outcome of a transfer or a mission command.
    Finished(SystemId, MissionOp, Ticket, Result<Option<OnboardMission>>),
    /// A mission file was read for `plan`.
    Loaded(PathBuf, PlanKey, std::result::Result<WplMission, String>),
    /// A mission file was written.
    Saved(PathBuf, std::result::Result<(), String>),
}

/// One draft and its editor state.
#[derive(Debug)]
struct Draft {
    mission: MissionDraft,
    selected: Option<ItemId>,
    /// In-progress text of the selected item's fields.
    edits: BTreeMap<Field, String>,
    /// Map clicks place waypoints.
    placing: bool,
}

impl Draft {
    fn new(plan: PlanKey) -> Self {
        Self {
            mission: MissionDraft::new(plan),
            selected: None,
            edits: BTreeMap::new(),
            placing: false,
        }
    }

    fn plan(&self) -> &MissionPlan {
        self.mission.plan()
    }

    fn plan_mut(&mut self) -> &mut MissionPlan {
        self.mission.plan_mut()
    }

    /// The plan's items were replaced: their ids, and so the selection, no
    /// longer resolve.
    fn replaced(&mut self, replaced: bool) {
        if replaced {
            self.select(None);
        }
    }

    fn select(&mut self, item: Option<ItemId>) {
        if self.selected != item {
            self.selected = item;
            self.edits.clear();
        }
    }
}

pub struct MissionEditor {
    drafts: BTreeMap<PlanKey, Draft>,
    operations: Operations<Key>,
    sender: mpsc::UnboundedSender<MissionEvent>,
    results: mpsc::UnboundedReceiver<MissionEvent>,
}

impl Default for MissionEditor {
    fn default() -> Self {
        let mut drafts = BTreeMap::new();
        drafts.insert(None, Draft::new(None));
        let (sender, results) = mpsc::unbounded_channel();
        Self {
            drafts,
            operations: Operations::default(),
            sender,
            results,
        }
    }
}

impl MissionEditor {
    /// The draft for `plan`, created on first use.
    fn draft_mut(&mut self, plan: PlanKey) -> &mut Draft {
        self.drafts.entry(plan).or_insert_with(|| Draft::new(plan))
    }

    /// Fold a new fleet snapshot in: every vehicle gets a draft, and each
    /// draft follows its vehicle as [`MissionDraft::follow`] decides.
    pub fn sync(&mut self, fleet: &FleetSnapshot) {
        for system in fleet.ids() {
            let draft = self.draft_mut(Some(system));
            let replaced = draft.mission.follow(fleet);
            draft.replaced(replaced);
        }
    }

    /// How the draft for `plan` relates to its vehicle.
    pub fn status(&self, plan: PlanKey, fleet: &FleetSnapshot) -> SyncStatus {
        self.drafts
            .get(&plan)
            .map_or(SyncStatus::Unknown, |draft| draft.mission.status(fleet))
    }

    /// Whether a transfer is running for `system`.
    pub fn transferring(&self, system: SystemId) -> bool {
        self.operations
            .any(|(running, operation)| *running == system && operation.is_transfer())
    }

    /// The running transfer's progress, if any.
    pub fn transfer_progress(
        &self,
        system: SystemId,
    ) -> Option<(MissionOp, Option<TransferProgress>)> {
        [MissionOp::Download, MissionOp::Upload, MissionOp::Clear]
            .into_iter()
            .find(|operation| self.operations.is_running(&(system, *operation)))
            .map(|operation| (operation, self.operations.progress(&(system, operation))))
    }

    /// Drain the operations' and file tasks' results, turning them into plan
    /// updates and notices.
    pub fn collect(&mut self, cx: &mut Context) {
        while let Ok(event) = self.results.try_recv() {
            match event {
                MissionEvent::Progress(system, op, ticket, progress) => {
                    self.operations.report(&(system, op), ticket, progress);
                }
                MissionEvent::Finished(system, op, ticket, result) => {
                    self.finished(system, op, ticket, result, cx);
                }
                MissionEvent::Loaded(path, plan, result) => match result {
                    Ok(mission) => {
                        let count = mission.items.len();
                        let draft = self.draft_mut(plan);
                        draft.plan_mut().set_items(mission.items);
                        draft.select(None);
                        cx.notices.push(
                            Notice::success(format!("Loaded {count} mission items"))
                                .detail(path.display()),
                        );
                        cx.request(Effect::RememberDirectory(path));
                    }
                    Err(error) => cx.notices.push(
                        Notice::error(format!("Could not load {}", path.display())).detail(error),
                    ),
                },
                MissionEvent::Saved(path, result) => match result {
                    Ok(()) => {
                        cx.notices
                            .push(Notice::success("Mission saved").detail(path.display()));
                        cx.request(Effect::RememberDirectory(path));
                    }
                    Err(error) => cx.notices.push(
                        Notice::error(format!("Could not save {}", path.display())).detail(error),
                    ),
                },
            }
        }
    }

    /// What became of a transfer or command: plan updates and notices.
    fn finished(
        &mut self,
        system: SystemId,
        operation: MissionOp,
        ticket: Ticket,
        result: Result<Option<OnboardMission>>,
        cx: &mut Context,
    ) {
        if !self.operations.finish(&(system, operation), ticket) {
            return;
        }

        let mission = match result {
            Ok(mission) => mission,
            Err(error) => {
                cx.notices.push(
                    Notice::error(format!("{} failed", operation.describe()))
                        .detail(error)
                        .vehicle(system),
                );
                return;
            }
        };

        let draft = self.draft_mut(Some(system));
        match operation {
            MissionOp::Download | MissionOp::Upload => {
                let mission = mission.unwrap_or_else(OnboardMission::empty);
                let replaced = draft.mission.transferred(&mission);
                draft.replaced(replaced);

                let count = mission.items.len();
                let title = match operation {
                    MissionOp::Download => format!("Downloaded {count} mission items"),
                    _ => format!("Uploaded {count} mission items"),
                };
                cx.notices.push(Notice::success(title).vehicle(system));
            }
            MissionOp::Clear => {
                draft.mission.cleared();
                cx.notices
                    .push(Notice::success("Mission cleared on the vehicle").vehicle(system));
            }
            MissionOp::Start | MissionOp::Pause | MissionOp::Resume | MissionOp::SetCurrent => {}
        }
    }

    /// Perform a mission action: ask for confirmation on the consequential
    /// ones, and start operations. `confirmed` skips the confirmation (the
    /// operator already agreed).
    pub fn perform(
        &mut self,
        plan: PlanKey,
        operation: MissionOp,
        item: Option<ItemId>,
        confirmed: bool,
        cx: &mut Context,
    ) {
        let Some(system) = plan else {
            return;
        };

        match operation {
            MissionOp::Download => {
                if !confirmed
                    && self
                        .drafts
                        .get(&plan)
                        .is_some_and(|draft| draft.mission.is_modified())
                {
                    cx.confirm(
                        super::confirm::Confirmation::new(
                            "Replace your changes?",
                            "Downloading replaces the plan you edited with the vehicle's mission.",
                            super::confirm::Action::Mission {
                                plan,
                                op: MissionOp::Download,
                                item: None,
                            },
                        )
                        .accept_label("Download"),
                    );
                    return;
                }

                let client = cx.services.vehicle(system);
                self.transfer(system, MissionOp::Download, move |progress| async move {
                    client.download_mission(&*progress).await.map(Some)
                });
            }
            MissionOp::Upload => {
                let armed = cx
                    .fleet
                    .vehicle(system)
                    .and_then(|vehicle| vehicle.armed())
                    .unwrap_or(false);

                if !confirmed && armed {
                    cx.confirm(
                        super::confirm::Confirmation::new(
                            format!("Upload to armed {}?", cx.fleet.vehicle_name(system)),
                            "The vehicle is armed. If it is flying the mission, the new plan takes effect immediately.",
                            super::confirm::Action::Mission {
                                plan,
                                op: MissionOp::Upload,
                                item: None,
                            },
                        )
                        .accept_label("Upload")
                        .danger(),
                    );
                    return;
                }

                let Some(draft) = self.drafts.get(&plan) else {
                    return;
                };
                let items = draft.plan().mission_items();
                let client = cx.services.vehicle(system);
                self.transfer(system, MissionOp::Upload, move |progress| async move {
                    client.upload_mission(items, &*progress).await.map(Some)
                });
            }
            MissionOp::Clear => {
                if !confirmed {
                    cx.confirm(
                        super::confirm::Confirmation::new(
                            format!("Clear the mission on {}?", cx.fleet.vehicle_name(system)),
                            "The vehicle's stored mission is deleted. Your plan here is kept.",
                            super::confirm::Action::Mission {
                                plan,
                                op: MissionOp::Clear,
                                item: None,
                            },
                        )
                        .accept_label("Clear")
                        .danger(),
                    );
                    return;
                }

                let client = cx.services.vehicle(system);
                self.transfer(system, MissionOp::Clear, move |_| async move {
                    client.clear_mission().await.map(|()| None)
                });
            }
            MissionOp::Start => {
                if !confirmed {
                    cx.confirm(
                        super::confirm::Confirmation::new(
                            format!("Start the mission on {}?", cx.fleet.vehicle_name(system)),
                            "The vehicle starts flying its stored mission from the beginning.",
                            super::confirm::Action::Mission {
                                plan,
                                op: MissionOp::Start,
                                item: None,
                            },
                        )
                        .accept_label("Start"),
                    );
                    return;
                }
                self.command(system, MissionOp::Start, cx);
            }
            MissionOp::Pause | MissionOp::Resume => {
                self.command(system, operation, cx);
            }
            MissionOp::SetCurrent => {
                let Some(item_id) = item else { return };

                // Indices only mean the same thing on the vehicle when the
                // plan is what the vehicle holds.
                if self.status(plan, cx.fleet) != SyncStatus::Synced {
                    cx.notices.push(
                        Notice::warning("Upload the plan first")
                            .detail("The plan differs from the vehicle's mission, so its item numbers do not match.")
                            .vehicle(system),
                    );
                    return;
                }

                let Some(index) = self
                    .drafts
                    .get(&plan)
                    .and_then(|draft| draft.plan().index_of(item_id))
                else {
                    return;
                };

                if !confirmed {
                    cx.confirm(
                        super::confirm::Confirmation::new(
                            format!("Continue at item {}?", index + 1),
                            "The vehicle skips to this item of its mission.",
                            super::confirm::Action::Mission {
                                plan,
                                op: MissionOp::SetCurrent,
                                item: Some(item_id),
                            },
                        )
                        .accept_label("Jump"),
                    );
                    return;
                }

                let client = cx.services.vehicle(system);
                let sender = self.sender.clone();
                self.operations.start(
                    (system, MissionOp::SetCurrent),
                    async move { client.set_current_mission_item(index).await.map(|()| None) },
                    sender,
                    move |ticket, result| {
                        MissionEvent::Finished(system, MissionOp::SetCurrent, ticket, result)
                    },
                );
            }
        }
    }

    /// Run a mission command (start, pause, resume) that needs no
    /// confirmation.
    fn command(&mut self, system: SystemId, operation: MissionOp, cx: &mut Context) {
        let client = cx.services.vehicle(system);
        let sender = self.sender.clone();
        self.operations.start(
            (system, operation),
            async move {
                match operation {
                    MissionOp::Start => client.start_mission().await,
                    MissionOp::Pause => client.pause_mission(true).await,
                    _ => client.pause_mission(false).await,
                }
                .map(|()| None)
            },
            sender,
            move |ticket, result| MissionEvent::Finished(system, operation, ticket, result),
        );
    }

    /// Start a transfer with progress, unless one is already running.
    fn transfer<Fut>(
        &mut self,
        system: SystemId,
        operation: MissionOp,
        run: impl FnOnce(super::operations::Progress) -> Fut + Send + 'static,
    ) where
        Fut: std::future::Future<Output = Result<Option<OnboardMission>>> + Send + 'static,
    {
        if self.transferring(system) {
            return;
        }

        let sender = self.sender.clone();
        self.operations.start_with_progress(
            (system, operation),
            run,
            sender,
            move |ticket, progress| MissionEvent::Progress(system, operation, ticket, progress),
            move |ticket, result| MissionEvent::Finished(system, operation, ticket, result),
        );
    }

    // --- Editing ----------------------------------------------------------

    /// Whether "Place on map" is on for `plan`.
    pub fn is_placing(&self, plan: PlanKey) -> bool {
        self.drafts.get(&plan).is_some_and(|draft| draft.placing)
    }

    /// A read over the draft's plan, for the map layers.
    pub fn plan(&self, plan: PlanKey) -> Option<&MissionPlan> {
        self.drafts.get(&plan).map(Draft::plan)
    }

    /// The draft's selected item, for the map's highlighted marker.
    pub fn selected_item(&self, plan: PlanKey) -> Option<ItemId> {
        self.drafts.get(&plan).and_then(|draft| draft.selected)
    }

    /// Apply an edit to a draft. Edits to a vehicle's draft are refused while
    /// a transfer runs, so the transfer's result cannot clobber them.
    fn edit_guard(&mut self, plan: PlanKey) -> Option<&mut Draft> {
        match plan {
            Some(system) if self.transferring(system) => None,
            _ => Some(self.draft_mut(plan)),
        }
    }

    /// Append a waypoint where [`MissionDraft::add_waypoint`] puts it.
    pub fn add_waypoint(&mut self, plan: PlanKey, fleet: &FleetSnapshot, default_altitude: f32) {
        let Some(draft) = self.edit_guard(plan) else {
            return;
        };

        let id = draft.mission.add_waypoint(fleet, default_altitude);
        draft.select(Some(id));
    }

    /// Place a waypoint where the map was clicked, while placing is active.
    pub fn place_at(&mut self, plan: PlanKey, at: GeoPosition, default_altitude: f32) {
        let Some(draft) = self.edit_guard(plan) else {
            return;
        };

        if draft.placing {
            let id = draft.mission.place_waypoint(at, default_altitude);
            draft.select(Some(id));
        }
    }

    /// Toggle "Place on map".
    pub fn toggle_placing(&mut self, plan: PlanKey) {
        if let Some(draft) = self.edit_guard(plan) {
            draft.placing = !draft.placing;
        }
    }

    pub fn select_item(&mut self, plan: PlanKey, item: Option<ItemId>) {
        let Some(draft) = self.edit_guard(plan) else {
            return;
        };
        let item = item.filter(|item| draft.plan().get(*item).is_some());
        draft.select(item);
    }

    pub fn remove_item(&mut self, plan: PlanKey, item: ItemId) {
        let Some(draft) = self.edit_guard(plan) else {
            return;
        };

        draft.plan_mut().remove(item);
        if draft.selected == Some(item) {
            draft.select(None);
        }
    }

    pub fn move_item(&mut self, plan: PlanKey, item: ItemId, up: bool) {
        if let Some(draft) = self.edit_guard(plan) {
            if up {
                draft.plan_mut().move_up(item);
            } else {
                draft.plan_mut().move_down(item);
            }
        }
    }

    pub fn change_command(&mut self, plan: PlanKey, item: ItemId, command: MavCmd) {
        if let Some(draft) = self.edit_guard(plan)
            && draft
                .plan_mut()
                .update(item, |current| *current = current.with_command(command))
            && draft.selected == Some(item)
        {
            draft.edits.clear();
        }
    }

    /// Set one field of `item` from editor text, and remember the text so
    /// partial input survives the next redraw.
    pub fn edit_field(&mut self, plan: PlanKey, item: ItemId, field: Field, text: &str) {
        let typed = text.to_owned();
        if let Some(draft) = self.edit_guard(plan) {
            draft.plan_mut().update(item, |current| {
                current.set_field_from_str(field, &typed);
            });
            if draft.selected == Some(item) {
                draft.edits.insert(field, typed);
            }
        }
    }

    /// Empty the draft; the vehicle is untouched.
    pub fn discard(&mut self, plan: PlanKey) {
        let Some(draft) = self.edit_guard(plan) else {
            return;
        };

        draft.plan_mut().clear();
        draft.select(None);
    }

    /// Move an item from a map drag, live. The item keeps its id, so the
    /// drag never redirects to another item.
    pub fn set_item_position(&mut self, plan: PlanKey, item: ItemId, at: GeoPosition) {
        let Some(draft) = self.edit_guard(plan) else {
            return;
        };

        draft
            .plan_mut()
            .update(item, |current| current.set_lat_lon(at.lat, at.lon));
        if draft.selected == Some(item) {
            // Show the dragged position rather than stale typing.
            draft.edits.remove(&Field::Latitude);
            draft.edits.remove(&Field::Longitude);
        }
    }

    // --- Files ------------------------------------------------------------

    /// Ask for a destination and write the plan's items into it. The items
    /// and home are snapshotted now, not when the dialog closes.
    pub fn save_dialog(&mut self, plan: PlanKey, cx: &mut Context) {
        let Some(draft) = self.drafts.get(&plan) else {
            return;
        };

        let items = draft.plan().mission_items();
        let home = plan
            .and_then(|system| cx.fleet.vehicle(system))
            .and_then(|vehicle| {
                vehicle
                    .home()
                    .map(|home| home.mission_item())
                    .or_else(|| vehicle.mission.onboard.as_ref()?.home.clone())
            });
        let directory = cx.directory.map(|directory| directory.to_path_buf());
        let sender = self.sender.clone();

        tokio::spawn(async move {
            let dialog = file_dialog(directory.as_deref()).set_file_name("mission.waypoints");

            let Some(path) = dialog.save_file().await.map(|file| file.path().to_owned()) else {
                return;
            };

            // The write runs off the async runtime's workers; the UI never
            // blocks on the filesystem.
            let write_path = path.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                std::fs::write(&write_path, write_wpl(&items, home.as_ref()))
                    .map_err(|error| error.to_string())
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));

            let _ = sender.send(MissionEvent::Saved(path, outcome));
        });
    }

    /// Ask for a mission file and load it into the draft.
    pub fn load_dialog(&mut self, plan: PlanKey, cx: &mut Context) {
        let directory = cx.directory.map(|directory| directory.to_path_buf());
        let sender = self.sender.clone();

        tokio::spawn(async move {
            let dialog = file_dialog(directory.as_deref());

            let Some(path) = dialog.pick_file().await.map(|file| file.path().to_owned()) else {
                return;
            };

            let read_path = path.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                std::fs::read_to_string(&read_path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| parse_wpl(&text).map_err(|error| error.to_string()))
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));

            let _ = sender.send(MissionEvent::Loaded(path, plan, outcome));
        });
    }

    // --- Deferred application ---------------------------------------------

    /// Apply the intents drawing collected. All draft mutations happen here,
    /// after drawing, where `&mut self` is free.
    pub fn apply(&mut self, intents: &[MissionIntent], cx: &mut Context) {
        let default_altitude = cx.settings.default_waypoint_altitude_m;

        for intent in intents {
            match intent {
                MissionIntent::Perform { plan, op, item } => {
                    self.perform(*plan, *op, *item, false, cx);
                }
                MissionIntent::Cancel { plan, op } => {
                    if let Some(system) = *plan
                        && self.operations.cancel(&(system, *op))
                    {
                        cx.notices.push(
                            Notice::info(format!("{} cancelled", op.describe())).vehicle(system),
                        );
                    }
                }
                MissionIntent::Save { plan } => self.save_dialog(*plan, cx),
                MissionIntent::Load { plan } => self.load_dialog(*plan, cx),
                MissionIntent::AddWaypoint { plan } => {
                    self.add_waypoint(*plan, cx.fleet, default_altitude);
                }
                MissionIntent::TogglePlacing { plan } => self.toggle_placing(*plan),
                MissionIntent::Discard { plan } => self.discard(*plan),
                MissionIntent::Select { plan, item } => {
                    self.select_item(*plan, *item);
                }
                MissionIntent::ChangeCommand {
                    plan,
                    item,
                    command,
                } => {
                    self.change_command(*plan, *item, *command);
                }
                MissionIntent::EditField {
                    plan,
                    item,
                    field,
                    text,
                } => {
                    self.edit_field(*plan, *item, *field, text);
                }
                MissionIntent::MoveItem { plan, item, up } => {
                    self.move_item(*plan, *item, *up);
                }
                MissionIntent::Remove { plan, item } => {
                    self.remove_item(*plan, *item);
                }
                MissionIntent::At { plan, item, at } => {
                    self.set_item_position(*plan, *item, *at);
                }
                MissionIntent::PlaceAt { plan, at } => {
                    self.place_at(*plan, *at, default_altitude);
                }
            }
        }
    }
}

/// What drawing discovered the operator wants done. Applied after drawing,
/// when `&mut self` is free again.
#[derive(Debug, Clone, PartialEq)]
pub enum MissionIntent {
    /// Run a mission operation (confirmation happens inside `perform`).
    Perform {
        plan: PlanKey,
        op: MissionOp,
        item: Option<ItemId>,
    },
    /// Cancel a running transfer.
    Cancel { plan: PlanKey, op: MissionOp },
    /// Save the plan to a file.
    Save { plan: PlanKey },
    /// Load a plan from a file.
    Load { plan: PlanKey },
    /// Append a waypoint at the plan's next position.
    AddWaypoint { plan: PlanKey },
    /// Toggle placing waypoints by clicking the map.
    TogglePlacing { plan: PlanKey },
    /// Empty the draft; the vehicle is untouched.
    Discard { plan: PlanKey },
    /// Select an item (or none).
    Select { plan: PlanKey, item: Option<ItemId> },
    /// Change an item's command.
    ChangeCommand {
        plan: PlanKey,
        item: ItemId,
        command: MavCmd,
    },
    /// Set one item field from editor text.
    EditField {
        plan: PlanKey,
        item: ItemId,
        field: Field,
        text: String,
    },
    /// Reorder an item, up or down.
    MoveItem {
        plan: PlanKey,
        item: ItemId,
        up: bool,
    },
    /// Remove an item.
    Remove { plan: PlanKey, item: ItemId },
    /// Dragged at a map position, live.
    At {
        plan: PlanKey,
        item: ItemId,
        at: GeoPosition,
    },
    /// A map click while placing.
    PlaceAt { plan: PlanKey, at: GeoPosition },
}

impl MissionEditor {
    /// The panel: toolbars, status, transfer progress, item list, editor.
    /// Entirely `&self`: edits go through [`MissionIntent`]s, applied by
    /// [`MissionEditor::apply`].
    pub fn view(
        &self,
        ui: &mut egui::Ui,
        fleet: &FleetSnapshot,
        selected: Option<SystemId>,
        intents: &mut Vec<MissionIntent>,
    ) {
        let plan: PlanKey = selected.filter(|system| fleet.contains(*system));
        let Some(draft) = self.drafts.get(&plan) else {
            return;
        };

        let status = self.status(plan, fleet);
        let assigning = plan.is_some_and(|system| self.transferring(system));
        let editable = !assigning;

        // Transfer toolbar.
        ui.horizontal_wrapped(|ui| {
            for (label, op) in [
                ("Download", MissionOp::Download),
                ("Upload", MissionOp::Upload),
                ("Clear vehicle", MissionOp::Clear),
                ("Start", MissionOp::Start),
                ("Pause", MissionOp::Pause),
                ("Resume", MissionOp::Resume),
            ] {
                let enabled = plan.is_some() && !assigning;
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    intents.push(MissionIntent::Perform {
                        plan,
                        op,
                        item: None,
                    });
                }
            }
        });

        // Editing toolbar.
        let placing = draft.placing;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(editable, egui::Button::new("Add waypoint"))
                .clicked()
            {
                intents.push(MissionIntent::AddWaypoint { plan });
            }
            if ui
                .add_enabled(
                    editable,
                    egui::Button::new(if placing {
                        "Placing… (click map)"
                    } else {
                        "Place on map"
                    })
                    .fill(if placing {
                        style::map::SELECTED.gamma_multiply(0.6)
                    } else {
                        egui::Color32::TRANSPARENT
                    }),
                )
                .clicked()
            {
                intents.push(MissionIntent::TogglePlacing { plan });
            }
            if ui
                .add_enabled(editable, egui::Button::new("Discard plan"))
                .clicked()
            {
                intents.push(MissionIntent::Discard { plan });
            }
            if ui
                .add_enabled(editable, egui::Button::new("Save…"))
                .clicked()
            {
                intents.push(MissionIntent::Save { plan });
            }
            if ui
                .add_enabled(editable, egui::Button::new("Load…"))
                .clicked()
            {
                intents.push(MissionIntent::Load { plan });
            }
        });

        let status_line = (status_text(status), status_color(status));
        let count = match draft.plan().len() {
            1 => "1 item".to_owned(),
            count => format!("{count} items"),
        };
        ui.label(
            egui::RichText::new(format!("{} · {}", status_line.0, count)).color(status_line.1),
        );

        // Transfer progress and cancel.
        if let Some(system) = plan
            && let Some((operation, progress)) = self.transfer_progress(system)
        {
            let (done, total) = progress.map_or((0.0, 1.0), |progress| {
                (f32::from(progress.done), f32::from(progress.total.max(1)))
            });

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(match progress {
                            Some(progress) => format!(
                                "{}: {}/{}",
                                operation.describe(),
                                progress.done,
                                progress.total
                            ),
                            None => format!("{}…", operation.describe()),
                        })
                        .size(11.0),
                    );
                    ui.add(egui::ProgressBar::new(done / total.max(1.0)).desired_height(6.0));
                });
                if ui.add(egui::Button::new("Cancel")).clicked() {
                    intents.push(MissionIntent::Cancel {
                        plan,
                        op: operation,
                    });
                }
            });
        }

        // The list, and the selected item's editor, share the height.
        let current = plan
            .and_then(|system| fleet.vehicle(system))
            .filter(|_| status == SyncStatus::Synced)
            .and_then(|vehicle| vehicle.current_mission_index());

        if let Some(id) = draft.selected
            && draft.plan().get(id).is_some()
        {
            let half = ui.available_height() * 0.45;

            ui.allocate_ui(egui::vec2(ui.available_width(), half), |list_ui| {
                view_list(list_ui, draft, current, plan, intents);
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().show(ui, |editor_ui| {
                view_item_editor(editor_ui, plan, draft, status, editable, intents);
            });
        } else {
            view_list(ui, draft, current, plan, intents);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Select an item to edit it.")
                    .size(11.0)
                    .weak(),
            );
        }
    }
}

fn status_text(status: SyncStatus) -> String {
    match status {
        SyncStatus::Offline => "Offline plan".to_owned(),
        SyncStatus::Unknown => "Vehicle mission not downloaded".to_owned(),
        SyncStatus::Synced => "Synced with the vehicle".to_owned(),
        SyncStatus::Modified => "Modified (not uploaded)".to_owned(),
        SyncStatus::VehicleChanged => {
            "The vehicle's mission was changed elsewhere; download it".to_owned()
        }
    }
}

fn status_color(status: SyncStatus) -> egui::Color32 {
    match status {
        SyncStatus::Offline | SyncStatus::Unknown => style::severity::DEBUG,
        SyncStatus::Synced => style::map::HOME,
        SyncStatus::Modified => style::severity::WARNING,
        SyncStatus::VehicleChanged => style::severity::ERROR,
    }
}

/// Height of a single mission-list row.
const ROW_HEIGHT: f32 = 32.0;
/// Column starts, in points from the row's left edge.
const COLUMN_INDEX: f32 = 6.0;
const COLUMN_NAME: f32 = 40.0;
const COLUMN_SUMMARY: f32 = 176.0;
const COLUMN_CURRENT: f32 = 60.0;

/// A one-line summary of an item for the list.
fn summary(item: &MissionItem) -> String {
    if let Some(position) = item.position() {
        return format!(
            "{:.6}, {:.6}, {:.1} m",
            position.lat, position.lon, position.alt_m
        );
    }

    item.fields()
        .iter()
        .filter(|view| !view.value.is_nan())
        .take(3)
        .map(|view| format!("{} {}", view.label, format_field_value(*view)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A field value formatted for display and editing.
fn format_field_value(view: groundhog_logic::domain::mission::FieldView) -> String {
    use groundhog_logic::domain::mission::{Field, FieldKind};

    if view.value.is_nan() {
        return String::new();
    }

    match (view.field, view.kind) {
        (Field::Latitude | Field::Longitude, _) => format!("{:.7}", view.value),
        (_, FieldKind::Integer | FieldKind::ItemNumber) => format!("{}", view.value.round()),
        _ => view.value.to_string(),
    }
}

/// The item list: one fixed-height row per item, so it virtualises.
fn view_list(
    ui: &mut egui::Ui,
    draft: &Draft,
    current: Option<usize>,
    plan: PlanKey,
    intents: &mut Vec<MissionIntent>,
) {
    if draft.plan().is_empty() {
        ui.label(egui::RichText::new("The plan is empty.").size(11.0).weak());
        return;
    }

    let selected = draft.selected;
    let items = draft.plan().items();
    let count = items.len();

    egui::ScrollArea::vertical().show_rows(ui, ROW_HEIGHT, count, |ui, range| {
        for index in range {
            let planned = &items[index];
            let id = planned.id();
            let item = &planned.item;

            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ROW_HEIGHT),
                egui::Sense::click(),
            );

            // Row background: selection, hover, then stripes.
            let fill = if selected == Some(id) {
                style::map::SELECTED.gamma_multiply(0.35)
            } else if response.hovered() {
                egui::Color32::from_rgb(0x2e, 0x33, 0x3c)
            } else if index % 2 == 1 {
                egui::Color32::from_rgb(0x24, 0x29, 0x30)
            } else {
                egui::Color32::TRANSPARENT
            };
            if fill != egui::Color32::TRANSPARENT {
                ui.painter().rect_filled(rect, 2.0, fill);
            }

            let painter = ui.painter();
            let font = egui::TextStyle::Body.resolve(ui.style());
            painter.text(
                rect.left_top() + egui::vec2(COLUMN_INDEX, ROW_HEIGHT / 2.0),
                egui::Align2::LEFT_CENTER,
                (index + 1).to_string(),
                font.clone(),
                ui.visuals().text_color(),
            );
            painter.text(
                rect.left_top() + egui::vec2(COLUMN_NAME, ROW_HEIGHT / 2.0),
                egui::Align2::LEFT_CENTER,
                item.name().as_ref(),
                font.clone(),
                ui.visuals().strong_text_color(),
            );
            painter.text(
                rect.left_top() + egui::vec2(COLUMN_SUMMARY, ROW_HEIGHT / 2.0),
                egui::Align2::LEFT_CENTER,
                summary(item),
                font.clone(),
                ui.visuals().text_color(),
            );
            if current == Some(index) {
                painter.text(
                    rect.right_center() - egui::vec2(COLUMN_CURRENT, 0.0),
                    egui::Align2::LEFT_CENTER,
                    "▶ current",
                    font,
                    style::map::HOME,
                );
            }

            if response.clicked() {
                intents.push(MissionIntent::Select {
                    plan,
                    item: Some(id),
                });
            }
        }
    });
}

/// The selected item's editor: command picker, fields, and actions.
fn view_item_editor(
    ui: &mut egui::Ui,
    plan: PlanKey,
    draft: &Draft,
    status: SyncStatus,
    editable: bool,
    intents: &mut Vec<MissionIntent>,
) {
    use groundhog_logic::domain::mission::command::{SPECS, spec_of};

    let Some(id) = draft.selected else { return };
    let Some(item) = draft.plan().get(id) else {
        return;
    };

    let index = draft.plan().index_of(id).unwrap_or_default();
    ui.separator();
    ui.horizontal(|ui| {
        ui.heading(format!("Item {}", index + 1));

        // The command picker: the table's specs, plus the item's own command
        // when the editor does not know it.
        let choices: Vec<MavCmd> = SPECS
            .iter()
            .map(|spec| spec.command)
            .chain(std::iter::once(item.command).filter(|_| item.spec().is_none()))
            .collect();

        egui::ComboBox::from_id_salt("mission.command")
            .selected_text(item.name().as_ref())
            .width(ui.available_width() - 16.0)
            .show_ui(ui, |combo| {
                for command in choices {
                    let label = spec_of(command)
                        .map_or_else(|| format!("{command:?}"), |spec| spec.name.to_owned());
                    if combo
                        .selectable_label(command == item.command, label)
                        .clicked()
                        && command != item.command
                    {
                        intents.push(MissionIntent::ChangeCommand {
                            plan,
                            item: id,
                            command,
                        });
                    }
                }
            });
    });

    // The fields the command table describes, or the raw wire fields.
    egui::Grid::new("mission.fields")
        .num_columns(3)
        .show(ui, |grid| {
            for view in item.fields() {
                grid.label(view.label);
                field_widget(grid, draft, plan, view, id, intents);
                grid.label(egui::RichText::new(view.units).weak().size(11.0));
                grid.end_row();
            }
        });

    // Item actions.
    let synced = plan.is_some() && status == SyncStatus::Synced;
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(editable, egui::Button::new("Move up"))
            .clicked()
        {
            intents.push(MissionIntent::MoveItem {
                plan,
                item: id,
                up: true,
            });
        }
        if ui
            .add_enabled(editable, egui::Button::new("Move down"))
            .clicked()
        {
            intents.push(MissionIntent::MoveItem {
                plan,
                item: id,
                up: false,
            });
        }
        if ui
            .add_enabled(editable, egui::Button::new("Remove"))
            .clicked()
        {
            intents.push(MissionIntent::Remove { plan, item: id });
        }
        if ui
            .add_enabled(synced && editable, egui::Button::new("Continue here"))
            .clicked()
        {
            intents.push(MissionIntent::Perform {
                plan,
                op: MissionOp::SetCurrent,
                item: Some(id),
            });
        }
        if ui
            .add_enabled(editable, egui::Button::new("Deselect"))
            .clicked()
        {
            intents.push(MissionIntent::Select { plan, item: None });
        }
    });
}

/// The widget for one field: a checkbox for flags, a picker for choices, a
/// text box for numbers.
fn field_widget(
    ui: &mut egui::Ui,
    draft: &Draft,
    plan: PlanKey,
    view: groundhog_logic::domain::mission::FieldView,
    id: ItemId,
    intents: &mut Vec<MissionIntent>,
) {
    use groundhog_logic::domain::mission::FieldKind;

    let mut push_edit = |text: String| {
        intents.push(MissionIntent::EditField {
            plan,
            item: id,
            field: view.field,
            text,
        });
    };

    match view.kind {
        FieldKind::Bool => {
            let mut checked = view.value != 0.0;
            if ui.checkbox(&mut checked, "").changed() {
                push_edit(if checked {
                    "1".to_owned()
                } else {
                    "0".to_owned()
                });
            }
        }
        FieldKind::Choice(options) => {
            let selected = options
                .iter()
                .copied()
                .find(|(value, _)| f64::from(*value) == view.value);

            let id_salt = egui::Id::new(("mission.field", view.field, id.raw()));
            if let Some((_, label)) = selected {
                egui::ComboBox::from_id_salt(id_salt)
                    .selected_text(label)
                    .width(ui.available_width())
                    .show_ui(ui, |combo| {
                        for (value, label) in options {
                            if combo
                                .selectable_label(f64::from(*value) == view.value, *label)
                                .clicked()
                            {
                                push_edit(value.to_string());
                            }
                        }
                    });
            } else {
                // A value outside the known choices (from another planner):
                // edit it as a number rather than hide it.
                number_field(ui, draft, view, id, plan, intents);
            }
        }
        FieldKind::Number
        | FieldKind::Integer
        | FieldKind::OptionalNumber
        | FieldKind::ItemNumber => {
            number_field(ui, draft, view, id, plan, intents);
        }
    }
}

/// A number as a text box whose value is parsed per keystroke, like the
/// iced editor's `on_input`.
fn number_field(
    ui: &mut egui::Ui,
    draft: &Draft,
    view: groundhog_logic::domain::mission::FieldView,
    id: ItemId,
    plan: PlanKey,
    intents: &mut Vec<MissionIntent>,
) {
    use groundhog_logic::domain::mission::FieldKind;

    let value = draft
        .edits
        .get(&view.field)
        .cloned()
        .unwrap_or_else(|| format_field_value(view));

    let mut buffer = value;
    let hint = if view.kind == FieldKind::OptionalNumber {
        "vehicle default"
    } else {
        ""
    };

    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut buffer)
                .hint_text(hint)
                .desired_width(ui.available_width()),
        )
        .changed();

    if changed {
        intents.push(MissionIntent::EditField {
            plan,
            item: id,
            field: view.field,
            text: buffer,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use groundhog_logic::{
        Backend, BackendOptions, MavlinkId,
        domain::{fleet::Fleet, mission::Position},
        services::{Error, FleetHandle},
    };
    use mavio::default_dialect::{
        enums::{MavAutopilot, MavModeFlag, MavType},
        messages,
    };

    use super::*;
    use crate::config::Settings;
    use crate::features::{
        confirm::Action,
        notifications::{Level, Notifications},
    };

    const A: SystemId = SystemId(1);

    /// The environment the tests drive the editor through: a real backend,
    /// a folded fleet, and the feature's own state.
    struct Env {
        fleet: Fleet,
        handle: FleetHandle,
        notices: Notifications,
        settings: Settings,
    }

    impl Env {
        fn new() -> Self {
            let (backend, _events) = Backend::start(BackendOptions {
                bootstrap: false,
                ..BackendOptions::default()
            });
            let handle = backend.fleet().clone();

            let mut fleet = Fleet::default();
            fleet.observe(
                MavlinkId {
                    system: A.0,
                    component: 1,
                },
                Instant::now(),
                Some(&mavio::DefaultDialect::Heartbeat(messages::Heartbeat {
                    type_: MavType::Quadrotor,
                    autopilot: MavAutopilot::Ardupilotmega,
                    base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
                    ..Default::default()
                })),
            );

            Self {
                fleet,
                handle,
                notices: Notifications::default(),
                settings: Settings::default(),
            }
        }

        /// Put the vehicle on the ground at a known spot (altitude zero).
        fn place_vehicle(&mut self) {
            self.fleet.observe(
                MavlinkId {
                    system: A.0,
                    component: 1,
                },
                Instant::now(),
                Some(&mavio::DefaultDialect::GlobalPositionInt(
                    messages::GlobalPositionInt {
                        lat: 474_000_000,
                        lon: 85_000_000,
                        relative_alt: 0,
                        ..Default::default()
                    },
                )),
            );
        }

        /// Apply intents the way the drawing loop does, returning what the
        /// editor requested of the application.
        fn apply(
            &mut self,
            editor: &mut MissionEditor,
            intents: Vec<MissionIntent>,
        ) -> Vec<Effect> {
            let snapshot = self.fleet.snapshot();
            let mut cx = Context::new(
                &snapshot,
                &self.handle,
                &mut self.notices,
                None,
                &self.settings,
            );
            editor.apply(&intents, &mut cx);
            cx.into_effects()
        }

        /// One intents worth: run a mission operation unconfirmed.
        fn perform(
            &mut self,
            editor: &mut MissionEditor,
            plan: PlanKey,
            op: MissionOp,
            item: Option<ItemId>,
        ) -> Vec<Effect> {
            self.apply(editor, vec![MissionIntent::Perform { plan, op, item }])
        }

        /// Simulate an operation run's outcome, as the middleware would.
        fn finish(
            &mut self,
            editor: &mut MissionEditor,
            system: SystemId,
            op: MissionOp,
            ticket: Ticket,
            result: Result<Option<OnboardMission>>,
        ) {
            let _ = editor
                .sender
                .send(MissionEvent::Finished(system, op, ticket, result));
            let snapshot = self.fleet.snapshot();
            let mut cx = Context::new(
                &snapshot,
                &self.handle,
                &mut self.notices,
                None,
                &self.settings,
            );
            editor.collect(&mut cx);
        }

        fn set_onboard(
            &mut self,
            editor: &mut MissionEditor,
            items: Vec<MissionItem>,
            opaque_id: u32,
        ) {
            self.fleet.set_onboard_mission(
                A,
                OnboardMission {
                    items,
                    home: None,
                    opaque_id,
                },
            );
            editor.sync(&self.fleet.snapshot());
        }
    }

    /// What the application does when the operator accepts a confirmation:
    /// re-enters `perform` with `confirmed: true`.
    fn confirmed_perform(
        env: &mut Env,
        editor: &mut MissionEditor,
        plan: PlanKey,
        op: MissionOp,
        item: Option<ItemId>,
    ) -> Vec<Effect> {
        let snapshot = env.fleet.snapshot();
        let mut cx = Context::new(
            &snapshot,
            &env.handle,
            &mut env.notices,
            None,
            &env.settings,
        );
        editor.perform(plan, op, item, true, &mut cx);
        cx.into_effects()
    }

    fn draft(editor: &MissionEditor, plan: PlanKey) -> &Draft {
        editor.drafts.get(&plan).expect("draft exists")
    }

    fn waypoint(lat: f64) -> MissionItem {
        MissionItem::waypoint(Position {
            lat,
            lon: 8.0,
            alt_m: 40.0,
        })
    }

    fn add_waypoint(plan: PlanKey) -> MissionIntent {
        MissionIntent::AddWaypoint { plan }
    }

    fn edit_altitude(plan: PlanKey, item: ItemId, text: &str) -> MissionIntent {
        MissionIntent::EditField {
            plan,
            item,
            field: Field::Altitude,
            text: text.to_owned(),
        }
    }

    #[tokio::test]
    async fn new_waypoints_never_take_the_vehicles_ground_altitude() {
        let mut env = Env::new();
        env.place_vehicle();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());

        env.apply(&mut editor, vec![add_waypoint(Some(A))]);

        let plan = &draft(&editor, Some(A)).plan();
        let position = plan.items()[0].item.position().unwrap();
        assert!((position.lat - 47.4).abs() < 1e-6, "placed at the vehicle");
        assert_eq!(
            position.alt_m, env.settings.default_waypoint_altitude_m,
            "at the default altitude, not the vehicle's zero"
        );

        // Further waypoints take the previous waypoint's altitude.
        let first = plan.items()[0].id();
        env.apply(&mut editor, vec![edit_altitude(Some(A), first, "120")]);
        env.apply(
            &mut editor,
            vec![
                MissionIntent::TogglePlacing { plan: Some(A) },
                MissionIntent::PlaceAt {
                    plan: Some(A),
                    at: GeoPosition::new(47.5, 8.5),
                },
            ],
        );
        let plan = &draft(&editor, Some(A)).plan();
        assert_eq!(plan.items()[1].item.position().unwrap().alt_m, 120.0);
    }

    #[tokio::test]
    async fn map_clicks_only_place_while_placing() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());

        env.apply(
            &mut editor,
            vec![MissionIntent::PlaceAt {
                plan: Some(A),
                at: GeoPosition::new(1.0, 1.0),
            }],
        );
        assert!(draft(&editor, Some(A)).plan().is_empty());

        env.apply(
            &mut editor,
            vec![
                MissionIntent::TogglePlacing { plan: Some(A) },
                MissionIntent::PlaceAt {
                    plan: Some(A),
                    at: GeoPosition::new(1.0, 1.0),
                },
            ],
        );
        assert_eq!(draft(&editor, Some(A)).plan().len(), 1);
    }

    #[tokio::test]
    async fn an_unmodified_draft_follows_the_onboard_mission() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());
        assert_eq!(
            editor.status(Some(A), &env.fleet.snapshot()),
            SyncStatus::Unknown
        );

        env.set_onboard(&mut editor, vec![waypoint(1.0)], 1);
        assert!(draft(&editor, Some(A)).plan().matches(&[waypoint(1.0)]));
        assert_eq!(
            editor.status(Some(A), &env.fleet.snapshot()),
            SyncStatus::Synced
        );

        // Another GCS uploads a different mission: still unmodified here, so
        // the draft follows.
        env.set_onboard(&mut editor, vec![waypoint(2.0), waypoint(3.0)], 2);
        assert_eq!(draft(&editor, Some(A)).plan().len(), 2);
    }

    #[tokio::test]
    async fn a_modified_draft_is_never_overwritten() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        env.set_onboard(&mut editor, vec![waypoint(1.0)], 1);

        env.apply(&mut editor, vec![add_waypoint(Some(A))]);
        assert_eq!(
            editor.status(Some(A), &env.fleet.snapshot()),
            SyncStatus::Modified
        );

        env.set_onboard(&mut editor, vec![waypoint(9.0)], 2);
        let plan = &draft(&editor, Some(A)).plan();
        assert_eq!(plan.len(), 2, "the operator's edit survives");
        assert_eq!(
            editor.status(Some(A), &env.fleet.snapshot()),
            SyncStatus::Modified
        );
    }

    #[tokio::test]
    async fn downloading_over_changes_asks_first() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());
        env.apply(&mut editor, vec![add_waypoint(Some(A))]);

        let effects = env.perform(&mut editor, Some(A), MissionOp::Download, None);
        assert!(
            matches!(&effects[..], [Effect::Confirm(confirmation)]
                if matches!(confirmation.action, Action::Mission { op: MissionOp::Download, .. })),
            "a confirmation that leads to the real download"
        );
        assert!(!editor.transferring(A), "nothing started yet");

        confirmed_perform(&mut env, &mut editor, Some(A), MissionOp::Download, None);
        assert!(editor.transferring(A));
    }

    #[tokio::test]
    async fn edits_wait_for_a_running_transfer() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());

        confirmed_perform(&mut env, &mut editor, Some(A), MissionOp::Download, None);
        assert!(editor.transferring(A));

        env.apply(&mut editor, vec![add_waypoint(Some(A))]);
        assert!(
            draft(&editor, Some(A)).plan().is_empty(),
            "refused during the download"
        );

        // The offline plan is unaffected by a vehicle's transfer.
        env.apply(&mut editor, vec![add_waypoint(None)]);
        assert_eq!(draft(&editor, None).plan().len(), 1);
    }

    #[tokio::test]
    async fn a_finished_download_replaces_the_draft_and_a_stale_one_is_ignored() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());

        confirmed_perform(&mut env, &mut editor, Some(A), MissionOp::Download, None);
        let ticket = editor
            .operations
            .ticket_of(&(A, MissionOp::Download))
            .unwrap();

        let mission = OnboardMission {
            items: vec![waypoint(5.0)],
            home: None,
            opaque_id: 3,
        };
        env.finish(
            &mut editor,
            A,
            MissionOp::Download,
            ticket,
            Ok(Some(mission.clone())),
        );
        assert!(draft(&editor, Some(A)).plan().matches(&mission.items));
        assert!(!editor.transferring(A));

        // The same result again (a stale ticket) changes nothing.
        env.apply(&mut editor, vec![MissionIntent::Discard { plan: Some(A) }]);
        env.finish(
            &mut editor,
            A,
            MissionOp::Download,
            ticket,
            Ok(Some(mission)),
        );
        assert!(draft(&editor, Some(A)).plan().is_empty());
    }

    #[tokio::test]
    async fn failures_are_shown_to_the_operator() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());

        confirmed_perform(&mut env, &mut editor, Some(A), MissionOp::Upload, None);
        let ticket = editor
            .operations
            .ticket_of(&(A, MissionOp::Upload))
            .unwrap();
        env.finish(
            &mut editor,
            A,
            MissionOp::Upload,
            ticket,
            Err(Error::Timeout),
        );

        let notice = env.notices.history().last().expect("a notice");
        assert_eq!(notice.level, Level::Error);
        assert!(notice.title.contains("upload"), "{}", notice.title);
    }

    #[tokio::test]
    async fn drags_address_items_by_id() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());
        env.apply(&mut editor, vec![add_waypoint(None), add_waypoint(None)]);

        let first = draft(&editor, None).plan().items()[0].id();
        env.apply(
            &mut editor,
            vec![
                MissionIntent::MoveItem {
                    plan: None,
                    item: first,
                    up: false,
                },
                MissionIntent::At {
                    plan: None,
                    item: first,
                    at: GeoPosition::new(10.0, 20.0),
                },
            ],
        );

        let plan = &draft(&editor, None).plan();
        assert_eq!(plan.index_of(first), Some(1));
        let moved = plan.get(first).unwrap().position().unwrap();
        assert!(
            (moved.lat - 10.0).abs() < 1e-6,
            "the moved item, not whatever is first"
        );

        // An id from before the plan was replaced resolves to nothing.
        env.apply(&mut editor, vec![MissionIntent::Discard { plan: None }]);
        env.apply(&mut editor, vec![add_waypoint(None)]);
        env.apply(
            &mut editor,
            vec![MissionIntent::At {
                plan: None,
                item: first,
                at: GeoPosition::new(-5.0, -5.0),
            }],
        );
        let plan = &draft(&editor, None).plan();
        assert!(plan.items()[0].item.position().unwrap().lat > -1.0);
    }

    #[tokio::test]
    async fn jumping_needs_a_synced_plan_and_a_confirmation() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        env.set_onboard(&mut editor, vec![waypoint(1.0), waypoint(2.0)], 1);
        let second = draft(&editor, Some(A)).plan().items()[1].id();

        let effects = env.perform(&mut editor, Some(A), MissionOp::SetCurrent, Some(second));
        assert!(matches!(&effects[..], [Effect::Confirm(_)]));

        // Once the plan is modified, item numbers no longer match the
        // vehicle.
        env.apply(&mut editor, vec![add_waypoint(Some(A))]);

        let effects = confirmed_perform(
            &mut env,
            &mut editor,
            Some(A),
            MissionOp::SetCurrent,
            Some(second),
        );
        assert!(effects.is_empty());
        assert!(!editor.operations.is_running(&(A, MissionOp::SetCurrent)));
        assert_eq!(env.notices.history().last().unwrap().level, Level::Warning);
    }

    #[tokio::test]
    async fn clearing_the_vehicle_always_asks() {
        let mut env = Env::new();
        let mut editor = MissionEditor::default();
        editor.sync(&env.fleet.snapshot());

        let effects = env.perform(&mut editor, Some(A), MissionOp::Clear, None);
        assert!(matches!(
            &effects[..],
            [Effect::Confirm(confirmation)] if confirmation.danger
        ));
    }
}
