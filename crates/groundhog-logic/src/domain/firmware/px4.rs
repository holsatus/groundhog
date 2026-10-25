//! PX4.
//!
//! PX4 packs its mode into `custom_mode` as `main << 16 | sub << 24`, where the
//! sub mode only matters for the `AUTO` main mode. See
//! <https://github.com/PX4/PX4-Autopilot/blob/main/src/modules/commander/px4_custom_mode.h>.

use super::{Firmware, FlightMode, ModeKind as K, SetMode};
use crate::{
    domain::{
        calibration::{CalibrationProtocol, px4::Px4Calibration},
        vehicle::class::VehicleClass,
    },
    wire::param::ParamEncoding,
};
use mavio::default_dialect::enums::MavModeFlag;

/// The PX4 adapter.
pub(crate) struct Px4;

const MAIN_MANUAL: u32 = 1;
const MAIN_ALTCTL: u32 = 2;
const MAIN_POSCTL: u32 = 3;
const MAIN_AUTO: u32 = 4;
const MAIN_ACRO: u32 = 5;
const MAIN_OFFBOARD: u32 = 6;
const MAIN_STABILIZED: u32 = 7;

const AUTO_READY: u32 = 1;
const AUTO_TAKEOFF: u32 = 2;
const AUTO_LOITER: u32 = 3;
const AUTO_MISSION: u32 = 4;
const AUTO_RTL: u32 = 5;
const AUTO_LAND: u32 = 6;
const AUTO_FOLLOW_TARGET: u32 = 8;
const AUTO_PRECLAND: u32 = 9;
const AUTO_VTOL_TAKEOFF: u32 = 10;

const fn encode(main: u32, sub: u32) -> u32 {
    (main << 16) | (sub << 24)
}

const fn m(name: &'static str, kind: K, main: u32, sub: u32) -> FlightMode {
    FlightMode::new(name, kind, encode(main, sub))
}

const MODES: &[FlightMode] = &[
    m("Manual", K::Manual, MAIN_MANUAL, 0),
    m("Stabilized", K::Stabilized, MAIN_STABILIZED, 0),
    m("Acro", K::Other, MAIN_ACRO, 0),
    m("Altitude", K::AltitudeHold, MAIN_ALTCTL, 0),
    m("Position", K::PositionHold, MAIN_POSCTL, 0),
    m("Offboard", K::Guided, MAIN_OFFBOARD, 0),
    FlightMode::reported("Ready", K::Other, encode(MAIN_AUTO, AUTO_READY)),
    m("Takeoff", K::Takeoff, MAIN_AUTO, AUTO_TAKEOFF),
    m("Hold", K::Loiter, MAIN_AUTO, AUTO_LOITER),
    m("Mission", K::Mission, MAIN_AUTO, AUTO_MISSION),
    m("Return", K::ReturnToLaunch, MAIN_AUTO, AUTO_RTL),
    m("Land", K::Land, MAIN_AUTO, AUTO_LAND),
    m("Follow Me", K::Other, MAIN_AUTO, AUTO_FOLLOW_TARGET),
    m("Precision Land", K::Land, MAIN_AUTO, AUTO_PRECLAND),
    m("VTOL Takeoff", K::Takeoff, MAIN_AUTO, AUTO_VTOL_TAKEOFF),
];

/// The main and sub mode packed into a `custom_mode`.
fn split(custom_mode: u32) -> (u32, u32) {
    ((custom_mode >> 16) & 0xff, (custom_mode >> 24) & 0xff)
}

impl Firmware for Px4 {
    fn name(&self) -> &'static str {
        "PX4"
    }

    fn modes(&self, class: VehicleClass) -> &'static [FlightMode] {
        if class.is_vehicle() { MODES } else { &[] }
    }

    fn decode_mode(
        &self,
        class: VehicleClass,
        base_mode: MavModeFlag,
        custom_mode: u32,
    ) -> Option<FlightMode> {
        if !base_mode.contains(MavModeFlag::CUSTOM_MODE_ENABLED) {
            return None;
        }

        // Only AUTO distinguishes sub modes; elsewhere the sub byte is noise.
        let (main, sub) = split(custom_mode);
        let normalized = if main == MAIN_AUTO {
            encode(main, sub)
        } else {
            encode(main, 0)
        };

        self.modes(class)
            .iter()
            .find(|mode| mode.custom_mode == normalized)
            .copied()
    }

    fn set_mode(&self, mode: &FlightMode) -> SetMode {
        let (main, sub) = split(mode.custom_mode);
        SetMode {
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED.bits() as f32,
            custom_mode: main as f32,
            custom_sub_mode: sub as f32,
        }
    }

    /// PX4 encodes parameters bytewise (and advertises it).
    fn param_encoding(&self) -> Option<ParamEncoding> {
        Some(ParamEncoding::Bytewise)
    }

    fn calibration(&self) -> Option<&'static dyn CalibrationProtocol> {
        Some(&Px4Calibration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_sub_modes_decode() {
        let mode = Px4
            .decode_mode(
                VehicleClass::Copter,
                MavModeFlag::CUSTOM_MODE_ENABLED,
                encode(MAIN_AUTO, AUTO_RTL),
            )
            .unwrap();
        assert_eq!(mode.kind, K::ReturnToLaunch);
    }

    #[test]
    fn the_sub_byte_of_a_manual_mode_is_ignored() {
        let mode = Px4
            .decode_mode(
                VehicleClass::Copter,
                MavModeFlag::CUSTOM_MODE_ENABLED,
                encode(MAIN_POSCTL, 3),
            )
            .unwrap();
        assert_eq!(mode.name, "Position");
    }

    #[test]
    fn set_mode_splits_main_and_sub() {
        let mission = *Px4
            .modes(VehicleClass::Copter)
            .iter()
            .find(|mode| mode.kind == K::Mission)
            .unwrap();
        let command = Px4.set_mode(&mission);
        assert_eq!(command.custom_mode, MAIN_AUTO as f32);
        assert_eq!(command.custom_sub_mode, AUTO_MISSION as f32);
    }
}
