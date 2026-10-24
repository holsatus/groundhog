//! The router: the links it owns, and every frame in and out of them.
//!
//! Each attached link gets a reader, a writer and a heartbeat task. The
//! reader parses frames, drops copies of a packet already heard on another
//! link (see `dedup`), refreshes the route to the sender from the first copy,
//! hands command ACKs and transaction replies to whoever waits for them, and
//! passes every message on through one lossless event channel, which pushes
//! back on the transports rather than drop. The writer drains two queues per
//! link, control before bulk, so a burst of parameter re-requests never
//! delays a command. A frame to a target takes the link that most recently
//! brought the first copy of a frame from it, and every link while the
//! target has not been heard from.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use mavio::{
    Frame,
    default_dialect::{enums::MavCmd, messages::CommandAck},
    prelude::{V2, Versionless},
    protocol::{FrameBuilder, Payload},
};
use tokio::sync::{Semaphore, SemaphorePermit, mpsc};
use tokio_util::sync::{CancellationToken, DropGuard};

use super::{
    Targeted,
    acks::PendingAckMap,
    dedup::{Dedup, PacketKey},
    error::{MavError, Result},
    io::{new_async_receiver, new_async_sender},
    lock,
    locks::{KeyGuard, LockKey, LockMap},
    matches_target,
};
use crate::{
    MavlinkId,
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

/// Consecutive transport read errors after which the reader gives up.
const MAX_CONSECUTIVE_READ_ERRORS: u32 = 10;

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

/// Bound on router events waiting for the consumer.
///
/// The channel is lossless: when it is full, the link reader tasks wait for
/// room, which pushes back on the transports (TCP flow control; datagram and
/// serial buffers overflow at the OS instead). The consumer folds a message in
/// microseconds, so the bound is only reached if the consumer is stuck.
const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// Outbound queue bounds per link. Control traffic (heartbeats, commands,
/// mission and parameter transactions) overtakes bulk traffic (parameter
/// re-requests), so a burst of bulk frames never delays a command.
const CONTROL_QUEUE_CAPACITY: usize = 256;
const BULK_QUEUE_CAPACITY: usize = 1024;

/// Which outbound queue a frame joins on its link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// Heartbeats, commands and transaction traffic: always sent first.
    #[default]
    Control,
    /// Bulk traffic that tolerates delay, such as parameter re-requests.
    Bulk,
}

/// A decoded inbound MAVLink message, along with where it came from.
#[derive(Debug)]
pub struct InboundMessage {
    pub received_at: Instant,
    pub source_link: LinkId,
    pub source_mav: MavlinkId,
    pub frame: Frame<Versionless>,
}

/// Events emitted from the router to its consumer.
#[derive(Debug, Clone)]
pub enum Event {
    Message(Arc<InboundMessage>),
    LinkClosed(LinkId),
}

/// The receiving end of the router's lossless event channel.
pub type EventReceiver = mpsc::Receiver<Event>;

struct MessageFilter {
    filter: Box<dyn Fn(&InboundMessage) -> bool + Send>,
    tx: mpsc::UnboundedSender<Arc<InboundMessage>>,
}

/// A message encoded for the wire, waiting in an outbound queue. The frame is
/// assembled by the writer task, so sequence numbers are assigned in the order
/// frames actually leave, whatever their priority.
struct Outbound {
    payload: Payload,
    crc_extra: u8,
}

impl Outbound {
    fn encode(message: &dyn mavio::Message) -> Result<Self> {
        Ok(Outbound {
            payload: message
                .encode(mavio::protocol::MavLinkVersion::V2)
                .map_err(|error| MavError::Encode(format!("{error:?}")))?,
            crc_extra: message.crc_extra(),
        })
    }

    fn into_frame(self, gcs: MavlinkId, sequence: u8) -> Frame<Versionless> {
        FrameBuilder::new()
            .version(V2)
            .sequence(sequence)
            .system_id(gcs.system)
            .component_id(gcs.component)
            .message_id(self.payload.id())
            .payload(self.payload.bytes())
            .crc_extra(self.crc_extra)
            .build()
            .into_versionless()
    }
}

/// Whether `frame`'s checksum does not match its message, as known to the
/// dialect. Messages outside the dialect cannot be checked, and pass.
fn is_corrupt(frame: &Frame<Versionless>) -> bool {
    matches!(
        frame.validate_checksum::<mavio::DefaultDialect>(),
        Err(mavio::error::Error::Frame(
            mavio::error::FrameError::Checksum
        ))
    )
}

/// Traffic counters of one link attachment.
#[derive(Debug, Default)]
struct LinkCounters {
    rx_frames: AtomicU64,
    tx_frames: AtomicU64,
    read_errors: AtomicU64,
    parse_errors: AtomicU64,
    write_errors: AtomicU64,
    /// Milliseconds since the router's epoch at the last received frame, plus
    /// one (zero means never).
    last_rx: AtomicU64,
}

/// A snapshot of one link's traffic counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkStats {
    /// Frames received intact.
    pub rx_frames: u64,
    pub tx_frames: u64,
    /// Reads from the transport that failed.
    pub read_errors: u64,
    /// Received data that did not parse as a frame, or a frame whose
    /// checksum does not match its message.
    pub parse_errors: u64,
    /// Writes to the transport that failed.
    pub write_errors: u64,
    /// When the last frame was received on this link, if ever.
    pub last_rx: Option<Instant>,
}

struct Link {
    control: mpsc::Sender<Outbound>,
    bulk: mpsc::Sender<Outbound>,
    generation: u64,
    /// Whether this transport's latency warrants the scaled mission-transfer
    /// timeouts ([`MavlinkRouter::transfer_timeouts`]).
    high_latency: bool,
    counters: Arc<LinkCounters>,
}

impl Link {
    fn send(&self, message: &dyn mavio::Message, priority: Priority) -> Result<()> {
        let outbound = Outbound::encode(message)?;

        let queue = match priority {
            Priority::Control => &self.control,
            Priority::Bulk => &self.bulk,
        };

        queue.try_send(outbound).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => MavError::LinkCongested,
            mpsc::error::TrySendError::Closed(_) => MavError::NoLink,
        })
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
    epoch: Instant,
    events: mpsc::Sender<Event>,
    links: Mutex<BTreeMap<LinkId, Link>>,
    next_link_generation: AtomicU64,
    routes: Mutex<MessageRouteMap>,
    /// The cross-link duplicate suppression used by [`MavlinkRouter::handle_frame`],
    /// keyed per source, with reset guards.
    dedup: Dedup,
    filters: Mutex<MessageFilterMap>,
    acks: PendingAckMap,
    locks: LockMap,
    param_permit: Semaphore,
}

impl MavlinkRouter {
    /// Create a new router stamping outbound frames with `gcs_id`, along with
    /// the receiving end of its lossless event channel.
    ///
    /// Exactly one consumer drains the events. Transactions do not depend on
    /// it: they install their own filters (see [`MavlinkRouter::receive_filtered`]).
    pub fn new(gcs_id: MavlinkId) -> (Self, EventReceiver) {
        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);

        let router = Self {
            inner: Arc::new(MavlinkRouterInner {
                gcs_id,
                epoch: Instant::now(),
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

        (router, rx)
    }

    /// The identity stamped on every outbound frame.
    pub fn gcs_id(&self) -> MavlinkId {
        self.inner.gcs_id
    }

    /// Whether a transaction for `key` is currently in flight or queued.
    /// Useful for reflecting in the UI whether an operation is ongoing.
    #[cfg(test)] // Only exercised by the middleware tests
    pub fn is_busy(&self, key: &LockKey) -> bool {
        self.inner.locks.is_busy(key)
    }

    /// Send a message to `target`, filling in the target fields.
    pub fn send_targeted<M: Targeted>(&self, target: MavlinkId, mut message: M) -> Result<()> {
        message.set_target(target);
        self.route(target, &message, Priority::Control)
    }

    /// Like [`MavlinkRouter::send_targeted`], on the bulk queue.
    pub fn send_targeted_bulk<M: Targeted>(&self, target: MavlinkId, mut message: M) -> Result<()> {
        message.set_target(target);
        self.route(target, &message, Priority::Bulk)
    }

    /// Acquire the lock for `key`, waiting in line if it is already held.
    pub(super) async fn lock_key(&self, key: LockKey) -> KeyGuard {
        self.inner.locks.lock(key).await
    }

    /// Acquire the lock for `key` only if nobody else holds or waits for it.
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
    pub(super) fn route(
        &self,
        target: MavlinkId,
        message: &dyn mavio::Message,
        priority: Priority,
    ) -> Result<()> {
        let best = self.best_link_for(target);
        let links = lock(&self.inner.links);

        // Error when no links
        if links.is_empty() {
            return Err(MavError::NoLink);
        }

        // Send on the link that is known to be best for this target
        if let Some(link) = best.and_then(|id| links.get(&id)) {
            return link.send(message, priority);
        }

        // Unknown target: broadcast on all links to ensure arrival. The
        // broadcast succeeds if at least one link accepted the frame; only when
        // every link fails is the (last) error reported.
        let mut delivered = false;
        let mut last_error = MavError::NoLink;
        for link in links.values() {
            match link.send(message, priority) {
                Ok(()) => delivered = true,
                Err(error) => last_error = error,
            }
        }

        if delivered { Ok(()) } else { Err(last_error) }
    }

    fn send_on_link(&self, link: &LinkId, message: &dyn mavio::Message) -> Result<()> {
        lock(&self.inner.links)
            .get(link)
            .ok_or(MavError::NoLink)?
            .send(message, Priority::Control)
    }

    /// The link a frame towards `target` should take: the one that most
    /// recently delivered the *first* copy of a frame from it.
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
    #[cfg(test)]
    pub fn is_attached(&self, link: &LinkId) -> bool {
        lock(&self.inner.links).contains_key(link)
    }

    /// The traffic counters of `link`'s current attachment, if attached.
    pub fn link_stats(&self, link: &LinkId) -> Option<LinkStats> {
        let links = lock(&self.inner.links);
        let counters = &links.get(link)?.counters;
        let last_rx = counters.last_rx.load(Ordering::Relaxed);

        Some(LinkStats {
            rx_frames: counters.rx_frames.load(Ordering::Relaxed),
            tx_frames: counters.tx_frames.load(Ordering::Relaxed),
            read_errors: counters.read_errors.load(Ordering::Relaxed),
            parse_errors: counters.parse_errors.load(Ordering::Relaxed),
            write_errors: counters.write_errors.load(Ordering::Relaxed),
            last_rx: last_rx
                .checked_sub(1)
                .map(|millis| self.inner.epoch + Duration::from_millis(millis)),
        })
    }

    /// Process one inbound frame from `link`: route bookkeeping, duplicate
    /// suppression, ACK routing and transaction filters. Returns the event for
    /// the consumer, or `None` for a duplicate.
    fn handle_frame(&self, link: &LinkId, frame: Frame<Versionless>) -> Option<Event> {
        let source = MavlinkId {
            system: frame.system_id(),
            component: frame.component_id(),
        };

        self.dispatch(InboundMessage {
            source_link: link.clone(),
            source_mav: source,
            received_at: Instant::now(),
            frame,
        })
    }

    fn dispatch(&self, inbound: InboundMessage) -> Option<Event> {
        let packet = PacketKey {
            sequence: inbound.frame.sequence(),
            message_id: inbound.frame.message_id(),
            checksum: inbound.frame.checksum(),
        };

        // Cross-link duplicate suppression: the same packet reaching state
        // consumers twice would double-fold into vehicle state (duplicated
        // statustexts, doubled trail points). Only the first arriving copy
        // advances.
        if self
            .inner
            .dedup
            .seen_before(inbound.source_mav, packet, inbound.received_at)
        {
            log::trace!(
                "Dropping duplicate packet {packet:?} from {:?} on {:?}",
                inbound.source_mav,
                inbound.source_link
            );
            return None;
        }

        // Only a first-arriving copy refreshes the route: with redundant links
        // the duplicate always lands on the slower one, which must not become
        // the preferred path for commands. A link that only ever delivers
        // duplicates takes over as soon as the faster link stops delivering.
        lock(&self.inner.routes)
            .map
            .entry(inbound.source_mav)
            .or_default()
            .insert(inbound.source_link.clone(), inbound.received_at);

        let inbound = Arc::new(inbound);

        if let Ok(ack) = inbound.frame.decode_message::<CommandAck>() {
            self.inner
                .acks
                .route_ack(self.inner.gcs_id, inbound.source_mav, &ack);
        }

        lock(&self.inner.filters).map.retain(|_, waiter| {
            !(waiter.filter)(&inbound) || waiter.tx.send(inbound.clone()).is_ok()
        });

        Some(Event::Message(inbound))
    }

    /// Detach `link` if its current attachment is `generation`, returning
    /// whether it did.
    fn remove_link(&self, link: &LinkId, generation: u64) -> bool {
        {
            let mut links = lock(&self.inner.links);
            match links.get(link) {
                Some(existing) if existing.generation == generation => {
                    links.remove(link);
                }
                _ => return false,
            }
        }

        for links in lock(&self.inner.routes).map.values_mut() {
            links.remove(link);
        }

        true
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
        let (control_tx, mut control_rx) = mpsc::channel::<Outbound>(CONTROL_QUEUE_CAPACITY);
        let (bulk_tx, mut bulk_rx) = mpsc::channel::<Outbound>(BULK_QUEUE_CAPACITY);
        let counters = Arc::new(LinkCounters::default());

        let generation = self
            .inner
            .next_link_generation
            .fetch_add(1, Ordering::Relaxed);

        lock(&self.inner.links).insert(
            link_id.clone(),
            Link {
                control: control_tx,
                bulk: bulk_tx,
                generation,
                high_latency,
                counters: counters.clone(),
            },
        );

        // Writer: drains the outbound queues into the transport, control
        // first. Repeated send failures mean the transport is gone: the
        // reader task's tail performs the teardown, so give up sending after a
        // bounded streak.
        let mut sender = new_async_sender(writer);
        let token = cancel.clone();
        let id = link_id.clone();
        let gcs = self.inner.gcs_id;
        let tx_counters = counters.clone();
        tokio::spawn(async move {
            token
                .run_until_cancelled(async {
                    let mut failures = 0;
                    let mut sequence: u8 = 0;

                    loop {
                        let outbound = tokio::select! {
                            biased;
                            Some(outbound) = control_rx.recv() => outbound,
                            Some(outbound) = bulk_rx.recv() => outbound,
                            else => break,
                        };

                        let frame = outbound.into_frame(gcs, sequence);
                        sequence = sequence.wrapping_add(1);

                        match sender.send(&frame).await {
                            Ok(_) => {
                                failures = 0;
                                tx_counters.tx_frames.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(error) => {
                                failures += 1;
                                tx_counters.write_errors.fetch_add(1, Ordering::Relaxed);
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
        let mut receiver = new_async_receiver(reader);
        let router = self.clone();
        let token = cancel.clone();
        let id = link_id.clone();
        tokio::spawn(async move {
            token
                .run_until_cancelled(async {
                    let mut consecutive_errors = 0;

                    while consecutive_errors < MAX_CONSECUTIVE_READ_ERRORS {
                        match receiver.recv().await {
                            Ok(frame) => {
                                consecutive_errors = 0;
                                // A frame whose checksum does not match its
                                // message was corrupted in transit. It is still
                                // routed (decoding refuses it later), but it
                                // counts as a parse error, not a good frame.
                                let counter = if is_corrupt(&frame) {
                                    &counters.parse_errors
                                } else {
                                    &counters.rx_frames
                                };
                                counter.fetch_add(1, Ordering::Relaxed);
                                let millis = router.inner.epoch.elapsed().as_millis() as u64;
                                counters.last_rx.store(millis + 1, Ordering::Relaxed);

                                if let Some(event) = router.handle_frame(&id, frame) {
                                    // Lossless: wait for room rather than drop.
                                    // A closed channel means nobody consumes
                                    // events any more, which is not an error
                                    // for the transactions.
                                    let _ = router.inner.events.send(event).await;
                                }
                            }
                            Err(error) => {
                                consecutive_errors += 1;
                                let counter = match error {
                                    mavio::error::Error::Io(_) => &counters.read_errors,
                                    _ => &counters.parse_errors,
                                };
                                counter.fetch_add(1, Ordering::Relaxed);
                                log::debug!("Receive error on {id:?}: {error:?}")
                            }
                        }
                    }
                    log::warn!("Link {id:?} closing due to high error rate");
                })
                .await;

            token.cancel();
            if router.remove_link(&id, generation) {
                let _ = router.inner.events.send(Event::LinkClosed(id)).await;
            }
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
                            enums::{MavAutopilot, MavState, MavType},
                            messages::Heartbeat,
                        };

                        // The defaults would send protocol version 0 and an
                        // uninitialised state; the message defines both.
                        let heartbeat = Heartbeat {
                            type_: MavType::Gcs,
                            autopilot: MavAutopilot::Invalid,
                            system_status: MavState::Active,
                            mavlink_version: 3,
                            ..Default::default()
                        };

                        match router.send_on_link(&id, &heartbeat) {
                            Ok(()) => {}
                            // A momentarily full queue is not a dead link: the
                            // writer is draining, and the next beat retries.
                            Err(MavError::LinkCongested) => {
                                log::debug!("heartbeat skipped on congested {id:?}");
                            }
                            Err(error) => {
                                log::warn!("heartbeat stopped on {id:?}: {error}");
                                break;
                            }
                        }
                    }
                })
                .await;

            // Converge on the writer's teardown path: whichever subsystem gives
            // up first, the reader's tail performs the actual `remove_link`.
            token.cancel();
        });

        LinkGuard {
            _keep_alive: Arc::new(cancel.clone().drop_guard()),
            closed: cancel,
        }
    }
}

/// Keeps a link alive. The link is closed when the last clone is dropped.
#[derive(Debug, Clone)]
pub struct LinkGuard {
    _keep_alive: Arc<DropGuard>,
    closed: CancellationToken,
}

impl LinkGuard {
    /// Resolves once the attachment has been torn down, for whatever reason:
    /// a transport failure, or another clone of this guard being dropped.
    pub async fn closed(&self) {
        self.closed.cancelled().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::DEFAULT_GCS_ID;
    use mavio::default_dialect::messages::Attitude;
    use mavio::dialects::minimal::messages::Heartbeat;

    /// A link whose queues are drained by the returned receivers (or not, if
    /// they are dropped).
    fn link_with_receiver() -> (Link, mpsc::Receiver<Outbound>, mpsc::Receiver<Outbound>) {
        let (control, control_rx) = mpsc::channel(CONTROL_QUEUE_CAPACITY);
        let (bulk, bulk_rx) = mpsc::channel(BULK_QUEUE_CAPACITY);
        (
            Link {
                control,
                bulk,
                generation: 0,
                high_latency: false,
                counters: Arc::default(),
            },
            control_rx,
            bulk_rx,
        )
    }

    const UNKNOWN: MavlinkId = MavlinkId {
        system: 42,
        component: 7,
    };

    const SOURCE: MavlinkId = MavlinkId {
        system: 9,
        component: 1,
    };

    /// Build an inbound frame from `source` with packet `sequence`.
    fn frame_from(
        source: MavlinkId,
        sequence: u8,
        message: &dyn mavio::Message,
    ) -> Frame<Versionless> {
        FrameBuilder::new()
            .system_id(source.system)
            .component_id(source.component)
            .version(V2)
            .sequence(sequence)
            .message(message)
            .unwrap()
            .build()
            .into_versionless()
    }

    #[test]
    fn heartbeat_interval_slows_for_high_latency() {
        assert_eq!(heartbeat_interval(false), HEARTBEAT_INTERVAL);
        assert_eq!(heartbeat_interval(true), HIGH_LATENCY_HEARTBEAT_INTERVAL);
        assert!(heartbeat_interval(true) > heartbeat_interval(false));
    }

    #[test]
    fn broadcast_succeeds_when_at_least_one_link_delivers() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);

        // One healthy link (receivers kept alive) and one dead link (receivers
        // dropped, so its `send` fails).
        let (healthy, _healthy_control, _healthy_bulk) = link_with_receiver();
        let (dead, _, _) = link_with_receiver();

        {
            let mut links = lock(&router.inner.links);
            links.insert(LinkId::new("healthy"), healthy);
            links.insert(LinkId::new("dead"), dead);
        }

        // Unknown target: broadcast. A single delivery is enough to succeed.
        assert!(
            router
                .route(UNKNOWN, &Heartbeat::default(), Priority::Control)
                .is_ok()
        );
    }

    #[test]
    fn broadcast_fails_when_every_link_fails() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);

        let (a, _, _) = link_with_receiver();
        let (b, _, _) = link_with_receiver();

        {
            let mut links = lock(&router.inner.links);
            links.insert(LinkId::new("a"), a);
            links.insert(LinkId::new("b"), b);
        }

        assert!(
            router
                .route(UNKNOWN, &Heartbeat::default(), Priority::Control)
                .is_err()
        );
    }

    #[test]
    fn a_full_queue_reports_congestion_not_a_dead_link() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (link, _control_rx, _bulk_rx) = link_with_receiver();
        lock(&router.inner.links).insert(LinkId::new("slow"), link);

        for _ in 0..BULK_QUEUE_CAPACITY {
            router
                .route(UNKNOWN, &Heartbeat::default(), Priority::Bulk)
                .expect("room in the bulk queue");
        }

        assert!(matches!(
            router.route(UNKNOWN, &Heartbeat::default(), Priority::Bulk),
            Err(MavError::LinkCongested)
        ));
        // A full bulk queue leaves control traffic unaffected.
        assert!(
            router
                .route(UNKNOWN, &Heartbeat::default(), Priority::Control)
                .is_ok()
        );
    }

    #[test]
    fn stale_remove_link_does_not_evict_newer_attachment() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let id = LinkId::new("link");

        let (newer, _control, _bulk) = link_with_receiver();
        lock(&router.inner.links).insert(
            id.clone(),
            Link {
                generation: 2,
                ..newer
            },
        );

        // A stale reader (generation 1) must not evict the generation-2 link.
        assert!(!router.remove_link(&id, 1));
        assert!(lock(&router.inner.links).contains_key(&id));

        // The matching generation removes it.
        assert!(router.remove_link(&id, 2));
        assert!(!lock(&router.inner.links).contains_key(&id));
    }

    #[test]
    fn duplicate_frames_across_links_reach_consumers_once() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);

        let (tx, mut waiter) = mpsc::unbounded_channel();
        router.insert_waiter(|_| true, tx);

        let link_a = LinkId::new("a");
        let link_b = LinkId::new("b");

        // The same packet heard on two links: delivered to consumers exactly
        // once.
        let first = router.handle_frame(&link_a, frame_from(SOURCE, 7, &Heartbeat::default()));
        let second = router.handle_frame(&link_b, frame_from(SOURCE, 7, &Heartbeat::default()));

        assert!(
            matches!(first, Some(Event::Message(_))),
            "the first copy is emitted"
        );
        assert!(second.is_none(), "the duplicate is not emitted");
        assert!(waiter.try_recv().is_ok(), "the first copy is delivered");
        assert!(
            waiter.try_recv().is_err(),
            "the duplicate must not reach consumers again"
        );

        // A different sequence is never a duplicate.
        assert!(
            router
                .handle_frame(&link_a, frame_from(SOURCE, 8, &Heartbeat::default()))
                .is_some()
        );

        // Window semantics: after another 192 distinct sequences the ring has
        // evicted the original one, so it may be delivered again.
        for seq in 9..=200u8 {
            router.handle_frame(&link_b, frame_from(SOURCE, seq, &Heartbeat::default()));
        }
        assert!(
            router
                .handle_frame(&link_a, frame_from(SOURCE, 7, &Heartbeat::default()))
                .is_some(),
            "an evicted sequence passes again"
        );
    }

    /// Autopilots number packets per channel: two links to one vehicle carry
    /// different packets with colliding sequence numbers, and none of them may
    /// be dropped as a duplicate.
    #[test]
    fn independent_channel_sequences_are_all_delivered() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (radio, lte) = (LinkId::new("radio"), LinkId::new("lte"));

        let mut delivered = 0;
        let total = 2000u32;
        for i in 0..total {
            let attitude = Attitude {
                time_boot_ms: i,
                ..Default::default()
            };
            let heartbeat = Heartbeat {
                custom_mode: i,
                ..Default::default()
            };
            delivered += usize::from(
                router
                    .handle_frame(&radio, frame_from(SOURCE, i as u8, &heartbeat))
                    .is_some(),
            );
            delivered += usize::from(
                router
                    .handle_frame(
                        &lte,
                        frame_from(SOURCE, (i as u8).wrapping_add(37), &attitude),
                    )
                    .is_some(),
            );
        }

        assert_eq!(delivered, 2 * total as usize);
    }

    /// With redundant links the duplicate of every packet lands on the slower
    /// link; the faster one must stay the preferred route.
    #[test]
    fn the_link_delivering_first_copies_is_the_preferred_route() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (fast, slow) = (LinkId::new("fast"), LinkId::new("slow"));

        for seq in 0..20u8 {
            router.handle_frame(&fast, frame_from(SOURCE, seq, &Heartbeat::default()));
            std::thread::sleep(Duration::from_millis(1));
            router.handle_frame(&slow, frame_from(SOURCE, seq, &Heartbeat::default()));
        }
        assert_eq!(router.best_link_for(SOURCE), Some(fast.clone()));

        // When the fast link goes quiet, the slow one's copies become first
        // copies, and it takes over.
        std::thread::sleep(Duration::from_millis(1));
        router.handle_frame(&slow, frame_from(SOURCE, 100, &Heartbeat::default()));
        assert_eq!(router.best_link_for(SOURCE), Some(slow));
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
    async fn link_closed_is_delivered_losslessly_and_guard_reports_it() {
        let (router, mut events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let id = LinkId::new("link");

        let (peer, gcs) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(gcs);
        let guard = router.attach_reader_writer(id.clone(), reader, writer, false);

        // The transport dies: EOF on every read.
        drop(peer);

        tokio::time::timeout(Duration::from_secs(2), guard.closed())
            .await
            .expect("the guard observes the teardown");

        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("an event arrives")
            .expect("the channel is open");
        assert!(matches!(event, Event::LinkClosed(closed) if closed == id));
    }

    #[tokio::test]
    async fn frames_carry_monotonic_sequences_across_priorities() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let id = LinkId::new("link");

        let (peer, gcs) = tokio::io::duplex(1 << 16);
        let (reader, writer) = tokio::io::split(gcs);
        let _guard = router.attach_reader_writer(id.clone(), reader, writer, false);

        for index in 0..20 {
            let priority = if index % 2 == 0 {
                Priority::Bulk
            } else {
                Priority::Control
            };
            router
                .route(UNKNOWN, &Heartbeat::default(), priority)
                .unwrap();
        }

        let (peer_reader, _peer_writer) = tokio::io::split(peer);
        let mut receiver = new_async_receiver(peer_reader);
        let mut previous: Option<u8> = None;
        for _ in 0..20 {
            let frame = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
                .unwrap();
            if let Some(previous) = previous {
                assert_eq!(frame.sequence(), previous.wrapping_add(1));
            }
            previous = Some(frame.sequence());
        }

        let stats = router.link_stats(&id).expect("attached");
        assert!(stats.tx_frames >= 20);
    }

    #[tokio::test]
    async fn frames_that_do_not_parse_are_counted() {
        use tokio::io::AsyncWriteExt;

        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let id = LinkId::new("link");

        let (mut peer, gcs) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(gcs);
        let _guard = router.attach_reader_writer(id.clone(), reader, writer, false);

        let frame = frame_from(SOURCE, 0, &Heartbeat::default());
        let mut bytes = [0u8; 300];
        let length = frame.serialize(&mut bytes).unwrap();
        let mut corrupt = bytes[..length].to_vec();
        // A payload byte changed in transit: the checksum no longer matches.
        corrupt[12] ^= 0xff;

        for chunk in [&bytes[..length], &corrupt, &bytes[..length]] {
            AsyncWriteExt::write_all(&mut peer, chunk).await.unwrap();
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let stats = router.link_stats(&id).expect("attached");
            if stats.rx_frames == 2 && stats.parse_errors == 1 {
                assert_eq!(stats.read_errors, 0);
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "{stats:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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
            let _ = router.send_on_link(&id, &Heartbeat::default());
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
        let (dead_tx, dead_rx) = mpsc::channel(1);
        drop(dead_rx);
        let _live_tx = {
            let mut links = lock(&router.inner.links);
            let link = links.get_mut(&id).expect("just attached");
            std::mem::replace(&mut link.control, dead_tx)
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
        let (fast, _fast_control, _fast_bulk) = link_with_receiver();
        let (slow, _slow_control, _slow_bulk) = link_with_receiver();
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
