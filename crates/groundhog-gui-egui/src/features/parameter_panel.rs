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

use std::path::PathBuf;

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
    storage::parameter::{ParamFileFormat, read_parameters, write_parameters},
};
use mav_param::{Ident, Value};
use tokio::sync::mpsc;

use super::{
    Context, Effect, failure_notice, file_dialog,
    notifications::Notice,
    operations::{Operations, Ticket},
};
use crate::style;

type UploadKey = (MavlinkId, Ident);

/// What the parameter operations and file tasks report back.
#[derive(Debug)]
enum ParamEvent {
    Reloaded(MavlinkId, Ticket, Result<ParamListSummary, String>),
    Uploaded {
        address: MavlinkId,
        ident: Ident,
        ticket: Ticket,
        requested: Value,
        result: Result<Value, String>,
    },
    Saved {
        path: PathBuf,
        result: Result<(), String>,
    },
    Loaded {
        path: PathBuf,
        address: MavlinkId,
        result: Result<ParamValues, String>,
    },
}

type ParamListSummary = groundhog_logic::services::ParamListSummary;

/// A parameter filter: name pattern plus "only changed".
#[derive(Default, Debug, Clone, PartialEq)]
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

/// The panel state: filter, pending edits per component, operations in
/// flight, and the component each vehicle's viewer shows.
pub struct ParameterPanel {
    filter: ParameterFilter,
    /// Pending edits, per component and parameter. Kept here (not in the
    /// fleet) so switching components or vehicles and back keeps them.
    edits: std::collections::BTreeMap<MavlinkId, ParamEdits>,
    uploads: Operations<UploadKey>,
    reloads: Operations<MavlinkId>,
    /// The component the operator chose to view, per vehicle.
    component: std::collections::BTreeMap<SystemId, u8>,
    sender: mpsc::UnboundedSender<ParamEvent>,
    results: mpsc::UnboundedReceiver<ParamEvent>,
}

impl ParameterPanel {
    pub fn new() -> Self {
        let (sender, results) = mpsc::unbounded_channel();
        Self {
            filter: ParameterFilter::default(),
            edits: std::collections::BTreeMap::new(),
            uploads: Operations::default(),
            reloads: Operations::default(),
            component: std::collections::BTreeMap::new(),
            sender,
            results,
        }
    }

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

    /// The pending edits of `address`.
    fn edits_of(&self, address: MavlinkId) -> impl Iterator<Item = (&Ident, &ParamEdit)> {
        self.edits
            .get(&address)
            .into_iter()
            .flat_map(ParamEdits::iter)
    }

    /// Record which component's parameters are shown.
    pub fn select_component(&mut self, address: MavlinkId) {
        self.component
            .insert(address.system_id(), address.component);
    }

    /// Drain the operations' and file tasks' results.
    pub fn collect(&mut self, cx: &mut Context) {
        while let Ok(event) = self.results.try_recv() {
            match event {
                ParamEvent::Reloaded(address, ticket, result) => {
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
                            Err(error) => cx.notices.push(failure_notice(
                                "Parameter download failed".to_owned(),
                                error,
                            )),
                        }
                    }
                }
                ParamEvent::Uploaded {
                    address,
                    ident,
                    ticket,
                    requested,
                    result,
                } => {
                    let key = (address, ident.clone());
                    if !self.uploads.finish(&key, ticket) {
                        continue;
                    }

                    let outcome = self
                        .edits
                        .get_mut(&address)
                        .and_then(|edits| edits.finish_upload(&ident, requested, result));

                    match outcome {
                        Some(UploadOutcome::Failed(error)) => cx.notices.push(failure_notice(
                            format!("Setting {} failed", ident.as_str()),
                            error,
                        )),
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
                ParamEvent::Saved { path, result } => match result {
                    Ok(()) => {
                        cx.notices
                            .push(Notice::success("Parameters saved").detail(path.display()));
                        cx.request(Effect::RememberDirectory(path));
                    }
                    Err(error) => cx.notices.push(
                        Notice::error(format!("Could not save {}", path.display())).detail(error),
                    ),
                },
                ParamEvent::Loaded {
                    path,
                    address,
                    result,
                } => match result {
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
        }
    }

    /// The operator typed into a value field: an upload in flight is
    /// cancelled, and the text becomes (or stops being) a pending edit.
    pub fn on_edit(&mut self, cx: &Context, address: MavlinkId, ident: Ident, text: String) {
        self.uploads.cancel(&(address, ident.clone()));

        let current = cx
            .fleet
            .parameters(address)
            .and_then(|parameters| parameters.value(&ident));
        self.edits
            .entry(address)
            .or_default()
            .edit(ident, current, text);
    }

    /// Discard a pending edit.
    pub fn on_reset(&mut self, address: MavlinkId, ident: &Ident) {
        self.uploads.cancel(&(address, ident.clone()));
        if let Some(edits) = self.edits.get_mut(&address) {
            edits.reset(ident);
        }
    }

    /// Discard every pending edit of the component.
    pub fn on_reset_all(&mut self, address: &MavlinkId) {
        if let Some(edits) = self.edits.remove(address) {
            for (ident, _) in edits.iter() {
                self.uploads.cancel(&(*address, ident.clone()));
            }
        }
    }

    /// Upload one changed parameter.
    pub fn on_upload(&mut self, cx: &Context, address: MavlinkId, ident: Ident) {
        // The middleware bounds how many sets are in flight.
        let Some(value) = self
            .edits
            .get_mut(&address)
            .and_then(|edits| edits.begin_upload(&ident))
        else {
            return;
        };

        let client = cx.services.component(address);
        let sender = self.sender.clone();
        let request_ident = ident.clone();
        self.uploads.start(
            (address, ident.clone()),
            async move { client.set_parameter(request_ident, value).await },
            sender,
            move |ticket, result| ParamEvent::Uploaded {
                address,
                ident,
                ticket,
                requested: value,
                result: result.map_err(|error| error.to_string()),
            },
        );
    }

    /// Upload every changed, uploadable parameter of the component.
    pub fn on_upload_all(&mut self, cx: &Context, address: MavlinkId) {
        let idents: Vec<Ident> = self
            .edits
            .get(&address)
            .into_iter()
            .flat_map(ParamEdits::uploadable)
            .map(|(ident, _)| ident.clone())
            .collect();

        for ident in idents {
            self.on_upload(cx, address, ident);
        }
    }

    /// Download the component's parameter list again.
    pub fn on_reload(&mut self, cx: &Context, address: MavlinkId) {
        let client = cx.services.component(address);
        let sender = self.sender.clone();
        self.reloads.start(
            address,
            async move { client.fetch_parameters().await },
            sender,
            move |ticket, result| {
                ParamEvent::Reloaded(address, ticket, result.map_err(|error| error.to_string()))
            },
        );
    }

    /// Save the component's parameters to a file, format by extension.
    pub fn on_save_dialog(&mut self, cx: &Context, address: MavlinkId) {
        // Snapshot the values now, not when the dialog closes.
        let Some(values) = cx.fleet.parameters(address).map(Parameters::values) else {
            return;
        };
        let directory = cx.directory.map(|directory| directory.to_path_buf());

        let sender = self.sender.clone();
        tokio::spawn(async move {
            let dialog = file_dialog(directory.as_deref())
                .set_file_name("parameters.param")
                .add_filter("Mission Planner", &["param", "parm"])
                .add_filter("QGroundControl", &["params"])
                .add_filter("Groundhog INI", &["ini"]);

            if let Some(path) = dialog.save_file().await.map(|file| file.path().to_owned()) {
                let save_path = path.clone();
                let outcome = tokio::task::spawn_blocking(move || {
                    let text = write_parameters(ParamFileFormat::of(&save_path), &values, address);
                    std::fs::write(&save_path, text).map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()));

                let _ = sender.send(ParamEvent::Saved {
                    path,
                    result: outcome,
                });
            }
        });
    }

    /// Load a parameter file into pending edits (nothing is sent until the
    /// operator uploads). Untyped formats read as the reported types.
    pub fn on_load_dialog(&mut self, cx: &Context, address: MavlinkId) {
        let reported = cx
            .fleet
            .parameters(address)
            .map(Parameters::values)
            .unwrap_or_default();
        let directory = cx.directory.map(|directory| directory.to_path_buf());
        let sender = self.sender.clone();

        tokio::spawn(async move {
            let dialog = file_dialog(directory.as_deref());

            if let Some(path) = dialog.pick_file().await.map(|file| file.path().to_owned()) {
                let format_path = path.clone();
                let outcome = tokio::task::spawn_blocking(move || {
                    std::fs::read_to_string(&format_path)
                        .map_err(|error| error.to_string())
                        .and_then(|text| {
                            read_parameters(ParamFileFormat::of(&format_path), &text, |ident| {
                                reported.get(ident).copied()
                            })
                        })
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()));

                let _ = sender.send(ParamEvent::Loaded {
                    path,
                    address,
                    result: outcome,
                });
            }
        });
    }

    /// The panel: component picker, toolbar, filter, and the list.
    pub fn view(
        &mut self,
        ui: &mut egui::Ui,
        fleet: &FleetSnapshot,
        selected: Option<SystemId>,
        cx: &mut Context,
    ) {
        let Some(vehicle) = selected.and_then(|system| fleet.vehicle(system)) else {
            ui.label("No vehicle selected.");
            return;
        };

        let system = vehicle.system;
        let component = self.selected_component(vehicle);
        let address = vehicle.target_for(component);
        let parameters = vehicle.parameters(component);
        let encoding_known = vehicle.param_encoding(component).is_some();

        // Which component's parameters are shown; the autopilot is marked.
        let component_ids = vehicle.component_ids();
        if !component_ids.is_empty() {
            let autopilot = vehicle.autopilot_component();
            egui::ComboBox::from_id_salt("parameters.component")
                .selected_text(if Some(component) == autopilot {
                    format!("Component {component} (autopilot)")
                } else {
                    format!("Component {component}")
                })
                .width(ui.available_width())
                .show_ui(ui, |combo| {
                    for id in component_ids {
                        let label = if Some(id) == autopilot {
                            format!("Component {id} (autopilot)")
                        } else {
                            format!("Component {id}")
                        };
                        if combo.selectable_label(component == id, label).clicked() {
                            self.select_component(MavlinkId {
                                system: system.0,
                                component: id,
                            });
                        }
                    }
                });
        }

        let downloading = self.reloads.is_running(&address)
            || parameters
                .is_some_and(|parameters| *parameters.download() == DownloadState::InProgress);
        let pending = self
            .edits_of(address)
            .filter(|(_, edit)| edit.uploadable().is_some())
            .count();

        // Toolbar: transfers, files, and discarding edits.
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !downloading,
                    egui::Button::new(if downloading {
                        "Downloading…"
                    } else {
                        "Download parameters"
                    }),
                )
                .clicked()
            {
                self.on_reload(cx, address);
            }
            if ui
                .add_enabled(
                    pending > 0 && encoding_known,
                    egui::Button::new(format!("Upload {pending} changed")),
                )
                .clicked()
            {
                self.on_upload_all(cx, address);
            }
            if ui.button("Save to file…").clicked() {
                self.on_save_dialog(cx, address);
            }
            if ui.button("Load from file…").clicked() {
                self.on_load_dialog(cx, address);
            }
            if ui
                .add_enabled(pending > 0, egui::Button::new("Discard changes"))
                .clicked()
            {
                self.on_reset_all(&address);
            }
        });

        ui.add(
            egui::TextEdit::singleline(&mut self.filter.ident_pattern)
                .hint_text("Filter parameters")
                .desired_width(ui.available_width()),
        );

        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut self.filter.only_changed, "Only changed")
                .clicked()
            {
                // Nothing else to do: the list reads the filter as it is.
            }
        });

        // The encoding warning, while values may display wrongly.
        if !encoding_known && parameters.is_some_and(|parameters| !parameters.is_empty()) {
            ui.label(
                egui::RichText::new(
                    "The component's parameter encoding is not known yet, so values may \
                     display wrongly and uploads are disabled.",
                )
                .color(style::severity::WARNING)
                .size(11.0),
            );
        }

        // Download progress, and the failure of the last download.
        let (got, expected) = parameters.map_or((0, 0), |parameters| {
            (
                parameters.loading().loaded(),
                parameters.loading().expected(),
            )
        });
        let complete = parameters.is_some_and(|parameters| parameters.loading().is_complete());
        ui.horizontal(|ui| {
            ui.add(
                egui::ProgressBar::new(got as f32 / f32::from(expected.max(1).max(1)))
                    .desired_height(8.0),
            );
            let fill = if complete {
                style::map::HOME
            } else {
                style::map::SELECTED
            };
            ui.add(egui::Label::new(
                egui::RichText::new(format!("{got}/{expected}"))
                    .color(fill)
                    .small(),
            ));
        });
        if let Some(DownloadState::Failed(error)) = parameters.map(Parameters::download) {
            ui.label(
                egui::RichText::new(format!("Last download: {error}"))
                    .color(style::severity::CRITICAL)
                    .size(11.0),
            );
        }

        let mut actions = Vec::new();
        view_param_list(ui, address, parameters, self, encoding_known, &mut actions);
        self.apply_row_actions(&actions, cx);
    }
}

/// One row's interaction, deferred while the list is drawn.
enum ParamRowAction {
    Edit(MavlinkId, Ident, String),
    Reset(MavlinkId, Ident),
    Upload(MavlinkId, Ident),
}

impl ParameterPanel {
    /// Apply the row interactions the list collected.
    fn apply_row_actions(&mut self, actions: &[ParamRowAction], cx: &mut Context) {
        for action in actions {
            match action {
                ParamRowAction::Edit(address, ident, text) => {
                    self.on_edit(cx, *address, ident.clone(), text.clone());
                }
                ParamRowAction::Reset(address, ident) => {
                    self.on_reset(*address, ident);
                }
                ParamRowAction::Upload(address, ident) => {
                    self.on_upload(cx, *address, ident.clone());
                }
            }
        }
    }
}

/// One line of the list: a section header, or one parameter.
enum RowItem {
    Section {
        name: Option<String>,
        separator: bool,
    },
    Param {
        ident: Ident,
        reported: ReportedParam,
        edit: Option<ParamEdit>,
    },
}

/// Height of a single row; sections get the same height, so the list
/// virtualises uniformly.
const ROW_HEIGHT: f32 = 30.0;
const NAME_WIDTH: f32 = 168.0;
const TYPE_WIDTH: f32 = 46.0;

/// The visible list: sections and filtered parameters, in report order.
fn view_param_list(
    ui: &mut egui::Ui,
    address: MavlinkId,
    parameters: Option<&Parameters>,
    panel: &ParameterPanel,
    encoding_known: bool,
    actions: &mut Vec<ParamRowAction>,
) {
    let Some(parameters) = parameters else { return };

    // The filtered sequence, with each parameter's edit folded in.
    let pattern = panel.filter.ident_pattern.to_lowercase();
    let mut items: Vec<RowItem> = Vec::with_capacity(parameters.len() + 64);
    let mut section: Option<&str> = None;

    for (ident, param) in parameters.iter() {
        if !panel
            .filter
            .matches(ident, panel.edit(address, ident).is_some(), &pattern)
        {
            continue;
        }

        let this_section = group_of(ident);
        if this_section != section {
            let separator = section.is_some();
            if separator || this_section.is_some() {
                items.push(RowItem::Section {
                    name: this_section.map(str::to_owned),
                    separator,
                });
            }
            section = this_section;
        }

        items.push(RowItem::Param {
            ident: ident.clone(),
            reported: param.clone(),
            edit: panel.edit(address, ident).cloned(),
        });
    }

    let count = items.len();
    if count == 0 {
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show_rows(ui, ROW_HEIGHT, count, |rows, range| {
            for index in range {
                match &items[index] {
                    RowItem::Section { name, separator } => {
                        let (rect, _) = rows.allocate_exact_size(
                            egui::vec2(rows.available_width(), ROW_HEIGHT),
                            egui::Sense::hover(),
                        );
                        let painter = rows.painter();
                        if *separator {
                            painter.hline(
                                rect.x_range(),
                                rect.top() + 4.0,
                                rows.visuals().widgets.noninteractive.bg_stroke,
                            );
                        }
                        if let Some(name) = name {
                            painter.text(
                                rect.left_top() + egui::vec2(4.0, ROW_HEIGHT / 2.0 + 6.0),
                                egui::Align2::LEFT_CENTER,
                                format!("[{name}]"),
                                egui::FontId::monospace(20.0),
                                rows.visuals().strong_text_color(),
                            );
                        }
                    }
                    RowItem::Param {
                        ident,
                        reported,
                        edit,
                    } => {
                        draw_param_row(
                            rows,
                            address,
                            ident,
                            reported,
                            edit.as_ref(),
                            encoding_known,
                            actions,
                        );
                    }
                }
            }
        });
}

/// One parameter's row: name, type, value editor, and restore/upload buttons.
fn draw_param_row(
    ui: &mut egui::Ui,
    address: MavlinkId,
    ident: &Ident,
    param: &ReportedParam,
    edit: Option<&ParamEdit>,
    encoding_known: bool,
    actions: &mut Vec<ParamRowAction>,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::hover(),
    );

    let value_text = match edit {
        Some(edit) => edit.text.clone(),
        None => value_to_string(param.value()),
    };

    let _highlight = edit.map(|edit| match &edit.status {
        EditStatus::Pending if edit.value.is_some() => style::map::HOME,
        EditStatus::Pending => style::severity::DEBUG,
        EditStatus::Uploading => style::map::SELECTED,
        EditStatus::Failed(_) => style::severity::WARNING,
        EditStatus::Rejected { .. } => style::severity::ERROR,
    });

    let _ = rect;
    let input_height = ROW_HEIGHT - 4.0;

    ui.horizontal(|row| {
        row.spacing_mut().item_spacing.x = 8.0;

        row.add_sized(
            [NAME_WIDTH, input_height],
            egui::Label::new(egui::RichText::new(ident.as_str()).monospace()),
        );
        row.add_sized(
            [TYPE_WIDTH, input_height],
            egui::Label::new(
                egui::RichText::new(value_type_name(param.value()))
                    .color(style::severity::DEBUG)
                    .monospace(),
            ),
        );

        // The value input, with the edit's highlight on request.
        let id_salt = egui::Id::new((
            "parameter.value",
            address.system,
            address.component,
            ident.as_str(),
        ));
        let mut buffer = value_text;
        let changed = row
            .add(
                egui::TextEdit::singleline(&mut buffer)
                    .id_salt(id_salt)
                    .hint_text("Value"),
            )
            .changed();

        if changed {
            actions.push(ParamRowAction::Edit(address, ident.clone(), buffer));
        }

        // Restore to the reported value.
        let restore_hint = match edit {
            Some(_) => format!("Restore to {}", value_to_string(param.value())),
            None => "Nothing to restore".to_owned(),
        };
        if row
            .add_enabled(
                edit.is_some(),
                egui::Button::new("⤺")
                    .small()
                    .min_size(egui::vec2(26.0, input_height)),
            )
            .on_hover_text(restore_hint)
            .clicked()
        {
            actions.push(ParamRowAction::Reset(address, ident.clone()));
        }

        // Upload the pending edit.
        let uploadable = edit
            .and_then(ParamEdit::uploadable)
            .filter(|_| encoding_known);
        let upload_hint = match (edit.map(|edit| &edit.status), uploadable) {
            _ if edit.is_some_and(ParamEdit::is_uploading) => "Uploading…".to_owned(),
            (Some(EditStatus::Failed(error)), _) => format!("Failed: {error}"),
            (Some(EditStatus::Rejected { reported }), _) => {
                format!("The vehicle kept {}", value_to_string(*reported))
            }
            (_, Some(value)) => format!("Upload as {}", value_to_string(value)),
            (Some(_), None) if !encoding_known => "Encoding unknown; uploads disabled".to_owned(),
            _ => "Nothing to upload".to_owned(),
        };

        if row
            .add_enabled(
                uploadable.is_some(),
                egui::Button::new("↥")
                    .small()
                    .min_size(egui::vec2(26.0, input_height)),
            )
            .on_hover_text(upload_hint)
            .clicked()
        {
            actions.push(ParamRowAction::Upload(address, ident.clone()));
        }
    });
}
