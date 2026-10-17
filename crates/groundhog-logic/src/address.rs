//! Who is talking: a vehicle's identity, and the wire address of one of its
//! components.
//!
//! The two are deliberately distinct types. A [`SystemId`] names a vehicle;
//! state, events and operations are keyed by it. A [`MavlinkId`] is where a
//! frame goes or comes from: every command, mission transfer and parameter
//! operation is addressed to one component of a system.

/// Identifies a MAVLink system: a vehicle.
///
/// A vehicle is a system; its components (autopilot, gimbal, companion
/// computer) are endpoints within it, each with its own [`MavlinkId`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SystemId(pub u8);

impl From<u8> for SystemId {
    fn from(system: u8) -> Self {
        SystemId(system)
    }
}

impl From<SystemId> for u8 {
    fn from(id: SystemId) -> Self {
        id.0
    }
}

impl std::fmt::Display for SystemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sys{}", self.0)
    }
}

/// A MAVLink *address*: the system/component pair a frame is sent to or comes
/// from.
///
/// A component of `0` is treated as a wildcard in the direction of a target:
/// it matches every component of the system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MavlinkId {
    pub system: u8,
    pub component: u8,
}

impl MavlinkId {
    /// The vehicle this address belongs to.
    pub fn system_id(self) -> SystemId {
        SystemId(self.system)
    }
}
