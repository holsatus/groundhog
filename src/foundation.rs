//! The shared wire address, plus the common error aliases.
//!
//! [`MavlinkId`] is the one value type deliberately shared across layers, and
//! it is shared as an *address*, not as an identity. The domain uses it as "an
//! address to target" — every command, mission transfer, and parameter
//! operation names a [`MavlinkId`] (see
//! [`Vehicle::target`](crate::domain::vehicle::Vehicle::target)) — while the
//! transports use it to send and receive. A vehicle's *identity*, by contrast,
//! is the domain's own identity type, which lives with the domain and is never
//! needed on the wire.
//!
//! The error aliases are the shared carriers for boxed and reference-counted
//! errors crossing layer boundaries.

use std::sync::Arc;

pub type ArcError = Arc<dyn std::error::Error + Send + Sync + 'static>;

/// A MAVLink *address*: the system/component pair a frame is sent to or comes
/// from.
///
/// This is transport-level addressing, not identity: every command, mission
/// transfer, and parameter operation is addressed to a specific component of a
/// system. A component of `0` is treated as a wildcard in the direction of a
/// target (see `crate::mavlink::matches_target`). For the identity of a
/// *vehicle*, see the domain's own identity type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MavlinkId {
    pub system: u8,
    pub component: u8,
}
