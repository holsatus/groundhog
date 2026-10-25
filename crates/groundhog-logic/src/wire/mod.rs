//! Pure MAVLink wire codecs shared across layers.
//!
//! This module owns the lossless conversions between typed values and their
//! MAVLink wire representations. It depends only on `mav_param` and `mavio`:
//! the domain folds raw messages into typed state using these codecs, and the
//! middleware uses them to encode outbound parameters, so neither layer has to
//! reach into the other for wire logic.

pub(crate) mod param;

/// Integer units per WGS-84 degree in `GLOBAL_POSITION_INT`,
/// `HOME_POSITION` and `MISSION_ITEM_INT`, which count degrees × 1e7. Exact
/// in `f64`, unlike its inverse: conversions divide or multiply by it.
const UNITS_PER_DEGREE: f64 = 1e7;

/// Degrees from a wire coordinate in degrees × 1e7.
pub(crate) fn degrees(e7: i32) -> f64 {
    f64::from(e7) / UNITS_PER_DEGREE
}

/// A wire coordinate in degrees × 1e7 from degrees, to the nearest unit
/// (saturating outside the `i32` range).
pub(crate) fn degrees_e7(degrees: f64) -> i32 {
    (degrees * UNITS_PER_DEGREE).round() as i32
}

/// Degrees `(lat, lon)` from the integer coordinates of a reported position,
/// or `None` for exactly zero, which autopilots send while they have no
/// location: ArduPilot reports the zeroed location it starts with until its
/// estimator has an origin, and treats zero as "never had a position" itself.
pub(crate) fn latlon(lat: i32, lon: i32) -> Option<(f64, f64)> {
    (lat != 0 || lon != 0).then_some((degrees(lat), degrees(lon)))
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
    fn coordinates_convert_exactly_and_round_trip() {
        for e7 in [i32::MAX, i32::MIN, -1, 1, 473_977_418, -1_223_456_789] {
            assert_eq!(degrees(e7), f64::from(e7) / 1e7, "{e7}");
            assert_eq!(degrees_e7(degrees(e7)), e7, "{e7}");
        }
    }

    #[test]
    fn latlon_reads_zero_as_no_location() {
        assert_eq!(latlon(0, 0), None);
        assert!(latlon(0, 1).is_some() && latlon(1, 0).is_some());
    }
}
