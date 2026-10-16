//! Application theme selection.
//!
//! Two custom palettes, dark and light, are defined here as [`Seed`]s; the
//! concrete iced [`Theme`] is derived from the selected [`ThemeMode`]. Nothing
//! else in the application references a built-in iced theme, so the look is
//! controlled from this one place.

use std::fmt;

use iced::Theme;
use iced::theme::palette::Seed;

/// Which of the application's palettes to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    /// Every mode, for a picker.
    pub const ALL: [ThemeMode; 2] = [ThemeMode::Dark, ThemeMode::Light];

    /// The base colors this mode is generated from.
    pub fn seed(self) -> Seed {
        match self {
            ThemeMode::Dark => DARK,
            ThemeMode::Light => LIGHT,
        }
    }

    /// The iced theme for this mode.
    pub fn theme(self) -> Theme {
        Theme::custom(self.name(), self.seed())
    }

    /// A human-readable name, shown in the theme picker and stored on the theme.
    pub fn name(self) -> &'static str {
        match self {
            ThemeMode::Dark => "Groundhog Dark",
            ThemeMode::Light => "Groundhog Light",
        }
    }
}

impl fmt::Display for ThemeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
        })
    }
}

/// Neutral surfaces with a cool accent. Deliberately close to grayscale, so the
/// map and the telemetry overlays stay the most saturated things on screen.
const DARK: Seed = Seed {
    background: iced::color!(0x101010),
    text: iced::color!(0xe4e6eb),
    primary: iced::color!(0x5098E5),
    success: iced::color!(0x5CCC75),
    warning: iced::color!(0xE5CB45),
    danger: iced::color!(0xD95763),
};

/// The light counterpart, with the same accents darkened for contrast on white.
const LIGHT: Seed = Seed {
    background: iced::color!(0xFFFFFF),
    text: iced::color!(0x1f2328),
    primary: iced::color!(0x3b6fe0),
    success: iced::color!(0x3e9e5a),
    warning: iced::color!(0xb7791f),
    danger: iced::color!(0xd64545),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_derive_the_expected_palette() {
        for mode in ThemeMode::ALL {
            let palette = iced::theme::Palette::generate(mode.seed());
            assert_eq!(palette.is_dark, mode == ThemeMode::Dark);
            // The custom theme resolves back to the same seed.
            assert_eq!(mode.theme().seed(), mode.seed());
        }
    }
}
