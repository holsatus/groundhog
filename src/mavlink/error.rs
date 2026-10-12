use mavio::default_dialect::enums::{MavMissionResult, MavResult};

use crate::foundation::MavlinkId;

#[derive(Debug, Clone, thiserror::Error)]
pub enum MavError {
    #[error("no link available to reach the target")]
    NoLink,
    #[error("timed out waiting for a response")]
    Timeout,
    #[error("command rejected with result {0:?}")]
    Rejected(MavResult),
    #[error("a transaction to {target:?} is already in flight")]
    Busy { target: MavlinkId },
    #[error("failed to encode message: {0}")]
    Encode(String),
    #[error("parameter value could not be encoded or decoded")]
    ParamEncoding,
    #[error("mission operation failed: {0:?}")]
    MissionFailed(MavMissionResult),
    #[error("a required resource was not available")]
    ResourceClosed,
}

pub type Result<T, E = MavError> = std::result::Result<T, E>;
