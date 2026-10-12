//! The services layer's own error type.
//!
//! Deliberately coarse: features match on these variants without naming any
//! `mavio` or middleware type. The middleware's [`MavError`] is mapped into this
//! at the boundary, so the details it carries (a rejected `MavResult`, a busy
//! target, a mission result) do not leak upward.

use crate::mavlink::MavError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("no link available to reach the target")]
    NoLink,
    #[error("timed out waiting for a response")]
    Timeout,
    #[error("command rejected")]
    Rejected,
    #[error("a transaction is already in flight")]
    Busy,
    #[error("failed to encode message: {0}")]
    Encode(String),
    #[error("parameter value could not be encoded or decoded")]
    ParamEncoding,
    #[error("mission operation failed")]
    MissionFailed,
    #[error("a required resource was not available")]
    ResourceClosed,
}

/// The services layer's result type.
pub type Result<T> = std::result::Result<T, Error>;

impl From<MavError> for Error {
    fn from(error: MavError) -> Self {
        match error {
            MavError::NoLink => Error::NoLink,
            MavError::Timeout => Error::Timeout,
            MavError::Rejected(_) => Error::Rejected,
            MavError::Busy { .. } => Error::Busy,
            MavError::Encode(message) => Error::Encode(message),
            MavError::ParamEncoding => Error::ParamEncoding,
            MavError::MissionFailed(_) => Error::MissionFailed,
            MavError::ResourceClosed => Error::ResourceClosed,
        }
    }
}
