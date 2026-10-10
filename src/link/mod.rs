//! Link transports and their configuration.

pub mod config;
pub mod transport;
pub mod udp;

pub use config::{
    BAUDRATES, LinkBuild, LinkBuilder, LinkConfig, LinkConfigurator, LinkVariant,
};
pub use transport::Event;
