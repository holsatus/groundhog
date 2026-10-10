//! Vehicle position: the current fix, the recent trail, and its pruning.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use mavio::default_dialect::messages;
use nalgebra::Vector3;

use crate::domain::geo::GeoPosition;

pub const GLOBAL_POSITION_MAX_LEN: usize = 1000;
pub const GLOBAL_POSITION_MAX_AGE: Duration = Duration::from_secs(60 * 15);

#[derive(Debug, Clone, Default)]
pub struct PositionTrack {
    pub current: Option<GlobalPosition>,
    pub trail: VecDeque<GlobalPosition>,
    pub last_pop_count: usize,
    pub velocity_ned: Option<Vector3<f32>>,
    pub altitude_msl: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct GlobalPosition {
    pub at: Instant,
    pub alt_agl: f32,
    pub pos: GeoPosition,
}

impl PositionTrack {
    pub fn observe_global_position_int(
        &mut self,
        message: &messages::GlobalPositionInt,
        time: Instant,
    ) {
        self.register(GlobalPosition {
            at: time,
            pos: GeoPosition::new(message.lat as f64 * 1e-7, message.lon as f64 * 1e-7),
            alt_agl: message.relative_alt as f32 * 1e-3,
        });
        self.velocity_ned = Some(Vector3::new(
            message.vx as f32 / 100.,
            message.vy as f32 / 100.,
            message.vz as f32 / 100.,
        )); // [cm/s]
        self.altitude_msl = Some(message.alt as f32 / 1000.); // [mm]
    }

    pub fn observe_global_position_int_cov(
        &mut self,
        message: &messages::GlobalPositionIntCov,
        time: Instant,
    ) {
        self.register(GlobalPosition {
            at: time,
            pos: GeoPosition::new(message.lat as f64 * 1e-7, message.lon as f64 * 1e-7),
            alt_agl: message.relative_alt as f32 * 1e-3,
        });
        self.velocity_ned = Some(Vector3::new(
            message.vx / 100.,
            message.vy / 100.,
            message.vz / 100.,
        )); // [cm/s]
        self.altitude_msl = Some(message.alt as f32 / 1000.); // [mm]
    }

    pub fn register(&mut self, position: GlobalPosition) {
        self.current = Some(position.clone());
        self.trail.push_back(position);
        if self.trail.len() > GLOBAL_POSITION_MAX_LEN {
            self.prune_one();
        }
    }

    fn prune_one(&mut self) {
        // Plain closure so the early `return`s below exit the pruning step.
        (|| {
            if self.trail.len() < 3 {
                self.trail.pop_front();
                return;
            }

            if let Some(oldest) = self.trail.front()
                && oldest.at.elapsed() > GLOBAL_POSITION_MAX_AGE
            {
                self.trail.pop_front();
                return;
            }

            let mut prune_idx: Option<(usize, f32)> = None;
            let mut prev = self.trail[0].clone();
            let mut eval = self.trail[1].clone();

            // Enumerate starting from index 2 up to the end of the list
            for (next_idx, next) in self.trail.iter().enumerate().skip(2) {
                // Do not allow the time gap between two points to be too large
                if eval.at.duration_since(prev.at) > Duration::from_secs(20) {
                    prev = eval;
                    eval = next.clone();
                    continue;
                }

                // Shoelace formula triangle area, computed on degrees rather
                // than projected coordinates. Web-Mercator x is a positive
                // constant times longitude, and locally y is an affine function
                // of latitude, so this differs from the projected area only by a
                // constant positive factor: the argmin below — and therefore the
                // pruned point — is unchanged. (The old `* 1e6` y-scaling was a
                // constant factor and has been dropped.)
                let area = (prev.pos.lon * (eval.pos.lat - next.pos.lat)
                    + eval.pos.lon * (next.pos.lat - prev.pos.lat)
                    + next.pos.lon * (prev.pos.lat - eval.pos.lat))
                    .abs();

                // Cost is dynamically weighed by the span of time beteen the outer points
                let span = next.at.saturating_duration_since(prev.at);
                let cost = (area as f32) * (1.0 + span.as_secs_f32());

                match prune_idx {
                    Some((_, other_cost)) => {
                        if cost < other_cost {
                            prune_idx = Some((next_idx - 1, cost));
                        }
                    }
                    None => prune_idx = Some((next_idx - 1, cost)),
                }

                prev = eval;
                eval = next.clone();
            }

            if let Some((idx, _)) = prune_idx {
                self.last_pop_count += 1;
                self.trail.remove(idx);
            } else {
                self.last_pop_count = 0;
                self.trail.pop_front();
            }
        })();
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
    fn global_position_int_stores_degrees_without_swapping_lat_lon() {
        let mut track = PositionTrack::default();

        let message = messages::GlobalPositionInt {
            time_boot_ms: 0,
            // Raw scaled ints: 474_000_000 -> 47.4 deg lat, 85_000_000 -> 8.5 deg lon.
            lat: 474_000_000,
            lon: 85_000_000,
            alt: 150_000,
            relative_alt: 100_000,
            vx: 0,
            vy: 0,
            vz: 0,
            hdg: 0,
        };

        track.observe_global_position_int(&message, Instant::now());

        let pos = track.current.expect("position recorded").pos;
        assert!(
            (pos.lat - 47.4).abs() < 1e-7,
            "lat was {} (expected 47.4)",
            pos.lat
        );
        assert!(
            (pos.lon - 8.5).abs() < 1e-7,
            "lon was {} (expected 8.5)",
            pos.lon
        );
    }

    #[test]
    fn trail_never_exceeds_max_len() {
        let mut track = PositionTrack::default();

        for i in 0..GLOBAL_POSITION_MAX_LEN + 50 {
            track.register(position(
                Instant::now(),
                47.0 + i as f64 * 1e-3,
                8.0 + i as f64 * 1e-3,
            ));
        }

        assert_eq!(track.trail.len(), GLOBAL_POSITION_MAX_LEN);
    }

    #[test]
    fn stale_point_older_than_max_age_is_dropped() {
        let mut track = PositionTrack::default();

        let old = Instant::now() - (GLOBAL_POSITION_MAX_AGE + Duration::from_secs(1));
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
