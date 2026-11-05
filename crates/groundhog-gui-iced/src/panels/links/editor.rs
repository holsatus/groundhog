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

/// A text field of a network link's draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkField {
    Address,
    Port,
    /// The UDP peer to send to; empty for a listening link.
    Peer,
}

impl LinkBuilder {
    /// Set a text field. A field the variant does not have is ignored.
    pub fn set_field(&mut self, field: LinkField, value: String) {
        let target = match (self, field) {
            (LinkBuilder::Tcp { addr, .. } | LinkBuilder::Udp { addr, .. }, LinkField::Address) => {
                addr
            }
            (LinkBuilder::Tcp { port, .. } | LinkBuilder::Udp { port, .. }, LinkField::Port) => {
                port
            }
            (LinkBuilder::Udp { peer, .. }, LinkField::Peer) => peer,
            _ => return,
        };
        *target = value;
    }

    /// Choose the serial port; ignored for network links.
    pub fn select_serial_port(&mut self, selected: String) {
        if let LinkBuilder::Serial { port, .. } = self {
            *port = Some(selected);
        }
    }

    /// Choose the serial baud rate; ignored for network links.
    pub fn select_baud(&mut self, selected: u32) {
        if let LinkBuilder::Serial { baud, .. } = self {
            *baud = selected;
        }
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

impl LinkBuilder {
    /// The draft of an existing link, for editing it.
    pub fn from_build(build: &LinkBuild) -> LinkBuilder {
        match build {
            LinkBuild::Tcp { sock_addr } => LinkBuilder::Tcp {
                addr: sock_addr.ip().to_string(),
                port: sock_addr.port().to_string(),
            },
            LinkBuild::Udp { sock_addr, peer } => LinkBuilder::Udp {
                addr: sock_addr.ip().to_string(),
                port: sock_addr.port().to_string(),
                peer: peer.map(|peer| peer.to_string()).unwrap_or_default(),
            },
            LinkBuild::Serial { port, baud } => LinkBuilder::Serial {
                port: Some(port.clone()),
                baud: *baud,
                // Detected when the operator opens the port list.
                available_ports: Vec::new(),
            },
        }
    }
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

    /// A draft of `config`, to change it and save it again.
    pub fn from_config(config: &LinkConfig) -> LinkConfigurator {
        LinkConfigurator {
            name: config.name.clone(),
            auto_connect: config.auto_connect,
            high_latency: config.high_latency,
            auto_reconnect: config.auto_reconnect,
            builder: LinkBuilder::from_build(&config.build),
            build: Some(config.build.clone()),
        }
    }

    /// Change the draft's builder, and parse it again.
    pub fn edit_builder(&mut self, edit: impl FnOnce(&mut LinkBuilder)) {
        edit(&mut self.builder);
        self.build = self.builder.try_build();
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
    fn a_saved_link_comes_back_as_its_own_draft() {
        let builds = [
            LinkBuild::Tcp {
                sock_addr: SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 5760),
            },
            LinkBuild::Udp {
                sock_addr: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 14550),
                peer: Some(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 2), 14551)),
            },
            LinkBuild::Serial {
                port: "/dev/ttyACM0".into(),
                baud: 57_600,
            },
        ];

        for build in builds {
            let config = LinkConfig {
                name: "Saved".into(),
                auto_connect: true,
                high_latency: true,
                auto_reconnect: false,
                build,
            };
            let draft = LinkConfigurator::from_config(&config);

            // The fields parse back to the same link, and so does the draft.
            let rebuilt = draft.builder.try_build().expect("the draft parses");
            assert_eq!(format!("{rebuilt:?}"), format!("{:?}", config.build));
            let saved = draft.parse().expect("the draft saves");
            assert_eq!(format!("{saved:?}"), format!("{config:?}"));
        }
    }

    #[test]
    fn fields_edit_their_variant_only() {
        let mut configurator = LinkConfigurator::new(LinkBuilder::default_tcp(), "A".into());
        configurator.edit_builder(|builder| builder.set_field(LinkField::Port, "5761".into()));
        let Some(LinkBuild::Tcp { sock_addr }) = configurator.build else {
            panic!("the edited draft builds a TCP link");
        };
        assert_eq!(sock_addr, SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 5761));

        // TCP links have no peer, nor a baud rate.
        configurator.edit_builder(|builder| {
            builder.set_field(LinkField::Peer, "1.2.3.4:5".into());
            builder.select_baud(9_600);
        });
        assert!(matches!(configurator.build, Some(LinkBuild::Tcp { .. })));

        configurator.edit_builder(|builder| builder.set_field(LinkField::Port, "port".into()));
        assert!(configurator.build.is_none(), "an unparseable port");
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
