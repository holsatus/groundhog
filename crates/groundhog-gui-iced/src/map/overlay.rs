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
    Point, Rectangle, Renderer, Theme, keyboard, mouse,
    widget::canvas::{self, Action, Event, Frame, Geometry},
};
use slippery::Viewpoint;

use super::{
    layer::{Anchor, Handle, HandleId, Interaction, LayerEvent, LayerId, MapLayer},
    view::{MapFrame, MapView},
};

/// Pointer travel, in pixels, that turns a press into a drag (or a pan).
pub const DRAG_THRESHOLD: f32 = 4.0;

/// The canvas program hosting a frame's layers.
pub(super) struct Overlay<'a, Message> {
    layers: Vec<Box<dyn MapLayer<Message> + 'a>>,
    viewpoint: Viewpoint,
}

impl<'a, Message> Overlay<'a, Message> {
    pub fn new(layers: Vec<Box<dyn MapLayer<Message> + 'a>>, viewpoint: Viewpoint) -> Self {
        Self { layers, viewpoint }
    }

    fn view(&self, bounds: Rectangle) -> MapView {
        MapView::new(self.viewpoint, bounds.size())
    }

    fn layer(&self, id: LayerId) -> Option<&(dyn MapLayer<Message> + 'a)> {
        self.layers
            .iter()
            .find(|layer| layer.id() == id)
            .map(|layer| layer.as_ref())
    }

    /// The topmost handle under `point`.
    fn hit(&self, view: &MapView, point: Point) -> Option<Target> {
        for layer in self.layers.iter().rev() {
            let closest = layer
                .handles(view)
                .into_iter()
                .filter_map(|handle| {
                    let center = anchor_point(view, &handle);
                    let distance = center.distance(point);
                    (distance <= handle.radius).then_some((handle, distance))
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

impl<Message> canvas::Program<Message> for Overlay<'_, Message> {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
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
                let target = self.hit(&view, point);
                state.pointer = Pointer::Pressed { at: point, target };

                // Only a draggable handle keeps the map from panning.
                target
                    .filter(|target| target.draggable)
                    .map(|_| Action::capture())
            }

            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let point = point?;

                match state.pointer {
                    Pointer::Pressed { at, target } if at.distance(point) > DRAG_THRESHOLD => {
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
                            .then(|| self.hit(&view, point))
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

            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => {
                let Pointer::Dragging { target } = state.pointer else {
                    return None;
                };
                state.pointer = Pointer::Idle;

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

            Event::Mouse(mouse::Event::CursorLeft) => {
                state.hovered.take().map(|_| Action::request_redraw())
            }

            _ => None,
        }
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let view = self.view(bounds);
        let view_key = view.key();
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
                        (revision, view_key, interaction).hash(&mut hasher);
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

        let escape = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            modified_key: keyboard::Key::Named(keyboard::key::Named::Escape),
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::default(),
            text: None,
            repeat: false,
        });
        let (cancelled, _) = message(overlay.update(&mut state, &escape, BOUNDS, far));
        assert_eq!(cancelled, Some(Msg::Cancelled(MARKER)));

        // The release after a cancel does nothing.
        assert_eq!(
            message(overlay.update(&mut state, &release(), BOUNDS, far)).0,
            None
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
}
