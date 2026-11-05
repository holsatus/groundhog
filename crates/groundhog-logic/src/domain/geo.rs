//! Geographic coordinates in degrees, free of any map-projection concerns.
//!
//! Positions are WGS-84 `(lat, lon)` degrees. Turning them into map
//! coordinates (Web Mercator) is a frontend's job, and into the integer
//! coordinates of MAVLink messages the wire codecs' (`wire`), so this module
//! knows neither.

/// A WGS-84 geographic position in degrees.
///
/// The constructor takes `(lat, lon)`. Beware that the map layer's geodetic
/// constructor takes `(lon, lat)` — the opposite order — so convert with care.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPosition {
    /// Latitude in degrees, positive north.
    pub lat: f64,
    /// Longitude in degrees, positive east.
    pub lon: f64,
}

/// The mean radius of the Earth, in metres.
const EARTH_RADIUS_M: f64 = 6_371_000.0;

impl GeoPosition {
    /// Create a position from latitude and longitude, both in degrees.
    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }

    /// The great-circle distance to `other`, in metres.
    pub fn distance_to(self, other: GeoPosition) -> f64 {
        let (lat_a, lat_b) = (self.lat.to_radians(), other.lat.to_radians());
        let d_lat = lat_b - lat_a;
        let d_lon = (other.lon - self.lon).to_radians();

        let a =
            (d_lat / 2.0).sin().powi(2) + lat_a.cos() * lat_b.cos() * (d_lon / 2.0).sin().powi(2);
        2.0 * EARTH_RADIUS_M * a.sqrt().asin()
    }

    /// The initial bearing towards `other`, in degrees clockwise from north
    /// (`0..360`).
    pub fn bearing_to(self, other: GeoPosition) -> f64 {
        let (lat_a, lat_b) = (self.lat.to_radians(), other.lat.to_radians());
        let d_lon = (other.lon - self.lon).to_radians();

        let y = d_lon.sin() * lat_b.cos();
        let x = lat_a.cos() * lat_b.sin() - lat_a.sin() * lat_b.cos() * d_lon.cos();
        y.atan2(x).to_degrees().rem_euclid(360.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances_and_bearings_follow_the_compass() {
        let origin = GeoPosition::new(55.0, 10.0);

        // One minute of latitude is about a nautical mile.
        let north = GeoPosition::new(55.0 + 1.0 / 60.0, 10.0);
        assert!((origin.distance_to(north) - 1853.0).abs() < 5.0);
        assert!(origin.bearing_to(north).abs() < 1e-6);

        let east = GeoPosition::new(55.0, 10.01);
        assert!((origin.bearing_to(east) - 90.0).abs() < 0.01);
        let west = GeoPosition::new(55.0, 9.99);
        assert!((origin.bearing_to(west) - 270.0).abs() < 0.01);

        assert_eq!(origin.distance_to(origin), 0.0);
    }
}
