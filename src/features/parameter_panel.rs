//! The parameter feature: browsing, filtering, editing, uploading, and
//! saving/loading a component's parameter set.
//!
//! Parameters are per MAVLink *component*: the user picks which component to
//! view (the autopilot by default), and every transaction is addressed to that
//! `(system, component)` pair. Parameter data itself lives on the [`Fleet`]
//! (the single owner of vehicle state), which the root passes in by mutable
//! reference; the fleet services are passed in for transactions. Writing the
//! remembered file-picker directory back to the configuration is requested
//! through [`Event`].
//!
//! The filtered view is derived on every frame from the live parameter map, so
//! there is no cached copy to keep in sync when the vehicle or filter changes.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use iced::{
    Alignment, Color, Element, Font, Length, Task, Theme, Widget,
    alignment::Vertical,
    widget::{
        Button, Column, ProgressBar, Space, Text, TextInput, container, pick_list, progress_bar,
        row, rule, space, tooltip,
    },
};
use mav_param::{Ident, Value};
use rfd::AsyncFileDialog;

use crate::{
    domain::fleet::Fleet,
    domain::parameter::{
        self, ChangedStatus, ParamState, Parameter, Parameters, value_as_string, value_parse_as,
        value_type_matches, value_type_name,
    },
    domain::vehicle::Vehicle,
    foundation::{MavlinkId, SystemId},
    services::{FleetServices, ParamListSummary, Result},
    style,
    widget::virtualized::virtual_list,
};

#[derive(Default, PartialEq)]
pub struct ParameterFilter {
    pub ident_pattern: String,
    pub only_changed: bool,
}

impl ParameterFilter {
    /// Whether `param` matches the filter, given the already-lowercased pattern.
    fn matches(&self, ident: &Ident, param: &Parameter, pattern_lower: &str) -> bool {
        if self.only_changed && matches!(param.state(), ParamState::Unchanged) {
            return false;
        }

        contains_ignore_ascii_case(ident.as_str(), pattern_lower)
    }
}

/// Non-allocating ASCII-case-insensitive substring search. MAVLink parameter
/// names are ASCII, so this is equivalent to lowercasing both sides.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }

    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    h.windows(n.len())
        .any(|window| window.eq_ignore_ascii_case(n))
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Filter for only certain parameters
    FilterIdentPattern(String),

    /// Whether to only show changed parameters
    FilterOnlyChanged(bool),

    /// Initiate a parameter list request to the target
    ListReload(MavlinkId),

    /// A parameter list request to the target has started, e.g. the automatic
    /// initialisation on first sight of a vehicle.
    ListReloadStarted(MavlinkId),

    /// The parameter list request to the target has finished
    ListReloadResult(MavlinkId, Result<ParamListSummary>),

    /// Upload all modified parameters to the target
    UploadAll(MavlinkId),

    /// Reset a local parameter to its target-default
    ValueReset(MavlinkId, Ident),

    /// Upload a local parameter to the target
    ValueUpload(MavlinkId, Ident, Value),

    /// The result of uploading a local parameter to the target
    ValueUploadResult(MavlinkId, Ident, Value, Result<Value>),

    /// Edit the text-value buffer of the local parameter
    BufferEdit(MavlinkId, Ident, String),

    /// Select which component's parameters the panel shows.
    SelectComponent(MavlinkId),

    /// Open a file-picker dialog to choose a save location for the addressed
    /// component.
    SaveDialog(MavlinkId),

    /// Save the parameters to the chosen path
    SaveToFile(PathBuf, Parameters),

    /// The result of writing the parameters to disk
    SaveFinished(PathBuf, Result<(), String>),

    /// Open a file-picker dialog to choose file to load
    LoadDialog(MavlinkId),

    /// Load parameters from the chosen path
    LoadFromFile(PathBuf, MavlinkId),

    /// The result of reading a parameter file from disk
    LoadFinished(PathBuf, MavlinkId, Result<Parameters, String>),
}

/// Everything the panel needs from the rest of the application to run a
/// parameter transaction.
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

pub struct ParameterPanel {
    filter: ParameterFilter,
    namespace_sep: char,
    /// Text currently typed into each parameter's value field, keyed by the
    /// parameter's address. Kept here (not on the domain `Parameter`) so that
    /// switching components or vehicles and back restores the in-progress edit.
    editing: BTreeMap<MavlinkId, BTreeMap<Ident, String>>,
    /// Abort handles for in-flight uploads, dropped (aborting) when an upload
    /// is superseded by an edit, a reset, or its own completion.
    uploads: BTreeMap<(MavlinkId, Ident), iced::task::Handle>,
    /// Addresses with a parameter list download in flight, so the view can
    /// disable the reload button without querying the router.
    reloading: BTreeSet<MavlinkId>,
    /// The component whose parameters the user chose to view, per vehicle.
    component: BTreeMap<SystemId, u8>,
}

impl ParameterPanel {
    pub fn new() -> Self {
        Self {
            filter: ParameterFilter::default(),
            namespace_sep: '_',
            editing: BTreeMap::new(),
            uploads: BTreeMap::new(),
            reloading: BTreeSet::new(),
            component: BTreeMap::new(),
        }
    }

    /// The component shown for `vehicle`: the user's selection, else its
    /// autopilot (or the conventional component 1 before one is known).
    fn selected_component(&self, vehicle: &Vehicle) -> u8 {
        self.component
            .get(&vehicle.system)
            .copied()
            .unwrap_or_else(|| vehicle.autopilot_component().unwrap_or(1))
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
            Message::FilterIdentPattern(buffer) => self.filter.ident_pattern = buffer,
            Message::FilterOnlyChanged(changed) => self.filter.only_changed = changed,
            Message::ListReload(address) => return self.list_reload(address, ctx),
            Message::ListReloadStarted(address) => {
                self.reloading.insert(address);
            }
            Message::ListReloadResult(address, result) => self.list_reload_result(address, result),
            Message::UploadAll(address) => return self.upload_all(address, ctx),
            Message::BufferEdit(address, ident, buffer) => {
                self.buffer_edit(address, ident, buffer, ctx)?
            }
            Message::ValueReset(address, ident) => self.value_reset(address, ident, ctx)?,
            Message::ValueUpload(address, ident, value) => {
                return self.value_upload(address, ident, value, ctx);
            }
            Message::ValueUploadResult(address, ident, requested, result) => {
                self.value_upload_result(address, ident, requested, result, ctx)?
            }
            Message::SelectComponent(address) => self.select_component(address),
            Message::SaveDialog(address) => return Some(self.save_dialog(address, ctx)),
            Message::SaveToFile(save_path, parameters) => {
                return Some(self.save_to_file(save_path, parameters));
            }
            Message::SaveFinished(save_path, result) => {
                return Some(self.save_finished(save_path, result));
            }
            Message::LoadDialog(address) => return Some(self.load_dialog(address, ctx)),
            Message::LoadFromFile(load_path, address) => {
                return Some(self.load_from_file(load_path, address));
            }
            Message::LoadFinished(load_path, address, loaded_params) => {
                return self.load_finished(load_path, address, loaded_params, ctx);
            }
        }

        None
    }

    /// Request the addressed component's parameter list.
    fn list_reload(
        &mut self,
        address: MavlinkId,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let system = SystemId::from(address.system);
        let component = address.component;
        let client = ctx.services.vehicle(address);
        let vehicle = ctx.fleet.vehicle_mut(system)?;

        let request_capabilities = vehicle.capabilities(component).is_none();
        vehicle.parameters_mut(component).reset_loading();
        self.reloading.insert(address);

        let task = Task::perform(
            async move { client.fetch_parameters(request_capabilities).await },
            move |result| Message::ListReloadResult(address, result),
        );

        Some((task, None))
    }

    /// Clear the in-flight flag and log a list-download result.
    fn list_reload_result(&mut self, address: MavlinkId, result: Result<ParamListSummary>) {
        self.reloading.remove(&address);
        match result {
            Ok(summary) => log::info!("Parameters from {address:?}: {summary:?}"),
            Err(error) => log::warn!("Parameter download from {address:?} failed: {error}"),
        }
    }

    /// Upload every changed parameter on the addressed component.
    fn upload_all(
        &mut self,
        address: MavlinkId,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let system = SystemId::from(address.system);
        let vehicle = ctx.fleet.vehicle(system)?;
        let parameters = vehicle.parameters(address.component)?;

        // The number of concurrent uploads is limited by the middleware
        let uploads: Vec<_> = parameters
            .iter()
            .filter_map(|(ident, param)| match param.state() {
                ParamState::Changed { value, .. } => Some(Task::done(Message::ValueUpload(
                    address,
                    ident.clone(),
                    *value,
                ))),
                _ => None,
            })
            .collect();

        log::info!("Uploading {} changed parameters", uploads.len());
        Some((Task::batch(uploads), None))
    }

    /// Buffer an edited value and fold it into the parameter.
    fn buffer_edit(
        &mut self,
        address: MavlinkId,
        ident: Ident,
        buffer: String,
        ctx: &mut Context,
    ) -> Option<()> {
        // Editing supersedes any upload in flight for this parameter.
        self.uploads.remove(&(address, ident.clone()));

        {
            let system = SystemId::from(address.system);
            let entry = ctx.fleet.vehicle_mut(system)?;
            let param = entry.parameters_mut(address.component).get_mut(&ident)?;

            param.set_edited(value_parse_as(param.value(), &buffer));
        }

        self.editing
            .entry(address)
            .or_default()
            .insert(ident, buffer);
        Some(())
    }

    /// Reset a pending edit and drop its buffer.
    fn value_reset(&mut self, address: MavlinkId, ident: Ident, ctx: &mut Context) -> Option<()> {
        self.uploads.remove(&(address, ident.clone()));

        {
            let system = SystemId::from(address.system);
            let entry = ctx.fleet.vehicle_mut(system)?;
            let param = entry.parameters_mut(address.component).get_mut(&ident)?;

            param.reset();
        }

        if let Some(editing) = self.editing.get_mut(&address) {
            editing.remove(&ident);
        }
        Some(())
    }

    /// Upload one parameter to the addressed component.
    fn value_upload(
        &mut self,
        address: MavlinkId,
        ident: Ident,
        value: Value,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let system = SystemId::from(address.system);
        let client = ctx.services.vehicle(address);
        let vehicle = ctx.fleet.vehicle_mut(system)?;
        let cap = vehicle.capabilities(address.component).unwrap_or_default();
        let param = vehicle.parameters_mut(address.component).get_mut(&ident)?;

        let ident_cloned = ident.clone();
        let ident_for_result = ident.clone();
        let (task, handle) = Task::perform(
            async move { client.set_parameter(ident_cloned, value, cap).await },
            move |result| {
                Message::ValueUploadResult(address, ident_for_result.clone(), value, result)
            },
        )
        .abortable();

        // Holding the abort handle lets an edit or reset cancel the
        // upload by dropping it; completion drops it too.
        self.uploads
            .insert((address, ident.clone()), handle.abort_on_drop());
        param.begin_upload(value);

        Some((task, None))
    }

    /// Fold a parameter upload result into the addressed component.
    fn value_upload_result(
        &mut self,
        address: MavlinkId,
        ident: Ident,
        requested: Value,
        result: Result<Value>,
        ctx: &mut Context,
    ) -> Option<()> {
        let system = SystemId::from(address.system);
        let component = address.component;
        let vehicle = ctx.fleet.vehicle_mut(system)?;

        // A result only updates state it already owns; if the component
        // was never heard from, it is a no-op.
        vehicle.parameters(component)?;

        let param = vehicle.parameters_mut(component).get_mut(&ident)?;

        if !matches!(param.state(), ParamState::Uploading { value } if *value == requested) {
            return None;
        }

        self.uploads.remove(&(address, ident.clone()));

        match result {
            Ok(value) if value == requested => param.upload_succeeded(value),
            Ok(value) => {
                log::warn!(
                    "Parameter '{}' was set to {value:?} instead of {requested:?}",
                    ident.as_str()
                );
                param.upload_rejected(value, requested);
            }
            Err(error) => {
                log::warn!("Parameter upload for '{}' failed: {error}", ident.as_str());
                param.upload_failed(requested);
            }
        }
        Some(())
    }

    /// Remember which component's parameters the panel should show.
    fn select_component(&mut self, address: MavlinkId) {
        self.component
            .insert(SystemId::from(address.system), address.component);
    }

    /// Open a file dialog to pick a parameter save path.
    ///
    /// The component's parameter set is snapshotted here, when the dialog is
    /// opened, rather than in `view`.
    fn save_dialog(
        &mut self,
        address: MavlinkId,
        ctx: &mut Context,
    ) -> (Task<Message>, Option<Event>) {
        let Some(parameters) = ctx
            .fleet
            .vehicle(SystemId::from(address.system))
            .and_then(|vehicle| vehicle.parameters(address.component))
            .cloned()
        else {
            return (Task::none(), None);
        };

        let file_dialog = file_dialog(ctx.file_picker_path);

        (
            Task::future(
                async move { file_dialog.save_file().await.map(|file| (file, parameters)) },
            )
            .and_then(|(file, parameters)| {
                Task::done(Message::SaveToFile(file.path().to_owned(), parameters))
            }),
            None,
        )
    }

    /// Write a parameter set to disk off the UI thread.
    fn save_to_file(
        &mut self,
        save_path: PathBuf,
        parameters: Parameters,
    ) -> (Task<Message>, Option<Event>) {
        log::info!("Saving parameters to file: {save_path:?}");

        let path = save_path.clone();
        (
            iced_runtime::task::blocking(move |mut sender| {
                let result =
                    std::fs::write(&path, parameter::parameters_to_ini_string(&parameters))
                        .map_err(|error| error.to_string());
                let _ = sender.try_send(Message::SaveFinished(save_path, result));
            }),
            None,
        )
    }

    /// Log a save failure and remember the chosen directory.
    fn save_finished(
        &mut self,
        save_path: PathBuf,
        result: Result<(), String>,
    ) -> (Task<Message>, Option<Event>) {
        if let Err(error) = result {
            log::error!("Error saving to file: {error}");
        }

        (Task::none(), Some(Event::SetFilePickerPath(save_path)))
    }

    /// Open a file dialog to pick a parameter file to load.
    fn load_dialog(
        &mut self,
        address: MavlinkId,
        ctx: &mut Context,
    ) -> (Task<Message>, Option<Event>) {
        let file_dialog = file_dialog(ctx.file_picker_path);

        (
            Task::future(async move { file_dialog.pick_file().await.map(|file| (file, address)) })
                .and_then(|(file, address)| {
                    Task::done(Message::LoadFromFile(file.path().to_owned(), address))
                }),
            None,
        )
    }

    /// Read and parse a parameter file off the UI thread.
    fn load_from_file(
        &mut self,
        load_path: PathBuf,
        address: MavlinkId,
    ) -> (Task<Message>, Option<Event>) {
        log::info!("Loading parameters from file: {load_path:?}");

        let path = load_path.clone();
        (
            iced_runtime::task::blocking(move |mut sender| {
                let result = std::fs::read_to_string(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| {
                        parameter::parameters_from_ini_str(&text).map_err(|error| error.to_string())
                    });
                let _ = sender.try_send(Message::LoadFinished(load_path, address, result));
            }),
            None,
        )
    }

    /// Apply loaded values as pending edits on the addressed component.
    fn load_finished(
        &mut self,
        load_path: PathBuf,
        address: MavlinkId,
        loaded_params: Result<Parameters, String>,
        ctx: &mut Context,
    ) -> Option<(Task<Message>, Option<Event>)> {
        let loaded_params = match loaded_params {
            Ok(loaded_params) => loaded_params,
            Err(error) => {
                log::error!("Could not load parameter file: {error}");
                return None;
            }
        };

        let system = SystemId::from(address.system);
        let component = address.component;
        let vehicle = ctx.fleet.vehicle_mut(system)?;

        // Mark the loaded values as pending edits on the addressed
        // component, filling the buffers so the new values are shown.
        let editing = self.editing.entry(address).or_default();
        for (ident, new_param) in loaded_params.iter() {
            let Some(current) = vehicle.parameters(component).and_then(|p| p.get(ident)) else {
                continue;
            };
            let current_value = current.value();
            if !value_type_matches(current_value, new_param.value())
                || current_value == new_param.value()
            {
                continue;
            }

            if let Some(old_param) = vehicle.parameters_mut(component).get_mut(ident) {
                old_param.mark_changed(new_param.value());
                editing.insert(ident.clone(), value_as_string(new_param.value()));
            }
        }

        Some((Task::none(), Some(Event::SetFilePickerPath(load_path))))
    }

    /// Render the parameter panel. A pure function of panel state: whether a
    /// list download is in flight comes from [`Self::reloading`], not the router.
    pub fn view<'a>(
        &'a self,
        vehicles: &'a BTreeMap<SystemId, Vehicle>,
        primary: Option<SystemId>,
        theme: &Theme,
    ) -> Element<'a, Message> {
        let Some(system) = primary else {
            return space::Space::new().boxed();
        };

        let Some(vehicle) = vehicles.get(&system) else {
            return space::Space::new().boxed();
        };

        let component = self.selected_component(vehicle);
        let address = vehicle.target_for(component);
        let parameters = vehicle.parameters(component);

        let mut entries: Vec<Element<'_, Message>> = Vec::with_capacity(128);

        // Which component's parameters are shown; the autopilot is marked.
        let component_ids = vehicle.component_ids();
        if !component_ids.is_empty() {
            let autopilot = vehicle.autopilot_component();
            let picker = pick_list(Some(component), component_ids, move |id: &u8| {
                if Some(*id) == autopilot {
                    format!("comp {id} (autopilot)")
                } else {
                    format!("comp {id}")
                }
            })
            .on_select(move |id| {
                Message::SelectComponent(MavlinkId {
                    system: system.0,
                    component: id,
                })
            });

            entries.push(picker.boxed());
        }

        // Disabled while a parameter list download is in progress
        let reloading = self.reloading.contains(&address);
        let reload_button = Button::new("Reload parameters")
            .width(Length::Fill)
            .on_press_maybe((!reloading).then(|| Message::ListReload(address)));

        let upload_button = Button::new("Upload parameters")
            .width(Length::FillPortion(1))
            .on_press(Message::UploadAll(address));

        let save_button = Button::new("Save parameters to file")
            .width(Length::FillPortion(1))
            .on_press(Message::SaveDialog(address));

        let load_button = Button::new("Load parameters from file")
            .width(Length::FillPortion(1))
            .on_press(Message::LoadDialog(address));

        let filter_field = TextInput::new("Filter parameters", &self.filter.ident_pattern)
            .width(Length::FillPortion(1))
            .on_input(Message::FilterIdentPattern);

        entries.push(
            iced::widget::row![reload_button, upload_button]
                .spacing(5.0)
                .boxed(),
        );
        entries.push(
            iced::widget::row![save_button, load_button]
                .spacing(5.0)
                .boxed(),
        );

        entries.push(filter_field.boxed());

        entries.push(
            iced::widget::checkbox(self.filter.only_changed)
                .label("Only changed")
                .on_toggle(Message::FilterOnlyChanged)
                .boxed(),
        );

        let (got, exp) = parameters.map_or((0, 0), |parameters| {
            (
                parameters.loading().loaded(),
                parameters.loading().expected(),
            )
        });

        let style = if got < exp as usize {
            progress_bar::primary
        } else {
            progress_bar::success
        };

        let progress = ProgressBar::new((0.0)..=(exp as f32), got as f32)
            .style(style)
            .girth(10.0)
            .boxed();

        entries.push(progress);

        container(
            iced::widget::column![
                iced::widget::Column::from_iter(entries).spacing(5.0),
                self.view_param_list(address, parameters, theme),
            ]
            .spacing(5.0),
        )
        .padding(8.0)
        .width(Length::Fill)
        .boxed()
    }

    fn view_param_list<'a>(
        &'a self,
        address: MavlinkId,
        parameters: Option<&'a Parameters>,
        theme: &Theme,
    ) -> Element<'a, Message> {
        /// Heights must fit the tallest content of each item kind
        const ROW_HEIGHT: f32 = 32.0;
        const HEADER_HEIGHT: f32 = 40.0;
        const SEPARATOR_HEIGHT: f32 = 1.0;

        enum Item<'a> {
            /// `first` is the ident following the section, used as its key
            Section {
                name: Option<&'a str>,
                separator: bool,
                first: &'a Ident,
            },
            Param(&'a Ident, &'a Parameter),
        }

        let Some(parameters) = parameters else {
            return space::Space::new().boxed();
        };

        let pattern = self.filter.ident_pattern.to_lowercase();
        let editing = &self.editing;
        // Copied up front so the draw closure does not borrow the theme.
        let type_color = theme.palette().secondary.base.color;

        let mut items = Vec::with_capacity(parameters.len() + 64);
        let mut section = None;

        for (ident, param) in parameters.iter() {
            if !self.filter.matches(ident, param, &pattern) {
                continue;
            }

            let this_section = ident
                .as_str()
                .split_once(self.namespace_sep)
                .map(|(sec, _)| sec);

            // Add larger section headers and separators
            if this_section != section {
                let separator = section.is_some();
                if separator || this_section.is_some() {
                    items.push(Item::Section {
                        name: this_section,
                        separator,
                        first: ident,
                    });
                }
                section = this_section;
            }

            items.push(Item::Param(ident, param));
        }

        let list = virtual_list(items, move |item| match item {
            Item::Section {
                name, separator, ..
            } => {
                let header = name.map(|name| -> Text<'_, iced::Theme> {
                    Text::new(format!("[{name}]"))
                        .size(24.0)
                        .font(Font::MONOSPACE)
                });
                Column::<Element<'_, Message>>::new()
                    .push(separator.then(|| rule::horizontal(1.0)).boxed())
                    .push(header.boxed())
                    .spacing(5.0)
                    .boxed()
            }
            Item::Param(ident, param) => {
                let buffer = editing
                    .get(&address)
                    .and_then(|entry| entry.get(*ident))
                    .map(String::as_str);
                view_param_row(address, ident, param, buffer, type_color).boxed()
            }
        })
        .item_height(|item| match item {
            Item::Section { name: None, .. } => SEPARATOR_HEIGHT,
            Item::Section { .. } => HEADER_HEIGHT,
            Item::Param(..) => ROW_HEIGHT,
        })
        .key(move |item| match item {
            Item::Section { first, .. } => ((address, (*first).clone()), true),
            Item::Param(ident, _) => ((address, (*ident).clone()), false),
        })
        .spacing(5.0);

        iced::widget::scrollable(list)
            .style(iced::widget::scrollable::default)
            .spacing(5.0)
            .boxed()
    }
}

fn view_param_row<'a>(
    address: MavlinkId,
    ident: &'a Ident,
    param: &'a Parameter,
    editing: Option<&'a str>,
    type_color: Color,
) -> Element<'a, Message> {
    let type_name = value_type_name(param.value());

    let value_string = editing
        .map(str::to_owned)
        .unwrap_or_else(|| value_as_string(param.value()));

    let text_input = iced::widget::TextInput::new("Write value", value_string)
        .on_input(move |string| Message::BufferEdit(address, ident.clone(), string))
        .style(move |theme, status| {
            let mut style = iced::widget::text_input::default(theme, status);
            let palette = theme.palette();
            match param.state() {
                ParamState::Unchanged => {}
                ParamState::Changed { status, .. } => {
                    style.background = match status {
                        ChangedStatus::None => palette.success.base.color,
                        ChangedStatus::Failed => palette.warning.base.color,
                        ChangedStatus::Rejected => palette.danger.base.color,
                    }
                    .into();
                }
                ParamState::Uploading { .. } => {
                    style.background = palette.secondary.base.color.into();
                }
            }
            style
        })
        .width(Length::Fill);

    let restore_button = Button::new("Restore")
        .on_press_maybe(match param.state() {
            ParamState::Changed { .. } => Some(Message::ValueReset(address, ident.clone())),
            _ => None,
        })
        .width(80.0);

    let restore_tooltip = container(Text::new(match param.state() {
        ParamState::Changed { .. } => format!("Restore to: {}", value_as_string(param.value())),
        _ => "Nothing to restore".to_string(),
    }))
    .padding(10.0)
    .style(style::tooltip);

    let restore_button = tooltip(restore_button, restore_tooltip, Default::default())
        .delay(std::time::Duration::from_millis(250));

    let upload_button = Button::new("Upload")
        .on_press_maybe(match param.state() {
            ParamState::Changed { value, .. } => {
                Some(Message::ValueUpload(address, ident.clone(), *value))
            }
            _ => None,
        })
        .width(80.0);

    row![
        Text::new(ident.as_str().to_string())
            .width(160.0)
            .font(Font::MONOSPACE),
        Text::new(type_name)
            .width(50.0)
            .font(Font::MONOSPACE)
            .align_x(Alignment::End)
            .color(type_color),
        Space::new().width(10.0),
        text_input,
        restore_button,
        upload_button,
    ]
    .spacing(5.0)
    .align_y(Vertical::Center)
    .boxed()
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
    use crate::foundation::{DEFAULT_GCS_ID, MavlinkId};
    use crate::services::{MavError, MavlinkRouter};

    const A: SystemId = SystemId(1);
    const B: SystemId = SystemId(2);

    fn ident(name: &str) -> Ident {
        Ident::from_str_truncated(name)
    }

    /// The `(system, component 1)` address used throughout these tests.
    fn addr(id: SystemId) -> MavlinkId {
        MavlinkId {
            system: id.0,
            component: 1,
        }
    }

    /// An address for a specific component of `id`'s system.
    fn addr_c(id: SystemId, component: u8) -> MavlinkId {
        MavlinkId {
            system: id.0,
            component,
        }
    }

    /// Register `id` and seed its autopilot (component 1) with `params`.
    fn add_vehicle(fleet: &mut Fleet, id: SystemId, params: &[(&str, Value)]) {
        fleet.observe(addr(id), std::time::Instant::now(), None);
        let vehicle = fleet.vehicle_mut(id).unwrap();
        for (name, value) in params {
            vehicle.parameters_mut(1).insert(ident(name), *value);
        }
    }

    fn param<'a>(fleet: &'a Fleet, id: SystemId, key: &Ident) -> &'a Parameter {
        param_c(fleet, id, 1, key)
    }

    fn param_c<'a>(fleet: &'a Fleet, id: SystemId, component: u8, key: &Ident) -> &'a Parameter {
        fleet
            .vehicle(id)
            .unwrap()
            .parameters(component)
            .unwrap()
            .get(key)
            .unwrap()
    }

    /// Run one message against the panel with a fresh context, returning any event.
    fn step(
        panel: &mut ParameterPanel,
        router: &MavlinkRouter,
        fleet: &mut Fleet,
        message: Message,
    ) -> Option<Event> {
        let services = FleetServices::new(router);
        let mut ctx = Context {
            services: &services,
            fleet,
            file_picker_path: None,
        };
        panel.update(message, &mut ctx).1
    }

    fn changed_state(param: &Parameter) -> Option<&ChangedStatus> {
        match param.state() {
            ParamState::Changed { status, .. } => Some(status),
            _ => None,
        }
    }

    #[test]
    fn filter_matches_by_substring_and_only_changed() {
        let unchanged = Parameter::new(Value::F32(1.0));
        let mut changed = Parameter::new(Value::F32(1.0));
        changed.set_edited(Some(Value::F32(2.0)));

        let all = ParameterFilter::default();
        assert!(all.matches(&ident("BAT_CAPACITY"), &unchanged, ""));
        assert!(all.matches(&ident("BAT_CAPACITY"), &unchanged, "bat"));
        assert!(!all.matches(&ident("BAT_CAPACITY"), &unchanged, "gps"));

        let only_changed = ParameterFilter {
            only_changed: true,
            ..ParameterFilter::default()
        };
        assert!(!only_changed.matches(&ident("BAT_CAPACITY"), &unchanged, ""));
        assert!(only_changed.matches(&ident("BAT_CAPACITY"), &changed, ""));
    }

    #[test]
    fn filter_matches_case_insensitively() {
        let filter = ParameterFilter::default();
        let param = Parameter::new(Value::F32(1.0));

        assert!(filter.matches(&ident("BAT_CAPACITY"), &param, "bat"));
        assert!(filter.matches(&ident("BAT_CAPACITY"), &param, "capacity"));
        assert!(filter.matches(&ident("BAT_CAPACITY"), &param, ""));
        assert!(!filter.matches(&ident("BAT_CAPACITY"), &param, "gps"));
        assert!(!filter.matches(&ident("BAT_CAPACITY"), &param, "capacityx"));
    }

    #[test]
    fn contains_ignore_ascii_case_handles_edges() {
        assert!(contains_ignore_ascii_case("BAT_CAPACITY", "bat"));
        assert!(contains_ignore_ascii_case("BAT_CAPACITY", "BAT"));
        assert!(contains_ignore_ascii_case("BAT_CAPACITY", ""));
        assert!(!contains_ignore_ascii_case("BAT_CAPACITY", "gps"));
        assert!(!contains_ignore_ascii_case("", "gps"));
    }

    #[test]
    fn buffer_edit_marks_changed_and_reset_reverts() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();
        let key = ident("PARAM_ONE");

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::BufferEdit(addr(A), key.clone(), "2.0".to_owned()),
        );

        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Changed { .. }
        ));
        assert_eq!(panel.editing[&addr(A)][&key], "2.0");

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueReset(addr(A), key.clone()),
        );

        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Unchanged
        ));
        assert!(!panel.editing[&addr(A)].contains_key(&key));
    }

    #[test]
    fn upload_result_transitions_state() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();
        let key = ident("PARAM_ONE");

        // Start an upload; it is recorded but not driven (no link is attached).
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUpload(addr(A), key.clone(), Value::F32(2.0)),
        );
        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Uploading { .. }
        ));
        assert!(panel.uploads.contains_key(&(addr(A), key.clone())));

        // Echoed exactly: accepted.
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUploadResult(addr(A), key.clone(), Value::F32(2.0), Ok(Value::F32(2.0))),
        );
        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Unchanged
        ));
        assert_eq!(param(&fleet, A, &key).value(), Value::F32(2.0));
        assert!(!panel.uploads.contains_key(&(addr(A), key.clone())));

        // Clamped by the target: rejected, keeping the requested value.
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUpload(addr(A), key.clone(), Value::F32(5.0)),
        );
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUploadResult(addr(A), key.clone(), Value::F32(5.0), Ok(Value::F32(3.0))),
        );
        assert_eq!(
            changed_state(param(&fleet, A, &key)),
            Some(&ChangedStatus::Rejected)
        );

        // Transport error: failed.
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUpload(addr(A), key.clone(), Value::F32(7.0)),
        );
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUploadResult(
                addr(A),
                key.clone(),
                Value::F32(7.0),
                Err(MavError::Timeout),
            ),
        );
        assert_eq!(
            changed_state(param(&fleet, A, &key)),
            Some(&ChangedStatus::Failed)
        );
    }

    #[test]
    fn stale_upload_result_is_ignored() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();
        let key = ident("PARAM_ONE");

        // A result arriving while nothing is uploading must not change the value.
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUploadResult(addr(A), key.clone(), Value::F32(9.0), Ok(Value::F32(9.0))),
        );

        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Unchanged
        ));
        assert_eq!(param(&fleet, A, &key).value(), Value::F32(1.0));
    }

    #[test]
    fn list_reload_clears_loaded_markers() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        fleet
            .vehicle_mut(A)
            .unwrap()
            .parameters_mut(1)
            .note_loaded_index(0, 5);
        let mut panel = ParameterPanel::new();

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ListReload(addr(A)),
        );

        assert_eq!(
            fleet
                .vehicle(A)
                .unwrap()
                .parameters(1)
                .unwrap()
                .loading()
                .loaded(),
            0
        );
    }

    #[test]
    fn load_finished_marks_changed_and_fills_buffer() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();
        let key = ident("PARAM_ONE");

        let mut loaded = Parameters::default();
        loaded.insert(key.clone(), Value::F32(3.0));

        let event = step(
            &mut panel,
            &router,
            &mut fleet,
            Message::LoadFinished(PathBuf::from("/tmp/params.ini"), addr(A), Ok(loaded)),
        );

        assert!(matches!(event, Some(Event::SetFilePickerPath(_))));
        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Changed { value: Value::F32(v), .. } if *v == 3.0
        ));
        assert_eq!(panel.editing[&addr(A)][&key], "3");
    }

    #[test]
    fn editing_buffers_are_kept_per_component() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        fleet
            .vehicle_mut(A)
            .unwrap()
            .parameters_mut(154)
            .insert(ident("PARAM_ONE"), Value::F32(5.0));
        let mut panel = ParameterPanel::new();
        let key = ident("PARAM_ONE");

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::BufferEdit(addr_c(A, 1), key.clone(), "2.0".to_owned()),
        );

        // The autopilot's edit is isolated from the gimbal's set.
        assert!(matches!(
            param_c(&fleet, A, 1, &key).state(),
            ParamState::Changed { .. }
        ));
        assert!(matches!(
            param_c(&fleet, A, 154, &key).state(),
            ParamState::Unchanged
        ));
        assert_eq!(panel.editing[&addr_c(A, 1)][&key], "2.0");
        assert!(!panel.editing.contains_key(&addr_c(A, 154)));
    }

    #[test]
    fn editing_buffers_are_kept_per_vehicle() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        add_vehicle(&mut fleet, B, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();
        let key = ident("PARAM_ONE");

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::BufferEdit(addr(A), key.clone(), "42".to_owned()),
        );

        // The second vehicle's buffer is untouched.
        assert!(
            !panel
                .editing
                .get(&addr(B))
                .is_some_and(|entry| entry.contains_key(&key))
        );
        assert_eq!(panel.editing[&addr(A)][&key], "42");
    }

    #[test]
    fn list_reload_tracks_in_flight_download() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ListReload(addr(A)),
        );
        assert!(panel.reloading.contains(&addr(A)));
    }

    #[test]
    fn list_reload_started_tracks_in_flight_download() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ListReloadStarted(addr(A)),
        );
        assert!(panel.reloading.contains(&addr(A)));
    }

    #[test]
    fn list_reload_result_clears_in_flight_download() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();

        // A successful download clears the flag.
        panel.reloading.insert(addr(A));
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ListReloadResult(
                addr(A),
                Ok(ParamListSummary {
                    received: 1,
                    expected: 1,
                }),
            ),
        );
        assert!(!panel.reloading.contains(&addr(A)));

        // A failed download clears it too.
        panel.reloading.insert(addr(A));
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ListReloadResult(addr(A), Err(MavError::Timeout)),
        );
        assert!(!panel.reloading.contains(&addr(A)));
    }

    #[test]
    fn load_finished_targets_the_address_carried_in_the_message() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        add_vehicle(&mut fleet, B, &[]);
        fleet
            .vehicle_mut(B)
            .unwrap()
            .parameters_mut(154)
            .insert(ident("PARAM_ONE"), Value::F32(1.0));
        let mut panel = ParameterPanel::new();
        let key = ident("PARAM_ONE");

        let mut loaded = Parameters::default();
        loaded.insert(key.clone(), Value::F32(3.0));

        let event = step(
            &mut panel,
            &router,
            &mut fleet,
            Message::LoadFinished(PathBuf::from("/tmp/params.ini"), addr_c(B, 154), Ok(loaded)),
        );

        assert!(matches!(event, Some(Event::SetFilePickerPath(_))));
        // The loaded values land on `B`'s component 154...
        assert!(matches!(
            param_c(&fleet, B, 154, &key).state(),
            ParamState::Changed { value: Value::F32(v), .. } if *v == 3.0
        ));
        assert_eq!(panel.editing[&addr_c(B, 154)][&key], "3");

        // ...and leave the primary `A` untouched.
        assert!(matches!(
            param_c(&fleet, A, 1, &key).state(),
            ParamState::Unchanged
        ));
    }

    #[test]
    fn value_upload_result_targets_the_address_carried_in_the_message() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[]);
        fleet
            .vehicle_mut(A)
            .unwrap()
            .parameters_mut(154)
            .insert(ident("PARAM_ONE"), Value::F32(1.0));
        let mut panel = ParameterPanel::new();
        let key = ident("PARAM_ONE");
        let address = addr_c(A, 154);

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUpload(address, key.clone(), Value::F32(2.0)),
        );
        assert!(panel.uploads.contains_key(&(address, key.clone())));

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUploadResult(address, key.clone(), Value::F32(2.0), Ok(Value::F32(2.0))),
        );
        assert!(matches!(
            param_c(&fleet, A, 154, &key).state(),
            ParamState::Unchanged
        ));
        assert_eq!(param_c(&fleet, A, 154, &key).value(), Value::F32(2.0));
        assert!(!panel.uploads.contains_key(&(address, key.clone())));
    }

    #[test]
    fn component_selection_defaults_to_autopilot_and_tracks_selection() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        fleet
            .vehicle_mut(A)
            .unwrap()
            .parameters_mut(154)
            .insert(ident("GIMBAL_X"), Value::F32(1.0));
        let mut panel = ParameterPanel::new();

        // No explicit selection: falls back to the autopilot (component 1).
        assert_eq!(panel.selected_component(fleet.vehicle(A).unwrap()), 1);

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::SelectComponent(addr_c(A, 154)),
        );
        assert_eq!(panel.selected_component(fleet.vehicle(A).unwrap()), 154);
    }
}
