//! The mission feature: editing a vehicle's flight plan and driving the
//! MAVLink mission transactions against the primary vehicle.
//!
//! Mission data itself lives on the [`Fleet`] (the single owner of vehicle
//! state), which the root passes in by mutable reference; the router is passed
//! in for transactions. All operations target the primary vehicle.
//!
//! The panel owns only view-local state: which item's editor is open and the
//! in-progress text buffers, so partial input survives a redraw.

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
    foundation::MavlinkId,
    mav::{self, MavlinkRouter},
    services::vehicle::VehicleClient,
    style,
    widget::virtualized::virtual_list,
};

/// Height of a single mission-list row.
const ROW_HEIGHT: f32 = 28.0;

pub struct MissionPanel {
    /// Index of the item whose editor is open.
    selected: Option<usize>,
    /// In-progress text for the selected item's fields (survives partial input).
    edit: BTreeMap<MissionField, String>,
    /// A transfer/command is in flight; the toolbar is disabled.
    busy: bool,
    /// Whether the next map click should place a new waypoint.
    placing: bool,
}

impl Default for MissionPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything the panel needs from the rest of the application.
pub struct Context<'a> {
    pub router: &'a MavlinkRouter,
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

#[derive(Debug, Clone)]
pub enum Message {
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
    SetCurrent(usize),
    ChangeCommand(usize, MissionCommandKind),
    EditField(usize, MissionField, String),

    /// Toggle "place waypoint" mode for the next map click.
    TogglePlacing,
    /// A map drag moved waypoint `usize` to a new geographic position.
    MoveWaypointTo(usize, GeoPosition),
    /// A map click in "place waypoint" mode adds a waypoint at this position.
    AddWaypointAt(GeoPosition),

    Downloaded(MavlinkId, mav::Result<Mission>),
    Uploaded(MavlinkId, mav::Result<u32>),
    Cleared(MavlinkId, mav::Result<()>),
    CurrentSet(MavlinkId, mav::Result<()>),
    CommandFinished(MavlinkId, MissionAction, mav::Result<CommandAck>),

    /// Open a file-picker dialog to choose a save location.
    SaveDialog,
    /// Write the primary vehicle's mission to the chosen path.
    SaveToFile(PathBuf),
    /// The result of writing the mission to disk.
    SaveFinished(PathBuf, Result<(), String>),
    /// Open a file-picker dialog to choose a file to load.
    LoadDialog,
    /// Parse the chosen file and replace the primary vehicle's mission.
    LoadFromFile(PathBuf),
    /// The result of reading a mission file from disk.
    LoadFinished(PathBuf, Result<Vec<MissionItem>, String>),
}

impl MissionPanel {
    pub fn new() -> Self {
        Self {
            selected: None,
            edit: BTreeMap::new(),
            busy: false,
            placing: false,
        }
    }

    /// Whether the map is in "place waypoint" mode.
    pub fn is_placing(&self) -> bool {
        self.placing
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
        match message {
            Message::Download => {
                let id = ctx.fleet.primary()?;

                self.busy = true;
                let client = VehicleClient::new(ctx.router, id);
                return Some((
                    Task::perform(
                        async move { client.download_mission().await },
                        move |result| Message::Downloaded(id, result),
                    ),
                    None,
                ));
            }
            Message::Downloaded(id, result) => {
                self.busy = false;

                match result {
                    Ok(mission) => {
                        if let Some(vehicle) = ctx.fleet.vehicle_mut(id) {
                            vehicle.mission = mission;
                        }
                        self.selected = None;
                        self.edit.clear();
                    }
                    Err(error) => log::warn!("Mission download for {id:?} failed: {error}"),
                }
            }
            Message::Upload => {
                let id = ctx.fleet.primary()?;
                let vehicle = ctx.fleet.vehicle(id)?;

                let mission = vehicle.mission.clone();
                self.busy = true;
                let client = VehicleClient::new(ctx.router, id);
                return Some((
                    Task::perform(
                        async move { client.upload_mission(&mission).await },
                        move |result| Message::Uploaded(id, result),
                    ),
                    None,
                ));
            }
            Message::Uploaded(id, result) => {
                self.busy = false;

                match result {
                    Ok(vehicle_id) => {
                        if let Some(vehicle) = ctx.fleet.vehicle_mut(id) {
                            vehicle.mission.mark_synced(Some(vehicle_id));
                        }
                    }
                    Err(error) => log::warn!("Mission upload to {id:?} failed: {error}"),
                }
            }
            Message::Clear => {
                let id = ctx.fleet.primary()?;

                self.busy = true;
                let client = VehicleClient::new(ctx.router, id);
                return Some((
                    Task::perform(async move { client.clear_mission().await }, move |result| {
                        Message::Cleared(id, result)
                    }),
                    None,
                ));
            }
            Message::Cleared(id, result) => {
                self.busy = false;

                match result {
                    Ok(()) => {
                        if let Some(vehicle) = ctx.fleet.vehicle_mut(id) {
                            vehicle.mission.clear();
                            vehicle.mission.mark_synced(None);
                        }
                        self.selected = None;
                        self.edit.clear();
                    }
                    Err(error) => log::warn!("Mission clear on {id:?} failed: {error}"),
                }
            }
            Message::Start => return Some((self.run_command(MissionAction::Start, ctx), None)),
            Message::Pause => return Some((self.run_command(MissionAction::Pause, ctx), None)),
            Message::Resume => return Some((self.run_command(MissionAction::Resume, ctx), None)),
            Message::CommandFinished(id, action, result) => {
                self.busy = false;
                if let Err(error) = result {
                    log::warn!("Mission {action:?} on {id:?} failed: {error}");
                }
            }
            Message::SetCurrent(index) => {
                let id = ctx.fleet.primary()?;

                let client = VehicleClient::new(ctx.router, id);
                return Some((
                    Task::perform(
                        async move { client.set_current_mission_item(index as u16).await },
                        move |result| Message::CurrentSet(id, result),
                    ),
                    None,
                ));
            }
            Message::CurrentSet(id, result) => {
                if let Err(error) = result {
                    log::warn!("Setting the current mission item on {id:?} failed: {error}");
                }
            }
            Message::AddItem => {
                let id = ctx.fleet.primary()?;
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

                self.selected = Some(vehicle.mission.len() - 1);
                self.edit.clear();
            }
            Message::RemoveItem(index) => {
                let id = ctx.fleet.primary()?;
                let vehicle = ctx.fleet.vehicle_mut(id)?;

                vehicle.mission.remove(index)?;

                match self.selected {
                    Some(selected) if selected == index => {
                        self.selected = None;
                        self.edit.clear();
                    }
                    Some(selected) if selected > index => self.selected = Some(selected - 1),
                    _ => {}
                }
            }
            Message::MoveUp(index) => {
                self.move_item(ctx, index, true);
            }
            Message::MoveDown(index) => {
                self.move_item(ctx, index, false);
            }
            Message::Select(index) => {
                // Selection indexes the primary vehicle's mission; meaningless without one.
                ctx.fleet.primary()?;

                if self.selected != Some(index) {
                    self.selected = Some(index);
                    self.edit.clear();
                }
            }
            Message::ChangeCommand(index, kind) => {
                let id = ctx.fleet.primary()?;
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
                    self.edit.clear();
                }
            }
            Message::EditField(index, field, text) => {
                let id = ctx.fleet.primary()?;
                let vehicle = ctx.fleet.vehicle_mut(id)?;

                self.edit.insert(field, text.clone());

                if let Some(value) = parse_field_value(&text) {
                    vehicle
                        .mission
                        .update(index, |item| item.command.set_field(field, value));
                }
            }
            Message::TogglePlacing => {
                self.placing = !self.placing;
            }
            Message::MoveWaypointTo(index, position) => {
                let id = ctx.fleet.primary()?;
                let vehicle = ctx.fleet.vehicle_mut(id)?;

                vehicle.mission.update(index, |item| {
                    item.command.set_field(MissionField::Latitude, position.lat);
                    item.command
                        .set_field(MissionField::Longitude, position.lon);
                });
            }
            Message::AddWaypointAt(position) => {
                let id = ctx.fleet.primary()?;
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

                self.selected = Some(vehicle.mission.len() - 1);
                self.edit.clear();
            }
            Message::SaveDialog => {
                let file_dialog =
                    file_dialog(ctx.file_picker_path).set_file_name("mission.waypoints");

                return Some((
                    Task::future(async move {
                        file_dialog
                            .save_file()
                            .await
                            .map(|file| file.path().to_owned())
                    })
                    .and_then(|path| Task::done(Message::SaveToFile(path))),
                    None,
                ));
            }
            Message::SaveToFile(path) => {
                let id = ctx.fleet.primary()?;
                let vehicle = ctx.fleet.vehicle(id)?;
                let items = vehicle.mission.items().to_vec();
                log::info!("Saving mission to file: {path:?}");

                let save_path = path.clone();
                return Some((
                    iced_runtime::task::blocking(move |mut sender| {
                        let result = std::fs::write(&save_path, write_wpl(&items))
                            .map_err(|error| error.to_string());
                        let _ = sender.try_send(Message::SaveFinished(save_path, result));
                    }),
                    None,
                ));
            }
            Message::SaveFinished(path, result) => {
                if let Err(error) = result {
                    log::error!("Error saving mission to file: {error}");
                }

                return Some((Task::none(), Some(Event::SetFilePickerPath(path))));
            }
            Message::LoadDialog => {
                let file_dialog = file_dialog(ctx.file_picker_path);

                return Some((
                    Task::future(async move {
                        file_dialog
                            .pick_file()
                            .await
                            .map(|file| file.path().to_owned())
                    })
                    .and_then(|path| Task::done(Message::LoadFromFile(path))),
                    None,
                ));
            }
            Message::LoadFromFile(path) => {
                log::info!("Loading mission from file: {path:?}");

                let load_path = path.clone();
                return Some((
                    iced_runtime::task::blocking(move |mut sender| {
                        let result = std::fs::read_to_string(&load_path)
                            .map_err(|error| error.to_string())
                            .and_then(|text| parse_wpl(&text).map_err(|error| error.to_string()));
                        let _ = sender.try_send(Message::LoadFinished(load_path, result));
                    }),
                    None,
                ));
            }
            Message::LoadFinished(path, result) => match result {
                Ok(items) => {
                    if let Some(id) = ctx.fleet.primary()
                        && let Some(vehicle) = ctx.fleet.vehicle_mut(id)
                    {
                        vehicle.mission.set_items(items);
                        vehicle.mission.mark_dirty();
                    }
                    self.selected = None;
                    self.edit.clear();

                    return Some((Task::none(), Some(Event::SetFilePickerPath(path))));
                }
                Err(error) => log::error!("Error loading mission from file: {error}"),
            },
        }

        None
    }
    fn run_command(&mut self, action: MissionAction, ctx: &mut Context) -> Task<Message> {
        let Some(id) = ctx.fleet.primary() else {
            return Task::none();
        };

        self.busy = true;
        let client = VehicleClient::new(ctx.router, id);
        Task::perform(
            async move {
                match action {
                    MissionAction::Start => client.start_mission().await,
                    MissionAction::Pause => client.pause_mission(true).await,
                    MissionAction::Resume => client.pause_mission(false).await,
                }
            },
            move |result| Message::CommandFinished(id, action, result),
        )
    }

    fn move_item(&mut self, ctx: &mut Context, index: usize, up: bool) {
        let Some(id) = ctx.fleet.primary() else {
            return;
        };
        let Some(vehicle) = ctx.fleet.vehicle_mut(id) else {
            return;
        };

        let moved = if up {
            vehicle.mission.move_up(index)
        } else {
            vehicle.mission.move_down(index)
        };

        if moved {
            self.selected = Some(if up { index - 1 } else { index + 1 });
        }
    }

    /// Render the mission panel. A pure function of panel state and the primary
    /// vehicle's mission.
    pub fn view<'a>(
        &'a self,
        vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
        primary: Option<MavlinkId>,
        theme: &Theme,
    ) -> Element<'a, Message> {
        let Some(mav_id) = primary else {
            return Space::new().boxed();
        };

        let Some(vehicle) = vehicles.get(&mav_id) else {
            return Space::new().boxed();
        };

        let mission = &vehicle.mission;

        let enabled = !self.busy;
        let toolbar = row![
            Button::new("Download").on_press_maybe(enabled.then_some(Message::Download)),
            Button::new("Upload").on_press_maybe(enabled.then_some(Message::Upload)),
            Button::new("Clear").on_press_maybe(enabled.then_some(Message::Clear)),
            Button::new("Start").on_press_maybe(enabled.then_some(Message::Start)),
            Button::new("Pause").on_press_maybe(enabled.then_some(Message::Pause)),
            Button::new("Resume").on_press_maybe(enabled.then_some(Message::Resume)),
            Button::new("Add waypoint").on_press_maybe(enabled.then_some(Message::AddItem)),
            Button::new(if self.placing {
                "Placing…"
            } else {
                "Place waypoint"
            })
            .on_press_maybe(enabled.then_some(Message::TogglePlacing)),
            Button::new("Save to file").on_press_maybe(enabled.then_some(Message::SaveDialog)),
            Button::new("Load from file").on_press_maybe(enabled.then_some(Message::LoadDialog)),
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
                Button::new("Edit").on_press(Message::Select(index)),
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

        let editor: Element<'_, Message> = match self.selected {
            Some(index) if index < mission.len() => {
                self.view_editor(index, mission.get(index).expect("index checked"))
            }
            _ => Space::new().boxed(),
        };

        container(column![toolbar, text(status), list, editor].spacing(6.0))
            .padding(8.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .boxed()
    }

    fn view_editor<'a>(&'a self, index: usize, item: &'a MissionItem) -> Element<'a, Message> {
        let kind = pick_list(
            item.command.kind(),
            MissionCommandKind::ALL,
            |kind: &MissionCommandKind| kind.name().to_string(),
        )
        .on_select(move |kind| Message::ChangeCommand(index, kind));

        let mut fields = Column::<Element<'a, Message>>::new().spacing(4.0);

        for spec in item.command.fields() {
            let value = self
                .edit
                .get(&spec.field)
                .cloned()
                .unwrap_or_else(|| format_field_value(spec.value));

            let input = TextInput::new("", value)
                .on_input(move |text| Message::EditField(index, spec.field, text))
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
            Button::new("Remove").on_press(Message::RemoveItem(index)),
            Button::new("Move up").on_press(Message::MoveUp(index)),
            Button::new("Move down").on_press(Message::MoveDown(index)),
            Button::new("Set current").on_press(Message::SetCurrent(index)),
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

    use crate::foundation::DEFAULT_GCS_ID;

    const A: MavlinkId = MavlinkId {
        system: 1,
        component: 1,
    };
    const B: MavlinkId = MavlinkId {
        system: 2,
        component: 1,
    };

    fn setup() -> (MavlinkRouter, Fleet) {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        fleet.observe(A, Instant::now(), None);
        (router, fleet)
    }

    fn step(
        panel: &mut MissionPanel,
        router: &MavlinkRouter,
        fleet: &mut Fleet,
        message: Message,
    ) -> (Task<Message>, Option<Event>) {
        let mut ctx = Context {
            router,
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

        let _ = step(&mut panel, &router, &mut fleet, Message::AddItem);

        assert_eq!(mission(&fleet).len(), 1);
        assert!(mission(&fleet).dirty);
        assert_eq!(panel.selected, Some(0));
        assert!(matches!(
            mission(&fleet).get(0).unwrap().command,
            MissionCommand::Waypoint { .. }
        ));
    }

    #[test]
    fn edit_field_updates_domain_and_buffer() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let _ = step(&mut panel, &router, &mut fleet, Message::AddItem);
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            Message::EditField(0, MissionField::Latitude, "47.5".to_string()),
        );

        assert!((lat(&fleet, 0) - 47.5).abs() < 1e-9);
        assert_eq!(panel.edit[&MissionField::Latitude], "47.5");

        // Unparseable input keeps the buffer but leaves the domain untouched.
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            Message::EditField(0, MissionField::Latitude, "abc".to_string()),
        );
        assert_eq!(panel.edit[&MissionField::Latitude], "abc");
        assert!((lat(&fleet, 0) - 47.5).abs() < 1e-9);
    }

    #[test]
    fn change_command_replaces_the_variant() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let _ = step(&mut panel, &router, &mut fleet, Message::AddItem);
        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ChangeCommand(0, MissionCommandKind::ConditionDelay),
        );

        assert_eq!(
            mission(&fleet).get(0).unwrap().command.kind(),
            Some(MissionCommandKind::ConditionDelay)
        );
        assert!(panel.edit.is_empty());
    }

    #[test]
    fn move_and_remove_reorder_the_list() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        for _ in 0..3 {
            let _ = step(&mut panel, &router, &mut fleet, Message::AddItem);
        }
        for (index, value) in ["1.0", "2.0", "3.0"].iter().enumerate() {
            let _ = step(
                &mut panel,
                &router,
                &mut fleet,
                Message::EditField(index, MissionField::Latitude, (*value).to_string()),
            );
        }

        let _ = step(&mut panel, &router, &mut fleet, Message::MoveUp(2));
        assert_eq!(
            [lat(&fleet, 0), lat(&fleet, 1), lat(&fleet, 2)],
            [1.0, 3.0, 2.0]
        );
        assert_eq!(panel.selected, Some(1));

        let _ = step(&mut panel, &router, &mut fleet, Message::RemoveItem(1));
        assert_eq!(mission(&fleet).len(), 2);
        assert_eq!([lat(&fleet, 0), lat(&fleet, 1)], [1.0, 2.0]);
        assert_eq!(panel.selected, None);

        // Out-of-range moves are no-ops.
        let _ = step(&mut panel, &router, &mut fleet, Message::MoveDown(1));
        let _ = step(&mut panel, &router, &mut fleet, Message::MoveUp(0));
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

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            Message::Downloaded(A, Ok(downloaded)),
        );

        assert_eq!(mission(&fleet).len(), 1);
        assert_eq!(mission(&fleet).vehicle_id, Some(5));
        assert!(!mission(&fleet).dirty);
        assert_eq!(panel.selected, None);
    }

    #[test]
    fn downloaded_result_targets_the_named_vehicle() {
        let (router, mut fleet) = setup();
        fleet.observe(B, Instant::now(), None);
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
            Message::Downloaded(B, Ok(downloaded)),
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
    fn every_message_is_a_no_op_without_a_vehicle() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        let mut panel = MissionPanel::new();

        let messages = [
            Message::Download,
            Message::Upload,
            Message::Clear,
            Message::Start,
            Message::Pause,
            Message::Resume,
            Message::AddItem,
            Message::RemoveItem(0),
            Message::MoveUp(0),
            Message::MoveDown(0),
            Message::Select(0),
            Message::SetCurrent(0),
            Message::ChangeCommand(0, MissionCommandKind::Waypoint),
            Message::EditField(0, MissionField::Latitude, "1.0".to_string()),
            Message::Downloaded(A, Err(crate::mav::error::MavError::Timeout)),
            Message::Uploaded(A, Err(crate::mav::error::MavError::Timeout)),
            Message::Cleared(A, Err(crate::mav::error::MavError::Timeout)),
            Message::CurrentSet(A, Err(crate::mav::error::MavError::Timeout)),
            Message::CommandFinished(
                A,
                MissionAction::Start,
                Err(crate::mav::error::MavError::Timeout),
            ),
        ];

        for message in messages {
            let _ = step(&mut panel, &router, &mut fleet, message);
        }

        assert!(fleet.vehicles().is_empty());
        assert_eq!(panel.selected, None);
    }

    #[test]
    fn move_waypoint_to_updates_position_and_dirty() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        let _ = step(&mut panel, &router, &mut fleet, Message::AddItem);
        mission_mark_synced(&mut fleet);
        assert!(!mission(&fleet).dirty);

        let _ = step(
            &mut panel,
            &router,
            &mut fleet,
            Message::MoveWaypointTo(
                0,
                GeoPosition {
                    lat: 12.5,
                    lon: -3.25,
                },
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
            Message::AddWaypointAt(GeoPosition {
                lat: 47.0,
                lon: 8.0,
            }),
        );

        assert_eq!(mission(&fleet).len(), 1);
        assert!(mission(&fleet).dirty);
        assert_eq!(panel.selected, Some(0));
        assert!((lat(&fleet, 0) - 47.0).abs() < 1e-9);
        assert!((lon(&fleet, 0) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn toggle_placing_flips_is_placing() {
        let (router, mut fleet) = setup();
        let mut panel = MissionPanel::new();

        assert!(!panel.is_placing());
        let _ = step(&mut panel, &router, &mut fleet, Message::TogglePlacing);
        assert!(panel.is_placing());
        let _ = step(&mut panel, &router, &mut fleet, Message::TogglePlacing);
        assert!(!panel.is_placing());
    }
}
