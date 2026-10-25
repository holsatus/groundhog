//! Decoding frames into the default dialect, tolerating enum values newer
//! than the bundled definitions.
//!
//! `mavio` generates the dialect from MAVLink definitions that lag current
//! firmware, and its decoder rejects a whole message when one enum field holds
//! a value it does not know. Dropping such a message is silent, and can hide a
//! vehicle (a newer `HEARTBEAT`) or break a transaction (a newer
//! `COMMAND_ACK` result). This is the one place frames are decoded: on an
//! unknown enum value it patches that field to a documented fallback and
//! decodes again. Messages and fields outside [`TOLERANT`] are still dropped.

use mavio::{
    DefaultDialect,
    default_dialect::enums::{MagCalStatus, MavAutopilot, MavMissionResult, MavResult, MavType},
    error::SpecError,
    mavspec::rust::spec::{Dialect, MessageSpecStatic},
    prelude::Versionless,
    protocol::Payload,
};

/// A frame as it comes off a link, before any dialect decodes it.
type Frame = mavio::Frame<Versionless>;

/// One enum field whose unknown values have a meaningful fallback.
struct Fallback {
    /// The Rust name the generated enum reports in
    /// [`SpecError::InvalidEnumValue`].
    enum_name: &'static str,
    /// The field's offset in the payload, which is fixed by the message
    /// definition.
    offset: usize,
    /// The value to decode in its place.
    value: u8,
}

/// A message with enum fields that have meaningful fallbacks.
struct TolerantMessage {
    message_id: u32,
    fields: &'static [Fallback],
}

/// Every message this build decodes tolerantly. Add a row (and a test) when a
/// message carries an enum a newer firmware may extend; everything else keeps
/// being dropped.
const TOLERANT: &[TolerantMessage] = &[
    // `custom_mode` (u32) is first, then the u8 fields in definition order.
    TolerantMessage {
        message_id: 0,
        fields: &[
            Fallback {
                enum_name: "MAV_TYPE",
                offset: 4,
                value: MavType::Generic as u8,
            },
            Fallback {
                enum_name: "MAV_AUTOPILOT",
                offset: 5,
                value: MavAutopilot::Generic as u8,
            },
        ],
    },
    // `target_system`, `target_component`, `type`, then the extensions.
    TolerantMessage {
        message_id: 47,
        fields: &[Fallback {
            enum_name: "MAV_MISSION_RESULT",
            offset: 2,
            value: MavMissionResult::MavMissionError as u8,
        }],
    },
    // `command` (u16) then `result`; progress and target fields are extensions.
    TolerantMessage {
        message_id: 77,
        fields: &[Fallback {
            enum_name: "MAV_RESULT",
            offset: 2,
            value: MavResult::Unsupported as u8,
        }],
    },
    // Ten f32 fields, then `compass_id`, `cal_mask`, `cal_status`.
    TolerantMessage {
        message_id: 192,
        fields: &[Fallback {
            enum_name: "MAG_CAL_STATUS",
            offset: 42,
            value: MagCalStatus::MagCalFailed as u8,
        }],
    },
];

/// Decode a frame into the default dialect. `None` for a message from another
/// dialect, a corrupt frame, or an unknown enum value without a fallback.
pub(crate) fn decode(frame: &Frame) -> Option<DefaultDialect> {
    match frame.decode::<DefaultDialect>() {
        Ok(message) => Some(message),
        Err(mavio::Error::Spec(SpecError::InvalidEnumValue { enum_name })) => {
            let payload = patched(frame, enum_name)?;
            let info = DefaultDialect::message_info(frame.message_id()).ok()?;
            frame
                .validate_checksum_with_crc_extra(info.crc_extra())
                .ok()?;
            DefaultDialect::decode(&payload).ok()
        }
        Err(mavio::Error::Spec(SpecError::NotInDialect(id))) => {
            log::trace!("mavlink message id {id} is not in the default dialect");
            None
        }
        Err(error) => {
            log::trace!("could not decode mavlink frame: {error}");
            None
        }
    }
}

/// Decode a frame into one known message type, with the same tolerance as
/// [`decode`]. The frame's checksum is validated before the message is
/// accepted, as for `Frame::decode_message`.
pub(crate) fn decode_message<M>(frame: &Frame) -> Option<M>
where
    M: MessageSpecStatic + for<'a> TryFrom<&'a Payload, Error = SpecError>,
{
    match frame.decode_message::<M>() {
        Ok(message) => Some(message),
        Err(mavio::Error::Spec(SpecError::InvalidEnumValue { enum_name })) => {
            let payload = patched(frame, enum_name)?;
            frame
                .validate_checksum_with_crc_extra(<M as MessageSpecStatic>::crc_extra())
                .ok()?;
            M::try_from(&payload).ok()
        }
        Err(_) => None,
    }
}

/// The frame's payload with the unknown enum field replaced by its fallback.
/// `None` when the message or the field has no entry in [`TOLERANT`], or when
/// MAVLink 2 truncation left the field out.
fn patched(frame: &Frame, enum_name: &str) -> Option<Payload> {
    let message = TOLERANT
        .iter()
        .find(|message| message.message_id == frame.message_id())?;
    let fallback = message
        .fields
        .iter()
        .find(|field| field.enum_name == enum_name)?;

    let payload = frame.payload();
    let mut bytes = payload.bytes().to_vec();
    let raw = *bytes.get(fallback.offset)?;

    log::debug!(
        "message {} has unknown {} {raw}; reading it as {}",
        frame.message_id(),
        enum_name,
        fallback.value
    );
    bytes[fallback.offset] = fallback.value;

    Some(Payload::new(payload.id(), &bytes, payload.version()))
}

#[cfg(test)]
mod tests {
    use mavio::{
        default_dialect::{
            enums::{MavCmd, MavMissionResult, MavResult, MavType},
            messages::{CommandAck, Heartbeat, MagCalReport, MissionAck},
        },
        mavspec::rust::spec::MessageSpecStatic,
        prelude::V2,
        protocol::{FrameBuilder, MavLinkVersion},
    };

    use super::*;

    /// Builds a frame from `message`, replacing payload byte `offset` with
    /// `value`. The checksum is computed over the patched payload, as a newer
    /// firmware would have sent it.
    fn tampered<M: mavio::Message + MessageSpecStatic>(
        message: &M,
        offset: usize,
        value: u8,
    ) -> Frame {
        frame(
            message,
            offset,
            value,
            <M as MessageSpecStatic>::crc_extra(),
        )
    }

    fn frame<M: mavio::Message + MessageSpecStatic>(
        message: &M,
        offset: usize,
        value: u8,
        crc_extra: u8,
    ) -> Frame {
        let payload = message.encode(MavLinkVersion::V2).expect("encodable");
        let mut bytes = payload.bytes().to_vec();
        if bytes.len() <= offset {
            bytes.resize(offset + 1, 0);
        }
        bytes[offset] = value;

        FrameBuilder::new()
            .version(V2)
            .sequence(0)
            .system_id(1)
            .component_id(1)
            .message_id(<M as MessageSpecStatic>::message_id())
            .payload(&bytes)
            .crc_extra(crc_extra)
            .build()
            .into_versionless()
    }

    #[test]
    fn a_heartbeat_with_a_newer_type_still_decodes() {
        let heartbeat = Heartbeat {
            type_: MavType::Quadrotor,
            autopilot: MavAutopilot::Ardupilotmega,
            ..Default::default()
        };

        let decoded = decode(&tampered(&heartbeat, 4, 200)).expect("patched");
        let DefaultDialect::Heartbeat(decoded) = decoded else {
            panic!("wrong message")
        };
        assert_eq!(decoded.type_, MavType::Generic);
        assert_eq!(
            decoded.autopilot,
            MavAutopilot::Ardupilotmega,
            "the other fields are untouched"
        );
    }

    #[test]
    fn a_heartbeat_with_a_newer_autopilot_still_decodes() {
        let heartbeat = Heartbeat {
            type_: MavType::Quadrotor,
            autopilot: MavAutopilot::Ardupilotmega,
            ..Default::default()
        };

        let decoded = decode(&tampered(&heartbeat, 5, 200)).expect("patched");
        let DefaultDialect::Heartbeat(decoded) = decoded else {
            panic!("wrong message")
        };
        assert_eq!(decoded.type_, MavType::Quadrotor);
        assert_eq!(decoded.autopilot, MavAutopilot::Generic);
    }

    #[test]
    fn a_command_ack_with_a_newer_result_reads_as_unsupported() {
        let ack = CommandAck {
            command: MavCmd::NavWaypoint,
            result: MavResult::Accepted,
            ..Default::default()
        };

        let decoded = decode(&tampered(&ack, 2, 200)).expect("patched");
        let DefaultDialect::CommandAck(decoded) = decoded else {
            panic!("wrong message")
        };
        assert_eq!(decoded.result, MavResult::Unsupported);
    }

    #[test]
    fn a_mission_ack_with_a_newer_result_reads_as_an_error() {
        let ack = MissionAck {
            type_: MavMissionResult::MavMissionAccepted,
            ..Default::default()
        };

        let decoded = decode(&tampered(&ack, 2, 200)).expect("patched");
        let DefaultDialect::MissionAck(decoded) = decoded else {
            panic!("wrong message")
        };
        assert_eq!(decoded.type_, MavMissionResult::MavMissionError);
    }

    #[test]
    fn a_mag_cal_report_with_a_newer_status_reads_as_failed() {
        let report = MagCalReport::default();

        let decoded = decode(&tampered(&report, 42, 200)).expect("patched");
        let DefaultDialect::MagCalReport(decoded) = decoded else {
            panic!("wrong message")
        };
        assert_eq!(decoded.cal_status, MagCalStatus::MagCalFailed);
    }

    #[test]
    fn an_unknown_enum_without_a_fallback_is_still_dropped() {
        let report = MagCalReport::default();
        // `old_orientation`, an extension enum with no fallback.
        assert!(decode(&tampered(&report, 48, 200)).is_none());
    }

    #[test]
    fn a_patched_frame_with_a_bad_checksum_is_refused() {
        let heartbeat = Heartbeat::default();
        let bad = frame(&heartbeat, 4, 200, Heartbeat::crc_extra().wrapping_add(1));

        assert!(decode(&bad).is_none());
    }

    #[test]
    fn decode_message_applies_the_same_fallback() {
        let ack = CommandAck {
            command: MavCmd::NavWaypoint,
            result: MavResult::Accepted,
            ..Default::default()
        };

        let decoded = decode_message::<CommandAck>(&tampered(&ack, 2, 200)).expect("patched");
        assert_eq!(decoded.command, MavCmd::NavWaypoint);
        assert_eq!(decoded.result, MavResult::Unsupported);
    }
}
