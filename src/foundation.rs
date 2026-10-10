//! Small shared value types used across layers.
//!
//! Anything in here is deliberately tiny and dependency-free so that the
//! domain, transport, and UI layers can all agree on it without creating a
//! dependency between one another.

use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc,
};

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

/// The name of a configured link. Cheap to clone and usable as a map key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(Arc<str>);

impl LinkId {
    pub fn new(id: &str) -> Self {
        LinkId(Arc::from(id))
    }

    pub fn str(&self) -> &str {
        &self.0
    }
}

/// The GCS's own MAVLink identity, read by the router when stamping outbound
/// frames. Stored as a packed `AtomicU16` (little-endian `[system, component]`).
#[derive(Debug)]
pub struct AtomicMavlinkId(AtomicU16);

impl AtomicMavlinkId {
    pub const fn new(sys: u8, com: u8) -> Self {
        Self(AtomicU16::new(u16::from_le_bytes([sys, com])))
    }

    pub fn load(&self) -> MavlinkId {
        let inner = self.0.load(Ordering::Relaxed);
        let [system, component] = inner.to_le_bytes();
        MavlinkId { system, component }
    }

    #[allow(unused)]
    pub fn store(&self, mav_id: MavlinkId) {
        let inner = u16::from_le_bytes([mav_id.system, mav_id.component]);
        self.0.store(inner, Ordering::Relaxed);
    }
}

pub static GCS_MAVLINK_ID: AtomicMavlinkId = AtomicMavlinkId::new(255, 1);
