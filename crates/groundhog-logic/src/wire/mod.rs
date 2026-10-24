//! Pure MAVLink wire codecs shared across layers.
//!
//! This module owns the lossless conversions between typed values and their
//! MAVLink wire representations. It depends only on `mav_param` and `mavio`:
//! the domain folds raw messages into typed state using these codecs, and the
//! middleware uses them to encode outbound parameters, so neither layer has to
//! reach into the other for wire logic.

pub mod param;

/// Scale between WGS-84 degrees and the integer coordinates of
/// `GLOBAL_POSITION_INT`, `HOME_POSITION` and `MISSION_ITEM_INT` (all in
/// degrees × 1e7).
pub const LATLON_SCALE: f64 = 1e-7;

/// Degrees `(lat, lon)` from the integer coordinates of a reported position,
/// or `None` for exactly zero, which autopilots send while they have no
/// location: ArduPilot reports the zeroed location it starts with until its
/// estimator has an origin, and treats zero as "never had a position" itself.
pub fn latlon(lat: i32, lon: i32) -> Option<(f64, f64)> {
    (lat != 0 || lon != 0).then_some((lat as f64 * LATLON_SCALE, lon as f64 * LATLON_SCALE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latlon_keeps_latitude_and_longitude_apart() {
        let (lat, lon) = latlon(474_000_000, 85_000_000).expect("a location");
        assert!((lat - 47.4).abs() < 1e-7, "lat was {lat}");
        assert!((lon - 8.5).abs() < 1e-7, "lon was {lon}");
    }

    #[test]
    fn latlon_reads_zero_as_no_location() {
        assert_eq!(latlon(0, 0), None);
        assert!(latlon(0, 1).is_some() && latlon(1, 0).is_some());
    }
}
