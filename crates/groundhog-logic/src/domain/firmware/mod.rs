//! Firmware adapters: everything that differs between autopilot stacks.
//!
//! The rest of the application never matches on an autopilot. It asks the
//! vehicle's [`Firmware`] instead, resolved once from the heartbeat's
//! `autopilot` field:
//!
//! - how `custom_mode` maps to a named [`FlightMode`], which modes an operator
//!   may select on a given vehicle, and how to command one ([`SetMode`]);
//! - how parameter values are encoded when the vehicle does not advertise it;
//! - how the mission is laid out on the wire ([`MissionLayout`]);
//! - how sensors are calibrated ([`CalibrationProtocol`]).
//!
//! Supporting another autopilot means implementing [`Firmware`] and adding it
//! to `for_autopilot`; nothing else changes.

pub mod ardupilot;
mod generic;
pub mod px4;

use mavio::default_dialect::enums::{MavAutopilot, MavModeFlag};

use crate::{
    domain::{
        calibration::CalibrationProtocol, parameter::Parameters, vehicle::class::VehicleClass,
    },
    wire::param::ParamEncoding,
};

/// What a flight mode means, independent of how a firmware numbers it.
///
/// Lets the UI offer "Return" or "Land" without knowing that ArduPilot calls
/// it `RTL` and PX4 calls it `AUTO.RTL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeKind {
    Manual,
    Stabilized,
    AltitudeHold,
    PositionHold,
    Loiter,
    Mission,
    Guided,
    ReturnToLaunch,
    Land,
    Takeoff,
    Other,
}

/// A firmware's flight mode: its display name and wire encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlightMode {
    pub name: &'static str,
    pub kind: ModeKind,
    /// The heartbeat `custom_mode` this mode reports as.
    pub custom_mode: u32,
    /// Whether an operator may select it. Some modes are only ever reported
    /// (initialising, or modes entered through other means).
    pub selectable: bool,
}

impl FlightMode {
    pub const fn new(name: &'static str, kind: ModeKind, custom_mode: u32) -> Self {
        Self {
            name,
            kind,
            custom_mode,
            selectable: true,
        }
    }

    /// A mode that is reported but never offered for selection.
    pub const fn reported(name: &'static str, kind: ModeKind, custom_mode: u32) -> Self {
        Self {
            selectable: false,
            ..Self::new(name, kind, custom_mode)
        }
    }
}

impl std::fmt::Display for FlightMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

/// The parameters of `MAV_CMD_DO_SET_MODE` that select a mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetMode {
    /// `param1`: the base mode flags.
    pub base_mode: f32,
    /// `param2`: the custom (main) mode.
    pub custom_mode: f32,
    /// `param3`: the custom sub mode.
    pub custom_sub_mode: f32,
}

/// How a firmware lays out a mission on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MissionLayout {
    /// The firmware reserves mission sequence 0 for the home position
    /// (ArduPilot). Plan item `i` then travels as sequence `i + 1`, and jump
    /// targets and `MISSION_CURRENT` are offset accordingly.
    pub home_slot: bool,
}

impl MissionLayout {
    /// The wire sequence of the first plan item.
    pub fn offset(self) -> u16 {
        u16::from(self.home_slot)
    }

    /// The wire sequence of plan item `index`.
    pub(crate) fn wire_seq(self, index: usize) -> u16 {
        index as u16 + self.offset()
    }

    /// The plan index of wire sequence `seq`, or `None` for the home slot.
    pub(crate) fn plan_index(self, seq: u16) -> Option<usize> {
        seq.checked_sub(self.offset()).map(usize::from)
    }
}

/// Everything that differs between autopilot stacks.
pub trait Firmware: Send + Sync + 'static {
    /// A short human-readable name, such as `"ArduPilot"`.
    fn name(&self) -> &'static str;

    /// Every mode this firmware knows for vehicles of `class`.
    fn modes(&self, class: VehicleClass) -> &'static [FlightMode];

    /// The mode a heartbeat reports, if known.
    fn decode_mode(
        &self,
        class: VehicleClass,
        base_mode: MavModeFlag,
        custom_mode: u32,
    ) -> Option<FlightMode> {
        if !base_mode.contains(MavModeFlag::CUSTOM_MODE_ENABLED) {
            return None;
        }

        self.modes(class)
            .iter()
            .find(|mode| mode.custom_mode == custom_mode)
            .copied()
    }

    /// The `MAV_CMD_DO_SET_MODE` parameters that select `mode`.
    fn set_mode(&self, mode: &FlightMode) -> SetMode {
        SetMode {
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED.bits() as f32,
            custom_mode: mode.custom_mode as f32,
            custom_sub_mode: 0.0,
        }
    }

    /// Whether an operator may select `mode` on a vehicle of `class` whose
    /// autopilot holds `parameters`. Some modes exist only while a feature
    /// is enabled; while the parameters cannot tell, the mode is offered,
    /// since the vehicle refuses a mode it lacks but a hidden one is lost.
    fn offers(
        &self,
        mode: &FlightMode,
        _class: VehicleClass,
        _parameters: Option<&Parameters>,
    ) -> bool {
        mode.selectable
    }

    /// The parameter encoding to assume when the vehicle does not advertise
    /// one in its capabilities, or `None` when it cannot be known.
    fn param_encoding(&self) -> Option<ParamEncoding>;

    /// How this firmware lays out a mission on the wire.
    fn mission_layout(&self) -> MissionLayout {
        MissionLayout::default()
    }

    /// How this firmware runs sensor calibrations, if it is known.
    fn calibration(&self) -> Option<&'static dyn CalibrationProtocol> {
        None
    }
}

/// The adapter for vehicles reporting `autopilot` in their heartbeat.
pub(crate) fn for_autopilot(autopilot: MavAutopilot) -> &'static dyn Firmware {
    match autopilot {
        MavAutopilot::Ardupilotmega => &ardupilot::ArduPilot,
        MavAutopilot::Px4 => &px4::Px4,
        _ => &generic::Generic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_slot_offsets_every_plan_index() {
        let ardupilot = MissionLayout { home_slot: true };
        assert_eq!(ardupilot.wire_seq(0), 1);
        assert_eq!(ardupilot.plan_index(1), Some(0));
        assert_eq!(ardupilot.plan_index(0), None, "seq 0 is home");

        let px4 = MissionLayout { home_slot: false };
        assert_eq!(px4.wire_seq(0), 0);
        assert_eq!(px4.plan_index(0), Some(0));
    }

    #[test]
    fn adapters_resolve_from_the_autopilot() {
        assert_eq!(
            for_autopilot(MavAutopilot::Ardupilotmega).name(),
            "ArduPilot"
        );
        assert_eq!(for_autopilot(MavAutopilot::Px4).name(), "PX4");
        assert_eq!(for_autopilot(MavAutopilot::Generic).name(), "Generic");
    }

    #[test]
    fn modes_need_the_custom_mode_flag() {
        let firmware = for_autopilot(MavAutopilot::Ardupilotmega);
        assert!(
            firmware
                .decode_mode(VehicleClass::Copter, MavModeFlag::empty(), 5)
                .is_none()
        );
        assert_eq!(
            firmware
                .decode_mode(VehicleClass::Copter, MavModeFlag::CUSTOM_MODE_ENABLED, 5)
                .map(|mode| mode.name),
            Some("Loiter")
        );
    }
}
