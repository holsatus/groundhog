//! The vehicle's mission: what it holds onboard and how far it has come.

use std::sync::Arc;

use mavio::default_dialect::{enums::MissionState as WireMissionState, messages};

use crate::domain::{firmware::MissionLayout, mission::OnboardMission};

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
    /// The last wire sequence reported as reached.
    pub reached_seq: Option<u16>,
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

    pub(crate) fn observe_reached(&mut self, seq: u16) {
        self.reached_seq = Some(seq);
    }

    /// The plan index being executed, under the firmware's layout. `None`
    /// before any report and while the home slot is current.
    pub fn current_index(&self, layout: MissionLayout) -> Option<usize> {
        layout.plan_index(self.current_seq?)
    }
}

/// The vehicle's mission state.
#[derive(Debug, Clone, Default)]
pub struct VehicleMission {
    /// The mission as of the last transfer, if one completed.
    pub onboard: Option<Arc<OnboardMission>>,
    pub progress: MissionProgress,
}

impl VehicleMission {
    /// Whether the vehicle now reports holding a different mission than the
    /// one last transferred (another GCS uploaded one, or it was changed
    /// onboard). Only decidable for vehicles that report mission ids.
    pub fn onboard_is_stale(&self) -> bool {
        match (&self.onboard, self.progress.mission_id) {
            (Some(onboard), Some(reported)) => {
                onboard.opaque_id != 0 && onboard.opaque_id != reported
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        mission.progress.observe_current(&messages::MissionCurrent {
            mission_id: 11,
            ..Default::default()
        });
        assert!(!mission.onboard_is_stale());

        mission.progress.observe_current(&messages::MissionCurrent {
            mission_id: 12,
            ..Default::default()
        });
        assert!(mission.onboard_is_stale());
    }
}
