//! The label that follows the pointer while something is dragged. A rail
//! button (`drag_source`) and a pane in the pane grid draw the same one, in
//! the same place, so both drags look alike.

use iced::{
    Point, Rectangle, Size,
    advanced::{Layout, Renderer, Widget, mouse, renderer, widget::Tree},
};

/// The gap between the cursor and the preview above it.
pub const LIFT: f32 = 0.0;

/// The same under a finger, which covers what is right above the point it
/// touches: about a touch target's height.
pub const TOUCH_LIFT: f32 = 44.0;

/// Draws `preview`, laid out alone in `tree`, centred `lift` above
/// `pointer` and kept inside the `window`, in a layer of its own so it is
/// drawn over whatever the drag highlights.
#[expect(
    clippy::too_many_arguments,
    reason = "a widget's draw arguments, plus where to put it"
)]
pub fn draw<Message, Theme, R: Renderer>(
    preview: &impl Widget<Message, Theme, R>,
    tree: &Tree,
    renderer: &mut R,
    theme: &Theme,
    style: &renderer::Style,
    pointer: Point,
    lift: f32,
    window: Size,
) {
    let size = tree.size;
    let layout = Layout::new(size).move_to(placement(pointer, size, lift, window));

    renderer.with_layer(layout.bounds(), |renderer| {
        preview.draw(
            tree,
            renderer,
            theme,
            style,
            layout,
            mouse::Cursor::Unavailable,
            &Rectangle::INFINITE,
        );
    });
}

/// Where a preview of `size` goes: centred `lift` above `pointer`, moved
/// back inside `window` where that would leave it.
fn placement(pointer: Point, size: Size, lift: f32, window: Size) -> Point {
    let x = pointer.x - size.width / 2.0;
    let y = pointer.y - size.height - lift;

    Point::new(
        x.min(window.width - size.width).max(0.0),
        y.min(window.height - size.height).max(0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_clears_the_finger_and_stays_in_the_window() {
        let window = Size::new(400.0, 800.0);
        let size = Size::new(100.0, 30.0);
        let finger = Point::new(200.0, 400.0);

        let at = placement(finger, size, TOUCH_LIFT, window);
        assert_eq!(at.x + size.width / 2.0, finger.x, "centred on the finger");
        assert_eq!(finger.y - (at.y + size.height), TOUCH_LIFT);

        // Dragged from the rail at the window's top-left corner.
        let corner = placement(Point::new(10.0, 10.0), size, TOUCH_LIFT, window);
        assert_eq!(corner, Point::ORIGIN);
        let edge = placement(Point::new(395.0, 400.0), size, TOUCH_LIFT, window);
        assert_eq!(edge.x, window.width - size.width);
    }
}
