//! The map engine: a slippy tile map with a layered, interactive overlay.
//!
//! The engine knows nothing about vehicles or missions. Features contribute
//! [`MapLayer`]s — usually through a [`MapPlugin`] — and the engine draws them
//! in z-order, caches their geometry by revision, hit-tests their
//! [`Handle`]s, and runs the pointer state machine (hover, click, drag,
//! cancel), reporting gestures back to the owning layer as [`LayerEvent`]s.
//!
//! ```ignore
//! let mut layers = LayerSet::new();
//! layers.plugin(&self.vehicles, context, Message::Vehicles);
//! layers.plugin(&self.mission, context, Message::Mission);
//! self.map.view(layers, Message::Map, &theme)
//! ```

mod layer;
mod layered;
mod overlay;
mod view;

pub use layer::{
    Anchor, Handle, HandleId, Interaction, LayerEvent, LayerId, LayerSet, MapLayer, MapPlugin,
};
pub use view::{MapFrame, MapView};

use iced::{
    Color, Element, Length, Point, Size, Task, Theme, Widget,
    widget::canvas::{Canvas, Fill, LineJoin, Path, Stroke, Style, fill},
};
use slippery::{CacheMessage, MapWidget, Projector, TileCache, Viewpoint, Zoom, location};

use groundhog_logic::domain::geo::GeoPosition;

/// The map's own state: where it looks, and its tile cache.
pub struct Map {
    viewpoint: Viewpoint,
    tile_cache: TileCache,
    /// Whether the view follows a position fed through [`Map::follow_to`].
    follow: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The operator moved or zoomed the map.
    Projector(Projector),
    /// A tile cache message produced by the map widget.
    Cache(CacheMessage),
}

impl Default for Map {
    fn default() -> Self {
        Self::new()
    }
}

impl Map {
    pub fn new() -> Self {
        Self {
            viewpoint: Viewpoint {
                position: location::paris().as_mercator(),
                zoom: Zoom::try_from(8.0).expect("a valid zoom"),
            },
            tile_cache: TileCache::new(slippery::sources::OpenStreetMap),
            follow: false,
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Projector(projector) => {
                // The operator took over the view.
                self.viewpoint = projector.viewpoint;
                self.follow = false;
                Task::none()
            }
            Message::Cache(message) => self.tile_cache.update(message).map(Message::Cache),
        }
    }

    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Keep the view centred on positions fed through [`Map::follow_to`],
    /// until the operator moves the map.
    pub fn set_following(&mut self, follow: bool) {
        self.follow = follow;
    }

    /// Centre the view on `position` if following.
    pub fn follow_to(&mut self, position: GeoPosition) {
        if self.follow {
            self.center_on(position);
        }
    }

    /// Centre the view on `position`.
    pub fn center_on(&mut self, position: GeoPosition) {
        self.viewpoint.position = view::mercator(position);
    }

    /// The map with `layers` on top. `wrap` lifts the map's own messages into
    /// the layers' message type.
    pub fn view<'a, M: 'static>(
        &'a self,
        mut layers: LayerSet<'a, M>,
        wrap: impl Fn(Message) -> M + 'a,
        theme: &Theme,
    ) -> Element<'a, M> {
        let palette = theme.palette();
        layers.push(Chrome {
            id: LayerId::new("map.tint"),
            z_index: i32::MIN,
            kind: ChromeKind::Tint(palette.background.base.color.scale_alpha(0.3)),
        });
        layers.push(Chrome {
            id: LayerId::new("map.frame"),
            z_index: i32::MAX,
            kind: ChromeKind::Frame {
                background: palette.background.base.color,
                border: palette.background.strongest.color,
            },
        });

        let tiles: Element<'a, Message> =
            MapWidget::new(&self.tile_cache, Message::Cache, self.viewpoint)
                .on_update(Message::Projector)
                .boxed();

        let overlay = Canvas::new(overlay::Overlay::new(layers.into_sorted(), self.viewpoint))
            .width(Length::Fill)
            .height(Length::Fill);

        // Not a `Stack`: it would hide the pointer from the tiles while a
        // handle is hovered, disabling zoom and pan there (see `layered`).
        layered::Layered::new(tiles.map(wrap).boxed(), overlay.boxed()).boxed()
    }
}

/// The map's own decoration, drawn as ordinary layers.
struct Chrome {
    id: LayerId,
    z_index: i32,
    kind: ChromeKind,
}

enum ChromeKind {
    /// A wash over the tiles, so overlays stand out.
    Tint(Color),
    /// The pane's rounded corners and border: the map paints opaque tiles
    /// across its whole pane, which a rounded container cannot clip.
    Frame { background: Color, border: Color },
}

impl<M> MapLayer<M> for Chrome {
    fn id(&self) -> LayerId {
        self.id
    }

    fn z_index(&self) -> i32 {
        self.z_index
    }

    fn revision(&self) -> Option<u64> {
        // Only the view (size) and the theme change these, and the engine
        // keys every cache on both.
        Some(0)
    }

    fn draw(&self, frame: &mut MapFrame<'_>, _interaction: Interaction) {
        let size = frame.view().size();
        let frame = frame.frame();

        match self.kind {
            ChromeKind::Tint(color) => frame.fill_rectangle(Point::ORIGIN, size, color),
            ChromeKind::Frame { background, border } => {
                draw_rounded_frame(
                    frame,
                    size,
                    crate::ui::style::PANE_RADIUS,
                    crate::ui::style::PANE_BORDER_WIDTH,
                    background,
                    border,
                );
            }
        }
    }
}

fn draw_rounded_frame(
    frame: &mut iced::widget::canvas::Frame,
    size: Size,
    radius: f32,
    width: f32,
    background: Color,
    border: Color,
) {
    // Rectangle minus rounded rectangle, even-odd: exactly the four corners.
    let corners = Path::new(|builder| {
        builder.rectangle(Point::ORIGIN, size);

        let half_width = width / 2.0;
        builder.rounded_rectangle(
            Point::new(half_width, half_width),
            Size::new(size.width - width, size.height - width),
            (radius - half_width).into(),
        );
    });

    frame.fill(
        &corners,
        Fill {
            style: Style::Solid(background),
            rule: fill::Rule::EvenOdd,
        },
    );

    // Inset by half the stroke so the border stays inside the bounds; the
    // overlay canvas is not clipped to them.
    let inset = width / 2.0;
    let outline = Path::rounded_rectangle(
        Point::new(inset, inset),
        Size::new(size.width - width, size.height - width),
        (radius - inset).max(0.0).into(),
    );

    frame.stroke(
        &outline,
        Stroke {
            style: Style::Solid(border),
            width,
            line_join: LineJoin::Round,
            ..Stroke::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{Point, Theme, event::Status, mouse};

    const CENTRE: Point = Point::new(512.0, 384.0);
    const AWAY: Point = Point::new(100.0, 100.0);

    /// A handle at the centre of the simulator's 1024x768 window.
    struct CentreHandle {
        draggable: bool,
    }

    impl MapLayer<Message> for CentreHandle {
        fn id(&self) -> LayerId {
            LayerId::new("test.centre")
        }

        fn draw(&self, _frame: &mut MapFrame<'_>, _interaction: Interaction) {}

        fn handles(&self, _view: &MapView) -> Vec<Handle> {
            vec![Handle {
                id: HandleId(1),
                anchor: Anchor::Screen(CENTRE),
                radius: 20.0,
                draggable: self.draggable,
            }]
        }
    }

    /// Hover `at`, then deliver `event`; the event's status.
    fn after_hovering(draggable: bool, at: Point, event: mouse::Event) -> Status {
        let map = Map::new();
        let mut layers = LayerSet::new();
        layers.push(CentreHandle { draggable });
        let mut simulator =
            iced_test::Simulator::new(map.view(layers, |message| message, &Theme::Dark));

        simulator.point_at(at);
        simulator.simulate([iced::Event::Mouse(mouse::Event::CursorMoved {
            position: at,
        })]);
        simulator.simulate([iced::Event::Mouse(event)])[0]
    }

    fn wheel() -> mouse::Event {
        mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
        }
    }

    /// The tiles capture the wheel to zoom. A hovered handle (which shows a
    /// grab cursor) must not hide the pointer from them.
    #[test]
    fn the_map_zooms_over_hovered_handles() {
        assert_eq!(after_hovering(true, AWAY, wheel()), Status::Captured);
        assert_eq!(after_hovering(true, CENTRE, wheel()), Status::Captured);
        assert_eq!(after_hovering(false, CENTRE, wheel()), Status::Captured);
    }

    /// Pressing a clickable (not draggable) handle still lets the map start
    /// a pan, so dragging from a vehicle icon pans the map.
    #[test]
    fn the_map_pans_from_clickable_handles() {
        let press = mouse::Event::ButtonPressed(mouse::Button::Left);
        assert_eq!(after_hovering(false, CENTRE, press), Status::Captured);
    }
}
