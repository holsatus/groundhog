//! The parameter feature: browsing, filtering, editing, uploading, and
//! saving/loading a vehicle's parameter set.
//!
//! Owns the filter and the per-parameter edit buffers. Parameter data itself
//! lives on the [`Fleet`] (the single owner of vehicle state), which the root
//! passes in by mutable reference; the router is passed in for transactions.
//! Writing the remembered file-picker directory back to the configuration is
//! requested through [`Event`].
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
        Button, Column, ProgressBar, Space, Text, TextInput, container, progress_bar, row, rule,
        space, tooltip,
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
    foundation::MavlinkId,
    mav::{self, MavlinkRouter},
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
    ListReloadResult(MavlinkId, mav::Result<mav::ParamListSummary>),

    /// Upload all modified parameters to the target
    UploadAll(MavlinkId),

    /// Reset a local parameter to its target-default
    ValueReset(MavlinkId, Ident),

    /// Upload a local parameter to the target
    ValueUpload(MavlinkId, Ident, Value),

    /// The result of uploading a local parameter to the target
    ValueUploadResult(MavlinkId, Ident, Value, mav::Result<Value>),

    /// Edit the text-value buffer of the local parameter
    BufferEdit(MavlinkId, Ident, String),

    /// Open a file-picker dialog to choose a save location
    SaveDialog(Parameters),

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

pub struct ParameterPanel {
    filter: ParameterFilter,
    namespace_sep: char,
    /// Text currently typed into each parameter's value field, keyed by the
    /// parameter it belongs to. Kept here (not on the domain `Parameter`) so
    /// that switching vehicles and back restores the in-progress edit.
    editing: BTreeMap<MavlinkId, BTreeMap<Ident, String>>,
    /// Abort handles for in-flight uploads, dropped (aborting) when an upload
    /// is superseded by an edit, a reset, or its own completion.
    uploads: BTreeMap<(MavlinkId, Ident), iced::task::Handle>,
    /// Vehicles with a parameter list download in flight, so the view can
    /// disable the reload button without querying the router.
    reloading: BTreeSet<MavlinkId>,
}

impl ParameterPanel {
    pub fn new() -> Self {
        Self {
            filter: ParameterFilter::default(),
            namespace_sep: '_',
            editing: BTreeMap::new(),
            uploads: BTreeMap::new(),
            reloading: BTreeSet::new(),
        }
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
            Message::FilterIdentPattern(buffer) => {
                self.filter.ident_pattern = buffer;
            }
            Message::FilterOnlyChanged(changed) => {
                self.filter.only_changed = changed;
            }
            Message::ListReload(mav_id) => {
                let mav = ctx.router.clone();
                let vehicle = ctx.fleet.vehicle_mut(mav_id)?;

                let request_capabilities = vehicle.identity.capabilities.is_none();
                vehicle.parameters_mut().reset_loading();
                self.reloading.insert(mav_id);

                let task = Task::perform(
                    async move { mav.param_list(mav_id, request_capabilities).await },
                    move |result| Message::ListReloadResult(mav_id, result),
                );

                return Some((task, None));
            }
            Message::ListReloadStarted(mav_id) => {
                self.reloading.insert(mav_id);
            }
            Message::ListReloadResult(mav_id, result) => {
                self.reloading.remove(&mav_id);
                match result {
                    Ok(summary) => log::info!("Parameters from {mav_id:?}: {summary:?}"),
                    Err(error) => log::warn!("Parameter download from {mav_id:?} failed: {error}"),
                }
            }
            Message::UploadAll(mav_id) => {
                let vehicle = ctx.fleet.vehicle(mav_id)?;

                // The number of concurrent uploads is limited by the middleware
                let uploads: Vec<_> = vehicle
                    .parameters()
                    .iter()
                    .filter_map(|(ident, param)| match param.state() {
                        ParamState::Changed { value, .. } => Some(Task::done(
                            Message::ValueUpload(mav_id, ident.clone(), *value),
                        )),
                        _ => None,
                    })
                    .collect();

                log::info!("Uploading {} changed parameters", uploads.len());
                return Some((Task::batch(uploads), None));
            }
            Message::BufferEdit(mav_id, ident, buffer) => {
                // Editing supersedes any upload in flight for this parameter.
                self.uploads.remove(&(mav_id, ident.clone()));

                {
                    let entry = ctx.fleet.vehicle_mut(mav_id)?;
                    let param = entry.parameters_mut().get_mut(&ident)?;

                    param.set_edited(value_parse_as(param.value(), &buffer));
                }

                self.editing
                    .entry(mav_id)
                    .or_default()
                    .insert(ident, buffer);
            }
            Message::ValueReset(mav_id, ident) => {
                self.uploads.remove(&(mav_id, ident.clone()));

                {
                    let entry = ctx.fleet.vehicle_mut(mav_id)?;
                    let param = entry.parameters_mut().get_mut(&ident)?;

                    param.reset();
                }

                if let Some(editing) = self.editing.get_mut(&mav_id) {
                    editing.remove(&ident);
                }
            }
            Message::ValueUpload(mav_id, ident, value) => {
                let mav = ctx.router.clone();
                let vehicle = ctx.fleet.vehicle_mut(mav_id)?;
                let cap = vehicle.identity.capabilities.unwrap_or_default();
                let param = vehicle.parameters_mut().get_mut(&ident)?;

                let ident_cloned = ident.clone();
                let ident_for_result = ident.clone();
                let (task, handle) = Task::perform(
                    async move { mav.param_set(mav_id, ident_cloned, value, cap).await },
                    move |result| {
                        Message::ValueUploadResult(mav_id, ident_for_result.clone(), value, result)
                    },
                )
                .abortable();

                // Holding the abort handle lets an edit or reset cancel the
                // upload by dropping it; completion drops it too.
                self.uploads
                    .insert((mav_id, ident.clone()), handle.abort_on_drop());
                param.begin_upload(value);

                return Some((task, None));
            }
            Message::ValueUploadResult(mav_id, ident, requested, result) => {
                let vehicle = ctx.fleet.vehicle_mut(mav_id)?;
                let param = vehicle.parameters_mut().get_mut(&ident)?;

                if !matches!(param.state(), ParamState::Uploading { value } if *value == requested)
                {
                    return None;
                }

                self.uploads.remove(&(mav_id, ident.clone()));

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
            }
            Message::SaveDialog(parameters) => {
                let file_dialog = file_dialog(ctx.file_picker_path);

                return Some((
                    Task::future(async move {
                        file_dialog.save_file().await.map(|file| (file, parameters))
                    })
                    .and_then(|(file, parameters)| {
                        Task::done(Message::SaveToFile(file.path().to_owned(), parameters))
                    }),
                    None,
                ));
            }
            Message::SaveToFile(save_path, parameters) => {
                log::info!("Saving parameters to file: {save_path:?}");

                let path = save_path.clone();
                return Some((
                    iced_runtime::task::blocking(move |mut sender| {
                        let result = parameter::save_parameters_to_ini(&path, parameters)
                            .map_err(|error| error.to_string());
                        let _ = sender.try_send(Message::SaveFinished(save_path, result));
                    }),
                    None,
                ));
            }
            Message::SaveFinished(save_path, result) => {
                if let Err(error) = result {
                    log::error!("Error saving to file: {error}");
                }

                return Some((Task::none(), Some(Event::SetFilePickerPath(save_path))));
            }
            Message::LoadDialog(mav_id) => {
                let file_dialog = file_dialog(ctx.file_picker_path);

                return Some((
                    Task::future(async move {
                        file_dialog.pick_file().await.map(|file| (file, mav_id))
                    })
                    .and_then(|(file, mav_id)| {
                        Task::done(Message::LoadFromFile(file.path().to_owned(), mav_id))
                    }),
                    None,
                ));
            }
            Message::LoadFromFile(load_path, mav_id) => {
                log::info!("Loading parameters from file: {load_path:?}");

                let path = load_path.clone();
                return Some((
                    iced_runtime::task::blocking(move |mut sender| {
                        let result = parameter::load_parameters_from_ini(&path)
                            .map_err(|error| error.to_string());
                        let _ = sender.try_send(Message::LoadFinished(load_path, mav_id, result));
                    }),
                    None,
                ));
            }
            Message::LoadFinished(load_path, mav_id, loaded_params) => {
                let loaded_params = match loaded_params {
                    Ok(loaded_params) => loaded_params,
                    Err(error) => {
                        log::error!("Could not load parameter file: {error}");
                        return None;
                    }
                };

                let vehicle = ctx.fleet.vehicle_mut(mav_id)?;
                let editing = self.editing.entry(mav_id).or_default();

                // Set the parameters of the vehicle as modified if that is the case
                for (ident, new_param) in loaded_params.iter() {
                    let Some(current) = vehicle.parameters().get(ident) else {
                        continue;
                    };
                    if !value_type_matches(current.value(), new_param.value())
                        || current.value() == new_param.value()
                    {
                        continue;
                    }

                    if let Some(old_param) = vehicle.parameters_mut().get_mut(ident) {
                        old_param.mark_changed(new_param.value());
                        editing.insert(ident.clone(), value_as_string(new_param.value()));
                    }
                }

                return Some((Task::none(), Some(Event::SetFilePickerPath(load_path))));
            }
        }

        None
    }

    /// Render the parameter panel. A pure function of panel state: whether a
    /// list download is in flight comes from [`Self::reloading`], not the router.
    pub fn view<'a>(
        &'a self,
        vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
        primary: Option<MavlinkId>,
        theme: &Theme,
    ) -> Element<'a, Message> {
        let Some(mav_id) = primary else {
            return space::Space::new().boxed();
        };

        let Some(vehicle) = vehicles.get(&mav_id) else {
            return space::Space::new().boxed();
        };

        let mut entries: Vec<Element<'_, Message>> = Vec::with_capacity(128);

        // Disabled while a parameter list download is in progress
        let reloading = self.reloading.contains(&mav_id);
        let reload_button = Button::new("Reload parameters")
            .width(Length::Fill)
            .on_press_maybe((!reloading).then(|| Message::ListReload(mav_id)));

        let upload_button = Button::new("Upload parameters")
            .width(Length::FillPortion(1))
            .on_press(Message::UploadAll(mav_id));

        let save_button = Button::new("Save parameters to file")
            .width(Length::FillPortion(1))
            .on_press_with(|| Message::SaveDialog(vehicle.parameters().clone()));

        let load_button = Button::new("Load parameters from file")
            .width(Length::FillPortion(1))
            .on_press(Message::LoadDialog(mav_id));

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

        let got = vehicle.parameters().loading().loaded();
        let exp = vehicle.parameters().loading().expected();

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
                self.view_param_list(mav_id, vehicle, theme),
            ]
            .spacing(5.0),
        )
        .padding(8.0)
        .width(Length::Fill)
        .boxed()
    }

    fn view_param_list<'a>(
        &'a self,
        mav_id: MavlinkId,
        vehicle: &'a Vehicle,
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

        let pattern = self.filter.ident_pattern.to_lowercase();
        let editing = &self.editing;
        // Copied up front so the draw closure does not borrow the theme.
        let type_color = theme.palette().secondary.base.color;

        let mut items = Vec::with_capacity(vehicle.parameters().len() + 64);
        let mut section = None;

        for (ident, param) in vehicle.parameters().iter() {
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
                    .get(&mav_id)
                    .and_then(|entry| entry.get(*ident))
                    .map(String::as_str);
                view_param_row(mav_id, ident, param, buffer, type_color).boxed()
            }
        })
        .item_height(|item| match item {
            Item::Section { name: None, .. } => SEPARATOR_HEIGHT,
            Item::Section { .. } => HEADER_HEIGHT,
            Item::Param(..) => ROW_HEIGHT,
        })
        .key(move |item| match item {
            Item::Section { first, .. } => ((mav_id, (*first).clone()), true),
            Item::Param(ident, _) => ((mav_id, (*ident).clone()), false),
        })
        .spacing(5.0);

        iced::widget::scrollable(list)
            .style(iced::widget::scrollable::default)
            .spacing(5.0)
            .boxed()
    }
}

fn view_param_row<'a>(
    mav_id: MavlinkId,
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
        .on_input(move |string| Message::BufferEdit(mav_id, ident.clone(), string))
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
            ParamState::Changed { .. } => Some(Message::ValueReset(mav_id, ident.clone())),
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
                Some(Message::ValueUpload(mav_id, ident.clone(), *value))
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
    use crate::foundation::DEFAULT_GCS_ID;

    const A: MavlinkId = MavlinkId {
        system: 1,
        component: 1,
    };
    const B: MavlinkId = MavlinkId {
        system: 2,
        component: 1,
    };

    fn ident(name: &str) -> Ident {
        Ident::from_str_truncated(name)
    }

    /// Register `id` and seed it with `params`, using only the public domain API.
    fn add_vehicle(fleet: &mut Fleet, id: MavlinkId, params: &[(&str, Value)]) {
        fleet.observe(id, std::time::Instant::now(), None);
        let vehicle = fleet.vehicle_mut(id).unwrap();
        for (name, value) in params {
            vehicle.parameters_mut().insert(ident(name), *value);
        }
    }

    fn param<'a>(fleet: &'a Fleet, id: MavlinkId, key: &Ident) -> &'a Parameter {
        fleet.vehicle(id).unwrap().parameters().get(key).unwrap()
    }

    /// Run one message against the panel with a fresh context, returning any event.
    fn step(
        panel: &mut ParameterPanel,
        router: &MavlinkRouter,
        fleet: &mut Fleet,
        message: Message,
    ) -> Option<Event> {
        let mut ctx = Context {
            router,
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
            Message::BufferEdit(A, key.clone(), "2.0".to_owned()),
        );

        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Changed { .. }
        ));
        assert_eq!(panel.editing[&A][&key], "2.0");

        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueReset(A, key.clone()),
        );

        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Unchanged
        ));
        assert!(!panel.editing[&A].contains_key(&key));
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
            Message::ValueUpload(A, key.clone(), Value::F32(2.0)),
        );
        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Uploading { .. }
        ));
        assert!(panel.uploads.contains_key(&(A, key.clone())));

        // Echoed exactly: accepted.
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUploadResult(A, key.clone(), Value::F32(2.0), Ok(Value::F32(2.0))),
        );
        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Unchanged
        ));
        assert_eq!(param(&fleet, A, &key).value(), Value::F32(2.0));
        assert!(!panel.uploads.contains_key(&(A, key.clone())));

        // Clamped by the target: rejected, keeping the requested value.
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUpload(A, key.clone(), Value::F32(5.0)),
        );
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUploadResult(A, key.clone(), Value::F32(5.0), Ok(Value::F32(3.0))),
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
            Message::ValueUpload(A, key.clone(), Value::F32(7.0)),
        );
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ValueUploadResult(
                A,
                key.clone(),
                Value::F32(7.0),
                Err(crate::mav::error::MavError::Timeout),
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
            Message::ValueUploadResult(A, key.clone(), Value::F32(9.0), Ok(Value::F32(9.0))),
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
            .parameters_mut()
            .note_loaded_index(0, 5);
        let mut panel = ParameterPanel::new();

        step(&mut panel, &router, &mut fleet, Message::ListReload(A));

        assert_eq!(fleet.vehicle(A).unwrap().parameters().loading().loaded(), 0);
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
            Message::LoadFinished(PathBuf::from("/tmp/params.ini"), A, Ok(loaded)),
        );

        assert!(matches!(event, Some(Event::SetFilePickerPath(_))));
        assert!(matches!(
            param(&fleet, A, &key).state(),
            ParamState::Changed { value: Value::F32(v), .. } if *v == 3.0
        ));
        assert_eq!(panel.editing[&A][&key], "3");
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
            Message::BufferEdit(A, key.clone(), "42".to_owned()),
        );

        // The second vehicle's buffer is untouched.
        assert!(
            !panel
                .editing
                .get(&B)
                .is_some_and(|entry| entry.contains_key(&key))
        );
        assert_eq!(panel.editing[&A][&key], "42");
    }

    #[test]
    fn list_reload_tracks_in_flight_download() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();

        step(&mut panel, &router, &mut fleet, Message::ListReload(A));
        assert!(panel.reloading.contains(&A));
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
            Message::ListReloadStarted(A),
        );
        assert!(panel.reloading.contains(&A));
    }

    #[test]
    fn list_reload_result_clears_in_flight_download() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let mut fleet = Fleet::default();
        add_vehicle(&mut fleet, A, &[("PARAM_ONE", Value::F32(1.0))]);
        let mut panel = ParameterPanel::new();

        // A successful download clears the flag.
        panel.reloading.insert(A);
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ListReloadResult(
                A,
                Ok(crate::mav::ParamListSummary {
                    received: 1,
                    expected: 1,
                }),
            ),
        );
        assert!(!panel.reloading.contains(&A));

        // A failed download clears it too.
        panel.reloading.insert(A);
        step(
            &mut panel,
            &router,
            &mut fleet,
            Message::ListReloadResult(A, Err(crate::mav::error::MavError::Timeout)),
        );
        assert!(!panel.reloading.contains(&A));
    }
}
