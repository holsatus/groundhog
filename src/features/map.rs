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
    widget::{
        canvas::{Fill, Frame, Gradient, LineCap, Path, Stroke, Style, Text, gradient::Linear},
        image::Handle,
    },
};
use slippery::{
    CacheMessage, Geodetic, MapProgram, Mercator, Projector, TileCache, Viewpoint, Zoom, location,
};

use crate::{domain::geo::GeoPosition, domain::vehicle::Vehicle, foundation::MavlinkId};

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
        }

        iced::Task::none()
    }

    pub fn view<'a>(
        &'a self,
        vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
        primary: Option<MavlinkId>,
    ) -> Element<'a, Message> {
        let draw_dots = self.draw_dots;

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
                        draw_mission(vehicle, projector, frame);
                    }
                })
            })
            .build(self.viewpoint)
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
/// waypoint. The current execution item is highlighted.
///
/// A pure function of the mission and the [`Projector`], kept separate from
/// [`draw_vehicle`] so screen-to-geodetic editing can be layered on later.
/// Never panics on an empty mission, an `Unknown` command, or a mission with no
/// positioned items.
fn draw_mission(vehicle: &Vehicle, projector: &Projector, frame: &mut Frame) {
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
        let is_current = current == Some(index as u16);
        let (radius, color) = if is_current {
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
