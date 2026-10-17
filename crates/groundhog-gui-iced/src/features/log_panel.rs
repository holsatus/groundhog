//! The log panel: one timeline of vehicles' status texts and the GCS's own
//! notices.
//!
//! Messages from every vehicle are shown together (tagged with the vehicle) so
//! that alerts from a non-selected vehicle are not missed. The scope can be
//! narrowed to the selected vehicle or to the GCS, and a minimum severity can
//! be applied.

use std::time::Instant;

use groundhog_logic::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        vehicle::{StatusText, status},
    },
};
use iced::{
    Alignment, Element, Length,
    widget::{column, row, scrollable},
};
use mavio::default_dialect::enums::MavSeverity;

use super::notifications::{Level, Notice, Notifications};
use crate::{
    ui::{self, TextExt},
    widget::virtualized::virtual_list,
};

/// Which messages to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Selected,
    Gcs,
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

#[derive(Debug, Clone, Copy)]
pub struct Filter {
    scope: Scope,
    /// The least severe message to show.
    min_severity: MavSeverity,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            scope: Scope::All,
            min_severity: MavSeverity::Debug,
        }
    }
}

#[derive(Default)]
pub struct LogPanel {
    filter: Filter,
}

#[derive(Debug, Clone)]
pub enum Message {
    SetScope(Scope),
    SetMinSeverity(MavSeverity),
}

/// One line of the timeline.
#[derive(Clone, Copy)]
enum Entry<'a> {
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

/// One line of a timeline of messages: where it came from, how severe it is,
/// and what it says.
fn timeline_row<'a, M: 'a>(
    index: usize,
    source: String,
    severity: MavSeverity,
    message: String,
) -> Element<'a, M> {
    ui::list_row(
        index,
        row![
            ui::text(source).width(Length::Fixed(72.0)),
            ui::severity(severity).width(Length::Fixed(96.0)),
            ui::text(message).single_line().width(Length::Fill),
        ]
        .spacing(ui::space::M),
    )
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

impl LogPanel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::SetScope(scope) => self.filter.scope = scope,
            Message::SetMinSeverity(severity) => self.filter.min_severity = severity,
        }
    }

    pub fn view<'a>(
        &'a self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
        notices: &'a Notifications,
    ) -> Element<'a, Message> {
        let mut entries = self.entries(fleet, selected, notices);

        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }

        let entries = entries.into_iter().enumerate().collect::<Vec<_>>();
        let list = virtual_list(entries, |&(index, entry)| match entry {
            Entry::Vehicle(system, status) => timeline_row(
                index,
                format!("{}:{}", system.0, status.component),
                status.severity,
                status.text.clone(),
            ),
            Entry::Gcs(notice) => timeline_row(
                index,
                "GCS".to_string(),
                entry.severity(),
                match &notice.detail {
                    Some(detail) => format!("{}: {detail}", notice.title),
                    None => notice.title.clone(),
                },
            ),
        })
        .item_height(|_| ui::size::LIST_ROW)
        .key(|(index, entry)| (*index, entry.at()));

        let controls = row![
            ui::text("Show"),
            ui::pick_list(
                Some(self.filter.scope),
                [Scope::All, Scope::Selected, Scope::Gcs],
                Scope::to_string,
            )
            .on_select(Message::SetScope)
            .width(Length::Fixed(200.0)),
            ui::text("at least"),
            ui::pick_list(
                Some(self.filter.min_severity),
                status::ORDERED,
                |severity| format!("{severity:?}"),
            )
            .on_select(Message::SetMinSeverity)
            .width(Length::Fixed(140.0)),
        ]
        .spacing(ui::space::M)
        .align_y(Alignment::Center);

        ui::panel(
            column![
                controls,
                scrollable(list)
                    .anchor_bottom()
                    .height(Length::Fill)
                    .width(Length::Fill)
            ]
            .spacing(ui::space::S),
        )
    }

    /// The entries matching the filter, oldest first.
    fn entries<'a>(
        &self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
        notices: &'a Notifications,
    ) -> Vec<Entry<'a>> {
        let mut entries = Vec::new();

        let vehicles = match self.filter.scope {
            Scope::All => true,
            Scope::Selected => true,
            Scope::Gcs => false,
        };

        if vehicles {
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
        entries
    }
}
