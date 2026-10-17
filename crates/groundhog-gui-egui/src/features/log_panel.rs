//! The log panel: one timeline of vehicles' status texts and the GCS's own
//! notices.
//!
//! Ported from the iced frontend. Messages from every vehicle are shown
//! together (tagged with the vehicle) so that alerts from a non-selected
//! vehicle are not missed. The scope can be narrowed to the selected vehicle
//! or to the GCS, and a minimum severity can be applied.

use std::time::Instant;

use groundhog_logic::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        vehicle::{StatusText, status},
    },
};
use mavio::default_dialect::enums::MavSeverity;

use super::notifications::{Level, Notice, Notifications};

/// Which messages to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Selected,
    Gcs,
}

impl Scope {
    pub fn list() -> [Scope; 3] {
        [Scope::All, Scope::Selected, Scope::Gcs]
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Scope::All => "Everything",
            Scope::Selected => "Selected vehicle",
            Scope::Gcs => "Ground station",
        })
    }
}

/// The largest number of messages rendered at once; older ones are dropped.
const MAX_ENTRIES: usize = 2000;

/// Height of a single log row: one line of text plus padding.
const LOG_ROW_HEIGHT: f32 = 22.0;

/// A filter over both vehicle texts and GCS notices.
#[derive(Debug, Clone, Copy)]
pub struct LogFilter {
    scope: Scope,
    /// The least severe message to show.
    min_severity: MavSeverity,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self {
            scope: Scope::All,
            min_severity: MavSeverity::Debug,
        }
    }
}

#[derive(Default)]
pub struct LogPanel {
    filter: LogFilter,
}

impl LogPanel {
    /// The merged timeline, filtered, oldest first.
    pub fn entries<'a>(
        &self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
        notices: &'a Notifications,
    ) -> Vec<Entry<'a>> {
        let mut entries: Vec<Entry<'a>> = Vec::new();

        if self.filter.scope != Scope::Gcs {
            for vehicle in fleet.vehicles() {
                if self.filter.scope == Scope::Selected && Some(vehicle.system) != selected {
                    continue;
                }
                entries.extend(
                    vehicle
                        .status
                        .texts
                        .iter()
                        .map(|text| Entry::Vehicle(vehicle.system, text)),
                );
            }
        }

        if self.filter.scope != Scope::Selected {
            entries.extend(notices.history().map(Entry::Gcs));
        }

        let threshold = status::rank(self.filter.min_severity);
        entries.retain(|entry| status::rank(entry.severity()) <= threshold);
        entries.sort_by_key(Entry::at);
        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }
        entries
    }

    /// The panel: scope and severity pickers above the timeline.
    pub fn view<'a>(
        &'a mut self,
        ui: &mut egui::Ui,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
        notices: &'a Notifications,
    ) {
        let scope = self.filter.scope;
        let min_severity = self.filter.min_severity;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Show:").weak());

            egui::ComboBox::from_id_salt("logs.scope")
                .selected_text(scope.to_string())
                .width(140.0)
                .show_ui(ui, |combo| {
                    for scope_choice in Scope::list() {
                        let active = scope == scope_choice;
                        if combo
                            .selectable_label(active, scope_choice.to_string())
                            .clicked()
                            && !active
                        {
                            self.filter.scope = scope_choice;
                        }
                    }
                });

            ui.label(egui::RichText::new("min severity:").weak());
            egui::ComboBox::from_id_salt("logs.severity")
                .selected_text(format!("{min_severity:?}"))
                .width(100.0)
                .show_ui(ui, |combo| {
                    for severity in status::ORDERED {
                        let active = min_severity == severity;
                        if combo
                            .selectable_label(active, format!("{severity:?}"))
                            .clicked()
                            && !active
                        {
                            self.filter.min_severity = severity;
                        }
                    }
                });
        });

        let entries = self.entries(fleet, selected, notices);
        if entries.is_empty() {
            ui.label(egui::RichText::new("Nothing logged yet.").weak());
            return;
        }

        let count = entries.len();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show_rows(ui, LOG_ROW_HEIGHT, count, |rows, range| {
                for index in range {
                    let entry = &entries[index];
                    let (rect, _) = rows.allocate_exact_size(
                        egui::vec2(rows.available_width(), LOG_ROW_HEIGHT),
                        egui::Sense::hover(),
                    );

                    let stripe = if index % 2 == 1 {
                        egui::Color32::from_rgb(0x24, 0x29, 0x30)
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    if stripe != egui::Color32::TRANSPARENT {
                        rows.painter().rect_filled(rect, 2.0, stripe);
                    }

                    let (source, message) = match entry {
                        Entry::Vehicle(id, status) => (
                            format!("{}:{}", id.0, status.component),
                            status.text.clone(),
                        ),
                        Entry::Gcs(notice) => (
                            "GCS".to_owned(),
                            match &notice.detail {
                                Some(detail) => format!("{}: {detail}", notice.title),
                                None => notice.title.clone(),
                            },
                        ),
                    };

                    let font = egui::TextStyle::Body.resolve(rows.style());
                    let painter = rows.painter();
                    painter.text(
                        rect.left_top() + egui::vec2(4.0, LOG_ROW_HEIGHT / 2.0),
                        egui::Align2::LEFT_CENTER,
                        source,
                        font.clone(),
                        rows.visuals().text_color(),
                    );
                    painter.text(
                        rect.left_top() + egui::vec2(80.0, LOG_ROW_HEIGHT / 2.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{:?}", entry.severity()),
                        font.clone(),
                        crate::style::severity::color(entry.severity()),
                    );
                    painter.text(
                        rect.left_top() + egui::vec2(184.0, LOG_ROW_HEIGHT / 2.0),
                        egui::Align2::LEFT_CENTER,
                        message,
                        font,
                        rows.visuals().text_color(),
                    );
                }
            });
    }
}

/// One line of the timeline.
#[derive(Clone, Copy)]
pub enum Entry<'a> {
    Vehicle(SystemId, &'a StatusText),
    Gcs(&'a Notice),
}

impl Entry<'_> {
    fn at(&self) -> Instant {
        match self {
            Entry::Vehicle(_, text) => text.at,
            Entry::Gcs(notice) => notice.at,
        }
    }

    fn severity(&self) -> MavSeverity {
        match self {
            Entry::Vehicle(_, text) => text.severity,
            Entry::Gcs(notice) => severity_of(notice.level),
        }
    }
}

/// Where a GCS notice ranks among status text severities.
fn severity_of(level: Level) -> MavSeverity {
    match level {
        Level::Error => MavSeverity::Error,
        Level::Warning => MavSeverity::Warning,
        Level::Success => MavSeverity::Notice,
        Level::Info => MavSeverity::Info,
    }
}
