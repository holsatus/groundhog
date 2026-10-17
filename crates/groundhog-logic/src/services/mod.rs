//! The backend's operations: everything between the MAVLink middleware and a
//! frontend.
//!
//! - [`Backend`]: starts the router, the fleet runtime and the link
//!   supervisors, and owns them;
//! - [`FleetHandle`] / [`VehicleClient`]: the latest snapshot, and
//!   intent-level vehicle operations resolved against it;
//! - [`LinkManager`]: configured links and their supervisors;
//! - [`TlogRecorder`]: telemetry recording.
//!
//! No middleware type appears in these APIs, so the middleware stays an
//! implementation detail. Plain data it produces, such as [`LinkStats`] and
//! [`TransferProgress`], is re-exported here.

mod backend;
mod error;
mod fleet;
mod link;
mod recording;
mod runtime;
mod vehicle;

pub use backend::{Backend, BackendEvents, BackendOptions};
pub use error::{Error, MissionRejection, Rejection, Result};
pub use fleet::FleetHandle;
pub use link::{LinkEvent, LinkEvents, LinkManager, LinkStatus, detect_serial_ports};
pub use recording::TlogRecorder;
pub use runtime::{FleetUpdate, FleetUpdates};
pub use vehicle::{ParamListSummary, TransferProgress, VehicleClient};

pub use crate::mavlink::LinkStats;

#[cfg(test)]
mod tests;
