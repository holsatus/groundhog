//! How a link is identified and described.
//!
//! [`LinkConfig`] is the persisted description of a link a frontend edits and
//! stores; [`LinkBuild`] is the part that says how to open its transport. The
//! backend opens and supervises links from these, and names each by a
//! [`LinkId`].

use std::{net::SocketAddrV4, sync::Arc};

/// An opaque handle identifying one configured link.
///
/// Deliberately independent of the link's display name: renaming a link never
/// changes its identity, and therefore never changes the router attachment it
/// names. Cheap to clone and usable as a map key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(Arc<str>);

impl LinkId {
    pub fn new(id: &str) -> Self {
        LinkId(Arc::from(id))
    }
}

/// A fully configured link.
///
/// Frontends persist it, so its serde shape is a compatibility promise:
/// variant names are pinned, and a field added later has a default, so a
/// file written before it existed still loads.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LinkConfig {
    pub name: String,
    /// Connect this link automatically on startup.
    #[serde(default)]
    pub auto_connect: bool,
    /// Use a slower heartbeat for this link, to save bandwidth on expensive
    /// links such as satellite.
    #[serde(default)]
    pub high_latency: bool,
    /// Reopen the transport when it fails, with backoff, until disconnected.
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
    pub build: LinkBuild,
}

fn default_true() -> bool {
    true
}

/// The parsed, serialisable description of how to open a link.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LinkBuild {
    #[serde(rename = "Tcp")]
    Tcp { sock_addr: SocketAddrV4 },
    #[serde(rename = "Udp")]
    Udp {
        sock_addr: SocketAddrV4,
        #[serde(default)]
        peer: Option<SocketAddrV4>,
    },
    #[serde(rename = "Serial")]
    Serial { port: String, baud: u32 },
}
