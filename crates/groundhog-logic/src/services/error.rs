//! The services layer's error type.
//!
//! Features match on these variants and show them to the operator without
//! naming any `mavio` or middleware type. The middleware's [`MavError`] is
//! mapped into this at the boundary, keeping what the operator needs to act
//! on: *why* a vehicle refused a command or a mission.

use std::fmt;

use mavio::default_dialect::enums::{MavMissionResult, MavResult};

use crate::mavlink::MavError;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("no link reaches the vehicle")]
    NoLink,
    #[error("the link is congested; try again")]
    LinkCongested,
    #[error("the vehicle did not respond")]
    Timeout,
    #[error("the vehicle refused: {0}")]
    Rejected(Rejection),
    #[error("the vehicle refused the mission: {0}")]
    MissionRejected(MissionRejection),
    #[error("the vehicle is not known")]
    UnknownVehicle,
    #[error("the vehicle's parameter encoding is not known yet; refusing to write")]
    ParamEncodingUnknown,
    #[error("the value cannot be represented as a MAVLink parameter")]
    ParamEncoding,
    #[error("the vehicle has no flight mode of that kind")]
    NoSuchMode,
    #[error("the vehicle's firmware does not support this calibration")]
    CalibrationUnsupported,
    #[error("no calibration is waiting for that")]
    NotCalibrating,
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("failed to encode message: {0}")]
    Encode(String),
    #[error("the connection to the router is closed")]
    Unavailable,
}

/// Why a vehicle refused a command (`MAV_RESULT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// Valid, but not possible right now (e.g. not armed, wrong mode).
    TemporarilyRejected,
    /// Invalid and will stay invalid (e.g. a parameter out of range).
    Denied,
    /// The vehicle does not support the command.
    Unsupported,
    /// Executing the command failed.
    Failed,
    /// The operation was cancelled.
    Cancelled,
    /// The command must be sent as the other message kind or frame.
    WrongEncoding,
    Unknown,
}

impl From<MavResult> for Rejection {
    fn from(result: MavResult) -> Self {
        match result {
            MavResult::TemporarilyRejected => Rejection::TemporarilyRejected,
            MavResult::Denied => Rejection::Denied,
            MavResult::Unsupported => Rejection::Unsupported,
            MavResult::Failed => Rejection::Failed,
            MavResult::Cancelled => Rejection::Cancelled,
            MavResult::CommandLongOnly
            | MavResult::CommandIntOnly
            | MavResult::CommandUnsupportedMavFrame => Rejection::WrongEncoding,
            MavResult::Accepted | MavResult::InProgress => Rejection::Unknown,
        }
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Rejection::TemporarilyRejected => "not possible right now",
            Rejection::Denied => "denied",
            Rejection::Unsupported => "not supported",
            Rejection::Failed => "it failed",
            Rejection::Cancelled => "cancelled",
            Rejection::WrongEncoding => "wrong command encoding",
            Rejection::Unknown => "unknown reason",
        })
    }
}

/// Why a vehicle refused a mission operation (`MAV_MISSION_RESULT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionRejection {
    Error,
    UnsupportedFrame,
    Unsupported,
    NoSpace,
    Invalid,
    /// An item parameter (1..=7) is out of range.
    InvalidParam(u8),
    InvalidSequence,
    Denied,
    Cancelled,
}

impl From<MavMissionResult> for MissionRejection {
    fn from(result: MavMissionResult) -> Self {
        use MavMissionResult as R;
        match result {
            R::MavMissionError | R::MavMissionAccepted => MissionRejection::Error,
            R::MavMissionUnsupportedFrame => MissionRejection::UnsupportedFrame,
            R::MavMissionUnsupported => MissionRejection::Unsupported,
            R::MavMissionNoSpace => MissionRejection::NoSpace,
            R::MavMissionInvalid => MissionRejection::Invalid,
            R::MavMissionInvalidParam1 => MissionRejection::InvalidParam(1),
            R::MavMissionInvalidParam2 => MissionRejection::InvalidParam(2),
            R::MavMissionInvalidParam3 => MissionRejection::InvalidParam(3),
            R::MavMissionInvalidParam4 => MissionRejection::InvalidParam(4),
            R::MavMissionInvalidParam5X => MissionRejection::InvalidParam(5),
            R::MavMissionInvalidParam6Y => MissionRejection::InvalidParam(6),
            R::MavMissionInvalidParam7 => MissionRejection::InvalidParam(7),
            R::MavMissionInvalidSequence => MissionRejection::InvalidSequence,
            R::MavMissionDenied => MissionRejection::Denied,
            R::MavMissionOperationCancelled => MissionRejection::Cancelled,
        }
    }
}

impl fmt::Display for MissionRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissionRejection::Error => f.write_str("generic error"),
            MissionRejection::UnsupportedFrame => f.write_str("an item uses an unsupported frame"),
            MissionRejection::Unsupported => f.write_str("an item uses an unsupported command"),
            MissionRejection::NoSpace => f.write_str("not enough space for the mission"),
            MissionRejection::Invalid => f.write_str("an item is invalid"),
            MissionRejection::InvalidParam(param) => {
                write!(f, "an item's parameter {param} is out of range")
            }
            MissionRejection::InvalidSequence => f.write_str("items arrived out of sequence"),
            MissionRejection::Denied => f.write_str("not accepted right now"),
            MissionRejection::Cancelled => f.write_str("the transfer was cancelled"),
        }
    }
}

/// The services layer's result type.
pub type Result<T> = std::result::Result<T, Error>;

impl From<MavError> for Error {
    fn from(error: MavError) -> Self {
        match error {
            MavError::NoLink => Error::NoLink,
            MavError::LinkCongested => Error::LinkCongested,
            MavError::Timeout => Error::Timeout,
            MavError::Rejected(result) => Error::Rejected(result.into()),
            MavError::Encode(message) => Error::Encode(message),
            MavError::ParamEncoding => Error::ParamEncoding,
            MavError::MissionFailed(result) => Error::MissionRejected(result.into()),
            MavError::MissionProtocol(message) => Error::Protocol(message),
            MavError::ResourceClosed => Error::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejections_keep_their_reason() {
        let error = Error::from(MavError::Rejected(MavResult::TemporarilyRejected));
        assert_eq!(error, Error::Rejected(Rejection::TemporarilyRejected));
        assert_eq!(
            error.to_string(),
            "the vehicle refused: not possible right now"
        );

        let error = Error::from(MavError::MissionFailed(MavMissionResult::MavMissionNoSpace));
        assert_eq!(
            error.to_string(),
            "the vehicle refused the mission: not enough space for the mission"
        );
    }
}
