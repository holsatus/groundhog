//! The application settings panel.
//!
//! A view over [`Settings`], which the application owns, persists and
//! applies: the panel asks for a change with [`Effect::Settings`]. New
//! settings are added to [`Settings`] and surfaced here.
//!
//! The scale is applied when its slider is released, not while it moves:
//! every change of scale lays the whole window out again, under the finger
//! or cursor dragging the slider.

use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{column, row, scrollable},
};

use super::{Context, Effect, Panel, ViewContext, confirm::Confirmation};
use crate::{
    config::Settings,
    features::video,
    shell::ShellMode,
    ui::{self, TextExt, Tone, theme::ThemeMode},
};

/// The panel's own state: the scale its slider is being dragged to.
#[derive(Debug, Default)]
pub struct SettingsPanel {
    scale_preview: Option<f32>,
}

impl Panel for SettingsPanel {
    type Message = Message;

    fn update(&mut self, message: Message, cx: &mut Context<Message>) -> Task<Message> {
        let mut settings = *cx.settings;

        match message {
            Message::Theme(mode) => settings.theme = mode,
            Message::Shell(mode) => settings.shell = mode,
            Message::TouchControls(touch) => settings.touch_controls = touch,
            Message::PreviewScaleFactor(scale) => self.scale_preview = Some(scale),
            Message::CommitScaleFactor => {
                if let Some(scale) = self.scale_preview.take() {
                    settings.scale_factor = scale;
                }
            }
            Message::DefaultAltitude(altitude) => {
                settings.default_waypoint_altitude_m = altitude;
            }
            Message::RecordTelemetry(record) => settings.record_telemetry = record,
            Message::RecordVideoWhileArmed(record) => settings.record_video_while_armed = record,
            Message::RestoreDefaults => cx.confirm(
                Confirmation::new(
                    "Restore the default settings?",
                    "Every setting goes back to its default. Links and the panel layout are kept.",
                    Message::DefaultsConfirmed,
                )
                .accept_label("Restore"),
            ),
            Message::DefaultsConfirmed => settings = Settings::default(),
        }

        if settings != *cx.settings {
            cx.request(Effect::Settings(settings));
        }
        Task::none()
    }

    fn view<'a>(&'a self, cx: ViewContext<'a>) -> Element<'a, Message> {
        view(cx.settings, self.scale_preview, cx.recorder, cx.recordings).boxed()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Switch the application palette.
    Theme(ThemeMode),
    /// Choose how the window is laid out.
    Shell(ShellMode),
    /// Use touch-sized controls in every layout.
    TouchControls(bool),
    /// The scale slider moved; the scale is previewed on the slider only.
    PreviewScaleFactor(f32),
    /// The scale slider was released: apply the previewed scale.
    CommitScaleFactor,
    RecordTelemetry(bool),
    RecordVideoWhileArmed(bool),
    DefaultAltitude(f32),
    /// Ask whether to put every setting back to its default.
    RestoreDefaults,
    /// The operator confirmed [`Message::RestoreDefaults`].
    DefaultsConfirmed,
}

/// `recording` is the file telemetry is being recorded to, if any,
/// `videos` where videos are recorded to, and `scale_preview` the scale the
/// slider is being dragged to.
fn view<'a>(
    settings: &'a Settings,
    scale_preview: Option<f32>,
    recording: Option<&'a std::path::Path>,
    videos: Option<&'a std::path::Path>,
) -> impl Widget<Message> + 'a {
    let scale = scale_preview.unwrap_or(settings.scale_factor);

    let appearance = ui::section(
        "Appearance",
        [
            ui::field(
                "Theme",
                ui::pick_list(Some(settings.theme), ThemeMode::ALL, ThemeMode::to_string)
                    .on_select(Message::Theme),
            )
            .boxed(),
            ui::field(
                "Layout",
                ui::pick_list(Some(settings.shell), ShellMode::ALL, ShellMode::to_string)
                    .on_select(Message::Shell),
            )
            .boxed(),
            ui::checkbox("Touch-sized controls", settings.touch_controls)
                .on_toggle(Message::TouchControls)
                .boxed(),
            ui::caption("The floating layout always has them.")
                .tone(Tone::Muted)
                .boxed(),
            ui::field(
                "Scale",
                slider_with_value(
                    ui::slider(0.5..=2.0, scale, Message::PreviewScaleFactor)
                        .on_release(Message::CommitScaleFactor)
                        .step(0.1),
                    format!("{scale:.1}×"),
                ),
            )
            .boxed(),
        ],
    );

    let planning = ui::section(
        "Planning",
        [ui::field(
            "First waypoint",
            slider_with_value(
                ui::slider(
                    5.0..=500.0,
                    settings.default_waypoint_altitude_m,
                    Message::DefaultAltitude,
                )
                .step(5.0),
                format!("{:.0} m", settings.default_waypoint_altitude_m),
            ),
        )
        .boxed()],
    );

    let recording = ui::section(
        "Recording",
        [
            ui::checkbox("Record telemetry logs (.tlog)", settings.record_telemetry)
                .on_toggle(Message::RecordTelemetry)
                .boxed(),
            ui::caption(match recording {
                Some(path) => format!("Recording to {}", path.display()),
                None => "Not recording".to_string(),
            })
            .tone(Tone::Muted)
            .boxed(),
            ui::checkbox(
                "Record video while armed",
                settings.record_video_while_armed,
            )
            .on_toggle(Message::RecordVideoWhileArmed)
            .boxed(),
            ui::caption(match videos {
                Some(directory) => format!(
                    "While the video is on, from arming until {} s after the last vehicle \
                     disarms. Videos go to {}.",
                    video::AFTER_DISARM.as_secs(),
                    directory.display()
                ),
                None => "There is no data directory to record videos into.".to_string(),
            })
            .tone(Tone::Muted)
            .boxed(),
        ],
    );

    let restore = ui::button("Restore to defaults")
        .on_press(Message::RestoreDefaults)
        .width(Length::Fill)
        .boxed();

    ui::panel(scrollable(
        column![appearance, planning, recording, restore].spacing(ui::space::L),
    ))
}

/// A slider with its value beside it.
fn slider_with_value<'a>(
    slider: impl Widget<Message> + 'a,
    value: String,
) -> impl Widget<Message> + 'a {
    row![slider, ui::text(value).width(Length::Fixed(56.0))]
        .spacing(ui::space::M)
        .align_y(Alignment::Center)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::test_support::Env;

    /// The settings a panel's effects ask for, if any.
    fn requested(effects: &[Effect<Message>]) -> Option<Settings> {
        effects.iter().find_map(|effect| match effect {
            Effect::Settings(settings) => Some(*settings),
            _ => None,
        })
    }

    #[tokio::test]
    async fn a_change_is_asked_of_the_application() {
        let (mut env, mut panel) = (Env::new(), SettingsPanel::default());

        let effects = env.step(&mut panel, Message::RecordVideoWhileArmed(true));

        let settings = requested(&effects).expect("a change");
        assert!(settings.record_video_while_armed);
    }

    #[tokio::test]
    async fn the_scale_changes_when_the_slider_is_released() {
        let (mut env, mut panel) = (Env::new(), SettingsPanel::default());

        let effects = env.step(&mut panel, Message::PreviewScaleFactor(1.5));
        assert!(requested(&effects).is_none(), "only previewed");

        let effects = env.step(&mut panel, Message::CommitScaleFactor);
        assert_eq!(
            requested(&effects).map(|settings| settings.scale_factor),
            Some(1.5)
        );
    }

    #[tokio::test]
    async fn restoring_the_defaults_asks_first() {
        let mut env = Env::new();
        env.settings.record_video_while_armed = true;
        let mut panel = SettingsPanel::default();

        let effects = env.step(&mut panel, Message::RestoreDefaults);
        let [Effect::Confirm(confirmation)] = effects.as_slice() else {
            panic!("expected a confirmation: {effects:?}");
        };
        assert!(matches!(confirmation.accept, Message::DefaultsConfirmed));

        let effects = env.step(&mut panel, Message::DefaultsConfirmed);
        assert_eq!(requested(&effects), Some(Settings::default()));
    }

    #[tokio::test]
    async fn an_unchanged_setting_asks_for_nothing() {
        let (mut env, mut panel) = (Env::new(), SettingsPanel::default());
        let theme = env.settings.theme;

        let effects = env.step(&mut panel, Message::Theme(theme));

        assert!(effects.is_empty());
    }
}
