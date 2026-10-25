//! The overlay canvas: draws every layer and runs the pointer state machine.
//!
//! Pointer state lives in the canvas's own widget state, not in application
//! state. A drag therefore cannot outlive its release, cannot be "latched" by
//! a lost event, and needs no plumbing through the application: layers just
//! receive [`LayerEvent`]s.
//!
//! Gestures:
//! - press on a draggable handle captures the pointer (the map does not pan);
//!   moving past [`DRAG_THRESHOLD`] starts a drag, releasing ends it, and
//!   escape cancels it;
//! - press anywhere else is left to the map (it pans), and becomes a click if
//!   released without moving past the threshold;
//! - a click on a handle goes to the handle's layer; a click on empty map is
//!   offered to layers from the top down until one answers.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
};

use groundhog_logic::domain::geo::GeoPosition;
use iced::{
    Padding, Point, Rectangle, Renderer, Theme, keyboard, mouse, touch,
    widget::canvas::{self, Action, Event, Frame, Geometry},
    window,
};
use slippery::Viewpoint;

use super::{
    layer::{Anchor, Handle, HandleId, Interaction, LayerEvent, LayerId, MapLayer},
    view::{MapFrame, MapView},
};

/// Pointer travel, in pixels, that turns a mouse press into a drag (or a pan).
pub const DRAG_THRESHOLD: f32 = 4.0;

/// The same for a finger: it rolls as it presses, so the slop is larger.
pub const TOUCH_DRAG_THRESHOLD: f32 = 10.0;

/// How much of a handle's surroundings a finger hits: a touch target should
/// be about 44 logical pixels across.
pub const TOUCH_TARGET_RADIUS: f32 = 22.0;

/// Where a gesture came from, which decides its slop and hit area.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    Mouse,
    Touch,
}

impl Source {
    /// How far the pointer may travel before the press becomes a drag.
    fn threshold(self) -> f32 {
        match self {
            Source::Mouse => DRAG_THRESHOLD,
            Source::Touch => TOUCH_DRAG_THRESHOLD,
        }
    }

    /// How much bigger a handle's hit area is under this pointer.
    fn hit_radius(self) -> f32 {
        match self {
            Source::Mouse => 0.0,
            Source::Touch => TOUCH_TARGET_RADIUS,
        }
    }
}

/// The canvas program hosting a frame's layers.
pub(super) struct Overlay<'a, Message> {
    layers: Vec<Box<dyn MapLayer<Message> + 'a>>,
    viewpoint: Viewpoint,
    /// How much of each edge floats under something else.
    covered: Padding,
}

impl<'a, Message> Overlay<'a, Message> {
    pub fn new(
        layers: Vec<Box<dyn MapLayer<Message> + 'a>>,
        viewpoint: Viewpoint,
        covered: Padding,
    ) -> Self {
        Self {
            layers,
            viewpoint,
            covered,
        }
    }

    fn view(&self, bounds: Rectangle) -> MapView {
        MapView::new(self.viewpoint, bounds.size(), self.covered)
    }

    fn layer(&self, id: LayerId) -> Option<&(dyn MapLayer<Message> + 'a)> {
        self.layers
            .iter()
            .find(|layer| layer.id() == id)
            .map(|layer| layer.as_ref())
    }

    /// The topmost handle under `point`, with every handle's hit area at
    /// least `min_radius`.
    fn hit(&self, view: &MapView, point: Point, min_radius: f32) -> Option<Target> {
        for layer in self.layers.iter().rev() {
            let closest = layer
                .handles(view)
                .into_iter()
                .filter_map(|handle| {
                    let center = anchor_point(view, &handle);
                    let distance = center.distance(point);
                    (distance <= handle.radius.max(min_radius)).then_some((handle, distance))
                })
                .min_by(|(_, a), (_, b)| a.total_cmp(b));

            if let Some((handle, _)) = closest {
                return Some(Target {
                    layer: layer.id(),
                    handle: handle.id,
                    draggable: handle.draggable,
                    origin: view.to_geo(anchor_point(view, &handle)),
                });
            }
        }

        None
    }

    /// Offer a click on empty map to layers from the top down.
    fn click_empty(&self, at: GeoPosition) -> Option<Message> {
        let event = LayerEvent::Click { handle: None, at };
        self.layers
            .iter()
            .rev()
            .find_map(|layer| layer.on_event(&event))
    }

    fn send(&self, layer: LayerId, event: LayerEvent) -> Option<Message> {
        self.layer(layer)?.on_event(&event)
    }
}

fn anchor_point(view: &MapView, handle: &Handle) -> Point {
    match handle.anchor {
        Anchor::Geo(position) => view.to_screen(position),
        Anchor::Screen(point) => point,
    }
}

/// A handle the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Target {
    layer: LayerId,
    handle: HandleId,
    draggable: bool,
    /// Where the handle was when the gesture began.
    origin: GeoPosition,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Pointer {
    #[default]
    Idle,
    /// The button is down and the pointer has not moved past the threshold.
    Pressed { at: Point, target: Option<Target> },
    /// A draggable handle is being dragged.
    Dragging { target: Target },
    /// The press became a map pan; its release is not a click.
    Panning,
}

/// One layer's cached geometry, and the key it was drawn under.
#[derive(Default)]
struct LayerCache {
    cache: canvas::Cache,
    key: Cell<Option<u64>>,
}

#[derive(Default)]
pub(super) struct State {
    pointer: Pointer,
    /// The finger driving `pointer`, while one is down.
    finger: Option<touch::Finger>,
    hovered: Option<(LayerId, HandleId)>,
    caches: RefCell<HashMap<LayerId, LayerCache>>,
}

impl State {
    fn interaction(&self, layer: LayerId) -> Interaction {
        Interaction {
            hovered: self
                .hovered
                .filter(|(hovered, _)| *hovered == layer)
                .map(|(_, handle)| handle),
            dragged: match self.pointer {
                Pointer::Dragging { target } if target.layer == layer => Some(target.handle),
                _ => None,
            },
        }
    }
}

/// A value identifying `theme`'s colours, for cache keys: layers draw in
/// colours from the theme, so cached geometry is stale once it changes.
fn theme_key(theme: &Theme) -> u64 {
    let seed = theme.seed();
    let mut hasher = DefaultHasher::new();
    for color in [
        seed.background,
        seed.text,
        seed.primary,
        seed.success,
        seed.warning,
        seed.danger,
    ] {
        color.into_rgba8().hash(&mut hasher);
    }
    hasher.finish()
}

impl<Message> canvas::Program<Message> for Overlay<'_, Message> {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        // A finger is the same gesture as the mouse, with more slop and a
        // bigger hit area; whatever the overlay does not capture, the map
        // below pans and zooms with.
        if let Event::Touch(finger) = event {
            return self.touch(state, *finger, bounds);
        }

        self.pointer(state, event, bounds, cursor, Source::Mouse)
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let view = self.view(bounds);
        let view_key = view.key();
        let theme_key = theme_key(theme);
        let mut caches = state.caches.borrow_mut();
        let mut drawn = HashSet::with_capacity(self.layers.len());

        let geometries = self
            .layers
            .iter()
            .map(|layer| {
                let id = layer.id();
                let interaction = state.interaction(id);
                drawn.insert(id);

                let draw = |frame: &mut Frame| {
                    layer.draw(&mut MapFrame::new(frame, &view), interaction);
                };

                match layer.revision() {
                    Some(revision) => {
                        let mut hasher = DefaultHasher::new();
                        (revision, view_key, theme_key, interaction).hash(&mut hasher);
                        let key = hasher.finish();

                        let entry = caches.entry(id).or_default();
                        if entry.key.get() != Some(key) {
                            entry.cache.clear();
                            entry.key.set(Some(key));
                        }
                        entry.cache.draw(renderer, bounds.size(), draw)
                    }
                    None => {
                        let mut frame = Frame::new(renderer, bounds.size());
                        draw(&mut frame);
                        frame.into_geometry()
                    }
                }
            })
            .collect();

        // Forget the caches of layers that are gone.
        caches.retain(|id, _| drawn.contains(id));

        geometries
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if matches!(state.pointer, Pointer::Dragging { .. }) {
            return mouse::Interaction::Grabbing;
        }

        if !cursor.is_over(bounds) {
            return mouse::Interaction::default();
        }

        let Some((layer, handle)) = state.hovered else {
            return mouse::Interaction::default();
        };

        let draggable = self.layer(layer).is_some_and(|layer| {
            layer
                .handles(&self.view(bounds))
                .iter()
                .any(|candidate| candidate.id == handle && candidate.draggable)
        });

        if draggable {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::Pointer
        }
    }
}

impl<Message> Overlay<'_, Message> {
    /// A finger, put through the pointer state machine as if it were the
    /// mouse.
    ///
    /// Only the first finger down on the overlay drives the gesture. A second
    /// one makes it a pinch: a press still waiting to become a click or a
    /// drag is handed to the map instead (so a two-finger tap places nothing),
    /// and a drag in progress keeps its handle, the second finger captured so
    /// the map does not pan under it.
    fn touch(
        &self,
        state: &mut State,
        event: touch::Event,
        bounds: Rectangle,
    ) -> Option<Action<Message>> {
        let (id, position) = match event {
            touch::Event::FingerPressed { id, position }
            | touch::Event::FingerMoved { id, position }
            | touch::Event::FingerLifted { id, position }
            | touch::Event::FingerLost { id, position } => (id, position),
        };
        let cursor = mouse::Cursor::Available(position);

        if state.finger.is_some_and(|finger| finger != id) {
            if !matches!(event, touch::Event::FingerPressed { .. }) {
                return None;
            }

            return match state.pointer {
                Pointer::Pressed { .. } => {
                    state.pointer = Pointer::Panning;
                    None
                }
                Pointer::Dragging { .. } => Some(Action::capture()),
                Pointer::Idle | Pointer::Panning => None,
            };
        }

        let event = match event {
            touch::Event::FingerPressed { .. } => {
                if !bounds.contains(position) {
                    return None;
                }
                state.finger = Some(id);
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            }
            touch::Event::FingerMoved { .. } if state.finger.is_some() => {
                Event::Mouse(mouse::Event::CursorMoved { position })
            }
            touch::Event::FingerLifted { .. } if state.finger.is_some() => {
                state.finger = None;
                // A finger leaves nothing hovered behind it.
                state.hovered = None;
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            }
            // The system took the gesture: a drag must not stay stuck, and a
            // press must not become a click.
            touch::Event::FingerLost { .. } if state.finger.is_some() => {
                state.finger = None;
                state.hovered = None;
                let cancelled = self.cancel(state);
                state.pointer = Pointer::Idle;
                return cancelled.or(Some(Action::request_redraw()));
            }
            _ => return None,
        };

        self.pointer(state, &event, bounds, cursor, Source::Touch)
    }

    /// The pointer state machine, shared by the mouse and a finger.
    fn pointer(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        source: Source,
    ) -> Option<Action<Message>> {
        let view = self.view(bounds);
        // Relative to the overlay; kept even outside it, so a drag can leave
        // the map and come back.
        let point = cursor
            .position()
            .map(|point| Point::new(point.x - bounds.x, point.y - bounds.y));
        let inside = cursor.is_over(bounds);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let point = point.filter(|_| inside)?;
                // A press while a gesture is live means its release was lost:
                // end that gesture before starting this one.
                let cancelled = self.cancel(state);
                let target = self.hit(&view, point, source.hit_radius());
                state.pointer = Pointer::Pressed { at: point, target };

                // Only a draggable handle keeps the map from panning.
                let pressed = target
                    .filter(|target| target.draggable)
                    .map(|_| Action::capture());
                cancelled.or(pressed)
            }

            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let point = point?;

                match state.pointer {
                    Pointer::Pressed { at, target } if at.distance(point) > source.threshold() => {
                        match target.filter(|target| target.draggable) {
                            Some(target) => {
                                state.pointer = Pointer::Dragging { target };
                                Some(self.drag_action(target, view.to_geo(point)))
                            }
                            None => {
                                state.pointer = Pointer::Panning;
                                None
                            }
                        }
                    }
                    Pointer::Dragging { target } => {
                        Some(self.drag_action(target, view.to_geo(point)))
                    }
                    Pointer::Idle => {
                        let hovered = inside
                            .then(|| self.hit(&view, point, source.hit_radius()))
                            .flatten()
                            .map(|target| (target.layer, target.handle));

                        (hovered != state.hovered).then(|| {
                            state.hovered = hovered;
                            Action::request_redraw()
                        })
                    }
                    Pointer::Pressed { .. } | Pointer::Panning => None,
                }
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let pointer = std::mem::take(&mut state.pointer);

                // A release is never captured, so the map always ends its own
                // pan, whatever the overlay made of the gesture.
                let message = match pointer {
                    Pointer::Pressed { target, at } => {
                        let at = view.to_geo(point.unwrap_or(at));
                        match target {
                            Some(target) => self.send(
                                target.layer,
                                LayerEvent::Click {
                                    handle: Some(target.handle),
                                    at,
                                },
                            ),
                            None => self.click_empty(at),
                        }
                    }
                    Pointer::Dragging { target } => {
                        let at = point
                            .map(|point| view.to_geo(point))
                            .unwrap_or(target.origin);
                        self.send(
                            target.layer,
                            LayerEvent::DragEnd {
                                handle: target.handle,
                                at,
                            },
                        )
                    }
                    Pointer::Idle | Pointer::Panning => None,
                };

                Some(match message {
                    Some(message) => Action::publish(message),
                    None => Action::request_redraw(),
                })
            }

            // Escape, or a window that lost focus and may never see the
            // release (alt-tab mid-drag), ends the gesture.
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            })
            | Event::Window(window::Event::Unfocused) => self.cancel(state),

            Event::Mouse(mouse::Event::CursorLeft) => {
                state.hovered.take().map(|_| Action::request_redraw())
            }

            _ => None,
        }
    }

    /// Ends the gesture in progress, if any, when it will not end on its
    /// own: escape, the window losing focus, a press while one is live, a
    /// finger the system took away. A drag is cancelled on its layer, which
    /// restores its origin; a press is forgotten, so its release is no click.
    fn cancel(&self, state: &mut State) -> Option<Action<Message>> {
        let target = match std::mem::take(&mut state.pointer) {
            Pointer::Dragging { target } => target,
            Pointer::Pressed { .. } | Pointer::Panning => return Some(Action::request_redraw()),
            Pointer::Idle => return None,
        };

        let message = self.send(
            target.layer,
            LayerEvent::DragCancel {
                handle: target.handle,
                origin: target.origin,
            },
        );

        Some(match message {
            Some(message) => Action::publish(message).and_capture(),
            None => Action::capture(),
        })
    }

    fn drag_action(&self, target: Target, at: GeoPosition) -> Action<Message> {
        match self.send(
            target.layer,
            LayerEvent::Drag {
                handle: target.handle,
                at,
            },
        ) {
            Some(message) => Action::publish(message).and_capture(),
            None => Action::capture(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::canvas::Program;
    use slippery::{Geodetic, Zoom};

    const LAYER: LayerId = LayerId::new("test");

    #[test]
    fn cached_layers_are_keyed_on_the_theme() {
        use crate::ui::theme::ThemeMode;

        let dark = theme_key(&ThemeMode::Dark.theme());
        assert_eq!(dark, theme_key(&ThemeMode::Dark.theme()), "stable");
        assert_ne!(dark, theme_key(&ThemeMode::Light.theme()));
    }
    const MARKER: HandleId = HandleId(7);

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Clicked(Option<HandleId>),
        Moved(HandleId),
        Dropped(HandleId),
        Cancelled(HandleId),
    }

    /// A layer with one draggable marker at the viewpoint (overlay centre).
    struct Marker;

    impl MapLayer<Msg> for Marker {
        fn id(&self) -> LayerId {
            LAYER
        }

        fn draw(&self, _frame: &mut MapFrame<'_>, _interaction: Interaction) {}

        fn handles(&self, _view: &MapView) -> Vec<Handle> {
            vec![Handle::at(MARKER, GeoPosition::new(47.4, 8.5), 10.0).draggable()]
        }

        fn on_event(&self, event: &LayerEvent) -> Option<Msg> {
            Some(match *event {
                LayerEvent::Click { handle, .. } => Msg::Clicked(handle),
                LayerEvent::Drag { handle, .. } => Msg::Moved(handle),
                LayerEvent::DragEnd { handle, .. } => Msg::Dropped(handle),
                LayerEvent::DragCancel { handle, .. } => Msg::Cancelled(handle),
            })
        }
    }

    const BOUNDS: Rectangle = Rectangle {
        x: 100.0,
        y: 50.0,
        width: 400.0,
        height: 300.0,
    };

    fn overlay() -> Overlay<'static, Msg> {
        Overlay::new(
            vec![Box::new(Marker)],
            Viewpoint {
                position: Geodetic::new(8.5, 47.4).as_mercator(),
                zoom: Zoom::try_from(12.0).unwrap(),
            },
            Padding::ZERO,
        )
    }

    /// A cursor at an overlay-relative point.
    fn at(x: f32, y: f32) -> mouse::Cursor {
        mouse::Cursor::Available(Point::new(BOUNDS.x + x, BOUNDS.y + y))
    }

    fn press() -> Event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
    }

    fn release() -> Event {
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
    }

    fn moved(cursor: mouse::Cursor) -> Event {
        Event::Mouse(mouse::Event::CursorMoved {
            position: cursor.position().unwrap(),
        })
    }

    fn message(action: Option<Action<Msg>>) -> (Option<Msg>, bool) {
        match action {
            Some(action) => {
                let (message, _, status) = action.into_inner();
                (message, status == iced::event::Status::Captured)
            }
            None => (None, false),
        }
    }

    const CENTRE: (f32, f32) = (200.0, 150.0);

    #[test]
    fn pressing_a_draggable_handle_captures_and_drags() {
        let overlay = overlay();
        let mut state = State::default();
        let centre = at(CENTRE.0, CENTRE.1);

        let (_, captured) = message(overlay.update(&mut state, &press(), BOUNDS, centre));
        assert!(captured, "the map must not pan under a drag");

        // Within the threshold: nothing yet.
        let nudge = at(CENTRE.0 + 2.0, CENTRE.1);
        assert_eq!(
            message(overlay.update(&mut state, &moved(nudge), BOUNDS, nudge)).0,
            None
        );

        let far = at(CENTRE.0 + 30.0, CENTRE.1);
        let (moved_message, captured) =
            message(overlay.update(&mut state, &moved(far), BOUNDS, far));
        assert_eq!(moved_message, Some(Msg::Moved(MARKER)));
        assert!(captured);

        let (dropped, captured) = message(overlay.update(&mut state, &release(), BOUNDS, far));
        assert_eq!(dropped, Some(Msg::Dropped(MARKER)));
        assert!(!captured, "releases always reach the map");
        assert_eq!(state.pointer, Pointer::Idle);
    }

    #[test]
    fn press_and_release_on_a_handle_is_a_click() {
        let overlay = overlay();
        let mut state = State::default();
        let centre = at(CENTRE.0, CENTRE.1);

        overlay.update(&mut state, &press(), BOUNDS, centre);
        let (clicked, _) = message(overlay.update(&mut state, &release(), BOUNDS, centre));
        assert_eq!(clicked, Some(Msg::Clicked(Some(MARKER))));
    }

    #[test]
    fn a_press_on_empty_map_pans_and_a_still_release_clicks() {
        let overlay = overlay();
        let mut state = State::default();
        let corner = at(10.0, 10.0);

        let (_, captured) = message(overlay.update(&mut state, &press(), BOUNDS, corner));
        assert!(!captured, "the map pans");
        let (clicked, _) = message(overlay.update(&mut state, &release(), BOUNDS, corner));
        assert_eq!(clicked, Some(Msg::Clicked(None)));

        // A press that moved is a pan, and its release is not a click.
        overlay.update(&mut state, &press(), BOUNDS, corner);
        let elsewhere = at(80.0, 80.0);
        overlay.update(&mut state, &moved(elsewhere), BOUNDS, elsewhere);
        assert_eq!(
            message(overlay.update(&mut state, &release(), BOUNDS, elsewhere)).0,
            None
        );
    }

    #[test]
    fn escape_cancels_a_drag() {
        let overlay = overlay();
        let mut state = State::default();
        let centre = at(CENTRE.0, CENTRE.1);
        let far = at(CENTRE.0 + 30.0, CENTRE.1);

        overlay.update(&mut state, &press(), BOUNDS, centre);
        overlay.update(&mut state, &moved(far), BOUNDS, far);

        let (cancelled, _) = message(overlay.update(&mut state, &escape(), BOUNDS, far));
        assert_eq!(cancelled, Some(Msg::Cancelled(MARKER)));

        // The release after a cancel does nothing.
        assert_eq!(
            message(overlay.update(&mut state, &release(), BOUNDS, far)).0,
            None
        );
    }

    fn escape() -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            modified_key: keyboard::Key::Named(keyboard::key::Named::Escape),
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::default(),
            text: None,
            repeat: false,
        })
    }

    /// A window that loses focus mid-drag may never see the release: the
    /// drag ends there, and the pointer coming back drags nothing.
    #[test]
    fn losing_focus_cancels_a_drag() {
        let overlay = overlay();
        let mut state = State::default();
        let centre = at(CENTRE.0, CENTRE.1);
        let far = at(CENTRE.0 + 30.0, CENTRE.1);

        overlay.update(&mut state, &press(), BOUNDS, centre);
        overlay.update(&mut state, &moved(far), BOUNDS, far);

        let unfocused = Event::Window(window::Event::Unfocused);
        let (cancelled, _) = message(overlay.update(&mut state, &unfocused, BOUNDS, far));
        assert_eq!(cancelled, Some(Msg::Cancelled(MARKER)));
        assert_eq!(state.pointer, Pointer::Idle);

        let back = at(CENTRE.0 + 60.0, CENTRE.1);
        assert_eq!(
            message(overlay.update(&mut state, &moved(back), BOUNDS, back)).0,
            None,
            "no drag without a button held"
        );
    }

    /// A press while a drag is live means its release was lost: the drag is
    /// cancelled before the new press starts.
    #[test]
    fn a_press_during_a_drag_cancels_it_first() {
        let overlay = overlay();
        let mut state = State::default();
        let centre = at(CENTRE.0, CENTRE.1);
        let far = at(CENTRE.0 + 30.0, CENTRE.1);

        overlay.update(&mut state, &press(), BOUNDS, centre);
        overlay.update(&mut state, &moved(far), BOUNDS, far);

        let corner = at(10.0, 10.0);
        let (cancelled, _) = message(overlay.update(&mut state, &press(), BOUNDS, corner));
        assert_eq!(cancelled, Some(Msg::Cancelled(MARKER)));
        assert!(matches!(state.pointer, Pointer::Pressed { .. }));
    }

    /// Escape forgets a press, so its release places nothing.
    #[test]
    fn escape_cancels_a_press() {
        let overlay = overlay();
        let mut state = State::default();
        let corner = at(10.0, 10.0);

        overlay.update(&mut state, &press(), BOUNDS, corner);
        overlay.update(&mut state, &escape(), BOUNDS, corner);
        assert_eq!(
            message(overlay.update(&mut state, &release(), BOUNDS, corner)).0,
            None,
            "no click"
        );
    }

    #[test]
    fn hovering_tracks_the_handle_under_the_pointer() {
        let overlay = overlay();
        let mut state = State::default();
        let centre = at(CENTRE.0, CENTRE.1);

        overlay.update(&mut state, &moved(centre), BOUNDS, centre);
        assert_eq!(state.hovered, Some((LAYER, MARKER)));
        assert_eq!(
            overlay.mouse_interaction(&state, BOUNDS, centre),
            mouse::Interaction::Grab
        );

        let away = at(10.0, 10.0);
        overlay.update(&mut state, &moved(away), BOUNDS, away);
        assert_eq!(state.hovered, None);
    }

    /// A finger at an overlay-relative point.
    fn finger_position(x: f32, y: f32) -> Point {
        Point::new(BOUNDS.x + x, BOUNDS.y + y)
    }

    fn finger_down(x: f32, y: f32) -> Event {
        Event::Touch(touch::Event::FingerPressed {
            id: touch::Finger(0),
            position: finger_position(x, y),
        })
    }

    fn finger_moved(x: f32, y: f32) -> Event {
        Event::Touch(touch::Event::FingerMoved {
            id: touch::Finger(0),
            position: finger_position(x, y),
        })
    }

    fn finger_up(x: f32, y: f32) -> Event {
        Event::Touch(touch::Event::FingerLifted {
            id: touch::Finger(0),
            position: finger_position(x, y),
        })
    }

    fn finger_lost(x: f32, y: f32) -> Event {
        Event::Touch(touch::Event::FingerLost {
            id: touch::Finger(0),
            position: finger_position(x, y),
        })
    }

    /// The touch path ignores the mouse cursor, which the platform may not
    /// move at all.
    const NO_CURSOR: mouse::Cursor = mouse::Cursor::Unavailable;

    #[test]
    fn a_finger_drags_a_handle_with_a_larger_slop() {
        let overlay = overlay();
        let mut state = State::default();

        let (_, captured) = message(overlay.update(
            &mut state,
            &finger_down(CENTRE.0, CENTRE.1),
            BOUNDS,
            NO_CURSOR,
        ));
        assert!(captured, "a draggable handle keeps the map from panning");

        // Past the mouse's threshold, still within a finger's slop.
        let nudge = (CENTRE.0 + 8.0, CENTRE.1);
        assert_eq!(
            message(overlay.update(
                &mut state,
                &finger_moved(nudge.0, nudge.1),
                BOUNDS,
                NO_CURSOR
            ))
            .0,
            None
        );

        let far = (CENTRE.0 + 40.0, CENTRE.1);
        let (moved, captured) =
            message(overlay.update(&mut state, &finger_moved(far.0, far.1), BOUNDS, NO_CURSOR));
        assert_eq!(moved, Some(Msg::Moved(MARKER)));
        assert!(captured);

        let (dropped, captured) =
            message(overlay.update(&mut state, &finger_up(far.0, far.1), BOUNDS, NO_CURSOR));
        assert_eq!(dropped, Some(Msg::Dropped(MARKER)));
        assert!(!captured, "releases always reach the map");
    }

    #[test]
    fn a_finger_hits_a_larger_area_than_a_cursor() {
        let overlay = overlay();
        let offset = TOUCH_TARGET_RADIUS - 2.0;

        // The marker's own radius is 10: a cursor misses at 20 pixels...
        let mut state = State::default();
        let near = at(CENTRE.0 + offset, CENTRE.1);
        let (_, captured) = message(overlay.update(&mut state, &press(), BOUNDS, near));
        assert!(!captured, "the map pans instead");

        // ...a finger does not.
        let mut state = State::default();
        let (_, captured) = message(overlay.update(
            &mut state,
            &finger_down(CENTRE.0 + offset, CENTRE.1),
            BOUNDS,
            NO_CURSOR,
        ));
        assert!(captured);

        let (clicked, _) = message(overlay.update(
            &mut state,
            &finger_up(CENTRE.0 + offset, CENTRE.1),
            BOUNDS,
            NO_CURSOR,
        ));
        assert_eq!(clicked, Some(Msg::Clicked(Some(MARKER))));
    }

    #[test]
    fn a_finger_on_empty_map_is_left_to_the_map() {
        let overlay = overlay();
        let mut state = State::default();

        let (_, captured) =
            message(overlay.update(&mut state, &finger_down(10.0, 10.0), BOUNDS, NO_CURSOR));
        assert!(!captured, "the map pans and pinches with the finger");

        let (clicked, _) =
            message(overlay.update(&mut state, &finger_up(10.0, 10.0), BOUNDS, NO_CURSOR));
        assert_eq!(clicked, Some(Msg::Clicked(None)), "a tap places a waypoint");
    }

    #[test]
    fn a_lost_finger_cancels_a_drag() {
        let overlay = overlay();
        let mut state = State::default();

        overlay.update(
            &mut state,
            &finger_down(CENTRE.0, CENTRE.1),
            BOUNDS,
            NO_CURSOR,
        );

        let far = (CENTRE.0 + 40.0, CENTRE.1);
        overlay.update(&mut state, &finger_moved(far.0, far.1), BOUNDS, NO_CURSOR);

        let (cancelled, captured) =
            message(overlay.update(&mut state, &finger_lost(far.0, far.1), BOUNDS, NO_CURSOR));
        assert_eq!(cancelled, Some(Msg::Cancelled(MARKER)));
        assert!(captured);
        assert_eq!(state.pointer, Pointer::Idle);
    }

    /// A second finger's event at an overlay-relative point.
    fn second_finger(event: fn(touch::Finger, Point) -> touch::Event, x: f32, y: f32) -> Event {
        Event::Touch(event(touch::Finger(1), finger_position(x, y)))
    }

    fn pressed(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerPressed { id, position }
    }

    fn lifted(id: touch::Finger, position: Point) -> touch::Event {
        touch::Event::FingerLifted { id, position }
    }

    #[test]
    fn a_two_finger_tap_places_nothing() {
        let overlay = overlay();
        let mut state = State::default();

        overlay.update(&mut state, &finger_down(10.0, 10.0), BOUNDS, NO_CURSOR);
        let (_, captured) = message(overlay.update(
            &mut state,
            &second_finger(pressed, 60.0, 10.0),
            BOUNDS,
            NO_CURSOR,
        ));
        assert!(!captured, "the map pinches with both fingers");

        let (clicked, _) =
            message(overlay.update(&mut state, &finger_up(10.0, 10.0), BOUNDS, NO_CURSOR));
        assert_eq!(clicked, None);
        let (clicked, _) = message(overlay.update(
            &mut state,
            &second_finger(lifted, 60.0, 10.0),
            BOUNDS,
            NO_CURSOR,
        ));
        assert_eq!(clicked, None);
        assert_eq!(state.pointer, Pointer::Idle);
    }

    #[test]
    fn a_second_finger_does_not_interrupt_a_drag() {
        let overlay = overlay();
        let mut state = State::default();

        overlay.update(
            &mut state,
            &finger_down(CENTRE.0, CENTRE.1),
            BOUNDS,
            NO_CURSOR,
        );
        let far = (CENTRE.0 + 40.0, CENTRE.1);
        overlay.update(&mut state, &finger_moved(far.0, far.1), BOUNDS, NO_CURSOR);

        let (_, captured) = message(overlay.update(
            &mut state,
            &second_finger(pressed, 10.0, 10.0),
            BOUNDS,
            NO_CURSOR,
        ));
        assert!(captured, "the map must not pan under the drag");
        assert!(matches!(state.pointer, Pointer::Dragging { .. }));

        // The second finger lifting first ends nothing.
        let (ended, _) = message(overlay.update(
            &mut state,
            &second_finger(lifted, 10.0, 10.0),
            BOUNDS,
            NO_CURSOR,
        ));
        assert_eq!(ended, None);

        let (dropped, _) =
            message(overlay.update(&mut state, &finger_up(far.0, far.1), BOUNDS, NO_CURSOR));
        assert_eq!(dropped, Some(Msg::Dropped(MARKER)));
    }
}
