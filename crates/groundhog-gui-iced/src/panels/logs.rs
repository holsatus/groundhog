//! The log panel: one timeline of vehicles' status texts and the GCS's own
//! notices.
//!
//! Messages from every vehicle are shown together (tagged with the vehicle) so
//! that alerts from a non-selected vehicle are not missed. The scope can be
//! narrowed to the selected vehicle or to the GCS, and a minimum severity can
//! be applied.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use groundhog_logic::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        vehicle::{StatusText, status},
    },
};
use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{column, container, row, scrollable, tooltip},
};
use mavio::default_dialect::enums::MavSeverity;

use super::{
    Context, Panel, ViewContext,
    notifications::{Level, Notice, Notifications},
};
use crate::{
    ui::{self, TextExt, Tone},
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

/// The source column: room for a system and a component number in monospace,
/// so the severities beside them line up.
const SOURCE_WIDTH: f32 = 56.0;

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
///
/// The row is one line high, so a message that does not fit is cut. The
/// tooltip is the whole record of it: when it was received, its source, its
/// severity in full, and every character of the message.
fn timeline_row<'a, M: 'a>(
    index: usize,
    received: SystemTime,
    source: String,
    severity: MavSeverity,
    message: String,
) -> impl Widget<M> + 'a {
    let detail = column![
        row![
            ui::mono(time_of_day(received)).tone(Tone::Muted),
            ui::mono(source.clone()).tone(Tone::Muted),
            ui::severity_name(severity),
        ]
        .spacing(ui::space::M),
        ui::text(message.clone()).width(Length::Fit.max(ui::size::TOOLTIP_MAX_WIDTH)),
    ]
    .spacing(ui::space::S);

    let line = ui::list_row(
        index,
        row![
            ui::mono(source)
                .tone(Tone::Muted)
                .single_line()
                .width(Length::Fixed(SOURCE_WIDTH)),
            ui::severity(severity),
            ui::text(message).single_line().width(Length::Fill),
        ]
        .spacing(ui::space::S),
    );

    ui::tooltip_with(line, detail, tooltip::Position::FollowCursor)
}

/// The clock an entry is shown by.
///
/// An `Instant` has no epoch, so the view anchors one to the wall clock as it
/// is built: the entry's time is the wall clock less what has passed since.
struct WallClock {
    wall: SystemTime,
    monotonic: Instant,
}

impl WallClock {
    fn now() -> Self {
        Self {
            wall: SystemTime::now(),
            monotonic: Instant::now(),
        }
    }

    /// When the moment `at` was, on the wall clock.
    fn at(&self, at: Instant) -> SystemTime {
        self.wall
            .checked_sub(self.monotonic.saturating_duration_since(at))
            .unwrap_or(self.wall)
    }
}

/// The time of day of `time` in UTC, to the second.
fn time_of_day(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let of_day = seconds % 86_400;

    format!(
        "{:02}:{:02}:{:02} UTC",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
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

impl Panel for LogPanel {
    type Message = Message;

    fn update(&mut self, message: Message, _cx: &mut Context<Message>) -> Task<Message> {
        match message {
            Message::SetScope(scope) => self.filter.scope = scope,
            Message::SetMinSeverity(severity) => self.filter.min_severity = severity,
        }
        Task::none()
    }

    fn view<'a>(&'a self, cx: ViewContext<'a>) -> Element<'a, Message> {
        self.view_log(cx.fleet, cx.selected, cx.notices).boxed()
    }
}

impl LogPanel {
    pub fn new() -> Self {
        Self::default()
    }

    fn view_log<'a>(
        &'a self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
        notices: &'a Notifications,
    ) -> impl Widget<Message> + 'a {
        let mut entries = self.entries(fleet, selected, notices);

        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }

        let count = entries.len();
        let entries = entries.into_iter().enumerate().collect::<Vec<_>>();
        let row_height = ui::size::list_row();
        let clock = WallClock::now();
        let list = virtual_list(entries, move |&(index, entry)| {
            let (source, message) = match entry {
                Entry::Vehicle(system, status) => (
                    format!("{}:{}", system.0, status.component),
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

            timeline_row(
                index,
                clock.at(entry.at()),
                source,
                entry.severity(),
                message,
            )
        })
        .item_height(move |_| row_height)
        .key(|(index, entry)| (*index, entry.at()));

        // The filter is what the panel is about: whose messages, and how
        // severe at least.
        let filter = row![
            ui::pick_list(
                Some(self.filter.scope),
                [Scope::All, Scope::Selected, Scope::Gcs],
                Scope::to_string,
            )
            .on_select(Message::SetScope)
            .width(Length::Fixed(180.0)),
            ui::pick_list(
                Some(self.filter.min_severity),
                status::ORDERED,
                |severity| format!("{severity:?} and up"),
            )
            .on_select(Message::SetMinSeverity)
            .width(Length::Fixed(180.0)),
        ]
        .spacing(ui::space::S)
        .align_y(Alignment::Center);
        let header = ui::header(
            filter,
            [ui::caption(match count {
                1 => "1 message".to_string(),
                count => format!("{count} messages"),
            })
            .tone(Tone::Muted)
            .boxed()],
            [],
        );

        let body = if count == 0 {
            container(ui::text("Nothing to show yet.").tone(Tone::Muted))
                .center(Length::Fill)
                .boxed()
        } else {
            scrollable(list)
                .anchor_bottom()
                .height(Length::Fill)
                .width(Length::Fill)
                .boxed()
        };

        ui::panel_with(header, body)
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A clock time is the time of day in UTC, whatever the day.
    #[test]
    fn the_time_of_day_is_utc() {
        let time = UNIX_EPOCH + Duration::from_secs(2 * 86_400 + 3_661);
        assert_eq!(time_of_day(time), "01:01:01 UTC");
    }

    /// The tooltip shows when the entry was received, not when it was opened:
    /// both are anchored to the wall clock when the view is built.
    #[test]
    fn an_entry_keeps_the_time_it_was_received() {
        let monotonic = Instant::now();
        let clock = WallClock {
            wall: UNIX_EPOCH + Duration::from_secs(3_600),
            monotonic,
        };

        let received = clock.at(monotonic - Duration::from_secs(90));

        assert_eq!(time_of_day(received), "00:58:30 UTC");
    }
}
