//! The parameter feature: browsing, filtering, editing, uploading, and
//! saving/loading a component's parameters.
//!
//! Parameters are per MAVLink *component*: the operator picks which component
//! to view (the autopilot by default), and every operation addresses that
//! `(system, component)` pair. Reported values come from the fleet snapshot;
//! the panel owns only the operator's pending [`ParamEdits`] and the uploads
//! in flight.
//!
//! Uploads are refused while the component's parameter encoding is unknown:
//! a value written with a guessed encoding may be stored as something else
//! entirely.

use std::{collections::BTreeMap, path::PathBuf};

use groundhog_logic::{
    MavlinkId, SystemId,
    domain::{
        fleet::FleetSnapshot,
        parameter::{
            DownloadState, EditStatus, LoadSummary, ParamEdit, ParamEdits, ParamValues, Parameters,
            ReportedParam, UploadOutcome, group_of, value_to_string, value_type_name,
        },
        vehicle::Vehicle,
    },
    services::{ParamListSummary, Result},
    storage::parameter::{ParamFileFormat, read_parameters, write_parameters},
};
use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{column, progress_bar, row, scrollable, space},
};
use mav_param::{Ident, Value};

use super::{
    Context, Effect, file_dialog,
    notifications::Notice,
    operations::{Operations, Ticket},
};
use crate::{
    ui::{self, Icon, TextExt, Tone, style},
    widget::virtualized::virtual_list,
};

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
    edits: BTreeMap<MavlinkId, ParamEdits>,
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
        self.edits
            .get(&address)
            .into_iter()
            .flat_map(ParamEdits::iter)
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
                self.edits
                    .entry(address)
                    .or_default()
                    .edit(ident, current, text);
            }
            Message::Reset(address, ident) => {
                self.uploads.cancel(&(address, ident.clone()));
                if let Some(edits) = self.edits.get_mut(&address) {
                    edits.reset(&ident);
                }
            }
            Message::ResetAll(address) => {
                if let Some(edits) = self.edits.remove(&address) {
                    for (ident, _) in edits.iter() {
                        self.uploads.cancel(&(address, ident.clone()));
                    }
                }
            }
            Message::Upload(address, ident) => return self.upload(address, ident, cx),
            Message::UploadAll(address) => {
                let idents: Vec<Ident> = self
                    .edits
                    .get(&address)
                    .into_iter()
                    .flat_map(ParamEdits::uploadable)
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
                let outcome = self
                    .edits
                    .get_mut(&address)
                    .and_then(|edits| edits.finish_upload(&ident, requested, result));

                match outcome {
                    Some(UploadOutcome::Failed(error)) => cx.notices.push(
                        Notice::error(format!("Setting {} failed", ident.as_str())).detail(error),
                    ),
                    Some(UploadOutcome::Rejected {
                        requested,
                        reported,
                    }) => cx.notices.push(
                        Notice::warning(format!("{} was not set as requested", ident.as_str()))
                            .detail(format!(
                                "Requested {}, the vehicle holds {}",
                                value_to_string(requested),
                                value_to_string(reported)
                            )),
                    ),
                    Some(UploadOutcome::Set) | None => {}
                }
            }
            Message::SaveDialog(address) => {
                // Snapshot the values now, not when the dialog closes.
                let Some(values) = cx.fleet.parameters(address).map(Parameters::values) else {
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
                let reported = cx
                    .fleet
                    .parameters(address)
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
                    let LoadSummary { changed, skipped } = self
                        .edits
                        .entry(address)
                        .or_default()
                        .load(cx.fleet.parameters(address), &loaded);
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

    fn upload(
        &mut self,
        address: MavlinkId,
        ident: Ident,
        cx: &mut Context<Message>,
    ) -> Task<Message> {
        let Some(value) = self
            .edits
            .get_mut(&address)
            .and_then(|edits| edits.begin_upload(&ident))
        else {
            return Task::none();
        };

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
    ) -> Element<'a, Message> {
        let Some(vehicle) = selected.and_then(|system| fleet.vehicle(system)) else {
            return ui::empty("No vehicle selected.");
        };

        let system = vehicle.system;
        let component = self.selected_component(vehicle);
        let address = vehicle.target_for(component);
        let parameters = vehicle.parameters(component);
        let encoding_known = vehicle.param_encoding(component).is_some();

        // Which component's parameters are shown; the autopilot is marked.
        let component_ids = vehicle.component_ids();
        let autopilot = vehicle.autopilot_component();
        let picker = (!component_ids.is_empty()).then(|| {
            ui::pick_list(Some(component), component_ids, move |id: &u8| {
                if Some(*id) == autopilot {
                    format!("Component {id} (autopilot)")
                } else {
                    format!("Component {id}")
                }
            })
            .on_select(move |id| {
                Message::SelectComponent(MavlinkId {
                    system: system.0,
                    component: id,
                })
            })
        });

        let downloading = self.reloads.is_running(&address)
            || parameters
                .is_some_and(|parameters| *parameters.download() == DownloadState::InProgress);
        let pending = self
            .edits_of(address)
            .filter(|(_, edit)| edit.uploadable().is_some())
            .count();

        let action = |label: String, message: Option<Message>| {
            ui::button(label)
                .width(Length::FillPortion(1))
                .on_press_maybe(message)
                .boxed()
        };

        let transfers = row![
            ui::button("Download parameters")
                .width(Length::FillPortion(1))
                .busy(downloading, "Downloading…")
                .on_press(Message::Reload(address))
                .boxed(),
            action(
                format!("Upload {pending} changed"),
                (pending > 0 && encoding_known).then_some(Message::UploadAll(address)),
            ),
        ]
        .spacing(ui::space::S);

        let files = row![
            action("Save to file".into(), Some(Message::SaveDialog(address))),
            action("Load from file".into(), Some(Message::LoadDialog(address))),
            action(
                "Discard changes".into(),
                (pending > 0).then_some(Message::ResetAll(address)),
            ),
        ]
        .spacing(ui::space::S);

        let encoding_warning = (!encoding_known
            && parameters.is_some_and(|parameters| !parameters.is_empty()))
        .then(|| {
            ui::caption(
                "The component's parameter encoding is not known yet, so values may \
                 display wrongly and uploads are disabled.",
            )
            .tone(Tone::Warning)
        });

        let loading = parameters.map(Parameters::loading);
        let (got, expected) =
            loading.map_or((0, 0), |loading| (loading.loaded(), loading.expected()));
        let complete = loading.is_some_and(|loading| loading.is_complete());
        let progress = ui::progress(got as f32, f32::from(expected)).style(if complete {
            progress_bar::success
        } else {
            progress_bar::primary
        });

        let failure = match parameters.map(Parameters::download) {
            Some(DownloadState::Failed(error)) => {
                Some(ui::caption(format!("Last download: {error}")).tone(Tone::Danger))
            }
            _ => None,
        };

        let filter = ui::card(
            column![
                ui::text_input("Filter parameters", &self.filter.ident_pattern)
                    .on_input(Message::FilterIdentPattern),
                ui::checkbox("Only changed", self.filter.only_changed)
                    .on_toggle(Message::FilterOnlyChanged),
            ]
            .spacing(ui::space::S),
        );

        ui::panel(
            column![
                picker,
                transfers,
                files,
                encoding_warning,
                progress,
                failure,
                filter,
                self.view_param_list(address, parameters, encoding_known),
            ]
            .spacing(ui::space::S),
        )
    }

    fn view_param_list<'a>(
        &'a self,
        address: MavlinkId,
        parameters: Option<&'a Parameters>,
        encoding_known: bool,
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
            } => column![
                separator.then(ui::divider),
                name.map(|name| ui::heading(format!("[{name}]")).font(iced::Font::MONOSPACE)),
            ]
            .spacing(ui::space::XS)
            .boxed(),
            Item::Param(ident, param, edit) => {
                view_param_row(address, ident, param, *edit, encoding_known)
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
        .spacing(ui::space::XS);

        scrollable(list).spacing(ui::space::XS).boxed()
    }
}

fn reported<'a>(
    fleet: &'a FleetSnapshot,
    address: MavlinkId,
    ident: &Ident,
) -> Option<&'a ReportedParam> {
    fleet.parameters(address)?.get(ident)
}

/// How an edit is marked: by whether it can be uploaded and how its last
/// upload went.
fn edit_tone(edit: &ParamEdit) -> Tone {
    match &edit.status {
        EditStatus::Pending if edit.value.is_some() => Tone::Success,
        EditStatus::Pending => Tone::Muted,
        EditStatus::Uploading => Tone::Accent,
        EditStatus::Failed(_) => Tone::Warning,
        EditStatus::Rejected { .. } => Tone::Danger,
    }
}

fn view_param_row<'a>(
    address: MavlinkId,
    ident: &'a Ident,
    param: &'a ReportedParam,
    edit: Option<&'a ParamEdit>,
    encoding_known: bool,
) -> Element<'a, Message> {
    let value_text = edit.map_or_else(|| value_to_string(param.value()), |edit| edit.text.clone());

    let input = ui::text_input("Value", value_text)
        .on_input(move |text| Message::Edit(address, ident.clone(), text));
    let input = match edit {
        Some(edit) => input.style(style::marked_text_input(edit_tone(edit))),
        None => input,
    };

    let restore = ui::icon_button(Icon::Revert)
        .hint(match edit {
            Some(_) => format!("Restore to {}", value_to_string(param.value())),
            None => "Nothing to restore".to_string(),
        })
        .on_press_maybe(edit.map(|_| Message::Reset(address, ident.clone())));

    let uploadable = edit
        .and_then(ParamEdit::uploadable)
        .filter(|_| encoding_known);
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
    let upload = ui::icon_button(Icon::Upload)
        .hint(upload_hint)
        .on_press_maybe(uploadable.map(|_| Message::Upload(address, ident.clone())));

    row![
        ui::mono(ident.as_str()).width(160.0),
        ui::mono(value_type_name(param.value()))
            .width(50.0)
            .align_x(Alignment::End)
            .tone(Tone::Muted),
        space::horizontal().width(ui::space::M),
        input,
        restore.boxed(),
        upload.boxed(),
    ]
    .spacing(ui::space::XS)
    .align_y(Alignment::Center)
    .boxed()
}

#[cfg(test)]
mod tests;
