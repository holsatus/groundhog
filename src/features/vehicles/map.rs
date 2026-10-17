//! The vehicles' map layers: trails, home markers, vehicle icons and the
//! attitude HUD of the selected vehicle.

use std::{
    collections::BTreeMap,
    hash::{DefaultHasher, Hash, Hasher},
    time::Duration,
};

use iced::{
    Color, Point,
    color::Oklch,
    theme::Palette,
    widget::canvas::{LineCap, LineJoin, Path, Stroke, Style},
};

use super::Message;
use crate::{
    domain::{
        SystemId,
        fleet::FleetSnapshot,
        vehicle::{Vehicle, position::GlobalPosition},
    },
    features::MapContext,
    map::{
        Anchor, Handle, HandleId, Interaction, LayerEvent, LayerId, LayerSet, MapFrame, MapLayer,
        MapPlugin, MapView,
    },
    widget::attitude,
};

/// Trail points closer than this on screen are merged.
const DECIMATE_PX: f32 = 1.5;
/// A gap between fixes longer than this is drawn faint: the track broke.
const GAP: Duration = Duration::from_secs(30);
/// Altitude bands of the trail colouring, each one stroke per pass.
const BANDS: usize = 24;
const BAND_MAX_ALT_M: f32 = 1000.0;
const ICON_SIZE: f32 = 40.0;

impl<'a> MapPlugin<'a, MapContext<'a>> for super::VehiclePanel {
    type Message = Message;

    fn layers(&'a self, context: MapContext<'a>, layers: &mut LayerSet<'a, Message>) {
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

/// Where each vehicle has been, coloured by altitude.
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
        // Segments grouped by (band, faint), so a whole trail takes a few
        // strokes rather than one per segment.
        let mut groups: BTreeMap<(usize, bool), Vec<(Point, Point)>> = BTreeMap::new();
        let view = frame.view().clone();

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

                let faint = fix.at.saturating_duration_since(last_fix.at) > GAP;
                let visible = view.contains(last_point, 50.0) || view.contains(point, 50.0);
                if visible {
                    let band = band_of((last_fix.alt_agl + fix.alt_agl) / 2.0);
                    groups
                        .entry((band, faint))
                        .or_default()
                        .push((last_point, point));
                }

                previous = Some((point, fix));
            }
        }

        let (width, brightness) = self.pass.style();
        for ((band, faint), segments) in &groups {
            let alpha = if *faint { 0.25 } else { 1.0 };
            let path = Path::new(|builder| {
                for (from, to) in segments {
                    builder.move_to(*from);
                    builder.line_to(*to);
                }
            });

            frame.frame().stroke(
                &path,
                Stroke {
                    style: Style::Solid(band_color(*band, brightness).scale_alpha(alpha)),
                    width,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                },
            );
        }
    }
}

fn band_of(altitude_m: f32) -> usize {
    let t = (altitude_m / BAND_MAX_ALT_M).clamp(0.0, 1.0);
    ((t * (BANDS - 1) as f32).round() as usize).min(BANDS - 1)
}

/// The colour of altitude band `band`: yellow → green → blue → purple → red
/// from the ground to [`BAND_MAX_ALT_M`].
fn band_color(band: usize, brightness: f32) -> Color {
    const YELLOW_HUE_DEG: f32 = 90.0;
    const RED_HUE_DEG: f32 = 380.0;

    let t = band as f32 / (BANDS - 1) as f32;
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
                crate::style::map_home().scale_alpha(alpha),
            );
            frame.label(home.position, "H", 12.0, crate::style::marker_label());
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

            if self.selected == Some(vehicle.system)
                || interaction.is_active(Self::handle(vehicle.system))
            {
                frame.frame().stroke(
                    &Path::circle(center, ICON_SIZE * 0.7),
                    Stroke {
                        style: Style::Solid(self.palette.primary.base.color),
                        width: 2.0,
                        ..Stroke::default()
                    },
                );
            }

            let palette = self.palette;
            frame.frame().with_save(|frame| {
                attitude::draw_arrow_head(frame, center, ICON_SIZE, yaw, &palette, true);
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
        return attitude.attitude.euler_angles().2;
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
            let (roll, pitch, yaw) = current.attitude.euler_angles();
            attitude::overlay(frame.frame(), roll, pitch, yaw, &self.palette);
        }
    }

    /// The instrument itself is a button: clicking it brings the vehicle
    /// into view.
    fn handles(&self, view: &MapView) -> Vec<Handle> {
        if self.vehicle.attitude.current.is_none() {
            return Vec::new();
        }

        let (center, radius) = attitude::placement(view.size());
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
        layers.plugin(&panel, context, |message| message);
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

    #[test]
    fn bands_cover_the_altitude_range() {
        assert_eq!(band_of(-10.0), 0);
        assert_eq!(band_of(0.0), 0);
        assert_eq!(band_of(BAND_MAX_ALT_M), BANDS - 1);
        assert_eq!(band_of(10_000.0), BANDS - 1);
        assert!(band_of(500.0) > band_of(100.0));
    }
}
