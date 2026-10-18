//! A button that opens a menu: the overflow of a header too narrow for its
//! icon actions, among others.
//!
//! The menu opens below its button (above it, where the window has no room
//! below), right-aligned to it. Pressing anywhere outside closes it without
//! the press reaching what lies beneath, as a menu on a phone does; a press
//! released inside it (choosing an entry) closes it too, and so does
//! Escape.

use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Meta, Operation, Tree, tree},
    },
    keyboard, touch,
};

/// The gap between the button and its menu.
const GAP: f32 = 4.0;

/// `trigger`, which opens `menu` when pressed. `trigger` is drawn as given
/// and does not see the press; `menu`'s entries publish their own messages.
pub fn menu_button<'a, M: 'a>(
    trigger: impl Widget<M, Theme, Renderer> + 'a,
    menu: impl Widget<M, Theme, Renderer> + 'a,
) -> MenuButton<'a, M> {
    use iced::Widget as _;

    MenuButton {
        children: vec![trigger.boxed(), menu.boxed()],
    }
}

pub struct MenuButton<'a, Message> {
    /// `[trigger, menu]`.
    children: Vec<Element<'a, Message>>,
}

const TRIGGER: usize = 0;
const MENU: usize = 1;

#[derive(Default)]
struct State {
    open: bool,
}

/// Whether `event` presses at a point, and where; `cursor` stands in for
/// the mouse.
fn press(event: &Event, cursor: mouse::Cursor) -> Option<Point> {
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(_)) => cursor.position(),
        Event::Touch(touch::Event::FingerPressed { position, .. }) => Some(*position),
        _ => None,
    }
}

/// Whether `event` releases at a point, and where.
fn release(event: &Event, cursor: mouse::Cursor) -> Option<Point> {
    match event {
        Event::Mouse(mouse::Event::ButtonReleased(_)) => cursor.position(),
        Event::Touch(touch::Event::FingerLifted { position, .. }) => Some(*position),
        _ => None,
    }
}

impl<Message> Meta for MenuButton<'_, Message> {}

impl<Message> Widget<Message, Theme, Renderer> for MenuButton<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        self.children[TRIGGER].size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.children[TRIGGER].layout(&mut tree.children[TRIGGER], renderer, limits);
        tree.size = tree.children[TRIGGER].size;
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.children[TRIGGER].operate(
            &mut tree.children[TRIGGER],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();

        if let Some(at) = press(event, cursor)
            && layout.bounds().contains(at)
        {
            state.open = !state.open;
            shell.capture_event();
            shell.request_redraw();
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.children[TRIGGER].draw(
            &tree.children[TRIGGER],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        let Tree {
            state, children, ..
        } = tree;
        let state = state.downcast_mut::<State>();
        if !state.open {
            return Vec::new();
        }

        let menu = &mut self.children[MENU];
        let menu_tree = &mut children[MENU];
        let trigger = layout.bounds() + translation;

        let below = window.height - (trigger.y + trigger.height + GAP);
        let above = trigger.y - GAP;
        let limits = layout::Limits::new(
            Size::ZERO,
            Size::new(window.width, below.max(above).max(0.0)),
        );
        menu.layout(menu_tree, renderer, &limits);
        let size = menu_tree.size;

        let x = (trigger.x + trigger.width - size.width)
            .clamp(0.0, (window.width - size.width).max(0.0));
        let y = if below >= size.height || below >= above {
            trigger.y + trigger.height + GAP
        } else {
            trigger.y - GAP - size.height
        };

        vec![overlay::Element::new(Box::new(Menu {
            menu,
            tree: menu_tree,
            layout: Layout::new(size).move_to(Point::new(x, y)),
            open: &mut state.open,
        }))]
    }
}

/// The open menu.
struct Menu<'a, 'b, Message> {
    menu: &'a mut Element<'b, Message>,
    tree: &'a mut Tree,
    layout: Layout,
    open: &'a mut bool,
}

impl<Message> overlay::Overlay<Message, Theme, Renderer> for Menu<'_, '_, Message> {
    fn update(
        &mut self,
        event: &Event,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = self.layout.bounds();

        if matches!(
            event,
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            })
        ) {
            *self.open = false;
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        if let Some(at) = press(event, cursor)
            && !bounds.contains(at)
        {
            // A press outside closes the menu and goes no further. (The
            // button's own press is outside too: it closes here, and does
            // not reopen.)
            *self.open = false;
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        let released = release(event, cursor).is_some_and(|at| bounds.contains(at));

        self.menu.update(
            self.tree,
            event,
            self.layout,
            cursor,
            renderer,
            shell,
            &bounds,
        );

        if released {
            *self.open = false;
            shell.request_redraw();
        }
    }

    fn mouse_interaction(&self, cursor: mouse::Cursor, renderer: &Renderer) -> mouse::Interaction {
        let bounds = self.layout.bounds();
        let interaction =
            self.menu
                .mouse_interaction(self.tree, self.layout, cursor, &bounds, renderer);

        if interaction == mouse::Interaction::None && cursor.is_over(bounds) {
            mouse::Interaction::Idle
        } else {
            interaction
        }
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;

        let bounds = self.layout.bounds();
        renderer.with_layer(bounds, |renderer| {
            self.menu.draw(
                self.tree,
                renderer,
                theme,
                style,
                self.layout,
                cursor,
                &bounds,
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{
        Widget as _,
        widget::{button, column, container, text},
    };

    #[derive(Debug, Clone, PartialEq)]
    enum Message {
        Chosen(&'static str),
        Beneath,
    }

    /// A menu button at the top-left of a window filled by a button of its
    /// own, put through presses and releases at `taps`.
    fn run(taps: &[Point]) -> Vec<Message> {
        let menu = menu_button(
            container(text("More")).width(60).height(30),
            column![
                button(text("Open")).on_press(Message::Chosen("open")),
                button(text("Save")).on_press(Message::Chosen("save")),
            ],
        );
        let beneath = button(container(menu).width(Length::Fill).height(Length::Fill))
            .on_press(Message::Beneath);

        let mut simulator = iced_test::Simulator::new(beneath.boxed());
        for at in taps {
            simulator.point_at(*at);
            for event in [
                Event::Mouse(mouse::Event::CursorMoved { position: *at }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ] {
                simulator.simulate([event]);
            }
        }
        simulator.into_messages().collect()
    }

    const TRIGGER_AT: Point = Point::new(10.0, 10.0);

    #[test]
    fn an_entry_is_chosen_from_the_open_menu() {
        // The menu opens below the button: its first entry is just under it.
        let first_entry = Point::new(20.0, 30.0 + GAP + 10.0);
        assert_eq!(run(&[TRIGGER_AT, first_entry]), [Message::Chosen("open")]);
        // Closed, the same point is the button beneath.
        assert_eq!(run(&[first_entry]), [Message::Beneath]);
    }

    #[test]
    fn a_press_outside_closes_the_menu_and_goes_no_further() {
        let elsewhere = Point::new(500.0, 500.0);
        assert_eq!(run(&[TRIGGER_AT, elsewhere]), []);
        assert_eq!(run(&[TRIGGER_AT, elsewhere, elsewhere]), [Message::Beneath]);
    }
}
