//! The layer API: what a feature implements to put things on the map.
//!
//! A [`MapLayer`] draws into the map overlay and, optionally, exposes
//! interactive [`Handle`]s. The map engine owns all pointer state (hover,
//! press, drag, click detection, escape-to-cancel) and reports the outcome to
//! the layer as a [`LayerEvent`]; the layer answers with a message of its
//! feature. Layers are rebuilt each frame from the feature's state, borrowed
//! for the frame, and type-erased, so the map knows nothing about missions,
//! vehicles or any other feature.
//!
//! A feature usually contributes its layers through [`MapPlugin`], which the
//! application calls with its own context type (fleet snapshot, theme, ...)
//! to collect every plugin's layers into one [`LayerSet`].

use std::rc::Rc;

use groundhog_logic::domain::geo::GeoPosition;
use iced::Point;

use super::view::{MapFrame, MapView};

/// A layer's stable identity across frames. The engine keys its geometry
/// cache and pointer state on it, so it must not change between frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerId(&'static str);

impl LayerId {
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    #[cfg(test)]
    pub fn name(self) -> &'static str {
        self.0
    }
}

/// One interactive element's identity within its layer, chosen by the layer
/// (a mission item id, a system id, ...). It must be stable across frames for
/// a drag to survive the frames it spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandleId(pub u64);

/// Where a handle sits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    /// At a geographic position; moves with the map.
    Geo(GeoPosition),
    /// At a fixed point of the overlay, in pixels from its top-left corner.
    Screen(Point),
}

/// An interactive element: a circular hit area around an anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    pub id: HandleId,
    pub anchor: Anchor,
    /// The hit radius in pixels.
    pub radius: f32,
    /// Whether pressing it starts a drag (which keeps the map from panning).
    pub draggable: bool,
}

impl Handle {
    /// A clickable handle at a geographic position.
    pub fn at(id: HandleId, position: GeoPosition, radius: f32) -> Self {
        Self {
            id,
            anchor: Anchor::Geo(position),
            radius,
            draggable: false,
        }
    }

    pub fn draggable(mut self) -> Self {
        self.draggable = true;
        self
    }
}

/// A pointer gesture, as resolved by the engine for one layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayerEvent {
    /// A press and release without movement. `handle` is the layer's handle
    /// under the pointer; `None` for a click on empty map, which is offered to
    /// layers from the top down until one answers with a message.
    Click {
        handle: Option<HandleId>,
        at: GeoPosition,
    },
    /// A draggable handle moved to `at`. Sent for every pointer move of the
    /// drag, starting once the pointer has moved past a small threshold.
    Drag { handle: HandleId, at: GeoPosition },
    /// The drag ended with the button released at `at`.
    DragEnd { handle: HandleId, at: GeoPosition },
    /// The drag was cancelled (escape); `origin` is where the handle was
    /// when the drag began, so the layer can put it back.
    DragCancel {
        handle: HandleId,
        origin: GeoPosition,
    },
}

/// The pointer's relation to a layer's handles this frame, for drawing hover
/// and drag feedback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Interaction {
    pub hovered: Option<HandleId>,
    pub dragged: Option<HandleId>,
}

impl Interaction {
    /// Whether `handle` is hovered or dragged.
    pub fn is_active(&self, handle: HandleId) -> bool {
        self.hovered == Some(handle) || self.dragged == Some(handle)
    }
}

/// Something drawn on the map, possibly interactive.
///
/// Only [`id`](MapLayer::id) and [`draw`](MapLayer::draw) are required. A
/// layer whose content changes rarely should report a
/// [`revision`](MapLayer::revision): while it and the view are unchanged, the
/// engine reuses the layer's previous geometry instead of drawing again.
pub trait MapLayer<Message> {
    fn id(&self) -> LayerId;

    /// Paint order: higher is drawn later (on top) and is hit-tested first.
    fn z_index(&self) -> i32 {
        0
    }

    /// A value that changes whenever the layer's drawing would. `None` (the
    /// default) redraws every frame.
    fn revision(&self) -> Option<u64> {
        None
    }

    /// Draw the layer. `interaction` tells which of its handles the pointer
    /// is hovering or dragging.
    fn draw(&self, frame: &mut MapFrame<'_>, interaction: Interaction);

    /// The layer's interactive elements under the current view.
    fn handles(&self, _view: &MapView) -> Vec<Handle> {
        Vec::new()
    }

    /// React to a gesture on this layer, with a message for its feature.
    fn on_event(&self, _event: &LayerEvent) -> Option<Message> {
        None
    }
}

/// The layers of one map frame, type-erased to a common message type.
pub struct LayerSet<'a, Message> {
    layers: Vec<Box<dyn MapLayer<Message> + 'a>>,
}

impl<Message> Default for LayerSet<'_, Message> {
    fn default() -> Self {
        Self { layers: Vec::new() }
    }
}

impl<'a, Message: 'a> LayerSet<'a, Message> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, layer: impl MapLayer<Message> + 'a) {
        self.layers.push(Box::new(layer));
    }

    /// Collect `plugin`'s layers, translating their messages with `map`.
    pub fn plugin<Context, P: MapPlugin<'a, Context>>(
        &mut self,
        plugin: &'a P,
        context: Context,
        map: impl Fn(P::Message) -> Message + 'a,
    ) where
        P::Message: 'a,
    {
        let mut layers = LayerSet::new();
        plugin.layers(context, &mut layers);
        self.extend(layers, map);
    }

    /// Add every layer of `other`, translating its messages with `map`.
    pub fn extend<Other: 'a>(
        &mut self,
        other: LayerSet<'a, Other>,
        map: impl Fn(Other) -> Message + 'a,
    ) {
        let map: Rc<dyn Fn(Other) -> Message + 'a> = Rc::new(map);

        for layer in other.layers {
            self.layers.push(Box::new(Mapped {
                layer,
                map: map.clone(),
            }));
        }
    }

    /// The layers in paint order (lowest z first; insertion order breaks ties).
    pub(crate) fn into_sorted(mut self) -> Vec<Box<dyn MapLayer<Message> + 'a>> {
        self.layers.sort_by_key(|layer| layer.z_index());
        self.layers
    }
}

/// A source of map layers: typically a feature, contributing layers built
/// from its own state and the application's `Context`.
///
/// The context is passed by value and borrows for the frame (`'a`), so the
/// application can build it on the fly in its `view`.
pub trait MapPlugin<'a, Context> {
    type Message;

    fn layers(&'a self, context: Context, layers: &mut LayerSet<'a, Self::Message>);
}

/// A layer whose messages are translated to another type.
struct Mapped<'a, A, B> {
    layer: Box<dyn MapLayer<A> + 'a>,
    map: Rc<dyn Fn(A) -> B + 'a>,
}

impl<A, B> MapLayer<B> for Mapped<'_, A, B> {
    fn id(&self) -> LayerId {
        self.layer.id()
    }

    fn z_index(&self) -> i32 {
        self.layer.z_index()
    }

    fn revision(&self) -> Option<u64> {
        self.layer.revision()
    }

    fn draw(&self, frame: &mut MapFrame<'_>, interaction: Interaction) {
        self.layer.draw(frame, interaction)
    }

    fn handles(&self, view: &MapView) -> Vec<Handle> {
        self.layer.handles(view)
    }

    fn on_event(&self, event: &LayerEvent) -> Option<B> {
        self.layer.on_event(event).map(&*self.map)
    }
}
