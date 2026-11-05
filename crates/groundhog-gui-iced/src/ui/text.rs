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

/// A MAVLink severity's name, four letters in brackets: monospaced and in its
/// tone, so a column of them stays narrow and lines up.
pub fn severity<'a>(severity: MavSeverity) -> Text<'a> {
    mono(abbreviation(severity)).tone(Tone::of_severity(severity))
}

/// A MAVLink severity's full name, in its tone: what [`severity`] stands
/// for.
pub fn severity_name<'a>(severity: MavSeverity) -> Text<'a> {
    text(format!("{severity:?}")).tone(Tone::of_severity(severity))
}

/// A severity's name, four letters between brackets so every one is as wide
/// as the rest.
fn abbreviation(severity: MavSeverity) -> &'static str {
    match severity {
        MavSeverity::Emergency => "[EMRG]",
        MavSeverity::Alert => "[ALRT]",
        MavSeverity::Critical => "[CRIT]",
        MavSeverity::Error => "[ERRO]",
        MavSeverity::Warning => "[WARN]",
        MavSeverity::Notice => "[NOTI]",
        MavSeverity::Info => "[INFO]",
        MavSeverity::Debug => "[DEBG]",
    }
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

    fn single_line_ellipsis_start(self) -> Self;
}

impl TextExt for Text<'_> {
    fn tone(self, tone: Tone) -> Self {
        self.style(style::toned(tone))
    }

    fn single_line(self) -> Self {
        self.wrapping(Wrapping::None).ellipsis(Ellipsis::End)
    }

    fn single_line_ellipsis_start(self) -> Self {
        self.wrapping(Wrapping::None).ellipsis(Ellipsis::Start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every abbreviation is four letters and two brackets: one that is
    /// longer shifts the severities after it out of their column.
    #[test]
    fn every_severity_is_six_characters() {
        for severity in [
            MavSeverity::Emergency,
            MavSeverity::Alert,
            MavSeverity::Critical,
            MavSeverity::Error,
            MavSeverity::Warning,
            MavSeverity::Notice,
            MavSeverity::Info,
            MavSeverity::Debug,
        ] {
            let abbreviation = abbreviation(severity);
            assert_eq!(abbreviation.chars().count(), 6, "{abbreviation}");
        }
    }
}
