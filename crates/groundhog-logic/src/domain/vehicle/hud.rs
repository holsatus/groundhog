//! What the vehicle reports for a head-up display (`VFR_HUD`): its airspeed
//! and throttle.
//!
//! The message also carries heading, ground speed, altitude and climb rate,
//! which the vehicle already reports more precisely elsewhere (`ATTITUDE`,
//! `GLOBAL_POSITION_INT`), so only what is unique to it is kept.
//!
//! The airspeed is filled in whether or not the vehicle has an airspeed
//! sensor: without one, the autopilot sends an estimate from its ground speed
//! and wind. [`Vehicle::airspeed`](super::Vehicle::airspeed) tells the two
//! apart.

use mavio::default_dialect::messages;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Hud {
    /// Speed through the air, metres per second, measured or estimated.
    pub airspeed: Option<f32>,
    /// Throttle, percent of its range.
    pub throttle: Option<u16>,
}

impl Hud {
    pub(crate) fn observe_vfr_hud(&mut self, message: &messages::VfrHud) {
        // A sender without an estimate may send NaN.
        self.airspeed = message.airspeed.is_finite().then_some(message.airspeed);
        self.throttle = Some(message.throttle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airspeed_and_throttle_are_kept() {
        let mut hud = Hud::default();
        hud.observe_vfr_hud(&messages::VfrHud {
            airspeed: 18.5,
            groundspeed: 21.0,
            throttle: 64,
            ..Default::default()
        });

        assert_eq!(hud.airspeed, Some(18.5));
        assert_eq!(hud.throttle, Some(64));
    }

    #[test]
    fn an_airspeed_that_is_not_a_number_is_unknown() {
        let mut hud = Hud {
            airspeed: Some(12.0),
            throttle: None,
        };
        hud.observe_vfr_hud(&messages::VfrHud {
            airspeed: f32::NAN,
            throttle: 10,
            ..Default::default()
        });

        assert_eq!(hud.airspeed, None, "the earlier value is not kept");
        assert_eq!(hud.throttle, Some(10));
    }
}
