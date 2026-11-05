//! The vehicles' map layers: trails, home markers, vehicle icons and the
//! attitude HUD of the selected vehicle.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    time::Duration,
};

use groundhog_logic::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        vehicle::{Vehicle, position::GlobalPosition},
    },
};
use iced::{
    Color, Point,
    color::Oklch,
    theme::Palette,
    widget::canvas::{LineCap, LineJoin, Path, Stroke, Style, gradient},
};

use super::Message;
use crate::{
    map::{
        Anchor, Handle, HandleId, Interaction, LayerEvent, LayerId, LayerSet, MapFrame, MapLayer,
        MapView,
    },
    panels::MapContext,
    widget::attitude,
};

/// Trail points closer than this on screen are merged.
const DECIMATE_PX: f32 = 1.5;
/// A gap between fixes longer than this is drawn faint: the track broke.
const GAP: Duration = Duration::from_secs(30);
/// The altitude at which the trail's colour reaches its last hue.
const TOP_ALT_M: f32 = 1000.0;
/// The smallest step an 8-bit screen shows: colours closer than this in
/// every channel look the same.
const INVISIBLE: f32 = 1.0 / 255.0;
const ICON_SIZE: f32 = 40.0;

impl super::VehiclePanel {
    /// The vehicles' trails, homes and icons, and the selected vehicle's
    /// attitude.
    pub(super) fn map_layers<'a>(
        &'a self,
        context: MapContext<'a>,
        layers: &mut LayerSet<'a, Message>,
    ) {
        // Every casing below every core, so a tail never covers the body's
        // core where the two meet.
        for pass in [TrailPass::Casing, TrailPass::Core] {
            for part in [TrailPart::Body, TrailPart::Tail] {
                layers.push(Trails {
                    fleet: context.fleet,
                    part,
                    pass,
                });
            }
        }
        layers.push(Homes {
            fleet: context.fleet,
            selected: context.selected,
        });
        layers.push(Icons {
            fleet: context.fleet,
            selected: context.selected,
            palette: context.palette,
        });
        if let Some(vehicle) = context.selected.and_then(|id| context.fleet.vehicle(id)) {
            layers.push(Hud {
                vehicle,
                palette: context.palette,
            });
        }
    }
}

/// How many of the newest fixes the uncached tail draws. The body is
/// re-stroked once per this many fixes; between re-strokes the tail covers the
/// fixes the cached body does not have yet.
const TAIL_FIXES: u64 = 16;

/// Which part of the trails a layer draws.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TrailPart {
    /// Every fix, cached and re-stroked only every [`TAIL_FIXES`] fixes (or
    /// when the view changes).
    Body,
    /// The newest fixes, drawn every frame. Cheap: a handful of segments.
    Tail,
}

/// Which stroke of the trails a layer draws.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TrailPass {
    /// The wide, dark outline.
    Casing,
    /// The narrow, bright line on top of the casing.
    Core,
}

impl TrailPass {
    /// Stroke width and brightness.
    fn style(self) -> (f32, f32) {
        match self {
            TrailPass::Casing => (8.0, 0.4),
            TrailPass::Core => (3.0, 0.85),
        }
    }
}

/// Where each vehicle has been, coloured by altitude: each segment shades
/// from the colour of its first fix's altitude to its second's, so the colour
/// changes continuously along the trail. A segment whose ends look the same
/// colour is drawn solid instead, which is cheaper (see [`segment_style`]).
///
/// Stroking a long trail is the most expensive drawing on the map, and a
/// moving vehicle changes its trail on every fix. So the trail is split: a
/// cached body that is only redrawn every [`TAIL_FIXES`] fixes, and a short
/// tail from the body's end to the newest fix, drawn every frame. The two
/// overlap by a few segments, which is invisible.
///
/// Each part is drawn in two layers, one per [`TrailPass`], and every casing
/// layer sits below every core layer: the whole outline is drawn before any
/// bright line, across parts and vehicles.
struct Trails<'a> {
    fleet: &'a FleetSnapshot,
    part: TrailPart,
    pass: TrailPass,
}

impl MapLayer<Message> for Trails<'_> {
    fn id(&self) -> LayerId {
        match (self.pass, self.part) {
            (TrailPass::Casing, TrailPart::Body) => LayerId::new("vehicles.trail-casing"),
            (TrailPass::Casing, TrailPart::Tail) => LayerId::new("vehicles.trail-casing-tail"),
            (TrailPass::Core, TrailPart::Body) => LayerId::new("vehicles.trail-core"),
            (TrailPass::Core, TrailPart::Tail) => LayerId::new("vehicles.trail-core-tail"),
        }
    }

    fn z_index(&self) -> i32 {
        match self.pass {
            TrailPass::Casing => 0,
            TrailPass::Core => 1,
        }
    }

    fn revision(&self) -> Option<u64> {
        if self.part == TrailPart::Tail {
            return None;
        }

        let mut hasher = DefaultHasher::new();
        for vehicle in self.fleet.vehicles() {
            (vehicle.system, vehicle.position.revision() / TAIL_FIXES).hash(&mut hasher);
        }
        Some(hasher.finish())
    }

    fn draw(&self, frame: &mut MapFrame<'_>, _interaction: Interaction) {
        let view = frame.view().clone();
        let (width, brightness) = self.pass.style();
        let stroke = |style| Stroke {
            style,
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        // Solid segments are stroked as they come, shaded ones after them
        // all: the renderer starts a new mesh (and a draw call) each time
        // the strokes switch between solid and gradient, and keeps one mesh
        // for any number in a row.
        let mut shaded = Vec::new();

        for vehicle in self.fleet.vehicles() {
            let trail = &vehicle.position.trail;
            // One more than the fixes not yet in the body, to join up with it.
            let skip = match self.part {
                TrailPart::Body => 0,
                TrailPart::Tail => trail.len().saturating_sub(TAIL_FIXES as usize + 1),
            };

            let mut previous: Option<(Point, &GlobalPosition)> = None;

            for fix in trail.iter().skip(skip) {
                let point = view.to_screen(fix.pos);

                let Some((last_point, last_fix)) = previous else {
                    previous = Some((point, fix));
                    continue;
                };

                if last_point.distance(point) < DECIMATE_PX {
                    continue;
                }

                let visible = view.contains(last_point, 50.0) || view.contains(point, 50.0);
                if visible {
                    let faint = fix.at.saturating_duration_since(last_fix.at) > GAP;
                    let alpha = if faint { 0.25 } else { 1.0 };
                    let path = Path::line(last_point, point);
                    match segment_style(
                        (last_point, last_fix.alt_agl),
                        (point, fix.alt_agl),
                        brightness,
                        alpha,
                    ) {
                        style @ Style::Solid(_) => frame.frame().stroke(&path, stroke(style)),
                        style @ Style::Gradient(_) => shaded.push((path, style)),
                    }
                }

                previous = Some((point, fix));
            }
        }

        for (path, style) in shaded {
            frame.frame().stroke(&path, stroke(style));
        }
    }
}

/// The colouring of a segment from `from` to `to`, each a point on screen
/// and its altitude: the colour of the first altitude at `from`, shading to
/// the colour of the second at `to`. A segment's ends share their colours
/// with the segments on either side, so the trail has no seams.
///
/// A gradient takes four times the vertex data of a solid colour, so a
/// segment whose ends look alike ([`INVISIBLE`]) is solid, in the colour of
/// its middle altitude: it is off by less than half a step at either end,
/// which no screen shows. Level flight comes out solid, a climb shaded.
fn segment_style(
    (from, from_alt_m): (Point, f32),
    (to, to_alt_m): (Point, f32),
    brightness: f32,
    alpha: f32,
) -> Style {
    let first = altitude_color(from_alt_m, brightness).scale_alpha(alpha);
    let second = altitude_color(to_alt_m, brightness).scale_alpha(alpha);
    if difference(first, second) < INVISIBLE {
        let middle = altitude_color((from_alt_m + to_alt_m) / 2.0, brightness);
        return Style::Solid(middle.scale_alpha(alpha));
    }

    Style::Gradient(
        gradient::Linear::new(from, to)
            .add_stop(0.0, first)
            .add_stop(1.0, second)
            .into(),
    )
}

/// The largest difference between two colours' channels, alpha aside.
fn difference(a: Color, b: Color) -> f32 {
    [a.r - b.r, a.g - b.g, a.b - b.b]
        .into_iter()
        .fold(0.0, |most, channel| channel.abs().max(most))
}

/// The trail's colour at `altitude_m`: yellow → green → blue → purple → red
/// from the ground to [`TOP_ALT_M`], continuously.
fn altitude_color(altitude_m: f32, brightness: f32) -> Color {
    const YELLOW_HUE_DEG: f32 = 90.0;
    const RED_HUE_DEG: f32 = 380.0;

    let t = (altitude_m / TOP_ALT_M).clamp(0.0, 1.0);
    let hue_deg = YELLOW_HUE_DEG + t * (RED_HUE_DEG - YELLOW_HUE_DEG);

    Color::from(Oklch {
        l: brightness,
        c: 0.2 * brightness,
        h: hue_deg.to_radians(),
        a: 1.0,
    })
}

/// Each vehicle's home position.
struct Homes<'a> {
    fleet: &'a FleetSnapshot,
    selected: Option<SystemId>,
}

impl MapLayer<Message> for Homes<'_> {
    fn id(&self) -> LayerId {
        LayerId::new("vehicles.homes")
    }

    fn z_index(&self) -> i32 {
        5
    }

    fn revision(&self) -> Option<u64> {
        let mut hasher = DefaultHasher::new();
        self.selected.hash(&mut hasher);
        for vehicle in self.fleet.vehicles() {
            if let Some(home) = vehicle.home() {
                (
                    vehicle.system,
                    home.position.lat.to_bits(),
                    home.position.lon.to_bits(),
                )
                    .hash(&mut hasher);
            }
        }
        Some(hasher.finish())
    }

    fn draw(&self, frame: &mut MapFrame<'_>, _interaction: Interaction) {
        for vehicle in self.fleet.vehicles() {
            let Some(home) = vehicle.home() else { continue };
            let selected = self.selected == Some(vehicle.system);
            let alpha = if selected { 1.0 } else { 0.6 };

            frame.circle(
                home.position,
                9.0,
                crate::ui::style::map_home().scale_alpha(alpha),
            );
            frame.label(home.position, "H", 12.0, crate::ui::style::marker_label());
        }
    }
}

/// A heading arrow per vehicle; clicking one selects it.
struct Icons<'a> {
    fleet: &'a FleetSnapshot,
    selected: Option<SystemId>,
    palette: Palette,
}

impl Icons<'_> {
    fn handle(system: SystemId) -> HandleId {
        HandleId(u64::from(system.0))
    }
}

impl MapLayer<Message> for Icons<'_> {
    fn id(&self) -> LayerId {
        LayerId::new("vehicles.icons")
    }

    fn z_index(&self) -> i32 {
        20
    }

    fn draw(&self, frame: &mut MapFrame<'_>, interaction: Interaction) {
        for vehicle in self.fleet.vehicles() {
            let Some(position) = &vehicle.position.current else {
                continue;
            };
            let center = frame.view().to_screen(position.pos);
            let yaw = heading(vehicle);

            let outline_width = if self.selected == Some(vehicle.system)
                || interaction.is_active(Self::handle(vehicle.system))
            {
                10.0
            } else {
                4.0
            };

            let palette = self.palette;
            frame.frame().with_save(|frame| {
                attitude::draw_arrow_head(frame, center, ICON_SIZE, yaw, &palette, outline_width);
            });

            let below = frame
                .view()
                .to_geo(Point::new(center.x, center.y + ICON_SIZE * 0.8));
            frame.label(below, vehicle.system.0.to_string(), 12.0, Color::WHITE);
        }
    }

    fn handles(&self, _view: &MapView) -> Vec<Handle> {
        self.fleet
            .vehicles()
            .filter_map(|vehicle| {
                let position = vehicle.position.current.as_ref()?;
                Some(Handle::at(
                    Self::handle(vehicle.system),
                    position.pos,
                    ICON_SIZE * 0.6,
                ))
            })
            .collect()
    }

    fn on_event(&self, event: &LayerEvent) -> Option<Message> {
        match event {
            LayerEvent::Click {
                handle: Some(HandleId(system)),
                ..
            } => Some(Message::Select(SystemId(*system as u8))),
            _ => None,
        }
    }
}

/// The vehicle's heading in radians: its attitude yaw, else its course over
/// ground.
fn heading(vehicle: &Vehicle) -> f32 {
    if let Some(attitude) = &vehicle.attitude.current {
        return attitude.euler_angles().2;
    }

    vehicle
        .position
        .velocity_ned
        .filter(|velocity| velocity.x.hypot(velocity.y) > 0.5)
        .map(|velocity| velocity.y.atan2(velocity.x))
        .unwrap_or_default()
}

/// The selected vehicle's attitude indicator, in the overlay's corner.
struct Hud<'a> {
    vehicle: &'a Vehicle,
    palette: Palette,
}

impl MapLayer<Message> for Hud<'_> {
    fn id(&self) -> LayerId {
        LayerId::new("vehicles.hud")
    }

    fn z_index(&self) -> i32 {
        100
    }

    fn draw(&self, frame: &mut MapFrame<'_>, _interaction: Interaction) {
        if let Some(current) = &self.vehicle.attitude.current {
            let (roll, pitch, yaw) = current.euler_angles();
            let area = frame.view().visible();
            attitude::overlay(frame.frame(), area, roll, pitch, yaw, &self.palette);
        }
    }

    /// The instrument itself is a button: clicking it brings the vehicle
    /// into view.
    fn handles(&self, view: &MapView) -> Vec<Handle> {
        if self.vehicle.attitude.current.is_none() {
            return Vec::new();
        }

        let (center, radius) = attitude::placement(view.visible());
        vec![Handle {
            id: HandleId(0),
            anchor: Anchor::Screen(center),
            radius,
            draggable: false,
        }]
    }

    fn on_event(&self, event: &LayerEvent) -> Option<Message> {
        matches!(
            event,
            LayerEvent::Click {
                handle: Some(_),
                ..
            }
        )
        .then_some(Message::Locate(self.vehicle.system))
    }
}

#[cfg(test)]
mod tests {
    use iced::widget::canvas::Gradient;

    use super::*;

    /// The whole outline is drawn before any bright line: across body and
    /// tail (and so across the seam between them) and across vehicles.
    #[test]
    fn every_casing_is_painted_before_any_core() {
        let fleet = FleetSnapshot::default();
        let panel = super::super::VehiclePanel::default();
        let context = MapContext {
            fleet: &fleet,
            selected: None,
            palette: *iced::Theme::Dark.palette(),
        };

        let mut layers = LayerSet::new();
        panel.map_layers(context, &mut layers);
        let order: Vec<&str> = layers
            .into_sorted()
            .iter()
            .map(|layer| layer.id().name())
            .collect();

        let position = |id: &str| order.iter().position(|name| *name == id).unwrap();
        let last_casing =
            position("vehicles.trail-casing").max(position("vehicles.trail-casing-tail"));
        let first_core = position("vehicles.trail-core").min(position("vehicles.trail-core-tail"));
        assert!(last_casing < first_core, "{order:?}");
        assert!(
            first_core < position("vehicles.homes"),
            "trails stay below markers"
        );
    }

    /// No metre of altitude changes the colour by much, in either pass's
    /// brightness. Altitude bands, the trail's colouring before, jumped by
    /// 0.1 at a typical band edge and up to 0.3. The steepest metre left is
    /// about 0.025, near 270 m in the core's brightness, where the scale
    /// leaves what sRGB can show and a channel clips to zero.
    #[test]
    fn the_colour_follows_the_altitude_without_steps() {
        for pass in [TrailPass::Casing, TrailPass::Core] {
            let (_, brightness) = pass.style();
            let at = |altitude_m| altitude_color(altitude_m, brightness);
            assert_eq!(at(-10.0), at(0.0), "below the ground is the ground");
            assert_eq!(at(10_000.0), at(TOP_ALT_M), "above the top is the top");
            assert_ne!(at(0.0), at(TOP_ALT_M));

            let steepest = (0..1000)
                .map(|metres| metres as f32)
                .map(|altitude_m| difference(at(altitude_m), at(altitude_m + 1.0)))
                .fold(0.0, f32::max);
            assert!(steepest < 0.03, "{steepest} in a metre at {brightness}");
        }
    }

    /// A segment's gradient runs the way the vehicle flew: from its first
    /// fix's colour at its first point to its second's at its second.
    #[test]
    fn a_segment_shades_from_its_first_fix_to_its_second() {
        let (from, to) = (Point::new(10.0, 20.0), Point::new(30.0, 5.0));

        let Style::Gradient(Gradient::Linear(linear)) =
            segment_style((from, 0.0), (to, 600.0), 0.85, 0.25)
        else {
            panic!("a climb is shaded");
        };

        assert_eq!((linear.start, linear.end), (from, to));
        let stops: Vec<_> = linear.stops.iter().flatten().collect();
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].offset, 0.0);
        assert_eq!(stops[0].color, altitude_color(0.0, 0.85).scale_alpha(0.25));
        assert_eq!(stops[1].offset, 1.0);
        assert_eq!(
            stops[1].color,
            altitude_color(600.0, 0.85).scale_alpha(0.25)
        );
    }

    /// A segment flown level is solid, in a colour no screen tells apart
    /// from either end's, so it joins its neighbours without a seam.
    #[test]
    fn a_level_segment_is_solid_and_seamless() {
        let (from, to) = (Point::new(10.0, 20.0), Point::new(30.0, 5.0));
        for (first_m, second_m) in [(120.0, 120.0), (120.0, 120.3), (271.0, 271.1)] {
            let Style::Solid(color) = segment_style((from, first_m), (to, second_m), 0.85, 1.0)
            else {
                panic!("{first_m} to {second_m} m is level");
            };
            for end_m in [first_m, second_m] {
                let off = difference(color, altitude_color(end_m, 0.85));
                assert!(off < INVISIBLE / 2.0, "{off} from the end at {end_m} m");
            }
        }
    }
}
