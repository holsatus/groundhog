//! The camera, and how the frontend places and moves it.
//!
//! The camera is the origin of the rendered world: it stays at `(0, 0, 0)`, while everything
//! else is positioned relative to it. Its actual position is kept in `f64` ECEF coordinates in
//! [`View`], as `f32` can not represent positions on the earth to better than about a meter.
//!
//! It renders into an image rather than a window (see [`crate::output`]), and moves only by
//! what the frontend sends: [`Gesture`]s, or the samples of a pose it rides along with
//! ([`crate::ride`]). It has no input devices of its own. What it looks at is published back
//! as an [`Overhead`], the same terms a frontend places it in, so a flat map and the globe can
//! hand a view back and forth.

use std::time::Instant;

use bevy::{
    camera::Hdr,
    math::{DQuat, DVec2, DVec3},
    prelude::*,
};
use tokio::sync::watch;

use crate::{
    geo,
    ride::{Fix, Orientation, Ride},
    tiles::Tiles,
};

/// How much closer to the ground one step of zoom takes the camera.
const ZOOM_STEP: f64 = 0.2;
/// Closest the camera may get to the terrain.
const MIN_CLEARANCE: f64 = 2.0;
/// Closest a ridden camera may get to the terrain: the vehicle carrying it may sit on the
/// ground, which the terrain only roughly follows.
const RIDING_CLEARANCE: f64 = 0.5;
const MAX_HEIGHT: f64 = 40_000_000.0;
/// The detail of the elevation the camera keeps clear of and measures the ground with.
const GROUND_ZOOM: u8 = 15;
/// How far below the horizon the camera looks when placed overhead: not quite straight down,
/// where it would have no heading to keep.
const OVERHEAD_PITCH_DEG: f64 = 89.0;
/// The steepest the camera looks up or down, as the cosine of its angle from the vertical.
const MAX_STEEPNESS: f64 = 0.9999;

pub(crate) struct CameraPlugin {
    pub start: Placement,
    pub focus: watch::Sender<Option<Overhead>>,
}

/// Where the camera looks from, with angles in degrees and the height in meters above sea level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartView {
    pub lat: f64,
    pub lon: f64,
    pub height: f64,
    /// Clockwise from north.
    pub heading: f64,
    /// Above the horizon.
    pub pitch: f64,
}

impl Default for StartView {
    /// Overlooking the Alps from the north.
    fn default() -> Self {
        Self {
            lat: 46.35,
            lon: 7.6,
            height: 9_000.0,
            heading: 165.0,
            pitch: -18.0,
        }
    }
}

/// Looking down on a point of the ground, north up, as a map does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overhead {
    /// The point at the centre of the picture, in degrees.
    pub lat: f64,
    pub lon: f64,
    /// The meters of ground the picture's height spans at its centre: the scale, in terms
    /// that do not depend on the picture's size or pixel density.
    pub span_m: f64,
}

/// Where to put the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    View(StartView),
    /// Overhead, kept at its scale above the ground as the elevation streams in, until the
    /// camera is moved.
    Overhead(Overhead),
}

impl Default for Placement {
    fn default() -> Self {
        Placement::View(StartView::default())
    }
}

/// A gesture on the picture that moves the camera. Distances are fractions of the picture's
/// height, rightwards and downwards, so they mean the same at any size and pixel density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// Drag the ground: the point under the pointer follows it, as near as a flat ground
    /// below the camera allows.
    Pan { dx: f64, dy: f64 },
    /// Turn the camera in place, as a head turns: a drag the picture's height turns it by the
    /// field of view.
    Look { dx: f64, dy: f64 },
    /// Move towards what the camera looks at, by steps (negative steps move away).
    Zoom { steps: f64 },
    /// Jump to a placement, ending a ride.
    Go(Placement),
}

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        let mut view = View::new(StartView::default());
        view.apply(Gesture::Go(self.start));
        app.insert_resource(view)
            .insert_resource(Focus(self.focus.clone()))
            .add_systems(
                Update,
                (move_camera, publish_focus).chain().in_set(CameraSystems),
            )
            .add_systems(PostUpdate, sync_camera.before(TransformSystems::Propagate));
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CameraSystems;

/// The camera that renders the globe for the frontend.
#[derive(Component)]
pub(crate) struct MainCamera;

/// What moves the camera besides gestures.
#[derive(Debug)]
enum Mode {
    /// Only gestures.
    Free,
    /// Kept overhead, above the ground as its elevation streams in, until a gesture moves it.
    Pinned(Overhead),
    /// Carried along with a ride; gestures do not move it.
    Riding(Box<Ride>),
}

/// Where what the camera looks at is published.
#[derive(Resource)]
struct Focus(watch::Sender<Option<Overhead>>);

/// The camera's actual position and orientation.
#[derive(Resource)]
pub(crate) struct View {
    pub position: DVec3,
    pub forward: DVec3,
    /// Vertical field of view in radians.
    pub fov: f64,
    pub aspect: f64,
    pub viewport_height: f64,
    /// Height of the terrain below the camera.
    pub ground: f64,
    mode: Mode,
    /// The camera's up while it rides, rolled with what carries it. Otherwise it is away from
    /// the earth, and the horizon level.
    rolled_up: Option<DVec3>,
    /// Where the camera was last settled, to keep it level from there as it moves.
    settled: DVec3,
}

impl View {
    fn new(start: StartView) -> Self {
        let mut view = Self {
            position: DVec3::X * geo::EARTH_RADIUS,
            forward: DVec3::Z,
            fov: 60f64.to_radians(),
            aspect: 16.0 / 9.0,
            viewport_height: 1080.0,
            ground: 0.0,
            mode: Mode::Free,
            rolled_up: None,
            settled: DVec3::X,
        };
        view.go(start);
        view
    }

    fn go(&mut self, start: StartView) {
        let lon_lat = DVec2::new(start.lon.to_radians(), start.lat.to_radians());
        let position = geo::to_ecef(lon_lat, start.height);
        let (east, north, up) = geo::enu(position);
        let (heading, pitch) = (start.heading.to_radians(), start.pitch.to_radians());
        let horizontal = north * heading.cos() + east * heading.sin();
        self.position = position;
        self.forward = (horizontal * pitch.cos() + up * pitch.sin()).normalize();
        self.settled = position;
    }

    /// Look down at `overhead`, its point raised to the ground's height there.
    fn place(&mut self, overhead: Overhead, ground: impl Fn(DVec2) -> f64) {
        let lon_lat = DVec2::new(overhead.lon.to_radians(), overhead.lat.to_radians());
        let target = geo::to_ecef(lon_lat, ground(lon_lat));
        let (_, north, up) = geo::enu(target);
        let pitch = OVERHEAD_PITCH_DEG.to_radians();
        let forward = north * pitch.cos() - up * pitch.sin();
        let distance = overhead.span_m / (2.0 * (self.fov / 2.0).tan());
        self.position = target - forward * distance;
        self.forward = forward;
        self.settled = self.position;
    }

    /// What the centre of the picture shows, and the ground the picture spans there. Looking
    /// above the horizon, the ground below the camera.
    fn focus(&self, ground: impl Fn(DVec2) -> f64) -> Overhead {
        let hit = |height: f64| hit_sphere(self.position, self.forward, geo::EARTH_RADIUS + height);
        // Where the ray meets the ground at the height below the camera, refined once with the
        // height there
        let point = hit(self.ground).map(|first| {
            let (lon_lat, _) = geo::from_ecef(first);
            hit(ground(lon_lat)).unwrap_or(first)
        });
        let (point, distance) = match point {
            Some(point) => (point, (point - self.position).length()),
            None => (self.position, self.height_above_ground()),
        };
        let (lon_lat, _) = geo::from_ecef(point);
        Overhead {
            lat: lon_lat.y.to_degrees(),
            lon: lon_lat.x.to_degrees(),
            span_m: 2.0 * distance.max(MIN_CLEARANCE) * (self.fov / 2.0).tan(),
        }
    }

    pub(crate) fn up(&self) -> DVec3 {
        self.position.normalize()
    }

    /// The camera's right, rolled while it rides, as the picture is: the terrain is chosen
    /// for the view this spans.
    pub(crate) fn right(&self) -> DVec3 {
        self.forward.cross(self.camera_up()).normalize()
    }

    /// The camera's up: rolled while it rides.
    fn camera_up(&self) -> DVec3 {
        self.rolled_up.unwrap_or_else(|| self.up())
    }

    /// Ride along with the samples sent from here on, starting over if already riding.
    pub(crate) fn ride(&mut self, now: Instant) {
        self.mode = Mode::Riding(Box::new(Ride::new(now)));
    }

    /// Stop riding, the camera free where the ride left it, level again.
    pub(crate) fn release(&mut self) {
        if matches!(self.mode, Mode::Riding(_)) {
            self.mode = Mode::Free;
            self.rolled_up = None;
            self.settled = self.position;
        }
    }

    /// Take a fix for the ride, which reached it at `now`.
    pub(crate) fn fix(&mut self, fix: Fix, now: Instant) {
        if let Mode::Riding(ride) = &mut self.mode {
            ride.fix(fix, now);
        }
    }

    /// Take an orientation for the ride, which reached it at `now`.
    pub(crate) fn orientation(&mut self, orientation: Orientation, now: Instant) {
        if let Mode::Riding(ride) = &mut self.mode {
            ride.orientation(orientation, now);
        }
    }

    pub(crate) fn height_above_ground(&self) -> f64 {
        self.position.length() - geo::EARTH_RADIUS - self.ground
    }

    /// Meters on the ground below the camera per the picture's height.
    fn meters_per_height(&self) -> f64 {
        self.height_above_ground().max(MIN_CLEARANCE) * 2.0 * (self.fov / 2.0).tan()
    }

    /// Move by a gesture. An overhead placement is kept by [`move_camera`], which has the
    /// ground to place it on.
    pub(crate) fn apply(&mut self, gesture: Gesture) {
        if let Gesture::Go(placement) = gesture {
            self.rolled_up = None;
            self.mode = match placement {
                Placement::View(start) => {
                    self.go(start);
                    Mode::Free
                }
                Placement::Overhead(overhead) => Mode::Pinned(overhead),
            };
            return;
        }
        if matches!(self.mode, Mode::Riding(_)) {
            return;
        }
        // The operator takes over from a placement
        self.mode = Mode::Free;
        let up = self.up();
        match gesture {
            Gesture::Pan { dx, dy } => {
                let ahead = up.cross(self.right()).normalize_or_zero();
                let scale = self.meters_per_height();
                self.position += (-self.right() * dx + ahead * dy) * scale;
            }
            Gesture::Look { dx, dy } => {
                self.forward = DQuat::from_axis_angle(up, -dx * self.fov) * self.forward;
                let pitched = DQuat::from_axis_angle(self.right(), -dy * self.fov) * self.forward;
                if pitched.dot(up).abs() < MAX_STEEPNESS {
                    self.forward = pitched;
                }
            }
            Gesture::Zoom { steps } => {
                // Halfway between the height and the slant distance to the ground ahead, so
                // looking at the horizon does not fly off into it
                let height = self.height_above_ground().max(MIN_CLEARANCE);
                let down = (-self.forward.dot(up)).max(0.1);
                let reach = 0.5 * (height + height / down);
                let factor = (1.0 - ZOOM_STEP).powf(steps);
                self.position += self.forward * reach * (1.0 - factor);
            }
            Gesture::Go(_) => {}
        }
    }

    /// Move as the mode has it at `now`, and keep the camera above the terrain and within
    /// sight of the earth. Unless it rides, also level with the horizon as it moves around
    /// the globe: a ridden camera keeps the attitude of what carries it.
    fn advance(&mut self, now: Instant, ground: impl Fn(DVec2) -> f64) {
        let (pinned, ridden) = match &mut self.mode {
            Mode::Free => (None, None),
            Mode::Pinned(overhead) => (Some(*overhead), None),
            Mode::Riding(ride) => (None, Some(ride.pose(now))),
        };
        // Placed anew each frame, as the elevation beneath may have come in
        if let Some(overhead) = pinned {
            self.place(overhead, &ground);
        }
        // Until the ride's first fix, the camera waits where it is
        if let Some(pose) = ridden.flatten() {
            self.position = pose.position;
            self.forward = pose.forward;
            self.rolled_up = Some(pose.up);
            self.settled = pose.position;
        }
        let clearance = match self.mode {
            Mode::Riding(_) => RIDING_CLEARANCE,
            Mode::Free | Mode::Pinned(_) => MIN_CLEARANCE,
        };

        let (lon_lat, _) = geo::from_ecef(self.position);
        self.ground = ground(lon_lat);
        let min = geo::EARTH_RADIUS + self.ground + clearance;
        self.position = self.position.normalize()
            * self
                .position
                .length()
                .clamp(min, geo::EARTH_RADIUS + MAX_HEIGHT);
        if self.rolled_up.is_none() {
            self.forward =
                (DQuat::from_rotation_arc(self.settled.normalize(), self.position.normalize())
                    * self.forward)
                    .normalize();
        }
        self.settled = self.position;
    }
}

/// Where the ray from `origin` along `direction` (of unit length) first meets the sphere of
/// `radius` around the earth's centre, if it does ahead of the origin.
fn hit_sphere(origin: DVec3, direction: DVec3, radius: f64) -> Option<DVec3> {
    let b = origin.dot(direction);
    let c = origin.length_squared() - radius * radius;
    let discriminant = b * b - c;
    if discriminant < 0.0 {
        return None;
    }
    let t = -b - discriminant.sqrt();
    (t > 0.0).then(|| origin + direction * t)
}

/// The height of the ground at a position, as far as its elevation has streamed in.
fn ground_of(tiles: &Tiles) -> impl Fn(DVec2) -> f64 + '_ {
    |lon_lat| f64::from(tiles.height_at(lon_lat, GROUND_ZOOM).unwrap_or(0.0))
}

fn move_camera(tiles: Res<Tiles>, mut view: ResMut<View>) {
    view.advance(Instant::now(), ground_of(&tiles));
}

fn publish_focus(view: Res<View>, tiles: Res<Tiles>, focus: Res<Focus>) {
    focus.0.send_replace(Some(view.focus(ground_of(&tiles))));
}

pub(crate) fn camera_bundle() -> impl Bundle {
    (
        MainCamera,
        Camera3d::default(),
        Hdr,
        Projection::Perspective(PerspectiveProjection {
            fov: 60f32.to_radians(),
            near: 0.5,
            // Only used for culling, the projection itself has no far plane
            far: 1e12,
            ..default()
        }),
        Transform::default(),
    )
}

fn sync_camera(
    mut view: ResMut<View>,
    camera: Single<(&mut Transform, &mut Projection, &Camera), With<MainCamera>>,
) {
    let (mut transform, mut projection, camera) = camera.into_inner();
    *transform =
        Transform::default().looking_to(view.forward.as_vec3(), view.camera_up().as_vec3());

    if let Projection::Perspective(perspective) = projection.as_mut() {
        // The depth buffer is reversed and has no far plane, so precision is plentiful
        // even with a near plane this close
        let near = view.height_above_ground() * 0.05;
        perspective.near = near.clamp(0.02, 1000.0) as f32;
        view.fov = f64::from(perspective.fov);
        view.aspect = f64::from(perspective.aspect_ratio);
    }
    if let Some(size) = camera.physical_viewport_size() {
        view.viewport_height = f64::from(size.y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View::new(StartView {
            lat: 0.0,
            lon: 0.0,
            height: 1000.0,
            heading: 0.0,
            pitch: -90.0 + 1.0,
        })
    }

    fn flat(_: DVec2) -> f64 {
        0.0
    }

    fn assert_close(found: Overhead, expected: Overhead) {
        assert!(
            (found.lat - expected.lat).abs() < 1e-7
                && (found.lon - expected.lon).abs() < 1e-7
                && (found.span_m / expected.span_m - 1.0).abs() < 1e-6,
            "{found:?} is not {expected:?}"
        );
    }

    #[test]
    fn panning_moves_the_camera_against_the_drag() {
        let mut view = view();
        let start = view.position;
        // Dragging right moves the ground right, so the camera moves west
        view.apply(Gesture::Pan { dx: 0.1, dy: 0.0 });
        let (east, _, _) = geo::enu(start);
        assert!((view.position - start).dot(east) < -1.0);
    }

    #[test]
    fn zooming_in_and_out_returns_close_to_the_start() {
        let mut view = view();
        let start = view.height_above_ground();
        view.apply(Gesture::Zoom { steps: 1.0 });
        let closer = view.height_above_ground();
        assert!(closer < start * 0.9, "{closer} from {start}");
        view.apply(Gesture::Zoom { steps: -1.0 });
        assert!((view.height_above_ground() - start).abs() < start * 0.1);
    }

    #[test]
    fn a_placement_overhead_is_what_the_camera_reports() {
        for overhead in [
            Overhead {
                lat: 55.7,
                lon: 12.5,
                span_m: 800.0,
            },
            Overhead {
                lat: -33.9,
                lon: 151.2,
                span_m: 250_000.0,
            },
        ] {
            let mut view = view();
            view.place(overhead, flat);
            assert_close(view.focus(flat), overhead);
        }
    }

    #[test]
    fn a_placement_overhead_stands_above_the_ground() {
        let overhead = Overhead {
            lat: 46.0,
            lon: 7.7,
            span_m: 300.0,
        };
        let alps = |_: DVec2| 3000.0;
        let mut view = view();
        view.place(overhead, alps);
        view.ground = 3000.0;
        assert!(
            view.height_above_ground() > 200.0,
            "{}",
            view.height_above_ground()
        );
        assert_close(view.focus(alps), overhead);
    }

    #[test]
    fn looking_at_the_sky_reports_the_ground_below() {
        let mut view = view();
        view.forward = view.up();
        let focus = view.focus(flat);
        assert!(
            focus.lat.abs() < 1e-9 && focus.lon.abs() < 1e-9,
            "{focus:?}"
        );
        let expected = 2.0 * 1000.0 * (view.fov / 2.0).tan();
        assert!((focus.span_m - expected).abs() < 1e-6, "{focus:?}");
    }

    fn fix_at(now: Instant) -> Fix {
        Fix {
            taken_s: None,
            received: now,
            lat: 55.0,
            lon: 10.0,
            alt_msl_m: 150.0,
            velocity_ned: None,
        }
    }

    #[test]
    fn a_ridden_camera_goes_where_the_ride_does_and_rolls_with_it() {
        let now = Instant::now();
        let mut view = view();
        view.ride(now);
        view.fix(fix_at(now), now);
        view.orientation(
            Orientation {
                taken_s: None,
                received: now,
                roll: 20f64.to_radians(),
                pitch: 0.0,
                yaw: 0.0,
            },
            now,
        );
        view.advance(now + std::time::Duration::from_millis(50), flat);

        let (lon_lat, height) = geo::from_ecef(view.position);
        assert!((lon_lat.y.to_degrees() - 55.0).abs() < 1e-9);
        assert!((height - 150.0).abs() < 1e-6, "{height}");
        let (_, north, _) = geo::enu(view.position);
        assert!(view.forward.dot(north) > 0.999);
        let roll = view.camera_up().dot(view.up()).acos().to_degrees();
        assert!((roll - 20.0).abs() < 1e-6, "{roll}");

        // The terrain is chosen for the picture drawn, rolled as it is, or the tiles in its
        // lower corners would be culled in a bank
        let drawn_right = view.forward.cross(view.camera_up()).normalize();
        assert!(
            view.right().distance(drawn_right) < 1e-9,
            "{}",
            view.right()
        );
    }

    #[test]
    fn gestures_do_not_move_a_ridden_camera_but_a_placement_ends_the_ride() {
        let now = Instant::now();
        let mut view = view();
        view.ride(now);
        view.fix(fix_at(now), now);
        view.advance(now, flat);
        let riding = view.position;

        view.apply(Gesture::Pan { dx: 0.5, dy: 0.5 });
        view.apply(Gesture::Zoom { steps: 3.0 });
        assert_eq!(view.position, riding);

        view.apply(Gesture::Go(Placement::Overhead(Overhead {
            lat: 0.0,
            lon: 0.0,
            span_m: 1000.0,
        })));
        view.fix(fix_at(now), now);
        view.advance(now, flat);
        assert_close(
            view.focus(flat),
            Overhead {
                lat: 0.0,
                lon: 0.0,
                span_m: 1000.0,
            },
        );
        assert!(view.rolled_up.is_none(), "level again");
    }

    #[test]
    fn a_released_ride_leaves_the_camera_where_it_was() {
        let now = Instant::now();
        let mut view = view();
        view.ride(now);
        view.fix(fix_at(now), now);
        view.advance(now, flat);
        let riding = view.position;
        view.release();
        view.advance(now, flat);
        assert!((view.position - riding).length() < 1e-6);
        view.apply(Gesture::Pan { dx: 0.5, dy: 0.0 });
        assert!((view.position - riding).length() > 1.0, "free to move");
    }

    #[test]
    fn looking_turns_up_from_overhead_but_never_upside_down() {
        let mut view = view();
        view.place(
            Overhead {
                lat: 0.0,
                lon: 0.0,
                span_m: 1000.0,
            },
            flat,
        );
        let steep = view.forward.dot(view.up());
        view.apply(Gesture::Look { dx: 0.0, dy: -0.01 });
        assert!(view.forward.dot(view.up()) > steep, "turned up");

        view.apply(Gesture::Look { dx: 0.0, dy: 100.0 });
        assert!(view.forward.dot(view.up()).abs() < MAX_STEEPNESS);
        assert!(view.right().is_finite());
    }
}
