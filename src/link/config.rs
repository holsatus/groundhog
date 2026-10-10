//! Link configuration and drafts.
//!
//! [`LinkBuild`] is the parsed, serialisable form; [`LinkBuilder`] is the
//! editable draft (raw text fields); [`LinkConfigurator`] pairs a draft with
//! its last successfully parsed [`LinkBuild`].

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
};

pub const BAUDRATES: &[u32] = &[
    2_400, 4_800, 9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 500_000, 576_000,
    921_600, 1_000_000,
];

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

/// The editable draft of a link, held as raw text by the link editor.
#[derive(Debug, Clone)]
pub enum LinkBuilder {
    Tcp {
        addr: String,
        port: String,
    },
    Udp {
        addr: String,
        port: String,
    },
    Serial {
        port: Option<String>,
        baud: u32,
        available_ports: Vec<String>,
    },
}

impl LinkBuilder {
    pub fn default_tcp() -> LinkBuilder {
        LinkBuilder::Tcp {
            addr: "0.0.0.0".into(),
            port: "5760".into(),
        }
    }

    pub fn default_udp() -> LinkBuilder {
        LinkBuilder::Udp {
            addr: "0.0.0.0".into(),
            port: "14550".into(),
        }
    }

    pub fn default_serial() -> LinkBuilder {
        LinkBuilder::Serial {
            port: None,
            baud: 115_200,
            available_ports: Vec::new(),
        }
    }

    pub fn to_variant(&self) -> LinkVariant {
        match self {
            LinkBuilder::Tcp { .. } => LinkVariant::Tcp,
            LinkBuilder::Udp { .. } => LinkVariant::Udp,
            LinkBuilder::Serial { .. } => LinkVariant::Serial,
        }
    }

    pub fn try_build(&self) -> Option<LinkBuild> {
        let config = match self {
            LinkBuilder::Tcp { addr, port } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                let sock = SocketAddrV4::new(addr, port);
                LinkBuild::Tcp { sock_addr: sock }
            }
            LinkBuilder::Udp { addr, port } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                let sock = SocketAddrV4::new(addr, port);
                LinkBuild::Udp { sock_addr: sock }
            }
            LinkBuilder::Serial { port, baud, .. } => {
                let port = port.to_owned()?;
                LinkBuild::Serial { port, baud: *baud }
            }
        };

        Some(config)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinkVariant {
    Tcp,
    Udp,
    Serial,
}

impl LinkVariant {
    pub fn to_default_builder(self) -> LinkBuilder {
        match self {
            LinkVariant::Tcp => LinkBuilder::default_tcp(),
            LinkVariant::Udp => LinkBuilder::default_udp(),
            LinkVariant::Serial => LinkBuilder::default_serial(),
        }
    }

    pub fn list() -> &'static [LinkVariant] {
        &[LinkVariant::Tcp, LinkVariant::Udp, LinkVariant::Serial]
    }
}

impl std::fmt::Display for LinkVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkVariant::Tcp => f.write_str("TCP"),
            LinkVariant::Udp => f.write_str("UDP"),
            LinkVariant::Serial => f.write_str("Serial"),
        }
    }
}

/// The link editor's working state: a draft builder plus the last build that
/// parsed successfully (used to enable the save button and persist changes).
pub struct LinkConfigurator {
    pub name: String,
    pub auto_connect: bool,
    pub high_latency: bool,
    pub builder: LinkBuilder,
    pub build: Option<LinkBuild>,
}

impl LinkConfigurator {
    pub fn new(builder: LinkBuilder, name: String) -> LinkConfigurator {
        LinkConfigurator {
            name,
            auto_connect: false,
            high_latency: false,
            builder: builder.clone(),
            build: builder.try_build(),
        }
    }

    pub fn parse(&self) -> Option<LinkConfig> {
        self.build.as_ref().map(|build| LinkConfig {
            name: self.name.clone(),
            auto_connect: self.auto_connect,
            high_latency: self.high_latency,
            build: build.clone(),
        })
    }
}
