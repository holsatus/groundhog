//! The application settings panel.
//!
//! A stateless view over [`Settings`], which the root owns and persists. New
//! settings should be added to [`Settings`] and
//! surfaced here; the root applies the changes.
//!
//! The scale is applied when its slider is released, not while it moves:
//! every change of scale lays the whole window out again, under the finger
//! or cursor dragging the slider.

use iced::{
    Alignment, Element, Length, Widget,
    widget::{column, row, scrollable, slider},
};

use crate::{
    config::Settings,
    ui::{self, TextExt, Tone, theme::ThemeMode},
};

#[derive(Debug, Clone)]
pub enum Message {
    /// Switch the application palette.
    Theme(ThemeMode),
    /// The scale slider moved; the scale is previewed on the slider only.
    PreviewScaleFactor(f32),
    /// The scale slider was released: apply the previewed scale.
    CommitScaleFactor,
    RecordTelemetry(bool),
    DefaultAltitude(f32),
    /// Ask whether to put every setting back to its default.
    RestoreDefaults,
    /// The operator confirmed [`Message::RestoreDefaults`].
    DefaultsConfirmed,
}

/// `recording` is the file telemetry is being recorded to, if any, and
/// `scale_preview` the scale the slider is being dragged to.
pub fn view<'a>(
    settings: &'a Settings,
    scale_preview: Option<f32>,
    recording: Option<&'a std::path::Path>,
) -> Element<'a, Message> {
    let scale = scale_preview.unwrap_or(settings.scale_factor);

    let appearance = ui::section(
        "Appearance",
        [
            ui::field(
                "Theme",
                ui::pick_list(Some(settings.theme), ThemeMode::ALL, ThemeMode::to_string)
                    .on_select(Message::Theme),
            ),
            ui::field(
                "Scale",
                slider_with_value(
                    slider(0.5..=2.0, scale, Message::PreviewScaleFactor)
                        .on_release(Message::CommitScaleFactor)
                        .step(0.1),
                    format!("{scale:.1}×"),
                ),
            ),
        ],
    );

    let planning = ui::section(
        "Planning",
        [ui::field(
            "First waypoint",
            slider_with_value(
                slider(
                    5.0..=500.0,
                    settings.default_waypoint_altitude_m,
                    Message::DefaultAltitude,
                )
                .step(5.0),
                format!("{:.0} m", settings.default_waypoint_altitude_m),
            ),
        )],
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
fn slider_with_value<'a>(slider: impl Widget<Message> + 'a, value: String) -> Element<'a, Message> {
    row![slider, ui::text(value).width(Length::Fixed(56.0))]
        .spacing(ui::space::M)
        .align_y(Alignment::Center)
        .boxed()
}
