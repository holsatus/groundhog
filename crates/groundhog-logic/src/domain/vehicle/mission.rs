//! The vehicle's mission: what it holds onboard, how far it has come, and
//! the transfer Groundhog is running with it.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use mavio::default_dialect::{enums::MissionState as WireMissionState, messages};

use crate::{
    Progress,
    domain::{firmware::MissionLayout, mission::OnboardMission},
};

/// Mission execution progress, in wire sequence numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MissionProgress {
    /// The wire sequence the vehicle is executing.
    pub current_seq: Option<u16>,
    /// The number of mission items, home slot excluded (as the message
    /// defines it, and ArduPilot sends it). `None` while not reported.
    pub total: Option<u16>,
    pub state: WireMissionState,
    /// The vehicle's id of the mission it holds, when it reports one.
    pub mission_id: Option<u32>,
}

impl MissionProgress {
    pub(crate) fn observe_current(&mut self, msg: &messages::MissionCurrent) {
        self.current_seq = Some(msg.seq);
        // Zero means "not supported" and `UINT16_MAX` "no mission".
        self.total = (msg.total != 0 && msg.total != u16::MAX).then_some(msg.total);
        self.state = msg.mission_state;
        // Zero means "not supported" in this extension field.
        self.mission_id = (msg.mission_id != 0).then_some(msg.mission_id);
    }

    /// The plan index being executed, under the firmware's layout. `None`
    /// before any report and while the home slot is current.
    pub fn current_index(&self, layout: MissionLayout) -> Option<usize> {
        layout.plan_index(self.current_seq?)
    }
}

/// What a mission transfer does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Download,
    Upload,
    Clear,
}

/// A mission transfer Groundhog is running with the vehicle. Transfers to
/// one vehicle run one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionTransfer {
    pub kind: TransferKind,
    /// Items moved so far. `None` until the item count is known: a download
    /// before the vehicle has said how many it holds, and a clear throughout.
    pub progress: Option<Progress>,
    id: TransferId,
}

/// One mission transfer, so that a late report of a transfer that ended
/// cannot land on the one after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferId(u64);

/// The last transfer id handed out; ids only need to be unique within the
/// process.
static LAST_TRANSFER: AtomicU64 = AtomicU64::new(0);

impl TransferId {
    /// A transfer id no other transfer in this process has.
    pub(crate) fn next() -> Self {
        TransferId(LAST_TRANSFER.fetch_add(1, Ordering::Relaxed) + 1)
    }
}

/// What the operation running a mission transfer reports.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TransferUpdate {
    /// The transfer holds the vehicle's mission channel; nothing is sent yet.
    Start(TransferId, TransferKind),
    Progress(TransferId, Progress),
    /// The transfer is over, after its result (if any) was reported.
    End(TransferId),
}

/// The vehicle's mission state.
#[derive(Debug, Clone, Default)]
pub struct VehicleMission {
    /// The mission as of the last transfer, if one completed.
    pub onboard: Option<Arc<OnboardMission>>,
    pub progress: MissionProgress,
    /// The transfer running now, if any.
    pub transfer: Option<MissionTransfer>,
    /// Whether `progress` was reported after the last transfer ended and its
    /// result was recorded. Only such a report describes the mission
    /// `onboard` holds: one from before or during a transfer may name the
    /// mission that was replaced, or already the new one ahead of the
    /// transfer's result.
    reported_since_transfer: bool,
}

impl VehicleMission {
    /// Whether the vehicle now reports holding a different mission than the
    /// one last transferred (another GCS uploaded one, or it was changed
    /// onboard).
    ///
    /// Decided only from a `MISSION_CURRENT` that arrived after the last
    /// transfer, and never while one runs. A vehicle that reports mission
    /// ids is compared by id. One that does not is compared by item count,
    /// which catches only a change that adds or removes items.
    pub fn onboard_is_stale(&self) -> bool {
        let Some(onboard) = &self.onboard else {
            return false;
        };
        if !self.reported_since_transfer || self.transfer.is_some() {
            return false;
        }

        match (self.progress.mission_id, self.progress.total) {
            (Some(reported), _) => onboard.opaque_id != 0 && onboard.opaque_id != reported,
            (None, Some(total)) => usize::from(total) != onboard.items.len(),
            (None, None) => false,
        }
    }

    /// Fold in a `MISSION_CURRENT`.
    pub(crate) fn observe_current(&mut self, message: &messages::MissionCurrent) {
        self.progress.observe_current(message);
        if self.transfer.is_none() {
            self.reported_since_transfer = true;
        }
    }

    /// Record the mission the vehicle holds after a transfer.
    pub(crate) fn set_onboard(&mut self, mission: OnboardMission) {
        self.onboard = Some(Arc::new(mission));
        self.reported_since_transfer = false;
    }

    /// Apply what the operation running a transfer reports.
    pub(crate) fn update_transfer(&mut self, update: TransferUpdate) {
        match update {
            TransferUpdate::Start(id, kind) => {
                self.transfer = Some(MissionTransfer {
                    kind,
                    progress: None,
                    id,
                });
                self.reported_since_transfer = false;
            }
            TransferUpdate::Progress(id, progress) => {
                if let Some(transfer) = &mut self.transfer
                    && transfer.id == id
                {
                    transfer.progress = Some(progress);
                }
            }
            TransferUpdate::End(id) => {
                if self.transfer.is_some_and(|transfer| transfer.id == id) {
                    self.transfer = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mission::{MissionItem, Position};

    #[test]
    fn current_index_respects_the_home_slot() {
        let mut progress = MissionProgress::default();
        assert_eq!(progress.current_index(MissionLayout::default()), None);

        progress.observe_current(&messages::MissionCurrent {
            seq: 2,
            total: 8,
            mission_state: WireMissionState::Active,
            ..Default::default()
        });

        assert_eq!(
            progress.current_index(MissionLayout { home_slot: false }),
            Some(2)
        );
        assert_eq!(
            progress.current_index(MissionLayout { home_slot: true }),
            Some(1)
        );
        assert_eq!(progress.state, WireMissionState::Active);
    }

    #[test]
    fn a_different_reported_mission_id_marks_the_onboard_copy_stale() {
        let mut mission = VehicleMission {
            onboard: Some(Arc::new(OnboardMission {
                items: Vec::new(),
                home: None,
                opaque_id: 11,
            })),
            ..Default::default()
        };
        assert!(!mission.onboard_is_stale(), "no report yet");

        mission.observe_current(&current(11, 0));
        assert!(!mission.onboard_is_stale());

        mission.observe_current(&current(12, 0));
        assert!(mission.onboard_is_stale());
    }

    fn current(mission_id: u32, total: u16) -> messages::MissionCurrent {
        messages::MissionCurrent {
            total,
            mission_id,
            ..Default::default()
        }
    }

    fn onboard(items: usize, opaque_id: u32) -> OnboardMission {
        let waypoint = MissionItem::waypoint(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 50.0,
        });
        OnboardMission {
            items: vec![waypoint; items],
            home: None,
            opaque_id,
        }
    }

    /// A vehicle that announces the new id before the upload's result is
    /// recorded is not taken to hold another GCS's mission.
    #[test]
    fn reports_during_a_transfer_decide_nothing() {
        let mut mission = VehicleMission::default();
        mission.set_onboard(onboard(1, 10));
        mission.observe_current(&current(10, 1));

        let upload = TransferId::next();
        mission.update_transfer(TransferUpdate::Start(upload, TransferKind::Upload));
        mission.observe_current(&current(11, 2));
        assert!(!mission.onboard_is_stale(), "while the upload runs");

        mission.set_onboard(onboard(2, 11));
        mission.update_transfer(TransferUpdate::End(upload));
        assert!(!mission.onboard_is_stale(), "after it, before a new report");

        mission.observe_current(&current(11, 2));
        assert!(!mission.onboard_is_stale());
        mission.observe_current(&current(12, 2));
        assert!(mission.onboard_is_stale(), "a later change still counts");
    }

    /// An upload that failed on our side may still have reached the vehicle:
    /// the next report decides.
    #[test]
    fn a_failed_transfer_is_judged_by_the_next_report() {
        let mut mission = VehicleMission::default();
        mission.set_onboard(onboard(1, 10));

        let upload = TransferId::next();
        mission.update_transfer(TransferUpdate::Start(upload, TransferKind::Upload));
        mission.update_transfer(TransferUpdate::End(upload));
        assert!(!mission.onboard_is_stale());

        mission.observe_current(&current(11, 2));
        assert!(mission.onboard_is_stale());
    }

    /// Older firmware reports no mission id: a changed item count is all
    /// that shows.
    #[test]
    fn without_ids_a_changed_count_marks_the_onboard_copy_stale() {
        let mut mission = VehicleMission::default();
        mission.set_onboard(onboard(2, 0));

        mission.observe_current(&current(0, 2));
        assert!(!mission.onboard_is_stale(), "the same count");

        mission.observe_current(&current(0, 3));
        assert!(mission.onboard_is_stale(), "another count");
    }

    #[test]
    fn a_transfer_reports_its_progress_until_it_ends() {
        let mut mission = VehicleMission::default();
        let first = TransferId::next();
        let second = TransferId::next();

        mission.update_transfer(TransferUpdate::Start(first, TransferKind::Download));
        assert_eq!(
            mission.transfer.map(|transfer| transfer.progress),
            Some(None)
        );

        let progress = Progress { done: 1, total: 3 };
        mission.update_transfer(TransferUpdate::Progress(first, progress));
        assert_eq!(
            mission
                .transfer
                .map(|transfer| (transfer.kind, transfer.progress)),
            Some((TransferKind::Download, Some(progress)))
        );

        // The next transfer may start before the first one's end arrives;
        // that late end must not end the second.
        mission.update_transfer(TransferUpdate::Start(second, TransferKind::Upload));
        mission.update_transfer(TransferUpdate::Progress(
            first,
            Progress { done: 2, total: 3 },
        ));
        mission.update_transfer(TransferUpdate::End(first));
        assert_eq!(
            mission
                .transfer
                .map(|transfer| (transfer.kind, transfer.progress)),
            Some((TransferKind::Upload, None))
        );

        mission.update_transfer(TransferUpdate::End(second));
        assert_eq!(mission.transfer, None);
    }
}
