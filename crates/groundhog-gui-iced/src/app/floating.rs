//! The floating shell: navigation by task, as on a phone (see
//! [`crate::shell`]).
//!
//! A navigation bar is docked along the bottom (a rail along the left in
//! landscape), and beside it the destination's screen:
//!
//! - **the map**, for flying and planning: it runs under the platform's
//!   status bar, and the destination's panel floats over it in a sheet;
//! - **a page**: a top bar with the title (and a way back, for a page opened
//!   from a list), then a panel, or a list of the destination's panels.
//!
//! Everything that floats over the map is a [`surface`], so the presses that
//! land on it stay off the map, while the map keeps every press around it.

use iced::{
    Alignment, Element, Length, Padding, Size, Widget,
    widget::{Space, button, column, container, row, rule, scrollable, stack},
};

use super::{Application, Message};
use crate::{
    layout::Panel,
    map::Placement,
    shell::{self, Destination, Screen, SheetPlacement},
    ui::{
        self, Icon,
        style::{self, ButtonKind},
    },
    widget::surface::surface,
};

/// The window's size before it is first measured: a phone in portrait.
const ASSUMED_WINDOW: Size = Size::new(412.0, 915.0);

/// The height of the navigation bar, without the platform's inset beneath
/// it, and the width of the rail that takes its place in landscape.
const NAVIGATION_BAR: f32 = 64.0;
const NAVIGATION_RAIL: f32 = 80.0;
/// The height of a navigation item in the rail.
const RAIL_ITEM: f32 = 64.0;
/// The pill behind a navigation item's icon.
const INDICATOR: Size = Size::new(56.0, 32.0);
/// The height of a page's top bar.
const TOP_BAR: f32 = 56.0;

/// Where the pieces of the floating shell go in the window.
struct Geometry {
    /// The platform's chrome, in the application's pixels.
    insets: Padding,
    /// Whether the window is wider than tall: the rail goes along the left,
    /// and the sheet beside it.
    landscape: bool,
    /// The size of the screen beside the navigation.
    screen: Size,
    /// The insets on the screen's own edges (the navigation takes the rest).
    screen_insets: Padding,
}

impl Geometry {
    /// The gap that what floats keeps from the screen's edges, or from the
    /// platform's chrome where it covers them.
    fn edges(&self) -> Padding {
        let insets = self.screen_insets;
        Padding {
            top: insets.top + ui::space::S,
            right: insets.right + ui::space::S,
            bottom: insets.bottom + ui::space::S,
            left: insets.left + ui::space::S,
        }
    }

    /// Where a sheet goes on the map, if one is open and not expanded.
    fn sheet(&self, expanded: bool) -> Option<SheetPlacement> {
        let edges = self.edges();
        let area = Size::new(
            self.screen.width - edges.left - edges.right,
            self.screen.height - edges.top - edges.bottom,
        );
        SheetPlacement::of(area, expanded)
    }

    /// How much of each edge of the map is hidden under the platform's
    /// chrome and a sheet placed as `sheet`.
    fn covered(&self, sheet: Option<SheetPlacement>) -> Padding {
        let edges = self.edges();
        let mut covered = self.screen_insets;
        match sheet {
            Some(SheetPlacement::Bottom(height)) => covered.bottom = edges.bottom + height,
            Some(SheetPlacement::Side(width)) => covered.left = edges.left + width,
            None => {}
        }
        covered
    }
}

impl Application {
    fn geometry(&self) -> Geometry {
        let window = self.window.unwrap_or(ASSUMED_WINDOW);
        let insets = self.insets.padding(self.scale_factor());
        let landscape = window.width > window.height;

        // The navigation sits on the platform's inset on its own edge.
        let (screen, screen_insets) = if landscape {
            (
                Size::new(window.width - NAVIGATION_RAIL - insets.left, window.height),
                Padding {
                    left: 0.0,
                    ..insets
                },
            )
        } else {
            (
                Size::new(window.width, window.height - NAVIGATION_BAR - insets.bottom),
                Padding {
                    bottom: 0.0,
                    ..insets
                },
            )
        };

        Geometry {
            insets,
            landscape,
            screen,
            screen_insets,
        }
    }

    /// How much of each edge of the map floats under something.
    pub(super) fn floating_covered(&self) -> Padding {
        let geometry = self.geometry();
        let sheet = match self.floating.screen() {
            Screen::Map { sheet: Some(_) } => geometry.sheet(self.floating.is_expanded()),
            _ => None,
        };
        geometry.covered(sheet)
    }

    pub(super) fn view_floating(&self) -> Element<'_, Message> {
        let geometry = self.geometry();

        let screen = match self.floating.screen() {
            Screen::Map { sheet } => self.view_map_screen(sheet, &geometry),
            Screen::List(destination) => {
                let entries = destination.panels().iter().map(|panel| {
                    ui::entry(
                        panel.icon(),
                        panel.title(),
                        panel.summary(),
                        Message::Floating(shell::Message::Open(*panel)),
                    )
                });
                let list = ui::panel(scrollable(column(entries).spacing(ui::space::XS)));
                self.view_page(destination.title(), false, list, &geometry)
            }
            Screen::Page { panel, nested } => {
                // A destination of one panel is titled as the destination.
                let title = if nested {
                    panel.title()
                } else {
                    self.floating.destination().title()
                };
                self.view_page(title, nested, self.pane_body(panel), &geometry)
            }
        };

        let navigation = self.view_navigation(&geometry);
        if geometry.landscape {
            row![navigation, screen].boxed()
        } else {
            column![screen, navigation].boxed()
        }
    }

    /// The map, with `sheet` over it if one is open, and the toasts in the
    /// corner of what shows of the map.
    fn view_map_screen(&self, sheet: Option<Panel>, geometry: &Geometry) -> Element<'_, Message> {
        let expanded = self.floating.is_expanded();
        let placement = sheet.and_then(|_| geometry.sheet(expanded));
        let covered = geometry.covered(placement);

        let sheet = sheet.map(|panel| self.view_sheet(panel));
        let chrome: Element<'_, Message> = match (sheet, placement) {
            (Some(sheet), Some(SheetPlacement::Bottom(height))) => column![
                Space::new().height(Length::Fill),
                container(sheet).height(height).width(Length::Fill),
            ]
            .boxed(),
            (Some(sheet), Some(SheetPlacement::Side(width))) => row![
                container(sheet).width(width).height(Length::Fill),
                Space::new().width(Length::Fill),
            ]
            .boxed(),
            (Some(sheet), None) => sheet,
            (None, _) => Space::new().boxed(),
        };
        let chrome = container(chrome)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(geometry.edges());

        let toasts = self.view_toasts_within(covered);

        // A wash under the status bar, so its icons stay legible over any
        // tile.
        let scrim = container(Space::new())
            .width(Length::Fill)
            .height(geometry.insets.top)
            .style(style::scrim);

        let map = self.view_map(Placement::Window { covered });

        container(stack![map, scrim, chrome, toasts])
            .width(geometry.screen.width)
            .height(geometry.screen.height)
            .boxed()
    }

    /// A page: a top bar with `title` (and a back arrow, if `nested`), and
    /// `body` beneath it.
    fn view_page<'a>(
        &'a self,
        title: &'a str,
        nested: bool,
        body: Element<'a, Message>,
        geometry: &Geometry,
    ) -> Element<'a, Message> {
        let back = nested.then(|| {
            ui::icon_button(Icon::Back)
                .hint("Back")
                .on_press(Message::Floating(shell::Message::Back))
                .boxed()
        });
        let top_bar = container(
            row![back, ui::title(title)]
                .spacing(ui::space::S)
                .align_y(Alignment::Center),
        )
        .padding([0.0, ui::space::M])
        .center_y(TOP_BAR);

        let page = container(
            column![
                top_bar,
                rule::horizontal(style::PANE_BORDER_WIDTH).style(style::pane_divider),
                body,
            ]
            .height(Length::Fill),
        )
        .padding(geometry.screen_insets)
        .width(geometry.screen.width)
        .height(geometry.screen.height)
        .style(style::page);

        stack![page, self.view_toasts_within(geometry.screen_insets)].boxed()
    }

    /// The toasts, in the bottom-right corner of what `covered` leaves.
    fn view_toasts_within(&self, covered: Padding) -> Element<'_, Message> {
        container(self.notices.view().map(Message::Notifications))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(Padding {
                top: covered.top + ui::space::S,
                right: covered.right + ui::space::S,
                bottom: covered.bottom + ui::space::S,
                left: covered.left + ui::space::S,
            })
            .boxed()
    }

    /// The destinations, each an icon in a pill (filled for the one shown)
    /// over its name: a bar along the bottom, docked on the platform's
    /// inset, or a rail along the left in landscape.
    fn view_navigation(&self, geometry: &Geometry) -> Element<'_, Message> {
        let current = self.floating.destination();
        let item = |destination: Destination, height: f32| {
            let chosen = destination == current;
            let indicator = container(ui::icon::icon(
                destination.icon(),
                ui::size::ICON,
                ButtonKind::Subtle,
                true,
            ))
            .center_x(INDICATOR.width)
            .center_y(INDICATOR.height)
            .style(style::navigation_indicator(chosen));
            let content = column![indicator, ui::caption(destination.title())]
                .spacing(ui::space::XS)
                .align_x(Alignment::Center);

            button(container(content).center(Length::Fill))
                .padding(0)
                .width(Length::Fill)
                .height(height)
                .style(style::navigation_item)
                .on_press(Message::Floating(shell::Message::Go(destination)))
                .boxed()
        };

        let insets = geometry.insets;
        if geometry.landscape {
            let items = Destination::ALL
                .into_iter()
                .map(|destination| item(destination, RAIL_ITEM));
            container(column(items).spacing(ui::space::XS))
                .center_y(Length::Fill)
                .width(NAVIGATION_RAIL + insets.left)
                .padding(Padding {
                    top: insets.top,
                    right: 0.0,
                    bottom: insets.bottom,
                    left: insets.left,
                })
                .style(style::navigation)
                .boxed()
        } else {
            let items = Destination::ALL
                .into_iter()
                .map(|destination| item(destination, NAVIGATION_BAR));
            container(row(items))
                .width(Length::Fill)
                .height(NAVIGATION_BAR + insets.bottom)
                .padding(Padding {
                    top: 0.0,
                    right: insets.right,
                    bottom: insets.bottom,
                    left: insets.left,
                })
                .style(style::navigation)
                .boxed()
        }
    }

    /// `panel` in a sheet: a pane with its title, a button to let it take
    /// the whole window (or give the map its share back) and one to close it.
    fn view_sheet(&self, panel: Panel) -> Element<'_, Message> {
        let floating = |message| Message::Floating(message);
        let expand = if self.floating.is_expanded() {
            ui::icon_button(Icon::Minimize).hint("Show the map")
        } else {
            ui::icon_button(Icon::Maximize).hint("Expand")
        }
        .on_press(floating(shell::Message::ToggleExpanded));
        let close = ui::icon_button(Icon::Close)
            .hint("Close")
            .on_press(floating(shell::Message::Close));

        // As a pane's title bar: the controls as far from the right edge as
        // from the top and bottom, the border's width added where it is
        // drawn over the header.
        let header = container(
            row![
                ui::icon::icon(panel.icon(), ui::size::ICON, ButtonKind::Subtle, true),
                ui::text(panel.title()).width(Length::Fill),
                expand.boxed(),
                close.boxed(),
            ]
            .spacing(ui::space::S)
            .align_y(Alignment::Center),
        )
        .padding(Padding {
            top: ui::space::XS + style::PANE_BORDER_WIDTH,
            right: ui::space::XS + style::PANE_BORDER_WIDTH,
            bottom: ui::space::XS,
            left: ui::space::M,
        })
        .style(style::title_bar(false));

        let body = column![
            header,
            rule::horizontal(style::PANE_BORDER_WIDTH).style(style::pane_divider),
            self.pane_body(panel),
        ]
        .height(Length::Fill);

        surface(
            container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(style::floating),
        )
        .boxed()
    }
}
