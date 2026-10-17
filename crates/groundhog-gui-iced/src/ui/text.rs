//! Text in the application's typography: a few sizes, and tones for meaning.

use iced::{
    Font,
    widget::{
        Text,
        text::{Ellipsis, IntoFragment, Wrapping},
    },
};
use mavio::default_dialect::enums::MavSeverity;

use super::style::{self, Tone, size};

/// Secondary text: hints, units, details, the label of a value.
pub fn caption<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    text(content).size(size::CAPTION)
}

/// The title of a card or a dialog.
pub fn title<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    text(content).size(size::TITLE)
}

/// Monospaced text, for identifiers that line up in columns.
pub fn mono<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    text(content).font(Font::MONOSPACE)
}

/// A MAVLink severity's name, in its tone.
pub fn severity<'a>(severity: MavSeverity) -> Text<'a> {
    text(format!("{severity:?}")).tone(Tone::of_severity(severity))
}

/// Body text.
pub fn text<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    iced::widget::text(content)
}

/// Ways to style any [`Text`].
pub trait TextExt {
    /// Colour the text by what it means.
    fn tone(self, tone: Tone) -> Self;

    /// Keep the text on one line, cut with an ellipsis. Text in a list row
    /// of fixed height must be single-line.
    fn single_line(self) -> Self;
}

impl TextExt for Text<'_> {
    fn tone(self, tone: Tone) -> Self {
        self.style(style::toned(tone))
    }

    fn single_line(self) -> Self {
        self.wrapping(Wrapping::None).ellipsis(Ellipsis::End)
    }
}
