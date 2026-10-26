//! The tiled shell: the rail beside the workspace's three columns of panes
//! around the map (see [`crate::layout`]).

use iced::{
    Alignment, Element, Length, Padding, Widget,
    widget::{Space, button, column, container, mouse_area, row, sensor, stack, tooltip},
};

use super::{Application, Message};
use crate::{
    layout,
    shell::Destination,
    ui::{
        self, Icon,
        style::{self, ButtonKind},
    },
    widget::{
        drag_source::drag_source,
        pane_grid::{self, PaneGrid},
    },
};

impl Application {
    pub(super) fn view_tiled(&self) -> Element<'_, Message> {
        // The rail and the panes float on the window's background, one gap
        // apart from each other and from the window's edges. Where the
        // platform's chrome covers the window, the gap is measured from the
        // chrome instead.
        let insets = self.insets.padding(self.scale_factor());
        let base = container(row![self.view_rail(), self.view_panes()].spacing(ui::space::S))
            .padding(Padding {
                top: insets.top.max(ui::space::S),
                right: insets.right.max(ui::space::S),
                bottom: insets.bottom.max(ui::space::S),
                left: insets.left.max(ui::space::S),
            })
            .style(style::workspace);

        // Toasts pop up in the map's corner, unless a maximized pane hides
        // the map. The layer is there either way, so maximizing a pane does
        // not change the shape of the tree and rebuild every pane's state.
        let toasts = if self.map_is_shown() {
            Space::new().boxed()
        } else {
            self.view_toasts(ui::space::XL).boxed()
        };
        stack![base, toasts].boxed()
    }

    /// Whether the map is on screen: no other pane is maximized.
    fn map_is_shown(&self) -> bool {
        let panes = self.layout.panes();
        panes
            .maximized()
            .is_none_or(|pane| panes.get(pane) == Some(&layout::Panel::Map))
    }

    /// The rail: a button per panel, highlighted while it is open. Pressing
    /// an open panel's button closes it. A closed panel opens in the left,
    /// middle or right column by the mouse button pressed (or a plain,
    /// Ctrl/Cmd or Shift click), or where its button is dragged onto the
    /// grid.
    fn view_rail(&self) -> impl Widget<Message> + '_ {
        let button_of = |panel: layout::Panel| {
            let open = self.layout.is_open(panel);
            let icon = ui::icon::icon(panel.icon(), ui::size::RAIL_ICON, ButtonKind::Subtle, true);
            let button = button(container(icon).center(Length::Fill))
                .padding(0)
                .width(ui::size::RAIL_BUTTON)
                .height(ui::size::RAIL_BUTTON)
                .style(style::rail_button(open))
                .on_press(Message::Rail(panel));
            let toggle = |slot| Message::Layout(layout::Message::Toggle(panel, slot));
            let button = mouse_area(button)
                .on_middle_press(toggle(layout::Slot::Middle))
                .on_right_press(toggle(layout::Slot::Right));

            // Dragged, the button becomes a label for the panel, as its pane does.
            let button = drag_source(
                button,
                ui::drag_label(panel.icon(), panel.title()),
                Message::RailDragged(panel),
                Message::RailDropped,
            );

            ui::tooltip(button, panel.title(), tooltip::Position::Right)
        };

        // In the order the floating shell's destinations list them.
        let buttons = Destination::ALL
            .into_iter()
            .flat_map(|destination| destination.panels().iter().copied())
            .map(button_of);
        column(buttons).spacing(ui::space::S)
    }

    /// The centre of `panel`'s button on the rail, from the rail's top.
    #[cfg(test)]
    pub(super) fn rail_button_centre(panel: layout::Panel) -> f32 {
        let index = Destination::ALL
            .into_iter()
            .flat_map(|destination| destination.panels().iter().copied())
            .position(|candidate| candidate == panel)
            .unwrap_or_default();
        index as f32 * (ui::size::RAIL_BUTTON + ui::space::S) + ui::size::RAIL_BUTTON / 2.0
    }

    /// The panes, as the layout arranges them. The layout is told the
    /// grid's size, which it needs to keep the side columns' widths in
    /// pixels.
    fn view_panes(&self) -> impl Widget<Message> + '_ {
        let sized = |size| Message::Layout(layout::Message::Sized(size));
        let mut grid = PaneGrid::new(self.layout.panes(), |pane, panel, is_maximized| {
            // The map paints opaque tiles across its whole pane, which a rounded
            // container cannot clip; it draws its own frame instead (see
            // `map::Map::view`), and has no title bar to grab, so it cannot be
            // moved or closed.
            if panel.is_fixed() {
                let empty = Space::new().height(Length::Fixed(0.0)).boxed();
                return pane_grid::Content::new(self.pane_body(*panel))
                    .title_bar(pane_grid::TitleBar::new(empty).padding(0));
            }

            // Every other pane is a rounded card, its header split off by a rule.
            let body = self.pane_body(*panel);

            let layout = |message| Message::Layout(message);
            let size = if is_maximized {
                ui::icon_button(Icon::Minimize)
                    .hint("Restore")
                    .on_press(layout(layout::Message::Restore))
            } else {
                ui::icon_button(Icon::Maximize)
                    .hint("Maximize")
                    .on_press(layout(layout::Message::Maximize(pane)))
            };
            let close = ui::icon_button(Icon::Close)
                .hint("Close")
                .on_press(layout(layout::Message::Close(pane)));

            let title = row![
                ui::icon::icon(panel.icon(), ui::size::ICON, ButtonKind::Subtle, true),
                ui::text(panel.title())
            ]
            .spacing(ui::space::S)
            .align_y(Alignment::Center);

            let title_bar = pane_grid::TitleBar::new(title.boxed())
                .controls(pane_grid::Controls::new(
                    row![size.boxed(), close.boxed()].spacing(ui::space::XS),
                ))
                // The controls are as far from the bar's right edge as from
                // its top and bottom; the title gets more room on the left.
                // The pane's border is drawn over the bar's top and right
                // edges, while the divider lies below it, so those sides add
                // the border's width.
                .padding(Padding {
                    top: ui::space::XS + style::PANE_BORDER_WIDTH,
                    right: ui::space::XS + style::PANE_BORDER_WIDTH,
                    bottom: ui::space::XS,
                    left: ui::space::M,
                })
                .style(style::title_bar(self.layout.is_focused(pane)));

            // Dragged by its title bar, the pane stays put and a label for
            // it follows the cursor, as when its rail button is dragged.
            pane_grid::Content::new(body.boxed())
                .style(style::pane)
                .title_bar(title_bar)
                .preview(ui::drag_label(panel.icon(), panel.title()))
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .spacing(layout::SPACING)
        .min_size(layout::MIN_PANE)
        .style(style::pane_grid)
        .on_click(|pane| Message::Layout(layout::Message::Clicked(pane)))
        .on_drag(|event| Message::Layout(layout::Message::Dragged(event)))
        .drop_target(|dragged, cursor, regions| self.layout.drop_hint(dragged, cursor, regions))
        .on_resize(8, |event| Message::Layout(layout::Message::Resized(event)));

        if let Some(panel) = self.rail_drag {
            grid = grid.incoming(
                move |cursor, regions| self.layout.incoming_hint(panel, cursor, regions),
                move |target| Message::Layout(layout::Message::Placed(panel, target)),
            );
        }

        sensor(grid).on_show(sized).on_resize(sized)
    }
}
