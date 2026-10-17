//! The log panel: one timeline of vehicles' status texts and the GCS's own
//! notices.
//!
//! Messages from every vehicle are shown together (tagged with the vehicle) so
//! that alerts from a non-selected vehicle are not missed. The scope can be
//! narrowed to the selected vehicle or to the GCS, and a minimum severity can
//! be applied.

use std::time::Instant;

use iced::{
    Alignment, Element, Length, Theme, Widget,
    widget::{container, pick_list, row, scrollable, text, text::Wrapping},
};
use mavio::default_dialect::enums::MavSeverity;

use super::notifications::{Level, Notice, Notifications};
use crate::{
    domain::{
        SystemId,
        fleet::FleetSnapshot,
        vehicle::{StatusText, status},
    },
    style,
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

/// Height of a single log row: one line of text plus the container padding.
const LOG_ROW_HEIGHT: f32 = 26.0;

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
        theme: &Theme,
    ) -> Element<'a, Message> {
        let mut entries = self.entries(fleet, selected, notices);

        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }

        let entries = entries.into_iter().enumerate().collect::<Vec<_>>();
        let palette = *theme.palette();

        let list = virtual_list(entries, move |&(index, entry)| {
            let color = style::severity_color(palette, entry.severity());

            let (source, message) = match entry {
                Entry::Vehicle(id, status) => (
                    format!("{}:{}", id.0, status.component),
                    status.text.clone(),
                ),
                Entry::Gcs(notice) => (
                    "GCS".to_string(),
                    match &notice.detail {
                        Some(detail) => format!("{}: {detail}", notice.title),
                        None => notice.title.clone(),
                    },
                ),
            };

            container(
                row![
                    text(source).width(Length::Fixed(72.0)),
                    text(format!("{:?}", entry.severity()))
                        .color(color)
                        .width(Length::Fixed(96.0)),
                    text(message).width(Length::Fill).wrapping(Wrapping::None),
                ]
                .spacing(8.0),
            )
            .style(if index % 2 == 0 {
                style::list_default
            } else {
                style::list_brighter
            })
            .padding(2)
            .boxed()
        })
        .item_height(|_| LOG_ROW_HEIGHT)
        .key(|(index, entry)| (*index, entry.at()))
        .spacing(0.0);

        let list = scrollable(list)
            .anchor_bottom()
            .height(Length::Fill)
            .width(Length::Fill);

        let controls = row![
            text("Show:"),
            pick_list(
                Some(self.filter.scope),
                [Scope::All, Scope::Selected, Scope::Gcs],
                |scope| scope.to_string(),
            )
            .on_select(Message::SetScope),
            text("min severity:"),
            pick_list(
                Some(self.filter.min_severity),
                status::ORDERED,
                |severity| format!("{severity:?}"),
            )
            .on_select(Message::SetMinSeverity),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center);

        container(iced::widget::column![controls, list].spacing(6.0))
            .padding(6.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .boxed()
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
