//! The application core: everything between the MAVLink middleware and the UI.
//!
//! - [`runtime`]: the task that owns the fleet and publishes snapshots;
//! - [`FleetHandle`] / [`VehicleClient`]: intent-level vehicle operations,
//!   resolved against the latest snapshot;
//! - [`LinkManager`]: configured links and their supervisors;
//! - [`TlogRecorder`]: telemetry recording.
//!
//! Features use only this module and the domain. No middleware type appears
//! in these APIs, so the middleware stays an implementation detail.

pub mod error;
pub mod fleet;
pub mod link;
pub mod recording;
pub mod runtime;
pub mod vehicle;

pub use error::{Error, Result};
pub use fleet::FleetHandle;
pub use link::{LinkEvent, LinkManager, LinkStatus};
pub use recording::TlogRecorder;
pub use runtime::{FleetUpdate, RuntimeOptions};
pub use vehicle::{ParamListSummary, TransferProgress, VehicleClient};

// Re-exported so features can name a link handle without reaching into the
// `link` layer; only `link::config` types (configs, builds) are imported there.
pub use crate::link::LinkId;

#[cfg(test)]
mod tests;
