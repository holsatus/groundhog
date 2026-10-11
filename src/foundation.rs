//! Small shared value types used across layers.
//!
//! Anything in here is deliberately tiny and dependency-free so that the
//! domain, transport, and UI layers can all agree on it without creating a
//! dependency between one another.

use std::sync::Arc;

pub type ArcError = Arc<dyn std::error::Error + Send + Sync + 'static>;
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A MAVLink *address*: the system/component pair a frame is sent to or comes
/// from.
///
/// This is transport-level addressing, not identity: every command, mission
/// transfer, and parameter operation is addressed to a specific component of a
/// system. A component of `0` is treated as a wildcard in the direction of a
/// target (see `mav::matches_target`). For the identity of a *vehicle*, use
/// [`SystemId`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MavlinkId {
    pub system: u8,
    pub component: u8,
}

/// Identifies a MAVLink system — i.e. a vehicle.
///
/// This is the fleet's identity type. It is deliberately distinct from
/// [`MavlinkId`], which is an *address* (system + component) used for sending
/// and receiving. A vehicle is a system; components are endpoints within it.
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

/// The GCS's default MAVLink identity, stamped on outbound frames.
pub const DEFAULT_GCS_ID: MavlinkId = MavlinkId {
    system: 255,
    component: 1,
};

/// The name of a configured link. Cheap to clone and usable as a map key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(Arc<str>);

impl LinkId {
    pub fn new(id: &str) -> Self {
        LinkId(Arc::from(id))
    }
}
