//! Tracking on the picture: the gestures that ask its camera to track a
//! point or a rectangle, and what the camera tracks, drawn over the
//! picture (see `groundhog_logic::domain::vehicle::camera`).
//!
//! With a mouse, releasing the right button tracks the point under the
//! cursor, and dragging with the left button tracks the rectangle dragged
//! out. With a finger, a tap tracks a point and a drag a rectangle.
//! Escape, the window losing focus, or the system taking the finger
//! abandons a gesture. A gesture starts only on the picture, not on the
//! bands around it, and a drag keeps to the picture; positions are the
//! picture's own ([`ImagePoint`]), whatever size it shows at. The overlay
//! captures only the gestures the camera can act on, so what floats over
//! the picture keeps its presses, and a camera that tracks only points
//! leaves a left drag alone.
//!
//! The marks have fixed colours, which read over any picture, as the map's
//! overlays do over any tile.

use groundhog_logic::domain::vehicle::camera::{ImagePoint, ImageRect, Target, TrackingSupport};
use iced::{
    Color, Point, Rectangle, Renderer, Size, Theme, keyboard, mouse, touch,
    widget::canvas::{self, Action, Event, Frame, Geometry, LineDash, Path, Stroke, Style},
    window,
};

use crate::widget::video::picture_bounds;

/// How far a press must move, on both axes, to drag out a rectangle rather
/// than click or tap.
const DRAG: f32 = 8.0;

/// How large a point is asked to be tracked, as a share of the picture's
/// width (MAVLink's radius), and drawn when the camera does not say.
pub const POINT_RADIUS: f32 = 0.05;

/// What a tracked target is drawn in.
const TRACKED: Color = Color::from_rgb(0.24, 0.86, 0.52);
/// What a rectangle being dragged out is drawn in.
const SELECTING: Color = Color::WHITE;
/// Under every mark, so it reads over a light picture too.
const CASING: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.6);

/// What a gesture asks the camera to track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Request {
    Point(ImagePoint),
    Rectangle(ImageRect),
}

impl Request {
    /// As a target, to draw until the camera answers.
    pub fn target(self) -> Target {
        match self {
            Request::Point(at) => Target::Point {
                at,
                radius: Some(POINT_RADIUS),
            },
            Request::Rectangle(rect) => Target::Rectangle(rect),
        }
    }
}

/// How a target is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shade {
    /// The camera reports tracking it.
    Reported,
    /// The camera was asked to, and has yet to answer: dashed.
    Asked,
}

/// What lies over a picture for tracking. `on_request` makes the message a
/// gesture sends, if it sends one.
pub struct Overlay<F> {
    /// The frame's size in pixels; there is nothing to track or draw on
    /// before a frame.
    pub frame: Option<(u32, u32)>,
    /// What the camera can track: the gestures for anything else are left
    /// alone.
    pub support: TrackingSupport,
    pub target: Option<(Target, Shade)>,
    pub on_request: F,
}

/// The gesture in progress, if any.
#[derive(Debug, Default)]
pub struct Gesture {
    press: Option<Press>,
}

#[derive(Debug, Clone, Copy)]
struct Press {
    by: By,
    /// Where it started and where it is, in the window.
    from: Point,
    to: Point,
    /// Whether it moved far enough to be a drag.
    dragged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Left,
    Right,
    Finger(touch::Finger),
}

impl<M, F: Fn(Request) -> Option<M>> Overlay<F> {
    /// Where the picture is in the window, if there is one to track on.
    fn picture(&self, bounds: Rectangle) -> Option<Rectangle> {
        let (width, height) = self.frame?;
        let picture = picture_bounds(width, height, bounds.size());
        Some(Rectangle::new(
            Point::new(bounds.x + picture.x, bounds.y + picture.y),
            picture.size(),
        ))
    }

    fn start(&self, gesture: &mut Gesture, by: By, at: Point) -> Option<Action<M>> {
        gesture.press = Some(Press {
            by,
            from: at,
            to: at,
            dragged: false,
        });
        Some(Action::request_redraw().and_capture())
    }

    fn drag(
        &self,
        gesture: &mut Gesture,
        by: impl Fn(By) -> bool,
        to: Point,
        picture: Rectangle,
    ) -> Option<Action<M>> {
        let press = gesture.press.as_mut().filter(|press| by(press.by))?;
        press.to = clamp(to, picture);
        press.dragged |= is_drag(press.from, press.to);
        Some(Action::request_redraw().and_capture())
    }

    fn end(
        &self,
        gesture: &mut Gesture,
        by: By,
        at: Option<Point>,
        picture: Rectangle,
    ) -> Option<Action<M>> {
        let press = gesture.press.take_if(|press| press.by == by)?;
        let to = at.map_or(press.to, |at| clamp(at, picture));
        let point = || {
            // A point is where the gesture ends, on the picture.
            at.filter(|at| picture.contains(*at))
                .filter(|_| self.support.point)
                .and_then(|at| normalized(at, picture))
                .map(Request::Point)
        };
        let rectangle = || {
            if !self.support.rectangle || !is_rectangle(press.from, to) {
                return None;
            }
            let from = normalized(press.from, picture)?;
            let to = normalized(to, picture)?;
            Some(Request::Rectangle(ImageRect::spanning(from, to)))
        };

        let request = match press.by {
            By::Right => point(),
            By::Left => rectangle(),
            By::Finger(_) if press.dragged => rectangle(),
            By::Finger(_) => point(),
        };
        Some(match request.and_then(&self.on_request) {
            Some(message) => Action::publish(message).and_capture(),
            None => Action::request_redraw().and_capture(),
        })
    }
}

impl<M, F: Fn(Request) -> Option<M>> canvas::Program<M> for Overlay<F> {
    type State = Gesture;

    fn update(
        &self,
        gesture: &mut Gesture,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<M>> {
        if !self.support.any() {
            return None;
        }
        let picture = self.picture(bounds)?;
        let pressing = gesture.press.is_some();

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(button)) if !pressing => {
                let by = match button {
                    mouse::Button::Left if self.support.rectangle => By::Left,
                    mouse::Button::Right if self.support.point => By::Right,
                    _ => return None,
                };
                let at = cursor.position().filter(|at| picture.contains(*at))?;
                self.start(gesture, by, at)
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => self.drag(
                gesture,
                |by| matches!(by, By::Left | By::Right),
                *position,
                picture,
            ),
            Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                let by = match button {
                    mouse::Button::Left => By::Left,
                    mouse::Button::Right => By::Right,
                    _ => return None,
                };
                self.end(gesture, by, cursor.position(), picture)
            }

            Event::Touch(touch::Event::FingerPressed { id, position }) => {
                if pressing || !picture.contains(*position) {
                    return None;
                }
                self.start(gesture, By::Finger(*id), *position)
            }
            Event::Touch(touch::Event::FingerMoved { id, position }) => {
                self.drag(gesture, |by| by == By::Finger(*id), *position, picture)
            }
            Event::Touch(touch::Event::FingerLifted { id, position }) => {
                self.end(gesture, By::Finger(*id), Some(*position), picture)
            }
            Event::Touch(touch::Event::FingerLost { id, .. }) => gesture
                .press
                .take_if(|press| press.by == By::Finger(*id))
                .map(|_| Action::request_redraw().and_capture()),

            // Escape abandons a gesture, and only then is it the picture's;
            // a window that lost focus may never see the release.
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => gesture
                .press
                .take()
                .map(|_| Action::request_redraw().and_capture()),
            Event::Window(window::Event::Unfocused) => {
                gesture.press.take().map(|_| Action::request_redraw())
            }

            _ => None,
        }
    }

    fn draw(
        &self,
        gesture: &Gesture,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let Some((width, height)) = self.frame else {
            return Vec::new();
        };
        let picture = picture_bounds(width, height, bounds.size());
        let mut frame = Frame::new(renderer, bounds.size());

        if let Some((target, shade)) = self.target {
            draw_target(&mut frame, picture, target, shade);
        }

        // The rectangle being dragged out.
        if let Some(press) = gesture
            .press
            .filter(|press| press.dragged && press.by != By::Right)
        {
            let origin = bounds.position();
            let from = press.from - (origin - Point::ORIGIN);
            let to = press.to - (origin - Point::ORIGIN);
            let rect = Path::rectangle(
                Point::new(from.x.min(to.x), from.y.min(to.y)),
                Size::new((to.x - from.x).abs(), (to.y - from.y).abs()),
            );
            frame.fill(&rect, SELECTING.scale_alpha(0.15));
            mark(&mut frame, &rect, SELECTING, Shade::Reported);
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        gesture: &Gesture,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let over_picture = self
            .picture(bounds)
            .zip(cursor.position())
            .is_some_and(|(picture, at)| picture.contains(at));
        if gesture.press.is_some() || (self.support.any() && over_picture) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::None
        }
    }
}

/// `target` over `picture` (relative to the overlay).
fn draw_target(frame: &mut Frame, picture: Rectangle, target: Target, shade: Shade) {
    let at = |point: ImagePoint| {
        Point::new(
            picture.x + point.x * picture.width,
            picture.y + point.y * picture.height,
        )
    };
    match target {
        Target::Point { at: point, radius } => {
            let centre = at(point);
            let radius = (radius.unwrap_or(POINT_RADIUS) * picture.width).max(DRAG);
            mark(frame, &Path::circle(centre, radius), TRACKED, shade);
            frame.fill(&Path::circle(centre, 2.5), TRACKED);
        }
        Target::Rectangle(rect) => {
            let top_left = at(rect.top_left);
            let bottom_right = at(rect.bottom_right);
            let rect = Path::rectangle(
                top_left,
                Size::new(bottom_right.x - top_left.x, bottom_right.y - top_left.y),
            );
            mark(frame, &rect, TRACKED, shade);
        }
    }
}

/// Stroke `path` in `colour` over a dark casing; dashed while only asked.
fn mark(frame: &mut Frame, path: &Path, colour: Color, shade: Shade) {
    let line_dash = match shade {
        Shade::Reported => LineDash::default(),
        Shade::Asked => LineDash {
            segments: &[6.0, 4.0],
            offset: 0,
        },
    };
    frame.stroke(
        path,
        Stroke {
            style: Style::Solid(CASING),
            width: 4.0,
            line_dash,
            ..Stroke::default()
        },
    );
    frame.stroke(
        path,
        Stroke {
            style: Style::Solid(colour),
            width: 2.0,
            line_dash,
            ..Stroke::default()
        },
    );
}

fn clamp(at: Point, picture: Rectangle) -> Point {
    Point::new(
        at.x.clamp(picture.x, picture.x + picture.width),
        at.y.clamp(picture.y, picture.y + picture.height),
    )
}

fn is_drag(from: Point, to: Point) -> bool {
    (to.x - from.x).abs() >= DRAG || (to.y - from.y).abs() >= DRAG
}

/// Whether a drag from `from` to `to` spans a rectangle, not a line.
fn is_rectangle(from: Point, to: Point) -> bool {
    (to.x - from.x).abs() >= DRAG && (to.y - from.y).abs() >= DRAG
}

/// `at` (in the window) as a position in `picture`.
fn normalized(at: Point, picture: Rectangle) -> Option<ImagePoint> {
    if picture.width <= 0.0 || picture.height <= 0.0 {
        return None;
    }
    ImagePoint::new(
        ((at.x - picture.x) / picture.width).clamp(0.0, 1.0),
        ((at.y - picture.y) / picture.height).clamp(0.0, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use canvas::Program;
    use iced::event::Status;

    use super::*;

    /// A 400 by 400 widget at (100, 50), showing a 16:9 frame: the picture
    /// is 400 by 225, from (100, 137.5) to (500, 362.5).
    const BOUNDS: Rectangle = Rectangle {
        x: 100.0,
        y: 50.0,
        width: 400.0,
        height: 400.0,
    };

    type Requests = fn(Request) -> Option<Request>;

    fn overlay(support: TrackingSupport) -> Overlay<Requests> {
        Overlay {
            frame: Some((1600, 900)),
            support,
            target: None,
            on_request: Some,
        }
    }

    const BOTH: TrackingSupport = TrackingSupport {
        point: true,
        rectangle: true,
    };

    fn at(x: f32, y: f32) -> mouse::Cursor {
        mouse::Cursor::Available(Point::new(x, y))
    }

    fn pressed(button: mouse::Button) -> Event {
        Event::Mouse(mouse::Event::ButtonPressed(button))
    }

    fn released(button: mouse::Button) -> Event {
        Event::Mouse(mouse::Event::ButtonReleased(button))
    }

    fn moved(x: f32, y: f32) -> Event {
        Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(x, y),
        })
    }

    fn finger(event: fn(touch::Finger, Point) -> touch::Event, x: f32, y: f32) -> Event {
        Event::Touch(event(touch::Finger(1), Point::new(x, y)))
    }

    fn finger_pressed(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerPressed { id, position }
    }

    fn finger_moved(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerMoved { id, position }
    }

    fn finger_lifted(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerLifted { id, position }
    }

    fn finger_lost(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerLost { id, position }
    }

    /// What `event` sends, and whether it was captured.
    fn feed(
        overlay: &Overlay<Requests>,
        gesture: &mut Gesture,
        event: &Event,
        cursor: mouse::Cursor,
    ) -> (Option<Request>, bool) {
        match overlay.update(gesture, event, BOUNDS, cursor) {
            Some(action) => {
                let (message, _, status) = action.into_inner();
                (message, status == Status::Captured)
            }
            None => (None, false),
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn is_point(request: Option<Request>, x: f32, y: f32) -> bool {
        matches!(request, Some(Request::Point(at)) if close(at.x, x) && close(at.y, y))
    }

    fn is_rectangle(request: Option<Request>, from: (f32, f32), to: (f32, f32)) -> bool {
        matches!(request, Some(Request::Rectangle(rect))
            if close(rect.top_left.x, from.0) && close(rect.top_left.y, from.1)
                && close(rect.bottom_right.x, to.0) && close(rect.bottom_right.y, to.1))
    }

    #[test]
    fn a_left_drag_tracks_the_rectangle_in_the_pictures_own_terms() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();

        let start = at(400.0, 300.0);
        let (sent, captured) = feed(&overlay, &mut gesture, &pressed(mouse::Button::Left), start);
        assert_eq!((sent, captured), (None, true));
        feed(
            &overlay,
            &mut gesture,
            &moved(200.0, 160.0),
            at(200.0, 160.0),
        );
        let (sent, captured) = feed(
            &overlay,
            &mut gesture,
            &released(mouse::Button::Left),
            at(200.0, 160.0),
        );

        // Dragged up and left: the corners come out in order.
        assert!(
            is_rectangle(sent, (0.25, 0.1), (0.75, 0.7222222)),
            "{sent:?}"
        );
        assert!(captured);
    }

    #[test]
    fn a_left_click_tracks_nothing() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();

        feed(
            &overlay,
            &mut gesture,
            &pressed(mouse::Button::Left),
            at(300.0, 250.0),
        );
        feed(
            &overlay,
            &mut gesture,
            &moved(303.0, 252.0),
            at(303.0, 252.0),
        );
        let (sent, _) = feed(
            &overlay,
            &mut gesture,
            &released(mouse::Button::Left),
            at(303.0, 252.0),
        );
        assert_eq!(sent, None);
    }

    #[test]
    fn a_drag_keeps_to_the_picture() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();

        feed(
            &overlay,
            &mut gesture,
            &pressed(mouse::Button::Left),
            at(300.0, 250.0),
        );
        feed(
            &overlay,
            &mut gesture,
            &moved(600.0, 440.0),
            at(600.0, 440.0),
        );
        let (sent, _) = feed(
            &overlay,
            &mut gesture,
            &released(mouse::Button::Left),
            at(600.0, 440.0),
        );
        assert!(is_rectangle(sent, (0.5, 0.5), (1.0, 1.0)), "{sent:?}");
    }

    #[test]
    fn releasing_the_right_button_tracks_a_point() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();

        feed(
            &overlay,
            &mut gesture,
            &pressed(mouse::Button::Right),
            at(300.0, 250.0),
        );
        let (sent, captured) = feed(
            &overlay,
            &mut gesture,
            &released(mouse::Button::Right),
            at(200.0, 137.5),
        );
        assert!(is_point(sent, 0.25, 0.0), "where it was released: {sent:?}");
        assert!(captured);
    }

    #[test]
    fn a_right_release_off_the_picture_tracks_nothing() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();

        feed(
            &overlay,
            &mut gesture,
            &pressed(mouse::Button::Right),
            at(300.0, 250.0),
        );
        let (sent, _) = feed(
            &overlay,
            &mut gesture,
            &released(mouse::Button::Right),
            at(300.0, 100.0),
        );
        assert_eq!(sent, None);
    }

    #[test]
    fn the_bands_around_the_picture_take_nothing() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();

        let band = at(300.0, 100.0);
        assert_eq!(
            feed(&overlay, &mut gesture, &pressed(mouse::Button::Left), band),
            (None, false)
        );
        assert!(gesture.press.is_none());
    }

    #[test]
    fn only_what_the_camera_can_track_is_taken() {
        let points = overlay(TrackingSupport {
            point: true,
            rectangle: false,
        });
        let mut gesture = Gesture::default();
        let inside = at(300.0, 250.0);
        assert_eq!(
            feed(&points, &mut gesture, &pressed(mouse::Button::Left), inside),
            (None, false),
            "a camera without rectangles leaves the left button alone"
        );

        let nothing = overlay(TrackingSupport::default());
        assert_eq!(
            feed(
                &nothing,
                &mut gesture,
                &pressed(mouse::Button::Right),
                inside
            ),
            (None, false)
        );

        let no_frame = Overlay {
            frame: None,
            ..overlay(BOTH)
        };
        assert_eq!(
            feed(
                &no_frame,
                &mut gesture,
                &pressed(mouse::Button::Right),
                inside
            ),
            (None, false),
            "nothing to track on before a frame"
        );
    }

    #[test]
    fn a_tap_tracks_a_point_and_a_finger_drag_a_rectangle() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();
        let anywhere = mouse::Cursor::Unavailable;

        feed(
            &overlay,
            &mut gesture,
            &finger(finger_pressed, 300.0, 250.0),
            anywhere,
        );
        let (sent, _) = feed(
            &overlay,
            &mut gesture,
            &finger(finger_lifted, 301.0, 250.0),
            anywhere,
        );
        assert!(is_point(sent, 0.5025, 0.5), "{sent:?}");

        feed(
            &overlay,
            &mut gesture,
            &finger(finger_pressed, 200.0, 160.0),
            anywhere,
        );
        feed(
            &overlay,
            &mut gesture,
            &finger(finger_moved, 400.0, 300.0),
            anywhere,
        );
        let (sent, _) = feed(
            &overlay,
            &mut gesture,
            &finger(finger_lifted, 400.0, 300.0),
            anywhere,
        );
        assert!(
            is_rectangle(sent, (0.25, 0.1), (0.75, 0.7222222)),
            "{sent:?}"
        );
    }

    #[test]
    fn a_lost_finger_tracks_nothing() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();
        let anywhere = mouse::Cursor::Unavailable;

        feed(
            &overlay,
            &mut gesture,
            &finger(finger_pressed, 200.0, 160.0),
            anywhere,
        );
        feed(
            &overlay,
            &mut gesture,
            &finger(finger_moved, 400.0, 300.0),
            anywhere,
        );
        feed(
            &overlay,
            &mut gesture,
            &finger(finger_lost, 400.0, 300.0),
            anywhere,
        );
        let (sent, captured) = feed(
            &overlay,
            &mut gesture,
            &finger(finger_lifted, 400.0, 300.0),
            anywhere,
        );
        assert_eq!((sent, captured), (None, false));
    }

    #[test]
    fn escape_abandons_a_drag_and_is_otherwise_left_alone() {
        let overlay = overlay(BOTH);
        let mut gesture = Gesture::default();
        let escape = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            modified_key: keyboard::Key::Named(keyboard::key::Named::Escape),
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::default(),
            text: None,
            repeat: false,
        });
        let end = at(400.0, 300.0);

        assert_eq!(
            feed(&overlay, &mut gesture, &escape, end),
            (None, false),
            "Escape leaves the full screen when nothing is dragged"
        );

        feed(
            &overlay,
            &mut gesture,
            &pressed(mouse::Button::Left),
            at(200.0, 160.0),
        );
        feed(&overlay, &mut gesture, &moved(400.0, 300.0), end);
        assert_eq!(feed(&overlay, &mut gesture, &escape, end), (None, true));
        let (sent, _) = feed(&overlay, &mut gesture, &released(mouse::Button::Left), end);
        assert_eq!(sent, None);
    }
}
