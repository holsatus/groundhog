//! Link lifecycle: a headless manager owning the configured links and the
//! transports behind them.
//!
//! The manager is the store and the connect/disconnect/reconcile controller for
//! links. It owns the live [`LinkGuard`]s, so it lives here in the services
//! layer rather than in the link layer (which would recreate the cycle the
//! link/middleware split just broke) or the domain. The UI is a thin adapter
//! over this type.

use std::{collections::BTreeMap, future::Future};

use crate::{
    link::{LinkConfig, LinkId},
    mavlink::{LinkGuard, MavlinkRouter},
};

/// The connection state of one configured link.
#[derive(Debug, Clone)]
pub enum LinkStatus {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

/// The outcome of an attempted link attach.
///
/// Named for what it is, not when it fires: this value is the outcome of a
/// [`LinkManager::connect`], not a stream event like [`crate::mavlink::Event`].
#[derive(Debug, Clone)]
pub enum LinkUpdate {
    Connected(LinkId, LinkGuard),
    Failed(LinkId, String),
}

/// One configured link, its optional live attachment, and its connection state.
struct LinkEntry {
    config: LinkConfig,
    handle: Option<LinkGuard>,
    status: LinkStatus,
}

/// Owns the configured links and their connection lifecycle.
///
/// The manager holds a clone of the router every link is attached to, so a
/// connect future can carry its own clone into the task and outlive the borrow
/// of the manager that started it.
pub struct LinkManager {
    router: MavlinkRouter,
    links: BTreeMap<LinkId, LinkEntry>,
    /// Source of fresh link ids; never derived from the display name.
    next_id: u64,
}

impl LinkManager {
    /// Build a manager from the persisted links, returning it along with the
    /// ids of the links marked for auto-connect.
    pub fn from_configs(router: &MavlinkRouter, configs: Vec<LinkConfig>) -> (Self, Vec<LinkId>) {
        let mut manager = Self {
            router: router.clone(),
            links: BTreeMap::new(),
            next_id: 0,
        };

        let mut auto_connect = Vec::new();
        for config in configs {
            let id = manager.fresh_id();
            if config.auto_connect {
                auto_connect.push(id.clone());
            }
            manager.links.insert(
                id,
                LinkEntry {
                    config,
                    handle: None,
                    status: LinkStatus::Disconnected,
                },
            );
        }

        (manager, auto_connect)
    }

    /// Mint an opaque link id that does not depend on the display name.
    fn fresh_id(&mut self) -> LinkId {
        self.next_id += 1;
        LinkId::new(&format!("link-{:04}", self.next_id))
    }

    /// Start connecting `id`, or refuse when it is already connecting or
    /// connected.
    ///
    /// Refusal is what keeps a double-click on Connect from attaching a second
    /// reader/writer pair: the caller spawns a task only when this returns a
    /// future. The returned future is `'static`: it carries a router clone and
    /// the link's parsed build, so it can run after the borrow of the manager
    /// that produced it has ended.
    pub fn connect(&mut self, id: &LinkId) -> Option<impl Future<Output = LinkUpdate> + Send + 'static> {
        let (build, high_latency) = {
            let entry = self.links.get_mut(id)?;

            if matches!(entry.status, LinkStatus::Connecting | LinkStatus::Connected) {
                return None;
            }

            entry.status = LinkStatus::Connecting;
            (entry.config.build.clone(), entry.config.high_latency)
        };

        let router = self.router.clone();
        let id = id.clone();

        Some(async move {
            match crate::link::open(build).await {
                Ok((reader, writer)) => {
                    let handle =
                        router.attach_reader_writer(id.clone(), reader, writer, high_latency);
                    LinkUpdate::Connected(id, handle)
                }
                Err(error) => LinkUpdate::Failed(id, error.to_string()),
            }
        })
    }

    /// Fold a landed connect outcome back into the link's state.
    pub fn apply(&mut self, update: LinkUpdate) {
        match update {
            LinkUpdate::Connected(id, handle) => {
                if let Some(entry) = self.links.get_mut(&id) {
                    entry.handle = Some(handle);
                    entry.status = LinkStatus::Connected;
                }
            }
            LinkUpdate::Failed(id, error) => {
                log::error!("Connect failed for {id:?}: {error}");
                if let Some(entry) = self.links.get_mut(&id) {
                    entry.handle = None;
                    entry.status = LinkStatus::Failed(error);
                }
            }
        }
    }

    /// Drop the live attachment of `id`, if any.
    pub fn disconnect(&mut self, id: &LinkId) {
        if let Some(entry) = self.links.get_mut(id) {
            drop(entry.handle.take());
            entry.status = LinkStatus::Disconnected;
        }
    }

    /// Drop handles the router no longer knows about.
    ///
    /// `LinkClosed` rides the lossy telemetry ring, so a stored handle can
    /// outlive the attachment it names. A handle to a link the router has
    /// detached — however that detach happened — is nonsense, and keeping it
    /// would block a re-attach forever; this reconciles within one call.
    pub fn reconcile(&mut self) {
        for (id, entry) in &mut self.links {
            if entry.handle.is_some() && !self.router.is_attached(id) {
                log::info!("link {id:?} was lost by the router; reconciling");
                entry.handle = None;
                entry.status = LinkStatus::Disconnected;
            }
        }
    }

    /// Whether `id` has a live guard on an attachment the router still holds.
    pub fn is_connected(&self, id: &LinkId) -> bool {
        self.links
            .get(id)
            .is_some_and(|entry| entry.handle.is_some() && self.router.is_attached(id))
    }

    /// Add a link, or update the existing one with the same name in place.
    ///
    /// Reusing the id of a same-named link keeps its identity stable across
    /// edits; any live connection is dropped, since the configuration it was
    /// attached under no longer applies. Saving is always a mutation (an add or
    /// a replace), so this reports `true`.
    pub fn save(&mut self, config: LinkConfig) -> bool {
        let existing = self
            .links
            .iter()
            .find(|(_, entry)| entry.config.name == config.name)
            .map(|(id, _)| id.clone());
        let id = existing.unwrap_or_else(|| self.fresh_id());

        self.links.insert(
            id,
            LinkEntry {
                config,
                handle: None,
                status: LinkStatus::Disconnected,
            },
        );

        true
    }

    /// Remove a configured link, dropping any live connection with it.
    /// Reports whether a link was actually removed.
    pub fn remove(&mut self, id: &LinkId) -> bool {
        self.links.remove(id).is_some()
    }

    /// The configured links, in display order.
    pub fn configs(&self) -> Vec<LinkConfig> {
        self.links
            .values()
            .map(|entry| entry.config.clone())
            .collect()
    }

    /// The configured links with their status, for the view.
    pub fn entries(&self) -> impl Iterator<Item = (&LinkId, &LinkConfig, LinkStatus)> {
        self.links
            .iter()
            .map(|(id, entry)| (id, &entry.config, entry.status.clone()))
    }
}

/// Enumerate the USB serial ports available on this host.
///
/// The blocking OS enumeration runs off-thread; the caller gets a plain future
/// and decides how to turn it into work.
pub async fn detect_serial_ports() -> Result<Vec<String>, String> {
    tokio::task::spawn_blocking(|| -> Result<Vec<String>, String> {
        let mut names = serialport::available_ports()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|port| matches!(port.port_type, serialport::SerialPortType::UsbPort(_)))
            .map(|port| port.port_name)
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::DEFAULT_GCS_ID;
    use crate::link::config::LinkBuild;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn config(name: &str) -> LinkConfig {
        LinkConfig {
            name: name.to_string(),
            auto_connect: false,
            high_latency: false,
            build: LinkBuild::Udp {
                sock_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 14550),
                peer: None,
            },
        }
    }

    fn manager() -> (LinkManager, MavlinkRouter) {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (manager, _auto_connect) = LinkManager::from_configs(&router, Vec::new());
        (manager, router)
    }

    #[test]
    fn from_configs_loads_links_and_reports_auto_connect() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let auto = LinkConfig {
            auto_connect: true,
            ..config("auto")
        };
        let manual = config("manual");

        let (manager, auto_connect) = LinkManager::from_configs(&router, vec![auto, manual]);

        assert_eq!(manager.configs().len(), 2);
        assert_eq!(auto_connect.len(), 1);

        let auto_id = auto_connect[0].clone();
        assert!(!manager.is_connected(&auto_id));

        let names = manager
            .entries()
            .map(|(_, config, _)| config.name.clone())
            .collect::<Vec<_>>();
        assert!(names.contains(&"auto".to_string()));
        assert!(names.contains(&"manual".to_string()));
    }

    #[test]
    fn save_adds_then_updates_by_name_reusing_the_id() {
        let (mut manager, _router) = manager();

        assert!(manager.save(config("A")));
        assert_eq!(manager.configs().len(), 1);
        let first_id = manager.entries().next().unwrap().0.clone();

        // Saving the same name updates in place, keeping the id.
        assert!(manager.save(config("A")));
        assert_eq!(manager.configs().len(), 1);
        let second_id = manager.entries().next().unwrap().0.clone();
        assert_eq!(first_id, second_id);

        // A different name adds a link with a fresh id.
        assert!(manager.save(config("B")));
        assert_eq!(manager.configs().len(), 2);
    }

    #[test]
    fn remove_drops_the_link_and_unknown_is_a_noop() {
        let (mut manager, _router) = manager();

        assert!(manager.save(config("A")));
        let id = manager.entries().next().unwrap().0.clone();

        assert!(!manager.remove(&LinkId::new("does-not-exist")));
        assert_eq!(manager.configs().len(), 1);

        assert!(manager.remove(&id));
        assert!(manager.configs().is_empty());
    }

    #[tokio::test]
    async fn reconcile_drops_a_ghost_handle() {
        let (mut manager, _router) = manager();
        assert!(manager.save(config("A")));
        let id = manager.entries().next().unwrap().0.clone();

        // A guard from a *different* router: the manager claims a live
        // connection the router it talks to never attached. Such a handle can
        // only be a ghost (for example a LinkClosed lost to the lossy ring).
        let (other, _other_events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (_peer, gcs) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(gcs);
        let guard = other.attach_reader_writer(LinkId::new("other"), reader, writer, false);
        manager.apply(LinkUpdate::Connected(id.clone(), guard));

        assert!(
            !manager.is_connected(&id),
            "the router never attached this handle"
        );
        let status = manager.entries().next().unwrap().2;
        assert!(matches!(status, LinkStatus::Connected));

        // One reconcile repairs the stored state to the router's reality.
        manager.reconcile();
        assert!(!manager.is_connected(&id));
        let status = manager.entries().next().unwrap().2;
        assert!(matches!(status, LinkStatus::Disconnected));
    }

    #[tokio::test]
    async fn connect_is_refused_while_a_link_is_live() {
        let (mut manager, router) = manager();
        assert!(manager.save(config("A")));
        let id = manager.entries().next().unwrap().0.clone();

        // Attach a real guard to a live transport, under the link's own id,
        // like a landed connect would.
        let (_peer, gcs) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(gcs);
        let guard = router.attach_reader_writer(id.clone(), reader, writer, false);
        manager.apply(LinkUpdate::Connected(id.clone(), guard));

        assert!(manager.is_connected(&id));

        // The update must refuse outright: no future is produced at all, so no
        // second attach could take place, and the existing handle stays.
        assert!(
            manager.connect(&id).is_none(),
            "a live link must refuse a re-connect"
        );
        assert!(manager.is_connected(&id));
    }

    #[test]
    fn connect_unknown_id_is_refused() {
        let (mut manager, _router) = manager();
        assert!(manager.connect(&LinkId::new("does-not-exist")).is_none());
    }

    #[tokio::test]
    async fn connect_opens_a_transport_and_applies_the_guard() {
        // A silent UDP peer gives the connected link somewhere to point.
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, peer.local_addr().unwrap().port());

        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let config = LinkConfig {
            name: "A".into(),
            auto_connect: false,
            high_latency: false,
            build: LinkBuild::Udp {
                sock_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
                peer: Some(peer_addr),
            },
        };
        let (mut manager, _auto_connect) = LinkManager::from_configs(&router, vec![config]);
        let id = manager.entries().next().unwrap().0.clone();

        let update = manager.connect(&id).expect("the first connect is accepted");

        // The status is `Connecting` until the update lands.
        let status = manager.entries().next().unwrap().2;
        assert!(matches!(status, LinkStatus::Connecting));

        let update = update.await;
        manager.apply(update);

        assert!(manager.is_connected(&id));
    }
}
