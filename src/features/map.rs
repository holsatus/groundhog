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
    color::Oklch,
    widget::{
        canvas::{gradient::Linear, Fill, Frame, Gradient, LineCap, Path, Stroke, Style},
        image::Handle,
    },
    Color, Element, Point, Rectangle, Vector,
};
use slippery::{
    location, CacheMessage, MapProgram, Mercator, Projector, TileCache, Viewpoint, Zoom,
};

use crate::{domain::vehicle::Vehicle, foundation::MavlinkId};

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
    SetViewPosition(Mercator),
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
            Message::SetViewPosition(mercator) => {
                self.viewpoint.position = mercator;
            }
            Message::Cache(message) => {
                return self.tile_cache.update(message).map(Message::Cache);
            }
            Message::Follow(value) => self.follow = value,
            Message::DrawDots(value) => self.draw_dots = value,
        }

        iced::Task::none()
    }

    pub fn view<'a>(&'a self, vehicles: &'a BTreeMap<MavlinkId, Vehicle>) -> Element<'a, Message> {
        let draw_dots = self.draw_dots;

        MapProgram::new(&self.tile_cache)
            .on_cache(Message::Cache)
            .on_update(Message::Projector)
            .with_draw_layer(move |projector, frame| {
                // Darken the map background to make overlay stand out more
                let mut black_fill = Fill::default();
                black_fill.style = Style::Solid(Color::BLACK.scale_alpha(0.5));
                frame.fill_rectangle([0.0, 0.0].into(), frame.size(), black_fill);

                iced::debug::time_with("position_segments_view", || {
                    for vehicle in vehicles.values() {
                        draw_vehicle(vehicle, projector, frame, draw_dots);
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

fn draw_vehicle(vehicle: &Vehicle, projector: &Projector, frame: &mut Frame, draw_dots: bool) {
    let Some(position) = &vehicle.global_position else {
        return;
    };

    let center = projector.mercator_into_screen_space(position.pos);

    let yaw_angle = vehicle
        .attitude
        .as_ref()
        .map(|att| att.attitude.euler_angles().2)
        .unwrap_or_default();

    let screen_points = vehicle
        .global_positions
        .iter()
        .map(|position| ScreenPosition {
            point: projector.mercator_into_screen_space(position.pos),
            dark: altitude_to_color(position.alt_agl, 0.4),
            light: altitude_to_color(position.alt_agl, 0.85),
            time: position.at,
        })
        .collect::<Vec<_>>();

    let mut points_iterator = screen_points.iter();
    let Some(mut prev) = points_iterator.next() else {
        return;
    };

    // First draw all the dark background sections
    for next in points_iterator {
        let long_time = prev.time.saturating_duration_since(next.time) > Duration::from_secs(30);

        DrawLineSegment {
            from_point: prev.point,
            from_color: prev.dark,
            to_point: next.point,
            to_color: next.dark,
            faint: long_time,
            width: 10.0,
        }
        .draw(frame);

        prev = next;
    }

    let mut points_iterator = screen_points.iter();
    let Some(mut prev) = points_iterator.next() else {
        return;
    };

    // Then draw all the light foreground sections
    for next in points_iterator {
        let long_time = prev.time.saturating_duration_since(next.time) > Duration::from_secs(30);

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

        prev = next;
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
