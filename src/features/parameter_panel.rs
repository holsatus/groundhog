//! The parameter feature: browsing, filtering, editing, uploading, and
//! saving/loading a vehicle's parameter set.
//!
//! Owns the filter and its cached filtered view. Parameter data itself lives on
//! the [`Vehicle`]s, which the root owns and passes in; the router is passed in
//! for transactions. Writing the remembered file-picker directory back to the
//! configuration is requested through [`Event`].

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use iced::{
    Alignment, Color, Element, Font, Length, Task, Widget,
    alignment::Vertical,
    widget::{Button, Column, ProgressBar, Space, Text, TextInput, container, progress_bar, row, rule, space, tooltip},
};
use mav_param::{Ident, Value};
use rfd::AsyncFileDialog;

use crate::{
    domain::parameter::{
        self, ChangedStatus, ParamState, Parameter, Parameters, value_as_string, value_parse_as,
        value_type_matches, value_type_name,
    },
    domain::vehicle::Vehicle,
    foundation::MavlinkId,
    mav::{self, LockKey, MavlinkRouter},
    style::shaded_bordered_box,
    widget::virtualized::virtual_list,
};

#[derive(Default, PartialEq)]
pub struct ParameterFilter {
    pub ident_pattern: String,
    pub only_changed: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Filter for only certain parameters
    FilterIdentPattern(String),

    /// Whether to only show changed parameters
    FilterOnlyChanged(bool),

    /// Initiate a parameter list request to the target
    ListReload(MavlinkId),

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

    /// Open a file-picker dialog to choose file to load
    LoadDialog(MavlinkId),

    /// Load parameters from the chosen path
    LoadFromFile(PathBuf, MavlinkId),
}

/// Everything the panel needs from the rest of the application to run a
/// parameter transaction.
pub struct Context<'a> {
    pub router: &'a MavlinkRouter,
    pub vehicles: &'a mut BTreeMap<MavlinkId, Vehicle>,
    pub primary: Option<MavlinkId>,
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
    filtered: Option<Parameters>,
    namespace_sep: char,
}

impl ParameterPanel {
    pub fn new() -> Self {
        Self {
            filter: ParameterFilter::default(),
            filtered: None,
            namespace_sep: '_',
        }
    }

    /// Recompute the cached filtered parameter set for the primary vehicle.
    pub(crate) fn refresh(
        &mut self,
        primary: Option<MavlinkId>,
        vehicles: &BTreeMap<MavlinkId, Vehicle>,
    ) {
        // Shortcut if no filter has been applied
        if self.filter == ParameterFilter::default() {
            self.filtered = None;
            return;
        }

        let filter_string_lower = self.filter.ident_pattern.to_lowercase();

        let Some(vehicle) = primary.and_then(|id| vehicles.get(&id)) else {
            self.filtered = None;
            return;
        };

        let param_map = vehicle
            .parameters
            .map
            .iter()
            .filter(|(_, param)| {
                !self.filter.only_changed || !matches!(param.state, ParamState::Unchanged)
            })
            .filter(|(ident, _)| {
                filter_string_lower.is_empty()
                    || ident.as_str().to_lowercase().contains(&filter_string_lower)
            })
            .map(|(ident, param)| (ident.clone(), param.clone()));

        self.filtered = Some(Parameters {
            map: param_map.collect(),
            loading_state: vehicle.parameters.loading_state.clone(),
        });
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
                let vehicle = ctx.vehicles.get_mut(&mav_id)?;

                let request_capabilities = vehicle.capabilities.is_none();
                vehicle.parameters.loading_state.has_loaded.clear();

                let task = Task::perform(
                    async move { mav.param_list(mav_id, request_capabilities).await },
                    move |result| Message::ListReloadResult(mav_id, result),
                );

                return Some((task, None));
            }
            Message::ListReloadResult(mav_id, result) => match result {
                Ok(summary) => log::info!("Parameters from {mav_id:?}: {summary:?}"),
                Err(error) => log::warn!("Parameter download from {mav_id:?} failed: {error}"),
            },
            Message::UploadAll(mav_id) => {
                let vehicle = ctx.vehicles.get(&mav_id)?;

                // The number of concurrent uploads is limited by the middleware
                let uploads: Vec<_> = vehicle
                    .parameters
                    .map
                    .iter()
                    .filter_map(|(ident, param)| match param.state {
                        ParamState::Changed { value, .. } => {
                            Some(Task::done(Message::ValueUpload(mav_id, ident.clone(), value)))
                        }
                        _ => None,
                    })
                    .collect();

                log::info!("Uploading {} changed parameters", uploads.len());
                return Some((Task::batch(uploads), None));
            }
            Message::BufferEdit(mav_id, ident, buffer) => {
                let entry = ctx.vehicles.get_mut(&mav_id)?;
                let param = entry.parameters.map.get_mut(&ident)?;

                param.state = ParamState::Unchanged;

                if let Some(new_value) = value_parse_as(param.value, &buffer)
                    && new_value != param.value
                {
                    param.state = ParamState::Changed {
                        value: new_value,
                        status: ChangedStatus::None,
                    };
                }

                param.editing = Some(buffer);
            }
            Message::ValueReset(mav_id, ident) => {
                let entry = ctx.vehicles.get_mut(&mav_id)?;
                let param = entry.parameters.map.get_mut(&ident)?;

                param.editing = None;
                param.state = ParamState::Unchanged;
            }
            Message::ValueUpload(mav_id, ident, value) => {
                let mav = ctx.router.clone();
                let vehicle = ctx.vehicles.get_mut(&mav_id)?;
                let param = vehicle.parameters.map.get_mut(&ident)?;
                let cap = vehicle.capabilities.unwrap_or_default();

                let ident_cloned = ident.clone();
                let (task, handle) = Task::perform(
                    async move { mav.param_set(mav_id, ident_cloned, value, cap).await },
                    move |result| Message::ValueUploadResult(mav_id, ident.clone(), value, result),
                )
                .abortable();

                // Save the value we are currently uploading. Dropping the state
                // (e.g. by editing the value again) aborts the upload.
                param.state = ParamState::Uploading {
                    value,
                    handle: handle.abort_on_drop(),
                };

                return Some((task, None));
            }
            Message::ValueUploadResult(mav_id, ident, requested, result) => {
                let vehicle = ctx.vehicles.get_mut(&mav_id)?;
                let param = vehicle.parameters.map.get_mut(&ident)?;

                if !matches!(param.state, ParamState::Uploading { value, .. } if value == requested)
                {
                    return None;
                }

                match result {
                    Ok(value) if value == requested => {
                        param.value = value;
                        param.state = ParamState::Unchanged;
                    }
                    Ok(value) => {
                        log::warn!(
                            "Parameter '{}' was set to {value:?} instead of {requested:?}",
                            ident.as_str()
                        );
                        param.value = value;
                        param.state = ParamState::Changed {
                            value: requested,
                            status: ChangedStatus::Rejected,
                        };
                    }
                    Err(error) => {
                        log::warn!("Parameter upload for '{}' failed: {error}", ident.as_str());
                        param.state = ParamState::Changed {
                            value: requested,
                            status: ChangedStatus::Failed,
                        };
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

                if let Err(error) = parameter::save_parameters_to_ini(&save_path, parameters) {
                    log::error!("Error saving to file: {}", error);
                }

                self.refresh(ctx.primary, ctx.vehicles);
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

                let vehicle = ctx.vehicles.get_mut(&mav_id)?;
                let loaded_params = match parameter::load_parameters_from_ini(&load_path) {
                    Ok(loaded_params) => loaded_params,
                    Err(error) => {
                        log::error!("Could not load parameter file: {}", error);
                        return None;
                    }
                };

                // Set the parameters of the vehicle as modified if that is the case
                for (ident, new_param) in loaded_params.map.iter() {
                    if let Some(old_param) = vehicle.parameters.map.get_mut(ident)
                        && value_type_matches(old_param.value, new_param.value)
                        && old_param.value != new_param.value
                    {
                        old_param.state = ParamState::Changed {
                            value: new_param.value,
                            status: ChangedStatus::None,
                        };
                        old_param.editing = Some(value_as_string(new_param.value));
                    }
                }

                self.refresh(ctx.primary, ctx.vehicles);
                return Some((Task::none(), Some(Event::SetFilePickerPath(load_path))));
            }
        }

        self.refresh(ctx.primary, ctx.vehicles);
        None
    }

    pub fn view<'a>(
        &'a self,
        vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
        primary: Option<MavlinkId>,
        router: &'a MavlinkRouter,
    ) -> Element<'a, Message> {
        let Some(mav_id) = primary else {
            return space::Space::new().boxed();
        };

        let Some(vehicle) = vehicles.get(&mav_id) else {
            return space::Space::new().boxed();
        };

        let mut entries: Vec<Element<'_, Message>> = Vec::with_capacity(128);

        // Disabled while a parameter list download is in progress
        let reloading = router.is_busy(&LockKey::ParamList(mav_id));
        let reload_button = Button::new("Reload parameters")
            .width(Length::Fill)
            .on_press_maybe((!reloading).then(|| Message::ListReload(mav_id)));

        let upload_button = Button::new("Upload parameters")
            .width(Length::FillPortion(1))
            .on_press(Message::UploadAll(mav_id));

        let save_button = Button::new("Save parameters to file")
            .width(Length::FillPortion(1))
            .on_press_with(|| Message::SaveDialog(vehicle.parameters.clone()));

        let load_button = Button::new("Load parameters from file")
            .width(Length::FillPortion(1))
            .on_press(Message::LoadDialog(mav_id));

        let filter_field = TextInput::new("Filter parameters", &self.filter.ident_pattern)
            .width(Length::FillPortion(1))
            .on_input(Message::FilterIdentPattern);

        entries.push(iced::widget::row![reload_button, upload_button].spacing(5.0).boxed());
        entries.push(iced::widget::row![save_button, load_button].spacing(5.0).boxed());

        entries.push(filter_field.boxed());

        entries.push(
            iced::widget::checkbox(self.filter.only_changed)
                .label("Only changed")
                .on_toggle(Message::FilterOnlyChanged)
                .boxed(),
        );

        let got = vehicle.parameters.loading_state.has_loaded.len();
        let exp = vehicle.parameters.loading_state.expected_count;

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
                self.view_param_list(mav_id, vehicle),
            ]
            .spacing(5.0),
        )
        .padding(10.0)
        .width(500.0)
        .style(shaded_bordered_box)
        .boxed()
    }

    fn view_param_list<'a>(
        &'a self,
        mav_id: MavlinkId,
        vehicle: &'a Vehicle,
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

        let parameters = self.filtered.as_ref().unwrap_or(&vehicle.parameters);

        let mut items = Vec::with_capacity(parameters.map.len() + 64);
        let mut section = None;

        for (ident, param) in &parameters.map {
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
            Item::Param(ident, param) => view_param_row(mav_id, ident, param).boxed(),
        })
        .item_height(|item| match item {
            Item::Section { name: None, .. } => SEPARATOR_HEIGHT,
            Item::Section { .. } => HEADER_HEIGHT,
            Item::Param(..) => ROW_HEIGHT,
        })
        .key(|item| match item {
            Item::Section { first, .. } => ((*first).clone(), true),
            Item::Param(ident, _) => ((*ident).clone(), false),
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
) -> Element<'a, Message> {
    let type_name = value_type_name(param.value);

    let value_string = match param.editing.clone() {
        Some(buffer) => buffer.clone(),
        None => value_as_string(param.value),
    };

    let text_input = iced::widget::TextInput::new("Write value", value_string)
        .on_input(move |string| Message::BufferEdit(mav_id, ident.clone(), string))
        .style(move |theme, status| {
            let mut style = iced::widget::text_input::default(theme, status);
            let palette = theme.palette();
            match &param.state {
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
        .on_press_maybe(match param.state {
            ParamState::Changed { .. } => Some(Message::ValueReset(mav_id, ident.clone())),
            _ => None,
        })
        .width(80.0);

    let restore_tooltip = container(Text::new(match param.state {
        ParamState::Changed { .. } => format!("Restore to: {}", value_as_string(param.value)),
        _ => "Nothing to restore".to_string(),
    }))
    .padding(10.0)
    .style(shaded_bordered_box);

    let restore_button =
        tooltip(restore_button, restore_tooltip, Default::default()).delay(std::time::Duration::from_millis(250));

    let upload_button = Button::new("Upload")
        .on_press_maybe(match param.state {
            ParamState::Changed { value, .. } => Some(Message::ValueUpload(mav_id, ident.clone(), value)),
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
            .color(Color::from_rgba8(255, 255, 255, 0.5)),
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
