//! Link identity, transports, and their configuration.

pub mod config;
pub mod id;
pub mod transport;
pub mod udp;

pub use config::LinkConfig;
pub use id::LinkId;
pub use transport::open;
