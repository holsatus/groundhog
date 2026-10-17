//! The map: OpenStreetMap tiles through `walkers`, with the vehicles' and
//! the mission's overlays on top.
//!
//! The engine knows nothing about which feature contributes what beyond what
//! the application hands in per frame: the fleet snapshot, the mission plan
//! the panel shows, and where interactions go. Interactions (selecting a
//! vehicle by clicking its icon, dragging a mission marker, clicking while
//! placing) surface as [`MapIntent`]s the application applies to the owning
//! feature.
//!
//! Overlay interactions ride on egui's own interaction machinery
//! (`ui.interact` over the map widget's rect), so marker dragging depends on
//! egui's drag ownership resolving to the marker rather than the map's pan;
//! vehicle selection, marker clicks and placement only need clicks, which
//! are not exclusive.

use egui::{Align2, Color32, CursorIcon, FontId, Id, Pos2, Sense, Shape, Stroke, Ui, Vec2};
use walkers::{
    HttpTiles, Map as MapWidget, MapMemory, Plugin, Position as MapPosition, Projector,
    sources::OpenStreetMap,
};

use groundhog_logic::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        geo::GeoPosition,
        mission::{ItemId, MissionItem, MissionPlan},
        vehicle::Vehicle,
        vehicle::position::GlobalPosition,
    },
};

use crate::style;

/// Where the iced frontend starts: a fixed, familiar view.
const INITIAL_VIEW: GeoPosition = GeoPosition {
    lat: 48.8566,
    lon: 2.3522,
};
const INITIAL_ZOOM: f64 = 8.0;

/// A gap between fixes longer than this is drawn faint: the track broke.
const GAP_SECS: f32 = 30.0;
/// Screen-space hit radius of a mission item marker.
const MARKER_HIT_RADIUS: f32 = 14.0;
/// Vehicle icon size, in points.
const ICON_SIZE: f32 = 40.0;

/// An interaction the map observed the application should perform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapIntent {
    /// A vehicle icon was clicked.
    SelectVehicle(SystemId),
    /// A mission marker was clicked.
    MarkerClick { item: ItemId },
    /// A mission marker was dragged at `at`, live.
    MarkerAt { item: ItemId, at: GeoPosition },
    /// The empty map was clicked while the mission places waypoints.
    PlaceAt(GeoPosition),
    /// The attitude HUD was clicked: bring the vehicle into view.
    LocateVehicle,
}

/// A [lon, lat] pair, in walkers' own order.
fn at(position: GeoPosition) -> MapPosition {
    MapPosition::new(position.lon, position.lat)
}

/// A screen position, unprojected into geographic degrees.
fn geo_of(position: MapPosition) -> GeoPosition {
    GeoPosition {
        lat: position.y(),
        lon: position.x(),
    }
}

/// The map's own state: its tiles, and where it looks.
pub struct Map {
    tiles: Option<HttpTiles>,
    memory: MapMemory,
    follow: bool,
}

impl Map {
    /// Start the map with `ctx`'s tile downloads and the fixed initial view.
    pub fn new(ctx: egui::Context) -> Self {
        let mut memory = MapMemory::default();
        memory
            .set_zoom(INITIAL_ZOOM)
            .expect("the initial zoom is inside walkers' range");
        memory.center_at(at(INITIAL_VIEW));

        Self {
            tiles: Some(HttpTiles::new(OpenStreetMap, ctx)),
            memory,
            follow: false,
        }
    }

    /// Whether the map is following its target position.
    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Keep the view centred on positions fed through [`Map::follow_to`],
    /// until the operator moves the map.
    pub fn set_following(&mut self, follow: bool) {
        self.follow = follow;
    }

    /// Centre the view on `position`, stopping to follow whatever came
    /// before.
    pub fn center_on(&mut self, position: GeoPosition) {
        self.follow = false;
        self.memory.center_at(at(position));
    }

    /// Centre the view on `position` if following; the follow target comes
    /// from the selection anyway.
    pub fn follow_to(&mut self, position: GeoPosition) {
        if self.follow {
            self.memory.center_at(at(position));
        }
    }

    /// Show the map with its overlays inside `ui`.
    ///
    /// All borrowed data shares the `'m` lifetime so the plugins can hold it
    /// together.
    #[allow(clippy::too_many_arguments)]
    pub fn show<'m>(
        &mut self,
        ui: &mut Ui,
        fleet: &'m FleetSnapshot,
        selected: Option<SystemId>,
        mission: Option<&'m MissionPlan>,
        mission_selected: Option<ItemId>,
        mission_current: Option<usize>,
        mission_editable: bool,
        mission_placing: bool,
    ) -> Vec<MapIntent> {
        // While following, the widget's `my_position` argument centres the
        // view. When not following, make sure the memory is detached at the
        // current follow target so it stays where the operator left it.
        let target = follow_target(fleet, selected);

        if self.follow {
            self.memory.follow_my_position();
        } else if self.memory.detached().is_none() {
            self.memory.center_at(target.unwrap_or(at(INITIAL_VIEW)));
        }

        let my_position = target.unwrap_or(at(INITIAL_VIEW));

        // The plugins report their interactions over one channel, drained
        // when the frame is done.
        let (intents, mut intents_rx) = tokio::sync::mpsc::unbounded_channel();

        let tiles: Option<&mut dyn walkers::Tiles> = self
            .tiles
            .as_mut()
            .map(|tiles| tiles as &mut dyn walkers::Tiles);

        let mut map = MapWidget::new(tiles, &mut self.memory, my_position)
            .zoom_with_ctrl(false)
            .panning(false);

        // Trails are drawn twice: a dark, wide casing pass, then the bright
        // cores, so outlines never cut across cores (as the iced map does).
        map = map.with_plugin(Trails {
            fleet,
            casing: true,
        });
        map = map.with_plugin(Trails {
            fleet,
            casing: false,
        });
        map = map.with_plugin(Homes { fleet, selected });
        map = map.with_plugin(Icons {
            fleet,
            selected,
            intents: &intents,
        });
        if let Some(plan) = mission {
            map = map.with_plugin(MissionRoute { plan });
            map = map.with_plugin(MissionMarkers {
                plan,
                selected_item: mission_selected,
                current: mission_current,
                editable: mission_editable,
                placing: mission_placing,
                intents: &intents,
            });
        }
        if let Some(vehicle) = selected.and_then(|id| fleet.vehicle(id)) {
            map = map.with_plugin(Hud {
                vehicle,
                intents: &intents,
            });
        }

        map.show(ui, |_ui, _response, _projector, _map_memory| {
            // The overlays run as plugins; nothing else draws here.
        });

        // The operator taking the map by hand ends following; the widget
        // reports detached state once a drag starts.
        if self.follow && self.memory.detached().is_some() {
            self.follow = false;
        }

        let mut collected = Vec::new();
        while let Ok(intent) = intents_rx.try_recv() {
            collected.push(intent);
        }
        collected
    }
}

/// Where the map centres while following: the selected vehicle's last fix.
fn follow_target(fleet: &FleetSnapshot, selected: Option<SystemId>) -> Option<MapPosition> {
    selected
        .and_then(|id| fleet.vehicle(id))
        .and_then(|vehicle| vehicle.position.current.as_ref())
        .map(|position| at(position.pos))
}

/// Where each vehicle has been, coloured by altitude, split into runs.
///
/// A run is a contiguous polyline of fixes with one band and one gap-marker;
/// the whole outline is drawn before any bright line, across runs and
/// vehicles, so the two passes of [`Trails`] produce the iced map's casing
/// and core layers.
struct Trails<'m> {
    fleet: &'m FleetSnapshot,
    casing: bool,
}

impl Trails<'_> {
    fn style(&self) -> (f32, f32) {
        if self.casing {
            (8.0, style::map::TRAIL_CASE_BRIGHTNESS)
        } else {
            (3.0, style::map::TRAIL_CORE_BRIGHTNESS)
        }
    }
}

impl Plugin for Trails<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        _response: &egui::Response,
        projector: &Projector,
        _memory: &MapMemory,
    ) {
        let (width, brightness) = self.style();
        let clip = ui.painter().clip_rect();

        for vehicle in self.fleet.vehicles() {
            for (band, faint, run) in trail_runs(vehicle) {
                if run.len() < 2 {
                    continue;
                }

                let screen: Vec<Pos2> = run
                    .iter()
                    .map(|fix| projector.project(at(fix.pos)).to_pos2())
                    .collect();
                if screen.iter().all(|point| !clip.contains(*point)) {
                    continue;
                }

                let mut color = style::map::band_color(band, brightness);
                if faint {
                    color = color.gamma_multiply(0.25);
                }
                ui.painter()
                    .add(Shape::line(screen, Stroke::new(width, color)));
            }
        }
    }
}

/// One altitude band and whether the run crosses a gap.
type TrailRun = (usize, bool, Vec<GlobalPosition>);

/// The (band, faint, fixes) runs of one vehicle's trail, oldest first. A run
/// becomes one polyline stroke, so a stroke never changes band mid-air.
fn trail_runs(vehicle: &Vehicle) -> Vec<TrailRun> {
    let mut runs: Vec<TrailRun> = Vec::new();
    let trail = &vehicle.position.trail;
    let mut previous: Option<&GlobalPosition> = None;

    for fix in trail.iter() {
        let Some(last) = previous else {
            previous = Some(fix);
            continue;
        };

        let band = style::map::band_of((last.alt_agl + fix.alt_agl) / 2.0);
        let faint = fix.at.saturating_duration_since(last.at).as_secs_f32() > GAP_SECS;

        match runs.last_mut() {
            Some((run_band, run_faint, fixes)) if *run_band == band && *run_faint == faint => {
                fixes.push(fix.clone());
            }
            _ => runs.push((band, faint, vec![last.clone(), fix.clone()])),
        }

        previous = Some(fix);
    }

    runs
}

/// Each vehicle's home position: a marker with an `H`.
struct Homes<'m> {
    fleet: &'m FleetSnapshot,
    selected: Option<SystemId>,
}

impl Plugin for Homes<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        _response: &egui::Response,
        projector: &Projector,
        _memory: &MapMemory,
    ) {
        for vehicle in self.fleet.vehicles() {
            let Some(home) = vehicle.home() else { continue };
            let alpha = if self.selected == Some(vehicle.system) {
                1.0
            } else {
                0.6
            };
            let centre = projector.project(at(home.position)).to_pos2();
            let color = style::map::HOME.gamma_multiply(alpha);

            ui.painter().circle_filled(centre, 9.0, color);
            ui.painter().text(
                centre,
                Align2::CENTER_CENTER,
                "H",
                FontId::proportional(11.0),
                style::map::LABEL,
            );
        }
    }
}

/// A heading arrow per vehicle; clicking one selects it.
struct Icons<'m> {
    fleet: &'m FleetSnapshot,
    selected: Option<SystemId>,
    intents: &'m tokio::sync::mpsc::UnboundedSender<MapIntent>,
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

impl Plugin for Icons<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        response: &egui::Response,
        projector: &Projector,
        _memory: &MapMemory,
    ) {
        let mut nearest: Option<(f32, SystemId)> = None;

        for vehicle in self.fleet.vehicles() {
            let Some(position) = &vehicle.position.current else {
                continue;
            };

            let centre = projector.project(at(position.pos)).to_pos2();
            if !ui.painter().clip_rect().expand(ICON_SIZE).contains(centre) {
                continue;
            }

            let yaw = heading(vehicle);
            draw_vehicle_arrow(ui, centre, ICON_SIZE, yaw);

            if self.selected == Some(vehicle.system) {
                ui.painter().circle_stroke(
                    centre,
                    ICON_SIZE * 0.7,
                    Stroke::new(2.0, style::map::SELECTED),
                );
            }

            ui.painter().text(
                centre + egui::vec2(0.0, ICON_SIZE * 0.9),
                Align2::CENTER_TOP,
                vehicle.system.0.to_string(),
                FontId::monospace(12.0),
                style::map::LABEL,
            );

            if response.hovered() {
                let distance = (response.hover_pos().unwrap_or(centre) - centre).length();
                if distance <= ICON_SIZE * 0.7 && nearest.is_none_or(|(best, _)| distance < best) {
                    nearest = Some((distance, vehicle.system));
                }
            }
        }

        if let Some((_, system)) = nearest {
            ui.output_mut(|output| output.cursor_icon = CursorIcon::PointingHand);
            if response.clicked() {
                let _ = self.intents.send(MapIntent::SelectVehicle(system));
            }
        }
    }
}

/// The nose arrow: a two-tone dart pointing at `yaw` (radians, east from
/// north), rotated clockwise.
fn draw_vehicle_arrow(ui: &Ui, centre: Pos2, width: f32, yaw: f32) {
    let half = width / 2.0;
    let rotate = |point: (f32, f32)| -> Pos2 {
        let (sin, cos) = yaw.sin_cos();
        Pos2::new(
            centre.x + point.0 * cos - point.1 * sin,
            centre.y + point.0 * sin + point.1 * cos,
        )
    };

    let tip = rotate((0.0, -half));
    let notch = rotate((0.0, half / 2.0));
    let right = rotate((half, half));
    let left = rotate((-half, half));

    // The outline, so the arrow reads against both light and dark tiles.
    ui.painter().add(Shape::line(
        vec![tip, right, notch, left, tip],
        Stroke::new(6.0, style::map::OUTLINE),
    ));

    ui.painter().add(Shape::convex_polygon(
        vec![tip, right, notch],
        style::map::ARROW_RIGHT,
        Stroke::NONE,
    ));
    ui.painter().add(Shape::convex_polygon(
        vec![tip, left, notch],
        style::map::ARROW_LEFT,
        Stroke::NONE,
    ));
}

/// The dashed line through the positioned items.
struct MissionRoute<'m> {
    plan: &'m MissionPlan,
}

impl Plugin for MissionRoute<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        _response: &egui::Response,
        projector: &Projector,
        _memory: &MapMemory,
    ) {
        let points: Vec<Pos2> = self
            .plan
            .route()
            .map(|(_, _, position)| {
                projector
                    .project(at(GeoPosition {
                        lat: position.lat,
                        lon: position.lon,
                    }))
                    .to_pos2()
            })
            .collect();

        if points.len() < 2 {
            return;
        }

        let stroke = Stroke::new(2.0, style::map::ROUTE);
        for pair in points.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            for (dash_from, dash_to) in dashed(from, to, 15.0, 15.0) {
                ui.painter()
                    .add(Shape::line_segment([dash_from, dash_to], stroke));
            }
        }
    }
}

/// The dashed subsections of one segment: dash and gap lengths alternate.
fn dashed(from: Pos2, to: Pos2, dash: f32, gap: f32) -> Vec<(Pos2, Pos2)> {
    let length = (to - from).length();
    if length < dash || length <= 0.0 {
        return vec![(from, to)];
    }

    let direction = (to - from) / length;
    let mut dashes = Vec::new();
    let mut travelled = 0.0;
    while travelled < length {
        let dash_end = (travelled + dash).min(length);
        let start = from + direction * travelled;
        let end = from + direction * dash_end;
        dashes.push((start, end));
        travelled = dash_end + gap;
    }
    dashes
}

/// A numbered marker per positioned item; dragging one moves the item,
/// clicking one selects it; the empty map places waypoints while placing.
struct MissionMarkers<'m> {
    plan: &'m MissionPlan,
    selected_item: Option<ItemId>,
    /// The plan index the vehicle executes, when the plan is synced.
    current: Option<usize>,
    /// Whether the draft accepts edits (no transfer running).
    editable: bool,
    placing: bool,
    intents: &'m tokio::sync::mpsc::UnboundedSender<MapIntent>,
}

/// The loiter radius of an item, in metres, if it loiters.
fn loiter_radius(item: &MissionItem) -> Option<f32> {
    use mavio::default_dialect::enums::MavCmd;
    let slot = match item.command {
        MavCmd::NavLoiterUnlim | MavCmd::NavLoiterTurns | MavCmd::NavLoiterTime => 2,
        MavCmd::NavLoiterToAlt => 1,
        _ => return None,
    };
    let radius = item.params[slot].abs();
    (radius.is_finite() && radius > 0.0).then_some(radius)
}

impl Plugin for MissionMarkers<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        response: &egui::Response,
        projector: &Projector,
        _memory: &MapMemory,
    ) {
        let mut consumed = false;

        // The loiter circles, to scale: the airspace a loitering vehicle
        // sweeps.
        for (_, id, position) in self.plan.route() {
            let Some(item) = self.plan.get(id) else {
                continue;
            };
            let Some(radius_m) = loiter_radius(item) else {
                continue;
            };
            let centre_geo = GeoPosition::new(position.lat, position.lon);
            let radius_px = radius_m * projector.scale_pixel_per_meter(at(centre_geo));
            if radius_px > 3.0 {
                ui.painter().circle_stroke(
                    projector.project(at(centre_geo)).to_pos2(),
                    radius_px,
                    Stroke::new(1.5, style::map::ROUTE.gamma_multiply(0.6)),
                );
            }
        }

        for (index, id, position) in self.plan.route() {
            let at_screen = projector
                .project(at(GeoPosition {
                    lat: position.lat,
                    lon: position.lon,
                }))
                .to_pos2();
            if !ui
                .painter()
                .clip_rect()
                .expand(MARKER_HIT_RADIUS * 2.0)
                .contains(at_screen)
            {
                continue;
            }

            // Hit area, with a cursor fitting the gesture.
            let hit_rect =
                egui::Rect::from_center_size(at_screen, Vec2::splat(MARKER_HIT_RADIUS * 2.0));
            let marker_response = ui.interact(
                hit_rect,
                Id::new(("map.mission.marker", id.raw())),
                Sense::click_and_drag(),
            );

            if marker_response.hovered() {
                ui.output_mut(|output| {
                    output.cursor_icon = if self.editable {
                        CursorIcon::Grab
                    } else {
                        CursorIcon::Grabbing
                    }
                });
            }

            if marker_response.clicked() {
                let _ = self.intents.send(MapIntent::MarkerClick { item: id });
                consumed = true;
            }

            if self.editable
                && marker_response.dragged()
                && let Some(pointer) = marker_response.interact_pointer_pos()
            {
                ui.output_mut(|output| output.cursor_icon = CursorIcon::Grabbing);
                let _ = self.intents.send(MapIntent::MarkerAt {
                    item: id,
                    at: geo_of(projector.unproject(pointer.to_vec2())),
                });
            }

            let marker_color = if marker_response.dragged() {
                style::map::MARKER_DRAGGED
            } else if self.current == Some(index) {
                style::map::MARKER_CURRENT
            } else if self.selected_item == Some(id) || marker_response.hovered() {
                style::map::MARKER_SELECTED
            } else {
                style::map::MARKER_DEFAULT
            };
            let marker_radius = if marker_response.dragged() {
                12.0
            } else if self.selected_item == Some(id)
                || self.current == Some(index)
                || marker_response.hovered()
            {
                10.0
            } else {
                8.0
            };

            ui.painter()
                .circle_filled(at_screen, marker_radius, marker_color);
            ui.painter().text(
                at_screen,
                Align2::CENTER_CENTER,
                (index + 1).to_string(),
                FontId::proportional(12.0),
                style::map::OUTLINE,
            );
        }

        // The empty map places a waypoint while placing is active, unless a
        // marker took the click.
        if self.placing
            && !consumed
            && response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            // The receiver drains at the end of the frame, so the send
            // cannot fail.
            let _ = self.intents.send(MapIntent::PlaceAt(geo_of(
                projector.unproject(pointer.to_vec2()),
            )));
        }
    }
}

/// The selected vehicle's attitude indicator, anchored to the top-right of
/// the map pane. Clicking it brings the vehicle into view.
struct Hud<'m> {
    vehicle: &'m Vehicle,
    intents: &'m tokio::sync::mpsc::UnboundedSender<MapIntent>,
}

/// Inset of the overlay from the edges, in points.
const HUD_MARGIN: f32 = 16.0;
/// Largest compass dial radius, in points.
const HUD_MAX_RADIUS: f32 = 64.0;
/// Smallest radius: below it the cardinals stop being legible.
const HUD_MIN_RADIUS: f32 = 24.0;
/// How far the instrument's arcs reach, in dial radii.
const HUD_REACH: f32 = 1.0 + 0.1 + 0.2;
const MAX_ATTITUDE_DEG: f32 = 40.0;

impl Plugin for Hud<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        _response: &egui::Response,
        _projector: &Projector,
        _memory: &MapMemory,
    ) {
        let Some(current) = &self.vehicle.attitude.current else {
            return;
        };
        let (roll, pitch, yaw) = current.attitude.euler_angles();

        let size = ui.max_rect().size();
        let fit = (size.x.min(size.y) / 2.0 - HUD_MARGIN) / HUD_REACH;
        let radius = fit.clamp(HUD_MIN_RADIUS, HUD_MAX_RADIUS);
        let reach = radius * HUD_REACH;
        let centre = Pos2::new(
            ui.max_rect().right() - HUD_MARGIN - reach,
            ui.max_rect().top() + HUD_MARGIN + reach,
        );

        draw_attitude_indicator(ui, centre, radius, roll, pitch, yaw);

        // The instrument is a button: clicking it centres the vehicle.
        let hit_rect = egui::Rect::from_center_size(centre, Vec2::splat(reach * 2.0));
        let instrument = ui.interact(hit_rect, Id::new("map.attitude"), Sense::click());
        if instrument.clicked() {
            let _ = self.intents.send(MapIntent::LocateVehicle);
        }
    }
}

/// The QGC-style instrument: fixed compass card, nose arrow for heading, roll
/// arc above and pitch arc to the right.
fn draw_attitude_indicator(ui: &Ui, centre: Pos2, radius: f32, roll: f32, pitch: f32, yaw: f32) {
    let painter = ui.painter();
    let text = ui.visuals().strong_text_color();

    // The gauge arcs run outside the dial, so they are drawn first.
    attitude_arc(painter, centre, radius, 0.0, roll.to_degrees(), text);
    attitude_arc(painter, centre, radius, 90.0, -pitch.to_degrees(), text);

    // The opaque dial, its border, the tick ring and the four labels.
    painter.circle_filled(centre, radius, style::map::HUD_SURFACE);
    painter.circle_stroke(
        centre,
        radius,
        Stroke::new(1.0, style::map::OUTLINE.gamma_multiply(2.0)),
    );

    let ring = radius * 0.94;
    for step in 0..8 {
        let bearing = 45.0 * step as f32;
        let length = if step % 2 == 0 {
            radius * 0.05
        } else {
            radius * 0.15
        };

        painter.add(Shape::line_segment(
            [
                bearing_point(centre, ring - length, bearing),
                bearing_point(centre, ring, bearing),
            ],
            Stroke::new(2.0, text),
        ));
    }

    // Cardinals in two colour pairs, as the iced instrument showed.
    let (cool, warm) = (style::map::SELECTED, style::severity::WARNING);
    for (bearing, label, color) in [
        (0.0, "N", cool),
        (90.0, "E", cool),
        (180.0, "S", warm),
        (270.0, "W", warm),
    ] {
        painter.text(
            bearing_point(centre, radius * 0.72, bearing),
            Align2::CENTER_CENTER,
            label,
            FontId::monospace(radius * 0.26),
            color,
        );
    }

    // The nose arrow: a dart two thirds the dial size.
    draw_arrow_dart(ui, centre, radius * 2.0 / 3.0, yaw);
}

/// One roll/pitch gauge over the dial: a band spanning ±40°, a fill through
/// the current value and a zero tick.
fn attitude_arc(
    painter: &egui::Painter,
    centre: Pos2,
    dial_radius: f32,
    zero_bearing: f32,
    sweep: f32,
    text: Color32,
) {
    let width = dial_radius * 0.2;
    let radius = dial_radius * 1.1 + width / 2.0;

    let guarded = sweep.clamp(-MAX_ATTITUDE_DEG, MAX_ATTITUDE_DEG);

    // The background band.
    for pair in arc_points(
        centre,
        radius,
        zero_bearing - MAX_ATTITUDE_DEG,
        zero_bearing + MAX_ATTITUDE_DEG,
    )
    .windows(2)
    {
        painter.add(Shape::line_segment(
            [pair[0], pair[1]],
            Stroke::new(width, style::map::HUD_SURFACE.gamma_multiply(1.4)),
        ));
    }

    // The fill, from zero to `guarded`.
    if guarded.abs() > 0.5 {
        let (from, to) = if guarded >= 0.0 {
            (zero_bearing, zero_bearing + guarded)
        } else {
            (zero_bearing + guarded, zero_bearing)
        };
        for pair in arc_points(centre, radius, from, to).windows(2) {
            painter.add(Shape::line_segment(
                [pair[0], pair[1]],
                Stroke::new(width, style::map::SELECTED),
            ));
        }
    }

    // The zero tick spanning the band.
    painter.add(Shape::line_segment(
        [
            bearing_point(centre, radius - width / 2.0, zero_bearing),
            bearing_point(centre, radius + width / 2.0, zero_bearing),
        ],
        Stroke::new(2.0, text),
    ));
}

/// A point on a circle at `bearing` degrees clockwise from up.
fn bearing_point(centre: Pos2, radius: f32, bearing: f32) -> Pos2 {
    let radians = bearing.to_radians();
    Pos2::new(
        centre.x + radius * radians.sin(),
        centre.y - radius * radians.cos(),
    )
}

/// A circular arc's points, for `from..to` bearings clockwise from up.
fn arc_points(centre: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let steps = 24;
    (0..=steps)
        .map(|step| {
            let bearing = from + (to - from) * step as f32 / steps as f32;
            bearing_point(centre, radius, bearing)
        })
        .collect()
}

/// The nose arrow inside the dial: a dart rotated clockwise from north, the
/// right half lit, the left shaded.
fn draw_arrow_dart(ui: &Ui, centre: Pos2, width: f32, yaw: f32) {
    let painter = ui.painter();
    let half = width / 2.0;

    let rotate = |point: (f32, f32)| -> Pos2 {
        let (sin, cos) = yaw.sin_cos();
        Pos2::new(
            centre.x + point.0 * cos - point.1 * sin,
            centre.y + point.0 * sin + point.1 * cos,
        )
    };

    let tip = rotate((0.0, -half));
    let notch = rotate((0.0, half / 2.0));
    let right = rotate((half, half));
    let left = rotate((-half, half));

    painter.add(Shape::convex_polygon(
        vec![tip, right, notch],
        style::map::ARROW_RIGHT,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![tip, left, notch],
        style::map::ARROW_LEFT,
        Stroke::NONE,
    ));
}
