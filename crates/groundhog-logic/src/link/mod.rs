//! Links: how they are described, and the transports that open them.
//!
//! The descriptions ([`LinkId`], [`LinkConfig`], [`LinkBuild`]) are public,
//! for frontends to edit and persist; opening a transport is internal to the
//! backend.

mod config;
pub(crate) mod transport;
pub(crate) mod udp;

pub use config::{LinkBuild, LinkConfig, LinkId};
pub(crate) use transport::open;
