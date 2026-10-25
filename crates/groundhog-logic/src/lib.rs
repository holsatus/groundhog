//! Groundhog's backend: every piece of vehicle state and every rule about it.
//!
//! A frontend starts a [`Backend`] and from then on reads immutable fleet
//! snapshots and asks for operations; it never sees a frame, a transport or
//! the router. What it may use:
//!
//! - [`SystemId`] and [`MavlinkId`]: who a vehicle is, and how to address one
//!   of its components;
//! - [`domain`]: vehicle state as the snapshots hold it, positions, missions,
//!   parameters, firmware adapters and calibration protocols;
//! - [`services`]: the handles that read the fleet and operate vehicles and
//!   links;
//! - [`link`]: how a link is identified and described;
//! - [`storage`]: file formats for missions and parameters;
//! - [`persist`]: writing a frontend's own files (its configuration) safely.
//!
//! The MAVLink middleware (`mavlink`), the transports (inside `link`) and the
//! wire codecs (`wire`) are private to this crate.

// Public items in private modules are invisible to the dead-code and
// visibility checks the seam relies on; this makes them visible.
#![warn(unreachable_pub)]

mod address;
pub mod domain;
pub mod link;
mod mavlink;
pub mod persist;
pub mod services;
pub mod storage;
mod wire;

pub use address::{MavlinkId, SystemId};
pub use services::{Backend, BackendEvents, BackendOptions};

/// A transfer's progress: `done` of `total` units. Shared by the middleware,
/// the services and the frontends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}
