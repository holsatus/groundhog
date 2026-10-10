//! MAVLink mission protocol transactions (clear, download, upload, set current).
//!
//! These implement the transfer sequences from <https://mavlink.io/en/services/mission.html>:
//! the retransmission rules (per-message timeouts and bounded retries) are the
//! core of the state machine, so a flaky link still completes a transfer.
//!
//! All operations are serialized per target and mission type through
//! [`LockKey::Mission`], since the responses carry no transaction id we could
//! otherwise tell apart.

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
use crate::foundation::MavlinkId;

/// How long to wait for a `MISSION_ACK` (or `MISSION_COUNT`) before retrying.
const MISSION_ACK_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long to wait for a requested mission item before re-requesting it.
const MISSION_ITEM_TIMEOUT: Duration = Duration::from_millis(250);
/// Retries after the first attempt.
const MISSION_MAX_RETRIES: u8 = 5;

// Message ids of the mission-transaction messages a vehicle may send us. We
// implement the modern `MISSION_REQUEST_INT` <-> `MISSION_ITEM_INT` transfer
// only; the legacy float items (`MISSION_ITEM`/`MISSION_REQUEST`) are deprecated
// by the specification and are not handled.
const MISSION_COUNT: u32 = 44;
const MISSION_ACK: u32 = 47;
const MISSION_REQUEST_INT: u32 = 51;
const MISSION_ITEM_INT: u32 = 73;

/// A mission plan downloaded from a vehicle.
pub struct DownloadedMission {
    pub items: Vec<MissionItemInt>,
    /// The `opaque_id` reported by the vehicle, or 0 if it uses none.
    pub vehicle_id: u32,
}

/// A mission message decoded from the wire, filtered to the mission transaction.
enum MissionMessage {
    Count(MissionCount),
    Item(MissionItemInt),
    RequestInt(MissionRequestInt),
    Ack(MissionAck),
}

struct MissionListener {
    receiver: FilteredReceiver,
}

impl MissionListener {
    /// Listen for mission messages from `target`. The filter is installed before
    /// returning, so a request sent afterwards cannot race its own reply.
    fn new(router: &MavlinkRouter, target: MavlinkId) -> Self {
        let receiver = router.receive_filtered(move |inbound| {
            matches_target(target, inbound.source_mav)
                && is_mission_message(inbound.frame.message_id())
        });

        MissionListener { receiver }
    }

    /// The next decoded mission message, waiting at most `dur`. `None` on timeout
    /// or a closed transport; undecodable mission frames are skipped without
    /// resetting the deadline.
    async fn next(&mut self, dur: Duration) -> Option<MissionMessage> {
        let deadline = Instant::now() + dur;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());

            let Ok(Some(inbound)) = timeout(remaining, self.receiver.next()).await else {
                return None;
            };

            match inbound.frame.decode::<DefaultDialect>() {
                Ok(DefaultDialect::MissionCount(message)) => {
                    return Some(MissionMessage::Count(message));
                }
                Ok(DefaultDialect::MissionItemInt(message)) => {
                    return Some(MissionMessage::Item(message));
                }
                Ok(DefaultDialect::MissionRequestInt(message)) => {
                    return Some(MissionMessage::RequestInt(message));
                }
                Ok(DefaultDialect::MissionAck(message)) => {
                    return Some(MissionMessage::Ack(message));
                }
                _ => continue,
            }
        }
    }
}

fn is_mission_message(id: u32) -> bool {
    matches!(
        id,
        MISSION_COUNT | MISSION_ACK | MISSION_REQUEST_INT | MISSION_ITEM_INT
    )
}

impl MavlinkRouter {
    /// Clear the mission of `target` and wait for the vehicle to accept.
    pub async fn mission_clear(
        &self,
        target: MavlinkId,
        mission_type: MavMissionType,
    ) -> Result<()> {
        let _lock = self.lock_key(mission_lock(target, mission_type)).await;
        let mut listener = MissionListener::new(self, target);

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

            let deadline = Instant::now() + MISSION_ACK_TIMEOUT;
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

    /// Download the full mission from `target`.
    pub async fn mission_download(
        &self,
        target: MavlinkId,
        mission_type: MavMissionType,
    ) -> Result<DownloadedMission> {
        let _lock = self.lock_key(mission_lock(target, mission_type)).await;
        let mut listener = MissionListener::new(self, target);

        let (count, vehicle_id) = self
            .request_mission_list(&mut listener, target, mission_type)
            .await?;

        let mut items = Vec::with_capacity(count as usize);
        for seq in 0..count {
            items.push(
                self.request_mission_item(&mut listener, target, mission_type, seq)
                    .await?,
            );
        }

        // Acknowledge the completed download, whether or not it was empty.
        self.send_targeted(
            target,
            MissionAck {
                type_: MavMissionResult::MavMissionAccepted,
                mission_type,
                ..Default::default()
            },
        )?;

        Ok(DownloadedMission { items, vehicle_id })
    }

    /// Upload `items` to `target`, returning the `opaque_id` it acknowledges with.
    pub async fn mission_upload(
        &self,
        target: MavlinkId,
        mission_type: MavMissionType,
        items: &[MissionItemInt],
    ) -> Result<u32> {
        let _lock = self.lock_key(mission_lock(target, mission_type)).await;
        let mut listener = MissionListener::new(self, target);
        let count = items.len() as u16;

        for attempt in 0..=MISSION_MAX_RETRIES {
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

            let mut deadline = Instant::now() + MISSION_ACK_TIMEOUT;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());

                match listener.next(remaining).await {
                    Some(MissionMessage::RequestInt(request))
                        if request.mission_type == mission_type && request.seq < count =>
                    {
                        self.send_targeted(target, items[request.seq as usize].clone())?;
                        // Serving an item proves progress: give the transfer more time.
                        deadline = Instant::now() + MISSION_ACK_TIMEOUT;
                    }
                    Some(MissionMessage::Ack(ack)) => {
                        return accept(ack.type_).map(|()| ack.opaque_id);
                    }
                    Some(_) => continue,
                    None => break,
                }
            }
        }

        Err(MavError::Timeout)
    }

    /// Ask the vehicle to jump to mission item `seq`. Fire-and-forget: the
    /// confirmation arrives later as a broadcast `MISSION_CURRENT`.
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

            let deadline = Instant::now() + MISSION_ACK_TIMEOUT;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());

                match listener.next(remaining).await {
                    Some(MissionMessage::Count(count)) if count.mission_type == mission_type => {
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

            let deadline = Instant::now() + MISSION_ITEM_TIMEOUT;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());

                match listener.next(remaining).await {
                    Some(MissionMessage::Item(item)) if item.seq == seq => return Ok(item),
                    // A different item was in flight: drop it and keep waiting
                    // for ours without extending the deadline.
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
