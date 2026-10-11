//! Fleet-scoped entry point to the vehicle operations the UI drives.
//!
//! The application owns one [`MavlinkRouter`] and wraps it here, so features can
//! start transactions against any vehicle without naming the middleware.

use crate::{foundation::MavlinkId, mavlink::MavlinkRouter};

use super::VehicleClient;

/// A handle to the vehicle operations available to the application.
///
/// Wraps the MAVLink router so the UI layer can start transactions without
/// depending on the middleware. Cheap to clone.
#[derive(Clone)]
pub struct FleetServices {
    router: MavlinkRouter,
}

impl FleetServices {
    pub fn new(router: &MavlinkRouter) -> Self {
        Self {
            router: router.clone(),
        }
    }

    /// A client for the vehicle at the given address.
    pub fn vehicle(&self, target: MavlinkId) -> VehicleClient {
        VehicleClient::new(&self.router, target)
    }
}
