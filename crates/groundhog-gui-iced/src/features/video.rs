//! The video panel: where the camera's stream comes from, how it stands,
//! and the stream itself.
//!
//! While the stream is on, the panel owns its [`Receiver`]. The receiver's
//! state arrives as a message for every decoded frame, which is also what
//! makes the application draw each frame as soon as it is decoded: the
//! video widget reads the newest frame from the receiver's slot whenever
//! the view is built. A message of a receiver since replaced is ignored.
//!
//! The receiver retries on its own, every second, until video comes. The
//! panel does not show the attempts, which would flicker: only whether
//! video has yet to come, comes, or stopped ([`Showing`]). A picture that
//! stopped stays, dimmed, so it never passes for live; why the last
//! attempt failed is in the stream's details.
//!
//! The source and the low-latency switch are persisted by the application
//! (see [`Event`]), and so is whether the stream is on. On is the
//! operator's choice; whether it is received is also the application's:
//! a stream nothing can show is paused ([`VideoPanel::set_shown`]), so it
//! never decodes, holds its port and wakes the interface unseen. It
//! resumes when it shows again, at the next start too.
//!
//! The stream is recorded while the operator asks, and, when the setting
//! is on, while a vehicle is armed and for [`AFTER_DISARM`] after (see
//! [`WhileArmed`]). A stream being recorded is received even where nothing
//! shows it. The application tells the panel what it cannot know (whether
//! it shows, whether a vehicle is armed, the setting) and then has it
//! [`VideoPanel::sync`] its receiver with all of it.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use groundhog_video::{Config, FrameSlot, Receiver, Recording, Source, State, Status};
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
    /// Start or stop recording the stream.
    Record(bool),
    /// Receiver `id`'s state changed: a frame arrived, or its status.
    Updated(u64, State),
}

/// What the application keeps of the panel.
pub enum Event {
    /// The settings to persist.
    Changed(VideoSettings),
    /// The stream cannot be recorded, and why.
    RecordingFailed(String),
}

/// How long recording while armed goes on after the last vehicle disarms,
/// so a landing and what follows it are kept.
pub const AFTER_DISARM: Duration = Duration::from_secs(5);

/// Recording while a vehicle is armed: from arming until [`AFTER_DISARM`]
/// after the last vehicle disarms.
#[derive(Debug, Default)]
struct WhileArmed {
    /// The setting.
    enabled: bool,
    /// Whether a vehicle is armed.
    armed: bool,
    /// When the last vehicle disarmed, until [`AFTER_DISARM`] has passed.
    disarmed_at: Option<Instant>,
    /// The operator stopped this recording; the next arming starts another.
    dismissed: bool,
}

impl WhileArmed {
    fn observe(&mut self, armed: bool, now: Instant) {
        if armed && !self.armed {
            self.disarmed_at = None;
            self.dismissed = false;
        } else if !armed && self.armed {
            self.disarmed_at = Some(now);
        }
        self.armed = armed;
    }

    fn wants(&self, now: Instant) -> bool {
        self.enabled
            && !self.dismissed
            && (self.armed
                || self
                    .disarmed_at
                    .is_some_and(|at| now.duration_since(at) < AFTER_DISARM))
    }

    /// Whether a recording after disarming has yet to end: something must
    /// [`VideoPanel::sync`] the panel again once it has.
    fn lingers(&self) -> bool {
        self.disarmed_at.is_some()
    }

    fn expire(&mut self, now: Instant) {
        self.disarmed_at = self
            .disarmed_at
            .filter(|at| now.duration_since(*at) < AFTER_DISARM);
    }
}

/// What the operator is shown of a receiver's state. Opening the stream
/// and waiting to open it again are one state: the operator cannot act on
/// the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Showing {
    /// No video yet.
    Waiting,
    Live,
    /// Video came, and stopped; the receiver is trying to get it back.
    Lost,
    /// The stream cannot be received.
    Failed,
}

impl Showing {
    fn of(state: &State) -> Self {
        match state.status {
            Status::Streaming { .. } => Showing::Live,
            Status::Failed(_) => Showing::Failed,
            Status::Connecting | Status::Retrying { .. } if state.frames == 0 => Showing::Waiting,
            Status::Connecting | Status::Retrying { .. } => Showing::Lost,
        }
    }

    /// How it reads, in a badge.
    fn badge(self) -> (&'static str, Tone) {
        match self {
            Showing::Waiting => ("Connecting", Tone::Muted),
            Showing::Live => ("Live", Tone::Success),
            Showing::Lost => ("Video lost", Tone::Warning),
            Showing::Failed => ("Failed", Tone::Danger),
        }
    }
}

/// "Recording", while a file is written.
fn recording_badge<'a>(state: &State) -> Option<impl Widget<Message> + 'a> {
    matches!(state.recording, Recording::Writing(_)).then(|| ui::badge("Recording", Tone::Danger))
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
    /// Whether anything can show the picture; a stream on is received
    /// only while it can, or while it is recorded.
    shown: bool,
    /// Where recordings go; nowhere without a data directory.
    recordings: Option<PathBuf>,
    /// The operator asked to record.
    record: bool,
    while_armed: WhileArmed,
    /// The identity the next receiver gets.
    next: u64,
}

struct Receiving {
    id: u64,
    receiver: Receiver,
    state: State,
    /// Why the last attempt failed, until video comes.
    problem: Option<String>,
    /// Where the receiver was told to record, if anywhere.
    recording_into: Option<PathBuf>,
}

impl VideoPanel {
    /// The panel for `settings`, receiving nothing until it is shown,
    /// recording into `recordings`.
    pub fn new(settings: VideoSettings, recordings: Option<PathBuf>) -> Self {
        Self {
            source: settings.uri.clone(),
            settings,
            receiving: None,
            form: false,
            details: false,
            full_screen: false,
            shown: false,
            recordings,
            record: false,
            while_armed: WhileArmed::default(),
            next: 0,
        }
    }

    /// Whether anything can show the picture. Hidden, a stream that is on
    /// is paused (and leaves the full screen), unless it is recorded.
    pub fn set_shown(&mut self, shown: bool) {
        self.shown = shown;
    }

    /// Whether to record while a vehicle is armed (the setting).
    pub fn set_record_while_armed(&mut self, enabled: bool) {
        self.while_armed.enabled = enabled;
    }

    /// Whether a vehicle is armed, as of `now`.
    pub fn observe_armed(&mut self, armed: bool, now: Instant) {
        self.while_armed.observe(armed, now);
    }

    /// Whether the panel must be synced again later, without anything else
    /// changing: a recording after disarming ends on its own.
    pub fn needs_ticks(&self) -> bool {
        self.while_armed.lingers()
    }

    /// Bring the receiver in line with what is asked, as of `now`: receive
    /// a stream that is on while it shows or is recorded, and record while
    /// the operator or the setting asks.
    pub fn sync(&mut self, now: Instant) -> Task<Message> {
        self.while_armed.expire(now);
        let record = self.wants_recording(now);
        let receive = self.settings.enabled && (self.shown || record);
        if !self.shown {
            self.full_screen = false;
        }

        let task = match (receive, self.receiving.is_some()) {
            (true, false) => self.start(),
            (false, true) => {
                self.receiving = None;
                Task::none()
            }
            _ => Task::none(),
        };

        let into = self.recordings.clone().filter(|_| record);
        if let Some(receiving) = &mut self.receiving
            && receiving.recording_into != into
        {
            receiving.receiver.record(into.clone());
            receiving.recording_into = into;
        }
        task
    }

    /// Whether the stream is being recorded, or waits for video to record.
    pub fn is_recording(&self) -> bool {
        self.receiving
            .as_ref()
            .is_some_and(|receiving| receiving.recording_into.is_some())
    }

    /// Where recordings go, if anywhere.
    pub fn recordings(&self) -> Option<&Path> {
        self.recordings.as_deref()
    }

    fn wants_recording(&self, now: Instant) -> bool {
        self.recordings.is_some() && (self.record || self.while_armed.wants(now))
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
                self.receiving = None;
                let task = self.sync(Instant::now());
                (task, Some(Event::Changed(self.settings.clone())))
            }
            Message::Start => {
                if Source::parse(&self.source).is_err() {
                    return (Task::none(), None);
                }
                self.settings.uri = self.source.trim().to_string();
                self.settings.enabled = true;
                self.form = false;
                // A new source is a new receiver.
                self.receiving = None;
                let task = self.sync(Instant::now());
                (task, Some(Event::Changed(self.settings.clone())))
            }
            Message::Stop => {
                self.full_screen = false;
                self.settings.enabled = false;
                self.record = false;
                let task = self.sync(Instant::now());
                (task, Some(Event::Changed(self.settings.clone())))
            }
            Message::Record(record) => {
                self.record = record;
                // Stopped, a recording while armed waits for the next
                // arming.
                if !record {
                    self.while_armed.dismissed = true;
                }
                (self.sync(Instant::now()), None)
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
                let Some(receiving) = self.receiving.as_mut().filter(|it| it.id == id) else {
                    return (Task::none(), None);
                };
                match &state.status {
                    Status::Retrying { reason } => receiving.problem = Some(reason.clone()),
                    Status::Streaming { .. } => receiving.problem = None,
                    Status::Connecting | Status::Failed(_) => {}
                }
                let failed = match &state.recording {
                    Recording::Failed(reason) if receiving.state.recording != state.recording => {
                        Some(Event::RecordingFailed(reason.clone()))
                    }
                    _ => None,
                };
                receiving.state = state;
                (Task::none(), failed)
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
            problem: None,
            recording_into: None,
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
                [ui::badge("Off", Tone::Muted).boxed()],
                [],
            );
            return ui::panel_with(header, self.view_form()).boxed();
        };

        // The picture fills the panel; what the panel would otherwise put
        // around it floats over its corner, the details on demand.
        let picture = self.view_picture(receiving);

        let (label, tone) = Showing::of(&receiving.state).badge();
        // Hovered, the indicator explains itself; pressed (as a finger
        // must, having no hover), it keeps the details open.
        let indicator = ui::tooltip(
            mouse_area(ui::badge(label, tone))
                .on_press(Message::ToggleDetails)
                .interaction(mouse::Interaction::Pointer),
            self.details_text(receiving),
            iced::widget::tooltip::Position::Bottom,
        );
        let recording = self.is_recording();
        let record = ui::icon_button(Icon::Record)
            .chosen(recording)
            .hint(match (recording, self.recordings.is_some()) {
                (true, _) => "Stop recording",
                (false, true) => "Record the video",
                (false, false) => "Nowhere to record to",
            })
            .on_press_maybe(
                self.recordings
                    .is_some()
                    .then_some(Message::Record(!recording)),
            );
        let chip = ui::floating(
            row![
                indicator,
                recording_badge(&receiving.state),
                record.boxed(),
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
        };
        let overlay = column![
            chip,
            self.details
                .then(|| card(ui::text(self.details_text(receiving)).boxed())),
            self.form.then(|| card(self.view_form().boxed())),
        ]
        .spacing(ui::space::S)
        .padding(ui::space::S);

        ui::picture(stack![picture, overlay]).boxed()
    }

    /// The picture alone, filling the screen, with the stream's state and a
    /// way out (also Escape, or back) in its corner, clear of the platform's
    /// chrome by `insets`.
    pub fn view_full_screen(&self, insets: Padding) -> Element<'_, Message> {
        let Some(receiving) = &self.receiving else {
            return iced::widget::Space::new().boxed();
        };

        let (label, tone) = Showing::of(&receiving.state).badge();
        let chip = ui::floating(
            row![
                ui::badge(label, tone),
                recording_badge(&receiving.state),
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

        ui::screen(stack![self.view_picture(receiving), corner]).boxed()
    }

    /// The picture for elsewhere than the panel (the floating shell's map
    /// corner, or the screen the map makes way for): dimmed when it
    /// stopped, as in the panel, and, unless it is live, its state in a
    /// corner kept `clear` of what covers it. A frozen frame must not pass
    /// for live where the operator flies by it. Nothing while nothing is
    /// received.
    pub fn view_inset(&self, clear: Padding) -> Option<impl Widget<Message> + '_> {
        let receiving = self.receiving.as_ref()?;
        let showing = Showing::of(&receiving.state);
        // The mark's layer stays, empty, while the picture is live, so the
        // tree keeps its shape as the stream comes and goes.
        let mark = if showing == Showing::Live {
            iced::widget::Space::new().boxed()
        } else {
            let (label, tone) = showing.badge();
            ui::floating::<Message, _>(ui::badge(label, tone))
                .padding(ui::space::XS)
                .boxed()
        };

        Some(ui::screen(stack![
            video(receiving.receiver.slot()),
            ui::veil::<Message, _>(iced::widget::Space::new(), showing == Showing::Lost),
            container(mark).padding(clear),
        ]))
    }

    /// The stream's details: where it comes from, what it is, and how it is
    /// received.
    fn details_text(&self, receiving: &Receiving) -> String {
        let mut lines = vec![self.settings.uri.clone()];
        if let Status::Streaming {
            width,
            height,
            decoder,
        } = &receiving.state.status
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
        if let Some(problem) = &receiving.problem {
            lines.push(format!("Last attempt: {problem}"));
        }
        match &receiving.state.recording {
            Recording::Writing(path) => lines.push(format!("Recording to {}", path.display())),
            Recording::Waiting => lines.push("Recording once video comes".to_string()),
            Recording::Failed(reason) => lines.push(format!("Not recording: {reason}")),
            Recording::Off => {}
        }
        lines.join("\n")
    }

    /// The picture, and over it what it cannot show: dimmed when it
    /// stopped, and why there is none before the first frame.
    fn view_picture<'a>(&'a self, receiving: &'a Receiving) -> impl Widget<Message> + 'a {
        let showing = Showing::of(&receiving.state);
        stack![
            video(receiving.receiver.slot()),
            ui::veil(self.view_status(&receiving.state), showing == Showing::Lost),
        ]
    }

    /// What shows over the picture while there is none yet: why. White on
    /// the picture's black, in either theme. Nothing over a picture, live
    /// or stopped: its badge says which.
    fn view_status(&self, state: &State) -> Element<'_, Message> {
        let text = match (&state.status, Showing::of(state)) {
            (Status::Failed(reason), _) => reason.clone(),
            (_, Showing::Waiting) => format!("Waiting for video from: {}", self.settings.uri),
            _ => return iced::widget::Space::new().boxed(),
        };
        ui::text(text).boxed()
    }

    /// The source's form: its URI, the low-latency switch, and starting or
    /// stopping the stream.
    fn view_form(&self) -> impl Widget<Message> + '_ {
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
            problem.map(|problem| ui::caption(problem).tone(Tone::Danger)),
            ui::caption("rtsp://, udp://, udp265://, mpegts://, tcp://, or test://")
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
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Tell `panel` whether it shows, and sync it.
    fn show(panel: &mut VideoPanel, shown: bool) {
        panel.set_shown(shown);
        drop(panel.sync(Instant::now()));
    }

    /// The panel for `settings`, shown.
    fn panel(settings: VideoSettings) -> VideoPanel {
        let mut panel = VideoPanel::new(settings, None);
        show(&mut panel, true);
        panel
    }

    fn on(uri: &str) -> VideoSettings {
        VideoSettings {
            uri: uri.to_string(),
            enabled: true,
            ..VideoSettings::default()
        }
    }

    /// A stream that was on is received once something can show it, not
    /// before.
    #[test]
    fn a_stream_on_waits_to_be_shown() {
        let mut panel = VideoPanel::new(on("test://"), None);
        assert!(panel.slot().is_none(), "nothing shows it yet");

        show(&mut panel, true);
        assert!(panel.slot().is_some());
    }

    /// Hidden, the stream pauses: no receiver, no full screen, and nothing
    /// to persist, as it is still on. Shown again, it resumes.
    #[test]
    fn hiding_pauses_the_stream_and_showing_resumes_it() {
        let mut panel = panel(on("test://"));
        drop(panel.update(Message::FullScreen(true)));

        show(&mut panel, false);
        assert!(panel.slot().is_none());
        assert!(!panel.is_full_screen());
        assert!(panel.settings.enabled, "still on");

        show(&mut panel, true);
        assert!(panel.slot().is_some());
    }

    /// Started while nothing shows it, the stream is on but not received.
    #[test]
    fn a_stream_started_while_hidden_waits() {
        let mut panel = VideoPanel::new(VideoSettings::default(), None);
        drop(panel.update(Message::EditSource("test://".to_string())));

        let settings = changed(panel.update(Message::Start).1);

        assert!(settings.enabled);
        assert!(panel.slot().is_none());
        show(&mut panel, true);
        assert!(panel.slot().is_some());
    }

    /// Where a test's recordings would go. Nothing is written there: the
    /// test pattern is not recorded.
    fn recordings() -> Option<PathBuf> {
        Some(std::env::temp_dir().join("groundhog-video-panel-tests"))
    }

    /// Recording while armed starts at arming and ends `AFTER_DISARM`
    /// after the last vehicle disarms.
    #[test]
    fn recording_while_armed_outlasts_the_disarm() {
        let start = Instant::now();
        let mut armed = WhileArmed {
            enabled: true,
            ..WhileArmed::default()
        };
        assert!(!armed.wants(start));

        armed.observe(true, start);
        assert!(armed.wants(start));

        let disarmed = start + Duration::from_secs(60);
        armed.observe(false, disarmed);
        assert!(armed.wants(disarmed + AFTER_DISARM - Duration::from_millis(1)));
        assert!(armed.lingers());
        assert!(!armed.wants(disarmed + AFTER_DISARM));

        armed.expire(disarmed + AFTER_DISARM);
        assert!(!armed.lingers(), "nothing left to wait for");
    }

    /// Without the setting, arming records nothing.
    #[test]
    fn arming_records_only_with_the_setting() {
        let now = Instant::now();
        let mut armed = WhileArmed::default();
        armed.observe(true, now);
        assert!(!armed.wants(now));
    }

    /// Stopped by the operator, a recording while armed waits for the next
    /// arming.
    #[test]
    fn a_stopped_recording_waits_for_the_next_arming() {
        let mut panel = panel(on("test://"));
        panel.recordings = recordings();
        panel.set_record_while_armed(true);
        let now = Instant::now();
        panel.observe_armed(true, now);
        drop(panel.sync(now));
        assert!(panel.is_recording());

        drop(panel.update(Message::Record(false)));
        assert!(!panel.is_recording(), "stopped while still armed");

        panel.observe_armed(false, now);
        panel.observe_armed(true, now);
        drop(panel.sync(now));
        assert!(panel.is_recording(), "armed again");
    }

    /// A stream being recorded is received though nothing shows it, and
    /// paused once the recording ends.
    #[test]
    fn a_recorded_stream_is_received_while_hidden() {
        let mut panel = panel(on("test://"));
        panel.recordings = recordings();
        drop(panel.update(Message::Record(true)));
        show(&mut panel, false);
        assert!(panel.slot().is_some(), "received for the recording");
        assert!(panel.is_recording());

        drop(panel.update(Message::Record(false)));
        assert!(panel.slot().is_none(), "paused: nothing shows it");
    }

    /// Without a directory to record into, nothing is recorded.
    #[test]
    fn nothing_is_recorded_without_a_directory() {
        let mut panel = panel(on("test://"));
        drop(panel.update(Message::Record(true)));
        assert!(!panel.is_recording());
    }

    /// Stopping the video stops its recording, and the next stream is not
    /// recorded unasked.
    #[test]
    fn stopping_the_video_stops_recording() {
        let mut panel = panel(on("test://"));
        panel.recordings = recordings();
        drop(panel.update(Message::Record(true)));

        drop(panel.update(Message::Stop));
        assert!(!panel.is_recording());
        drop(panel.update(Message::Start));
        assert!(panel.slot().is_some());
        assert!(!panel.is_recording());
    }

    /// A recording that fails says so once, not with every frame.
    #[test]
    fn a_failed_recording_is_announced_once() {
        let mut panel = panel(on("test://"));
        let failed = State {
            recording: Recording::Failed("The test pattern is not recorded".to_string()),
            ..state(Status::Connecting, 0)
        };

        let (_, event) = panel.update(Message::Updated(0, failed.clone()));
        assert!(matches!(event, Some(Event::RecordingFailed(_))));
        let (_, event) = panel.update(Message::Updated(0, failed));
        assert!(event.is_none());
    }

    /// Whether `panel`'s picture outside the panel shows `text`.
    fn inset_shows(panel: &VideoPanel, text: &str) -> bool {
        let Some(inset) = panel.view_inset(Padding::ZERO) else {
            return false;
        };
        iced_test::Simulator::new(inset).find(text).is_ok()
    }

    /// Outside the panel, a picture that stopped says so, and a live one
    /// shows alone.
    #[test]
    fn the_inset_marks_a_lost_picture() {
        let mut panel = panel(on("test://"));
        let live = Status::Streaming {
            width: 1,
            height: 1,
            decoder: None,
        };

        drop(panel.update(Message::Updated(0, state(live, 30))));
        assert!(!inset_shows(&panel, "Video lost"));
        assert!(!inset_shows(&panel, "Live"), "a live picture is unmarked");

        drop(panel.update(Message::Updated(
            0,
            state(retrying("The video stopped"), 30),
        )));
        assert!(inset_shows(&panel, "Video lost"));
    }

    fn changed(event: Option<Event>) -> VideoSettings {
        match event {
            Some(Event::Changed(settings)) => settings,
            _ => panic!("the settings did not change"),
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
        drop(panel.update(Message::EditSource("5600".to_string())));

        let (_, event) = panel.update(Message::Start);

        assert!(event.is_none());
        assert!(panel.slot().is_none());
    }

    #[test]
    fn starting_and_stopping_is_remembered() {
        let mut panel = panel(VideoSettings::default());
        drop(panel.update(Message::EditSource(" test:// ".to_string())));

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
            recording: Recording::Off,
        };

        // Changing the latency starts a new receiver, the second.
        drop(panel.update(Message::LowLatency(false)));
        drop(panel.update(Message::Updated(0, streaming.clone())));
        assert_ne!(
            panel.receiving.as_ref().map(|it| &it.state),
            Some(&streaming),
            "the first receiver's state is stale"
        );

        drop(panel.update(Message::Updated(1, streaming.clone())));
        assert_eq!(
            panel.receiving.as_ref().map(|it| &it.state),
            Some(&streaming)
        );
    }

    fn state(status: Status, frames: u64) -> State {
        State {
            status,
            frames,
            recording: Recording::Off,
        }
    }

    fn retrying(reason: &str) -> Status {
        Status::Retrying {
            reason: reason.to_string(),
        }
    }

    /// The receiver's attempts do not show: before the first frame and
    /// after the last one, opening and waiting to open again read the same.
    #[test]
    fn retrying_reads_as_one_steady_state() {
        assert_eq!(Showing::of(&state(Status::Connecting, 0)), Showing::Waiting);
        assert_eq!(
            Showing::of(&state(retrying("No video arrives"), 0)),
            Showing::Waiting
        );

        assert_eq!(Showing::of(&state(Status::Connecting, 40)), Showing::Lost);
        assert_eq!(
            Showing::of(&state(retrying("The video stopped"), 40)),
            Showing::Lost
        );

        let live = Status::Streaming {
            width: 1,
            height: 1,
            decoder: None,
        };
        assert_eq!(Showing::of(&state(live, 41)), Showing::Live);
    }

    /// Why the last attempt failed stays in the details while the receiver
    /// tries again, and goes once video comes.
    #[test]
    fn the_last_problem_is_in_the_details_until_video_comes() {
        let mut panel = panel(VideoSettings {
            uri: "test://".to_string(),
            enabled: true,
            ..VideoSettings::default()
        });
        let details = |panel: &VideoPanel| {
            panel
                .receiving
                .as_ref()
                .map(|receiving| panel.details_text(receiving))
                .unwrap_or_default()
        };

        drop(panel.update(Message::Updated(
            0,
            state(retrying("Connection refused"), 0),
        )));
        drop(panel.update(Message::Updated(0, state(Status::Connecting, 0))));
        assert!(
            details(&panel).contains("Last attempt: Connection refused"),
            "{}",
            details(&panel)
        );

        let live = Status::Streaming {
            width: 1,
            height: 1,
            decoder: None,
        };
        drop(panel.update(Message::Updated(0, state(live, 1))));
        assert!(
            !details(&panel).contains("Last attempt"),
            "{}",
            details(&panel)
        );
    }

    #[test]
    fn full_screen_needs_a_stream_and_ends_with_it() {
        let mut panel = panel(VideoSettings::default());
        drop(panel.update(Message::FullScreen(true)));
        assert!(!panel.is_full_screen(), "nothing to show");

        drop(panel.update(Message::EditSource("test://".to_string())));
        drop(panel.update(Message::Start));
        drop(panel.update(Message::FullScreen(true)));
        assert!(panel.is_full_screen());

        drop(panel.update(Message::Stop));
        assert!(!panel.is_full_screen());
        drop(panel.update(Message::Start));
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
