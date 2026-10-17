//! Starting the backend: the one place the router, the fleet runtime and the
//! link supervisors are wired together.

use std::path::Path;

use super::{
    FleetHandle, LinkEvents, LinkManager, TlogRecorder,
    runtime::{self, FleetUpdates, RuntimeOptions},
};
use crate::{
    MavlinkId,
    link::LinkConfig,
    mavlink::{self, MavlinkRouter},
};

/// How to start a [`Backend`].
#[derive(Debug, Clone)]
pub struct BackendOptions {
    /// The address this ground station sends from.
    pub gcs_id: MavlinkId,
    /// The configured links. Those marked `auto_connect` connect at start.
    pub links: Vec<LinkConfig>,
    /// Bootstrap newly registered vehicles: request their capabilities and
    /// home, and download their parameters and mission.
    pub bootstrap: bool,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            gcs_id: mavlink::DEFAULT_GCS_ID,
            links: Vec::new(),
            bootstrap: true,
        }
    }
}

/// The running backend: the fleet, the links, and what taps their traffic.
///
/// Dropping it disconnects every link. The fleet runtime keeps running for as
/// long as a [`FleetHandle`] or the [`BackendEvents`] are alive.
pub struct Backend {
    router: MavlinkRouter,
    fleet: FleetHandle,
    links: LinkManager,
}

/// What the backend reports, for a frontend to drive.
pub struct BackendEvents {
    /// The fleet's snapshots and the events between them.
    pub fleet: FleetUpdates,
    /// Every change of a link's status.
    pub links: LinkEvents,
}

impl Backend {
    /// Start the backend on the current tokio runtime, connecting the links
    /// marked for auto-connect.
    pub fn start(options: BackendOptions) -> (Self, BackendEvents) {
        let (router, events) = MavlinkRouter::new(options.gcs_id);
        let (fleet, updates) = runtime::spawn(
            router.clone(),
            events,
            RuntimeOptions {
                bootstrap: options.bootstrap,
            },
        );
        let (mut links, auto_connect, link_events) = LinkManager::new(&router, options.links);
        for id in &auto_connect {
            links.connect(id);
        }

        let backend = Self {
            router,
            fleet,
            links,
        };
        let events = BackendEvents {
            fleet: updates,
            links: link_events,
        };
        (backend, events)
    }

    /// The fleet: its latest snapshot, and clients that operate vehicles.
    pub fn fleet(&self) -> &FleetHandle {
        &self.fleet
    }

    /// The configured links and their status.
    pub fn links(&self) -> &LinkManager {
        &self.links
    }

    /// The configured links, to connect, edit or remove them.
    pub fn links_mut(&mut self) -> &mut LinkManager {
        &mut self.links
    }

    /// Start recording every inbound frame into a new `.tlog` file in
    /// `directory`.
    pub fn record(&self, directory: &Path) -> std::io::Result<TlogRecorder> {
        TlogRecorder::start(&self.router, directory)
    }
}
