//! ArduPilot: Copter, Plane (including QuadPlane VTOL), Rover, Sub and
//! AntennaTracker.
//!
//! Mode numbers follow the firmware sources:
//! - <https://github.com/ArduPilot/ardupilot/blob/master/ArduCopter/mode.h>
//! - <https://github.com/ArduPilot/ardupilot/blob/master/ArduPlane/mode.h>
//! - <https://github.com/ArduPilot/ardupilot/blob/master/Rover/mode.h>
//! - <https://github.com/ArduPilot/ardupilot/blob/master/ArduSub/defines.h>
//! - <https://github.com/ArduPilot/ardupilot/blob/master/AntennaTracker/mode.h>

use mav_param::{Ident, Value};

use super::{Firmware, FlightMode, MissionLayout, ModeKind as K};
use crate::{
    domain::{
        calibration::{CalibrationProtocol, ardupilot::ArduPilotCalibration},
        parameter::Parameters,
        vehicle::class::VehicleClass,
    },
    wire::param::ParamEncoding,
};

/// The ArduPilot adapter.
pub(crate) struct ArduPilot;

const fn m(name: &'static str, kind: K, number: u32) -> FlightMode {
    FlightMode::new(name, kind, number)
}

const fn r(name: &'static str, kind: K, number: u32) -> FlightMode {
    FlightMode::reported(name, kind, number)
}

pub(crate) const COPTER: &[FlightMode] = &[
    m("Stabilize", K::Stabilized, 0),
    m("Acro", K::Manual, 1),
    m("AltHold", K::AltitudeHold, 2),
    m("Auto", K::Mission, 3),
    m("Guided", K::Guided, 4),
    m("Loiter", K::Loiter, 5),
    m("RTL", K::ReturnToLaunch, 6),
    m("Circle", K::Other, 7),
    m("Land", K::Land, 9),
    m("Drift", K::Other, 11),
    m("Sport", K::Other, 13),
    m("Flip", K::Other, 14),
    m("AutoTune", K::Other, 15),
    m("PosHold", K::PositionHold, 16),
    m("Brake", K::Other, 17),
    m("Throw", K::Other, 18),
    r("Avoid ADSB", K::Other, 19),
    m("Guided NoGPS", K::Guided, 20),
    m("Smart RTL", K::Other, 21),
    m("FlowHold", K::Other, 22),
    m("Follow", K::Other, 23),
    m("ZigZag", K::Other, 24),
    m("SystemID", K::Other, 25),
    m("Autorotate", K::Other, 26),
    r("Auto RTL", K::ReturnToLaunch, 27),
    m("Turtle", K::Other, 28),
];

pub(crate) const PLANE: &[FlightMode] = &[
    m("Manual", K::Manual, 0),
    m("Circle", K::Other, 1),
    m("Stabilize", K::Stabilized, 2),
    m("Training", K::Other, 3),
    m("Acro", K::Other, 4),
    m("FBWA", K::Stabilized, 5),
    m("FBWB", K::AltitudeHold, 6),
    m("Cruise", K::PositionHold, 7),
    m("AutoTune", K::Other, 8),
    m("Auto", K::Mission, 10),
    m("RTL", K::ReturnToLaunch, 11),
    m("Loiter", K::Loiter, 12),
    m("Takeoff", K::Takeoff, 13),
    r("Avoid ADSB", K::Other, 14),
    m("Guided", K::Guided, 15),
    r("Initialising", K::Other, 16),
    m("QStabilize", K::Stabilized, 17),
    m("QHover", K::AltitudeHold, 18),
    m("QLoiter", K::PositionHold, 19),
    m("QLand", K::Land, 20),
    m("QRTL", K::Other, 21),
    m("QAutoTune", K::Other, 22),
    m("QAcro", K::Other, 23),
    m("Thermal", K::Other, 24),
    m("Loiter to QLand", K::Other, 25),
    m("Autoland", K::Other, 26),
];

/// The custom modes of [`PLANE`] that fly the QuadPlane's lift motors. They
/// exist only while `Q_ENABLE` is set, and a QuadPlane that is neither a
/// tiltrotor nor a tailsitter reports itself as a fixed wing, so only the
/// parameter tells.
const QUADPLANE_MODES: [u32; 8] = [17, 18, 19, 20, 21, 22, 23, 25];

/// Whether the plane's parameters say it has no QuadPlane.
fn quadplane_disabled(parameters: Option<&Parameters>) -> bool {
    let q_enable =
        parameters.and_then(|parameters| parameters.value(&Ident::from_str_truncated("Q_ENABLE")));
    match q_enable {
        Some(Value::U8(v)) => v == 0,
        Some(Value::I8(v)) => v == 0,
        Some(Value::U16(v)) => v == 0,
        Some(Value::I16(v)) => v == 0,
        Some(Value::U32(v)) => v == 0,
        Some(Value::I32(v)) => v == 0,
        Some(Value::F32(v)) => v == 0.0,
        None => false,
    }
}

pub(crate) const ROVER: &[FlightMode] = &[
    m("Manual", K::Manual, 0),
    m("Acro", K::Other, 1),
    m("Steering", K::Stabilized, 3),
    m("Hold", K::PositionHold, 4),
    m("Loiter", K::Loiter, 5),
    m("Follow", K::Other, 6),
    m("Simple", K::Other, 7),
    m("Dock", K::Other, 8),
    m("Circle", K::Other, 9),
    m("Auto", K::Mission, 10),
    m("RTL", K::ReturnToLaunch, 11),
    m("Smart RTL", K::Other, 12),
    m("Guided", K::Guided, 15),
    r("Initialising", K::Other, 16),
];

pub(crate) const SUB: &[FlightMode] = &[
    m("Stabilize", K::Stabilized, 0),
    m("Acro", K::Other, 1),
    m("Depth Hold", K::AltitudeHold, 2),
    m("Auto", K::Mission, 3),
    m("Guided", K::Guided, 4),
    m("Circle", K::Other, 7),
    m("Surface", K::Other, 9),
    m("Position Hold", K::PositionHold, 16),
    m("Manual", K::Manual, 19),
    m("Motor Detect", K::Other, 20),
    m("Surftrak", K::Other, 21),
];

pub(crate) const TRACKER: &[FlightMode] = &[
    m("Manual", K::Manual, 0),
    m("Stop", K::Other, 1),
    m("Scan", K::Other, 2),
    m("Servo Test", K::Other, 3),
    m("Guided", K::Guided, 4),
    m("Auto", K::Mission, 10),
    r("Initialising", K::Other, 16),
];

impl Firmware for ArduPilot {
    fn name(&self) -> &'static str {
        "ArduPilot"
    }

    fn modes(&self, class: VehicleClass) -> &'static [FlightMode] {
        match class {
            VehicleClass::Copter => COPTER,
            VehicleClass::FixedWing | VehicleClass::Vtol => PLANE,
            VehicleClass::Rover | VehicleClass::Boat => ROVER,
            VehicleClass::Submarine => SUB,
            VehicleClass::AntennaTracker => TRACKER,
            VehicleClass::Airship | VehicleClass::Other | VehicleClass::NotAVehicle => &[],
        }
    }

    fn offers(
        &self,
        mode: &FlightMode,
        class: VehicleClass,
        parameters: Option<&Parameters>,
    ) -> bool {
        let plane = matches!(class, VehicleClass::FixedWing | VehicleClass::Vtol);
        mode.selectable
            && !(plane
                && QUADPLANE_MODES.contains(&mode.custom_mode)
                && quadplane_disabled(parameters))
    }

    /// ArduPilot encodes parameters by casting to float; current releases
    /// advertise this, older ones only set `PARAM_FLOAT` (meaning the same).
    fn param_encoding(&self) -> Option<ParamEncoding> {
        Some(ParamEncoding::CCast)
    }

    fn mission_layout(&self) -> MissionLayout {
        MissionLayout { home_slot: true }
    }

    fn calibration(&self) -> Option<&'static dyn CalibrationProtocol> {
        Some(&ArduPilotCalibration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavio::default_dialect::enums::MavModeFlag;

    fn mode_numbers_are_unique(modes: &[FlightMode]) {
        let mut numbers: Vec<_> = modes.iter().map(|mode| mode.custom_mode).collect();
        numbers.sort();
        numbers.dedup();
        assert_eq!(numbers.len(), modes.len());
    }

    #[test]
    fn mode_tables_have_unique_numbers() {
        for table in [COPTER, PLANE, ROVER, SUB, TRACKER] {
            mode_numbers_are_unique(table);
        }
    }

    #[test]
    fn hexacopters_resolve_copter_modes() {
        let mode = ArduPilot
            .decode_mode(
                VehicleClass::of(mavio::default_dialect::enums::MavType::Hexarotor),
                MavModeFlag::CUSTOM_MODE_ENABLED,
                6,
            )
            .expect("hexacopters are copters");
        assert_eq!(mode.kind, K::ReturnToLaunch);
    }

    #[test]
    fn vtol_planes_use_the_plane_table() {
        let mode = ArduPilot
            .decode_mode(VehicleClass::Vtol, MavModeFlag::CUSTOM_MODE_ENABLED, 19)
            .unwrap();
        assert_eq!(mode.name, "QLoiter");
    }

    fn of_kind(class: VehicleClass, kind: K) -> Option<FlightMode> {
        ArduPilot
            .modes(class)
            .iter()
            .find(|mode| mode.kind == kind && ArduPilot.offers(mode, class, None))
            .copied()
    }

    #[test]
    fn standard_kinds_resolve_per_class() {
        assert_eq!(
            of_kind(VehicleClass::Copter, K::Land).unwrap().custom_mode,
            9
        );
        let rtl = of_kind(VehicleClass::Rover, K::ReturnToLaunch).unwrap();
        assert_eq!(rtl.custom_mode, 11);
    }

    #[test]
    fn quadplane_modes_are_the_q_modes() {
        for number in QUADPLANE_MODES {
            let mode = PLANE
                .iter()
                .find(|mode| mode.custom_mode == number)
                .unwrap();
            assert!(mode.name.contains('Q'), "{}", mode.name);
        }
    }
}
