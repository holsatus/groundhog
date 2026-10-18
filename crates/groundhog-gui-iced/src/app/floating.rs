//! The floating shell: the map fills the window, under the platform's status
//! and navigation bars, and the navigation bar, the open panel's sheet and
//! the toasts float over it (see [`crate::shell`]).
//!
//! Everything that floats is a [`surface`], so the presses that land on it
//! stay off the map, while the map keeps every press around it.

use iced::{
    Alignment, Element, Length, Padding, Size, Widget,
    widget::{Space, button, column, container, row, rule, scrollable, stack},
};

use super::{Application, Message};
use crate::{
    layout::Panel,
    map::Placement,
    shell::{self, SheetPlacement},
    ui::{
        self, Icon,
        style::{self, ButtonKind},
    },
    widget::surface::surface,
};

/// The window's size before it is first measured: a phone in portrait.
const ASSUMED_WINDOW: Size = Size::new(412.0, 915.0);

/// How thick the navigation bar is: a button, and the bar's padding.
const NAV_THICKNESS: f32 = ui::size::RAIL_BUTTON + 2.0 * ui::space::XS;

/// Where the floating pieces go in the window.
struct Geometry {
    /// The platform's chrome, in the application's pixels.
    insets: Padding,
    /// The gap the pieces keep from the window's edges, or from the
    /// platform's chrome where it covers them.
    edges: Padding,
    /// Whether the window is wider than tall: the navigation bar and the
    /// sheet go along the side instead of the bottom.
    landscape: bool,
    /// Where the sheet goes, if one is open and not expanded.
    sheet: Option<SheetPlacement>,
    /// How much of each edge of the map is hidden under the platform's
    /// chrome, the navigation bar and the sheet.
    covered: Padding,
}

impl Application {
    fn geometry(&self) -> Geometry {
        let window = self.window.unwrap_or(ASSUMED_WINDOW);
        let insets = self.insets.padding(self.scale_factor());
        let edges = Padding {
            top: insets.top + ui::space::S,
            right: insets.right + ui::space::S,
            bottom: insets.bottom + ui::space::S,
            left: insets.left + ui::space::S,
        };
        let area = Size::new(
            window.width - edges.left - edges.right,
            window.height - edges.top - edges.bottom,
        );
        let landscape = area.width > area.height;
        let sheet = self
            .floating
            .sheet()
            .and_then(|_| SheetPlacement::of(area, self.floating.is_expanded()));

        // An expanded sheet hides the whole map; what it covers then does
        // not matter, and the map is placed as if the sheet were closed.
        let sheet_extent = match sheet {
            Some(SheetPlacement::Bottom(extent) | SheetPlacement::Side(extent)) => {
                extent + ui::space::S
            }
            None => 0.0,
        };
        let covered = if landscape {
            Padding {
                left: edges.left + NAV_THICKNESS + sheet_extent,
                ..insets
            }
        } else {
            Padding {
                bottom: edges.bottom + NAV_THICKNESS + sheet_extent,
                ..insets
            }
        };

        Geometry {
            insets,
            edges,
            landscape,
            sheet,
            covered,
        }
    }

    /// How much of each edge of the map floats under something.
    pub(super) fn floating_covered(&self) -> Padding {
        self.geometry().covered
    }

    pub(super) fn view_floating(&self) -> Element<'_, Message> {
        let Geometry {
            insets,
            edges,
            landscape,
            sheet: placement,
            covered,
        } = self.geometry();

        let nav = self.view_nav(landscape);
        let sheet = self.floating.sheet().map(|panel| self.view_sheet(panel));

        let chrome: Element<'_, Message> = if landscape {
            let nav = container(nav).center_y(Length::Fill);
            match (sheet, placement) {
                (Some(sheet), Some(SheetPlacement::Side(width))) => row![
                    nav,
                    container(sheet).width(width).height(Length::Fill),
                    Space::new().width(Length::Fill),
                ]
                .spacing(ui::space::S)
                .boxed(),
                (Some(sheet), _) => row![nav, sheet].spacing(ui::space::S).boxed(),
                (None, _) => row![nav, Space::new().width(Length::Fill)].boxed(),
            }
        } else {
            let nav = container(nav).center_x(Length::Fill);
            match (sheet, placement) {
                (Some(sheet), Some(SheetPlacement::Bottom(height))) => column![
                    Space::new().height(Length::Fill),
                    container(sheet).height(height).width(Length::Fill),
                    nav,
                ]
                .spacing(ui::space::S)
                .boxed(),
                (Some(sheet), _) => column![sheet, nav].spacing(ui::space::S).boxed(),
                (None, _) => column![Space::new().height(Length::Fill), nav].boxed(),
            }
        };

        let chrome = container(chrome)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(edges);

        // The toasts pop up in the corner of the map that shows, as they do
        // in the tiled shell's map pane.
        let toasts = container(self.notices.view().map(Message::Notifications))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(Padding {
                top: covered.top + ui::space::S,
                right: covered.right + ui::space::S,
                bottom: covered.bottom + ui::space::S,
                left: covered.left + ui::space::S,
            });

        let scrim = container(Space::new())
            .width(Length::Fill)
            .height(insets.top)
            .style(style::scrim);

        let map = self.view_map(Placement::Window { covered });

        stack![map, scrim, chrome, toasts].boxed()
    }

    /// A button per panel, the open one highlighted: a row along the
    /// bottom of a portrait window, a column along the side of a landscape
    /// one. It scrolls if the window is too small for all of them.
    fn view_nav(&self, vertical: bool) -> Element<'_, Message> {
        let buttons = Panel::DOCKABLE.into_iter().map(|panel| {
            let open = self.floating.sheet() == Some(panel);
            let icon = ui::icon::icon(panel.icon(), ui::size::RAIL_ICON, ButtonKind::Subtle, true);
            let button = button(container(icon).center(Length::Fill))
                .padding(0)
                .width(ui::size::RAIL_BUTTON)
                .height(ui::size::RAIL_BUTTON)
                .style(style::rail_button(open))
                .on_press(Message::Floating(shell::Message::Select(panel)));

            let side = if vertical {
                iced::widget::tooltip::Position::Right
            } else {
                iced::widget::tooltip::Position::Top
            };
            ui::tooltip(button, panel.title(), side)
        });

        let hidden = scrollable::Scrollbar::hidden();
        let bar: Element<'_, Message> = if vertical {
            scrollable(column(buttons).spacing(ui::space::XS))
                .direction(scrollable::Direction::Vertical(hidden))
                .boxed()
        } else {
            scrollable(row(buttons).spacing(ui::space::XS))
                .direction(scrollable::Direction::Horizontal(hidden))
                .boxed()
        };

        surface(container(bar).padding(ui::space::XS).style(style::floating)).boxed()
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
