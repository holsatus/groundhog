//! Content floating over the map, which keeps the presses that land on it.
//!
//! The floating shell stacks panels, the navigation bar and toasts over the
//! map. A `Stack` hides the pointer from lower layers only where an upper
//! one shows a mouse interaction, and the map follows a finger by the
//! finger's own position, not the cursor; a press on a panel's background
//! or text would reach the map beneath and pan it. A surface captures every
//! press (and wheel scroll) that lands on it, and shows at least the idle
//! pointer over itself, so the map sees neither.
//!
//! Only presses: a finger that went down on the map and slides over a
//! surface is still the map's, and its moves and its release must reach the
//! map, or the map would keep tracking a finger that has gone.

use iced::{
    Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Meta, Operation, Tree, tree},
    },
    touch,
};

/// `content`, keeping the presses that land on it from what lies beneath.
pub fn surface<W>(content: W) -> Surface<W> {
    Surface { content }
}

pub struct Surface<W> {
    content: W,
}

impl<W> Meta for Surface<W> {}

impl<W, Message> Widget<Message, Theme, Renderer> for Surface<W>
where
    W: Widget<Message, Theme, Renderer>,
{
    fn tag(&self) -> tree::Tag {
        self.content.tag()
    }

    fn state(&self) -> tree::State {
        self.content.state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.content.diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.content.layout(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let keep = match event {
            Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::WheelScrolled { .. }) => {
                cursor.is_over(bounds)
            }
            Event::Touch(touch::Event::FingerPressed { position, .. }) => {
                bounds.contains(*position)
            }
            _ => false,
        };

        self.content
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if keep {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let interaction = self
            .content
            .mouse_interaction(tree, layout, cursor, viewport, renderer);

        if interaction == mouse::Interaction::None && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Idle
        } else {
            interaction
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
        self.content
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{
        Point,
        widget::{
            canvas::{self, Action, Canvas, Geometry},
            container, space, stack, text,
        },
    };

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Phase {
        Pressed,
        Moved,
        Lifted,
    }

    /// Stands in for the map: it hears every finger, by the finger's own
    /// position, as the map does.
    struct Fingers;

    impl canvas::Program<Phase> for Fingers {
        type State = ();

        fn update(
            &self,
            _state: &mut (),
            event: &Event,
            _bounds: Rectangle,
            _cursor: mouse::Cursor,
        ) -> Option<Action<Phase>> {
            match event {
                Event::Touch(touch::Event::FingerPressed { .. }) => {
                    Some(Action::publish(Phase::Pressed).and_capture())
                }
                Event::Touch(touch::Event::FingerMoved { .. }) => {
                    Some(Action::publish(Phase::Moved))
                }
                Event::Touch(touch::Event::FingerLifted { .. }) => {
                    Some(Action::publish(Phase::Lifted))
                }
                _ => None,
            }
        }

        fn draw(
            &self,
            _state: &(),
            _renderer: &Renderer,
            _theme: &Theme,
            _bounds: Rectangle,
            _cursor: mouse::Cursor,
        ) -> Vec<Geometry> {
            Vec::new()
        }
    }

    /// Over the simulator's whole window, a 200 by 200 text on a surface at
    /// the top left.
    const ON: Point = Point::new(50.0, 50.0);
    const OFF: Point = Point::new(600.0, 400.0);

    fn finger(event: fn(touch::Finger, Point) -> touch::Event, at: Point) -> Event {
        Event::Touch(event(touch::Finger(0), at))
    }

    fn run(events: impl IntoIterator<Item = Event>) -> Vec<Phase> {
        let map = Canvas::new(Fingers)
            .width(Length::Fill)
            .height(Length::Fill);
        let panel = surface(container(text("Panel")).width(200).height(200));
        let chrome = container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(0);
        let root = stack![map, chrome, space()];

        let mut simulator = iced_test::Simulator::new(root);
        for event in events {
            if let Event::Touch(
                touch::Event::FingerPressed { position, .. }
                | touch::Event::FingerMoved { position, .. }
                | touch::Event::FingerLifted { position, .. },
            ) = event
            {
                simulator.point_at(position);
            }
            simulator.simulate([event]);
        }
        simulator.into_messages().collect()
    }

    fn pressed(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerPressed { id, position }
    }

    fn moved(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerMoved { id, position }
    }

    fn lifted(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerLifted { id, position }
    }

    #[test]
    fn a_press_on_the_surface_does_not_reach_the_map() {
        assert_eq!(run([finger(pressed, ON)]), []);
        assert_eq!(run([finger(pressed, OFF)]), [Phase::Pressed]);
    }

    #[test]
    fn a_finger_from_the_map_is_followed_over_the_surface() {
        assert_eq!(
            run([finger(pressed, OFF), finger(moved, ON), finger(lifted, ON)]),
            [Phase::Pressed, Phase::Moved, Phase::Lifted]
        );
    }
}
