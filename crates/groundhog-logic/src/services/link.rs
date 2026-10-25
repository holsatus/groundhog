//! Link lifecycle: the configured links, and a supervisor per connected link.
//!
//! [`LinkManager`] is the store of configured links. Connecting a link spawns
//! a supervisor task that owns the transport: it opens it, attaches it to the
//! router, waits for it to fail, and — unless the link is configured not to —
//! reopens it with backoff. A USB serial adapter that is unplugged and
//! replugged, or a TCP peer that restarts, comes back on its own.
//!
//! The supervisor publishes its [`LinkStatus`]; the manager reads it on demand,
//! so the status the UI shows is always the supervisor's own. Every status
//! change is also sent as a [`LinkEvent`], which wakes the UI and lets it
//! notify the operator.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use tokio::sync::{mpsc, watch};

use crate::{
    link::{LinkConfig, LinkId},
    mavlink::{LinkStats, MavlinkRouter},
};

/// How long an open may take before it counts as failed.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest wait between reconnection attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// The connection state of one configured link.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkStatus {
    Disconnected,
    /// Opening the transport.
    Connecting {
        attempt: u32,
    },
    Connected {
        since: Instant,
    },
    /// The transport failed or was lost; another attempt follows at `retry_at`.
    Reconnecting {
        error: String,
        attempt: u32,
        retry_at: Instant,
    },
    /// The transport failed and the link does not reconnect on its own.
    Failed(String),
}

impl LinkStatus {
    /// Whether a supervisor is running for the link (connected or trying).
    pub fn is_active(&self) -> bool {
        !matches!(self, LinkStatus::Disconnected | LinkStatus::Failed(_))
    }

    pub fn is_connected(&self) -> bool {
        matches!(self, LinkStatus::Connected { .. })
    }
}

/// A link's status changed.
#[derive(Debug, Clone)]
pub struct LinkEvent {
    pub id: LinkId,
    pub name: String,
    /// The status before this change, so a reaction can tell a lost
    /// connection from a failed first attempt.
    pub previous: LinkStatus,
    pub status: LinkStatus,
}

/// Aborts a task when dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A running supervisor.
struct Session {
    status: watch::Receiver<LinkStatus>,
    _task: AbortOnDrop,
}

/// One configured link and its supervisor, if connected.
struct LinkEntry {
    config: LinkConfig,
    session: Option<Session>,
    /// When the link's last connection ended; shared with its supervisor,
    /// which sees a connection end on its own.
    last_connected: LastConnected,
}

/// When a link's last connection ended, if it has had one.
#[derive(Clone, Default)]
struct LastConnected(Arc<Mutex<Option<Instant>>>);

impl LastConnected {
    fn get(&self) -> Option<Instant> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Note a change of status: a connection that ends ends now.
    fn note(&self, previous: &LinkStatus, status: &LinkStatus) {
        if previous.is_connected() && !status.is_connected() {
            *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        }
    }
}

impl LinkEntry {
    fn new(config: LinkConfig) -> Self {
        Self {
            config,
            session: None,
            last_connected: LastConnected::default(),
        }
    }
}

/// What saving a link did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    /// A new link, under a fresh id.
    Added(LinkId),
    /// The same-named link, updated; its connection, if any, kept.
    Updated(LinkId),
    /// The same-named link, updated, and its live connection restarted on
    /// the new transport.
    Reconnected(LinkId),
}

impl Saved {
    /// The link saved.
    pub fn id(&self) -> &LinkId {
        match self {
            Saved::Added(id) | Saved::Updated(id) | Saved::Reconnected(id) => id,
        }
    }
}

/// Why a link configuration cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LinkConfigError {
    #[error("A link needs a name")]
    EmptyName,
}

/// Owns the configured links and their supervisors.
pub struct LinkManager {
    router: MavlinkRouter,
    links: BTreeMap<LinkId, LinkEntry>,
    /// Source of fresh link ids; never derived from the display name.
    next_id: u64,
    events: mpsc::UnboundedSender<LinkEvent>,
}

/// The stream of [`LinkEvent`]s from a [`LinkManager`]'s supervisors.
pub struct LinkEvents(mpsc::UnboundedReceiver<LinkEvent>);

impl LinkEvents {
    pub async fn next(&mut self) -> Option<LinkEvent> {
        self.0.recv().await
    }

    /// The events as a stream, for a UI runtime to drive.
    pub fn into_stream(self) -> impl futures::Stream<Item = LinkEvent> + Send + 'static {
        futures::stream::unfold(self, |mut events| async move {
            events.next().await.map(|event| (event, events))
        })
    }
}

impl LinkManager {
    /// Build a manager from the persisted links, returning it along with the
    /// ids of the links marked for auto-connect and the event stream.
    pub fn new(
        router: &MavlinkRouter,
        configs: Vec<LinkConfig>,
    ) -> (Self, Vec<LinkId>, LinkEvents) {
        let (events, receiver) = mpsc::unbounded_channel();
        let mut manager = Self {
            router: router.clone(),
            links: BTreeMap::new(),
            next_id: 0,
            events,
        };

        let mut auto_connect = Vec::new();
        for config in configs {
            let id = manager.fresh_id();
            if config.auto_connect {
                auto_connect.push(id.clone());
            }
            manager.links.insert(id, LinkEntry::new(config));
        }

        (manager, auto_connect, LinkEvents(receiver))
    }

    /// Mint an opaque link id that does not depend on the display name.
    fn fresh_id(&mut self) -> LinkId {
        self.next_id += 1;
        LinkId::new(&format!("link-{:04}", self.next_id))
    }

    /// Start supervising `id`. Returns `false` (and does nothing) for an
    /// unknown link or one that is already active, so a double-click on
    /// Connect never attaches a second transport.
    ///
    /// Must be called within an async runtime.
    pub fn connect(&mut self, id: &LinkId) -> bool {
        let Some(entry) = self.links.get_mut(id) else {
            return false;
        };

        if entry
            .session
            .as_ref()
            .is_some_and(|session| session.status.borrow().is_active())
        {
            return false;
        }

        let (status_tx, status_rx) = watch::channel(LinkStatus::Connecting { attempt: 1 });
        let task = tokio::spawn(supervise(
            self.router.clone(),
            id.clone(),
            entry.config.clone(),
            status_tx,
            entry.last_connected.clone(),
            self.events.clone(),
        ));

        entry.session = Some(Session {
            status: status_rx,
            _task: AbortOnDrop(task),
        });
        true
    }

    /// Stop the link's supervisor, closing its transport.
    pub fn disconnect(&mut self, id: &LinkId) {
        if let Some(entry) = self.links.get_mut(id)
            && let Some(session) = entry.session.take()
        {
            let previous = session.status.borrow().clone();
            entry
                .last_connected
                .note(&previous, &LinkStatus::Disconnected);
            let _ = self.events.send(LinkEvent {
                id: id.clone(),
                name: entry.config.name.clone(),
                previous,
                status: LinkStatus::Disconnected,
            });
        }
    }

    /// The link's current status.
    pub fn status(&self, id: &LinkId) -> LinkStatus {
        self.links
            .get(id)
            .and_then(|entry| entry.session.as_ref())
            .map(|session| session.status.borrow().clone())
            .unwrap_or(LinkStatus::Disconnected)
    }

    /// When the link's last connection ended, if it has been connected since
    /// the manager started. While it is connected, its status says since
    /// when.
    pub fn last_connected(&self, id: &LinkId) -> Option<Instant> {
        self.links.get(id)?.last_connected.get()
    }

    /// Traffic counters of the link's current attachment.
    pub fn stats(&self, id: &LinkId) -> Option<LinkStats> {
        self.router.link_stats(id)
    }

    /// Add a link, or update the existing one with the same name in place.
    ///
    /// Reusing the id of a same-named link keeps its identity stable across
    /// edits. A live connection survives an update that leaves its transport
    /// as it was (the name, whether it connects at start); one whose
    /// transport changed is restarted with the new one. A blank name is
    /// refused, and names are stored trimmed.
    pub fn save(&mut self, mut config: LinkConfig) -> Result<Saved, LinkConfigError> {
        config.name = config.name.trim().to_string();
        if config.name.is_empty() {
            return Err(LinkConfigError::EmptyName);
        }

        let existing = self
            .links
            .iter()
            .find(|(_, entry)| entry.config.name == config.name)
            .map(|(id, _)| id.clone());
        let Some(id) = existing else {
            let id = self.fresh_id();
            self.links.insert(id.clone(), LinkEntry::new(config));
            return Ok(Saved::Added(id));
        };

        let active = self.status(&id).is_active();
        let entry = self.links.get_mut(&id).expect("the link was just found");
        // What its supervisor reads when it starts.
        let transport_changed = entry.config.build != config.build
            || entry.config.high_latency != config.high_latency
            || entry.config.auto_reconnect != config.auto_reconnect;
        entry.config = config;

        if transport_changed && active {
            self.disconnect(&id);
            self.connect(&id);
            Ok(Saved::Reconnected(id))
        } else {
            Ok(Saved::Updated(id))
        }
    }

    /// Remove a configured link, dropping any live connection with it.
    /// Reports whether a link was actually removed.
    pub fn remove(&mut self, id: &LinkId) -> bool {
        self.disconnect(id);
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
            .keys()
            .map(|id| (id, &self.links[id].config, self.status(id)))
    }
}

/// The wait before reconnection attempt `attempt` (1-based): 1 s, doubling,
/// capped at [`MAX_BACKOFF`].
fn backoff(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(4);
    (Duration::from_secs(1) * 2u32.pow(exponent)).min(MAX_BACKOFF)
}

/// Own one link's transport until aborted.
async fn supervise(
    router: MavlinkRouter,
    id: LinkId,
    config: LinkConfig,
    status: watch::Sender<LinkStatus>,
    last_connected: LastConnected,
    events: mpsc::UnboundedSender<LinkEvent>,
) {
    let publish = |state: LinkStatus| {
        let previous = status.send_replace(state.clone());
        last_connected.note(&previous, &state);
        let _ = events.send(LinkEvent {
            id: id.clone(),
            name: config.name.clone(),
            previous,
            status: state,
        });
    };

    let mut attempt = 0u32;

    loop {
        attempt += 1;
        publish(LinkStatus::Connecting { attempt });

        let opened = tokio::time::timeout(OPEN_TIMEOUT, crate::link::open(config.build.clone()))
            .await
            .unwrap_or_else(|_| {
                Err(std::sync::Arc::new(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "opening the transport timed out",
                )))
            });

        let error = match opened {
            Ok((reader, writer)) => {
                let guard =
                    router.attach_reader_writer(id.clone(), reader, writer, config.high_latency);
                log::info!("Link {} connected", config.name);
                publish(LinkStatus::Connected {
                    since: Instant::now(),
                });
                attempt = 0;

                // Held until the transport fails; dropping this task (a
                // disconnect) drops the guard and closes the link.
                guard.closed().await;
                log::warn!("Link {} lost", config.name);
                "the connection was lost".to_string()
            }
            Err(error) => {
                log::warn!("Link {} failed to open: {error}", config.name);
                error.to_string()
            }
        };

        if !config.auto_reconnect {
            publish(LinkStatus::Failed(error));
            return;
        }

        let delay = backoff(attempt.max(1));
        publish(LinkStatus::Reconnecting {
            error,
            attempt: attempt.max(1),
            retry_at: Instant::now() + delay,
        });
        tokio::time::sleep(delay).await;
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
    use crate::link::LinkBuild;

    use std::net::{Ipv4Addr, SocketAddrV4};

    use super::*;
    use crate::mavlink::DEFAULT_GCS_ID;

    fn config(name: &str) -> LinkConfig {
        LinkConfig {
            name: name.to_string(),
            auto_connect: false,
            high_latency: false,
            auto_reconnect: true,
            build: LinkBuild::Udp {
                sock_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
                peer: None,
            },
        }
    }

    fn manager() -> (LinkManager, LinkEvents, MavlinkRouter) {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (manager, _auto_connect, events) = LinkManager::new(&router, Vec::new());
        (manager, events, router)
    }

    async fn wait_for(events: &mut LinkEvents, done: impl Fn(&LinkStatus) -> bool) -> LinkStatus {
        wait_for_event(events, done).await.status
    }

    async fn wait_for_event(
        events: &mut LinkEvents,
        done: impl Fn(&LinkStatus) -> bool,
    ) -> LinkEvent {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), events.next())
                .await
                .expect("a status change arrives")
                .expect("the manager is alive");
            if done(&event.status) {
                return event;
            }
        }
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
        assert_eq!(backoff(3), Duration::from_secs(4));
        assert_eq!(backoff(10), MAX_BACKOFF);
    }

    #[test]
    fn new_loads_links_and_reports_auto_connect() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let auto = LinkConfig {
            auto_connect: true,
            ..config("auto")
        };

        let (manager, auto_connect, _) = LinkManager::new(&router, vec![auto, config("manual")]);

        assert_eq!(manager.configs().len(), 2);
        assert_eq!(auto_connect.len(), 1);
        assert_eq!(manager.status(&auto_connect[0]), LinkStatus::Disconnected);
    }

    /// Save `config` as a new link, and return its id.
    fn add(manager: &mut LinkManager, config: LinkConfig) -> LinkId {
        match manager.save(config) {
            Ok(Saved::Added(id)) => id,
            other => panic!("expected a new link, got {other:?}"),
        }
    }

    #[test]
    fn save_adds_then_updates_by_name_reusing_the_id() {
        let (mut manager, _, _) = manager();

        let first = add(&mut manager, config("A"));
        assert_eq!(manager.save(config("A")), Ok(Saved::Updated(first)));
        assert_eq!(manager.configs().len(), 1);

        add(&mut manager, config("B"));
        assert_eq!(manager.configs().len(), 2);
    }

    #[test]
    fn a_blank_name_is_refused() {
        let (mut manager, _, _) = manager();
        assert_eq!(manager.save(config("  ")), Err(LinkConfigError::EmptyName));
        assert!(manager.configs().is_empty());

        add(&mut manager, config(" A "));
        assert_eq!(manager.configs()[0].name, "A", "stored trimmed");
    }

    #[tokio::test]
    async fn saving_an_unchanged_transport_keeps_the_connection() {
        let (mut manager, mut events, _router) = manager();
        let id = add(&mut manager, config("A"));
        manager.connect(&id);
        wait_for(&mut events, LinkStatus::is_connected).await;

        let saved = manager.save(LinkConfig {
            auto_connect: true,
            ..config("A")
        });
        assert_eq!(saved, Ok(Saved::Updated(id.clone())));
        assert!(manager.status(&id).is_connected());
        assert!(events.0.try_recv().is_err(), "no status change");
    }

    #[tokio::test]
    async fn saving_a_changed_transport_reconnects() {
        let (mut manager, mut events, _router) = manager();
        let id = add(&mut manager, config("A"));
        manager.connect(&id);
        wait_for(&mut events, LinkStatus::is_connected).await;

        let saved = manager.save(LinkConfig {
            high_latency: true,
            ..config("A")
        });
        assert_eq!(saved, Ok(Saved::Reconnected(id.clone())));
        wait_for(&mut events, |status| *status == LinkStatus::Disconnected).await;
        wait_for(&mut events, LinkStatus::is_connected).await;
        assert!(manager.configs()[0].high_latency);
    }

    #[test]
    fn remove_drops_the_link_and_unknown_is_a_noop() {
        let (mut manager, _, _) = manager();
        let id = add(&mut manager, config("A"));

        assert!(!manager.remove(&LinkId::new("does-not-exist")));
        assert!(manager.remove(&id));
        assert!(manager.configs().is_empty());
    }

    #[tokio::test]
    async fn connect_attaches_and_disconnect_detaches() {
        let (mut manager, mut events, router) = manager();
        let id = add(&mut manager, config("A"));

        assert!(manager.connect(&id));
        assert!(
            !manager.connect(&id),
            "a live link refuses a second connect"
        );

        wait_for(&mut events, LinkStatus::is_connected).await;
        assert!(manager.status(&id).is_connected());
        assert!(router.is_attached(&id));

        manager.disconnect(&id);
        assert_eq!(manager.status(&id), LinkStatus::Disconnected);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!router.is_attached(&id), "the transport was closed");
    }

    #[tokio::test]
    async fn events_carry_the_status_they_replace() {
        let (mut manager, mut events, _router) = manager();
        let id = add(&mut manager, config("A"));

        manager.connect(&id);
        let connected = wait_for_event(&mut events, LinkStatus::is_connected).await;
        assert!(matches!(connected.previous, LinkStatus::Connecting { .. }));

        assert_eq!(manager.last_connected(&id), None, "connected now");
        manager.disconnect(&id);
        let disconnected =
            wait_for_event(&mut events, |status| *status == LinkStatus::Disconnected).await;
        assert!(disconnected.previous.is_connected());
        assert!(
            manager.last_connected(&id).is_some(),
            "the connection that just ended is remembered"
        );
    }

    #[tokio::test]
    async fn a_failed_open_retries_with_backoff() {
        let (mut manager, mut events, _router) = manager();

        // Nothing listens on a TCP port we just released.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            std::net::SocketAddr::V4(addr) => addr,
            _ => unreachable!(),
        };
        drop(listener);

        let id = add(
            &mut manager,
            LinkConfig {
                build: LinkBuild::Tcp { sock_addr: addr },
                ..config("tcp")
            },
        );
        assert!(manager.connect(&id));

        let status = wait_for(&mut events, |status| {
            matches!(status, LinkStatus::Reconnecting { .. })
        })
        .await;
        assert!(matches!(
            status,
            LinkStatus::Reconnecting { attempt: 1, .. }
        ));
        assert!(manager.status(&id).is_active(), "still trying");
    }

    #[tokio::test]
    async fn without_auto_reconnect_a_failure_is_final() {
        let (mut manager, mut events, _router) = manager();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            std::net::SocketAddr::V4(addr) => addr,
            _ => unreachable!(),
        };
        drop(listener);

        let id = add(
            &mut manager,
            LinkConfig {
                build: LinkBuild::Tcp { sock_addr: addr },
                auto_reconnect: false,
                ..config("tcp")
            },
        );
        manager.connect(&id);

        wait_for(&mut events, |status| {
            matches!(status, LinkStatus::Failed(_))
        })
        .await;
        assert!(!manager.status(&id).is_active());
        assert!(manager.connect(&id), "a failed link can be connected again");
    }
}
