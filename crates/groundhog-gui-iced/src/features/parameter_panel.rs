//! The parameter feature: browsing, filtering, editing, uploading, and
//! saving/loading a component's parameters.
//!
//! Parameters are per MAVLink *component*: the operator picks which component
//! to view (the autopilot by default), and every operation addresses that
//! `(system, component)` pair. Reported values come from the fleet snapshot;
//! the panel owns only the operator's pending [`ParamEdit`]s and the uploads
//! in flight.
//!
//! Uploads are refused while the component's parameter encoding is unknown:
//! a value written with a guessed encoding may be stored as something else
//! entirely.

use std::{collections::BTreeMap, path::PathBuf, sync::LazyLock};

use groundhog_logic::{
    MavlinkId, SystemId,
    domain::{
        fleet::FleetSnapshot,
        parameter::{
            DownloadState, EditStatus, ParamEdit, ParamValues, Parameters, ReportedParam, group_of,
            value_to_string, value_type_matches, value_type_name,
        },
        vehicle::Vehicle,
    },
    services::{ParamListSummary, Result},
    storage::parameter::{ParamFileFormat, read_parameters, write_parameters},
};
use iced::{
    Alignment, Color, Element, Font, Length, Task, Theme, Widget,
    alignment::Vertical,
    widget::{
        Button, Column, ProgressBar, Space, Svg, Text, TextInput, container, pick_list,
        progress_bar, row, rule, space, svg::Handle, tooltip,
    },
};
use mav_param::{Ident, Value};

use super::{
    Context, Effect, file_dialog,
    notifications::Notice,
    operations::{Operations, Ticket},
};
use crate::{style, widget::virtualized::virtual_list};

static UPLOAD_ICON: LazyLock<Handle> = LazyLock::new(|| {
    static BYTES: &[u8] = include_bytes!("../../assets/icons/upload.svg");
    Handle::from_memory(BYTES)
});

static RESTORE_ICON: LazyLock<Handle> = LazyLock::new(|| {
    static BYTES: &[u8] = include_bytes!("../../assets/icons/undo-2.svg");
    Handle::from_memory(BYTES)
});

#[derive(Default, PartialEq)]
pub struct ParameterFilter {
    pub ident_pattern: String,
    pub only_changed: bool,
}

impl ParameterFilter {
    /// Whether `ident` matches the filter, given the already-lowercased
    /// pattern and whether it has a pending edit.
    fn matches(&self, ident: &Ident, edited: bool, pattern_lower: &str) -> bool {
        if self.only_changed && !edited {
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

type UploadKey = (MavlinkId, Ident);

#[derive(Debug, Clone)]
pub enum Message {
    FilterIdentPattern(String),
    FilterOnlyChanged(bool),
    /// Show the parameters of this component.
    SelectComponent(MavlinkId),
    /// Download the component's parameter list again.
    Reload(MavlinkId),
    Reloaded(MavlinkId, Ticket, Result<ParamListSummary>),
    /// The operator typed into a value field.
    Edit(MavlinkId, Ident, String),
    /// Discard a pending edit.
    Reset(MavlinkId, Ident),
    /// Discard every pending edit of the component.
    ResetAll(MavlinkId),
    Upload(MavlinkId, Ident),
    UploadAll(MavlinkId),
    Uploaded(UploadKey, Ticket, Value, Result<Value>),
    SaveDialog(MavlinkId),
    SaveTo(PathBuf, MavlinkId, ParamValues),
    Saved(PathBuf, std::result::Result<(), String>),
    LoadDialog(MavlinkId),
    LoadFrom(PathBuf, MavlinkId),
    Loaded(PathBuf, MavlinkId, std::result::Result<ParamValues, String>),
}

#[derive(Default)]
pub struct ParameterPanel {
    filter: ParameterFilter,
    /// Pending edits, per component and parameter. Kept here (not in the
    /// fleet) so switching components or vehicles and back keeps them.
    edits: BTreeMap<MavlinkId, BTreeMap<Ident, ParamEdit>>,
    uploads: Operations<UploadKey>,
    reloads: Operations<MavlinkId>,
    /// The component the operator chose to view, per vehicle.
    component: BTreeMap<SystemId, u8>,
}

impl ParameterPanel {
    /// The component shown for `vehicle`: the operator's choice, else its
    /// autopilot (or the conventional component 1 before one is known).
    fn selected_component(&self, vehicle: &Vehicle) -> u8 {
        self.component
            .get(&vehicle.system)
            .copied()
            .unwrap_or_else(|| vehicle.default_parameter_component())
    }

    fn edit(&self, address: MavlinkId, ident: &Ident) -> Option<&ParamEdit> {
        self.edits.get(&address)?.get(ident)
    }

    /// The pending edits of `address`, for tests and the view.
    fn edits_of(&self, address: MavlinkId) -> impl Iterator<Item = (&Ident, &ParamEdit)> {
        self.edits.get(&address).into_iter().flatten()
    }

    pub fn update(&mut self, message: Message, cx: &mut Context<Message>) -> Task<Message> {
        match message {
            Message::FilterIdentPattern(pattern) => self.filter.ident_pattern = pattern,
            Message::FilterOnlyChanged(only) => self.filter.only_changed = only,
            Message::SelectComponent(address) => {
                self.component
                    .insert(address.system_id(), address.component);
            }
            Message::Reload(address) => {
                let client = cx.services.component(address);
                return self.reloads.start(
                    address,
                    async move { client.fetch_parameters().await },
                    move |ticket, result| Message::Reloaded(address, ticket, result),
                );
            }
            Message::Reloaded(address, ticket, result) => {
                if self.reloads.finish(&address, ticket) {
                    match result {
                        Ok(summary) if summary.received >= usize::from(summary.expected) => {
                            cx.notices.push(Notice::success(format!(
                                "Downloaded {} parameters",
                                summary.received
                            )));
                        }
                        Ok(summary) => cx.notices.push(
                            Notice::warning("Parameter download incomplete").detail(format!(
                                "Received {} of {} parameters",
                                summary.received, summary.expected
                            )),
                        ),
                        Err(error) => cx
                            .notices
                            .push(Notice::error("Parameter download failed").detail(error)),
                    }
                }
            }
            Message::Edit(address, ident, text) => {
                // Editing supersedes any upload in flight.
                self.uploads.cancel(&(address, ident.clone()));

                let current = reported(cx.fleet, address, &ident).map(ReportedParam::value);
                let edits = self.edits.entry(address).or_default();
                match current.and_then(|current| ParamEdit::new(current, text)) {
                    Some(edit) => {
                        edits.insert(ident, edit);
                    }
                    None => {
                        edits.remove(&ident);
                    }
                }
            }
            Message::Reset(address, ident) => {
                self.uploads.cancel(&(address, ident.clone()));
                if let Some(edits) = self.edits.get_mut(&address) {
                    edits.remove(&ident);
                }
            }
            Message::ResetAll(address) => {
                if let Some(edits) = self.edits.remove(&address) {
                    for ident in edits.into_keys() {
                        self.uploads.cancel(&(address, ident));
                    }
                }
            }
            Message::Upload(address, ident) => return self.upload(address, ident, cx),
            Message::UploadAll(address) => {
                let idents: Vec<Ident> = self
                    .edits_of(address)
                    .filter(|(_, edit)| edit.uploadable().is_some())
                    .map(|(ident, _)| ident.clone())
                    .collect();

                // The middleware bounds how many sets are in flight at once.
                return Task::batch(
                    idents
                        .into_iter()
                        .map(|ident| self.upload(address, ident, cx)),
                );
            }
            Message::Uploaded(key, ticket, requested, result) => {
                if !self.uploads.finish(&key, ticket) {
                    return Task::none();
                }

                let (address, ident) = key;
                let Some(edit) = self
                    .edits
                    .get_mut(&address)
                    .and_then(|edits| edits.get_mut(&ident))
                else {
                    return Task::none();
                };

                let result = result.map_err(|error| error.to_string());
                if let Err(error) = &result {
                    cx.notices.push(
                        Notice::error(format!("Setting {} failed", ident.as_str())).detail(error),
                    );
                } else if let Ok(reported) = result
                    && reported != requested
                {
                    cx.notices.push(
                        Notice::warning(format!("{} was not set as requested", ident.as_str()))
                            .detail(format!(
                                "Requested {}, the vehicle holds {}",
                                value_to_string(requested),
                                value_to_string(reported)
                            )),
                    );
                }

                if edit.finish_upload(requested, result)
                    && let Some(edits) = self.edits.get_mut(&address)
                {
                    edits.remove(&ident);
                }
            }
            Message::SaveDialog(address) => {
                // Snapshot the values now, not when the dialog closes.
                let Some(values) = parameters(cx.fleet, address).map(Parameters::values) else {
                    return Task::none();
                };
                let dialog = file_dialog(cx.directory)
                    .set_file_name("parameters.param")
                    .add_filter("Mission Planner", &["param", "parm"])
                    .add_filter("QGroundControl", &["params"])
                    .add_filter("Groundhog INI", &["ini"]);
                return Task::future(async move {
                    dialog
                        .save_file()
                        .await
                        .map(|file| (file.path().to_owned(), values))
                })
                .and_then(move |(path, values)| {
                    Task::done(Message::SaveTo(path, address, values))
                });
            }
            Message::SaveTo(path, address, values) => {
                return iced_runtime::task::blocking(move |mut sender| {
                    let text = write_parameters(ParamFileFormat::of(&path), &values, address);
                    let result = std::fs::write(&path, text).map_err(|error| error.to_string());
                    let _ = sender.try_send(Message::Saved(path, result));
                });
            }
            Message::Saved(path, result) => match result {
                Ok(()) => {
                    cx.notices
                        .push(Notice::success("Parameters saved").detail(path.display()));
                    cx.request(Effect::RememberDirectory(path));
                }
                Err(error) => cx.notices.push(
                    Notice::error(format!("Could not save {}", path.display())).detail(error),
                ),
            },
            Message::LoadDialog(address) => {
                let dialog = file_dialog(cx.directory);
                return Task::future(async move {
                    dialog.pick_file().await.map(|file| file.path().to_owned())
                })
                .and_then(move |path| Task::done(Message::LoadFrom(path, address)));
            }
            Message::LoadFrom(path, address) => {
                // Untyped formats are read as the types the component reports.
                let reported = parameters(cx.fleet, address)
                    .map(Parameters::values)
                    .unwrap_or_default();
                return iced_runtime::task::blocking(move |mut sender| {
                    let result = std::fs::read_to_string(&path)
                        .map_err(|error| error.to_string())
                        .and_then(|text| {
                            read_parameters(ParamFileFormat::of(&path), &text, |ident| {
                                reported.get(ident).copied()
                            })
                        });
                    let _ = sender.try_send(Message::Loaded(path, address, result));
                });
            }
            Message::Loaded(path, address, result) => match result {
                Ok(loaded) => {
                    let (changed, skipped) = self.load_edits(cx.fleet, address, &loaded);
                    let mut notice =
                        Notice::success(format!("Loaded {changed} changed parameters"));
                    if skipped > 0 {
                        notice = notice.detail(format!(
                            "{skipped} entries were unknown or of another type and were skipped"
                        ));
                    }
                    cx.notices.push(notice);
                    cx.request(Effect::RememberDirectory(path));
                }
                Err(error) => cx.notices.push(
                    Notice::error(format!("Could not load {}", path.display())).detail(error),
                ),
            },
        }

        Task::none()
    }

    /// Turn loaded values into pending edits where they differ from the
    /// component's. Returns `(changed, skipped)`.
    fn load_edits(
        &mut self,
        fleet: &FleetSnapshot,
        address: MavlinkId,
        loaded: &ParamValues,
    ) -> (usize, usize) {
        let Some(parameters) = parameters(fleet, address) else {
            return (0, loaded.len());
        };

        let edits = self.edits.entry(address).or_default();
        let (mut changed, mut skipped) = (0, 0);

        for (ident, value) in loaded {
            match parameters.value(ident) {
                Some(current) if value_type_matches(current, *value) => {
                    if current != *value {
                        edits.insert(ident.clone(), ParamEdit::to_value(*value));
                        changed += 1;
                    }
                }
                _ => skipped += 1,
            }
        }

        (changed, skipped)
    }

    fn upload(
        &mut self,
        address: MavlinkId,
        ident: Ident,
        cx: &mut Context<Message>,
    ) -> Task<Message> {
        let Some(edit) = self
            .edits
            .get_mut(&address)
            .and_then(|edits| edits.get_mut(&ident))
        else {
            return Task::none();
        };
        let Some(value) = edit.uploadable() else {
            return Task::none();
        };

        edit.status = EditStatus::Uploading;
        let client = cx.services.component(address);
        let key = (address, ident.clone());

        self.uploads.start(
            key.clone(),
            async move { client.set_parameter(ident, value).await },
            move |ticket, result| Message::Uploaded(key, ticket, value, result),
        )
    }

    /// Render the parameter panel.
    pub fn view<'a>(
        &'a self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
        theme: &Theme,
    ) -> Element<'a, Message> {
        let Some(vehicle) = selected.and_then(|system| fleet.vehicle(system)) else {
            return container(Text::new("No vehicle selected."))
                .padding(10.0)
                .boxed();
        };

        let system = vehicle.system;
        let component = self.selected_component(vehicle);
        let address = vehicle.target_for(component);
        let parameters = vehicle.parameters(component);
        let encoding_known = vehicle.param_encoding(component).is_some();

        let mut entries: Vec<Element<'_, Message>> = Vec::with_capacity(8);

        // Which component's parameters are shown; the autopilot is marked.
        let component_ids = vehicle.component_ids();
        if !component_ids.is_empty() {
            let autopilot = vehicle.autopilot_component();
            let picker = pick_list(Some(component), component_ids, move |id: &u8| {
                if Some(*id) == autopilot {
                    format!("Component {id} (autopilot)")
                } else {
                    format!("Component {id}")
                }
            })
            .style(crate::style::pick_list::default)
            .on_select(move |id| {
                Message::SelectComponent(MavlinkId {
                    system: system.0,
                    component: id,
                })
            })
            .width(Length::Fill);

            entries.push(picker.boxed());
        }

        let downloading = self.reloads.is_running(&address)
            || parameters
                .is_some_and(|parameters| *parameters.download() == DownloadState::InProgress);
        let pending = self
            .edits_of(address)
            .filter(|(_, edit)| edit.uploadable().is_some())
            .count();

        let action = |label: String, message: Option<Message>| {
            Button::new(Text::new(label))
                .style(crate::style::button::transparent)
                .width(Length::FillPortion(1))
                .on_press_maybe(message)
        };

        entries.push(
            row![
                action(
                    if downloading {
                        "Downloading…".to_string()
                    } else {
                        "Download parameters".to_string()
                    },
                    (!downloading).then_some(Message::Reload(address)),
                ),
                action(
                    format!("Upload {pending} changed"),
                    (pending > 0 && encoding_known).then_some(Message::UploadAll(address)),
                ),
            ]
            .spacing(5.0)
            .boxed(),
        );
        entries.push(
            row![
                action(
                    "Save to file".to_string(),
                    Some(Message::SaveDialog(address))
                ),
                action(
                    "Load from file".to_string(),
                    Some(Message::LoadDialog(address))
                ),
                action(
                    "Discard changes".to_string(),
                    (pending > 0).then_some(Message::ResetAll(address)),
                ),
            ]
            .spacing(5.0)
            .boxed(),
        );

        if !encoding_known && parameters.is_some_and(|parameters| !parameters.is_empty()) {
            entries.push(
                Text::new(
                    "The component's parameter encoding is not known yet, so values may \
                     display wrongly and uploads are disabled.",
                )
                .color(theme.palette().warning.base.color)
                .size(12)
                .boxed(),
            );
        }

        let (got, expected) = parameters.map_or((0, 0), |parameters| {
            (
                parameters.loading().loaded(),
                parameters.loading().expected(),
            )
        });

        let complete = parameters.is_some_and(|parameters| parameters.loading().is_complete());
        let style = if !complete {
            progress_bar::primary
        } else {
            progress_bar::success
        };

        entries.push(
            ProgressBar::new(0.0..=f32::from(expected.max(1)), got as f32)
                .style(style)
                .girth(10.0)
                .boxed(),
        );

        if let Some(DownloadState::Failed(error)) = parameters.map(Parameters::download) {
            entries.push(
                Text::new(format!("Last download: {error}"))
                    .color(theme.palette().danger.base.color)
                    .size(12)
                    .boxed(),
            );
        }

        entries.push(
            container(
                iced::widget::column![
                    TextInput::new("Filter parameters", &self.filter.ident_pattern)
                        .style(crate::style::text_input::default)
                        .on_input(Message::FilterIdentPattern),
                    iced::widget::checkbox(self.filter.only_changed)
                        .label("Only changed")
                        .on_toggle(Message::FilterOnlyChanged)
                ]
                .spacing(5.0),
            )
            .style(crate::style::inner_container)
            .padding(10.0)
            .boxed(),
        );

        container(
            iced::widget::column![
                Column::from_vec(entries).spacing(5.0),
                self.view_param_list(address, parameters, encoding_known, theme),
            ]
            .spacing(5.0),
        )
        .padding(10.0)
        .width(Length::Fill)
        .boxed()
    }

    fn view_param_list<'a>(
        &'a self,
        address: MavlinkId,
        parameters: Option<&'a Parameters>,
        encoding_known: bool,
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
            Param(&'a Ident, &'a ReportedParam, Option<&'a ParamEdit>),
        }

        let Some(parameters) = parameters else {
            return space::Space::new().boxed();
        };

        let pattern = self.filter.ident_pattern.to_lowercase();
        // Copied up front so the draw closure does not borrow the theme.
        let colors = RowColors::of(theme);

        let mut items = Vec::with_capacity(parameters.len() + 64);
        let mut section = None;

        for (ident, param) in parameters.iter() {
            let edit = self.edit(address, ident);
            if !self.filter.matches(ident, edit.is_some(), &pattern) {
                continue;
            }

            let this_section = group_of(ident);

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

            items.push(Item::Param(ident, param, edit));
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
            Item::Param(ident, param, edit) => {
                view_param_row(address, ident, param, *edit, encoding_known, colors)
            }
        })
        .item_height(|item| match item {
            Item::Section { name: None, .. } => SEPARATOR_HEIGHT,
            Item::Section { .. } => HEADER_HEIGHT,
            Item::Param(..) => ROW_HEIGHT,
        })
        .key(move |item| match item {
            Item::Section { first, .. } => ((address, (*first).clone()), true),
            Item::Param(ident, ..) => ((address, (*ident).clone()), false),
        })
        .spacing(5.0);

        iced::widget::scrollable(list)
            .style(iced::widget::scrollable::default)
            .spacing(5.0)
            .boxed()
    }
}

/// The parameters of `address`, if its vehicle and component are known.
fn parameters(fleet: &FleetSnapshot, address: MavlinkId) -> Option<&Parameters> {
    fleet
        .vehicle(address.system_id())?
        .parameters(address.component)
}

fn reported<'a>(
    fleet: &'a FleetSnapshot,
    address: MavlinkId,
    ident: &Ident,
) -> Option<&'a ReportedParam> {
    parameters(fleet, address)?.get(ident)
}

/// Theme colours a row needs, copied so rows do not borrow the theme.
#[derive(Clone, Copy)]
struct RowColors {
    type_label: Color,
    pending: Color,
    invalid: Color,
    uploading: Color,
    failed: Color,
    rejected: Color,
}

impl RowColors {
    fn of(theme: &Theme) -> Self {
        let palette = theme.palette();
        Self {
            type_label: style::parameter_type_color(theme),
            pending: palette.success.base.color,
            invalid: palette.secondary.base.color,
            uploading: palette.primary.base.color,
            failed: palette.warning.base.color,
            rejected: palette.danger.base.color,
        }
    }
}

fn view_param_row<'a>(
    address: MavlinkId,
    ident: &'a Ident,
    param: &'a ReportedParam,
    edit: Option<&'a ParamEdit>,
    encoding_known: bool,
    colors: RowColors,
) -> Element<'a, Message> {
    let value_text = edit.map_or_else(|| value_to_string(param.value()), |edit| edit.text.clone());

    let highlight = edit.map(|edit| match &edit.status {
        EditStatus::Pending if edit.value.is_some() => colors.pending,
        EditStatus::Pending => colors.invalid,
        EditStatus::Uploading => colors.uploading,
        EditStatus::Failed(_) => colors.failed,
        EditStatus::Rejected { .. } => colors.rejected,
    });

    let input = TextInput::new("Value", value_text)
        .on_input(move |text| Message::Edit(address, ident.clone(), text))
        .style(move |theme, status| {
            let mut style = crate::style::text_input::default(theme, status);
            if let Some(color) = highlight {
                style.border.color = color;
                style.background = color.scale_alpha(0.1).into();
                style.border.width = 2.0;
            }
            style
        })
        .width(Length::Fill);

    let icon = |handle: &Handle, active: bool| {
        Svg::new(handle.clone())
            .style(move |theme: &Theme, _| iced::widget::svg::Style {
                color: Some(if active {
                    theme.palette().background.base.text
                } else {
                    theme.palette().background.strongest.color
                }),
            })
            .width(40.0)
    };

    let restore =
        Button::new(container(icon(&RESTORE_ICON, edit.is_some())).align_y(Alignment::Center))
            .on_press_maybe(edit.map(|_| Message::Reset(address, ident.clone())))
            .style(crate::style::button::transparent)
            .width(38.0)
            .height(38.0);

    let restore_hint = match edit {
        Some(_) => format!("Restore to {}", value_to_string(param.value())),
        None => "Nothing to restore".to_string(),
    };

    let uploadable = edit
        .and_then(ParamEdit::uploadable)
        .filter(|_| encoding_known);
    let upload =
        Button::new(container(icon(&UPLOAD_ICON, uploadable.is_some())).align_y(Alignment::Center))
            .on_press_maybe(uploadable.map(|_| Message::Upload(address, ident.clone())))
            .style(crate::style::button::transparent)
            .width(38.0)
            .height(38.0);

    let upload_hint = match (edit.map(|edit| &edit.status), uploadable) {
        _ if edit.is_some_and(ParamEdit::is_uploading) => "Uploading…".to_string(),
        (Some(EditStatus::Failed(error)), _) => format!("Failed: {error}"),
        (Some(EditStatus::Rejected { reported }), _) => {
            format!("The vehicle kept {}", value_to_string(*reported))
        }
        (_, Some(value)) => format!("Upload as {}", value_to_string(value)),
        (Some(_), None) if !encoding_known => "Encoding unknown; uploads disabled".to_string(),
        _ => "Nothing to upload".to_string(),
    };

    let hint = |content: String| {
        container(Text::new(content))
            .padding(10.0)
            .style(style::tooltip)
    };

    row![
        Text::new(ident.as_str().to_string())
            .width(160.0)
            .font(Font::MONOSPACE),
        Text::new(value_type_name(param.value()))
            .width(50.0)
            .font(Font::MONOSPACE)
            .align_x(Alignment::End)
            .color(colors.type_label),
        Space::new().width(10.0),
        input,
        tooltip(restore, hint(restore_hint), Default::default())
            .delay(std::time::Duration::from_millis(250)),
        tooltip(upload, hint(upload_hint), Default::default())
            .delay(std::time::Duration::from_millis(250)),
    ]
    .spacing(5.0)
    .align_y(Vertical::Center)
    .boxed()
}

#[cfg(test)]
mod tests;
