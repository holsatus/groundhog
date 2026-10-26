//! Content that can be dragged out of its place, such as a rail button
//! dragged onto the pane grid.
//!
//! The source only reports that a drag started and that it ended; where it
//! ends is for the widget it is dropped on to decide (see
//! `PaneGrid::incoming`). While the drag lasts, a preview (a label for what
//! is dragged) follows the cursor in an overlay (see [`super::drag_preview`]). The content keeps working
//! as before when it is only clicked: a press becomes a drag once the
//! cursor moves [`DEADBAND`] away.

use super::drag_preview::{self, LIFT, TOUCH_LIFT};
use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Meta, Operation, Tree, tree},
    },
    keyboard, touch,
};

/// How far the cursor moves, pressed, before a press becomes a drag: as far
/// as a pane's title bar must move in the pane grid.
const DEADBAND: f32 = 10.0;

/// The same for a finger, which rolls and wobbles as it presses.
const TOUCH_DEADBAND: f32 = 16.0;

const CONTENT: usize = 0;
const PREVIEW: usize = 1;

/// A finger's phase, as the state machine needs it: the mouse's events,
/// except that a finger can be taken away by the system.
#[derive(Debug, Clone, Copy)]
enum TouchGesture {
    Press(Point),
    Move(Point),
    Release,
    Cancel,
}

/// `content` that publishes `on_drag` when it is dragged, and `on_end` when
/// the drag ends: on release anywhere, or on Escape. While dragged,
/// `preview` follows the cursor, centred [`LIFT`] above it (or
/// [`TOUCH_LIFT`] above a finger), so it hides neither the pointer nor what
/// is under it, and no finger hides it.
pub fn drag_source<'a, M: Clone + 'a>(
    content: impl Widget<M, Theme, Renderer> + 'a,
    preview: impl Widget<M, Theme, Renderer> + 'a,
    on_drag: M,
    on_end: M,
) -> DragSource<'a, M> {
    use iced::Widget as _;

    DragSource {
        children: vec![content.boxed(), preview.boxed()],
        on_drag,
        on_end,
    }
}

pub struct DragSource<'a, Message> {
    /// `[content, preview]`.
    children: Vec<Element<'a, Message>>,
    on_drag: Message,
    on_end: Message,
}

/// Where the left button went down on the content, and what became of it,
/// while it is held.
#[derive(Default)]
enum State {
    #[default]
    Idle,
    Pressed(Point),
    /// The preview is drawn `lift` above the pointer.
    Dragging {
        lift: f32,
    },
    /// Escape ended the drag; the button is still down.
    Cancelled,
}

impl<Message> Meta for DragSource<'_, Message> {}

impl<Message: Clone> Widget<Message, Theme, Renderer> for DragSource<'_, Message> {
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
        self.children[CONTENT].size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.children[CONTENT].layout(&mut tree.children[CONTENT], renderer, limits);
        tree.size = tree.children[CONTENT].size;
        // The preview's size, as it would be alone; it is placed when drawn.
        self.children[PREVIEW].layout(
            &mut tree.children[PREVIEW],
            renderer,
            &layout::Limits::new(Size::ZERO, Size::INFINITE),
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.children[CONTENT].operate(
            &mut tree.children[CONTENT],
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();

        // A finger drags like the mouse, and the content sees the finger's
        // own position: a phone reports a stale mouse cursor while a finger
        // is down.
        let (gesture, content_cursor) = match event {
            Event::Touch(touch) => match *touch {
                touch::Event::FingerPressed { position, .. } => (
                    Some(TouchGesture::Press(position)),
                    mouse::Cursor::Available(position),
                ),
                touch::Event::FingerMoved { position, .. } => (
                    Some(TouchGesture::Move(position)),
                    mouse::Cursor::Available(position),
                ),
                touch::Event::FingerLifted { position, .. } => (
                    Some(TouchGesture::Release),
                    mouse::Cursor::Available(position),
                ),
                touch::Event::FingerLost { position, .. } => (
                    Some(TouchGesture::Cancel),
                    mouse::Cursor::Available(position),
                ),
            },
            _ => (None, cursor),
        };

        // While dragging, the content does not see the cursor, so a release
        // back over it is not a click, and it does not show as hovered.
        let content_cursor = match state {
            State::Dragging { .. } | State::Cancelled => mouse::Cursor::Unavailable,
            State::Idle | State::Pressed(_) => content_cursor,
        };

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Noted before the content sees the press: a button
                // captures it.
                *state = cursor
                    .position_over(layout.bounds())
                    .map_or(State::Idle, State::Pressed);
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => match *state {
                State::Pressed(origin)
                    if cursor
                        .position()
                        .is_some_and(|position| position.distance(origin) > DEADBAND) =>
                {
                    *state = State::Dragging { lift: LIFT };
                    shell.publish(self.on_drag.clone());
                    shell.request_redraw();
                }
                State::Dragging { .. } => shell.request_redraw(),
                _ => {}
            },
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let State::Dragging { .. } = state {
                    shell.publish(self.on_end.clone());
                }
                *state = State::Idle;
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) if matches!(state, State::Dragging { .. }) => {
                // The release to come will be neither a drop nor a click.
                *state = State::Cancelled;
                shell.publish(self.on_end.clone());
                shell.capture_event();
                shell.request_redraw();
            }
            _ => {}
        }

        if let Some(gesture) = gesture {
            match gesture {
                TouchGesture::Press(position) => {
                    *state = if layout.bounds().contains(position) {
                        State::Pressed(position)
                    } else {
                        State::Idle
                    };
                }
                TouchGesture::Move(position) => match *state {
                    State::Pressed(origin) if position.distance(origin) > TOUCH_DEADBAND => {
                        *state = State::Dragging { lift: TOUCH_LIFT };
                        shell.publish(self.on_drag.clone());
                        shell.request_redraw();
                    }
                    State::Dragging { .. } => shell.request_redraw(),
                    _ => {}
                },
                TouchGesture::Release => {
                    if let State::Dragging { .. } = state {
                        shell.publish(self.on_end.clone());
                    }
                    *state = State::Idle;
                }
                TouchGesture::Cancel => {
                    // The system took the gesture; a drag must not stay stuck.
                    if let State::Dragging { .. } = state {
                        shell.publish(self.on_end.clone());
                    }
                    *state = State::Cancelled;
                }
            }
        }

        self.children[CONTENT].update(
            &mut tree.children[CONTENT],
            event,
            layout,
            content_cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if let State::Dragging { .. } = tree.state.downcast_ref::<State>() {
            return mouse::Interaction::Grabbing;
        }
        self.children[CONTENT].mouse_interaction(
            &tree.children[CONTENT],
            layout,
            cursor,
            viewport,
            renderer,
        )
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
        self.children[CONTENT].draw(
            &tree.children[CONTENT],
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
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        match *tree.state.downcast_ref::<State>() {
            State::Dragging { lift } => vec![overlay::Element::new(Box::new(Preview {
                preview: &self.children[PREVIEW],
                tree: &tree.children[PREVIEW],
                lift,
                window,
            }))],
            // Not while dragging: a tooltip would follow the press.
            _ => self.children[CONTENT].overlay(
                &mut tree.children[CONTENT],
                layout,
                renderer,
                viewport,
                translation,
                window,
            ),
        }
    }
}

/// The preview, drawn `lift` above the cursor and kept inside the
/// `window`. It reports no mouse interaction: an overlay that does hides the
/// cursor from the widgets beneath, and the one under the cursor decides the
/// drop. (The drag source itself shows the grabbing pointer.)
struct Preview<'a, 'b, Message> {
    preview: &'a Element<'b, Message>,
    tree: &'a Tree,
    lift: f32,
    window: Size,
}

impl<Message> overlay::Overlay<Message, Theme, Renderer> for Preview<'_, '_, Message> {
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        cursor: mouse::Cursor,
    ) {
        if let Some(pointer) = cursor.position() {
            drag_preview::draw(
                self.preview,
                self.tree,
                renderer,
                theme,
                style,
                pointer,
                self.lift,
                self.window,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::Named;

    #[derive(Debug, Clone, PartialEq)]
    enum Message {
        Clicked,
        Dragged,
        Ended,
    }

    const ON: Point = Point::new(5.0, 5.0);
    const AWAY: Point = Point::new(300.0, 200.0);

    enum Step {
        Move(Point),
        Press,
        Release,
        Escape,
        TouchPress(Point),
        TouchMove(Point),
        TouchRelease(Point),
    }

    /// A button in a drag source at the window's top-left, put through
    /// `steps`; the messages it published.
    fn run(steps: impl IntoIterator<Item = Step>) -> Vec<Message> {
        let button = iced::widget::button(iced::widget::text("Logs")).on_press(Message::Clicked);
        let preview = iced::widget::text("Logs");
        let mut simulator = iced_test::Simulator::new(drag_source(
            button,
            preview,
            Message::Dragged,
            Message::Ended,
        ));

        for step in steps {
            let event = match step {
                Step::Move(position) => {
                    simulator.point_at(position);
                    Event::Mouse(mouse::Event::CursorMoved { position })
                }
                Step::Press => Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Step::Release => Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                Step::Escape => iced_test::simulator::press_key(Named::Escape, None),
                Step::TouchPress(position) => Event::Touch(touch::Event::FingerPressed {
                    id: touch::Finger(0),
                    position,
                }),
                Step::TouchMove(position) => Event::Touch(touch::Event::FingerMoved {
                    id: touch::Finger(0),
                    position,
                }),
                Step::TouchRelease(position) => Event::Touch(touch::Event::FingerLifted {
                    id: touch::Finger(0),
                    position,
                }),
            };
            simulator.simulate([event]);
        }
        simulator.into_messages().collect()
    }

    #[test]
    fn a_click_stays_a_click() {
        use Step::*;
        assert_eq!(run([Move(ON), Press, Release]), [Message::Clicked]);
        // A jitter within the deadband is still a click.
        let near = Point::new(ON.x + 3.0, ON.y + 2.0);
        assert_eq!(
            run([Move(ON), Press, Move(near), Release]),
            [Message::Clicked]
        );
    }

    #[test]
    fn a_drag_reports_its_start_and_end_and_is_no_click() {
        use Step::*;
        assert_eq!(
            run([Move(ON), Press, Move(AWAY), Release]),
            [Message::Dragged, Message::Ended]
        );
        // Not even when dropped back on the button.
        assert_eq!(
            run([Move(ON), Press, Move(AWAY), Move(ON), Release]),
            [Message::Dragged, Message::Ended]
        );
    }

    #[test]
    fn escape_ends_a_drag_and_the_release_does_nothing() {
        use Step::*;
        assert_eq!(
            run([Move(ON), Press, Move(AWAY), Escape, Move(ON), Release]),
            [Message::Dragged, Message::Ended]
        );
    }

    #[test]
    fn a_press_elsewhere_starts_nothing() {
        use Step::*;
        assert!(run([Move(AWAY), Press, Move(ON), Release]).is_empty());
    }

    #[test]
    fn a_finger_drags_and_a_tap_stays_a_click() {
        use Step::*;

        // A finger wobbles more than a mouse before it is a drag.
        let jitter = Point::new(ON.x + 8.0, ON.y + 4.0);
        assert_eq!(
            run([TouchPress(ON), TouchMove(jitter), TouchRelease(jitter)]),
            [Message::Clicked]
        );

        assert_eq!(
            run([TouchPress(ON), TouchMove(AWAY), TouchRelease(AWAY)]),
            [Message::Dragged, Message::Ended]
        );
    }
}
