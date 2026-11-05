//! Raw `MISSION_ITEM_INT` codec.
//!
//! The dialect's `MissionItemInt` decodes `command`, `frame` and
//! `mission_type` as enums, so a value newer than the bundled MAVLink
//! definitions fails to decode and the frame is dropped: a mission item the
//! vehicle holds would then be re-requested until the transfer times out.
//! Missions have to round-trip what a vehicle reports, so this codec keeps
//! those fields as raw wire numbers. Everything else is the same fixed payload
//! layout, including the MAVLink 2 `mission_type` extension and truncation.

use mavio::{
    error::SpecError,
    mavspec::rust::spec::MessageSpecStatic,
    protocol::{
        CrcExtra, IntoPayload, MavLinkVersion, MessageId, MessageInfo, MessageSpec, Payload,
    },
};

/// `MISSION_ITEM_INT`'s message id.
const MESSAGE_ID: MessageId = 73;

/// `MISSION_ITEM_INT`'s `CRC_EXTRA`, from `common.xml`.
const CRC_EXTRA: CrcExtra = 38;

/// The core payload, without the MAVLink 2 `mission_type` extension.
const V1_PAYLOAD_LEN: usize = 37;

/// One `MISSION_ITEM_INT`, with every field in its raw wire form.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RawMissionItem {
    pub target_system: u8,
    pub target_component: u8,
    pub seq: u16,
    /// `MAV_FRAME`, raw.
    pub frame: u8,
    /// `MAV_CMD`, raw.
    pub command: u16,
    pub current: u8,
    pub autocontinue: u8,
    pub params: [f32; 4],
    pub x: i32,
    pub y: i32,
    pub z: f32,
    /// `MAV_MISSION_TYPE`, raw.
    pub mission_type: u8,
}

/// Float fields compare by bit pattern, so a `NaN` parameter ("vehicle
/// default") equals itself and a decoded item equals the one it came from.
impl PartialEq for RawMissionItem {
    fn eq(&self, other: &Self) -> bool {
        self.target_system == other.target_system
            && self.target_component == other.target_component
            && self.seq == other.seq
            && self.frame == other.frame
            && self.command == other.command
            && self.current == other.current
            && self.autocontinue == other.autocontinue
            && self
                .params
                .iter()
                .zip(other.params)
                .all(|(a, b)| a.to_bits() == b.to_bits())
            && self.x == other.x
            && self.y == other.y
            && self.z.to_bits() == other.z.to_bits()
            && self.mission_type == other.mission_type
    }
}

impl Default for RawMissionItem {
    fn default() -> Self {
        Self {
            target_system: 0,
            target_component: 0,
            seq: 0,
            frame: 0,
            command: 0,
            current: 0,
            autocontinue: 0,
            params: [0.0; 4],
            x: 0,
            y: 0,
            z: 0.0,
            mission_type: 0,
        }
    }
}

impl RawMissionItem {
    /// The payload in MAVLink wire order: fields sorted by size, the
    /// `mission_type` extension last and present only in MAVLink 2.
    fn bytes(self, version: MavLinkVersion) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(V1_PAYLOAD_LEN + 1);

        for param in self.params {
            bytes.extend_from_slice(&param.to_le_bytes());
        }
        bytes.extend_from_slice(&self.x.to_le_bytes());
        bytes.extend_from_slice(&self.y.to_le_bytes());
        bytes.extend_from_slice(&self.z.to_le_bytes());
        bytes.extend_from_slice(&self.seq.to_le_bytes());
        bytes.extend_from_slice(&self.command.to_le_bytes());
        bytes.push(self.target_system);
        bytes.push(self.target_component);
        bytes.push(self.frame);
        bytes.push(self.current);
        bytes.push(self.autocontinue);

        if version == MavLinkVersion::V2 {
            bytes.push(self.mission_type);
        }

        bytes
    }
}

impl MessageSpecStatic for RawMissionItem {
    fn spec() -> MessageInfo {
        MessageInfo::new(MESSAGE_ID, CRC_EXTRA)
    }

    fn message_id() -> MessageId {
        MESSAGE_ID
    }

    fn crc_extra() -> CrcExtra {
        CRC_EXTRA
    }

    fn min_supported_mavlink_version() -> MavLinkVersion {
        MavLinkVersion::V1
    }
}

impl MessageSpec for RawMissionItem {
    fn id(&self) -> MessageId {
        MESSAGE_ID
    }

    fn min_supported_mavlink_version(&self) -> MavLinkVersion {
        MavLinkVersion::V1
    }

    fn crc_extra(&self) -> CrcExtra {
        CRC_EXTRA
    }
}

impl IntoPayload for RawMissionItem {
    fn encode(&self, version: MavLinkVersion) -> Result<Payload, SpecError> {
        Ok(Payload::new(MESSAGE_ID, &self.bytes(version), version))
    }
}

impl mavio::Message for RawMissionItem {}

impl TryFrom<&Payload> for RawMissionItem {
    type Error = SpecError;

    /// Decodes any payload length: MAVLink 2 truncates trailing zeros, and a
    /// truncated field reads as its zero value.
    fn try_from(payload: &Payload) -> Result<Self, SpecError> {
        if payload.id() != MESSAGE_ID {
            return Err(SpecError::NotInDialect(payload.id()));
        }

        let mut reader = Reader::new(payload.bytes());

        Ok(RawMissionItem {
            params: [reader.f32(), reader.f32(), reader.f32(), reader.f32()],
            x: reader.i32(),
            y: reader.i32(),
            z: reader.f32(),
            seq: reader.u16(),
            command: reader.u16(),
            target_system: reader.u8(),
            target_component: reader.u8(),
            frame: reader.u8(),
            current: reader.u8(),
            autocontinue: reader.u8(),
            mission_type: reader.u8(),
        })
    }
}

/// Reads little-endian wires, filling fields beyond the payload with zero.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, length: usize) -> &'a [u8] {
        let start = self.at.min(self.bytes.len());
        let end = (self.at + length).min(self.bytes.len()).max(start);
        self.at += length;
        &self.bytes[start..end]
    }

    fn u8(&mut self) -> u8 {
        self.take(1).first().copied().unwrap_or(0)
    }

    fn u16(&mut self) -> u16 {
        let bytes = self.take(2);
        u16::from_le_bytes([
            bytes.first().copied().unwrap_or(0),
            bytes.get(1).copied().unwrap_or(0),
        ])
    }

    fn i32(&mut self) -> i32 {
        let bytes = self.take(4);
        let mut wire = [0u8; 4];
        for (slot, byte) in wire.iter_mut().zip(bytes) {
            *slot = *byte;
        }
        i32::from_le_bytes(wire)
    }

    fn f32(&mut self) -> f32 {
        f32::from_bits(self.i32() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> RawMissionItem {
        RawMissionItem {
            target_system: 1,
            target_component: 1,
            seq: 4,
            frame: 255,
            command: 31_050,
            current: 0,
            autocontinue: 1,
            params: [1.5, f32::NAN, -0.0, 4.0],
            x: 473_000_000,
            y: -85_000_000,
            z: 120.5,
            mission_type: 0,
        }
    }

    #[test]
    fn unknown_commands_and_frames_round_trip_through_a_payload() {
        let original = item();
        let payload = original.encode(MavLinkVersion::V2).expect("encodable");
        let decoded = RawMissionItem::try_from(&payload).expect("decodable");

        assert_eq!(decoded, original);
        assert_eq!(decoded.command, 31_050, "outside the bundled dialect");
        assert_eq!(decoded.frame, 255);
    }

    #[test]
    fn a_truncated_payload_reads_missing_fields_as_zero() {
        let mut bytes = item().bytes(MavLinkVersion::V2);
        // Through the `frame` byte: the one-byte fields that follow are read
        // as their zero values.
        bytes.truncate(35);

        let decoded =
            RawMissionItem::try_from(&Payload::new(MESSAGE_ID, &bytes, MavLinkVersion::V2))
                .expect("decodable");

        assert_eq!(decoded.target_system, 1);
        assert_eq!(decoded.frame, 255);
        assert_eq!(decoded.current, 0);
        assert_eq!(decoded.autocontinue, 0);
        assert_eq!(decoded.mission_type, 0);
        assert_eq!(decoded.command, 31_050, "earlier fields are intact");
    }

    #[test]
    fn payloads_of_another_message_are_refused() {
        let payload = Payload::new(0, &[0; 8], MavLinkVersion::V2);
        assert!(RawMissionItem::try_from(&payload).is_err());
    }
}
