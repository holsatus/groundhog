//! The link editor's draft model.
//!
//! [`LinkBuilder`] is the editable draft (raw text fields) and
//! [`LinkConfigurator`] pairs a draft with its last successfully parsed
//! [`LinkBuild`]. The persisted configuration model ([`LinkConfig`],
//! [`LinkBuild`]) lives in [`groundhog_logic::link`].

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
};

use groundhog_logic::link::{LinkBuild, LinkConfig};

pub const BAUDRATES: &[u32] = &[
    2_400, 4_800, 9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 500_000, 576_000,
    921_600, 1_000_000,
];

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
        peer: String,
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
            peer: String::new(),
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
            LinkBuilder::Udp { addr, port, peer } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                let sock = SocketAddrV4::new(addr, port);

                // An empty peer means a listening link; anything else must
                // parse as an `IPv4:port` pair.
                let peer = match peer.trim() {
                    "" => None,
                    peer => Some(SocketAddrV4::from_str(peer).ok()?),
                };

                LinkBuild::Udp {
                    sock_addr: sock,
                    peer,
                }
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
    pub auto_reconnect: bool,
    pub builder: LinkBuilder,
    pub build: Option<LinkBuild>,
}

impl LinkConfigurator {
    pub fn new(builder: LinkBuilder, name: String) -> LinkConfigurator {
        LinkConfigurator {
            name,
            auto_connect: false,
            high_latency: false,
            auto_reconnect: true,
            builder: builder.clone(),
            build: builder.try_build(),
        }
    }

    pub fn parse(&self) -> Option<LinkConfig> {
        self.build.as_ref().map(|build| LinkConfig {
            name: self.name.clone(),
            auto_connect: self.auto_connect,
            high_latency: self.high_latency,
            auto_reconnect: self.auto_reconnect,
            build: build.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_udp_parses_with_an_empty_peer() {
        let Some(LinkBuild::Udp { sock_addr, peer }) = LinkBuilder::default_udp().try_build()
        else {
            panic!("the default UDP draft must parse")
        };

        assert_eq!(sock_addr, SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 14550));
        assert_eq!(peer, None, "an empty draft peer is a listening link");
    }

    #[test]
    fn udp_peer_must_parse_or_the_build_fails() {
        let valid = LinkBuilder::Udp {
            addr: "0.0.0.0".into(),
            port: "14550".into(),
            peer: "192.168.1.2:14551".into(),
        };
        let Some(LinkBuild::Udp { peer, .. }) = valid.try_build() else {
            panic!("a parseable peer must build")
        };
        assert_eq!(
            peer,
            Some(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 2), 14551))
        );

        // Surrounding whitespace is trimmed away before parsing.
        let padded = LinkBuilder::Udp {
            addr: "0.0.0.0".into(),
            port: "14550".into(),
            peer: " 192.168.1.2:14551 ".into(),
        };
        assert!(padded.try_build().is_some());

        // An unparseable peer makes the whole build fail, exactly like a
        // malformed address or port does.
        let malformed = LinkBuilder::Udp {
            addr: "0.0.0.0".into(),
            port: "14550".into(),
            peer: "nonsense".into(),
        };
        assert!(
            malformed.try_build().is_none(),
            "an unparseable peer must invalidate the build"
        );
    }
}
