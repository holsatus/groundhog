//! Operator notifications: transient toasts, and a history for the log panel.
//!
//! Everything the operator should know about — a failed upload, a lost link,
//! a vehicle arming — becomes a [`Notice`]. Notices are shown as toasts that
//! expire after a while (errors linger longer) and are kept in a bounded
//! history that the log panel shows next to the vehicles' status texts.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use groundhog_logic::SystemId;
use iced::{
    Alignment, Element, Length, Widget, mouse,
    widget::{column, container, mouse_area, row},
};

use crate::ui::{self, Icon, Tone, style};

/// How many notices the history keeps.
const HISTORY_LEN: usize = 500;
/// The most toasts on screen; older ones make room.
const MAX_TOASTS: usize = 5;

/// How important a notice is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warning,
    Success,
    Info,
}

impl Level {
    /// How a notice of this level is accented.
    pub fn tone(self) -> Tone {
        match self {
            Level::Error => Tone::Danger,
            Level::Warning => Tone::Warning,
            Level::Success => Tone::Success,
            Level::Info => Tone::Accent,
        }
    }

    /// How long a toast of this level stays up.
    fn lifetime(self) -> Duration {
        match self {
            Level::Error => Duration::from_secs(20),
            Level::Warning => Duration::from_secs(10),
            Level::Success | Level::Info => Duration::from_secs(5),
        }
    }
}

/// Something the operator should know.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub level: Level,
    pub title: String,
    pub detail: Option<String>,
    /// The vehicle it concerns, if any.
    pub system: Option<SystemId>,
    pub at: Instant,
}

impl Notice {
    pub fn new(level: Level, title: impl Into<String>) -> Self {
        Self {
            level,
            title: title.into(),
            detail: None,
            system: None,
            at: Instant::now(),
        }
    }

    pub fn error(title: impl Into<String>) -> Self {
        Self::new(Level::Error, title)
    }

    pub fn warning(title: impl Into<String>) -> Self {
        Self::new(Level::Warning, title)
    }

    pub fn success(title: impl Into<String>) -> Self {
        Self::new(Level::Success, title)
    }

    pub fn info(title: impl Into<String>) -> Self {
        Self::new(Level::Info, title)
    }

    pub fn detail(mut self, detail: impl ToString) -> Self {
        self.detail = Some(detail.to_string());
        self
    }

    pub fn vehicle(mut self, system: SystemId) -> Self {
        self.system = Some(system);
        self
    }
}

struct Toast {
    id: u64,
    notice: Notice,
    expires: Instant,
}

/// The notices of this session.
#[derive(Default)]
pub struct Notifications {
    history: VecDeque<Notice>,
    toasts: Vec<Toast>,
    next_id: u64,
}

#[derive(Debug, Clone)]
pub enum Message {
    Dismiss(u64),
    /// Expire toasts as of the given time.
    Tick(Instant),
}

impl Notifications {
    /// Show `notice` and keep it in the history.
    pub fn push(&mut self, notice: Notice) {
        log::log!(
            match notice.level {
                Level::Error => log::Level::Error,
                Level::Warning => log::Level::Warn,
                Level::Success | Level::Info => log::Level::Info,
            },
            "{}{}",
            notice.title,
            notice
                .detail
                .as_ref()
                .map(|detail| format!(": {detail}"))
                .unwrap_or_default()
        );

        self.history.push_back(notice.clone());
        if self.history.len() > HISTORY_LEN {
            self.history.pop_front();
        }
        self.toast(notice);
    }

    /// Show `notice` without keeping it, for news already recorded elsewhere
    /// (a vehicle's status text is in its own log).
    pub fn toast(&mut self, notice: Notice) {
        self.next_id += 1;
        self.toasts.push(Toast {
            id: self.next_id,
            expires: notice.at + notice.level.lifetime(),
            notice,
        });

        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    pub fn history(&self) -> impl DoubleEndedIterator<Item = &Notice> {
        self.history.iter()
    }

    /// Whether toasts are showing (and so need expiry ticks).
    pub fn has_toasts(&self) -> bool {
        !self.toasts.is_empty()
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Dismiss(id) => self.toasts.retain(|toast| toast.id != id),
            Message::Tick(now) => self.toasts.retain(|toast| toast.expires > now),
        }
    }

    /// The toast stack, newest at the bottom.
    pub fn view(&self) -> Element<'_, Message> {
        let toasts = self.toasts.iter().map(|toast| {
            let notice = &toast.notice;
            let body = column![ui::text(&notice.title)]
                .push(notice.detail.as_ref().map(ui::caption).boxed())
                .spacing(ui::space::XXS)
                .width(Length::Fill);

            let dismiss = ui::icon_button(Icon::Close)
                .on_press(Message::Dismiss(toast.id))
                .boxed();

            let toast = container(
                row![body, dismiss]
                    .spacing(ui::space::M)
                    .align_y(Alignment::Center),
            )
            .padding([ui::space::S, ui::space::M])
            .width(Length::Fixed(340.0))
            .style(style::toast(notice.level.tone()));

            // A toast is opaque to the pointer: what lies beneath (the map)
            // is not hovered, zoomed or dragged through it.
            mouse_area(toast)
                .interaction(mouse::Interaction::Idle)
                .boxed()
        });

        column(toasts).spacing(ui::space::S).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toasts_expire_by_level() {
        let mut notifications = Notifications::default();
        let now = Instant::now();
        notifications.push(Notice::info("hello"));
        notifications.push(Notice::error("broken"));

        notifications.update(Message::Tick(now + Duration::from_secs(6)));
        assert_eq!(notifications.toasts.len(), 1, "info expired, error lingers");
        assert_eq!(notifications.history().count(), 2, "history keeps both");
    }

    #[test]
    fn toasts_are_bounded_and_dismissable() {
        let mut notifications = Notifications::default();
        for index in 0..10 {
            notifications.toast(Notice::info(format!("{index}")));
        }
        assert_eq!(notifications.toasts.len(), MAX_TOASTS);
        assert_eq!(notifications.history().count(), 0, "toast-only");

        let id = notifications.toasts[0].id;
        notifications.update(Message::Dismiss(id));
        assert_eq!(notifications.toasts.len(), MAX_TOASTS - 1);
    }
}
