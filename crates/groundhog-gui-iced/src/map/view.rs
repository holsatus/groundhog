//! What a layer sees of the map: the projection, and a frame to draw into.

use iced::{
    Color, Font, Padding, Pixels, Point, Rectangle, Size,
    advanced::text::Alignment as TextAlignment,
    alignment::Vertical,
    widget::canvas::{Fill, Frame, Path, Stroke, Text},
};
use slippery::{Geodetic, Mercator, Projector, Viewpoint};

use groundhog_logic::domain::geo::GeoPosition;

/// The map's current projection, in overlay coordinates: pixels from the
/// overlay's top-left corner.
#[derive(Debug, Clone, PartialEq)]
pub struct MapView {
    projector: Projector,
    visible: Rectangle,
}

impl MapView {
    /// The view of an overlay of `size`, `covered` along its edges by what
    /// floats over the map.
    pub(super) fn new(viewpoint: Viewpoint, size: Size, covered: Padding) -> Self {
        let bounds = Rectangle::with_size(size);
        Self {
            projector: Projector { viewpoint, bounds },
            visible: visible(bounds, covered),
        }
    }

    /// A fixed view for tests of layers.
    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self::new(
            Viewpoint {
                position: Geodetic::new(8.0, 47.0).as_mercator(),
                zoom: slippery::Zoom::try_from(14.0).expect("a valid zoom"),
            },
            Size::new(800.0, 600.0),
            Padding::ZERO,
        )
    }

    pub fn size(&self) -> Size {
        self.projector.bounds.size()
    }

    /// The part of the overlay nothing floats over. Layers anchored to the
    /// screen (an instrument in a corner) keep to it.
    pub fn visible(&self) -> Rectangle {
        self.visible
    }

    /// The zoom level (slippy-map convention, larger is closer).
    pub fn zoom(&self) -> f64 {
        self.projector.viewpoint.zoom.f64()
    }

    /// Where `position` appears, possibly outside the visible area.
    pub fn to_screen(&self, position: GeoPosition) -> Point {
        self.projector
            .mercator_into_screen_space(mercator(position))
    }

    /// The geographic position under an overlay point.
    pub fn to_geo(&self, point: Point) -> GeoPosition {
        let geodetic = self.projector.screen_space_into_geodetic(point);
        GeoPosition::new(geodetic.latitude(), geodetic.longitude())
    }

    /// Whether `point` lies within the overlay, grown by `margin` pixels.
    pub fn contains(&self, point: Point, margin: f32) -> bool {
        let size = self.size();
        point.x >= -margin
            && point.y >= -margin
            && point.x <= size.width + margin
            && point.y <= size.height + margin
    }

    /// Metres covered by one pixel at `latitude`.
    pub fn metres_per_pixel(&self, latitude: f64) -> f64 {
        const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
        const TILE_SIZE: f64 = 256.0;
        EARTH_CIRCUMFERENCE_M * latitude.to_radians().cos() / (TILE_SIZE * 2f64.powf(self.zoom()))
    }

    /// A value identifying this view for cache keys: equal views draw
    /// identically.
    pub(super) fn key(&self) -> [u64; 5] {
        let viewpoint = &self.projector.viewpoint;
        let size = self.size();
        [
            viewpoint.position.east_x().to_bits(),
            viewpoint.position.south_y().to_bits(),
            viewpoint.zoom.f64().to_bits(),
            u64::from(size.width.to_bits()),
            u64::from(size.height.to_bits()),
        ]
    }
}

/// Project a geographic position into Web-Mercator map coordinates.
///
/// `Geodetic::new` takes `(lon, lat)`, so the argument order is deliberate.
pub(super) fn mercator(position: GeoPosition) -> Mercator {
    Geodetic::new(position.lon, position.lat).as_mercator()
}

/// A frame to draw one layer into, with geographic drawing helpers.
///
/// Anything the helpers do not cover is available through
/// [`frame`](MapFrame::frame) in overlay coordinates.
pub struct MapFrame<'a> {
    frame: &'a mut Frame,
    view: &'a MapView,
}

impl<'a> MapFrame<'a> {
    pub(super) fn new(frame: &'a mut Frame, view: &'a MapView) -> Self {
        Self { frame, view }
    }

    pub fn view(&self) -> &MapView {
        self.view
    }

    /// The underlying canvas frame, in overlay coordinates.
    pub fn frame(&mut self) -> &mut Frame {
        self.frame
    }

    /// Stroke a line through `positions`.
    pub fn polyline(
        &mut self,
        positions: impl IntoIterator<Item = GeoPosition>,
        stroke: Stroke<'_>,
    ) {
        let view = self.view;
        let path = Path::new(|builder| {
            let mut points = positions
                .into_iter()
                .map(|position| view.to_screen(position));
            if let Some(first) = points.next() {
                builder.move_to(first);
                for point in points {
                    builder.line_to(point);
                }
            }
        });
        self.frame.stroke(&path, stroke);
    }

    /// Fill a circle of `radius` pixels at `position`.
    pub fn circle(&mut self, position: GeoPosition, radius: f32, fill: impl Into<Fill>) {
        let center = self.view.to_screen(position);
        self.frame.fill(&Path::circle(center, radius), fill);
    }

    /// Stroke the outline of a circle of `radius` pixels at `position`.
    pub fn ring(&mut self, position: GeoPosition, radius: f32, stroke: Stroke<'_>) {
        let center = self.view.to_screen(position);
        self.frame.stroke(&Path::circle(center, radius), stroke);
    }

    /// Draw `content` centred on `position`.
    pub fn label(
        &mut self,
        position: GeoPosition,
        content: impl Into<String>,
        size: f32,
        color: Color,
    ) {
        let mut text = Text::from(content.into());
        text.font = Font::MONOSPACE;
        text.position = self.view.to_screen(position);
        text.color = color;
        text.size = Pixels(size);
        text.align_x = TextAlignment::Center;
        text.align_y = Vertical::Center;
        self.frame.fill_text(text);
    }
}

/// `bounds` less `covered`, never less than nothing.
fn visible(bounds: Rectangle, covered: Padding) -> Rectangle {
    Rectangle {
        x: bounds.x + covered.left,
        y: bounds.y + covered.top,
        width: (bounds.width - covered.left - covered.right).max(0.0),
        height: (bounds.height - covered.top - covered.bottom).max(0.0),
    }
}

/// Where the map must look for `position` to appear in the middle of what
/// is visible of it, when `covered` of its edges is hidden.
pub(super) fn centred_in_visible(
    viewpoint: Viewpoint,
    position: Mercator,
    covered: Padding,
) -> Mercator {
    // The projection is linear around the bounds' centre, so any bounds do:
    // with `position` at the centre, the point as far from it as the visible
    // centre is from the whole centre, the other way, is where to look.
    let projector = Projector {
        viewpoint: Viewpoint {
            position,
            zoom: viewpoint.zoom,
        },
        bounds: Rectangle::with_size(Size::ZERO),
    };
    let shift = iced::Vector::new(
        (covered.left - covered.right) / 2.0,
        (covered.top - covered.bottom) / 2.0,
    );
    projector.screen_space_into_mercator(Point::ORIGIN - shift)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slippery::Zoom;

    fn view() -> MapView {
        MapView::new(
            Viewpoint {
                position: Geodetic::new(8.5, 47.4).as_mercator(),
                zoom: Zoom::try_from(12.0).unwrap(),
            },
            Size::new(800.0, 600.0),
            Padding::ZERO,
        )
    }

    #[test]
    fn a_position_centred_in_the_visible_part_appears_there() {
        let position = GeoPosition::new(47.4, 8.5);
        let covered = Padding {
            top: 40.0,
            right: 0.0,
            bottom: 300.0,
            left: 100.0,
        };
        let viewpoint = Viewpoint {
            position: centred_in_visible(view().projector.viewpoint, mercator(position), covered),
            zoom: Zoom::try_from(12.0).unwrap(),
        };

        let view = MapView::new(viewpoint, Size::new(800.0, 600.0), covered);
        let at = view.to_screen(position);
        let visible = view.visible();
        assert!(
            (at.x - visible.center_x()).abs() < 0.01,
            "{at:?} in {visible:?}"
        );
        assert!(
            (at.y - visible.center_y()).abs() < 0.01,
            "{at:?} in {visible:?}"
        );
    }

    #[test]
    fn the_viewpoint_is_the_centre_of_the_overlay() {
        let centre = view().to_screen(GeoPosition::new(47.4, 8.5));
        assert!((centre.x - 400.0).abs() < 0.01);
        assert!((centre.y - 300.0).abs() < 0.01);
    }

    #[test]
    fn projection_round_trips() {
        let view = view();
        let point = Point::new(123.0, 456.0);
        let back = view.to_screen(view.to_geo(point));
        assert!((back.x - point.x).abs() < 0.01 && (back.y - point.y).abs() < 0.01);
    }

    #[test]
    fn scale_shrinks_towards_the_poles() {
        let view = view();
        assert!(view.metres_per_pixel(60.0) < view.metres_per_pixel(0.0));
    }
}
