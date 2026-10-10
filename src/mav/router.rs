use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use iced::futures::{Stream, stream};
use mavio::{
    Frame,
    default_dialect::{enums::MavCmd, messages::CommandAck},
    prelude::{V2, Versionless},
    protocol::FrameBuilder,
};
use tokio::sync::{Semaphore, SemaphorePermit, broadcast, mpsc};
use tokio_util::sync::{CancellationToken, DropGuard};

use super::{
    Targeted,
    error::{MavError, Result},
    lock,
    locks::{KeyGuard, LockKey, LockMap},
    matches_target,
};
use crate::{
    foundation::{LinkId, MavlinkId},
    link::transport::{DynAsyncReader, DynAsyncWriter, new_async_receiver, new_async_sender},
};

/// Maximum number of PARAM_SET transactions in flight at once.
const PARAM_SET_PERMITS: usize = 4;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Bound on buffered application events. When the UI falls behind, the channel
/// drops the oldest events so the freshest telemetry is what reaches it.
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// A decoded inbound MAVLink message, along with where it came from.
#[derive(Debug)]
pub struct InboundMessage {
    pub received_at: Instant,
    pub source_link: LinkId,
    pub source_mav: MavlinkId,
    pub frame: Frame<Versionless>,
}

/// Events emitted from the router to the application.
#[derive(Debug, Clone)]
pub enum Event {
    Message(Arc<InboundMessage>),
    LinkClosed(LinkId),
}

struct MessageFilter {
    filter: Box<dyn Fn(&InboundMessage) -> bool + Send>,
    tx: mpsc::UnboundedSender<Arc<InboundMessage>>,
}

struct Link {
    sequence: AtomicU8,
    tx: mpsc::UnboundedSender<Frame<Versionless>>,
}

impl Link {
    fn send(&self, gcs: MavlinkId, message: &dyn mavio::Message) -> Result<()> {
        let frame = FrameBuilder::new()
            .system_id(gcs.system)
            .component_id(gcs.component)
            .version(V2)
            .sequence(self.sequence.fetch_add(1, Ordering::Relaxed))
            .message(message)
            .map_err(|error| MavError::Encode(format!("{error:?}")))?
            .build()
            .into_versionless();

        self.tx.send(frame).map_err(|_| MavError::NoLink)
    }
}

/// Routes messages between links and vehicles, and runs transactions (commands,
/// message requests, parameters) against specific targets. Cheap to clone.
#[derive(Clone)]
pub struct MavlinkRouter {
    inner: Arc<MavlinkRouterInner>,
}

#[derive(Default)]
struct MessageFilterMap {
    next_filter_index: u64,
    map: BTreeMap<u64, MessageFilter>,
}

#[derive(Default)]
struct MessageRouteMap {
    map: BTreeMap<MavlinkId, BTreeMap<LinkId, Instant>>,
}

#[derive(Default)]
struct PendingAckMap {
    map: BTreeMap<(MavlinkId, u16), mpsc::UnboundedSender<CommandAck>>,
}

struct MavlinkRouterInner {
    gcs_id: MavlinkId,
    events: broadcast::Sender<Event>,
    links: Mutex<BTreeMap<LinkId, Link>>,
    routes: Mutex<MessageRouteMap>,
    filters: Mutex<MessageFilterMap>,
    acks: Mutex<PendingAckMap>,
    locks: LockMap,
    param_permit: Semaphore,
}

impl MavlinkRouter {
    /// Create a new router stamping outbound frames with `gcs_id`, along with
    /// the stream of events for the application.
    pub fn new(gcs_id: MavlinkId) -> (Self, impl Stream<Item = Event> + Send + 'static) {
        let (tx, rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        let router = Self {
            inner: Arc::new(MavlinkRouterInner {
                gcs_id,
                links: Mutex::default(),
                routes: Mutex::default(),
                filters: Mutex::default(),
                acks: Mutex::default(),
                locks: LockMap::default(),
                param_permit: Semaphore::new(PARAM_SET_PERMITS),
                events: tx,
            }),
        };

        let events = stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(event) => return Some((event, rx)),
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        log::warn!("MAVLink event backlog overflowed; dropped {dropped} events");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        });

        (router, events)
    }

    /// Whether a transaction for `key` is currently in flight or queued.
    /// Useful for reflecting in the UI whether an operation is ongoing.
    #[allow(dead_code)] // Retained middleware API; only exercised by tests
    pub fn is_busy(&self, key: &LockKey) -> bool {
        self.inner.locks.is_busy(key)
    }

    /// Send a message to `target`, filling in the target fields.
    pub fn send_targeted<M: Targeted>(&self, target: MavlinkId, mut message: M) -> Result<()> {
        message.set_target(target);
        self.route(target, &message)
    }

    /// Acquire the lock for `key`, waiting in line if it is already held.
    pub(super) async fn lock_key(&self, key: LockKey) -> KeyGuard {
        self.inner.locks.lock(key).await
    }

    /// Acquire the lock for `key` only if nobody else holds or waits for it.
    pub(super) fn try_lock_key(&self, key: LockKey) -> Option<KeyGuard> {
        self.inner.locks.try_lock(key)
    }

    /// Wait for a slot in the window of PARAM_SET transactions in flight.
    pub(super) async fn param_permit(&self) -> Result<SemaphorePermit<'_>> {
        self.inner
            .param_permit
            .acquire()
            .await
            .map_err(|_| MavError::ResourceClosed)
    }

    /// Forward all inbound messages matching `filter` to `tx`, until removed.
    pub(super) fn insert_waiter(
        &self,
        filter: impl Fn(&InboundMessage) -> bool + Send + 'static,
        tx: mpsc::UnboundedSender<Arc<InboundMessage>>,
    ) -> u64 {
        let mut filters = lock(&self.inner.filters);

        let id = filters.next_filter_index;
        filters.next_filter_index += 1;
        filters.map.insert(
            id,
            MessageFilter {
                filter: Box::new(filter),
                tx,
            },
        );
        id
    }

    pub(super) fn remove_waiter(&self, id: u64) {
        lock(&self.inner.filters).map.remove(&id);
    }

    /// Forward the ACKs for `kind` commands from `target` to `tx`, until removed.
    pub(super) fn insert_pending(
        &self,
        target: MavlinkId,
        kind: MavCmd,
        tx: mpsc::UnboundedSender<CommandAck>,
    ) {
        lock(&self.inner.acks).map.insert((target, kind as u16), tx);
    }

    pub(super) fn remove_pending(&self, target: MavlinkId, kind: MavCmd) {
        lock(&self.inner.acks).map.remove(&(target, kind as u16));
    }

    /// Send a message towards `target` on the link it was most recently heard on.
    /// If the target has not been heard from, the message is sent on all links.
    pub(super) fn route(&self, target: MavlinkId, message: &dyn mavio::Message) -> Result<()> {
        let best = self.best_link_for(target);
        let links = lock(&self.inner.links);

        // Error when no links
        if links.is_empty() {
            return Err(MavError::NoLink);
        }

        // Send on the link that is known to be best for this target
        if let Some(link) = best.and_then(|id| links.get(&id)) {
            return link.send(self.inner.gcs_id, message);
        }

        // Unknown target: broadcast on all links to ensure arrival. The
        // broadcast succeeds if at least one link accepted the frame; only when
        // every link fails is the (last) error reported.
        let mut delivered = false;
        let mut last_error = MavError::NoLink;
        for link in links.values() {
            match link.send(self.inner.gcs_id, message) {
                Ok(()) => delivered = true,
                Err(error) => last_error = error,
            }
        }

        if delivered { Ok(()) } else { Err(last_error) }
    }

    fn send_on_link<M: mavio::Message>(&self, link: &LinkId, message: &M) -> Result<()> {
        lock(&self.inner.links)
            .get(link)
            .ok_or(MavError::NoLink)?
            .send(self.inner.gcs_id, message)
    }

    fn best_link_for(&self, target: MavlinkId) -> Option<LinkId> {
        let routes = lock(&self.inner.routes);
        let pick = |filter: &dyn Fn(&MavlinkId) -> bool| {
            routes
                .map
                .iter()
                .filter(|(source, _)| filter(source))
                .flat_map(|(_, links)| links.iter())
                .max_by_key(|(_, at)| **at)
                .map(|(link, _)| link.clone())
        };

        // Prefer the exact component, but fall back to any component of the same system
        pick(&|source| matches_target(target, *source))
            .or_else(|| pick(&|source| source.system == target.system))
    }

    fn handle_frame(&self, link: &LinkId, frame: Frame<Versionless>) {
        let source = MavlinkId {
            system: frame.system_id(),
            component: frame.component_id(),
        };

        self.dispatch(InboundMessage {
            source_link: link.clone(),
            source_mav: source,
            received_at: Instant::now(),
            frame,
        });
    }

    fn dispatch(&self, inbound: InboundMessage) {
        let inbound = Arc::new(inbound);

        lock(&self.inner.routes)
            .map
            .entry(inbound.source_mav)
            .or_default()
            .insert(inbound.source_link.clone(), inbound.received_at);

        if let Ok(ack) = inbound.frame.decode_message::<CommandAck>() {
            self.route_ack(inbound.source_mav, &ack);
        }

        lock(&self.inner.filters).map.retain(|_, waiter| {
            !(waiter.filter)(&inbound) || waiter.tx.send(inbound.clone()).is_ok()
        });

        _ = self.inner.events.send(Event::Message(inbound));
    }

    fn route_ack(&self, source: MavlinkId, ack: &CommandAck) {
        let gcs = self.inner.gcs_id;

        // Some systems leave the target fields empty, so accept those as well
        let for_us = (ack.target_system == 0 || ack.target_system == gcs.system)
            && (ack.target_component == 0 || ack.target_component == gcs.component);
        if !for_us {
            return;
        }

        let command = ack.command as u16;
        let system_wide = MavlinkId {
            system: source.system,
            component: 0,
        };

        let pending = lock(&self.inner.acks);
        let sender = pending
            .map
            .get(&(source, command))
            .or_else(|| pending.map.get(&(system_wide, command)));

        match sender {
            Some(sender) => _ = sender.send(ack.clone()),
            None => log::debug!("Unsolicited ACK from {source:?}: {ack:?}"),
        }
    }

    fn remove_link(&self, link: LinkId) {
        lock(&self.inner.links).remove(&link);
        for links in lock(&self.inner.routes).map.values_mut() {
            links.remove(&link);
        }
        _ = self.inner.events.send(Event::LinkClosed(link));
    }

    /// Attach a new transport. It stays attached until the returned handle is
    /// dropped or closed, or the transport fails.
    pub fn attach_reader_writer<R: DynAsyncReader + 'static, W: DynAsyncWriter + 'static>(
        &self,
        link_id: LinkId,
        reader: R,
        writer: W,
    ) -> LinkGuard {
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded_channel::<Frame<Versionless>>();

        lock(&self.inner.links).insert(
            link_id.clone(),
            Link {
                tx,
                sequence: AtomicU8::new(0),
            },
        );

        // Writer: drains the outbound queue into the transport
        let mut sender = new_async_sender(writer);
        let token = cancel.clone();
        let id = link_id.clone();
        tokio::spawn(async move {
            token
                .run_until_cancelled(async {
                    while let Some(frame) = rx.recv().await {
                        if let Err(error) = sender.send(&frame).await {
                            log::error!("Failed to send MAVLink frame on {id:?}: {error}");
                        }
                    }
                })
                .await;
        });

        // Reader: decodes inbound frames and dispatches them
        let mut consecutive_errors = 0;
        let mut receiver = new_async_receiver(reader);
        let router = self.clone();
        let token = cancel.clone();
        let id = link_id.clone();
        tokio::spawn(async move {
            token
                .run_until_cancelled(async {
                    while consecutive_errors < 10 {
                        let result = receiver.recv().await;

                        match result {
                            Ok(frame) => {
                                consecutive_errors = 0;
                                router.handle_frame(&id, frame)
                            }
                            Err(error) => {
                                consecutive_errors += 1;
                                log::debug!("Receive error on {id:?}: {error:?}")
                            }
                        }
                    }
                    log::warn!("Link {id:?} closing due to high error rate");
                })
                .await;

            token.cancel();
            router.remove_link(id);
        });

        // Heartbeat: announces our presence on this link
        let router = self.clone();
        let token = cancel.clone();
        let id = link_id.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
            token
                .run_until_cancelled(async {
                    loop {
                        interval.tick().await;

                        use mavio::dialects::minimal::{
                            enums::{MavAutopilot, MavType},
                            messages::Heartbeat,
                        };

                        let heartbeat = Heartbeat {
                            type_: MavType::Gcs,
                            autopilot: MavAutopilot::Invalid,
                            ..Default::default()
                        };

                        if router.send_on_link(&id, &heartbeat).is_err() {
                            break;
                        }
                    }
                })
                .await;
        });

        LinkGuard(Arc::new(cancel.drop_guard()))
    }
}

/// Keeps a link alive. The link is closed when the last clone is dropped, or
/// when [`LinkHandle::close`] is called.
#[derive(Debug, Clone)]
pub struct LinkGuard(#[allow(unused)] Arc<DropGuard>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::DEFAULT_GCS_ID;
    use iced::futures::StreamExt;
    use mavio::dialects::minimal::messages::Heartbeat;

    fn link_with_receiver() -> (Link, mpsc::UnboundedReceiver<Frame<Versionless>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Link {
                sequence: AtomicU8::new(0),
                tx,
            },
            rx,
        )
    }

    const UNKNOWN: MavlinkId = MavlinkId {
        system: 42,
        component: 7,
    };

    #[test]
    fn broadcast_succeeds_when_at_least_one_link_delivers() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);

        // One healthy link (receiver kept alive) and one dead link (receiver
        // dropped, so its `send` fails).
        let (healthy, _healthy_rx) = link_with_receiver();
        let (dead, dead_rx) = link_with_receiver();
        drop(dead_rx);

        {
            let mut links = lock(&router.inner.links);
            links.insert(LinkId::new("healthy"), healthy);
            links.insert(LinkId::new("dead"), dead);
        }

        // Unknown target: broadcast. A single delivery is enough to succeed.
        assert!(router.route(UNKNOWN, &Heartbeat::default()).is_ok());
    }

    #[test]
    fn broadcast_fails_when_every_link_fails() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);

        let (a, rx_a) = link_with_receiver();
        let (b, rx_b) = link_with_receiver();
        drop(rx_a);
        drop(rx_b);

        {
            let mut links = lock(&router.inner.links);
            links.insert(LinkId::new("a"), a);
            links.insert(LinkId::new("b"), b);
        }

        assert!(router.route(UNKNOWN, &Heartbeat::default()).is_err());
    }

    #[tokio::test]
    async fn event_stream_drops_oldest_on_overflow() {
        let (router, events) = MavlinkRouter::new(DEFAULT_GCS_ID);

        // Overflow the bounded channel without polling the stream.
        let total = EVENT_CHANNEL_CAPACITY + 10;
        for i in 0..total {
            let _ = router
                .inner
                .events
                .send(Event::LinkClosed(LinkId::new(&i.to_string())));
        }

        // Close the sender so the stream ends once its buffer is drained.
        drop(router);

        let ids: Vec<LinkId> = events
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter_map(|event| match event {
                Event::LinkClosed(id) => Some(id),
                Event::Message(_) => None,
            })
            .collect();

        // Exactly `total - EVENT_CHANNEL_CAPACITY` oldest events were dropped,
        // and the newest event still reaches the stream.
        assert_eq!(ids.first(), Some(&LinkId::new("10")));
        let newest = (total - 1).to_string();
        assert_eq!(ids.last(), Some(&LinkId::new(&newest)));
    }
}
