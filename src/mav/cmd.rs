//! Constructors for commonly used commands.

use mavio::{
    default_dialect::{enums::MavCmd, messages::CommandLong},
    mavspec::rust::spec::MessageSpecStatic,
};

use super::Command;

/// `MAV_CMD_REQUEST_MESSAGE` for the message with the given id.
pub fn request_message<M: MessageSpecStatic>() -> Command {
    request_message_id(M::message_id())
}

/// `MAV_CMD_REQUEST_MESSAGE` for the message with the given id.
pub fn request_message_id(message_id: u32) -> Command {
    CommandLong {
        command: MavCmd::RequestMessage,
        param1: message_id as f32,
        ..Default::default()
    }
    .into()
}

/// `MAV_CMD_MISSION_START` — start executing the stored mission.
pub fn mission_start() -> Command {
    CommandLong {
        command: MavCmd::MissionStart,
        ..Default::default()
    }
    .into()
}

/// `MAV_CMD_DO_PAUSE_CONTINUE` — pause or resume the running mission. In the
/// MAVLink spec param1 is 0 to pause and 1 to continue.
pub fn pause_continue(pause: bool) -> Command {
    CommandLong {
        command: MavCmd::DoPauseContinue,
        param1: if pause { 0.0 } else { 1.0 },
        ..Default::default()
    }
    .into()
}
