//! Intent-level operations the application drives, decoupled from the MAVLink
//! middleware.
//!
//! Features name their result types and vehicle operations through this module
//! rather than [`crate::mavlink`], so the middleware stays an implementation detail
//! of the services layer.

pub mod error;
pub mod fleet;
pub mod link;
pub mod vehicle;

pub use error::{Error, Result};
pub use fleet::FleetServices;
pub use link::{LinkManager, LinkStatus, LinkUpdate};
pub use vehicle::{DownloadedMission, ParamListSummary, VehicleClient};

// Re-exported so features can name a link handle without reaching into the
// `link` layer; only `link::config` types (configs, builds) are imported there.
pub use crate::link::LinkId;
