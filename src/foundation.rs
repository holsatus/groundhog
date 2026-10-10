//! Small shared value types used across layers.
//!
//! Anything in here is deliberately tiny and dependency-free so that the
//! domain, transport, and UI layers can all agree on it without creating a
//! dependency between one another.

use std::sync::Arc;

pub type ArcError = Arc<dyn std::error::Error + Send + Sync + 'static>;
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Identifies a MAVLink system/component pair.
///
/// A component of `0` is treated as a wildcard in the direction of a target
/// (see `mav::matches_target`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MavlinkId {
    pub system: u8,
    pub component: u8,
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

    #[allow(dead_code)] // Retained accessor; links are keyed by id, not name
    pub fn str(&self) -> &str {
        &self.0
    }
}
