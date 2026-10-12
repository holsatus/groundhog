//! Vehicle mission execution progress.
//!
//! Progress is telemetry the vehicle reports (`MISSION_CURRENT`,
//! `MISSION_ITEM_REACHED`), so it lives with the vehicle rather than with the
//! editable plan in `domain::mission`.

use mavio::default_dialect::{enums::MissionState, messages};

/// Execution progress as reported by the vehicle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MissionProgress {
    pub current_seq: Option<u16>,
    pub total: Option<u16>,
    pub state: MissionState,
}

impl MissionProgress {
    pub fn observe_current(&mut self, msg: &messages::MissionCurrent) {
        self.current_seq = Some(msg.seq);
        self.total = Some(msg.total);
        self.state = msg.mission_state;
    }

    pub fn observe_reached(&mut self, seq: u16) {
        self.current_seq = Some(seq);
    }

    /// Whether the mission item at list `index` is the vehicle's current one.
    ///
    /// MAVLink identifies the current item by sequence, and the plan preserves
    /// list order as sequence order, so the list index is the sequence.
    pub fn is_current(&self, index: usize) -> bool {
        self.current_seq == Some(index as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_current_and_reached_update_progress() {
        let mut progress = MissionProgress::default();

        progress.observe_current(&messages::MissionCurrent {
            seq: 3,
            total: 10,
            mission_state: MissionState::Active,
            ..Default::default()
        });

        assert_eq!(progress.current_seq, Some(3));
        assert_eq!(progress.total, Some(10));
        assert_eq!(progress.state, MissionState::Active);

        progress.observe_reached(5);
        assert_eq!(progress.current_seq, Some(5));
        assert_eq!(progress.total, Some(10));
        assert_eq!(progress.state, MissionState::Active);

        progress.observe_current(&messages::MissionCurrent {
            seq: 0,
            total: 0,
            mission_state: MissionState::Unknown,
            ..Default::default()
        });
        assert_eq!(
            progress,
            MissionProgress {
                current_seq: Some(0),
                total: Some(0),
                state: MissionState::Unknown,
            }
        );
    }

    #[test]
    fn is_current_matches_the_sequence_to_the_index() {
        let mut progress = MissionProgress::default();
        assert!(
            !progress.is_current(0),
            "no report means nothing is current"
        );

        progress.observe_current(&messages::MissionCurrent {
            seq: 2,
            total: 10,
            mission_state: MissionState::Active,
            ..Default::default()
        });

        assert!(progress.is_current(2));
        assert!(!progress.is_current(1));
        assert!(!progress.is_current(3));
    }
}
