//! Persisted link configuration.
//!
//! [`LinkBuild`] is the parsed, serialisable description of how to open a link,
//! and [`LinkConfig`] pairs it with the link's persisted settings. The editable
//! draft the link editor works with lives in the link feature.

use std::net::SocketAddrV4;

/// A fully configured link.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LinkConfig {
    pub name: String,
    /// Connect this link automatically on startup.
    pub auto_connect: bool,
    /// Use a slower heartbeat for this link, to save bandwidth on expensive
    /// links such as satellite.
    pub high_latency: bool,
    pub build: LinkBuild,
}

/// The parsed, serialisable description of how to open a link.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LinkBuild {
    Tcp { sock_addr: SocketAddrV4 },
    Udp { sock_addr: SocketAddrV4 },
    Serial { port: String, baud: u32 },
}
