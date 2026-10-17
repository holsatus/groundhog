//! The application settings panel.
//!
//! A stateless view over [`Settings`], which the root owns and persists. New
//! settings should be added to [`Settings`](crate::config::Settings) and
//! surfaced here; the root applies the changes.

use iced::widget::{checkbox, column, container, pick_list, row, rule, slider, text};
use iced::{Alignment, Element, Length, Widget};

use crate::{config::Settings, theme::ThemeMode};

#[derive(Debug, Clone)]
pub enum Message {
    /// Switch the application palette.
    Theme(ThemeMode),
    ScaleFactor(f32),
    RecordTelemetry(bool),
    DefaultAltitude(f32),
}

/// `recording` is the file telemetry is being recorded to, if any.
pub fn view<'a>(
    settings: &'a Settings,
    recording: Option<&'a std::path::Path>,
) -> Element<'a, Message> {
    let theme_row = row![
        text("Theme").width(Length::Fill),
        pick_list(Some(settings.theme), ThemeMode::ALL, |mode| mode
            .to_string())
        .on_select(Message::Theme)
        .style(crate::style::pick_list::default)
        .width(Length::Fixed(160.0)),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);

    let scale_row = row![
        text("Scale").width(Length::Fill),
        slider(0.5..=2.0, settings.scale_factor, Message::ScaleFactor)
            .step(0.1)
            .width(Length::Fixed(160.0)),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);

    let altitude_row = row![
        text(format!(
            "Default waypoint altitude: {:.0} m",
            settings.default_waypoint_altitude_m
        ))
        .width(Length::Fill),
        slider(
            5.0..=500.0,
            settings.default_waypoint_altitude_m,
            Message::DefaultAltitude
        )
        .step(5.0)
        .width(Length::Fixed(160.0)),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);

    let recording_row = column![
        checkbox(settings.record_telemetry)
            .label("Record telemetry logs (.tlog)")
            .on_toggle(Message::RecordTelemetry),
        text(match recording {
            Some(path) => format!("Recording to {}", path.display()),
            None => "Not recording".to_string(),
        })
        .size(12),
    ]
    .spacing(4.0);

    let contents = column![
        text("Application settings").size(24.0),
        rule::horizontal(1.0),
        theme_row,
        scale_row,
        rule::horizontal(1.0),
        text("Planning").size(18.0),
        altitude_row,
        rule::horizontal(1.0),
        text("Recording").size(18.0),
        recording_row,
    ]
    .spacing(10.0);

    container(contents).padding(8.0).width(Length::Fill).boxed()
}
