//! The mission feature: editing a vehicle's flight plan and driving the
//! MAVLink mission transactions for the addressed vehicle.
//!
//! Mission data itself lives on the [`Fleet`] (the single owner of vehicle
//! state), which the root passes in by mutable reference; the fleet services
//! are passed in for transactions. Every message is a [`Message`] envelope
//! carrying the [`SystemId`] it concerns, so an operation is not redirected
//! when the primary — the vehicle the panel *renders* — changes underneath it.
//! Handlers are silent no-ops for a vehicle the fleet does not know; view
//! state is only ever created for a known vehicle.
//!
//! The panel owns only view-local state: which item's editor is open, the
//! in-progress text buffers, and the mission-on-map interaction (placement mode
//! and the dragged waypoint). That state is keyed per vehicle, so switching the
//! primary carries none of it across to another mission.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use iced::{
    Element, Length, Task, Theme, Widget,
    alignment::Vertical,
    widget::{
        Button, Column, Space, TextInput, column, container, pick_list, row, scrollable, text,
    },
};
use mavio::default_dialect::{enums::MavFrame, messages::CommandAck};
use rfd::AsyncFileDialog;

use crate::{
    domain::fleet::Fleet,
    domain::geo::GeoPosition,
    domain::mission::{
        Mission, MissionCommand, MissionCommandKind, MissionField, MissionItem, Position,
        file::{parse_wpl, write_wpl},
    },
    domain::vehicle::Vehicle,
    foundation::SystemId,
    services::{FleetServices, Result},
    style,
    widget::virtualized::virtual_list,
};

/// Height of a single mission-list row.
const ROW_HEIGHT: f32 = 28.0;

/// The mission feature's view-local state for one vehicle.
///
/// Keyed by vehicle so the open editor, partial field input, drag, transfer
/// state, and placement mode travel with the vehicle rather than leaking across
/// a primary switch.
#[derive(Debug, Default)]
struct MissionUi {
    /// Item whose editor is open / highlighted.
    selected: Option<usize>,
    /// Item currently being dragged on the map, if any.
    dragging: Option<usize>,
    /// In-progress text for the selected item's fields (survives partial input).
    edit: BTreeMap<MissionField, String>,
    /// A transfer/command is in flight; the toolbar is disabled.
    busy: bool,
    /// Whether the next map click should place a new waypoint.
    placing: bool,
}

#[derive(Debug, Default)]
pub struct MissionPanel {
    ui: BTreeMap<SystemId, MissionUi>,
}

/// Everything the panel needs from the rest of the application.
pub struct Context<'a> {
    pub services: &'a FleetServices,
    pub fleet: &'a mut Fleet,
    pub file_picker_path: Option<&'a Path>,
}

/// A request the root must satisfy, since it owns the configuration file.
#[derive(Debug)]
pub enum Event {
    /// Remember the directory of the last used file dialog.
    SetFilePickerPath(PathBuf),
}

#[derive(Debug, Clone, Copy)]
pub enum MissionAction {
    Start,
    Pause,
    Resume,
}

/// A message the mission feature handles: an intent or a result, always
/// addressed to one vehicle.
///
/// `system` scopes the whole message: for operating intents (`Download`,
/// `AddItem`, `EditField`, ...) it is the vehicle the operation drives; for
/// result- and file-operation kinds it is the vehicle the transfer or file
/// concerns. The panel renders the primary vehicle, but messages are never
/// *implicitly* addressed to it.
#[derive(Debug, Clone)]
pub struct Message {
    /// The vehicle the message concerns.
    pub system: SystemId,
    pub kind: MessageKind,
}

#[derive(Debug, Clone)]
pub enum MessageKind {
    Download,
    Upload,
    Clear,
    Start,
    Pause,
    Resume,
    AddItem,
    RemoveItem(usize),
    MoveUp(usize),
    MoveDown(usize),
    Select(usize),
    /// A mission waypoint was pressed on the map: select it and begin a drag.
    BeginDrag(usize),
    /// A map drag ended.
    EndDrag,
    SetCurrent(usize),
    ChangeCommand(usize, MissionCommandKind),
    EditField(usize, MissionField, String),

    /// Toggle "place waypoint" mode for the next map click.
    TogglePlacing,
    /// A map drag moved waypoint `usize` to a new geographic position.
    MoveWaypointTo(usize, GeoPosition),
    /// A map click in "place waypoint" mode adds a waypoint at this position.
    AddWaypointAt(GeoPosition),

    // Results, for the envelope's vehicle.
    Downloaded(Result<Mission>),
    Uploaded(Result<u32>),
    Cleared(Result<()>),
    CurrentSet(Result<()>),
    CommandFinished(MissionAction, Result<CommandAck>),

    /// Open a file-picker dialog to choose a save location.
    SaveDialog,
    /// Write the given vehicle's mission to the chosen path.
    SaveToFile(PathBuf),
    /// The result of writing the mission to disk.
    SaveFinished(PathBuf, Result<(), String>),
    /// Open a file-picker dialog to choose a file to load.
    LoadDialog,
    /// Parse the chosen file and replace the given vehicle's mission.
    LoadFromFile(PathBuf),
    /// The result of reading a mission file from disk.
    LoadFinished(PathBuf, Result<Vec<MissionItem>, String>),
}

impl MissionPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// The view-local state for `id`, if that vehicle has any yet.
    fn ui(&self, id: SystemId) -> Option<&MissionUi> {
        self.ui.get(&id)
    }

    /// `id`'s view-local state, created on demand. Mission intents are only
    /// meaningful for an existing vehicle: handlers resolve
    /// `ctx.fleet.vehicle(id)` (or `vehicle_mut`) first and only then call
    /// this, so state is never created for a vehicle the fleet does not know.
    fn ui_mut(&mut self, id: SystemId) -> &mut MissionUi {
        self.ui.entry(id).or_default()
    }

    /// Whether the displayed (primary) vehicle's panel is in "place waypoint"
    /// mode.
    pub fn is_placing(&self, primary: Option<SystemId>) -> bool {
        primary
            .and_then(|id| self.ui(id))
            .is_some_and(|ui| ui.placing)
    }

    /// The mission item the displayed (primary) vehicle is currently dragging
    /// on the map, if any.
    pub fn dragging(&self, primary: Option<SystemId>) -> Option<usize> {
        primary
            .and_then(|id| self.ui(id))
            .and_then(|ui| ui.dragging)
    }

    pub fn update(
        &mut self,
        message: Message,
        ctx: &mut Context,
    ) -> (Task<Message>, Option<Event>) {
        self.apply(message, ctx).unwrap_or((Task::none(), None))
    }

    fn apply(
        &mut self,
        message: Message,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let Message { system: id, kind } = message;

        match kind {
            MessageKind::Download => return self.download(id, ctx),
            MessageKind::Downloaded(result) => self.downloaded(id, result, ctx),
            MessageKind::Upload => return self.upload(id, ctx),
            MessageKind::Uploaded(result) => self.uploaded(id, result, ctx),
            MessageKind::Clear => return self.clear(id, ctx),
            MessageKind::Cleared(result) => self.cleared(id, result, ctx),
            MessageKind::Start => {
                return Some((self.run_command(id, MissionAction::Start, ctx), None));
            }
            MessageKind::Pause => {
                return Some((self.run_command(id, MissionAction::Pause, ctx), None));
            }
            MessageKind::Resume => {
                return Some((self.run_command(id, MissionAction::Resume, ctx), None));
            }
            MessageKind::CommandFinished(action, result) => {
                self.command_finished(id, action, result)
            }
            MessageKind::SetCurrent(index) => return self.set_current(id, index, ctx),
            MessageKind::CurrentSet(result) => self.current_set(id, result),
            MessageKind::AddItem => self.add_item(id, ctx)?,
            MessageKind::RemoveItem(index) => self.remove_item(id, index, ctx)?,
            MessageKind::MoveUp(index) => self.move_item(id, index, true, ctx),
            MessageKind::MoveDown(index) => self.move_item(id, index, false, ctx),
            MessageKind::Select(index) => self.select(id, index, ctx)?,
            MessageKind::BeginDrag(index) => self.begin_drag(id, index, ctx)?,
            MessageKind::EndDrag => self.end_drag(id)?,
            MessageKind::ChangeCommand(index, command) => {
                self.change_command(id, index, command, ctx)?
            }
            MessageKind::EditField(index, field, text) => {
                self.edit_field(id, index, field, text, ctx)?
            }
            MessageKind::TogglePlacing => self.toggle_placing(id, ctx)?,
            MessageKind::MoveWaypointTo(index, position) => {
                self.move_waypoint_to(id, index, position, ctx)?
            }
            MessageKind::AddWaypointAt(position) => self.add_waypoint_at(id, position, ctx)?,
            MessageKind::SaveDialog => return self.save_dialog(id, ctx),
            MessageKind::SaveToFile(path) => return self.save_to_file(id, path, ctx),
            MessageKind::SaveFinished(path, result) => {
                return Some(self.save_finished(id, path, result));
            }
            MessageKind::LoadDialog => return self.load_dialog(id, ctx),
            MessageKind::LoadFromFile(path) => return Some(self.load_from_file(id, path)),
            MessageKind::LoadFinished(path, result) => {
                return self.load_finished(id, path, result, ctx);
            }
        }

        None
    }

    /// Start a mission download from the addressed vehicle.
    fn download(
        &mut self,
        id: SystemId,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let target = ctx.fleet.vehicle(id)?.target();

        let client = ctx.services.vehicle(target);
        self.ui_mut(id).busy = true;
        Some((
            Task::perform(
                async move { client.download_mission().await },
                move |result| Message {
                    system: id,
                    kind: MessageKind::Downloaded(result),
                },
            ),
            None,
        ))
    }

    /// Apply a download result to the vehicle it was started against.
    fn downloaded(&mut self, id: SystemId, result: Result<Mission>, ctx: &mut Context) {
        // Results update the state a transfer already established; they
        // never create it, so an unsolicited result is a no-op.
        if let Some(ui) = self.ui.get_mut(&id) {
            ui.busy = false;
        }

        match result {
            Ok(mission) => {
                if ctx.fleet.apply_mission_download(id, mission)
                    && let Some(ui) = self.ui.get_mut(&id)
                {
                    ui.selected = None;
                    ui.edit.clear();
                }
            }
            Err(error) => log::warn!("Mission download for {id:?} failed: {error}"),
        }
    }

    /// Upload the addressed vehicle's mission.
    fn upload(
        &mut self,
        id: SystemId,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let vehicle = ctx.fleet.vehicle(id)?;

        let mission = vehicle.mission.clone();
        let target = vehicle.target();
        let client = ctx.services.vehicle(target);
        self.ui_mut(id).busy = true;
        Some((
            Task::perform(
                async move { client.upload_mission(&mission).await },
                move |result| Message {
                    system: id,
                    kind: MessageKind::Uploaded(result),
                },
            ),
            None,
        ))
    }

    /// Apply an upload result to the vehicle it was started against.
    fn uploaded(&mut self, id: SystemId, result: Result<u32>, ctx: &mut Context) {
        if let Some(ui) = self.ui.get_mut(&id) {
            ui.busy = false;
        }

        match result {
            Ok(vehicle_id) => {
                // The fold landing is all this result implies for stored
                // state; an unknown vehicle silently ignores it.
                ctx.fleet.apply_mission_upload(id, vehicle_id);
            }
            Err(error) => log::warn!("Mission upload to {id:?} failed: {error}"),
        }
    }

    /// Clear the addressed vehicle's mission.
    fn clear(&mut self, id: SystemId, ctx: &mut Context) -> Option<(Task<Message>, Option<Event>)> {
        let target = ctx.fleet.vehicle(id)?.target();

        let client = ctx.services.vehicle(target);
        self.ui_mut(id).busy = true;
        Some((
            Task::perform(async move { client.clear_mission().await }, move |result| {
                Message {
                    system: id,
                    kind: MessageKind::Cleared(result),
                }
            }),
            None,
        ))
    }

    /// Apply a clear result to the vehicle it was started against.
    fn cleared(&mut self, id: SystemId, result: Result<()>, ctx: &mut Context) {
        if let Some(ui) = self.ui.get_mut(&id) {
            ui.busy = false;
        }

        match result {
            Ok(()) => {
                if ctx.fleet.apply_mission_clear(id)
                    && let Some(ui) = self.ui.get_mut(&id)
                {
                    ui.selected = None;
                    ui.edit.clear();
                }
            }
            Err(error) => log::warn!("Mission clear on {id:?} failed: {error}"),
        }
    }

    /// Clear the busy flag once a start/pause/resume command settles.
    fn command_finished(
        &mut self,
        id: SystemId,
        action: MissionAction,
        result: Result<CommandAck>,
    ) {
        if let Some(ui) = self.ui.get_mut(&id) {
            ui.busy = false;
        }
        if let Err(error) = result {
            log::warn!("Mission {action:?} on {id:?} failed: {error}");
        }
    }

    /// Ask the addressed vehicle to jump to mission item `index`.
    fn set_current(
        &mut self,
        id: SystemId,
        index: usize,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let target = ctx.fleet.vehicle(id)?.target();
        let client = ctx.services.vehicle(target);
        Some((
            Task::perform(
                async move { client.set_current_mission_item(index as u16).await },
                move |result| Message {
                    system: id,
                    kind: MessageKind::CurrentSet(result),
                },
            ),
            None,
        ))
    }

    /// Log a failed set-current result.
    fn current_set(&mut self, id: SystemId, result: Result<()>) {
        if let Err(error) = result {
            log::warn!("Setting the current mission item on {id:?} failed: {error}");
        }
    }

    /// Append a waypoint after the vehicle's current position or last item.
    fn add_item(&mut self, id: SystemId, ctx: &mut Context) -> Option<()> {
        let vehicle = ctx.fleet.vehicle_mut(id)?;

        let pos = vehicle
            .position
            .current
            .as_ref()
            .map(|position| Position {
                lat: position.pos.lat,
                lon: position.pos.lon,
                alt_m: position.alt_agl,
            })
            .or_else(|| {
                vehicle
                    .mission
                    .items()
                    .last()
                    .and_then(|item| item.command.position())
            })
            .unwrap_or(Position {
                lat: 0.0,
                lon: 0.0,
                alt_m: 50.0,
            });

        let command = MissionCommand::default_for(MissionCommandKind::Waypoint, pos);
        vehicle.mission.push(MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command,
        });

        let ui = self.ui_mut(id);
        ui.selected = Some(vehicle.mission.len() - 1);
        ui.edit.clear();
        Some(())
    }

    /// Remove an item and reindex the open editor.
    fn remove_item(&mut self, id: SystemId, index: usize, ctx: &mut Context) -> Option<()> {
        let vehicle = ctx.fleet.vehicle_mut(id)?;

        vehicle.mission.remove(index)?;

        let ui = self.ui_mut(id);
        match ui.selected {
            Some(selected) if selected == index => {
                ui.selected = None;
                ui.edit.clear();
            }
            Some(selected) if selected > index => ui.selected = Some(selected - 1),
            _ => {}
        }
        Some(())
    }

    /// Open the editor on mission item `index`.
    fn select(&mut self, id: SystemId, index: usize, ctx: &mut Context) -> Option<()> {
        // Selection indexes the addressed vehicle's mission; meaningless
        // without one, and it must not create view state for an unknown one.
        ctx.fleet.vehicle(id)?;

        let ui = self.ui_mut(id);
        if ui.selected != Some(index) {
            ui.selected = Some(index);
            ui.edit.clear();
        }
        Some(())
    }

    /// Select mission item `index` and begin dragging it on the map.
    fn begin_drag(&mut self, id: SystemId, index: usize, ctx: &mut Context) -> Option<()> {
        // As with `select`, only known vehicles get view state.
        ctx.fleet.vehicle(id)?;

        let ui = self.ui_mut(id);
        if ui.selected != Some(index) {
            ui.selected = Some(index);
            ui.edit.clear();
        }
        ui.dragging = Some(index);
        Some(())
    }

    /// End the addressed vehicle's map drag. Clearing only touches state that
    /// already exists: a vehicle without view state has no drag to end, so no
    /// entry is created here.
    fn end_drag(&mut self, id: SystemId) -> Option<()> {
        self.ui.get_mut(&id)?.dragging = None;
        Some(())
    }

    /// Replace mission item `index`'s command with `kind`.
    fn change_command(
        &mut self,
        id: SystemId,
        index: usize,
        kind: MissionCommandKind,
        ctx: &mut Context,
    ) -> Option<()> {
        let vehicle = ctx.fleet.vehicle_mut(id)?;

        let pos = vehicle
            .mission
            .get(index)
            .and_then(|item| item.command.position())
            .unwrap_or(Position {
                lat: 0.0,
                lon: 0.0,
                alt_m: 50.0,
            });

        if vehicle.mission.update(index, |item| {
            item.command = MissionCommand::default_for(kind, pos)
        }) {
            self.ui_mut(id).edit.clear();
        }
        Some(())
    }

    /// Buffer an edited field and fold it into the mission when parseable.
    fn edit_field(
        &mut self,
        id: SystemId,
        index: usize,
        field: MissionField,
        text: String,
        ctx: &mut Context,
    ) -> Option<()> {
        let vehicle = ctx.fleet.vehicle_mut(id)?;

        self.ui_mut(id).edit.insert(field, text.clone());

        if let Some(value) = parse_field_value(&text) {
            vehicle
                .mission
                .update(index, |item| item.command.set_field(field, value));
        }
        Some(())
    }

    /// Toggle "place waypoint" mode for the addressed vehicle.
    fn toggle_placing(&mut self, id: SystemId, ctx: &mut Context) -> Option<()> {
        let _vehicle = ctx.fleet.vehicle(id)?;

        let ui = self.ui_mut(id);
        ui.placing = !ui.placing;
        Some(())
    }

    /// Move a dragged waypoint to a new geographic position.
    fn move_waypoint_to(
        &mut self,
        id: SystemId,
        index: usize,
        position: GeoPosition,
        ctx: &mut Context,
    ) -> Option<()> {
        let vehicle = ctx.fleet.vehicle_mut(id)?;

        vehicle.mission.update(index, |item| {
            item.command.set_field(MissionField::Latitude, position.lat);
            item.command
                .set_field(MissionField::Longitude, position.lon);
        });
        Some(())
    }

    /// Append a waypoint at a map-click position.
    fn add_waypoint_at(
        &mut self,
        id: SystemId,
        position: GeoPosition,
        ctx: &mut Context,
    ) -> Option<()> {
        let vehicle = ctx.fleet.vehicle_mut(id)?;

        let alt_m = vehicle
            .position
            .current
            .as_ref()
            .map(|position| position.alt_agl)
            .unwrap_or(50.0);

        let command = MissionCommand::default_for(
            MissionCommandKind::Waypoint,
            Position {
                lat: position.lat,
                lon: position.lon,
                alt_m,
            },
        );

        vehicle.mission.push(MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command,
        });

        let ui = self.ui_mut(id);
        ui.selected = Some(vehicle.mission.len() - 1);
        ui.edit.clear();
        Some(())
    }

    /// Open a file dialog to pick a mission save path.
    fn save_dialog(
        &mut self,
        id: SystemId,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        // The dialog is opened for a vehicle; gate on its existence.
        ctx.fleet.vehicle(id)?;
        let file_dialog = file_dialog(ctx.file_picker_path).set_file_name("mission.waypoints");

        Some((
            Task::future(async move {
                file_dialog
                    .save_file()
                    .await
                    .map(|file| file.path().to_owned())
            })
            .and_then(move |path| {
                Task::done(Message {
                    system: id,
                    kind: MessageKind::SaveToFile(path),
                })
            }),
            None,
        ))
    }

    /// Write a vehicle's mission to disk off the UI thread.
    fn save_to_file(
        &mut self,
        id: SystemId,
        path: PathBuf,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let vehicle = ctx.fleet.vehicle(id)?;
        let items = vehicle.mission.items().to_vec();
        log::info!("Saving mission to file: {path:?}");

        let save_path = path.clone();
        Some((
            iced_runtime::task::blocking(move |mut sender| {
                let result = std::fs::write(&save_path, write_wpl(&items))
                    .map_err(|error| error.to_string());
                let _ = sender.try_send(Message {
                    system: id,
                    kind: MessageKind::SaveFinished(save_path, result),
                });
            }),
            None,
        ))
    }

    /// Log a save failure and remember the chosen directory.
    fn save_finished(
        &mut self,
        id: SystemId,
        path: PathBuf,
        result: Result<(), String>,
    ) -> (Task<Message>, Option<Event>) {
        if let Err(error) = result {
            log::error!("Error saving mission for {id:?} to file: {error}");
        }

        (Task::none(), Some(Event::SetFilePickerPath(path)))
    }

    /// Open a file dialog to pick a mission file to load.
    fn load_dialog(
        &mut self,
        id: SystemId,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        // The dialog is opened for a vehicle; gate on its existence.
        ctx.fleet.vehicle(id)?;
        let file_dialog = file_dialog(ctx.file_picker_path);

        Some((
            Task::future(async move {
                file_dialog
                    .pick_file()
                    .await
                    .map(|file| file.path().to_owned())
            })
            .and_then(move |path| {
                Task::done(Message {
                    system: id,
                    kind: MessageKind::LoadFromFile(path),
                })
            }),
            None,
        ))
    }

    /// Read and parse a mission file off the UI thread.
    fn load_from_file(&mut self, id: SystemId, path: PathBuf) -> (Task<Message>, Option<Event>) {
        log::info!("Loading mission from file: {path:?}");

        let load_path = path.clone();
        (
            iced_runtime::task::blocking(move |mut sender| {
                let result = std::fs::read_to_string(&load_path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| parse_wpl(&text).map_err(|error| error.to_string()));
                let _ = sender.try_send(Message {
                    system: id,
                    kind: MessageKind::LoadFinished(load_path, result),
                });
            }),
            None,
        )
    }

    /// Apply a loaded mission to the vehicle the dialog was opened for.
    fn load_finished(
        &mut self,
        id: SystemId,
        path: PathBuf,
        result: Result<Vec<MissionItem>, String>,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        match result {
            Ok(items) => {
                // As with the network results, a file result only updates
                // state it already owns, so an unsolicited result is a no-op.
                if let Some(vehicle) = ctx.fleet.vehicle_mut(id) {
                    vehicle.mission.set_items(items);
                    vehicle.mission.mark_dirty();
                }
                if let Some(ui) = self.ui.get_mut(&id) {
                    ui.selected = None;
                    ui.edit.clear();
                }

                Some((Task::none(), Some(Event::SetFilePickerPath(path))))
            }
            Err(error) => {
                log::error!("Error loading mission from file: {error}");
                None
            }
        }
    }

    fn run_command(
        &mut self,
        id: SystemId,
        action: MissionAction,
        ctx: &mut Context,
    ) -> Task<Message> {
        let Some(target) = ctx.fleet.vehicle(id).map(Vehicle::target) else {
            return Task::none();
        };

        self.ui_mut(id).busy = true;
        let client = ctx.services.vehicle(target);
        Task::perform(
            async move {
                match action {
                    MissionAction::Start => client.start_mission().await,
                    MissionAction::Pause => client.pause_mission(true).await,
                    MissionAction::Resume => client.pause_mission(false).await,
                }
            },
            move |result| Message {
                system: id,
                kind: MessageKind::CommandFinished(action, result),
            },
        )
    }

    fn move_item(&mut self, id: SystemId, index: usize, up: bool, ctx: &mut Context) {
        let Some(vehicle) = ctx.fleet.vehicle_mut(id) else {
            return;
        };

        let moved = if up {
            vehicle.mission.move_up(index)
        } else {
            vehicle.mission.move_down(index)
        };

        if moved {
            let ui = self.ui_mut(id);
            // The selection follows the moved item; the previous open
            // editor's in-progress values must not fold into it.
            ui.selected = Some(if up { index - 1 } else { index + 1 });
            ui.edit.clear();
        }
    }

    /// Render the mission panel. A pure function of panel state and the primary
    /// vehicle's mission. Every message constructed here is addressed to the
    /// displayed vehicle.
    pub fn view<'a>(
        &'a self,
        vehicles: &'a BTreeMap<SystemId, Vehicle>,
        primary: Option<SystemId>,
        theme: &Theme,
    ) -> Element<'a, Message> {
        let Some(id) = primary else {
            return Space::new().boxed();
        };

        let Some(vehicle) = vehicles.get(&id) else {
            return Space::new().boxed();
        };

        let ui = self.ui(id);
        let mission = &vehicle.mission;

        let enabled = !ui.is_some_and(|ui| ui.busy);
        let toolbar = row![
            Button::new("Download").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::Download,
            })),
            Button::new("Upload").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::Upload,
            })),
            Button::new("Clear").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::Clear,
            })),
            Button::new("Start").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::Start,
            })),
            Button::new("Pause").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::Pause,
            })),
            Button::new("Resume").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::Resume,
            })),
            Button::new("Add waypoint").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::AddItem,
            })),
            Button::new(if ui.is_some_and(|ui| ui.placing) {
                "Placing…"
            } else {
                "Place waypoint"
            })
            .on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::TogglePlacing,
            })),
            Button::new("Save to file").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::SaveDialog,
            })),
            Button::new("Load from file").on_press_maybe(enabled.then_some(Message {
                system: id,
                kind: MessageKind::LoadDialog,
            })),
        ]
        .spacing(6.0);

        let mut status = if mission.dirty {
            "modified".to_string()
        } else {
            "synced".to_string()
        };
        if let (Some(current), Some(total)) = (mission.progress.current_seq, mission.progress.total)
        {
            status.push_str(&format!(" · item {current}/{total}"));
        }

        let current_seq = mission.progress.current_seq;
        let current_color = theme.palette().success.base.color;

        let list = virtual_list(mission.items().iter().enumerate(), move |entry| {
            let &(index, item) = entry;
            let summary = command_summary(&item.command);
            let is_current = current_seq == Some(index as u16);

            let content = row![
                text(format!("{}", index + 1)).width(Length::Fixed(32.0)),
                text(item.command.name()).width(Length::Fixed(140.0)),
                text(summary).width(Length::Fill),
                text(if is_current { "current" } else { "" })
                    .color(current_color)
                    .width(Length::Fixed(64.0)),
                Button::new("Edit").on_press(Message {
                    system: id,
                    kind: MessageKind::Select(index),
                }),
            ]
            .spacing(8.0)
            .align_y(Vertical::Center);

            container(content)
                .style(if index % 2 == 0 {
                    style::list_default
                } else {
                    style::list_brighter
                })
                .padding(2)
                .width(Length::Fill)
                .boxed()
        })
        .item_height(|_| ROW_HEIGHT)
        .spacing(0.0);

        let list = scrollable(list).height(Length::Fill).width(Length::Fill);

        let editor: Element<'_, Message> = match ui {
            Some(ui) => match ui.selected {
                Some(index) if index < mission.len() => {
                    self.view_editor(id, ui, index, mission.get(index).expect("index checked"))
                }
                _ => Space::new().boxed(),
            },
            None => Space::new().boxed(),
        };

        container(column![toolbar, text(status), list, editor].spacing(6.0))
            .padding(8.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .boxed()
    }

    fn view_editor<'a>(
        &'a self,
        id: SystemId,
        ui: &'a MissionUi,
        index: usize,
        item: &'a MissionItem,
    ) -> Element<'a, Message> {
        let kind = pick_list(
            item.command.kind(),
            MissionCommandKind::ALL,
            |kind: &MissionCommandKind| kind.name().to_string(),
        )
        .on_select(move |command| Message {
            system: id,
            kind: MessageKind::ChangeCommand(index, command),
        });

        let mut fields = Column::<Element<'a, Message>>::new().spacing(4.0);

        for spec in item.command.fields() {
            let value = ui
                .edit
                .get(&spec.field)
                .cloned()
                .unwrap_or_else(|| format_field_value(spec.value));

            let input = TextInput::new("", value)
                .on_input(move |text| Message {
                    system: id,
                    kind: MessageKind::EditField(index, spec.field, text),
                })
                .width(Length::Fill);

            fields = fields.push(
                row![
                    text(spec.label).width(Length::Fixed(160.0)),
                    input,
                    text(spec.units).width(Length::Fixed(48.0)),
                ]
                .spacing(6.0)
                .align_y(Vertical::Center)
                .boxed(),
            );
        }

        let actions = row![
            Button::new("Remove").on_press(Message {
                system: id,
                kind: MessageKind::RemoveItem(index),
            }),
            Button::new("Move up").on_press(Message {
                system: id,
                kind: MessageKind::MoveUp(index),
            }),
            Button::new("Move down").on_press(Message {
                system: id,
                kind: MessageKind::MoveDown(index),
            }),
            Button::new("Set current").on_press(Message {
                system: id,
                kind: MessageKind::SetCurrent(index),
            }),
        ]
        .spacing(6.0);

        column![
            row![text("Command:"), kind]
                .spacing(6.0)
                .align_y(Vertical::Center),
            fields,
            actions,
        ]
        .spacing(6.0)
        .boxed()
    }
}

/// Parse an edited field: blank or malformed input yields `None` (buffer-only),
/// and a case-insensitive "nan" maps to `f64::NAN`.
fn parse_field_value(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("nan") {
        Some(f64::NAN)
    } else {
        trimmed.parse().ok()
    }
}

fn format_field_value(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_string()
    } else {
        value.to_string()
    }
}

/// A one-line summary of a command for the list: the position when it has one,
/// otherwise the first few field values. Never panics on `Unknown`.
fn command_summary(command: &MissionCommand) -> String {
    if let Some(pos) = command.position() {
        return format!("{:.6}, {:.6}, {:.1} m", pos.lat, pos.lon, pos.alt_m);
    }

    if let MissionCommand::Unknown { command, .. } = command {
        return format!("{command:?}");
    }

    command
        .fields()
        .iter()
        .take(3)
        .map(|spec| format!("{}={}", spec.label, format_field_value(spec.value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn file_dialog(path: Option<&Path>) -> AsyncFileDialog {
    let mut file_dialog = AsyncFileDialog::new();

    if let Some(path) = path {
        file_dialog = file_dialog.set_directory(path);
    }

    file_dialog
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use crate::foundation::{DEFAULT_GCS_ID, MavlinkId};
    use crate::services::{MavError, MavlinkRouter};

    const A: SystemId = SystemId(1);
    const B: SystemId = SystemId(2);

    /// An id that is not registered in the test fleet.
    const UNKNOWN: SystemId = SystemId(9);

    /// The address of `id`'s autopilot endpoint, for feeding `observe`.
    fn source(id: SystemId) -> MavlinkId {
        MavlinkId {
            system: id.0,
            component: 1,
        }
    }

    /// The view-local mission state for `id`.
    fn ui(panel: &MissionPanel, id: SystemId) -> &MissionUi {
        panel.ui.get(&id).expect("vehicle has mission ui")
    }

    /// Wrap `kind` in the envelope addressed to `system`.
    fn message(system: SystemId, kind: MessageKind) -> Message {
        Message { system, kind }
    }

    fn setup() -> (MavlinkRouter, Fleet) {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        fleet.observe(source(A), Instant::now(), None);
        (router, fleet)
    }

    fn step(
        panel: &mut MissionPanel,
        router: &MavlinkRouter,
        fleet: &mut Fleet,
        message: Message,
    ) -> (Task<Message>, Option<Event>) {
        let services = FleetServices::new(router);
        let mut ctx = Context {
            services: &services,
            fleet,
            file_picker_path: None,
        };
        panel.update(message, &mut ctx)
    }

    fn mission(fleet: &Fleet) -> &Mission {
        &fleet.vehicle(A).unwrap().mission
    }

    fn lat(fleet: &Fleet, index: usize) -> f64 {
        mission(fleet)
            .get(index)
            .unwrap()
            .command
            .position()
            .unwrap()
            .lat
    }

    fn lon(fleet: &Fleet, index: usize) -> f64 {
        mission(fleet)
            .get(index)
            .unwrap()
            .command
            .position()
            .unwrap()
            .lon
    }

    fn mission_mark_synced(fleet: &mut Fleet) {
        fleet.vehicle_mut(A).unwrap().mission.mark_synced(None);
    }

    #[test]
    fn add_item_appends_and_selects_a_waypoint() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::AddItem),
        );

        assert_eq!(mission(&fleet).len(), 1);
        assert!(mission(&fleet).dirty);
        assert_eq!(ui(&panel, A).selected, Some(0));
        assert!(matches!(
            mission(&fleet).get(0).unwrap().command,
            MissionCommand::Waypoint { .. }
        ));
    }

    #[test]
    fn edit_field_updates_domain_and_buffer() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::AddItem),
        );
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                A,
                MessageKind::EditField(0, MissionField::Latitude, "47.5".to_string()),
            ),
        );

        assert!((lat(&fleet, 0) - 47.5).abs() < 1e-9);
        assert_eq!(ui(&panel, A).edit[&MissionField::Latitude], "47.5");

        // Unparseable input keeps the buffer but leaves the domain untouched.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                A,
                MessageKind::EditField(0, MissionField::Latitude, "abc".to_string()),
            ),
        );
        assert_eq!(ui(&panel, A).edit[&MissionField::Latitude], "abc");
        assert!((lat(&fleet, 0) - 47.5).abs() < 1e-9);
    }

    #[test]
    fn change_command_replaces_the_variant() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::AddItem),
        );
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                A,
                MessageKind::ChangeCommand(0, MissionCommandKind::ConditionDelay),
            ),
        );

        assert_eq!(
            mission(&fleet).get(0).unwrap().command.kind(),
            Some(MissionCommandKind::ConditionDelay)
        );
        assert!(ui(&panel, A).edit.is_empty());
    }

    #[test]
    fn move_and_remove_reorder_the_list() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        for _ in 0..3 {
            let _ = step(
                &mut panel,
                &router,
                &mut fleet,
                message(A, MessageKind::AddItem),
            );
        }
        for (index, value) in ["1.0", "2.0", "3.0"].iter().enumerate() {
            let _ = step(
                &mut panel,
                &router,
                &mut fleet,
                message(
                    A,
                    MessageKind::EditField(index, MissionField::Latitude, (*value).to_string()),
                ),
            );
        }

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::MoveUp(2)),
        );
        assert_eq!(
            [lat(&fleet, 0), lat(&fleet, 1), lat(&fleet, 2)],
            [1.0, 3.0, 2.0]
        );
        assert_eq!(ui(&panel, A).selected, Some(1));

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::RemoveItem(1)),
        );
        assert_eq!(mission(&fleet).len(), 2);
        assert_eq!([lat(&fleet, 0), lat(&fleet, 1)], [1.0, 2.0]);
        assert_eq!(ui(&panel, A).selected, None);

        // Out-of-range moves are no-ops.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::MoveDown(1)),
        );
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::MoveUp(0)),
        );
    }

    #[test]
    fn downloaded_result_writes_the_mission() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let mut downloaded = Mission::default();
        downloaded.push(MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command: MissionCommand::default_for(
                MissionCommandKind::Waypoint,
                Position {
                    lat: 47.0,
                    lon: 8.0,
                    alt_m: 100.0,
                },
            ),
        });
        downloaded.mark_synced(Some(5));

        // Open an editor first, so the download has view state to reset.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::AddItem),
        );
        assert_eq!(ui(&panel, A).selected, Some(0));

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::Downloaded(Ok(downloaded))),
        );

        assert_eq!(mission(&fleet).len(), 1);
        assert_eq!(mission(&fleet).vehicle_id, Some(5));
        assert!(!mission(&fleet).dirty);
        assert_eq!(ui(&panel, A).selected, None);
    }

    #[test]
    fn downloaded_result_targets_the_named_vehicle() {
        let (router, mut fleet) = setup();
        fleet.observe(source(B), Instant::now(), None);
        assert_eq!(fleet.primary(), Some(A));
        let mut panel = MissionPanel::new();

        let mut downloaded = Mission::default();
        downloaded.push(MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command: MissionCommand::default_for(
                MissionCommandKind::Waypoint,
                Position {
                    lat: 12.0,
                    lon: 34.0,
                    alt_m: 56.0,
                },
            ),
        });
        downloaded.mark_synced(Some(9));

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(B, MessageKind::Downloaded(Ok(downloaded))),
        );

        // The result lands on `B`, the vehicle it was started against...
        let b = &fleet.vehicle(B).unwrap().mission;
        assert_eq!(b.len(), 1);
        assert_eq!(b.vehicle_id, Some(9));

        // ...and leaves the primary `A` untouched.
        assert!(mission(&fleet).is_empty());
        assert_eq!(mission(&fleet).vehicle_id, None);
    }

    #[test]
    fn load_finished_targets_the_named_vehicle() {
        let (router, mut fleet) = setup();
        fleet.observe(source(B), Instant::now(), None);
        assert_eq!(fleet.primary(), Some(A));
        let mut panel = MissionPanel::new();

        let items = vec![MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command: MissionCommand::default_for(
                MissionCommandKind::Waypoint,
                Position {
                    lat: 12.0,
                    lon: 34.0,
                    alt_m: 56.0,
                },
            ),
        }];

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                B,
                MessageKind::LoadFinished(PathBuf::from("/tmp/mission.waypoints"), Ok(items)),
            ),
        );

        // The loaded mission lands on `B`, the vehicle the dialog was opened for...
        let b = &fleet.vehicle(B).unwrap().mission;
        assert_eq!(b.len(), 1);
        assert!(b.dirty);

        // ...and leaves the primary `A` untouched.
        assert!(mission(&fleet).is_empty());
        assert!(!mission(&fleet).dirty);
    }

    #[test]
    fn every_message_is_a_no_op_without_a_vehicle() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        let mut panel = MissionPanel::new();

        // Every acting intent carries the id of a vehicle the fleet has never
        // heard of: each must be a silent no-op that creates no view state.
        let messages = [
            message(UNKNOWN, MessageKind::Download),
            message(UNKNOWN, MessageKind::Upload),
            message(UNKNOWN, MessageKind::Clear),
            message(UNKNOWN, MessageKind::Start),
            message(UNKNOWN, MessageKind::Pause),
            message(UNKNOWN, MessageKind::Resume),
            message(UNKNOWN, MessageKind::AddItem),
            message(UNKNOWN, MessageKind::RemoveItem(0)),
            message(UNKNOWN, MessageKind::MoveUp(0)),
            message(UNKNOWN, MessageKind::MoveDown(0)),
            message(UNKNOWN, MessageKind::Select(0)),
            message(UNKNOWN, MessageKind::BeginDrag(0)),
            message(UNKNOWN, MessageKind::EndDrag),
            message(UNKNOWN, MessageKind::SetCurrent(0)),
            message(
                UNKNOWN,
                MessageKind::ChangeCommand(0, MissionCommandKind::Waypoint),
            ),
            message(
                UNKNOWN,
                MessageKind::EditField(0, MissionField::Latitude, "1.0".to_string()),
            ),
            message(UNKNOWN, MessageKind::TogglePlacing),
            message(
                UNKNOWN,
                MessageKind::MoveWaypointTo(
                    0,
                    GeoPosition {
                        lat: 12.0,
                        lon: 34.0,
                    },
                ),
            ),
            message(
                UNKNOWN,
                MessageKind::AddWaypointAt(GeoPosition {
                    lat: 12.0,
                    lon: 34.0,
                }),
            ),
            message(UNKNOWN, MessageKind::SaveDialog),
            message(UNKNOWN, MessageKind::LoadDialog),
            message(UNKNOWN, MessageKind::Downloaded(Err(MavError::Timeout))),
            message(UNKNOWN, MessageKind::Uploaded(Err(MavError::Timeout))),
            message(UNKNOWN, MessageKind::Cleared(Err(MavError::Timeout))),
            message(UNKNOWN, MessageKind::CurrentSet(Err(MavError::Timeout))),
            message(
                UNKNOWN,
                MessageKind::CommandFinished(MissionAction::Start, Err(MavError::Timeout)),
            ),
        ];

        for envelope in messages {
            let _ = step(&mut panel, &router, &mut fleet, envelope);
        }

        assert!(fleet.vehicles().is_empty());
        assert!(panel.ui.is_empty());
    }

    #[test]
    fn operations_target_the_addressed_vehicle() {
        let (router, mut fleet) = setup();
        fleet.observe(source(B), Instant::now(), None);
        assert_eq!(fleet.primary(), Some(A));
        let mut panel = MissionPanel::new();

        // Both intents are addressed to `B` while `A` is the primary display.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(B, MessageKind::AddItem),
        );
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                B,
                MessageKind::EditField(0, MissionField::Latitude, "47.5".to_string()),
            ),
        );

        // `B`'s mission got the item and the value...
        let b = &fleet.vehicle(B).unwrap().mission;
        assert_eq!(b.len(), 1);
        let b_lat = b.get(0).unwrap().command.position().unwrap().lat;
        assert!((b_lat - 47.5).abs() < 1e-9);
        assert_eq!(ui(&panel, B).edit[&MissionField::Latitude], "47.5");

        // ...while the displayed `A` got neither mission content...
        assert!(mission(&fleet).is_empty());

        // ...nor view state: `ui_mut` is only reachable through an existing
        // vehicle.
        assert!(!panel.ui.contains_key(&A));

        // And now that address-only state exists for `A`, it is not disturbed
        // by intents that stay addressed to `B`.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::AddItem),
        );
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                B,
                MessageKind::EditField(0, MissionField::Longitude, "8.0".to_string()),
            ),
        );
        assert_eq!(ui(&panel, A).selected, Some(0));
        assert!(ui(&panel, A).edit.is_empty());
        let b_lon = fleet
            .vehicle(B)
            .unwrap()
            .mission
            .get(0)
            .unwrap()
            .command
            .position()
            .unwrap()
            .lon;
        assert!((b_lon - 8.0).abs() < 1e-9);
        assert!(mission(&fleet).dirty);
    }

    #[test]
    fn moving_items_clears_the_stale_editor_buffer() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        for _ in 0..3 {
            let _ = step(
                &mut panel,
                &router,
                &mut fleet,
                message(A, MessageKind::AddItem),
            );
        }
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                A,
                MessageKind::EditField(2, MissionField::Latitude, "47.5".to_string()),
            ),
        );
        assert!(!ui(&panel, A).edit.is_empty());

        // Moving the edited item opens it at a new index; the stale in-progress
        // values must not reappear there (or fold into the wrong item).
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::MoveUp(2)),
        );

        assert!(ui(&panel, A).edit.is_empty());
        assert_eq!(ui(&panel, A).selected, Some(1));
    }

    #[test]
    fn move_waypoint_to_updates_position_and_dirty() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::AddItem),
        );
        mission_mark_synced(&mut fleet);
        assert!(!mission(&fleet).dirty);

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                A,
                MessageKind::MoveWaypointTo(
                    0,
                    GeoPosition {
                        lat: 12.5,
                        lon: -3.25,
                    },
                ),
            ),
        );

        assert!((lat(&fleet, 0) - 12.5).abs() < 1e-9);
        assert!((lon(&fleet, 0) + 3.25).abs() < 1e-9);
        assert!(mission(&fleet).dirty);
    }

    #[test]
    fn add_waypoint_at_appends_and_selects() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                A,
                MessageKind::AddWaypointAt(GeoPosition {
                    lat: 47.0,
                    lon: 8.0,
                }),
            ),
        );

        assert_eq!(mission(&fleet).len(), 1);
        assert!(mission(&fleet).dirty);
        assert_eq!(ui(&panel, A).selected, Some(0));
        assert!((lat(&fleet, 0) - 47.0).abs() < 1e-9);
        assert!((lon(&fleet, 0) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn toggle_placing_flips_is_placing() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        assert!(!panel.is_placing(Some(A)));
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::TogglePlacing),
        );
        assert!(panel.is_placing(Some(A)));
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::TogglePlacing),
        );
        assert!(!panel.is_placing(Some(A)));
    }

    #[test]
    fn mission_ui_state_is_isolated_per_vehicle() {
        let (router, mut fleet) = setup();
        fleet.observe(source(B), Instant::now(), None);
        let mut panel = MissionPanel::new();

        // Populate A's editor, placement mode, and an in-flight transfer.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::AddItem),
        );
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(
                A,
                MessageKind::EditField(0, MissionField::Latitude, "1.5".to_string()),
            ),
        );
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::TogglePlacing),
        );
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::Download),
        );
        assert_eq!(ui(&panel, A).selected, Some(0));
        assert_eq!(ui(&panel, A).edit[&MissionField::Latitude], "1.5");
        assert!(ui(&panel, A).busy);
        assert!(panel.is_placing(Some(A)));

        // Switching primary exposes a clean slate for B.
        assert!(fleet.set_primary(B));
        assert!(!panel.ui.contains_key(&B));
        assert!(!panel.is_placing(Some(B)));
        assert_eq!(panel.dragging(Some(B)), None);

        // Switching back preserves everything A held.
        assert!(fleet.set_primary(A));
        assert_eq!(ui(&panel, A).selected, Some(0));
        assert_eq!(ui(&panel, A).edit[&MissionField::Latitude], "1.5");
        assert!(ui(&panel, A).busy);
        assert!(panel.is_placing(Some(A)));
    }

    #[test]
    fn begin_drag_selects_and_tracks_until_end_drag() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        for _ in 0..3 {
            let _ = step(
                &mut panel,
                &router,
                &mut fleet,
                message(A, MessageKind::AddItem),
            );
        }

        // The list's Edit button selects without starting a drag.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::Select(1)),
        );
        assert_eq!(ui(&panel, A).selected, Some(1));
        assert_eq!(ui(&panel, A).dragging, None);

        // A map press selects and drags the waypoint.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::BeginDrag(2)),
        );
        assert_eq!(ui(&panel, A).selected, Some(2));
        assert_eq!(ui(&panel, A).dragging, Some(2));
        assert_eq!(panel.dragging(Some(A)), Some(2));

        // Releasing the drag keeps the selection.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            message(A, MessageKind::EndDrag),
        );
        assert_eq!(ui(&panel, A).selected, Some(2));
        assert_eq!(ui(&panel, A).dragging, None);
        assert_eq!(panel.dragging(Some(A)), None);
    }
}
