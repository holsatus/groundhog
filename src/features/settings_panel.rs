//! The application settings panel.
//!
//! A stateless view over [`Settings`], which the root owns and persists. New
//! settings should be added to [`Settings`](crate::config::Settings) and
//! surfaced here; the root applies the changes.

use iced::widget::{column, container, pick_list, row, rule, slider, text};
use iced::{Alignment, Element, Length, Widget};

use crate::{config::Settings, theme::ThemeMode};

#[derive(Debug, Clone)]
pub enum Message {
    /// Switch the application palette.
    SetTheme(ThemeMode),
    SetScaleFactor(f32),
}

pub fn view(settings: &Settings) -> Element<'_, Message> {
    let theme_row = row![
        text("Theme").width(Length::Fill),
        pick_list(Some(settings.theme), ThemeMode::ALL, |mode| mode
            .to_string())
        .on_select(Message::SetTheme)
        .width(Length::Fixed(160.0)),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);

    let scale_row = row![
        text("Scale").width(Length::Fill),
        slider(0.5..=2.0, settings.scale_factor, Message::SetScaleFactor)
            .step(0.1)
            .width(Length::Fixed(160.0)),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);

    let contents = column![
        text("Application settings").size(24.0),
        rule::horizontal(1.0),
        theme_row,
        scale_row
    ]
    .spacing(10.0);

    container(contents).padding(8.0).width(Length::Fill).boxed()
}
