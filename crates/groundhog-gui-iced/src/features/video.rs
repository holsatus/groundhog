//! The video panel: where the camera's stream comes from, how it stands,
//! and the stream itself.
//!
//! While the stream is on, the panel owns its [`Receiver`]. The receiver's
//! state arrives as a message for every decoded frame, which is also what
//! makes the application draw each frame as soon as it is decoded: the
//! video widget reads the newest frame from the receiver's slot whenever
//! the view is built. A message of a receiver since replaced is ignored.
//!
//! The source and the low-latency switch are persisted by the application
//! (see [`Event`]), and so is whether the stream was on, so it comes back
//! on at the next start.

use groundhog_video::{Config, FrameSlot, Receiver, Source, State, Status};
use iced::{
    Alignment, Element, Length, Padding, Task, Widget, mouse,
    widget::{column, container, mouse_area, row, stack},
};

use crate::{
    config::VideoSettings,
    ui::{self, Icon, TextExt, Tone},
    widget::video::video,
};

#[derive(Debug, Clone)]
pub enum Message {
    EditSource(String),
    LowLatency(bool),
    /// Receive the source in the field.
    Start,
    Stop,
    /// Show or hide the source's form while the stream is on.
    ToggleForm,
    /// Show or hide the stream's details over the picture.
    ToggleDetails,
    /// Show the picture alone, filling the screen, or leave it.
    FullScreen(bool),
    /// Receiver `id`'s state changed: a frame arrived, or its status.
    Updated(u64, State),
}

/// What the application keeps of the panel.
pub enum Event {
    /// The settings to persist.
    Changed(VideoSettings),
}

/// How a receiver's status reads, in a badge.
fn status_badge(status: &Status) -> (&'static str, Tone) {
    match status {
        Status::Connecting => ("Connecting", Tone::Muted),
        Status::Streaming { .. } => ("Live", Tone::Success),
        Status::Retrying { .. } => ("No video", Tone::Warning),
        Status::Failed(_) => ("Failed", Tone::Danger),
    }
}

/// The widest the floating cards over the picture get.
const FORM_WIDTH: f32 = 420.0;

pub struct VideoPanel {
    settings: VideoSettings,
    /// The source's field, as typed.
    source: String,
    receiving: Option<Receiving>,
    /// Whether the source's form shows while the stream is on.
    form: bool,
    /// Whether the stream's details show over the picture.
    details: bool,
    /// Whether the picture fills the screen, alone.
    full_screen: bool,
    /// The identity the next receiver gets.
    next: u64,
}

struct Receiving {
    id: u64,
    receiver: Receiver,
    state: State,
}

impl VideoPanel {
    /// The panel for `settings`, receiving already if the stream was on.
    pub fn new(settings: VideoSettings) -> (Self, Task<Message>) {
        let mut panel = Self {
            source: settings.uri.clone(),
            settings,
            receiving: None,
            form: false,
            details: false,
            full_screen: false,
            next: 0,
        };
        let task = if panel.settings.enabled {
            panel.start()
        } else {
            Task::none()
        };
        (panel, task)
    }

    /// Whether the picture fills the screen, alone. Never without a
    /// stream.
    pub fn is_full_screen(&self) -> bool {
        self.full_screen && self.receiving.is_some()
    }

    /// Where the frames arrive, while the stream is on.
    pub fn slot(&self) -> Option<&FrameSlot> {
        self.receiving
            .as_ref()
            .map(|receiving| receiving.receiver.slot())
    }

    pub fn update(&mut self, message: Message) -> (Task<Message>, Option<Event>) {
        match message {
            Message::EditSource(source) => {
                self.source = source;
                (Task::none(), None)
            }
            Message::LowLatency(low_latency) => {
                self.settings.low_latency = low_latency;
                // A running stream takes the new setting at once.
                let task = if self.receiving.is_some() {
                    self.start()
                } else {
                    Task::none()
                };
                (task, Some(Event::Changed(self.settings.clone())))
            }
            Message::Start => {
                if Source::parse(&self.source).is_err() {
                    return (Task::none(), None);
                }
                self.settings.uri = self.source.trim().to_string();
                self.settings.enabled = true;
                self.form = false;
                (self.start(), Some(Event::Changed(self.settings.clone())))
            }
            Message::Stop => {
                self.receiving = None;
                self.full_screen = false;
                self.settings.enabled = false;
                (Task::none(), Some(Event::Changed(self.settings.clone())))
            }
            Message::ToggleForm => {
                self.form = !self.form;
                (Task::none(), None)
            }
            Message::ToggleDetails => {
                self.details = !self.details;
                (Task::none(), None)
            }
            Message::FullScreen(full_screen) => {
                self.full_screen = full_screen && self.receiving.is_some();
                (Task::none(), None)
            }
            Message::Updated(id, state) => {
                if let Some(receiving) = self.receiving.as_mut().filter(|it| it.id == id) {
                    receiving.state = state;
                }
                (Task::none(), None)
            }
        }
    }

    /// Start receiving the settings' source, replacing any receiver before.
    fn start(&mut self) -> Task<Message> {
        let id = self.next;
        self.next += 1;

        let receiver = Receiver::start(Config {
            uri: self.settings.uri.clone(),
            low_latency: self.settings.low_latency,
            jitter_buffer_ms: self.settings.jitter_buffer_ms,
        });
        let watcher = receiver.state();
        let state = watcher.borrow().clone();
        self.receiving = Some(Receiving {
            id,
            receiver,
            state,
        });

        // Ends when the receiver is dropped and its supervisor finishes.
        let updates = futures::stream::unfold(watcher, |mut watcher| async move {
            watcher.changed().await.ok()?;
            let state = watcher.borrow_and_update().clone();
            Some((state, watcher))
        });
        Task::stream(updates).map(move |state| Message::Updated(id, state))
    }

    pub fn view<'a>(&'a self) -> Element<'a, Message> {
        let Some(receiving) = &self.receiving else {
            let header = ui::header(
                ui::mono(&self.settings.uri).single_line(),
                [ui::badge("Off", Tone::Muted)],
                [],
            );
            return ui::panel_with(header, self.view_form());
        };

        // The picture fills the panel; what the panel would otherwise put
        // around it floats over its corner, the details on demand.
        let picture = stack![
            video(receiving.receiver.slot()),
            container(self.view_status(&receiving.state.status)).center(Length::Fill),
        ];

        let (label, tone) = status_badge(&receiving.state.status);
        // Hovered, the indicator explains itself; pressed (as a finger
        // must, having no hover), it keeps the details open.
        let indicator = ui::tooltip(
            mouse_area(ui::badge(label, tone))
                .on_press(Message::ToggleDetails)
                .interaction(mouse::Interaction::Pointer),
            self.details_text(&receiving.state.status),
            iced::widget::tooltip::Position::Bottom,
        );
        let chip = ui::floating(
            row![
                indicator,
                ui::icon_button(Icon::Edit)
                    .chosen(self.form)
                    .hint("Change the source")
                    .on_press(Message::ToggleForm)
                    .boxed(),
                ui::icon_button(Icon::Stop)
                    .hint("Stop the video")
                    .on_press(Message::Stop)
                    .boxed(),
                ui::icon_button(Icon::Maximize)
                    .hint("Full screen (Esc to leave)")
                    .on_press(Message::FullScreen(true))
                    .boxed(),
            ]
            .spacing(ui::space::XS)
            .align_y(Alignment::Center),
        )
        .padding(Padding {
            left: ui::space::M,
            ..Padding::new(ui::space::XS)
        });

        let card = |content: Element<'a, Message>| {
            ui::floating(content)
                .padding(ui::space::M)
                .width(Length::Shrink.max(FORM_WIDTH))
                .boxed()
        };
        let overlay = column![
            chip,
            self.details
                .then(|| card(ui::text(self.details_text(&receiving.state.status)).boxed())),
            self.form.then(|| card(self.view_form())),
        ]
        .spacing(ui::space::S)
        .padding(ui::space::S);

        ui::picture(stack![picture, overlay])
    }

    /// The picture alone, filling the screen, with the stream's state and a
    /// way out (also Escape, or back) in its corner, clear of the platform's
    /// chrome by `insets`.
    pub fn view_full_screen(&self, insets: Padding) -> Element<'_, Message> {
        let Some(receiving) = &self.receiving else {
            return iced::widget::Space::new().boxed();
        };

        let (label, tone) = status_badge(&receiving.state.status);
        let chip = ui::floating(
            row![
                ui::badge(label, tone),
                ui::icon_button(Icon::Minimize)
                    .hint("Leave full screen (Esc)")
                    .on_press(Message::FullScreen(false))
                    .boxed(),
            ]
            .spacing(ui::space::S)
            .align_y(Alignment::Center),
        )
        .padding(Padding {
            left: ui::space::M,
            ..Padding::new(ui::space::XS)
        });
        let corner = container(chip).padding(Padding {
            top: insets.top + ui::space::S,
            right: insets.right + ui::space::S,
            bottom: insets.bottom + ui::space::S,
            left: insets.left + ui::space::S,
        });

        ui::screen(stack![
            video(receiving.receiver.slot()),
            container(self.view_status(&receiving.state.status)).center(Length::Fill),
            corner,
        ])
    }

    /// The stream's details: where it comes from, what it is, and how it is
    /// received.
    fn details_text(&self, status: &Status) -> String {
        let mut lines = vec![self.settings.uri.clone()];
        if let Status::Streaming {
            width,
            height,
            decoder,
        } = status
        {
            let mut line = format!("{width} × {height}");
            if let Some(decoder) = decoder {
                line.push_str(&format!(", decoded by {decoder}"));
            }
            lines.push(line);
        }
        lines.push(if self.settings.low_latency {
            "Low latency: no jitter buffer".to_string()
        } else {
            format!("Jitter buffer of {} ms", self.settings.jitter_buffer_ms)
        });
        lines.join("\n")
    }

    /// What shows over the picture while there is none: why. White on the
    /// picture's black, in either theme.
    fn view_status(&self, status: &Status) -> Element<'_, Message> {
        let text = match status {
            Status::Streaming { .. } => return iced::widget::Space::new().boxed(),
            Status::Connecting => format!("Waiting for video from {}…", self.settings.uri),
            Status::Retrying { reason, retry_in } => {
                format!("{reason}. Trying again in {} s.", retry_in.as_secs())
            }
            Status::Failed(reason) => reason.clone(),
        };
        ui::text(text).boxed()
    }

    /// The source's form: its URI, the low-latency switch, and starting or
    /// stopping the stream.
    fn view_form(&self) -> Element<'_, Message> {
        let problem = match Source::parse(&self.source) {
            Err(problem) if !self.source.trim().is_empty() => Some(problem),
            _ => None,
        };
        let valid = problem.is_none() && !self.source.trim().is_empty();

        let mut field =
            ui::text_input("udp://:5600", self.source.clone()).on_input(Message::EditSource);
        if valid {
            field = field.on_submit(Message::Start);
        }

        let start = if self.receiving.is_some() {
            ui::button("Receive this source")
                .icon(Icon::Play)
                .on_press_maybe(valid.then_some(Message::Start))
        } else {
            ui::button("Start")
                .icon(Icon::Play)
                .primary()
                .on_press_maybe(valid.then_some(Message::Start))
        };
        let stop = self
            .receiving
            .as_ref()
            .map(|_| ui::button("Stop").on_press(Message::Stop).boxed());

        column![
            ui::field("Source", field),
            problem.map(|problem| ui::caption(problem).tone(Tone::Danger).boxed()),
            ui::caption(
                "rtsp://, udp:// (H.264 over RTP), udp265://, mpegts://, tcp://, or test://"
            )
            .tone(Tone::Muted),
            ui::checkbox("Low latency", self.settings.low_latency).on_toggle(Message::LowLatency),
            ui::caption(
                "Frames show as soon as they are decoded. On a lossy link, turn it off to put \
                 packets in order and ask again for lost ones, at the cost of a little delay.",
            )
            .tone(Tone::Muted),
            ui::actions([[Some(start.boxed()), stop].into_iter().flatten().collect()]),
        ]
        .spacing(ui::space::S)
        .width(Length::Fill)
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn panel(settings: VideoSettings) -> VideoPanel {
        VideoPanel::new(settings).0
    }

    fn changed(event: Option<Event>) -> VideoSettings {
        match event {
            Some(Event::Changed(settings)) => settings,
            None => panic!("the settings did not change"),
        }
    }

    #[test]
    fn a_stream_off_at_close_stays_off() {
        let panel = panel(VideoSettings::default());
        assert!(panel.slot().is_none());
    }

    #[test]
    fn an_invalid_source_does_not_start() {
        let mut panel = panel(VideoSettings::default());
        let _ = panel.update(Message::EditSource("5600".to_string()));

        let (_, event) = panel.update(Message::Start);

        assert!(event.is_none());
        assert!(panel.slot().is_none());
    }

    #[test]
    fn starting_and_stopping_is_remembered() {
        let mut panel = panel(VideoSettings::default());
        let _ = panel.update(Message::EditSource(" test:// ".to_string()));

        let settings = changed(panel.update(Message::Start).1);
        assert_eq!(settings.uri, "test://");
        assert!(settings.enabled);
        assert!(panel.slot().is_some());

        let settings = changed(panel.update(Message::Stop).1);
        assert!(!settings.enabled);
        assert!(panel.slot().is_none());
    }

    #[test]
    fn a_replaced_receiver_is_not_heard() {
        let mut panel = panel(VideoSettings {
            uri: "test://".to_string(),
            enabled: true,
            ..VideoSettings::default()
        });
        let streaming = State {
            status: Status::Streaming {
                width: 1,
                height: 1,
                decoder: None,
            },
            frames: 1,
        };

        // Changing the latency starts a new receiver, the second.
        let _ = panel.update(Message::LowLatency(false));
        let _ = panel.update(Message::Updated(0, streaming.clone()));
        assert_ne!(
            panel.receiving.as_ref().map(|it| &it.state),
            Some(&streaming),
            "the first receiver's state is stale"
        );

        let _ = panel.update(Message::Updated(1, streaming.clone()));
        assert_eq!(
            panel.receiving.as_ref().map(|it| &it.state),
            Some(&streaming)
        );
    }

    #[test]
    fn full_screen_needs_a_stream_and_ends_with_it() {
        let mut panel = panel(VideoSettings::default());
        let _ = panel.update(Message::FullScreen(true));
        assert!(!panel.is_full_screen(), "nothing to show");

        let _ = panel.update(Message::EditSource("test://".to_string()));
        let _ = panel.update(Message::Start);
        let _ = panel.update(Message::FullScreen(true));
        assert!(panel.is_full_screen());

        let _ = panel.update(Message::Stop);
        assert!(!panel.is_full_screen());
        let _ = panel.update(Message::Start);
        assert!(!panel.is_full_screen(), "a new stream starts in its panel");
    }

    /// The panel's receiver puts frames in the slot the video widget reads.
    #[test]
    fn the_test_pattern_reaches_the_slot() {
        let panel = panel(VideoSettings {
            uri: "test://".to_string(),
            enabled: true,
            ..VideoSettings::default()
        });
        let Some(slot) = panel.slot() else {
            panic!("the stream was on, so it is received");
        };

        let until = Instant::now() + Duration::from_secs(5);
        while slot.latest().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = panel
            .receiving
            .as_ref()
            .map(|it| it.receiver.state().borrow().status.clone());
        if matches!(status, Some(Status::Failed(_))) {
            eprintln!("skipped: GStreamer cannot receive here ({status:?})");
            return;
        }
        assert!(slot.latest().is_some(), "{status:?}");
    }
}
