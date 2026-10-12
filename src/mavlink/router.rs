use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use futures::{Stream, stream};
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
    acks::PendingAckMap,
    dedup::Dedup,
    error::{MavError, Result},
    io::{new_async_receiver, new_async_sender},
    lock,
    locks::{KeyGuard, LockKey, LockMap},
    matches_target,
};
use crate::{
    foundation::MavlinkId,
    link::{
        LinkId,
        transport::{DynAsyncReader, DynAsyncWriter},
    },
};

/// Maximum number of PARAM_SET transactions in flight at once.
const PARAM_SET_PERMITS: usize = 4;

/// Consecutive failed sends the writer task tolerates before it gives up and
/// cancels the link. The reader task's tail performs the actual teardown.
const MAX_CONSECUTIVE_SEND_FAILURES: u32 = 10;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// Reduced cadence for high-latency links (as QGroundControl does), to save
/// bandwidth on expensive links such as satellite.
const HIGH_LATENCY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

fn heartbeat_interval(high_latency: bool) -> Duration {
    if high_latency {
        HIGH_LATENCY_HEARTBEAT_INTERVAL
    } else {
        HEARTBEAT_INTERVAL
    }
}

/// How long a mission transfer waits for a `MISSION_ACK` (or `MISSION_COUNT`)
/// before retrying, on a normal-latency link.
pub(super) const MISSION_ACK_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long a mission transfer waits for a requested mission item before
/// re-requesting it, on a normal-latency link.
pub(super) const MISSION_ITEM_TIMEOUT: Duration = Duration::from_millis(250);

/// Mission-transfer timeout multipliers for high-latency links.
///
/// The 5 s high-latency heartbeat cadence already marks a link as slow: an
/// answer may not arrive until the next cadence plus a typical satcom RTT.
/// The short per-item wait that makes lost-item retransmission quick on a
/// fast link punishes a satellite link, so both waits scale by fixed
/// factors: the item timeout ×8 (250 ms → 2 s), the ack timeout ×3
/// (1.5 s → 4.5 s).
const HIGH_LATENCY_ITEM_FACTOR: u32 = 8;
const HIGH_LATENCY_ACK_FACTOR: u32 = 3;

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
    generation: u64,
    /// Whether this transport's latency warrants the scaled mission-transfer
    /// timeouts ([`MavlinkRouter::transfer_timeouts`]).
    high_latency: bool,
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

struct MavlinkRouterInner {
    gcs_id: MavlinkId,
    events: broadcast::Sender<Event>,
    links: Mutex<BTreeMap<LinkId, Link>>,
    next_link_generation: AtomicU64,
    routes: Mutex<MessageRouteMap>,
    /// The cross-link duplicate suppression used by [`MavlinkRouter::dispatch`],
    /// keyed per source, with reset guards.
    dedup: Dedup,
    filters: Mutex<MessageFilterMap>,
    acks: PendingAckMap,
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
                next_link_generation: AtomicU64::new(0),
                routes: Mutex::default(),
                dedup: Dedup::default(),
                filters: Mutex::default(),
                acks: PendingAckMap::default(),
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
    ///
    /// Calls are serialized per `(target, kind)` by their transactions; this
    /// replaces only a stale sender.
    pub(super) fn insert_pending(
        &self,
        target: MavlinkId,
        kind: MavCmd,
        tx: mpsc::UnboundedSender<CommandAck>,
    ) {
        self.inner.acks.insert(target, kind, tx);
    }

    pub(super) fn remove_pending(&self, target: MavlinkId, kind: MavCmd) {
        self.inner.acks.remove(target, kind);
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

    /// The mission-transfer timeouts for `target`: the wait for a requested
    /// mission item and the wait for an ack. Scaled up when the best route to
    /// `target` is a high-latency link; an unknown target keeps the fast
    /// baseline. The route is a snapshot for one transfer.
    pub(super) fn transfer_timeouts(&self, target: MavlinkId) -> (Duration, Duration) {
        let high_latency = self
            .best_link_for(target)
            .map(|link| {
                lock(&self.inner.links)
                    .get(&link)
                    .is_some_and(|link| link.high_latency)
            })
            .unwrap_or(false);

        if high_latency {
            (
                MISSION_ITEM_TIMEOUT * HIGH_LATENCY_ITEM_FACTOR,
                MISSION_ACK_TIMEOUT * HIGH_LATENCY_ACK_FACTOR,
            )
        } else {
            (MISSION_ITEM_TIMEOUT, MISSION_ACK_TIMEOUT)
        }
    }

    /// Whether `link` currently has a live attachment.
    ///
    /// Reports what the router itself tracks: a dropped guard's teardown and a
    /// failed transport both detach the link, even before the application has
    /// seen the corresponding event.
    pub fn is_attached(&self, link: &LinkId) -> bool {
        lock(&self.inner.links).contains_key(link)
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
        let sequence = inbound.frame.sequence();
        let inbound = Arc::new(inbound);

        // Hearing a frame on a link proves that link is alive, so the route
        // table is updated before the duplicate check: an inbound duplicate
        // still influences which link is best for further traffic.
        lock(&self.inner.routes)
            .map
            .entry(inbound.source_mav)
            .or_default()
            .insert(inbound.source_link.clone(), inbound.received_at);

        // Cross-link duplicate suppression: the same packet reaching state
        // consumers twice would double-fold into vehicle state (duplicated
        // statustexts, doubled trail points). Only the first arriving copy
        // advances; the needed ordering above is unaffected.
        if self
            .inner
            .dedup
            .seen_sequence_before(inbound.source_mav, sequence, inbound.received_at)
        {
            log::debug!(
                "Dropping duplicate packet sequence {sequence} from {:?} on {:?}",
                inbound.source_mav,
                inbound.source_link
            );
            return;
        }

        if let Ok(ack) = inbound.frame.decode_message::<CommandAck>() {
            self.inner
                .acks
                .route_ack(self.inner.gcs_id, inbound.source_mav, &ack);
        }

        lock(&self.inner.filters).map.retain(|_, waiter| {
            !(waiter.filter)(&inbound) || waiter.tx.send(inbound.clone()).is_ok()
        });

        _ = self.inner.events.send(Event::Message(inbound));
    }

    fn remove_link(&self, link: &LinkId, generation: u64) {
        {
            let mut links = lock(&self.inner.links);
            match links.get(link) {
                Some(existing) if existing.generation == generation => {
                    links.remove(link);
                }
                _ => return,
            }
        }

        for links in lock(&self.inner.routes).map.values_mut() {
            links.remove(link);
        }
        _ = self.inner.events.send(Event::LinkClosed(link.clone()));
    }

    /// Attach a new transport. It stays attached until the returned handle is
    /// dropped or closed, or the transport fails.
    pub fn attach_reader_writer<R: DynAsyncReader + 'static, W: DynAsyncWriter + 'static>(
        &self,
        link_id: LinkId,
        reader: R,
        writer: W,
        high_latency: bool,
    ) -> LinkGuard {
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded_channel::<Frame<Versionless>>();

        let generation = self
            .inner
            .next_link_generation
            .fetch_add(1, Ordering::Relaxed);

        lock(&self.inner.links).insert(
            link_id.clone(),
            Link {
                tx,
                sequence: AtomicU8::new(0),
                generation,
                high_latency,
            },
        );

        // Writer: drains the outbound queue into the transport. Repeated send
        // failures mean the transport is gone: the reader task's tail performs
        // the teardown, so give up sending after a bounded streak.
        let mut sender = new_async_sender(writer);
        let token = cancel.clone();
        let id = link_id.clone();
        tokio::spawn(async move {
            token
                .run_until_cancelled(async {
                    let mut failures = 0;

                    while let Some(frame) = rx.recv().await {
                        match sender.send(&frame).await {
                            Ok(_) => failures = 0,
                            Err(error) => {
                                failures += 1;
                                log::error!("Failed to send MAVLink frame on {id:?}: {error}");

                                if failures >= MAX_CONSECUTIVE_SEND_FAILURES {
                                    log::warn!(
                                        "Link {id:?} closing after \
                                         {MAX_CONSECUTIVE_SEND_FAILURES} consecutive send failures"
                                    );
                                    break;
                                }
                            }
                        }
                    }
                })
                .await;

            token.cancel();
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
            router.remove_link(&id, generation);
        });

        // Heartbeat: announces our presence on this link
        let router = self.clone();
        let token = cancel.clone();
        let id = link_id.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_interval(high_latency));
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

                        if let Err(error) = router.send_on_link(&id, &heartbeat) {
                            log::warn!("heartbeat stopped on {id:?}: {error}");
                            break;
                        }
                    }
                })
                .await;

            // Converge on the writer's teardown path: whichever subsystem gives
            // up first, the reader's tail performs the actual `remove_link`.
            token.cancel();
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
    use crate::mavlink::DEFAULT_GCS_ID;
    use futures::StreamExt;
    use mavio::dialects::minimal::messages::Heartbeat;

    fn link_with_receiver() -> (Link, mpsc::UnboundedReceiver<Frame<Versionless>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Link {
                sequence: AtomicU8::new(0),
                tx,
                generation: 0,
                high_latency: false,
            },
            rx,
        )
    }

    const UNKNOWN: MavlinkId = MavlinkId {
        system: 42,
        component: 7,
    };

    #[test]
    fn heartbeat_interval_slows_for_high_latency() {
        assert_eq!(heartbeat_interval(false), HEARTBEAT_INTERVAL);
        assert_eq!(heartbeat_interval(true), HIGH_LATENCY_HEARTBEAT_INTERVAL);
        assert!(heartbeat_interval(true) > heartbeat_interval(false));
    }

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

    #[test]
    fn stale_remove_link_does_not_evict_newer_attachment() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let id = LinkId::new("link");

        let (newer, _newer_rx) = link_with_receiver();
        lock(&router.inner.links).insert(
            id.clone(),
            Link {
                generation: 2,
                ..newer
            },
        );

        // A stale reader (generation 1) must not evict the generation-2 link.
        router.remove_link(&id, 1);
        assert!(lock(&router.inner.links).contains_key(&id));

        // The matching generation removes it.
        router.remove_link(&id, 2);
        assert!(!lock(&router.inner.links).contains_key(&id));
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

    /// Build an inbound frame from `source` with packet `sequence`.
    fn frame_from(source: MavlinkId, sequence: u8) -> Frame<Versionless> {
        FrameBuilder::new()
            .system_id(source.system)
            .component_id(source.component)
            .version(V2)
            .sequence(sequence)
            .message(&Heartbeat::default())
            .unwrap()
            .build()
            .into_versionless()
    }

    #[test]
    fn duplicate_frames_across_links_reach_consumers_once() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);

        let (tx, mut waiter) = mpsc::unbounded_channel();
        router.insert_waiter(|_| true, tx);
        let mut events = router.inner.events.subscribe();

        let source = MavlinkId {
            system: 9,
            component: 1,
        };
        let link_a = LinkId::new("a");
        let link_b = LinkId::new("b");

        // The same packet heard on two links: delivered to consumers exactly
        // once, while both links are recorded as alive for the source.
        router.handle_frame(&link_a, frame_from(source, 7));
        router.handle_frame(&link_b, frame_from(source, 7));

        assert!(waiter.try_recv().is_ok(), "the first copy is delivered");
        assert!(
            waiter.try_recv().is_err(),
            "the duplicate must not reach consumers again"
        );
        assert!(
            matches!(events.try_recv(), Ok(Event::Message(_))),
            "the first copy is broadcast"
        );
        assert!(events.try_recv().is_err(), "the duplicate is not broadcast");

        let routes = lock(&router.inner.routes);
        let heard_on = routes.map.get(&source).expect("source routed");
        assert!(heard_on.contains_key(&link_a), "first link recorded");
        assert!(
            heard_on.contains_key(&link_b),
            "duplicate still proved the second link alive"
        );
        drop(routes);

        // A different sequence is never a duplicate.
        router.handle_frame(&link_a, frame_from(source, 8));
        assert!(waiter.try_recv().is_ok());

        // Window semantics: after another 192 distinct sequences the ring has
        // evicted the original one, so it may be delivered again.
        for seq in 9..=200u8 {
            router.handle_frame(&link_b, frame_from(source, seq));
        }
        router.handle_frame(&link_a, frame_from(source, 7));
        assert!(
            waiter.try_recv().is_ok(),
            "an evicted sequence passes again"
        );
    }

    #[tokio::test]
    async fn is_attached_reflects_the_links_map() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let id = LinkId::new("link");

        let (_peer, gcs) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(gcs);
        let guard = router.attach_reader_writer(id.clone(), reader, writer, false);

        assert!(router.is_attached(&id));

        // Dropping the guard cancels the attachment; the reader task's tail
        // removes the link, which `is_attached` reflects after teardown.
        drop(guard);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!router.is_attached(&id));
    }

    #[tokio::test]
    async fn send_failures_cancel_the_link_connection() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let id = LinkId::new("link");

        let (peer, gcs) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(gcs);
        let _guard = router.attach_reader_writer(id.clone(), reader, writer, false);

        assert!(router.is_attached(&id));

        // Kill the transport peer, then push enough frames through the writer
        // task to hit the consecutive-failure threshold. The writer cancels
        // its token, and the reader task's tail runs the teardown.
        drop(peer);
        for _ in 0..MAX_CONSECUTIVE_SEND_FAILURES {
            router
                .send_on_link(&id, &Heartbeat::default())
                .expect("the writer queue accepted the frame");
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !router.is_attached(&id),
            "blocked transport must tear the link down"
        );
    }

    /// A link that can still read but can no longer send heartbeats must be torn
    /// down, not left silently "connected". The heartbeat task converges on the
    /// writer's teardown path: on send failure it cancels the token, so the
    /// reader's tail runs `remove_link`.
    #[tokio::test]
    async fn heartbeat_send_failure_tears_the_link_down() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let id = LinkId::new("link");

        let (_peer, gcs) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(gcs);
        let _guard = router.attach_reader_writer(id.clone(), reader, writer, false);

        assert!(router.is_attached(&id));

        // Detach the link's outbound queue from the live writer task: the next
        // heartbeat send fails, while the reader keeps the transport readable
        // and the writer's own queue stays connected, so only the heartbeat
        // notices. Keeping the original sender alive is what stops the writer
        // from seeing its channel close and tearing the link down itself.
        let (dead_tx, dead_rx) = mpsc::unbounded_channel();
        drop(dead_rx);
        let _live_tx = {
            let mut links = lock(&router.inner.links);
            let link = links.get_mut(&id).expect("just attached");
            std::mem::replace(&mut link.tx, dead_tx)
        };

        // Let the heartbeat task's first tick run and give up.
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(
            !router.is_attached(&id),
            "a failed heartbeat must tear the link down"
        );
    }

    #[test]
    fn transfer_timeouts_scale_by_link_latency() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);

        let target = MavlinkId {
            system: 7,
            component: 1,
        };

        // Fast link known as the best route: baselines.
        let (fast, _fast_rx) = link_with_receiver();
        let (slow, _slow_rx) = link_with_receiver();
        let slow = Link {
            high_latency: true,
            ..slow
        };

        {
            let mut links = lock(&router.inner.links);
            links.insert(LinkId::new("fast"), fast);
            links.insert(LinkId::new("slow"), slow);
        }

        // Seed the route map: the target heard on each link, most recently on
        // the high-latency one.
        let seed = |link: &str| {
            let mut routes = lock(&router.inner.routes);
            routes
                .map
                .entry(target)
                .or_default()
                .insert(LinkId::new(link), Instant::now());
        };

        seed("slow");
        let (item, ack) = router.transfer_timeouts(target);
        assert_eq!(item, MISSION_ITEM_TIMEOUT * HIGH_LATENCY_ITEM_FACTOR);
        assert_eq!(ack, MISSION_ACK_TIMEOUT * HIGH_LATENCY_ACK_FACTOR);

        // Hearing the target on the fast link afterwards makes it the best route.
        seed("fast");
        let (item, ack) = router.transfer_timeouts(target);
        assert_eq!(item, MISSION_ITEM_TIMEOUT);
        assert_eq!(ack, MISSION_ACK_TIMEOUT);

        // No route knowledge at all: baselines.
        let (item, ack) = router.transfer_timeouts(UNKNOWN);
        assert_eq!(item, MISSION_ITEM_TIMEOUT);
        assert_eq!(ack, MISSION_ACK_TIMEOUT);
    }
}
