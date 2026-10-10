//! The log panel: a merged, filterable view of vehicles' status messages.
//!
//! Messages from every vehicle are shown together (tagged with the vehicle) so
//! that alerts from a non-focused vehicle are not missed. The scope can be
//! narrowed to the focused vehicle, and a minimum severity can be applied.

use std::collections::BTreeMap;

use iced::{
    Alignment, Color, Element, Length, Theme, Widget,
    theme::Palette,
    widget::{Column, container, pick_list, row, scrollable, text},
};
use mavio::default_dialect::enums::MavSeverity;

use crate::{
    domain::vehicle::{StatusText, Vehicle},
    foundation::MavlinkId,
    style,
};

/// Which vehicles' messages to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Focused,
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Scope::All => f.write_str("All vehicles"),
            Scope::Focused => f.write_str("Focused vehicle"),
        }
    }
}

/// Severities from most to least severe, for the picker.
const SEVERITIES: [MavSeverity; 8] = [
    MavSeverity::Emergency,
    MavSeverity::Alert,
    MavSeverity::Critical,
    MavSeverity::Error,
    MavSeverity::Warning,
    MavSeverity::Notice,
    MavSeverity::Info,
    MavSeverity::Debug,
];

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

pub struct LogPanel {
    filter: Filter,
}

#[derive(Debug, Clone)]
pub enum Message {
    SetScope(Scope),
    SetMinSeverity(MavSeverity),
}

impl LogPanel {
    pub fn new() -> Self {
        Self {
            filter: Filter::default(),
        }
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::SetScope(scope) => self.filter.scope = scope,
            Message::SetMinSeverity(severity) => self.filter.min_severity = severity,
        }
    }

    pub fn view<'a>(
        &'a self,
        vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
        focused: Option<MavlinkId>,
        theme: &Theme,
    ) -> Element<'a, Message> {
        let mut entries = self.entries(vehicles, focused);

        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }

        let palette = theme.palette();

        let rows = entries
            .iter()
            .enumerate()
            .map(|(index, (id, status))| {
                let color = severity_color(palette, status.severity);

                container(
                    row![
                        text(format!("{}:{}", id.system, id.component)).width(Length::Fixed(56.0)),
                        text(format!("{:?}", status.severity))
                            .color(color)
                            .width(Length::Fixed(96.0)),
                        text(&status.text).width(Length::Fill),
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
            .collect::<Vec<Element<'_, Message>>>();

        let list = scrollable(Column::from_vec(rows))
            .anchor_bottom()
            .height(Length::Fill)
            .width(Length::Fill);

        let controls = row![
            text("Show:"),
            pick_list(
                Some(self.filter.scope),
                [Scope::All, Scope::Focused],
                |scope| scope.to_string(),
            )
            .on_select(Message::SetScope),
            text("min severity:"),
            pick_list(
                Some(self.filter.min_severity),
                SEVERITIES,
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

    /// Collect the messages matching the current filter, oldest first.
    fn entries<'a>(
        &self,
        vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
        focused: Option<MavlinkId>,
    ) -> Vec<(MavlinkId, &'a StatusText)> {
        let mut entries = Vec::new();

        let mut push = |id: MavlinkId, vehicle: &'a Vehicle| {
            for status in &vehicle.status_texts {
                if (status.severity as u8) <= (self.filter.min_severity as u8) {
                    entries.push((id, status));
                }
            }
        };

        match self.filter.scope {
            Scope::All => {
                for (id, vehicle) in vehicles {
                    push(*id, vehicle);
                }
            }
            Scope::Focused => {
                if let Some(id) = focused
                    && let Some(vehicle) = vehicles.get(&id)
                {
                    push(id, vehicle);
                }
            }
        }

        entries.sort_by_key(|(_, status)| status.at);
        entries
    }
}

fn severity_color(palette: &Palette, severity: MavSeverity) -> Color {
    match severity {
        MavSeverity::Emergency
        | MavSeverity::Alert
        | MavSeverity::Critical
        | MavSeverity::Error => palette.danger.base.color,
        MavSeverity::Warning | MavSeverity::Notice => palette.warning.base.color,
        MavSeverity::Info => palette.primary.base.color,
        MavSeverity::Debug => palette.secondary.base.color,
    }
}
