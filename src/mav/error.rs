use mavio::default_dialect::enums::{MavMissionResult, MavResult};

use super::LockKey;

#[derive(Debug, Clone, thiserror::Error)]
pub enum MavError {
    #[error("no link available to reach the target")]
    NoLink,
    #[error("timed out waiting for a response")]
    Timeout,
    #[error("command rejected with result {0:?}")]
    Rejected(MavResult),
    #[error("{0:?} is busy")]
    Busy(LockKey),
    #[error("failed to encode message: {0}")]
    Encode(String),
    #[error("parameter value could not be encoded or decoded")]
    ParamEncoding,
    #[allow(dead_code)] // Consumed by tests until the mission UI lands
    #[error("mission operation failed: {0:?}")]
    MissionFailed(MavMissionResult),
    #[error("a required resource was not available")]
    ResourceClosed,
}

pub type Result<T, E = MavError> = std::result::Result<T, E>;
