//! The mission feature: planning flight plans and transferring them.
//!
//! The editor keeps one [`MissionDraft`] per vehicle, plus an offline draft
//! for planning with no vehicle selected; the draft decides when it follows
//! the vehicle and how it relates to it. Around each draft the editor keeps
//! what only the UI needs: the selected item, half-typed field text, and
//! whether map clicks place waypoints. Items are addressed by [`ItemId`], so
//! map drags and editor input stay attached to the item they started on
//! whatever happens to the list meanwhile.

mod map;
mod view;

use std::{collections::BTreeMap, path::PathBuf};

use groundhog_logic::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        geo::GeoPosition,
        mission::{Field, ItemId, MissionDraft, MissionPlan, OnboardMission, SyncStatus},
    },
    services::{Result, TransferProgress},
    storage::mission::{WplMission, parse_wpl, write_wpl},
};
use iced::Task;
use mavio::default_dialect::enums::MavCmd;

use super::{
    Context, Effect,
    confirm::Confirmation,
    file_dialog,
    notifications::Notice,
    operations::{Operations, Ticket},
};

/// Which draft: a vehicle's, or the offline plan (`None`).
pub type PlanKey = Option<SystemId>;

/// The vehicle operations the editor runs, one of each per vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    Download,
    Upload,
    Clear,
    Start,
    Pause,
    Resume,
    SetCurrent,
}

impl Operation {
    fn describe(self) -> &'static str {
        match self {
            Operation::Download => "Mission download",
            Operation::Upload => "Mission upload",
            Operation::Clear => "Clearing the mission",
            Operation::Start => "Starting the mission",
            Operation::Pause => "Pausing the mission",
            Operation::Resume => "Resuming the mission",
            Operation::SetCurrent => "Jumping to the item",
        }
    }

    /// Whether the operation transfers the mission (and blocks other
    /// transfers and edits that would race it).
    fn is_transfer(self) -> bool {
        matches!(
            self,
            Operation::Download | Operation::Upload | Operation::Clear
        )
    }
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
    operations: Operations<(SystemId, Operation)>,
}

impl Default for MissionEditor {
    fn default() -> Self {
        let mut drafts = BTreeMap::new();
        drafts.insert(None, Draft::new(None));
        Self {
            drafts,
            operations: Operations::default(),
        }
    }
}

/// A message about one draft.
#[derive(Debug, Clone)]
pub struct Message {
    pub plan: PlanKey,
    pub kind: MessageKind,
}

#[derive(Debug, Clone)]
pub enum MessageKind {
    // Vehicle operations. `confirmed` marks the second pass after the
    // operator agreed to a confirmation.
    Download {
        confirmed: bool,
    },
    Upload {
        confirmed: bool,
    },
    Clear {
        confirmed: bool,
    },
    Start {
        confirmed: bool,
    },
    Pause,
    Resume,
    SetCurrent {
        item: ItemId,
        confirmed: bool,
    },
    Cancel(Operation),
    Progress(Operation, Ticket, TransferProgress),
    Finished(Operation, Ticket, Result<Option<OnboardMission>>),

    // Editing.
    AddWaypoint,
    Remove(ItemId),
    MoveUp(ItemId),
    MoveDown(ItemId),
    Select(Option<ItemId>),
    ChangeCommand(ItemId, MavCmd),
    EditField(ItemId, Field, String),
    /// Empty the draft (the vehicle is untouched).
    Discard,
    TogglePlacing,

    // Map interaction.
    PlaceAt(GeoPosition),
    Drag(ItemId, GeoPosition),
    Drop(ItemId, GeoPosition),

    // Files.
    SaveDialog,
    SaveTo(PathBuf),
    Saved(PathBuf, std::result::Result<(), String>),
    LoadDialog,
    LoadFrom(PathBuf),
    Loaded(PathBuf, std::result::Result<WplMission, String>),
}

impl MissionEditor {
    /// The draft the UI shows for `selected`.
    fn displayed(&self, selected: Option<SystemId>) -> Option<&Draft> {
        self.drafts.get(&selected)
    }

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
    fn transferring(&self, system: SystemId) -> bool {
        self.operations
            .any(|(running, operation)| *running == system && operation.is_transfer())
    }

    pub fn update(&mut self, message: Message, cx: &mut Context<Message>) -> Task<Message> {
        let Message { plan, kind } = message;
        let wrap = move |kind| Message { plan, kind };

        // Vehicle operations need a vehicle; edits need a draft.
        match kind {
            MessageKind::Download { confirmed } => self.download(plan, confirmed, cx),
            MessageKind::Upload { confirmed } => self.upload(plan, confirmed, cx),
            MessageKind::Clear { confirmed } => self.clear(plan, confirmed, cx),
            MessageKind::Start { confirmed } => self.start(plan, confirmed, cx),
            MessageKind::Pause => self.command(plan, Operation::Pause, cx),
            MessageKind::Resume => self.command(plan, Operation::Resume, cx),
            MessageKind::SetCurrent { item, confirmed } => {
                self.set_current(plan, item, confirmed, cx)
            }
            MessageKind::Cancel(operation) => {
                if let Some(system) = plan
                    && self.operations.cancel(&(system, operation))
                {
                    cx.notices.push(
                        Notice::info(format!("{} cancelled", operation.describe())).vehicle(system),
                    );
                }
                Task::none()
            }
            MessageKind::Progress(operation, ticket, progress) => {
                if let Some(system) = plan {
                    self.operations
                        .report(&(system, operation), ticket, progress);
                }
                Task::none()
            }
            MessageKind::Finished(operation, ticket, result) => {
                self.finished(plan, operation, ticket, result, cx);
                Task::none()
            }
            MessageKind::SaveDialog => self.save_dialog(plan, cx).map(wrap),
            MessageKind::SaveTo(path) => self.save_to(plan, path, cx).map(wrap),
            MessageKind::Saved(path, result) => {
                match result {
                    Ok(()) => {
                        cx.notices
                            .push(Notice::success("Mission saved").detail(path.display()));
                        cx.request(Effect::RememberDirectory(path));
                    }
                    Err(error) => cx.notices.push(
                        Notice::error(format!("Could not save {}", path.display())).detail(error),
                    ),
                }
                Task::none()
            }
            MessageKind::LoadDialog => {
                let dialog = file_dialog(cx.directory);
                Task::future(
                    async move { dialog.pick_file().await.map(|file| file.path().to_owned()) },
                )
                .and_then(move |path| Task::done(wrap(MessageKind::LoadFrom(path))))
            }
            MessageKind::LoadFrom(path) => {
                let load_path = path.clone();
                iced_runtime::task::blocking(move |mut sender| {
                    let result = std::fs::read_to_string(&load_path)
                        .map_err(|error| error.to_string())
                        .and_then(|text| parse_wpl(&text).map_err(|error| error.to_string()));
                    let _ = sender.try_send(wrap(MessageKind::Loaded(load_path, result)));
                })
            }
            MessageKind::Loaded(path, result) => {
                match result {
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
                }
                Task::none()
            }
            edit => {
                self.edit(plan, edit, cx);
                Task::none()
            }
        }
    }

    /// Apply an edit to a draft. Edits to a vehicle's draft are refused while
    /// a transfer runs, so the transfer's result cannot clobber them.
    fn edit(&mut self, plan: PlanKey, kind: MessageKind, cx: &mut Context<Message>) {
        if let Some(system) = plan
            && self.transferring(system)
        {
            return;
        }

        let default_altitude = cx.settings.default_waypoint_altitude_m;
        let draft = self.draft_mut(plan);

        match kind {
            MessageKind::AddWaypoint => {
                let id = draft.mission.add_waypoint(cx.fleet, default_altitude);
                draft.select(Some(id));
            }
            MessageKind::PlaceAt(position) => {
                if draft.placing {
                    let id = draft.mission.place_waypoint(position, default_altitude);
                    draft.select(Some(id));
                }
            }
            MessageKind::Remove(item) => {
                draft.plan_mut().remove(item);
                if draft.selected == Some(item) {
                    draft.select(None);
                }
            }
            MessageKind::MoveUp(item) => {
                draft.plan_mut().move_up(item);
            }
            MessageKind::MoveDown(item) => {
                draft.plan_mut().move_down(item);
            }
            MessageKind::Select(item) => {
                let item = item.filter(|item| draft.plan().get(*item).is_some());
                draft.select(item);
            }
            MessageKind::ChangeCommand(item, command) => {
                if draft
                    .plan_mut()
                    .update(item, |current| *current = current.with_command(command))
                    && draft.selected == Some(item)
                {
                    draft.edits.clear();
                }
            }
            MessageKind::EditField(item, field, text) => {
                draft.plan_mut().update(item, |current| {
                    current.set_field_from_str(field, &text);
                });
                if draft.selected == Some(item) {
                    draft.edits.insert(field, text);
                }
            }
            MessageKind::Discard => {
                draft.plan_mut().clear();
                draft.select(None);
            }
            MessageKind::TogglePlacing => draft.placing = !draft.placing,
            MessageKind::Drag(item, position) | MessageKind::Drop(item, position) => {
                draft.plan_mut().update(item, |current| {
                    current.set_lat_lon(position.lat, position.lon)
                });
                if draft.selected == Some(item) {
                    // Show the dragged position rather than stale typing.
                    draft.edits.remove(&Field::Latitude);
                    draft.edits.remove(&Field::Longitude);
                }
            }
            _ => {}
        }
    }

    fn download(
        &mut self,
        plan: PlanKey,
        confirmed: bool,
        cx: &mut Context<Message>,
    ) -> Task<Message> {
        let Some(system) = plan else {
            return Task::none();
        };

        if !confirmed
            && self
                .drafts
                .get(&plan)
                .is_some_and(|draft| draft.mission.is_modified())
        {
            cx.confirm(
                Confirmation::new(
                    "Replace your changes?",
                    "Downloading replaces the plan you edited with the vehicle's mission.",
                    Message {
                        plan,
                        kind: MessageKind::Download { confirmed: true },
                    },
                )
                .accept_label("Download"),
            );
            return Task::none();
        }

        let client = cx.services.vehicle(system);
        self.transfer(system, Operation::Download, move |progress| async move {
            client.download_mission(&*progress).await.map(Some)
        })
    }

    fn upload(
        &mut self,
        plan: PlanKey,
        confirmed: bool,
        cx: &mut Context<Message>,
    ) -> Task<Message> {
        let Some(system) = plan else {
            return Task::none();
        };
        let Some(draft) = self.drafts.get(&plan) else {
            return Task::none();
        };

        let armed = cx
            .fleet
            .vehicle(system)
            .and_then(|vehicle| vehicle.armed())
            .unwrap_or(false);

        if !confirmed && armed {
            cx.confirm(
                Confirmation::new(
                    format!("Upload to armed {}?", cx.fleet.vehicle_name(system)),
                    "The vehicle is armed. If it is flying the mission, the new plan takes effect immediately.",
                    Message {
                        plan,
                        kind: MessageKind::Upload { confirmed: true },
                    },
                )
                .accept_label("Upload")
                .danger(),
            );
            return Task::none();
        }

        let items = draft.plan().mission_items();
        let client = cx.services.vehicle(system);
        self.transfer(system, Operation::Upload, move |progress| async move {
            client.upload_mission(items, &*progress).await.map(Some)
        })
    }

    fn clear(
        &mut self,
        plan: PlanKey,
        confirmed: bool,
        cx: &mut Context<Message>,
    ) -> Task<Message> {
        let Some(system) = plan else {
            return Task::none();
        };

        if !confirmed {
            cx.confirm(
                Confirmation::new(
                    format!("Clear the mission on {}?", cx.fleet.vehicle_name(system)),
                    "The vehicle's stored mission is deleted. Your plan here is kept.",
                    Message {
                        plan,
                        kind: MessageKind::Clear { confirmed: true },
                    },
                )
                .accept_label("Clear")
                .danger(),
            );
            return Task::none();
        }

        let client = cx.services.vehicle(system);
        self.transfer(system, Operation::Clear, move |_| async move {
            client.clear_mission().await.map(|()| None)
        })
    }

    fn start(
        &mut self,
        plan: PlanKey,
        confirmed: bool,
        cx: &mut Context<Message>,
    ) -> Task<Message> {
        let Some(system) = plan else {
            return Task::none();
        };

        if !confirmed {
            cx.confirm(
                Confirmation::new(
                    format!("Start the mission on {}?", cx.fleet.vehicle_name(system)),
                    "The vehicle starts flying its stored mission from the beginning.",
                    Message {
                        plan,
                        kind: MessageKind::Start { confirmed: true },
                    },
                )
                .accept_label("Start"),
            );
            return Task::none();
        }

        self.command(plan, Operation::Start, cx)
    }

    fn set_current(
        &mut self,
        plan: PlanKey,
        item: ItemId,
        confirmed: bool,
        cx: &mut Context<Message>,
    ) -> Task<Message> {
        let Some(system) = plan else {
            return Task::none();
        };

        // Indices only mean the same thing on the vehicle when the plan is
        // what the vehicle holds.
        if self.status(plan, cx.fleet) != SyncStatus::Synced {
            cx.notices.push(
                Notice::warning("Upload the plan first")
                    .detail("The plan differs from the vehicle's mission, so its item numbers do not match.")
                    .vehicle(system),
            );
            return Task::none();
        }

        let Some(index) = self
            .drafts
            .get(&plan)
            .and_then(|draft| draft.plan().index_of(item))
        else {
            return Task::none();
        };

        if !confirmed {
            cx.confirm(
                Confirmation::new(
                    format!("Continue at item {}?", index + 1),
                    "The vehicle skips to this item of its mission.",
                    Message {
                        plan,
                        kind: MessageKind::SetCurrent {
                            item,
                            confirmed: true,
                        },
                    },
                )
                .accept_label("Jump"),
            );
            return Task::none();
        }

        let client = cx.services.vehicle(system);
        self.operations.start(
            (system, Operation::SetCurrent),
            async move { client.set_current_mission_item(index).await.map(|()| None) },
            move |ticket, result| Message {
                plan,
                kind: MessageKind::Finished(Operation::SetCurrent, ticket, result),
            },
        )
    }

    /// Run a mission command (start, pause, resume).
    fn command(
        &mut self,
        plan: PlanKey,
        operation: Operation,
        cx: &mut Context<Message>,
    ) -> Task<Message> {
        let Some(system) = plan else {
            return Task::none();
        };

        let client = cx.services.vehicle(system);
        self.operations.start(
            (system, operation),
            async move {
                match operation {
                    Operation::Start => client.start_mission().await,
                    Operation::Pause => client.pause_mission(true).await,
                    _ => client.pause_mission(false).await,
                }
                .map(|()| None)
            },
            move |ticket, result| Message {
                plan: Some(system),
                kind: MessageKind::Finished(operation, ticket, result),
            },
        )
    }

    /// Start a transfer with progress, unless one is already running.
    fn transfer<Fut>(
        &mut self,
        system: SystemId,
        operation: Operation,
        run: impl FnOnce(super::operations::Progress) -> Fut,
    ) -> Task<Message>
    where
        Fut: std::future::Future<Output = Result<Option<OnboardMission>>> + Send + 'static,
    {
        if self.transferring(system) {
            return Task::none();
        }

        let plan = Some(system);
        self.operations.start_with_progress(
            (system, operation),
            run,
            move |ticket, progress| Message {
                plan,
                kind: MessageKind::Progress(operation, ticket, progress),
            },
            move |ticket, result| Message {
                plan,
                kind: MessageKind::Finished(operation, ticket, result),
            },
        )
    }

    fn finished(
        &mut self,
        plan: PlanKey,
        operation: Operation,
        ticket: Ticket,
        result: Result<Option<OnboardMission>>,
        cx: &mut Context<Message>,
    ) {
        let Some(system) = plan else { return };
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

        let draft = self.draft_mut(plan);
        match operation {
            Operation::Download | Operation::Upload => {
                let mission = mission.unwrap_or_else(OnboardMission::empty);
                let replaced = draft.mission.transferred(&mission);
                draft.replaced(replaced);

                let count = mission.items.len();
                let title = match operation {
                    Operation::Download => format!("Downloaded {count} mission items"),
                    _ => format!("Uploaded {count} mission items"),
                };
                cx.notices.push(Notice::success(title).vehicle(system));
            }
            Operation::Clear => {
                draft.mission.cleared();
                cx.notices
                    .push(Notice::success("Mission cleared on the vehicle").vehicle(system));
            }
            Operation::Start | Operation::Pause | Operation::Resume | Operation::SetCurrent => {}
        }
    }

    fn save_dialog(&mut self, plan: PlanKey, cx: &mut Context<Message>) -> Task<MessageKind> {
        let dialog = file_dialog(cx.directory).set_file_name("mission.waypoints");
        let _ = plan;
        Task::future(async move { dialog.save_file().await.map(|file| file.path().to_owned()) })
            .and_then(|path| Task::done(MessageKind::SaveTo(path)))
    }

    fn save_to(
        &mut self,
        plan: PlanKey,
        path: PathBuf,
        cx: &mut Context<Message>,
    ) -> Task<MessageKind> {
        let Some(draft) = self.drafts.get(&plan) else {
            return Task::none();
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

        iced_runtime::task::blocking(move |mut sender| {
            let result = std::fs::write(&path, write_wpl(&items, home.as_ref()))
                .map_err(|error| error.to_string());
            let _ = sender.try_send(MessageKind::Saved(path, result));
        })
    }
}

#[cfg(test)]
mod tests;
