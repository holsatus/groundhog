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
    /// Altitude above mean sea level, metres.
    pub altitude_msl: Option<f32>,
    revision: u64,
}

#[derive(Clone, Debug)]
pub struct GlobalPosition {
    pub at: Instant,
    /// Altitude above home, metres.
    pub alt_agl: f32,
    pub pos: GeoPosition,
}

impl PositionTrack {
    /// Bumped on every new fix, for caches keyed on the trail.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Horizontal speed over ground, metres per second.
    /// The rate of climb, in metres per second (negative while descending).
    pub fn climb_rate(&self) -> Option<f32> {
        self.velocity_ned.map(|velocity| -velocity.z)
    }

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
            pos,
            alt_agl: message.relative_alt as f32 / 1000.0,
        });
        self.velocity_ned = Some(Vector3::new(
            message.vx as f32 / 100.,
            message.vy as f32 / 100.,
            message.vz as f32 / 100.,
        ));
        self.altitude_msl = Some(message.alt as f32 / 1000.);
    }

    /// Fold a position report whose coordinates the caller has decoded, so
    /// that a report without a location is never folded.
    pub(crate) fn observe_global_position_int_cov(
        &mut self,
        message: &messages::GlobalPositionIntCov,
        pos: GeoPosition,
        time: Instant,
    ) {
        self.register(GlobalPosition {
            at: time,
            pos,
            alt_agl: message.relative_alt as f32 / 1000.0,
        });
        self.velocity_ned = Some(Vector3::new(message.vx, message.vy, message.vz));
        self.altitude_msl = Some(message.alt as f32 / 1000.);
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
            if eval.at.saturating_duration_since(prev.at) > Duration::from_secs(20) {
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
            alt_agl: 0.0,
            pos: GeoPosition::new(lat, lon),
        }
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
}
