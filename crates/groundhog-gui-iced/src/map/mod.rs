//! The map engine: a slippy tile map with a layered, interactive overlay.
//!
//! The engine knows nothing about vehicles or missions. Panels contribute
//! [`MapLayer`]s, gathered in a [`LayerSet`], and the engine draws them
//! in z-order, caches their geometry by revision, hit-tests their
//! [`Handle`]s, and runs the pointer state machine (hover, click, drag,
//! cancel), reporting gestures back to the owning layer as [`LayerEvent`]s.
//!
//! ```ignore
//! let mut mission = LayerSet::new();
//! mission.push(Route { draft });
//! let mut layers = LayerSet::new();
//! layers.extend(mission, Message::Mission);
//! self.map.view(layers, Message::Map, &theme, Placement::Pane)
//! ```

mod layer;
mod layered;
mod overlay;
mod view;

pub use layer::{Anchor, Handle, HandleId, Interaction, LayerEvent, LayerId, LayerSet, MapLayer};
pub use view::{MapFrame, MapView};

use std::path::PathBuf;

use iced::{Color, Length, Padding, Point, Task, Theme, Widget, widget::canvas::Canvas};
use slippery::{CacheMessage, MapWidget, Projector, TileCache, Viewpoint, Zoom, location};

use groundhog_logic::domain::geo::GeoPosition;

/// The map's own state: where it looks, and its tile cache.
pub struct Map {
    viewpoint: Viewpoint,
    tile_cache: TileCache,
    /// Whether the view follows a position fed through [`Map::follow_to`].
    follow: bool,
}

/// How the map sits in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// In a pane, with nothing over it. The pane grid draws the pane's
    /// corners and border over the map.
    Pane,
    /// Running to the window's edges, with `covered` of each edge hidden
    /// under what floats over the map.
    Window { covered: Padding },
}

impl Placement {
    /// How much of each edge is hidden.
    pub fn covered(self) -> Padding {
        match self {
            Placement::Pane => Padding::ZERO,
            Placement::Window { covered } => covered,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The operator moved or zoomed the map.
    Projector(Projector),
    /// A tile cache message produced by the map widget.
    Cache(CacheMessage),
}

/// How much disk the stored tiles may take before the least recently used
/// are removed.
const TILE_CACHE_BYTES: u64 = 256 << 20;

impl Map {
    /// A map whose tiles are kept in `tile_dir`, so they survive restarts and
    /// show without a network connection. Without one, tiles are kept in
    /// memory only.
    pub fn new(tile_dir: Option<PathBuf>) -> Self {
        let tile_cache = TileCache::new(slippery::sources::OpenStreetMap);
        let tile_cache = match tile_dir {
            Some(dir) => tile_cache.with_disk_cache(dir, TILE_CACHE_BYTES),
            None => tile_cache,
        };

        Self {
            viewpoint: Viewpoint {
                position: location::paris().as_mercator(),
                zoom: Zoom::try_from(8.0).expect("a valid zoom"),
            },
            tile_cache,
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

    /// Where the view is centred, and the meters of ground a logical pixel
    /// spans there.
    #[cfg(not(target_os = "android"))]
    pub fn centre(&self) -> (GeoPosition, f64) {
        let geodetic = self.viewpoint.position.as_geodetic();
        let centre = GeoPosition::new(geodetic.latitude(), geodetic.longitude());
        (centre, metres_per_px(centre.lat, self.viewpoint.zoom.f64()))
    }

    /// Centre the view on `centre`, zoomed so a logical pixel spans
    /// `metres_per_px` of ground there, as near as the map's zoom allows.
    #[cfg(not(target_os = "android"))]
    pub fn look_at(&mut self, centre: GeoPosition, metres_per_px: f64) {
        let zoom = (metres_per_px_at_zoom_0(centre.lat) / metres_per_px)
            .log2()
            .clamp(Zoom::MIN.f64(), Zoom::MAX.f64());
        self.viewpoint = Viewpoint {
            position: view::mercator(centre),
            zoom: Zoom::try_from(zoom).unwrap_or(self.viewpoint.zoom),
        };
    }

    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Keep the view centred on positions fed through [`Map::follow_to`],
    /// until the operator moves the map.
    pub fn set_following(&mut self, follow: bool) {
        self.follow = follow;
    }

    /// Centre the view on `position` if following, in the part of the map
    /// placed as `placement` that nothing covers.
    pub fn follow_to(&mut self, position: GeoPosition, placement: Placement) {
        if self.follow {
            self.center_on(position, placement);
        }
    }

    /// Centre the view on `position`, in the part of the map placed as
    /// `placement` that nothing covers.
    pub fn center_on(&mut self, position: GeoPosition, placement: Placement) {
        self.viewpoint.position = view::centred_in_visible(
            self.viewpoint,
            view::mercator(position),
            placement.covered(),
        );
    }

    /// The map with `layers` on top, placed as `placement`. `wrap` lifts the
    /// map's own messages into the layers' message type.
    pub fn view<'a, M: 'static, F: Fn(Message) -> M + 'a>(
        &'a self,
        mut layers: LayerSet<'a, M>,
        wrap: F,
        theme: &Theme,
        placement: Placement,
    ) -> impl Widget<M> + use<'a, M, F> {
        layers.push(Tint(theme.palette().background.base.color.scale_alpha(0.3)));

        let tiles = MapWidget::new(&self.tile_cache, Message::Cache, self.viewpoint)
            .on_update(Message::Projector);

        let overlay = Canvas::new(overlay::Overlay::new(
            layers.into_sorted(),
            self.viewpoint,
            placement.covered(),
        ))
        .width(Length::Fill)
        .height(Length::Fill);

        // Not a `Stack`: it would hide the pointer from the tiles while a
        // handle is hovered, disabling zoom and pan there (see `layered`).
        layered::Layered::new(Widget::<Message>::map(tiles, wrap), overlay)
    }
}

/// The meters of ground a logical pixel spans at latitude `lat` (degrees)
/// on a map at `zoom`.
#[cfg(not(target_os = "android"))]
fn metres_per_px(lat: f64, zoom: f64) -> f64 {
    metres_per_px_at_zoom_0(lat) / zoom.exp2()
}

/// [`metres_per_px`] at zoom 0, where the world is one tile wide.
#[cfg(not(target_os = "android"))]
fn metres_per_px_at_zoom_0(lat: f64) -> f64 {
    /// The circumference of the earth at the equator, in meters.
    const EQUATOR_M: f64 = 40_075_016.7;
    // The east edge of the world is one half of its width east of the centre
    let world_px = 2.0 * slippery::Mercator::new(1.0, 0.0).into_pixel_space(0.0).x;
    EQUATOR_M * lat.to_radians().cos() / world_px
}

/// A wash over the tiles, so overlays stand out: the map's own decoration,
/// drawn as an ordinary layer, beneath every other.
struct Tint(Color);

impl<M> MapLayer<M> for Tint {
    fn id(&self) -> LayerId {
        LayerId::new("map.tint")
    }

    fn z_index(&self) -> i32 {
        i32::MIN
    }

    fn revision(&self) -> Option<u64> {
        // Only the view (size) and the theme change it, and the engine keys
        // every cache on both.
        Some(0)
    }

    fn draw(&self, frame: &mut MapFrame<'_>, _interaction: Interaction) {
        let size = frame.view().size();
        frame.frame().fill_rectangle(Point::ORIGIN, size, self.0);
    }
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
        let map = Map::new(None);
        let mut layers = LayerSet::new();
        layers.push(CentreHandle { draggable });
        let mut simulator = iced_test::Simulator::new(map.view(
            layers,
            |message| message,
            &Theme::Dark,
            Placement::Pane,
        ));

        simulator.point_at(at);
        simulator.simulate([iced::Event::Mouse(mouse::Event::CursorMoved {
            position: at,
        })]);
        simulator.simulate([iced::Event::Mouse(event)])[0]
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn the_scale_a_view_is_set_to_is_the_scale_it_reports() {
        let mut map = Map::new(None);
        let copenhagen = GeoPosition::new(55.68, 12.57);
        map.look_at(copenhagen, 2.5);
        let (centre, scale) = map.centre();
        assert!((centre.lat - copenhagen.lat).abs() < 1e-9, "{centre:?}");
        assert!((centre.lon - copenhagen.lon).abs() < 1e-9, "{centre:?}");
        assert!((scale - 2.5).abs() < 1e-9, "{scale}");

        // At zoom 0, the equator's 40 000 km across slippery's 512 pixels
        assert!((metres_per_px(0.0, 0.0) - 40_075_016.7 / 512.0).abs() < 1e-6);
        // Each zoom level halves it
        assert!((metres_per_px(0.0, 1.0) * 2.0 - metres_per_px(0.0, 0.0)).abs() < 1e-6);
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
