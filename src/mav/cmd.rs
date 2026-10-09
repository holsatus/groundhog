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
