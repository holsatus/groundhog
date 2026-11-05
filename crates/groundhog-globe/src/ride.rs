//! Riding along a moving pose: the camera carried by a vehicle, looking where it points.
//!
//! Samples arrive irregularly: positions and orientations each at a rate of their own, from a
//! few to a few tens a second, and late by however long the link took. Shown as they arrive,
//! the view would jump at each and stand still between them. So the ride is shown a little in
//! the past, at a time between samples already received, and interpolated there: positions
//! along a curve that meets each fix at its velocity, orientations by slerp.
//!
//! A sample is placed in time by when the source took it, where it says, mapped onto this
//! clock by the least delay seen ([`Clock`]), so the link's jitter does not move samples
//! about. How far in the past the ride is shown follows how far apart and how late samples
//! come ([`Track::wanted_delay`]), and changes gradually, so the view never jumps in time.
//! How late a sample is counts until the ride has it, not only until it arrived: the way
//! from the link to here (a snapshot every few tens of milliseconds, a busy thread) delays
//! it as much as the link does.

use std::{collections::VecDeque, time::Instant};

use bevy::math::{DQuat, DVec2, DVec3, EulerRot};

use crate::geo;

/// How fast a source's clock is let to fall behind this one, in seconds per second: clocks
/// drift by parts per million. A link that slows down for a moment makes samples late,
/// which the delay they are shown at takes up; one that slows down for good is a step
/// ([`CLOCK_WINDOW_S`]).
const CLOCK_DRIFT: f64 = 0.001;
/// How far a source's clock must go back to be taken as restarted, in seconds.
const CLOCK_REWIND_S: f64 = 1.0;
/// How long every sample must come later than the least delay, and by how much, for the
/// delay to be taken as having grown for good, in seconds. A simulator that stalls loses
/// that time for good, and a link whose buffers fill keeps them full.
const CLOCK_WINDOW_S: f64 = 1.0;
const CLOCK_STEP_S: f64 = 0.05;
/// How fast the lateness remembered of past samples fades, in seconds per second: slowly
/// enough to remember a link's worst jitter between the times it recurs, or a sample comes
/// too late every few seconds.
const LATENESS_DECAY: f64 = 0.01;
/// How much the interval between samples follows each new one.
const INTERVAL_WEIGHT: f64 = 0.1;
/// Added to the delay the samples want, for timing the samples do not show.
const DELAY_MARGIN_S: f64 = 0.02;
const MIN_DELAY_S: f64 = 0.02;
const MAX_DELAY_S: f64 = 1.0;
/// How fast the delay changes, in seconds per second: the ride plays at most this much
/// faster or slower than real time while it does.
const DELAY_RATE: f64 = 0.1;
/// How far past the newest fix its velocity carries the camera on, in seconds.
const MAX_EXTRAPOLATION_S: f64 = 0.5;

/// Where the ridden pose was, at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    /// When the source took it, in seconds by its own clock, if it says.
    pub taken_s: Option<f64>,
    /// When it arrived.
    pub received: Instant,
    /// Degrees.
    pub lat: f64,
    pub lon: f64,
    /// Height above mean sea level, in meters.
    pub alt_msl_m: f64,
    /// Velocity north, east and down, in meters per second, if known.
    pub velocity_ned: Option<[f64; 3]>,
}

/// Which way the ridden pose pointed, at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orientation {
    /// When the source took it, in seconds by its own clock (the same as its fixes'), if it says.
    pub taken_s: Option<f64>,
    /// When it arrived.
    pub received: Instant,
    /// Radians, from north-east-down, in the aerospace 3-2-1 order: yaw, then pitch, then roll.
    pub roll: f64,
    pub pitch: f64,
    pub yaw: f64,
}

/// Where the camera is and which way it looks, in ECEF.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pose {
    pub position: DVec3,
    pub forward: DVec3,
    pub up: DVec3,
}

/// A fix where it was, in ECEF.
#[derive(Clone, Copy, Debug)]
struct Point {
    position: DVec3,
    velocity: Option<DVec3>,
}

/// A ride under way: the samples received, and how far in the past it is shown.
#[derive(Debug)]
pub(crate) struct Ride {
    epoch: Instant,
    clock: Clock,
    fixes: Track<Point>,
    orientations: Track<DQuat>,
    delay: Option<f64>,
    /// The time last shown, in seconds since the epoch: never goes back.
    shown_s: f64,
    /// When the last pose was asked for, in seconds since the epoch.
    asked_s: Option<f64>,
}

impl Ride {
    pub(crate) fn new(epoch: Instant) -> Self {
        Self {
            epoch,
            clock: Clock::default(),
            fixes: Track::default(),
            orientations: Track::default(),
            delay: None,
            shown_s: f64::NEG_INFINITY,
            asked_s: None,
        }
    }

    fn seconds(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.epoch).as_secs_f64()
    }

    /// Take `fix`, which reached the ride at `now`.
    pub(crate) fn fix(&mut self, fix: Fix, now: Instant) {
        let values = [fix.lat, fix.lon, fix.alt_msl_m];
        if !values.iter().all(|value| value.is_finite()) {
            return;
        }
        let t = self.place(fix.taken_s, fix.received);
        let position = geo::to_ecef(
            DVec2::new(fix.lon.to_radians(), fix.lat.to_radians()),
            fix.alt_msl_m,
        );
        let velocity = fix
            .velocity_ned
            .filter(|velocity| velocity.iter().all(|value| value.is_finite()))
            .map(|[north, east, down]| ned_to_ecef(position, DVec3::new(north, east, down)));
        let now = self.seconds(now);
        self.fixes.push(t, now, Point { position, velocity });
    }

    /// Take `orientation`, which reached the ride at `now`.
    pub(crate) fn orientation(&mut self, orientation: Orientation, now: Instant) {
        let angles = [orientation.roll, orientation.pitch, orientation.yaw];
        if !angles.iter().all(|angle| angle.is_finite()) {
            return;
        }
        let t = self.place(orientation.taken_s, orientation.received);
        let rotation = DQuat::from_euler(
            EulerRot::ZYX,
            orientation.yaw,
            orientation.pitch,
            orientation.roll,
        );
        let now = self.seconds(now);
        self.orientations.push(t, now, rotation);
    }

    /// When a sample taken at `taken` by the source's clock and received at `received`
    /// happened, on this clock.
    fn place(&mut self, taken: Option<f64>, received: Instant) -> f64 {
        let received = self.seconds(received);
        let (t, step) = self.clock.place(taken, received);
        // The samples that came during the step were late by it, by the clock before it
        self.fixes.lateness = (self.fixes.lateness - step).max(0.0);
        self.orientations.lateness = (self.orientations.lateness - step).max(0.0);
        t
    }

    /// The pose to show at `now`, once a fix has arrived.
    pub(crate) fn pose(&mut self, now: Instant) -> Option<Pose> {
        let now_s = self.seconds(now);
        let elapsed = self.asked_s.map_or(0.0, |asked| (now_s - asked).max(0.0));
        self.asked_s = Some(now_s);

        let wanted = [self.fixes.wanted_delay(), self.orientations.wanted_delay()]
            .into_iter()
            .flatten()
            .reduce(f64::max);
        if let Some(wanted) = wanted {
            let wanted = (wanted + DELAY_MARGIN_S).clamp(MIN_DELAY_S, MAX_DELAY_S);
            let step = DELAY_RATE * elapsed;
            self.delay = Some(match self.delay {
                Some(delay) => delay + (wanted - delay).clamp(-step, step),
                None => wanted,
            });
        }
        let t = (now_s - self.delay.unwrap_or(0.0)).max(self.shown_s);
        self.shown_s = t;

        let position = self.fixes.position_at(t)?;
        let rotation = self.orientations.rotation_at(t);
        self.fixes.forget_before(t);
        self.orientations.forget_before(t);

        let (_, north, up) = geo::enu(position);
        let (forward, camera_up) = match rotation {
            Some(rotation) => (
                ned_to_ecef(position, rotation * DVec3::X),
                ned_to_ecef(position, rotation * DVec3::NEG_Z),
            ),
            None => {
                // Along the way it moves, level, until it says which way it points
                let heading = self
                    .fixes
                    .newest_velocity()
                    .map(|velocity| velocity - up * velocity.dot(up))
                    .filter(|horizontal| horizontal.length() > 0.5)
                    .map_or(north, DVec3::normalize);
                (heading, up)
            }
        };
        Some(Pose {
            position,
            forward: forward.normalize(),
            up: camera_up.normalize(),
        })
    }
}

/// A vector in the north-east-down frame at `position`, in ECEF.
fn ned_to_ecef(position: DVec3, ned: DVec3) -> DVec3 {
    let (east, north, up) = geo::enu(position);
    north * ned.x + east * ned.y - up * ned.z
}

/// Maps the times a source takes its samples at onto this clock.
///
/// A sample arrives some delay after it is taken: the least delay the link ever has, plus
/// whatever jitter it adds. The least `received - taken` seen is the best estimate of the
/// first, so the source's clock plus it places samples as evenly as they were taken. It may
/// grow slowly, as the clocks drift apart, and in a step when every sample of a while comes
/// later ([`CLOCK_WINDOW_S`]); it is started over when the source's clock goes back, as when
/// the source restarts.
#[derive(Debug)]
struct Clock {
    offset: Option<f64>,
    last_taken: f64,
    last_received: f64,
    /// The least delay of the samples received since `window_start`.
    window_least: f64,
    window_start: f64,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            offset: None,
            last_taken: 0.0,
            last_received: 0.0,
            window_least: f64::INFINITY,
            window_start: 0.0,
        }
    }
}

impl Clock {
    /// When a sample taken at `taken` (the source's seconds) and received at `received` (this
    /// clock's) happened, on this clock, and how far the delay stepped up with it, if it did.
    /// A sample without a time of its own happened when it arrived.
    fn place(&mut self, taken: Option<f64>, received: f64) -> (f64, f64) {
        let Some(taken) = taken else {
            return (received, 0.0);
        };
        let delay = received - taken;
        self.window_least = self.window_least.min(delay);
        let window_over = received - self.window_start >= CLOCK_WINDOW_S;
        let (offset, step) = match self.offset {
            Some(offset) if taken > self.last_taken - CLOCK_REWIND_S => {
                let allowed = offset + CLOCK_DRIFT * (received - self.last_received).max(0.0);
                match window_over && self.window_least > allowed + CLOCK_STEP_S {
                    true => (self.window_least, self.window_least - allowed),
                    false => (allowed.min(delay), 0.0),
                }
            }
            _ => (delay, 0.0),
        };
        if window_over || self.offset.is_none() {
            self.window_least = f64::INFINITY;
            self.window_start = received;
        }
        self.offset = Some(offset);
        self.last_taken = taken;
        self.last_received = received;
        (taken + offset, step)
    }
}

/// The samples of one kind, oldest first, and how they come.
#[derive(Debug)]
struct Track<T> {
    samples: VecDeque<(f64, T)>,
    /// How far apart samples come, averaged.
    interval: Option<f64>,
    /// How long after its time a sample reaches the ride, at most of late.
    lateness: f64,
    last_arrived: f64,
}

impl<T> Default for Track<T> {
    fn default() -> Self {
        Self {
            samples: VecDeque::new(),
            interval: None,
            lateness: 0.0,
            last_arrived: 0.0,
        }
    }
}

impl<T: Copy> Track<T> {
    /// Add a sample at `t` that reached the ride at `arrived`. One no newer than the newest
    /// is dropped: it is a repeat, or came out of order.
    fn push(&mut self, t: f64, arrived: f64, value: T) {
        if let Some(&(newest, _)) = self.samples.back() {
            if t <= newest {
                return;
            }
            let gap = t - newest;
            self.interval = Some(match self.interval {
                Some(interval) => interval + (gap - interval) * INTERVAL_WEIGHT,
                None => gap,
            });
        }
        let faded = self.lateness - LATENESS_DECAY * (arrived - self.last_arrived).max(0.0);
        self.lateness = (arrived - t).max(faded).max(0.0);
        self.last_arrived = arrived;
        self.samples.push_back((t, value));
    }

    /// How far in the past to show, for a sample to have arrived after each moment shown:
    /// one interval, and however late samples come.
    fn wanted_delay(&self) -> Option<f64> {
        self.interval.map(|interval| interval + self.lateness)
    }

    /// Drop the samples no longer needed to show `t` or later.
    fn forget_before(&mut self, t: f64) {
        while self.samples.get(1).is_some_and(|&(next, _)| next <= t) {
            self.samples.pop_front();
        }
    }

    /// The samples either side of `t`, and how far between them it lies; or the one sample
    /// before or after all of them, and how far past it `t` is.
    fn around(&self, t: f64) -> Option<Around<T>> {
        let after = self.samples.iter().position(|&(time, _)| time > t);
        match after {
            Some(0) => self
                .samples
                .front()
                .map(|&(_, value)| Around::Before(value)),
            Some(index) => {
                let (t0, a) = *self.samples.get(index - 1)?;
                let (t1, b) = *self.samples.get(index)?;
                Some(Around::Between(a, b, t1 - t0, (t - t0) / (t1 - t0)))
            }
            None => self
                .samples
                .back()
                .map(|&(time, value)| Around::After(value, t - time)),
        }
    }
}

enum Around<T> {
    Before(T),
    /// Two samples, the time between them, and how far between them, from 0 to 1.
    Between(T, T, f64, f64),
    /// The newest sample, and how far past it, in seconds.
    After(T, f64),
}

impl Track<Point> {
    fn position_at(&self, t: f64) -> Option<DVec3> {
        Some(match self.around(t)? {
            Around::Before(point) => point.position,
            Around::Between(a, b, span, s) => match (a.velocity, b.velocity) {
                (Some(va), Some(vb)) => hermite(a.position, va, b.position, vb, span, s),
                _ => a.position.lerp(b.position, s),
            },
            Around::After(point, past) => {
                let velocity = point.velocity.unwrap_or(DVec3::ZERO);
                point.position + velocity * past.min(MAX_EXTRAPOLATION_S)
            }
        })
    }

    fn newest_velocity(&self) -> Option<DVec3> {
        self.samples.back().and_then(|(_, point)| point.velocity)
    }
}

impl Track<DQuat> {
    fn rotation_at(&self, t: f64) -> Option<DQuat> {
        Some(match self.around(t)? {
            Around::Before(rotation) | Around::After(rotation, _) => rotation,
            Around::Between(a, b, _, s) => a.slerp(b, s),
        })
    }
}

/// The cubic from `p0` at velocity `v0` to `p1` at velocity `v1`, `span` seconds later, at
/// `s` of the way.
fn hermite(p0: DVec3, v0: DVec3, p1: DVec3, v1: DVec3, span: f64, s: f64) -> DVec3 {
    let (s2, s3) = (s * s, s * s * s);
    p0 * (2.0 * s3 - 3.0 * s2 + 1.0)
        + v0 * span * (s3 - 2.0 * s2 + s)
        + p1 * (-2.0 * s3 + 3.0 * s2)
        + v1 * span * (s3 - s2)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    impl Ride {
        /// Take a sample the moment it arrives.
        fn take_fix(&mut self, fix: Fix) {
            self.fix(fix, fix.received);
        }

        fn take_orientation(&mut self, orientation: Orientation) {
            self.orientation(orientation, orientation.received);
        }
    }

    fn after(epoch: Instant, seconds: f64) -> Instant {
        epoch + Duration::from_secs_f64(seconds)
    }

    /// A jitter that looks random, from 0 to `most` seconds.
    fn jitter(i: usize, most: f64) -> f64 {
        most * ((i * 7919 + 13) % 101) as f64 / 100.0
    }

    /// Northward along the meridian through 10° E at `speed` m/s, from 55° N, at 100 m.
    fn northward(epoch: Instant, taken: f64, late: f64, speed: f64) -> Fix {
        let metres_per_degree = geo::EARTH_RADIUS.to_radians();
        Fix {
            taken_s: Some(1000.0 + taken),
            received: after(epoch, taken + late),
            lat: 55.0 + speed * taken / (metres_per_degree * (1.0 + 100.0 / geo::EARTH_RADIUS)),
            lon: 10.0,
            alt_msl_m: 100.0,
            velocity_ned: Some([speed, 0.0, 0.0]),
        }
    }

    #[test]
    fn samples_are_placed_by_when_they_were_taken_not_when_they_came() {
        let mut clock = Clock::default();
        let placed: Vec<f64> = (0..20)
            .map(|i| {
                let taken = 500.0 + i as f64 * 0.1;
                // The third arrives with the least delay
                let jitter = if i == 2 { 0.0 } else { jitter(i, 0.04) };
                clock
                    .place(Some(taken), 3.0 + i as f64 * 0.1 + 0.05 + jitter)
                    .0
            })
            .collect();
        // Once the least delay is seen, they come out as evenly as they were taken, where
        // their arrival is up to 40 ms uneven
        for pair in placed.windows(2).skip(2) {
            assert!(((pair[1] - pair[0]) - 0.1).abs() < 0.002, "{placed:?}");
        }
    }

    #[test]
    fn a_source_that_restarts_starts_the_clock_over() {
        let mut clock = Clock::default();
        clock.place(Some(500.0), 10.0);
        // The source restarted: its clock starts near zero again
        assert!((clock.place(Some(0.5), 11.0).0 - 11.0).abs() < 1e-9);
        assert!((clock.place(Some(0.6), 11.1).0 - 11.1).abs() < 1e-9);
    }

    #[test]
    fn a_delay_that_grows_for_good_is_taken_up_once() {
        let mut clock = Clock::default();
        let mut steps = Vec::new();
        let mut late = Vec::new();
        for i in 0..60 {
            let taken = 500.0 + i as f64 * 0.1;
            // From the 20th on, every sample comes half a second later, as after a simulator
            // stalled
            let delay = if i < 20 { 0.05 } else { 0.55 };
            let received = 3.0 + i as f64 * 0.1 + delay;
            let (t, step) = clock.place(Some(taken), received);
            if step > 0.0 {
                steps.push(step);
            }
            late.push(received - t);
        }
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert!((steps[0] - 0.5).abs() < 0.01, "{steps:?}");
        // Within two windows, samples are placed when they come again, not half a second
        // before, which would leave the ride waiting for each
        assert!(late[40..].iter().all(|late| late.abs() < 1e-6), "{late:?}");
    }

    #[test]
    fn samples_held_up_on_the_way_to_the_ride_are_waited_for() {
        let epoch = Instant::now();
        let mut ride = Ride::new(epoch);
        let mut next = 0;
        // Fixes at 5 Hz, received at once, but handed to the ride up to 50 ms later, as a
        // snapshot holds them
        for i in 0..(144 * 6) {
            let now = i as f64 / 144.0;
            loop {
                let taken = next as f64 * 0.2;
                let reached = taken + jitter(next, 0.05);
                if reached > now {
                    break;
                }
                ride.fix(northward(epoch, taken, 0.0, 20.0), after(epoch, reached));
                next += 1;
            }
            ride.pose(after(epoch, now));
            if now > 3.0 {
                let newest = ride.fixes.samples.back().map(|&(t, _)| t);
                assert!(
                    newest.is_some_and(|newest| newest >= ride.shown_s),
                    "ran out of fixes at {now} s"
                );
            }
        }
    }

    #[test]
    fn a_ride_moves_steadily_between_jittery_fixes() {
        let epoch = Instant::now();
        let speed = 20.0;
        let mut ride = Ride::new(epoch);
        let mut next_fix = 0;
        let mut last: Option<DVec3> = None;
        let frame = 1.0 / 144.0;
        // Fixes taken at 5 Hz, arriving 50 to 130 ms later; frames at 144 Hz
        for i in 0..(144 * 6) {
            let now = i as f64 * frame;
            while next_fix as f64 * 0.2 + 0.05 + jitter(next_fix, 0.08) <= now {
                let taken = next_fix as f64 * 0.2;
                ride.take_fix(northward(
                    epoch,
                    taken,
                    0.05 + jitter(next_fix, 0.08),
                    speed,
                ));
                next_fix += 1;
            }
            let Some(pose) = ride.pose(after(epoch, now)) else {
                continue;
            };
            if let Some(last) = last.filter(|_| now > 3.0) {
                // Settled: each frame moves as far as the vehicle does in one
                let step = (pose.position - last).length();
                assert!(
                    (step - speed * frame).abs() < speed * frame * 0.15,
                    "step {step} m at {now} s"
                );
            }
            last = Some(pose.position);
        }
    }

    #[test]
    fn a_turn_is_followed_along_its_curve() {
        // A circle of 100 m around a point, fixes a quarter turn apart with their velocities
        let centre = geo::to_ecef(DVec2::new(10f64.to_radians(), 55f64.to_radians()), 100.0);
        let (east, north, _) = geo::enu(centre);
        let at = |angle: f64| centre + (east * angle.cos() + north * angle.sin()) * 100.0;
        let velocity = |angle: f64| (-east * angle.sin() + north * angle.cos()) * 100.0;
        let quarter = std::f64::consts::FRAC_PI_2;
        let middle = hermite(
            at(0.0),
            velocity(0.0),
            at(quarter),
            velocity(quarter),
            quarter,
            0.5,
        );
        let off = ((middle - centre).length() - 100.0).abs();
        assert!(off < 2.0, "{off} m off the circle");
        // Where the chord would cut the corner by 29 m
        let chord = at(0.0).lerp(at(quarter), 0.5);
        assert!(((chord - centre).length() - 100.0).abs() > 25.0);
    }

    #[test]
    fn the_camera_looks_where_the_vehicle_points_and_rolls_with_it() {
        let epoch = Instant::now();
        let mut ride = Ride::new(epoch);
        ride.take_fix(northward(epoch, 0.0, 0.0, 0.0));
        ride.take_orientation(Orientation {
            taken_s: Some(1000.0),
            received: epoch,
            roll: 30f64.to_radians(),
            pitch: 0.0,
            yaw: 90f64.to_radians(),
        });
        let pose = ride.pose(after(epoch, 0.1)).expect("a pose");
        let (east, _, up) = geo::enu(pose.position);
        assert!(pose.forward.dot(east) > 0.999, "facing east");
        // Rolled right, the camera's up leans to its right: south, facing east
        let lean = pose.up.dot(up).acos().to_degrees();
        assert!((lean - 30.0).abs() < 1e-6, "{lean}");
        let right = pose.forward.cross(up);
        assert!(pose.up.dot(right) > 0.0, "leans right");
    }

    #[test]
    fn orientations_are_turned_through_between_samples() {
        let epoch = Instant::now();
        let mut ride = Ride::new(epoch);
        ride.take_fix(northward(epoch, 0.0, 0.0, 0.0));
        for (i, yaw) in [0.0, 90.0].into_iter().enumerate() {
            ride.take_orientation(Orientation {
                taken_s: Some(1000.0 + i as f64),
                received: after(epoch, i as f64),
                roll: 0.0,
                pitch: 0.0,
                yaw: f64::to_radians(yaw),
            });
        }
        let rotation = ride.orientations.rotation_at(0.5).expect("a rotation");
        let (yaw, _, _) = rotation.to_euler(EulerRot::ZYX);
        assert!(
            (yaw.to_degrees() - 45.0).abs() < 1e-6,
            "{}",
            yaw.to_degrees()
        );
    }

    #[test]
    fn past_the_newest_fix_the_camera_coasts_only_briefly() {
        let epoch = Instant::now();
        let mut ride = Ride::new(epoch);
        ride.take_fix(northward(epoch, 0.0, 0.0, 10.0));
        ride.take_fix(northward(epoch, 0.2, 0.0, 10.0));
        let newest = ride.fixes.samples.back().map(|(_, point)| point.position);
        let pose = ride.pose(after(epoch, 30.0)).expect("a pose");
        let coasted = (pose.position - newest.expect("a fix")).length();
        assert!(
            (coasted - 10.0 * MAX_EXTRAPOLATION_S).abs() < 0.01,
            "{coasted}"
        );
    }

    #[test]
    fn the_ride_never_goes_back_in_time() {
        let epoch = Instant::now();
        let mut ride = Ride::new(epoch);
        let mut shown = f64::NEG_INFINITY;
        for i in 0..400 {
            let now = i as f64 / 100.0;
            // Samples come later and later, so the delay grows
            if i % 10 == 0 {
                let late = now * 0.1;
                ride.take_fix(northward(epoch, now - late, late, 5.0));
            }
            ride.pose(after(epoch, now));
            assert!(ride.shown_s >= shown);
            shown = ride.shown_s;
            assert!(ride.delay.unwrap_or(0.0) <= MAX_DELAY_S);
        }
    }

    #[test]
    fn nonsense_samples_are_ignored() {
        let epoch = Instant::now();
        let mut ride = Ride::new(epoch);
        let mut fix = northward(epoch, 0.0, 0.0, 0.0);
        fix.lat = f64::NAN;
        ride.take_fix(fix);
        assert!(ride.pose(epoch).is_none());
    }
}
