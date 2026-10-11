//! The map view: an OpenStreetMap tile layer with a canvas overlay that draws
//! vehicle position trails and heading arrows.
//!
//! Owns the viewpoint, projector, and tile cache. It is given the current set
//! of vehicles purely for drawing; it does not own vehicle state.

use std::{
    collections::BTreeMap,
    sync::LazyLock,
    time::{Duration, Instant},
};

use iced::{
    Color, Element, Pixels, Point, Rectangle, Vector,
    advanced::text::Alignment as TextAlignment,
    alignment::Vertical,
    color::Oklch,
    mouse,
    widget::{
        canvas::{
            Event, Fill, Frame, Gradient, LineCap, Path, Stroke, Style, Text, gradient::Linear,
        },
        image::Handle,
    },
};
use slippery::{
    Action, CacheMessage, Geodetic, MapProgram, Mercator, Projector, TileCache, Viewpoint, Zoom,
    location,
};

use crate::{domain::geo::GeoPosition, domain::vehicle::Vehicle, foundation::SystemId};

static ARROW_HANDLE: LazyLock<Handle> =
    LazyLock::new(|| Handle::from_bytes(include_bytes!("../../assets/pointer.png").as_slice()));

/// The map layer's own state.
pub struct Map {
    viewpoint: Viewpoint,
    projector: Option<Projector>,
    tile_cache: TileCache,
    /// Whether the viewpoint follows the primary vehicle's position.
    pub(crate) follow: bool,
    draw_dots: bool,
    /// The mission item currently being dragged, if any.
    drag: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The map has moved/zoomed; carries the new projector.
    Projector(Projector),
    /// Recenter the view on a position (used to follow the primary vehicle).
    SetViewPosition(GeoPosition),
    /// A tile cache message produced by the map widget.
    Cache(CacheMessage),
    /// Toggle following the primary vehicle.
    Follow(bool),
    /// Toggle drawing dots on the position trail (debug).
    #[allow(dead_code)] // UI control is currently commented out
    DrawDots(bool),
    /// A mission waypoint was pressed on the map (select it, begin a drag).
    WaypointPressed(usize),
    /// A dragged waypoint's new geographic position.
    WaypointDragged(usize, GeoPosition),
    /// A waypoint drag ended.
    WaypointReleased,
    /// The map was clicked in "place waypoint" mode.
    PlaceAt(GeoPosition),
}

impl Map {
    pub fn new() -> Self {
        Self {
            viewpoint: Viewpoint {
                position: location::paris().as_mercator(),
                zoom: Zoom::try_from(8.0).unwrap(),
            },
            projector: None,
            tile_cache: TileCache::new(slippery::sources::OpenStreetMap),
            follow: false,
            draw_dots: false,
            drag: None,
        }
    }

    pub fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::Projector(projector) => {
                self.viewpoint = projector.viewpoint;
                self.follow = false;
                self.projector = Some(projector);
            }
            Message::SetViewPosition(pos) => {
                self.viewpoint.position = mercator(pos);
            }
            Message::Cache(message) => {
                return self.tile_cache.update(message).map(Message::Cache);
            }
            Message::Follow(value) => self.follow = value,
            Message::DrawDots(value) => self.draw_dots = value,
            Message::WaypointPressed(index) => self.drag = Some(index),
            Message::WaypointReleased => self.drag = None,
            Message::WaypointDragged(..) | Message::PlaceAt(_) => {}
        }

        iced::Task::none()
    }

    pub fn view<'a>(
        &'a self,
        vehicles: &'a BTreeMap<SystemId, Vehicle>,
        primary: Option<SystemId>,
        placing: bool,
    ) -> Element<'a, Message> {
        let draw_dots = self.draw_dots;
        let drag = self.drag;

        MapProgram::new(&self.tile_cache)
            .on_cache(Message::Cache)
            .on_update(Message::Projector)
            .with_draw_layer(move |projector, frame| {
                // Darken the map background to make overlay stand out more
                let black_fill = Fill {
                    style: Style::Solid(Color::BLACK.scale_alpha(0.4)),
                    ..Fill::default()
                };
                frame.fill_rectangle([0.0, 0.0].into(), frame.size(), black_fill);

                iced::debug::time_with("position_segments_view", || {
                    for vehicle in vehicles.values() {
                        draw_vehicle(vehicle, projector, frame, draw_dots);
                    }

                    if let Some(id) = primary
                        && let Some(vehicle) = vehicles.get(&id)
                    {
                        draw_mission(vehicle, projector, frame, drag);
                    }
                })
            })
            .with_interaction(move |projector, cursor, event| {
                // The interaction projector's `bounds` are absolute screen
                // coordinates (iced passes `layout.bounds()`), and its projection
                // methods work in that same absolute space. The cursor is
                // therefore read with `position_over`; unlike the draw layer,
                // whose projector normalises bounds to the origin, we must NOT
                // use `position_in`.
                interact(projector, cursor, event, vehicles, primary, placing, drag)
            })
            .build(self.viewpoint)
    }
}

/// Decide what a canvas event means for the mission overlay.
///
/// `projector.bounds` are absolute screen coordinates and the projection
/// methods operate in that space, so the cursor is read with
/// [`mouse::Cursor::position_over`] (absolute), never `position_in` (relative).
fn interact(
    projector: &Projector,
    cursor: &mouse::Cursor,
    event: &Event,
    vehicles: &BTreeMap<SystemId, Vehicle>,
    primary: Option<SystemId>,
    placing: bool,
    drag: Option<usize>,
) -> Action<Message> {
    let point = cursor.position_over(projector.bounds);

    match event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
            let Some(point) = point else {
                return Action::None;
            };

            if let Some(vehicle) = primary.and_then(|id| vehicles.get(&id))
                && let Some(index) = waypoint_at(vehicle, projector, point)
            {
                return Action::Capture(Message::WaypointPressed(index));
            }

            if placing && primary.is_some() {
                return Action::Capture(Message::PlaceAt(cursor_geo(projector, point)));
            }

            Action::None
        }
        Event::Mouse(mouse::Event::CursorMoved { .. }) => {
            let Some(index) = drag else {
                return Action::None;
            };
            let Some(point) = point else {
                return Action::None;
            };

            Action::Capture(Message::WaypointDragged(
                index,
                cursor_geo(projector, point),
            ))
        }
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
            if drag.is_some() {
                Action::Capture(Message::WaypointReleased)
            } else {
                Action::None
            }
        }
        _ => Action::None,
    }
}

struct ScreenPosition {
    point: Point,
    dark: Color,
    light: Color,
    time: Instant,
}

struct DrawLineSegment {
    from_point: Point,
    to_point: Point,
    from_color: Color,
    to_color: Color,
    faint: bool,
    width: f32,
}

impl DrawLineSegment {
    fn draw(self, frame: &mut Frame) {
        let alpha = if self.faint { 0.25 } else { 1.0 };
        let mut stroke = Stroke::default()
            .with_width(self.width)
            .with_line_cap(LineCap::Round);
        stroke.style = Style::Gradient(Gradient::Linear(
            Linear::new(self.from_point, self.to_point)
                .add_stop(0.0, self.from_color.scale_alpha(alpha))
                .add_stop(1.0, self.to_color.scale_alpha(alpha)),
        ));

        frame.stroke(&Path::line(self.from_point, self.to_point), stroke);
    }
}

/// Draw one vehicle's trail and heading arrow onto the overlay.
///
/// The overlay re-projects every trail point and re-strokes every segment on
/// each draw, so its cost scales with trail length times redraw frequency. A
/// projection cache keyed by `(viewpoint, position-revision)` is the known next
/// step, but it was deferred: profiling shows stroke frequency dominates the
/// cost, and a cache would add untested staleness risk. Revisit with a profiler.
fn draw_vehicle(vehicle: &Vehicle, projector: &Projector, frame: &mut Frame, draw_dots: bool) {
    let Some(position) = &vehicle.position.current else {
        return;
    };

    let center = projector.mercator_into_screen_space(mercator(position.pos));

    let yaw_angle = vehicle
        .attitude
        .current
        .as_ref()
        .map(|att| att.attitude.euler_angles().2)
        .unwrap_or_default();

    // Trails need at least two points; the heading arrow is drawn regardless.
    // Only project the trail when there is actually a segment to draw.
    if vehicle.position.trail.len() >= 2 {
        let screen_points = vehicle
            .position
            .trail
            .iter()
            .map(|position| ScreenPosition {
                point: projector.mercator_into_screen_space(mercator(position.pos)),
                dark: altitude_to_color(position.alt_agl, 0.4),
                light: altitude_to_color(position.alt_agl, 0.85),
                time: position.at,
            })
            .collect::<Vec<_>>();

        // First draw all the dark background sections
        for pair in screen_points.windows(2) {
            let (prev, next) = (&pair[0], &pair[1]);
            let long_time =
                prev.time.saturating_duration_since(next.time) > Duration::from_secs(30);

            DrawLineSegment {
                from_point: prev.point,
                from_color: prev.dark,
                to_point: next.point,
                to_color: next.dark,
                faint: long_time,
                width: 10.0,
            }
            .draw(frame);
        }

        // Then draw all the light foreground sections
        for pair in screen_points.windows(2) {
            let (prev, next) = (&pair[0], &pair[1]);
            let long_time =
                prev.time.saturating_duration_since(next.time) > Duration::from_secs(30);

            DrawLineSegment {
                from_point: prev.point,
                from_color: prev.light,
                to_point: next.point,
                to_color: next.light,
                faint: long_time,
                width: 4.0,
            }
            .draw(frame);

            if draw_dots {
                frame.fill(&Path::circle(prev.point, 5.0), Fill::from(Color::WHITE));
            }
        }
    }

    frame.with_save(|frame| {
        frame.translate(Vector::new(center.x, center.y));
        frame.rotate(yaw_angle);

        const WIDTH: f32 = 60.0;
        frame.draw_image(
            Rectangle {
                x: -WIDTH / 2.0,
                y: -WIDTH / 2.0,
                width: WIDTH,
                height: WIDTH,
            },
            &*ARROW_HANDLE,
        );
    });
}

/// Draw the primary vehicle's mission onto the overlay: the route as line
/// segments between consecutive positioned items, and a numbered marker per
/// waypoint. The current execution item is highlighted, and `dragged` (if any)
/// is drawn distinctly so the drag is visible.
///
/// A pure function of the mission and the [`Projector`], kept separate from
/// [`draw_vehicle`] so screen-to-geodetic editing can be layered on later.
/// Never panics on an empty mission, an `Unknown` command, or a mission with no
/// positioned items.
fn draw_mission(
    vehicle: &Vehicle,
    projector: &Projector,
    frame: &mut Frame,
    dragged: Option<usize>,
) {
    let mission = &vehicle.mission;
    let current = mission.progress.current_seq;

    // Only positioned commands get a marker or take part in the route.
    let points: Vec<(usize, Point)> = mission
        .items()
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            item.command.position().map(|pos| {
                let geo = GeoPosition::new(pos.lat, pos.lon);
                (index, projector.mercator_into_screen_space(mercator(geo)))
            })
        })
        .collect();

    // Route: a single consistent stroke between consecutive positioned items.
    for pair in points.windows(2) {
        let stroke = Stroke {
            style: Style::Solid(Color::WHITE.scale_alpha(0.6)),
            width: 2.0,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };

        frame.stroke(&Path::line(pair[0].1, pair[1].1), stroke);
    }

    // Markers, each numbered with its 1-based mission index.
    for (index, point) in points {
        let (radius, color) = if dragged == Some(index) {
            (9.0, Color::from_rgb(1.0, 0.6, 0.1))
        } else if current == Some(index as u16) {
            (7.0, Color::from_rgb(0.3, 0.9, 0.4))
        } else {
            (5.0, Color::WHITE.scale_alpha(0.85))
        };

        frame.fill(&Path::circle(point, radius), Fill::from(color));

        let mut label = Text::from((index + 1).to_string());
        label.position = point;
        label.color = Color::BLACK;
        label.size = Pixels(11.0);
        label.align_x = TextAlignment::Center;
        label.align_y = Vertical::Center;
        frame.fill_text(label);
    }
}

/// Screen-space hit radius for a mission waypoint marker.
const WAYPOINT_HIT_RADIUS: f32 = 12.0;

/// The geographic position under a screen-space point.
fn cursor_geo(projector: &Projector, point: Point) -> GeoPosition {
    let geodetic = projector.screen_space_into_geodetic(point);
    GeoPosition::new(geodetic.latitude(), geodetic.longitude())
}

/// The nearest positioned mission item within [`WAYPOINT_HIT_RADIUS`] pixels of
/// `cursor`, if any.
fn waypoint_at(vehicle: &Vehicle, projector: &Projector, cursor: Point) -> Option<usize> {
    let radius_sq = WAYPOINT_HIT_RADIUS * WAYPOINT_HIT_RADIUS;

    vehicle
        .mission
        .items()
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let pos = item.command.position()?;
            let point = projector.geodetic_into_screen_space(Geodetic::new(pos.lon, pos.lat));
            let offset = point - cursor;
            let distance_sq = offset.x * offset.x + offset.y * offset.y;

            (distance_sq <= radius_sq).then_some((index, distance_sq))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(index, _)| index)
}

/// Project a geographic position into Web-Mercator map coordinates.
///
/// `Geodetic::new` takes `(lon, lat)`, so the argument order is deliberate.
fn mercator(pos: GeoPosition) -> Mercator {
    Geodetic::new(pos.lon, pos.lat).as_mercator()
}

/// Convert an altitude in meters to a corresponding color, for use with position trails.
/// The color sweeps from yellow -> green -> blue -> purple -> red as altitude goes from 0.0 -> 1000.0.
fn altitude_to_color(altitude_m: f32, brightness: f32) -> Color {
    const MIN_ALT_M: f32 = 0.0;
    const MAX_ALT_M: f32 = 1000.0;
    const YELLOW_HUE_DEG: f32 = 90.0;
    const PURPLE_HUE_DEG: f32 = 380.0;

    let t = ((altitude_m - MIN_ALT_M) / (MAX_ALT_M - MIN_ALT_M)).clamp(0.0, 1.0);
    let hue_deg = YELLOW_HUE_DEG + t * (PURPLE_HUE_DEG - YELLOW_HUE_DEG);

    Color::from(Oklch {
        l: brightness,
        c: 0.2 * brightness,
        h: hue_deg.to_radians(),
        a: 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::enums::MavFrame;

    use crate::domain::mission::{MissionCommand, MissionCommandKind, MissionItem, Position};

    fn projector() -> Projector {
        Projector {
            viewpoint: Viewpoint {
                position: Geodetic::new(8.5, 47.4).as_mercator(),
                zoom: Zoom::try_from(12.0).unwrap(),
            },
            bounds: Rectangle {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 1000.0,
            },
        }
    }

    fn vehicle_with_waypoint(lat: f64, lon: f64) -> Vehicle {
        let mut vehicle = Vehicle::new(SystemId(1));

        vehicle.mission.push(MissionItem {
            frame: MavFrame::GlobalRelativeAltInt,
            autocontinue: true,
            command: MissionCommand::default_for(
                MissionCommandKind::Waypoint,
                Position {
                    lat,
                    lon,
                    alt_m: 50.0,
                },
            ),
        });

        vehicle
    }

    #[test]
    fn waypoint_at_hits_the_marker_and_misses_elsewhere() {
        let projector = projector();
        let vehicle = vehicle_with_waypoint(47.4, 8.5);

        let on_marker = projector.geodetic_into_screen_space(Geodetic::new(8.5, 47.4));
        assert_eq!(waypoint_at(&vehicle, &projector, on_marker), Some(0));

        let away = Point::new(on_marker.x + 200.0, on_marker.y + 200.0);
        assert_eq!(waypoint_at(&vehicle, &projector, away), None);
    }

    #[test]
    fn cursor_geo_round_trips_through_screen_space() {
        let projector = projector();
        let point = Point::new(250.0, 750.0);

        let geo = cursor_geo(&projector, point);
        let back = projector.geodetic_into_screen_space(Geodetic::new(geo.lon, geo.lat));

        assert!(
            (back.x - point.x).abs() < 1e-3,
            "x {} vs {}",
            back.x,
            point.x
        );
        assert!(
            (back.y - point.y).abs() < 1e-3,
            "y {} vs {}",
            back.y,
            point.y
        );
    }

    /// The interaction layer must read the cursor in absolute screen space (the
    /// projector's bounds have a non-zero origin, as in the real window). With
    /// `position_in` (relative) this test fails, because the marker hit-test is
    /// offset by the map's absolute origin.
    #[test]
    fn interact_reads_absolute_screen_coordinates() {
        let projector = Projector {
            viewpoint: Viewpoint {
                position: Geodetic::new(8.5, 47.4).as_mercator(),
                zoom: Zoom::try_from(12.0).unwrap(),
            },
            bounds: Rectangle {
                x: 45.0,
                y: 30.0,
                width: 1000.0,
                height: 1000.0,
            },
        };

        let id = SystemId(1);
        let mut vehicles = BTreeMap::new();
        vehicles.insert(id, vehicle_with_waypoint(47.4, 8.5));

        let pressed = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));

        // The waypoint's absolute screen point must hit the marker.
        let absolute = projector.geodetic_into_screen_space(Geodetic::new(8.5, 47.4));
        assert!(matches!(
            interact(
                &projector,
                &mouse::Cursor::Available(absolute),
                &pressed,
                &vehicles,
                Some(id),
                false,
                None,
            ),
            Action::Capture(Message::WaypointPressed(0))
        ));

        // A press far from any marker is ignored when not placing...
        let away = Point::new(absolute.x + 200.0, absolute.y + 200.0);
        assert!(matches!(
            interact(
                &projector,
                &mouse::Cursor::Available(away),
                &pressed,
                &vehicles,
                Some(id),
                false,
                None,
            ),
            Action::None
        ));

        // ...and places a waypoint when placing.
        assert!(matches!(
            interact(
                &projector,
                &mouse::Cursor::Available(away),
                &pressed,
                &vehicles,
                Some(id),
                true,
                None,
            ),
            Action::Capture(Message::PlaceAt(_))
        ));
    }
}
