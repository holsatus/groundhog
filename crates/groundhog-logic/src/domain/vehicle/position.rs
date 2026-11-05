//! Vehicle position: the current fix, the recent trail, and its pruning.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use mavio::default_dialect::messages;
use nalgebra::Vector3;

use crate::domain::geo::GeoPosition;

/// The most points kept in a trail.
pub const TRAIL_MAX_LEN: usize = 1000;
/// Points older than this (relative to the newest) are dropped first.
pub const TRAIL_MAX_AGE: Duration = Duration::from_secs(60 * 15);

#[derive(Debug, Clone, Default)]
pub struct PositionTrack {
    pub current: Option<GlobalPosition>,
    /// Where the vehicle has been, oldest first, thinned to [`TRAIL_MAX_LEN`].
    pub trail: VecDeque<GlobalPosition>,
    /// Velocity in north/east/down, metres per second.
    pub velocity_ned: Option<Vector3<f32>>,
    revision: u64,
}

#[derive(Clone, Debug)]
pub struct GlobalPosition {
    /// When the report arrived.
    pub at: Instant,
    /// When the autopilot took the fix, by its clock since boot, where the
    /// report says: the spacing of fixes as taken, which their arrival blurs.
    pub time_boot_ms: Option<u32>,
    /// Altitude above home, metres.
    pub alt_agl: f32,
    /// Altitude above mean sea level, metres.
    pub alt_msl: f32,
    pub pos: GeoPosition,
}

impl PositionTrack {
    /// Bumped on every new fix, for caches keyed on the trail.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The rate of climb, in metres per second (negative while descending).
    pub fn climb_rate(&self) -> Option<f32> {
        self.velocity_ned.map(|velocity| -velocity.z)
    }

    /// Horizontal speed over ground, metres per second.
    pub fn ground_speed(&self) -> Option<f32> {
        self.velocity_ned
            .map(|velocity| (velocity.x * velocity.x + velocity.y * velocity.y).sqrt())
    }

    /// Fold a position report whose coordinates the caller has decoded, so
    /// that a report without a location is never folded.
    pub(crate) fn observe_global_position_int(
        &mut self,
        message: &messages::GlobalPositionInt,
        pos: GeoPosition,
        time: Instant,
    ) {
        self.register(GlobalPosition {
            at: time,
            time_boot_ms: Some(message.time_boot_ms),
            pos,
            alt_agl: message.relative_alt as f32 / 1000.0,
            alt_msl: message.alt as f32 / 1000.0,
        });
        self.velocity_ned = Some(Vector3::new(
            message.vx as f32 / 100.,
            message.vy as f32 / 100.,
            message.vz as f32 / 100.,
        ));
    }

    /// Fold a position report whose coordinates the caller has decoded, so
    /// that a report without a location is never folded.
    pub(crate) fn observe_global_position_int_cov(
        &mut self,
        message: &messages::GlobalPositionIntCov,
        pos: GeoPosition,
        time: Instant,
    ) {
        // Its `time_usec` may count from boot or from 1970, so it is not
        // taken as either
        self.register(GlobalPosition {
            at: time,
            time_boot_ms: None,
            pos,
            alt_agl: message.relative_alt as f32 / 1000.0,
            alt_msl: message.alt as f32 / 1000.0,
        });
        self.velocity_ned = Some(Vector3::new(message.vx, message.vy, message.vz));
    }

    pub(crate) fn register(&mut self, position: GlobalPosition) {
        self.current = Some(position.clone());
        self.trail.push_back(position);
        if self.trail.len() > TRAIL_MAX_LEN {
            self.prune_one();
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Drop one point from the trail according to the retention policy.
    ///
    /// Points older than [`TRAIL_MAX_AGE`] (measured against the newest point,
    /// so the policy is a function of the data alone) go first. Otherwise the
    /// interior point that deviates least from the line through its
    /// neighbours is removed (a triangle-area cost weighted by the time span
    /// it bridges), which keeps the drawn track's shape while memory stays
    /// bounded.
    fn prune_one(&mut self) {
        if self.trail.len() < 3 {
            self.trail.pop_front();
            return;
        }

        let newest = self.trail.back().map(|position| position.at);
        if let (Some(oldest), Some(newest)) = (self.trail.front(), newest)
            && newest.saturating_duration_since(oldest.at) > TRAIL_MAX_AGE
        {
            self.trail.pop_front();
            return;
        }

        let mut prune_idx: Option<(usize, f32)> = None;

        for index in 1..self.trail.len() - 1 {
            let (prev, eval, next) = (
                &self.trail[index - 1],
                &self.trail[index],
                &self.trail[index + 1],
            );

            // Never bridge a long gap: those points mark where the track broke.
            // Either side counts: removing the point before a gap connects the
            // track across it just as removing the point after one does.
            let bridged = |a: &GlobalPosition, b: &GlobalPosition| {
                b.at.saturating_duration_since(a.at) > Duration::from_secs(20)
            };
            if bridged(prev, eval) || bridged(eval, next) {
                continue;
            }

            // Shoelace triangle area on degrees: locally proportional to the
            // projected area, which is all the argmin needs.
            let area = (prev.pos.lon * (eval.pos.lat - next.pos.lat)
                + eval.pos.lon * (next.pos.lat - prev.pos.lat)
                + next.pos.lon * (prev.pos.lat - eval.pos.lat))
                .abs();

            // Cost is weighed by the span of time between the outer points
            let span = next.at.saturating_duration_since(prev.at);
            let cost = (area as f32) * (1.0 + span.as_secs_f32());

            if prune_idx.is_none_or(|(_, best)| cost < best) {
                prune_idx = Some((index, cost));
            }
        }

        match prune_idx {
            Some((index, _)) => {
                self.trail.remove(index);
            }
            None => {
                self.trail.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(at: Instant, lat: f64, lon: f64) -> GlobalPosition {
        GlobalPosition {
            at,
            time_boot_ms: None,
            alt_agl: 0.0,
            alt_msl: 0.0,
            pos: GeoPosition::new(lat, lon),
        }
    }

    #[test]
    fn a_fix_keeps_its_altitudes_and_when_it_was_taken() {
        let mut track = PositionTrack::default();
        let message = messages::GlobalPositionInt {
            time_boot_ms: 5000,
            alt: 123_456,
            relative_alt: 23_456,
            ..Default::default()
        };
        track.observe_global_position_int(&message, GeoPosition::new(1.0, 2.0), Instant::now());
        let fix = track.current.as_ref().expect("a fix");
        assert_eq!(fix.time_boot_ms, Some(5000));
        assert_eq!(fix.alt_msl, 123.456);
        assert_eq!(fix.alt_agl, 23.456);
    }

    #[test]
    fn trail_never_exceeds_max_len() {
        let mut track = PositionTrack::default();

        for i in 0..TRAIL_MAX_LEN + 50 {
            track.register(position(
                Instant::now(),
                47.0 + i as f64 * 1e-3,
                8.0 + i as f64 * 1e-3,
            ));
        }

        assert_eq!(track.trail.len(), TRAIL_MAX_LEN);
    }

    #[test]
    fn stale_point_older_than_max_age_is_dropped() {
        let mut track = PositionTrack::default();

        let old = Instant::now() - (TRAIL_MAX_AGE + Duration::from_secs(1));
        track.register(position(old, 47.0, 8.0));
        track.register(position(Instant::now(), 47.1, 8.1));
        track.register(position(Instant::now(), 47.2, 8.2));

        track.prune_one();

        assert_eq!(track.trail.len(), 2);
        assert_eq!(
            track.trail.front().unwrap().pos,
            GeoPosition::new(47.1, 8.1)
        );
    }

    #[test]
    fn collinear_middle_point_is_pruned() {
        let mut track = PositionTrack::default();

        let now = Instant::now();
        track.register(position(now, 0.0, 0.0));
        track.register(position(now, 1.0, 1.0));
        track.register(position(now, 2.0, 2.0));

        track.prune_one();

        assert_eq!(track.trail.len(), 2);
        assert_eq!(track.trail[0].pos, GeoPosition::new(0.0, 0.0));
        assert_eq!(track.trail[1].pos, GeoPosition::new(2.0, 2.0));
    }

    #[test]
    fn a_new_fix_bumps_the_revision() {
        let mut track = PositionTrack::default();
        let before = track.revision();

        track.register(position(Instant::now(), 47.0, 8.0));

        assert_ne!(track.revision(), before, "caches keyed on the trail expire");
    }

    #[test]
    fn velocity_gives_climb_rate_and_ground_speed() {
        let track = PositionTrack {
            velocity_ned: Some(Vector3::new(3.0, 4.0, -2.0)),
            ..Default::default()
        };

        assert_eq!(track.climb_rate(), Some(2.0), "negative z is a climb");
        assert_eq!(track.ground_speed(), Some(5.0));
    }

    /// The forward gap must be kept too: pruning the point before it draws a
    /// straight line across the break.
    #[test]
    fn a_forward_long_gap_is_not_bridged() {
        let mut track = PositionTrack::default();
        let start = Instant::now();
        let second = start + Duration::from_secs(1);
        let before_gap = start + Duration::from_secs(2);
        let after_gap = start + Duration::from_secs(40);

        // A is off the line; B, C and D are collinear, so C is the cheapest
        // interior point to remove — unless its forward gap protects it.
        track.register(position(start, 0.010, 0.0));
        track.register(position(second, 0.0, 0.0));
        track.register(position(before_gap, 0.0, 0.001));
        track.register(position(after_gap, 0.0, 0.002));

        track.prune_one();

        assert!(
            track.trail.iter().any(|point| point.at == before_gap),
            "the point before the gap stays: {:?}",
            track.trail.iter().map(|point| point.at).collect::<Vec<_>>()
        );
    }

    /// A backward gap marks a break as well.
    #[test]
    fn a_backward_long_gap_is_not_bridged() {
        let mut track = PositionTrack::default();
        let start = Instant::now();
        let after_gap = start + Duration::from_secs(40);

        track.register(position(start, 0.0, 0.0));
        track.register(position(after_gap, 0.0, 0.001));
        track.register(position(after_gap + Duration::from_secs(1), 0.0, 0.002));
        track.register(position(after_gap + Duration::from_secs(2), 0.0, 0.003));

        // The old point is pruned; the one the gap follows stays.
        track.prune_one();

        assert!(
            track.trail.iter().any(|point| point.at == after_gap),
            "the point after the gap stays"
        );
    }

    /// When every interior point sits beside a gap, the oldest is dropped:
    /// the trail must still shrink.
    #[test]
    fn a_track_of_gaps_drops_its_oldest_point() {
        let mut track = PositionTrack::default();
        let start = Instant::now();

        track.register(position(start, 0.0, 0.0));
        track.register(position(start + Duration::from_secs(25), 0.0, 0.001));
        track.register(position(start + Duration::from_secs(50), 0.0, 0.002));

        track.prune_one();

        assert_eq!(track.trail.len(), 2);
        assert_eq!(
            track.trail.front().unwrap().at,
            start + Duration::from_secs(25)
        );
    }
}
