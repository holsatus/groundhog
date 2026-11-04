//! The application's entry point to the fleet: the latest snapshot, and
//! clients for operating vehicles.

use std::sync::Arc;

use tokio::sync::{mpsc, watch};

use super::{VehicleClient, runtime::Input};
use crate::{MavlinkId, SystemId, domain::fleet::FleetSnapshot, mavlink::MavlinkRouter};

/// A handle to the fleet runtime. Cheap to clone; every clone sees the same
/// fleet. The runtime stops with its [`Backend`](super::Backend); a handle
/// that outlives it keeps the last snapshot, and `wait_for` returns `false`.
#[derive(Clone)]
pub struct FleetHandle {
    router: MavlinkRouter,
    inputs: mpsc::UnboundedSender<Input>,
    snapshot: watch::Receiver<Arc<FleetSnapshot>>,
}

impl FleetHandle {
    pub(super) fn new(
        router: MavlinkRouter,
        inputs: mpsc::UnboundedSender<Input>,
        snapshot: watch::Receiver<Arc<FleetSnapshot>>,
    ) -> Self {
        Self {
            router,
            inputs,
            snapshot,
        }
    }

    /// The fleet as last published.
    pub fn snapshot(&self) -> Arc<FleetSnapshot> {
        self.snapshot.borrow().clone()
    }

    /// Wait until `system` appears in a published snapshot, for at most
    /// `timeout`. Returns whether it did.
    pub async fn wait_for(&self, system: SystemId, timeout: std::time::Duration) -> bool {
        self.wait_until(timeout, |fleet| fleet.contains(system))
            .await
    }

    /// Wait until a published snapshot, the current one included, satisfies
    /// `done`, for at most `timeout`. Returns whether one did.
    pub(super) async fn wait_until(
        &self,
        timeout: std::time::Duration,
        done: impl FnMut(&Arc<FleetSnapshot>) -> bool,
    ) -> bool {
        let mut snapshot = self.snapshot.clone();
        let reached = snapshot.wait_for(done);
        matches!(tokio::time::timeout(timeout, reached).await, Ok(Ok(_)))
    }

    /// A client for the vehicle `system`, addressing its autopilot.
    pub fn vehicle(&self, system: SystemId) -> VehicleClient {
        VehicleClient::new(self.clone(), system, None)
    }

    /// A client for one component of a vehicle, for per-component operations
    /// such as parameters.
    pub fn component(&self, address: MavlinkId) -> VehicleClient {
        VehicleClient::new(self.clone(), address.system_id(), Some(address.component))
    }

    pub(super) fn router(&self) -> &MavlinkRouter {
        &self.router
    }

    pub(super) fn report(&self, input: Input) {
        // Once the backend is dropped and its runtime stopped, there is
        // nothing left to apply this to.
        drop(self.inputs.send(input));
    }
}
