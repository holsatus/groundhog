//! Which shell lays the window out, and the floating shell's state.
//!
//! The panels are the same in every shell (`Application::pane_body`); a shell
//! decides only where they go:
//!
//! - [`Shell::Tiled`]: the desktop workspace, three columns of panes around
//!   the map with a rail beside them (its state is [`crate::layout`]);
//! - [`Shell::Floating`]: a phone's navigation by task. A navigation bar
//!   (a rail in landscape) picks one of five [`Destination`]s. Flying and
//!   planning happen on the map, which runs under the platform's status
//!   bar, with the destination's panel in a sheet over it; the others are
//!   pages, and a page listing several panels opens each as a page of its
//!   own. Back steps out one level at a time.
//!
//! The operator picks one in the settings, or leaves it to
//! [`ShellMode::Automatic`], which floats on phones and in narrow windows.
//! It goes by the window's shortest side on touch screens, which rotating
//! the device does not change, so turning a phone keeps the shell and the
//! panel open in it.

use std::fmt;

use iced::Size;

use crate::{layout::Panel, ui::Icon};

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

/// Where the operator is in the floating shell: one of five tasks, chosen
/// in its navigation bar. The tiled shell's rail groups its panels the same
/// way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Destination {
    /// The selected vehicle, over the map.
    #[default]
    Fly,
    /// The mission, over the map.
    Plan,
    /// Configuring the vehicle: parameters, calibration.
    Setup,
    /// The links to vehicles.
    Links,
    /// The rest: the log, the settings.
    More,
}

impl Destination {
    pub const ALL: [Destination; 5] = [
        Destination::Fly,
        Destination::Plan,
        Destination::Setup,
        Destination::Links,
        Destination::More,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Destination::Fly => "Fly",
            Destination::Plan => "Plan",
            Destination::Setup => "Setup",
            Destination::Links => "Links",
            Destination::More => "More",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Destination::Fly => Icon::Vehicle,
            Destination::Plan => Icon::Mission,
            Destination::Setup => Icon::Setup,
            Destination::Links => Icon::Links,
            Destination::More => Icon::Others,
        }
    }

    /// The panels it holds, in the order they are listed.
    pub fn panels(self) -> &'static [Panel] {
        match self {
            Destination::Fly => &[Panel::Vehicles],
            Destination::Plan => &[Panel::Mission],
            Destination::Setup => &[Panel::Parameters, Panel::Calibration],
            Destination::Links => &[Panel::Links],
            Destination::More => &[Panel::Logs, Panel::Settings],
        }
    }

    /// Whether its work happens on the map, its panel in a sheet over it;
    /// the others are pages.
    pub fn on_map(self) -> bool {
        matches!(self, Destination::Fly | Destination::Plan)
    }
}

/// The floating shell's state: the destination, and where in it the
/// operator is. It is view state, not persisted.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Floating {
    destination: Destination,
    /// Whether a map destination shows its panel in a sheet.
    sheet: bool,
    /// Whether the sheet takes the whole window.
    expanded: bool,
    /// The panel opened from a page listing several (Setup's parameters).
    page: Option<Panel>,
}

/// What the floating shell shows besides the navigation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Screen {
    /// The map, with `panel` in a sheet over it if one is open.
    Map { sheet: Option<Panel> },
    /// A page listing the destination's panels.
    List(Destination),
    /// A page showing `panel`; `nested` when it was opened from a list, so
    /// it has a way back.
    Page { panel: Panel, nested: bool },
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    /// Go to a destination; going to the one shown returns to its start
    /// (or, on the map, shows or hides its sheet).
    Go(Destination),
    /// Open one of a list's panels.
    Open(Panel),
    /// Go back a step: the system's back gesture, or a page's back arrow.
    Back,
    /// Let the sheet take the whole window, or give the map its share back.
    ToggleExpanded,
    /// Close the sheet.
    Close,
}

impl Floating {
    pub fn destination(&self) -> Destination {
        self.destination
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// What to show for the destination.
    pub fn screen(&self) -> Screen {
        let destination = self.destination;
        let panels = destination.panels();

        if destination.on_map() {
            Screen::Map {
                sheet: panels.first().copied().filter(|_| self.sheet),
            }
        } else if let Some(panel) = self.page {
            Screen::Page {
                panel,
                nested: true,
            }
        } else if let [panel] = panels {
            Screen::Page {
                panel: *panel,
                nested: false,
            }
        } else {
            Screen::List(destination)
        }
    }

    /// Apply `message`; for [`Message::Back`], whether there was anywhere to
    /// go back to (if not, the application leaves, as a phone app does).
    pub fn update(&mut self, message: Message) -> bool {
        match message {
            Message::Go(destination) if destination == self.destination => {
                if destination.on_map() {
                    self.sheet = !self.sheet;
                } else {
                    self.page = None;
                }
            }
            Message::Go(destination) => {
                self.destination = destination;
                self.sheet = destination.on_map();
                self.page = None;
            }
            Message::Open(panel) => {
                if self.destination.panels().contains(&panel) {
                    self.page = Some(panel);
                }
            }
            Message::Back => return self.back(),
            Message::ToggleExpanded => self.expanded = !self.expanded,
            Message::Close => self.sheet = false,
        }

        // A sheet opens at its share of the window, never already expanded.
        if !self.sheet {
            self.expanded = false;
        }
        true
    }

    /// One step back: out of an expanded sheet, then the sheet, out of a
    /// nested page, then to the map. Nowhere to go from the bare map.
    fn back(&mut self) -> bool {
        if self.expanded {
            self.expanded = false;
        } else if self.destination.on_map() && self.sheet {
            self.sheet = false;
        } else if self.page.is_some() {
            self.page = None;
        } else if self.destination != Destination::Fly {
            self.destination = Destination::Fly;
            self.sheet = false;
        } else {
            return false;
        }
        true
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
    fn every_panel_but_the_map_has_one_destination() {
        let panels = [
            Panel::Map,
            Panel::Vehicles,
            Panel::Mission,
            Panel::Parameters,
            Panel::Calibration,
            Panel::Links,
            Panel::Logs,
            Panel::Settings,
        ];
        for panel in panels {
            let holding = Destination::ALL
                .iter()
                .filter(|destination| destination.panels().contains(&panel))
                .count();
            let expected = if panel == Panel::Map { 0 } else { 1 };
            assert_eq!(holding, expected, "{panel:?}");
        }
    }

    #[test]
    fn map_destinations_show_their_panel_in_a_sheet() {
        let mut floating = Floating::default();
        assert_eq!(floating.screen(), Screen::Map { sheet: None });

        floating.update(Message::Go(Destination::Plan));
        assert_eq!(
            floating.screen(),
            Screen::Map {
                sheet: Some(Panel::Mission)
            }
        );

        // Going to the destination shown hides and shows its sheet.
        floating.update(Message::Go(Destination::Plan));
        assert_eq!(floating.screen(), Screen::Map { sheet: None });
    }

    #[test]
    fn pages_list_several_panels_and_open_one() {
        let mut floating = Floating::default();

        floating.update(Message::Go(Destination::Links));
        assert_eq!(
            floating.screen(),
            Screen::Page {
                panel: Panel::Links,
                nested: false
            }
        );

        floating.update(Message::Go(Destination::Setup));
        assert_eq!(floating.screen(), Screen::List(Destination::Setup));
        floating.update(Message::Open(Panel::Calibration));
        assert_eq!(
            floating.screen(),
            Screen::Page {
                panel: Panel::Calibration,
                nested: true
            }
        );

        // Another destination's panel does not open here.
        floating.update(Message::Open(Panel::Logs));
        assert_eq!(
            floating.screen(),
            Screen::Page {
                panel: Panel::Calibration,
                nested: true
            }
        );

        // Going to the destination shown returns to its list.
        floating.update(Message::Go(Destination::Setup));
        assert_eq!(floating.screen(), Screen::List(Destination::Setup));
    }

    #[test]
    fn back_steps_out_one_level_at_a_time_and_then_leaves() {
        let mut floating = Floating::default();
        floating.update(Message::Go(Destination::More));
        floating.update(Message::Open(Panel::Settings));

        assert!(floating.update(Message::Back));
        assert_eq!(floating.screen(), Screen::List(Destination::More));
        assert!(floating.update(Message::Back));
        assert_eq!(floating.destination(), Destination::Fly);
        assert_eq!(floating.screen(), Screen::Map { sheet: None });
        assert!(!floating.update(Message::Back), "nowhere left to go");

        floating.update(Message::Go(Destination::Plan));
        floating.update(Message::Go(Destination::Fly));
        floating.update(Message::ToggleExpanded);
        assert!(floating.update(Message::Back));
        assert!(!floating.is_expanded());
        assert!(floating.update(Message::Back));
        assert_eq!(floating.screen(), Screen::Map { sheet: None });
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
