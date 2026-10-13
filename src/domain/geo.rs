//! Geographic coordinates in degrees, free of any map-projection concerns.
//!
//! The domain persists positions as WGS-84 `(lat, lon)` degrees. Turning those
//! into map coordinates (Web Mercator) is the view layer's job, so this module
//! deliberately does not depend on the map/tile crate.

/// Scale between WGS-84 degrees and the integer coordinates of
/// `GLOBAL_POSITION_INT` / `MISSION_ITEM_INT` (both in degrees × 1e7).
pub(crate) const LATLON_SCALE: f64 = 1e-7;

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

impl GeoPosition {
    /// Create a position from latitude and longitude, both in degrees.
    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }
}
