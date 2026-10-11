//! Intent-level operations the application drives, decoupled from the MAVLink
//! middleware.
//!
//! Features name their result types and vehicle operations through this module
//! rather than [`crate::mavlink`], so the middleware stays an implementation detail
//! of the services layer.

pub mod fleet;
pub mod vehicle;

pub use fleet::FleetServices;
pub use vehicle::VehicleClient;

pub use crate::mavlink::{ParamListSummary, Result};

// Tests build routers and error values directly; expose the underlying types
// through the services layer so feature test modules stay off `crate::mav`.
#[cfg(test)]
pub use crate::mavlink::MavlinkRouter;
#[cfg(test)]
pub use crate::mavlink::error::MavError;
