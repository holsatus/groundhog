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

const MANUAL: FlightMode = m("Manual", K::Manual, MAIN_MANUAL, 0);
const STABILIZED: FlightMode = m("Stabilized", K::Stabilized, MAIN_STABILIZED, 0);
const ACRO: FlightMode = m("Acro", K::Other, MAIN_ACRO, 0);
const ALTITUDE: FlightMode = m("Altitude", K::AltitudeHold, MAIN_ALTCTL, 0);
const POSITION: FlightMode = m("Position", K::PositionHold, MAIN_POSCTL, 0);
const OFFBOARD: FlightMode = m("Offboard", K::Guided, MAIN_OFFBOARD, 0);

const READY: FlightMode = FlightMode::reported("Ready", K::Other, encode(MAIN_AUTO, AUTO_READY));
const TAKEOFF: FlightMode = m("Takeoff", K::Takeoff, MAIN_AUTO, AUTO_TAKEOFF);
const HOLD: FlightMode = m("Hold", K::Loiter, MAIN_AUTO, AUTO_LOITER);
const MISSION: FlightMode = m("Mission", K::Mission, MAIN_AUTO, AUTO_MISSION);
const RETURN: FlightMode = m("Return", K::ReturnToLaunch, MAIN_AUTO, AUTO_RTL);
const LAND: FlightMode = m("Land", K::Land, MAIN_AUTO, AUTO_LAND);
const FOLLOW_ME: FlightMode = m("Follow Me", K::Other, MAIN_AUTO, AUTO_FOLLOW_TARGET);
const PRECISION_LAND: FlightMode = m("Precision Land", K::Land, MAIN_AUTO, AUTO_PRECLAND);
const VTOL_TAKEOFF: FlightMode = m("VTOL Takeoff", K::Takeoff, MAIN_AUTO, AUTO_VTOL_TAKEOFF);

/// The modes every PX4 vehicle offers: no auto mode assumes a kind of vehicle.
const BASIC: &[FlightMode] = &[MANUAL, STABILIZED, ACRO, ALTITUDE, POSITION, OFFBOARD];

const COPTER: &[FlightMode] = &[
    MANUAL,
    STABILIZED,
    ACRO,
    ALTITUDE,
    POSITION,
    OFFBOARD,
    READY,
    TAKEOFF,
    HOLD,
    MISSION,
    RETURN,
    LAND,
    FOLLOW_ME,
    PRECISION_LAND,
];

const FIXED_WING: &[FlightMode] = &[
    MANUAL, STABILIZED, ACRO, ALTITUDE, POSITION, OFFBOARD, READY, TAKEOFF, HOLD, MISSION, RETURN,
    LAND,
];

const VTOL: &[FlightMode] = &[
    MANUAL,
    STABILIZED,
    ACRO,
    ALTITUDE,
    POSITION,
    OFFBOARD,
    READY,
    TAKEOFF,
    HOLD,
    MISSION,
    RETURN,
    LAND,
    PRECISION_LAND,
    VTOL_TAKEOFF,
];

const ROVER: &[FlightMode] = &[
    MANUAL, STABILIZED, ACRO, ALTITUDE, POSITION, OFFBOARD, READY, HOLD, MISSION, RETURN,
];

const SUBMARINE: &[FlightMode] = &[MANUAL, STABILIZED, ACRO, ALTITUDE, POSITION, OFFBOARD];

/// Every mode of every class, so a mode the vehicle reports outside its own
/// class's table still gets its name.
const ALL: &[FlightMode] = &[
    MANUAL,
    STABILIZED,
    ACRO,
    ALTITUDE,
    POSITION,
    OFFBOARD,
    READY,
    TAKEOFF,
    HOLD,
    MISSION,
    RETURN,
    LAND,
    FOLLOW_ME,
    PRECISION_LAND,
    VTOL_TAKEOFF,
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
        match class {
            VehicleClass::Copter => COPTER,
            VehicleClass::FixedWing => FIXED_WING,
            VehicleClass::Vtol => VTOL,
            VehicleClass::Rover | VehicleClass::Boat => ROVER,
            VehicleClass::Submarine => SUBMARINE,
            VehicleClass::Airship | VehicleClass::Other => BASIC,
            VehicleClass::AntennaTracker | VehicleClass::NotAVehicle => &[],
        }
    }

    fn decode_mode(
        &self,
        _class: VehicleClass,
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

        ALL.iter()
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

    fn of_kind(class: VehicleClass, kind: K) -> Option<FlightMode> {
        Px4.modes(class)
            .iter()
            .find(|mode| mode.kind == kind)
            .copied()
    }

    #[test]
    fn each_class_offers_only_what_it_can_fly() {
        for class in [VehicleClass::Copter, VehicleClass::Vtol] {
            assert!(of_kind(class, K::Land).is_some(), "{class:?} lands");
            assert!(of_kind(class, K::Takeoff).is_some(), "{class:?} takes off");
        }

        assert!(
            Px4.modes(VehicleClass::Copter)
                .iter()
                .any(|mode| mode.name == "Precision Land"),
            "a multicopter precision-lands"
        );
        assert!(
            Px4.modes(VehicleClass::Vtol)
                .iter()
                .any(|mode| mode.name == "Precision Land"),
            "a VTOL precision-lands"
        );

        for class in [VehicleClass::Rover, VehicleClass::Boat] {
            assert!(
                of_kind(class, K::Takeoff).is_none(),
                "{class:?} cannot take off"
            );
            assert!(of_kind(class, K::Land).is_none(), "{class:?} cannot land");
            assert!(
                !Px4.modes(class)
                    .iter()
                    .any(|mode| mode.name == "Precision Land"),
                "{class:?} cannot precision-land"
            );
            assert!(
                of_kind(class, K::Mission).is_some(),
                "{class:?} flies missions"
            );
        }

        // A fixed wing takes off and lands, but has no VTOL-only mode.
        assert!(of_kind(VehicleClass::FixedWing, K::Land).is_some());
        assert!(
            !Px4.modes(VehicleClass::FixedWing)
                .iter()
                .any(|mode| mode.name == "VTOL Takeoff")
        );
        assert!(
            Px4.modes(VehicleClass::Vtol)
                .iter()
                .any(|mode| mode.name == "VTOL Takeoff")
        );

        // A submarine has only the modes that do not assume a kind of vehicle.
        for kind in [K::Takeoff, K::Land, K::Mission, K::ReturnToLaunch] {
            assert!(
                of_kind(VehicleClass::Submarine, kind).is_none(),
                "a submarine has no {kind:?}"
            );
        }
        assert!(of_kind(VehicleClass::Submarine, K::PositionHold).is_some());
    }

    /// Naming a reported mode does not depend on the class's table.
    #[test]
    fn a_reported_mode_outside_the_classs_table_still_decodes() {
        let mission = Px4
            .decode_mode(
                VehicleClass::Rover,
                MavModeFlag::CUSTOM_MODE_ENABLED,
                encode(MAIN_AUTO, AUTO_PRECLAND),
            )
            .expect("the union of the tables names it");

        assert_eq!(mission.kind, K::Land);
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
