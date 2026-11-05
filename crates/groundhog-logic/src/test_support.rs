//! Test doubles shared by the middleware and service tests.
//!
//! Both suites drive the real router over an in-memory transport from the
//! vehicle's side; keeping the fake vehicle in one place keeps its frame
//! handling (targets, sequence numbers, patched payloads) identical between
//! them.

use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavAutopilot, MavCmd, MavModeFlag, MavResult, MavType},
        messages::{CommandAck, CommandLong, Heartbeat},
    },
    prelude::{V2, Versionless},
    protocol::{FrameBuilder, IntoPayload, MavLinkVersion, MessageSpec},
};
use tokio::time::Duration;

use crate::{
    MavlinkId,
    mavlink::{
        DEFAULT_GCS_ID,
        io::{AsyncReceiver, AsyncSender},
    },
    wire::mission_item::RawMissionItem,
};

/// The vehicle end of an in-memory MAVLink link.
pub(crate) struct MockVehicle {
    pub(crate) rx: AsyncReceiver,
    pub(crate) tx: AsyncSender,
    pub(crate) sequence: u8,
    source: MavlinkId,
    /// Whether an undecodable frame from the GCS fails the test (the
    /// middleware suite checks that the client never sends one).
    strict: bool,
}

impl MockVehicle {
    /// A lenient vehicle: frames the bundled dialect cannot name are skipped,
    /// as another GCS's traffic would be.
    pub(crate) fn new(rx: AsyncReceiver, tx: AsyncSender, source: MavlinkId) -> Self {
        Self {
            rx,
            tx,
            sequence: 0,
            source,
            strict: false,
        }
    }

    /// A strict vehicle: any frame from the GCS it cannot decode fails the
    /// calling test.
    pub(crate) fn strict(rx: AsyncReceiver, tx: AsyncSender, source: MavlinkId) -> Self {
        Self {
            strict: true,
            ..Self::new(rx, tx, source)
        }
    }

    /// The next non-heartbeat message sent by the client.
    pub(crate) async fn recv(&mut self) -> DefaultDialect {
        loop {
            let frame = self.rx.recv().await.expect("transport closed");
            match frame.decode::<DefaultDialect>() {
                Ok(DefaultDialect::Heartbeat(_)) => continue,
                Ok(message) => return message,
                Err(error) if self.strict => panic!(
                    "the client sent message {} undecodable: {error}",
                    frame.message_id()
                ),
                Err(_) => continue,
            }
        }
    }

    /// The next frame from the GCS that is not a heartbeat, undecoded.
    pub(crate) async fn recv_frame(&mut self) -> mavio::Frame<Versionless> {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), self.rx.recv())
                .await
                .expect("the GCS sends something")
                .expect("transport open");
            if frame.message_id() != 0 {
                return frame;
            }
        }
    }

    /// The next command from the GCS.
    pub(crate) async fn command(&mut self) -> CommandLong {
        match self.recv().await {
            DefaultDialect::CommandLong(command) => command,
            other => panic!("expected a command, got {other:?}"),
        }
    }

    pub(crate) async fn send(&mut self, message: &dyn mavio::Message) {
        self.send_as(self.source.component, message).await;
    }

    /// Send `message` from another component of the vehicle.
    pub(crate) async fn send_as(&mut self, component: u8, message: &dyn mavio::Message) {
        let frame = FrameBuilder::new()
            .system_id(self.source.system)
            .component_id(component)
            .version(V2)
            .sequence(self.sequence)
            .message(message)
            .unwrap()
            .build()
            .into_versionless();
        self.sequence = self.sequence.wrapping_add(1);
        self.tx.send(&frame).await.unwrap();
    }

    /// Sends `message` with payload byte `offset` replaced, as a newer
    /// firmware whose enum value the bundled dialect does not know would.
    pub(crate) async fn send_patched<M: mavio::Message>(
        &mut self,
        message: &M,
        offset: usize,
        value: u8,
    ) {
        let payload = message.encode(MavLinkVersion::V2).expect("encodable");
        let mut bytes = payload.bytes().to_vec();
        if bytes.len() <= offset {
            bytes.resize(offset + 1, 0);
        }
        bytes[offset] = value;

        let frame = FrameBuilder::new()
            .system_id(self.source.system)
            .component_id(self.source.component)
            .version(V2)
            .sequence(self.sequence)
            .message_id(payload.id())
            .payload(&bytes)
            .crc_extra(message.crc_extra())
            .build()
            .into_versionless();
        self.sequence = self.sequence.wrapping_add(1);
        self.tx.send(&frame).await.unwrap();
    }

    /// Sends a mission item whose command and frame the bundled dialect does
    /// not know, built from the raw payload.
    pub(crate) async fn send_raw(&mut self, message: &RawMissionItem) {
        let payload = message.encode(MavLinkVersion::V2).expect("encodable");
        let frame = FrameBuilder::new()
            .system_id(self.source.system)
            .component_id(self.source.component)
            .version(V2)
            .sequence(self.sequence)
            .message_id(payload.id())
            .payload(payload.bytes())
            .crc_extra(message.crc_extra())
            .build()
            .into_versionless();
        self.sequence = self.sequence.wrapping_add(1);
        self.tx.send(&frame).await.unwrap();
    }

    pub(crate) async fn ack(&mut self, command: MavCmd, result: MavResult) {
        self.send(&CommandAck {
            command,
            result,
            target_system: DEFAULT_GCS_ID.system,
            target_component: DEFAULT_GCS_ID.component,
            ..Default::default()
        })
        .await;
    }

    pub(crate) async fn heartbeat(&mut self, autopilot: MavAutopilot) {
        self.heartbeat_of(autopilot, MavType::Quadrotor).await;
    }

    pub(crate) async fn heartbeat_of(&mut self, autopilot: MavAutopilot, type_: MavType) {
        self.send(&Heartbeat {
            type_,
            autopilot,
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
            ..Default::default()
        })
        .await;
    }
}
