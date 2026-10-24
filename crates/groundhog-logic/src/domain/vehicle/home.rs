//! The vehicle's home position.

use mavio::default_dialect::messages;

use crate::{
    domain::{
        geo::GeoPosition,
        mission::{MissionItem, Position},
    },
    wire,
};

/// Where the vehicle returns to, from `HOME_POSITION`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Home {
    pub position: GeoPosition,
    /// Altitude above mean sea level, in metres.
    pub altitude_msl: f32,
}

impl Home {
    /// The home a `HOME_POSITION` reports, unless it reports no location.
    pub(crate) fn from_message(message: &messages::HomePosition) -> Option<Self> {
        let (lat, lon) = wire::latlon(message.latitude, message.longitude)?;
        Some(Home {
            position: GeoPosition::new(lat, lon),
            altitude_msl: message.altitude as f32 / 1000.0,
        })
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
