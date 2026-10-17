//! MAVLink mission protocol transactions (clear, download, upload, set current).
//!
//! These implement the transfer sequences from <https://mavlink.io/en/services/mission.html>:
//! the retransmission rules (per-message timeouts and bounded retries) are the
//! core of the state machine, so a flaky link still completes a transfer.
//!
//! All operations are serialized per target and mission type through
//! [`LockKey::Mission`], since the responses carry no transaction id we could
//! otherwise tell apart. For the same reason a transfer only listens to
//! messages that are addressed to this GCS and concern its own mission type:
//! another GCS on the network may be running a transfer with the same vehicle,
//! and a fence or rally transfer may run alongside a mission transfer.
//!
//! A transfer that is abandoned midway (its future dropped, or a retry budget
//! exhausted) tells the vehicle with `MAV_MISSION_OPERATION_CANCELLED`, so the
//! vehicle's own state machine does not wait for a GCS that has left.

use std::time::{Duration, Instant};

use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavMissionResult, MavMissionType},
        messages::{
            MissionAck, MissionClearAll, MissionCount, MissionItemInt, MissionRequestInt,
            MissionRequestList, MissionSetCurrent,
        },
    },
};
use tokio::time::timeout;

use super::{
    error::{MavError, Result},
    locks::LockKey,
    matches_target,
    recv::FilteredReceiver,
    router::MavlinkRouter,
};
use crate::MavlinkId;

/// Retries after the first attempt.
const MISSION_MAX_RETRIES: u8 = 5;

// Message ids of the mission-transaction messages a vehicle may send us. An
// upload may be driven by either request form: `MISSION_REQUEST` (40, the
// legacy request ArduPilot still sends) or `MISSION_REQUEST_INT` (51). We always
// answer with `MISSION_ITEM_INT` (73), as the spec directs.
const MISSION_REQUEST: u32 = 40;
const MISSION_COUNT: u32 = 44;
const MISSION_ACK: u32 = 47;
const MISSION_REQUEST_INT: u32 = 51;
const MISSION_ITEM_INT: u32 = 73;

/// A mission plan downloaded from a vehicle, in wire form.
#[derive(Debug, Clone)]
pub struct DownloadedMission {
    pub items: Vec<MissionItemInt>,
    /// The `opaque_id` reported by the vehicle, or 0 if it uses none.
    pub opaque_id: u32,
}

/// How far a transfer has come: `done` of `total` items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub done: u16,
    pub total: u16,
}

/// A mission message decoded from the wire, filtered to the mission transaction.
enum MissionMessage {
    Count(MissionCount),
    Item(MissionItemInt),
    /// An item request, normalised from either `MISSION_REQUEST` or
    /// `MISSION_REQUEST_INT`.
    Request {
        seq: u16,
    },
    Ack(MissionAck),
}

struct MissionListener {
    receiver: FilteredReceiver,
    gcs: MavlinkId,
    mission_type: MavMissionType,
}

impl MissionListener {
    /// Listen for mission messages from `target`. The filter is installed before
    /// returning, so a request sent afterwards cannot race its own reply.
    fn new(router: &MavlinkRouter, target: MavlinkId, mission_type: MavMissionType) -> Self {
        let receiver = router.receive_filtered(move |inbound| {
            matches_target(target, inbound.source_mav)
                && is_mission_message(inbound.frame.message_id())
        });

        MissionListener {
            receiver,
            gcs: router.gcs_id(),
            mission_type,
        }
    }

    /// Whether a message's target fields name this GCS. Zero is the
    /// broadcast address and is accepted.
    fn is_for_us(&self, target_system: u8, target_component: u8) -> bool {
        (target_system == 0 || target_system == self.gcs.system)
            && (target_component == 0 || target_component == self.gcs.component)
    }

    /// The next decoded mission message of this transfer, waiting at most
    /// `dur`. `None` on timeout or a closed transport. Messages for another GCS
    /// or another mission type are skipped without resetting the deadline.
    async fn next(&mut self, dur: Duration) -> Option<MissionMessage> {
        let deadline = Instant::now() + dur;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());

            let Ok(Some(inbound)) = timeout(remaining, self.receiver.next()).await else {
                return None;
            };

            let message = match inbound.frame.decode::<DefaultDialect>() {
                Ok(DefaultDialect::MissionCount(m))
                    if self.is_for_us(m.target_system, m.target_component)
                        && m.mission_type == self.mission_type =>
                {
                    MissionMessage::Count(m)
                }
                Ok(DefaultDialect::MissionItemInt(m))
                    if self.is_for_us(m.target_system, m.target_component)
                        && m.mission_type == self.mission_type =>
                {
                    MissionMessage::Item(m)
                }
                Ok(DefaultDialect::MissionRequestInt(m))
                    if self.is_for_us(m.target_system, m.target_component)
                        && m.mission_type == self.mission_type =>
                {
                    MissionMessage::Request { seq: m.seq }
                }
                Ok(DefaultDialect::MissionRequest(m))
                    if self.is_for_us(m.target_system, m.target_component)
                        && m.mission_type == self.mission_type =>
                {
                    MissionMessage::Request { seq: m.seq }
                }
                Ok(DefaultDialect::MissionAck(m))
                    if self.is_for_us(m.target_system, m.target_component)
                        && m.mission_type == self.mission_type =>
                {
                    MissionMessage::Ack(m)
                }
                _ => continue,
            };

            return Some(message);
        }
    }
}

fn is_mission_message(id: u32) -> bool {
    matches!(
        id,
        MISSION_REQUEST | MISSION_COUNT | MISSION_ACK | MISSION_REQUEST_INT | MISSION_ITEM_INT
    )
}

/// Tells the vehicle a transfer was abandoned, unless disarmed first.
///
/// Armed once the vehicle has entered its transfer state (it answered a count
/// or asked for items), disarmed when the transfer ends normally or the
/// vehicle itself ended it with an ACK.
struct CancelOnDrop<'a> {
    router: &'a MavlinkRouter,
    target: MavlinkId,
    mission_type: MavMissionType,
    armed: bool,
}

impl<'a> CancelOnDrop<'a> {
    fn new(router: &'a MavlinkRouter, target: MavlinkId, mission_type: MavMissionType) -> Self {
        Self {
            router,
            target,
            mission_type,
            armed: false,
        }
    }
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            log::debug!("Cancelling mission transfer with {:?}", self.target);
            let _ = self.router.send_targeted(
                self.target,
                MissionAck {
                    type_: MavMissionResult::MavMissionOperationCancelled,
                    mission_type: self.mission_type,
                    ..Default::default()
                },
            );
        }
    }
}

impl MavlinkRouter {
    /// Clear the mission of `target` and wait for the vehicle to accept.
    pub async fn mission_clear(
        &self,
        target: MavlinkId,
        mission_type: MavMissionType,
    ) -> Result<()> {
        let _lock = self.lock_key(mission_lock(target, mission_type)).await;
        let mut listener = MissionListener::new(self, target, mission_type);
        let (_, ack_timeout) = self.transfer_timeouts(target);

        for attempt in 0..=MISSION_MAX_RETRIES {
            if attempt > 0 {
                log::debug!("Resending MISSION_CLEAR_ALL to {target:?} ({attempt})");
            }

            self.send_targeted(
                target,
                MissionClearAll {
                    mission_type,
                    ..Default::default()
                },
            )?;

            let deadline = Instant::now() + ack_timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());

                match listener.next(remaining).await {
                    Some(MissionMessage::Ack(ack)) => return accept(ack.type_),
                    // Ignore anything else without extending the deadline.
                    Some(_) => continue,
                    None => break,
                }
            }
        }

        Err(MavError::Timeout)
    }

    /// Download the full mission from `target`, reporting progress per item.
    pub async fn mission_download(
        &self,
        target: MavlinkId,
        mission_type: MavMissionType,
        progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<DownloadedMission> {
        let _lock = self.lock_key(mission_lock(target, mission_type)).await;
        let mut listener = MissionListener::new(self, target, mission_type);
        let (item_timeout, ack_timeout) = self.transfer_timeouts(target);
        let mut cancel = CancelOnDrop::new(self, target, mission_type);

        let (count, opaque_id) = self
            .request_mission_list(&mut listener, target, mission_type, ack_timeout)
            .await?;

        // The vehicle now waits for item requests: abandoning from here on
        // must be announced.
        cancel.armed = count > 0;
        progress(TransferProgress {
            done: 0,
            total: count,
        });

        let mut items = Vec::with_capacity(count as usize);
        for seq in 0..count {
            let item = self
                .request_mission_item(&mut listener, target, mission_type, seq, item_timeout)
                .await;

            match item {
                Ok(item) => items.push(item),
                Err(error @ MavError::MissionFailed(_)) => {
                    // The vehicle ended the transfer itself.
                    cancel.armed = false;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }

            progress(TransferProgress {
                done: seq + 1,
                total: count,
            });
        }

        // Acknowledge the completed download, whether or not it was empty.
        cancel.armed = false;
        self.send_targeted(
            target,
            MissionAck {
                type_: MavMissionResult::MavMissionAccepted,
                mission_type,
                ..Default::default()
            },
        )?;

        Ok(DownloadedMission { items, opaque_id })
    }

    /// Upload `items` to `target`, returning the `opaque_id` it acknowledges
    /// with. Progress counts the distinct items the vehicle has requested.
    pub async fn mission_upload(
        &self,
        target: MavlinkId,
        mission_type: MavMissionType,
        items: &[MissionItemInt],
        progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<u32> {
        let count = u16::try_from(items.len())
            .map_err(|_| MavError::Encode(format!("{} mission items", items.len())))?;

        let _lock = self.lock_key(mission_lock(target, mission_type)).await;
        let mut listener = MissionListener::new(self, target, mission_type);
        let (_, ack_timeout) = self.transfer_timeouts(target);
        let mut cancel = CancelOnDrop::new(self, target, mission_type);
        let mut highest_served: Option<u16> = None;

        progress(TransferProgress {
            done: 0,
            total: count,
        });

        for attempt in 0..=MISSION_MAX_RETRIES {
            // Once the vehicle has started requesting items, it drives the
            // transfer: a stall is answered by waiting for its re-request, not
            // by restarting the transfer with a new count.
            if highest_served.is_none() {
                if attempt > 0 {
                    log::debug!("Resending MISSION_COUNT to {target:?} ({attempt})");
                }

                self.send_targeted(
                    target,
                    MissionCount {
                        count,
                        mission_type,
                        ..Default::default()
                    },
                )?;
            } else {
                log::debug!("Waiting for {target:?} to re-request mission items ({attempt})");
            }

            let mut deadline = Instant::now() + ack_timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());

                match listener.next(remaining).await {
                    Some(MissionMessage::Request { seq }) if seq < count => {
                        cancel.armed = true;
                        self.send_targeted(target, items[seq as usize].clone())?;

                        if highest_served.is_none_or(|highest| seq > highest) {
                            highest_served = Some(seq);
                            progress(TransferProgress {
                                done: seq + 1,
                                total: count,
                            });
                        }

                        // Serving an item proves progress: give the transfer more time.
                        deadline = Instant::now() + ack_timeout;
                    }
                    Some(MissionMessage::Request { seq }) => {
                        return Err(MavError::MissionProtocol(format!(
                            "vehicle requested item {seq} of a {count}-item mission"
                        )));
                    }
                    Some(MissionMessage::Ack(ack)) => {
                        cancel.armed = false;
                        return accept(ack.type_).map(|()| ack.opaque_id);
                    }
                    Some(_) => continue,
                    None => break,
                }
            }
        }

        Err(MavError::Timeout)
    }

    /// Ask the vehicle to jump to mission item `seq` with the legacy
    /// `MISSION_SET_CURRENT` message. Fire-and-forget: the confirmation arrives
    /// later as a broadcast `MISSION_CURRENT`. Prefer the acknowledged
    /// `MAV_CMD_DO_SET_MISSION_CURRENT` where the vehicle supports it.
    pub async fn mission_set_current(&self, target: MavlinkId, seq: u16) -> Result<()> {
        self.send_targeted(
            target,
            MissionSetCurrent {
                seq,
                ..Default::default()
            },
        )
    }

    /// Ask for the mission count, retrying until the vehicle answers.
    async fn request_mission_list(
        &self,
        listener: &mut MissionListener,
        target: MavlinkId,
        mission_type: MavMissionType,
        ack_timeout: Duration,
    ) -> Result<(u16, u32)> {
        for attempt in 0..=MISSION_MAX_RETRIES {
            if attempt > 0 {
                log::debug!("Resending MISSION_REQUEST_LIST to {target:?} ({attempt})");
            }

            self.send_targeted(
                target,
                MissionRequestList {
                    mission_type,
                    ..Default::default()
                },
            )?;

            let deadline = Instant::now() + ack_timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());

                match listener.next(remaining).await {
                    Some(MissionMessage::Count(count)) => {
                        return Ok((count.count, count.opaque_id));
                    }
                    Some(MissionMessage::Ack(ack)) => {
                        return Err(MavError::MissionFailed(ack.type_));
                    }
                    Some(_) => continue,
                    None => break,
                }
            }
        }

        Err(MavError::Timeout)
    }

    /// Request one mission item, retrying until it arrives.
    async fn request_mission_item(
        &self,
        listener: &mut MissionListener,
        target: MavlinkId,
        mission_type: MavMissionType,
        seq: u16,
        item_timeout: Duration,
    ) -> Result<MissionItemInt> {
        for attempt in 0..=MISSION_MAX_RETRIES {
            if attempt > 0 {
                log::debug!("Re-requesting mission item {seq} from {target:?} ({attempt})");
            }

            self.send_targeted(
                target,
                MissionRequestInt {
                    seq,
                    mission_type,
                    ..Default::default()
                },
            )?;

            let deadline = Instant::now() + item_timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());

                match listener.next(remaining).await {
                    Some(MissionMessage::Item(item)) if item.seq == seq => return Ok(item),
                    Some(MissionMessage::Ack(ack)) => {
                        return Err(MavError::MissionFailed(ack.type_));
                    }
                    // A different item was in flight: drop it and keep waiting
                    // for ours without extending the deadline.
                    Some(_) => continue,
                    None => break,
                }
            }
        }

        Err(MavError::Timeout)
    }
}

fn mission_lock(target: MavlinkId, mission_type: MavMissionType) -> LockKey {
    LockKey::Mission(target, mission_type as u8)
}

/// Map a `MISSION_ACK` result to `Ok(())` or a [`MavError::MissionFailed`].
fn accept(result: MavMissionResult) -> Result<()> {
    if result == MavMissionResult::MavMissionAccepted {
        Ok(())
    } else {
        Err(MavError::MissionFailed(result))
    }
}
