//! Which shell lays the window out, and the floating shell's state.
//!
//! The panels are the same in every shell (`Application::pane_body`); a shell
//! decides only where they go:
//!
//! - [`Shell::Tiled`]: the desktop workspace, three columns of panes around
//!   the map with a rail beside them (its state is [`crate::layout`]);
//! - [`Shell::Floating`]: the map fills the whole window, under the
//!   platform's status and navigation bars, and everything else floats over
//!   it: a navigation bar choosing one panel, and that panel in a sheet
//!   (along the bottom in portrait, along the side in landscape).
//!
//! The operator picks one in the settings, or leaves it to
//! [`ShellMode::Automatic`], which floats on phones and in narrow windows.
//! It goes by the window's shortest side on touch screens, which rotating
//! the device does not change, so turning a phone keeps the shell and the
//! panel open in it.

use std::fmt;

use iced::Size;

use crate::layout::Panel;

/// Below this window width the workspace has no room for a side column next
/// to the map, whatever the platform.
pub const FLOATING_BELOW_WIDTH: f32 = 640.0;

/// On a touch screen, a window whose shortest side is below this is a
/// phone's: too small to tile, in either orientation.
pub const FLOATING_BELOW_SIDE: f32 = 600.0;

/// How the window is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Tiled,
    Floating,
}

/// The operator's choice of [`Shell`], as persisted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ShellMode {
    /// Floating on phones and in narrow windows, tiled otherwise.
    #[default]
    #[serde(rename = "Automatic")]
    Automatic,
    #[serde(rename = "Tiled")]
    Tiled,
    #[serde(rename = "Floating")]
    Floating,
}

impl ShellMode {
    pub const ALL: [ShellMode; 3] = [ShellMode::Automatic, ShellMode::Tiled, ShellMode::Floating];

    /// The shell for a window of `window` size (once measured), on a touch
    /// screen or not.
    pub fn resolve(self, window: Option<Size>, touch: bool) -> Shell {
        match self {
            ShellMode::Tiled => Shell::Tiled,
            ShellMode::Floating => Shell::Floating,
            ShellMode::Automatic => {
                let floating = match window {
                    Some(size) => {
                        size.width < FLOATING_BELOW_WIDTH
                            || (touch && size.width.min(size.height) < FLOATING_BELOW_SIDE)
                    }
                    // Before the window is measured, the platform is the best
                    // guess; a tablet corrects it on the first layout.
                    None => touch,
                };

                if floating {
                    Shell::Floating
                } else {
                    Shell::Tiled
                }
            }
        }
    }
}

impl fmt::Display for ShellMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ShellMode::Automatic => "Automatic",
            ShellMode::Tiled => "Tiled panels",
            ShellMode::Floating => "Floating over the map",
        })
    }
}

/// The floating shell's state: which panel the sheet shows, and how much
/// of the window it takes. It is view state, not persisted.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Floating {
    sheet: Option<Panel>,
    expanded: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    /// Show `panel` in the sheet, or close the sheet if it shows it already.
    Select(Panel),
    Close,
    /// Let the sheet take the whole window, or give the map its share back.
    ToggleExpanded,
}

impl Floating {
    /// The panel in the sheet, if it is open.
    pub fn sheet(&self) -> Option<Panel> {
        self.sheet
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Select(panel) if panel.is_fixed() => {}
            Message::Select(panel) if self.sheet == Some(panel) => self.sheet = None,
            Message::Select(panel) => self.sheet = Some(panel),
            Message::Close => self.sheet = None,
            Message::ToggleExpanded => self.expanded = !self.expanded,
        }

        // A sheet opens at its share of the window, never already expanded.
        if self.sheet.is_none() {
            self.expanded = false;
        }
    }
}

/// How a floating sheet sits in a window of `window` size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SheetPlacement {
    /// Along the bottom, this tall.
    Bottom(f32),
    /// Along the left side, this wide.
    Side(f32),
}

/// The share of a portrait window's height a sheet takes, leaving the rest
/// to the map.
const SHEET_HEIGHT: f32 = 0.5;
/// The width of a sheet beside the map in landscape, and the largest share
/// of the window it may take.
const SHEET_WIDTH: f32 = 380.0;
const SHEET_MAX_WIDTH: f32 = 0.5;

impl SheetPlacement {
    /// Where a sheet goes in `window` (the area left beside the navigation
    /// bar, inside the platform's chrome): along the bottom of a portrait
    /// window, along the side of a landscape one. `None` for an expanded
    /// sheet, which fills the area.
    pub fn of(window: Size, expanded: bool) -> Option<Self> {
        if expanded {
            return None;
        }

        Some(if window.width > window.height {
            SheetPlacement::Side(SHEET_WIDTH.min(window.width * SHEET_MAX_WIDTH))
        } else {
            SheetPlacement::Bottom(window.height * SHEET_HEIGHT)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE_PORTRAIT: Size = Size::new(412.0, 915.0);
    const PHONE_LANDSCAPE: Size = Size::new(915.0, 412.0);
    const TABLET: Size = Size::new(1280.0, 800.0);
    const DESKTOP: Size = Size::new(1600.0, 900.0);

    #[test]
    fn a_phone_floats_in_both_orientations() {
        let automatic = ShellMode::Automatic;
        assert_eq!(
            automatic.resolve(Some(PHONE_PORTRAIT), true),
            Shell::Floating
        );
        assert_eq!(
            automatic.resolve(Some(PHONE_LANDSCAPE), true),
            Shell::Floating
        );
        assert_eq!(automatic.resolve(None, true), Shell::Floating);
    }

    #[test]
    fn large_screens_tile_and_narrow_windows_float() {
        let automatic = ShellMode::Automatic;
        assert_eq!(automatic.resolve(Some(TABLET), true), Shell::Tiled);
        assert_eq!(automatic.resolve(Some(DESKTOP), false), Shell::Tiled);
        assert_eq!(automatic.resolve(None, false), Shell::Tiled);

        // A short desktop window is not a phone; a narrow one floats.
        assert_eq!(
            automatic.resolve(Some(PHONE_LANDSCAPE), false),
            Shell::Tiled
        );
        assert_eq!(
            automatic.resolve(Some(PHONE_PORTRAIT), false),
            Shell::Floating
        );
    }

    #[test]
    fn the_operator_can_choose_either_shell() {
        assert_eq!(
            ShellMode::Floating.resolve(Some(DESKTOP), false),
            Shell::Floating
        );
        assert_eq!(
            ShellMode::Tiled.resolve(Some(PHONE_PORTRAIT), true),
            Shell::Tiled
        );
    }

    #[test]
    fn selecting_the_open_panel_closes_the_sheet() {
        let mut floating = Floating::default();

        floating.update(Message::Select(Panel::Mission));
        assert_eq!(floating.sheet(), Some(Panel::Mission));

        floating.update(Message::Select(Panel::Links));
        assert_eq!(floating.sheet(), Some(Panel::Links));

        floating.update(Message::Select(Panel::Links));
        assert_eq!(floating.sheet(), None);

        floating.update(Message::Select(Panel::Map));
        assert_eq!(floating.sheet(), None, "the map is always there");
    }

    #[test]
    fn a_sheet_reopens_at_its_share() {
        let mut floating = Floating::default();
        floating.update(Message::Select(Panel::Mission));
        floating.update(Message::ToggleExpanded);
        assert!(floating.is_expanded());

        floating.update(Message::Close);
        floating.update(Message::Select(Panel::Mission));
        assert!(!floating.is_expanded());
    }

    #[test]
    fn sheets_sit_along_the_bottom_in_portrait_and_the_side_in_landscape() {
        assert_eq!(
            SheetPlacement::of(PHONE_PORTRAIT, false),
            Some(SheetPlacement::Bottom(PHONE_PORTRAIT.height * SHEET_HEIGHT))
        );
        assert_eq!(
            SheetPlacement::of(PHONE_LANDSCAPE, false),
            Some(SheetPlacement::Side(SHEET_WIDTH))
        );
        assert_eq!(SheetPlacement::of(PHONE_PORTRAIT, true), None);
    }
}
