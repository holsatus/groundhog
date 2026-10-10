//! Tests of the middleware against a fake vehicle over an in-memory transport.

use std::time::Duration;

use mav_param::{Ident, Value};
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavCmd, MavProtocolCapability, MavResult},
        messages::{CommandAck, ParamValue, SystemTime},
    },
    prelude::V2,
    protocol::FrameBuilder,
};

use super::*;
use crate::{
    foundation::{GCS_MAVLINK_ID, LinkId},
    link::transport::{AsyncReceiver, AsyncSender, new_async_receiver, new_async_sender},
};

const VEHICLE: MavlinkId = MavlinkId {
    system: 1,
    component: 1,
};

struct MockVehicle {
    rx: AsyncReceiver,
    tx: AsyncSender,
    sequence: u8,
}

impl MockVehicle {
    /// Receive the next non-heartbeat message sent by the client.
    async fn recv(&mut self) -> DefaultDialect {
        loop {
            let frame = self.rx.recv().await.expect("transport closed");
            match frame.decode::<DefaultDialect>() {
                Ok(DefaultDialect::Heartbeat(_)) | Err(_) => continue,
                Ok(message) => return message,
            }
        }
    }

    async fn send<M: mavio::Message>(&mut self, message: &M) {
        let frame = FrameBuilder::new()
            .system_id(VEHICLE.system)
            .component_id(VEHICLE.component)
            .version(V2)
            .sequence(self.sequence)
            .message(message)
            .unwrap()
            .build()
            .into_versionless();
        self.sequence = self.sequence.wrapping_add(1);
        self.tx.send(&frame).await.unwrap();
    }

    async fn ack(&mut self, command: MavCmd, result: MavResult) {
        let gcs = GCS_MAVLINK_ID.load();
        self.send(&CommandAck {
            command,
            result,
            target_system: gcs.system,
            target_component: gcs.component,
            ..Default::default()
        })
        .await;
    }
}

fn setup() -> (MavlinkRouter, LinkGuard, MockVehicle) {
    let id = LinkId::new("dummy");
    let (gcs, vehicle) = tokio::io::duplex(4096);
    let (gcs_reader, gcs_writer) = tokio::io::split(gcs);
    let (vehicle_reader, vehicle_writer) = tokio::io::split(vehicle);

    let (client, _events) = MavlinkRouter::new();
    let guard = client.attach_reader_writer(id, gcs_reader, gcs_writer);

    let fake = MockVehicle {
        rx: new_async_receiver(vehicle_reader),
        tx: new_async_sender(vehicle_writer),
        sequence: 0,
    };

    (client, guard, fake)
}

#[tokio::test]
async fn command_is_resent_until_acked() {
    let (router, _guard, mut vehicle) = setup();

    let command = router
        .command(VEHICLE, cmd::request_message::<SystemTime>())
        .timeout(Duration::from_millis(100));
    let task = tokio::spawn(command.into_future());

    // Ignore the first attempt
    let DefaultDialect::CommandLong(first) = vehicle.recv().await else {
        panic!("expected COMMAND_LONG")
    };
    assert_eq!(first.confirmation, 0);
    assert_eq!(first.target_system, VEHICLE.system);

    let DefaultDialect::CommandLong(second) = vehicle.recv().await else {
        panic!("expected COMMAND_LONG")
    };
    assert_eq!(second.confirmation, 1);

    vehicle.ack(second.command, MavResult::Accepted).await;
    let ack = task.await.unwrap().expect("command should be accepted");
    assert_eq!(ack.result, MavResult::Accepted);
}

#[tokio::test]
async fn command_in_progress_then_rejected() {
    let (router, _guard, mut vehicle) = setup();

    let command = router
        .command(VEHICLE, cmd::request_message::<SystemTime>())
        .timeout(Duration::from_millis(100));
    let task = tokio::spawn(command.into_future());

    let DefaultDialect::CommandLong(long) = vehicle.recv().await else {
        panic!("expected COMMAND_LONG")
    };
    vehicle.ack(long.command, MavResult::InProgress).await;

    // Longer than the ACK timeout, but shorter than the progress timeout
    tokio::time::sleep(Duration::from_millis(300)).await;
    vehicle.ack(long.command, MavResult::Denied).await;

    let result = task.await.unwrap();
    assert!(matches!(result, Err(MavError::Rejected(MavResult::Denied))));
}

#[tokio::test]
async fn busy_while_command_in_flight() {
    let (router, _guard, mut vehicle) = setup();
    let key = LockKey::command(VEHICLE, MavCmd::RequestMessage);
    assert!(!router.is_busy(&key));

    let command = router.command(VEHICLE, cmd::request_message::<SystemTime>());
    let task = tokio::spawn(command.into_future());

    let DefaultDialect::CommandLong(long) = vehicle.recv().await else {
        panic!("expected COMMAND_LONG")
    };
    assert!(router.is_busy(&key));

    let second = router
        .command(VEHICLE, cmd::request_message::<SystemTime>())
        .on_busy(Busy::Fail)
        .await;
    assert!(matches!(second, Err(MavError::Busy(_))));

    vehicle.ack(long.command, MavResult::Accepted).await;
    task.await.unwrap().unwrap();
    assert!(!router.is_busy(&key));
}

#[tokio::test]
async fn request_message_returns_reply() {
    let (router, _guard, mut vehicle) = setup();
    let task = tokio::spawn({
        let router = router.clone();
        async move { router.request_message::<SystemTime>(VEHICLE).await }
    });

    let DefaultDialect::CommandLong(long) = vehicle.recv().await else {
        panic!("expected COMMAND_LONG")
    };
    vehicle.ack(long.command, MavResult::Accepted).await;
    vehicle
        .send(&SystemTime {
            time_unix_usec: 42,
            ..Default::default()
        })
        .await;

    let reply = task.await.unwrap().expect("reply");
    assert_eq!(reply.time_unix_usec, 42);
}

#[tokio::test]
async fn param_set_returns_echoed_value() {
    let (router, _guard, mut vehicle) = setup();
    let ident = Ident::try_from(b"TEST_PARAM").unwrap();
    let cap = MavProtocolCapability::default();

    let task = tokio::spawn({
        let router = router.clone();
        let ident = ident.clone();
        async move { router.param_set(VEHICLE, ident, Value::F32(1.5), cap).await }
    });

    let DefaultDialect::ParamSet(set) = vehicle.recv().await else {
        panic!("expected PARAM_SET")
    };
    assert_eq!(set.param_id, *ident.as_raw());

    // An unrelated parameter must not complete the transaction
    vehicle
        .send(&ParamValue {
            param_id: *Ident::try_from(b"OTHER").unwrap().as_raw(),
            param_value: 9.0,
            param_type: set.param_type,
            ..Default::default()
        })
        .await;

    // Echo back a clamped value
    vehicle
        .send(&ParamValue {
            param_id: set.param_id,
            param_value: 1.0,
            param_type: set.param_type,
            ..Default::default()
        })
        .await;

    let value = task.await.unwrap().expect("echo");
    assert_eq!(value, Value::F32(1.0));
}
