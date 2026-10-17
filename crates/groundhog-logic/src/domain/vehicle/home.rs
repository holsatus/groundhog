//! The vehicle's home position.

use mavio::default_dialect::messages;

use crate::{
    domain::{
        geo::GeoPosition,
        mission::{MissionItem, Position},
    },
    wire::LATLON_SCALE,
};

/// Where the vehicle returns to, from `HOME_POSITION`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Home {
    pub position: GeoPosition,
    /// Altitude above mean sea level, in metres.
    pub altitude_msl: f32,
}

impl Home {
    pub(crate) fn from_message(message: &messages::HomePosition) -> Self {
        Home {
            position: GeoPosition::new(
                message.latitude as f64 * LATLON_SCALE,
                message.longitude as f64 * LATLON_SCALE,
            ),
            altitude_msl: message.altitude as f32 / 1000.0,
        }
    }

    /// The home as a firmware's home-slot mission item.
    pub fn mission_item(&self) -> MissionItem {
        MissionItem::home(Position {
            lat: self.position.lat,
            lon: self.position.lon,
            alt_m: self.altitude_msl,
        })
    }
}
