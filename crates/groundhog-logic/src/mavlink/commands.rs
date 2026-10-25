//! Constructors for commonly used commands.

use mavio::{
    default_dialect::{enums::MavCmd, messages::CommandLong},
    mavspec::rust::spec::MessageSpecStatic,
};

use super::Command;

/// `MAV_CMD_REQUEST_MESSAGE` for the message with the given id.
pub(crate) fn request_message<M: MessageSpecStatic>() -> Command {
    request_message_id(M::message_id())
}

/// `MAV_CMD_REQUEST_MESSAGE` for the message with the given id.
pub(crate) fn request_message_id(message_id: u32) -> Command {
    CommandLong {
        command: MavCmd::RequestMessage,
        param1: message_id as f32,
        ..Default::default()
    }
    .into()
}

/// `MAV_CMD_MISSION_START` - start executing the stored mission.
pub(crate) fn mission_start() -> Command {
    CommandLong {
        command: MavCmd::MissionStart,
        ..Default::default()
    }
    .into()
}

/// `MAV_CMD_DO_PAUSE_CONTINUE` - pause or resume the running mission.
/// In the MAVLink spec param1 is 0 to pause and 1 to continue.
pub(crate) fn pause_continue(pause: bool) -> Command {
    CommandLong {
        command: MavCmd::DoPauseContinue,
        param1: if pause { 0.0 } else { 1.0 },
        ..Default::default()
    }
    .into()
}

/// `MAV_CMD_COMPONENT_ARM_DISARM`. `force` overrides the arming checks
/// (`param2 = 21196`), so it must only ever follow an explicit operator choice.
pub(crate) fn arm_disarm(arm: bool, force: bool) -> Command {
    CommandLong {
        command: MavCmd::ComponentArmDisarm,
        param1: if arm { 1.0 } else { 0.0 },
        param2: if force { 21196.0 } else { 0.0 },
        ..Default::default()
    }
    .into()
}

/// `MAV_CMD_DO_SET_MODE` with the given base mode, custom mode and sub mode.
pub(crate) fn set_mode(base_mode: f32, custom_mode: f32, custom_sub_mode: f32) -> Command {
    CommandLong {
        command: MavCmd::DoSetMode,
        param1: base_mode,
        param2: custom_mode,
        param3: custom_sub_mode,
        ..Default::default()
    }
    .into()
}

/// `MAV_CMD_DO_SET_MISSION_CURRENT` - continue the mission at wire sequence
/// `seq`. Acknowledged, unlike the legacy `MISSION_SET_CURRENT` message.
pub(crate) fn set_mission_current(seq: u16) -> Command {
    CommandLong {
        command: MavCmd::DoSetMissionCurrent,
        param1: seq as f32,
        ..Default::default()
    }
    .into()
}
