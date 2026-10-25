//! Tests of the middleware against a fake vehicle over an in-memory transport.

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use mav_param::{Ident, Value};
use mavio::{
    DefaultDialect, Frame,
    default_dialect::{
        enums::{
            MavAutopilot, MavCmd, MavFrame, MavMissionResult, MavMissionType, MavModeFlag,
            MavResult, MavSeverity, MavState, MavType,
        },
        messages::{
            CommandAck, GlobalPositionInt, Heartbeat, MissionAck, MissionCount, MissionItemInt,
            MissionRequest, MissionRequestInt, MissionRequestList, ParamValue, Statustext,
            SysStatus, SystemTime,
        },
    },
    prelude::{V2, Versionless},
    protocol::{FrameBuilder, IntoPayload, MavLinkVersion, MessageSpec},
};
use tokio::io::{AsyncRead, ReadBuf};

use super::{error::MavError, locks::LockKey, *};
use crate::{
    SystemId,
    domain::fleet::Fleet,
    link::LinkId,
    mavlink::io::{AsyncReceiver, AsyncSender, new_async_receiver, new_async_sender},
    wire::mission_item::RawMissionItem,
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
    /// Receive the next non-heartbeat message sent by the client. A frame
    /// that does not decode fails the test: the client sent it corrupt.
    async fn recv(&mut self) -> DefaultDialect {
        loop {
            let frame = self.rx.recv().await.expect("transport closed");
            match frame.decode::<DefaultDialect>() {
                Ok(DefaultDialect::Heartbeat(_)) => continue,
                Ok(message) => return message,
                Err(error) => panic!(
                    "the client sent message {} undecodable: {error}",
                    frame.message_id()
                ),
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

    /// Sends `message` with payload byte `offset` replaced, as a newer
    /// firmware whose enum value the bundled dialect does not know would.
    async fn send_patched<M: mavio::Message>(&mut self, message: &M, offset: usize, value: u8) {
        let payload = message.encode(MavLinkVersion::V2).expect("encodable");
        let mut bytes = payload.bytes().to_vec();
        if bytes.len() <= offset {
            bytes.resize(offset + 1, 0);
        }
        bytes[offset] = value;

        let frame = FrameBuilder::new()
            .system_id(VEHICLE.system)
            .component_id(VEHICLE.component)
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
    async fn send_raw(&mut self, message: &RawMissionItem) {
        let payload = message.encode(MavLinkVersion::V2).expect("encodable");
        let frame = FrameBuilder::new()
            .system_id(VEHICLE.system)
            .component_id(VEHICLE.component)
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

    /// The next frame from the GCS that is not a heartbeat, undecoded.
    async fn recv_frame(&mut self) -> Frame<Versionless> {
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

    async fn ack(&mut self, command: MavCmd, result: MavResult) {
        self.send(&CommandAck {
            command,
            result,
            target_system: DEFAULT_GCS_ID.system,
            target_component: DEFAULT_GCS_ID.component,
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

    let (client, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
    let guard = client.attach_reader_writer(id, gcs_reader, gcs_writer, false);

    let fake = MockVehicle {
        rx: new_async_receiver(vehicle_reader),
        tx: new_async_sender(vehicle_writer),
        sequence: 0,
    };

    (client, guard, fake)
}

#[tokio::test]
async fn the_gcs_heartbeat_is_well_formed() {
    let (_router, _guard, mut vehicle) = setup();

    let frame = vehicle.rx.recv().await.expect("transport closed");
    let Ok(DefaultDialect::Heartbeat(heartbeat)) = frame.decode::<DefaultDialect>() else {
        panic!("expected HEARTBEAT")
    };
    assert_eq!(heartbeat.type_, MavType::Gcs);
    assert_eq!(heartbeat.autopilot, MavAutopilot::Invalid);
    assert_eq!(heartbeat.system_status, MavState::Active);
    assert_eq!(heartbeat.mavlink_version, 3);
}

#[tokio::test]
async fn command_is_resent_until_acked() {
    let (router, _guard, mut vehicle) = setup();

    let command = router
        .command(VEHICLE, commands::request_message::<SystemTime>())
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
        .command(VEHICLE, commands::request_message::<SystemTime>())
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

/// A result value newer than the bundled dialect's still reaches the
/// transaction, as the generic "not supported".
#[tokio::test]
async fn a_command_ack_with_a_newer_result_reaches_its_transaction() {
    let (router, _guard, mut vehicle) = setup();

    let command = router.command(VEHICLE, commands::request_message::<SystemTime>());
    let task = tokio::spawn(command.into_future());

    let DefaultDialect::CommandLong(long) = vehicle.recv().await else {
        panic!("expected COMMAND_LONG")
    };
    vehicle
        .send_patched(
            &CommandAck {
                command: long.command,
                result: MavResult::Accepted,
                target_system: DEFAULT_GCS_ID.system,
                target_component: DEFAULT_GCS_ID.component,
                ..Default::default()
            },
            2,
            200,
        )
        .await;

    let result = task.await.unwrap();
    assert!(matches!(
        result,
        Err(MavError::Rejected(MavResult::Unsupported))
    ));
}

#[tokio::test]
async fn a_command_in_flight_holds_its_lock() {
    let (router, _guard, mut vehicle) = setup();
    let key = LockKey::command(VEHICLE, MavCmd::RequestMessage);
    assert!(!router.is_busy(&key));

    let command = router.command(VEHICLE, commands::request_message::<SystemTime>());
    let task = tokio::spawn(command.into_future());

    let DefaultDialect::CommandLong(long) = vehicle.recv().await else {
        panic!("expected COMMAND_LONG")
    };
    assert!(router.is_busy(&key));

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

/// Two concurrent message requests of the same kind to one vehicle: they must
/// queue behind the command lock (each request owns its pending-ACK sender and
/// its reply filter for the whole operation), not starve the first arrival.
#[tokio::test]
async fn concurrent_request_messages_queue_instead_of_starving() {
    let (router, _guard, mut vehicle) = setup();

    let first = tokio::spawn({
        let router = router.clone();
        async move { router.request_message::<SystemTime>(VEHICLE).await }
    });
    let second = tokio::spawn({
        let router = router.clone();
        async move { router.request_message::<SystemTime>(VEHICLE).await }
    });

    // The mock answers each completed COMMAND_LONG with its ACK and the
    // message, in sequence: the replies arrive one round apart.
    for _ in 0..2 {
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
    }

    let first = first
        .await
        .unwrap()
        .expect("the queued first request must resolve");
    let second = second
        .await
        .unwrap()
        .expect("the queued second request must resolve");
    assert_eq!(first.time_unix_usec, 42);
    assert_eq!(second.time_unix_usec, 42);
    assert_eq!(first.time_unix_usec, second.time_unix_usec);
}

#[tokio::test]
async fn param_set_returns_echoed_value() {
    let (router, _guard, mut vehicle) = setup();
    let ident = Ident::try_from(b"TEST_PARAM").unwrap();
    let cap = crate::wire::param::ParamEncoding::Bytewise;

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

    // Echo back the value set, with the index a list would use too
    vehicle
        .send(&ParamValue {
            param_id: set.param_id,
            param_value: set.param_value,
            param_type: set.param_type,
            param_index: 3,
            param_count: 10,
        })
        .await;

    let value = task.await.unwrap().expect("echo");
    assert_eq!(value, Value::F32(1.5));
}

/// Entry `index` of a list of `count` parameters named `P<index>`.
fn list_value(index: u16, count: u16) -> ParamValue {
    let mut param_id = [0u8; 16];
    let name = format!("P{index}");
    param_id[..name.len()].copy_from_slice(name.as_bytes());
    ParamValue {
        param_id,
        param_value: f32::from(index),
        param_type: mavio::default_dialect::enums::MavParamType::Real32,
        param_index: index,
        param_count: count,
    }
}

/// Start downloading the parameter list.
fn start_list(
    router: &MavlinkRouter,
    capabilities: bool,
) -> tokio::task::JoinHandle<super::error::Result<(usize, u16)>> {
    let router = router.clone();
    tokio::spawn(async move { router.param_list(VEHICLE, capabilities).await })
}

#[tokio::test(start_paused = true)]
async fn param_list_completes() {
    let (router, _guard, mut vehicle) = setup();
    let task = start_list(&router, false);

    assert!(matches!(
        vehicle.recv().await,
        DefaultDialect::ParamRequestList(_)
    ));
    for index in 0..5 {
        vehicle.send(&list_value(index, 5)).await;
    }

    assert_eq!(task.await.unwrap().unwrap(), (5, 5));
    let more = tokio::time::timeout(Duration::from_secs(5), vehicle.recv()).await;
    assert!(more.is_err(), "nothing re-requested: {more:?}");
}

#[tokio::test(start_paused = true)]
async fn param_list_rerequests_the_missing_indices() {
    let (router, _guard, mut vehicle) = setup();
    let task = start_list(&router, false);

    vehicle.recv().await;
    for index in [0, 1, 3] {
        vehicle.send(&list_value(index, 4)).await;
    }

    let DefaultDialect::ParamRequestRead(read) = vehicle.recv().await else {
        panic!("expected PARAM_REQUEST_READ")
    };
    assert_eq!(read.param_index, 2);
    vehicle.send(&list_value(2, 4)).await;

    assert_eq!(task.await.unwrap().unwrap(), (4, 4));
}

#[tokio::test(start_paused = true)]
async fn param_list_ends_although_values_keep_coming() {
    let (router, _guard, mut vehicle) = setup();
    let started = tokio::time::Instant::now();
    let mut task = start_list(&router, false);

    vehicle.recv().await;
    vehicle.send(&list_value(0, 3)).await;
    vehicle.send(&list_value(1, 3)).await;

    // Index 0 again, every half round, for as long as the download lasts.
    let repeating = async {
        loop {
            tokio::select! {
                result = &mut task => break result.unwrap(),
                _ = tokio::time::sleep(param::PARAM_LIST_IDLE_TIMEOUT / 2) => {
                    vehicle.send(&list_value(0, 3)).await;
                }
            }
        }
    };
    let result = tokio::time::timeout(param::PARAM_LIST_IDLE_TIMEOUT * 60, repeating)
        .await
        .expect("the download ends");

    assert_eq!(result.unwrap(), (2, 3));
    let rounds = u32::from(param::PARAM_LIST_RETRIES) + 1;
    assert!(started.elapsed() <= param::PARAM_LIST_IDLE_TIMEOUT * (rounds + 1));
}

#[tokio::test(start_paused = true)]
async fn param_list_falls_back_to_component_information() {
    use mavio::default_dialect::messages::{AutopilotVersion, ComponentInformationBasic};

    let (router, _guard, mut vehicle) = setup();
    let task = start_list(&router, true);

    let requested = |message: DefaultDialect| match message {
        DefaultDialect::CommandLong(command) if command.command == MavCmd::RequestMessage => {
            command.param1 as u32
        }
        other => panic!("expected REQUEST_MESSAGE, got {other:?}"),
    };

    assert_eq!(
        requested(vehicle.recv().await),
        AutopilotVersion::message_id()
    );
    vehicle
        .ack(MavCmd::RequestMessage, MavResult::Unsupported)
        .await;

    assert_eq!(
        requested(vehicle.recv().await),
        ComponentInformationBasic::message_id()
    );
    vehicle
        .ack(MavCmd::RequestMessage, MavResult::Accepted)
        .await;
    vehicle.send(&ComponentInformationBasic::default()).await;

    assert!(matches!(
        vehicle.recv().await,
        DefaultDialect::ParamRequestList(_)
    ));
    vehicle.send(&list_value(0, 1)).await;
    assert_eq!(task.await.unwrap().unwrap(), (1, 1));
}

/// Start setting `TEST_PARAM` to `requested`, and receive the `PARAM_SET`.
async fn start_set(
    router: &MavlinkRouter,
    vehicle: &mut MockVehicle,
    requested: f32,
) -> (
    tokio::task::JoinHandle<super::error::Result<Value>>,
    mavio::default_dialect::messages::ParamSet,
) {
    let task = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .param_set(
                    VEHICLE,
                    Ident::try_from(b"TEST_PARAM").unwrap(),
                    Value::F32(requested),
                    crate::wire::param::ParamEncoding::Bytewise,
                )
                .await
        }
    });
    let DefaultDialect::ParamSet(set) = vehicle.recv().await else {
        panic!("expected PARAM_SET")
    };
    (task, set)
}

/// A `PARAM_VALUE` for the parameter of `set`, reporting `value`.
fn value_of(set: &mavio::default_dialect::messages::ParamSet, value: f32) -> ParamValue {
    let (param_value, param_type) =
        crate::wire::param::ParamEncoding::Bytewise.encode(Value::F32(value));
    assert_eq!(param_type, set.param_type);
    ParamValue {
        param_id: set.param_id,
        param_value,
        param_type,
        param_index: 7,
        param_count: 300,
    }
}

#[tokio::test]
async fn param_set_skips_a_list_value_for_the_same_parameter() {
    let (router, _guard, mut vehicle) = setup();
    let (task, set) = start_set(&router, &mut vehicle, 1.5).await;

    // A list download delivers the old value first, then the echo arrives.
    vehicle.send(&value_of(&set, 3.0)).await;
    vehicle.send(&value_of(&set, 1.5)).await;

    assert_eq!(task.await.unwrap().unwrap(), Value::F32(1.5));
}

/// A download holds the target's reply channel exclusively, so a set started
/// during it waits instead of taking one of the download's values.
#[tokio::test]
async fn param_set_waits_for_a_list_download() {
    let (router, _guard, mut vehicle) = setup();

    let download = tokio::spawn({
        let router = router.clone();
        async move { router.param_list(VEHICLE, false).await }
    });

    assert!(matches!(
        vehicle.recv().await,
        DefaultDialect::ParamRequestList(_)
    ));
    assert!(
        router.is_busy(&LockKey::ParamChannel(VEHICLE)),
        "the download holds the channel"
    );

    let set = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .param_set(
                    VEHICLE,
                    Ident::try_from(b"TEST_PARAM").unwrap(),
                    Value::F32(1.5),
                    crate::wire::param::ParamEncoding::Bytewise,
                )
                .await
        }
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(100), vehicle.recv())
            .await
            .is_err(),
        "the set sends nothing while the download runs"
    );

    vehicle.send(&list_value(0, 1)).await;
    assert_eq!(download.await.unwrap().unwrap(), (1, 1));

    let DefaultDialect::ParamSet(message) = vehicle.recv().await else {
        panic!("expected PARAM_SET")
    };
    vehicle.send(&value_of(&message, 1.5)).await;

    assert_eq!(set.await.unwrap().unwrap(), Value::F32(1.5));
}

/// Sets of different parameters share the channel and still overlap.
#[tokio::test]
async fn parameter_sets_of_different_parameters_overlap() {
    let (router, _guard, mut vehicle) = setup();

    let first = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .param_set(
                    VEHICLE,
                    Ident::try_from(b"FIRST").unwrap(),
                    Value::F32(1.5),
                    crate::wire::param::ParamEncoding::Bytewise,
                )
                .await
        }
    });
    let DefaultDialect::ParamSet(first_set) = vehicle.recv().await else {
        panic!("expected PARAM_SET")
    };

    let second = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .param_set(
                    VEHICLE,
                    Ident::try_from(b"SECOND").unwrap(),
                    Value::F32(2.5),
                    crate::wire::param::ParamEncoding::Bytewise,
                )
                .await
        }
    });
    let DefaultDialect::ParamSet(second_set) = vehicle.recv().await else {
        panic!("the second set is not held behind the first")
    };

    vehicle.send(&value_of(&first_set, 1.5)).await;
    vehicle.send(&value_of(&second_set, 2.5)).await;

    assert_eq!(first.await.unwrap().unwrap(), Value::F32(1.5));
    assert_eq!(second.await.unwrap().unwrap(), Value::F32(2.5));
}

#[tokio::test(start_paused = true)]
async fn param_set_reports_a_clamped_value_after_the_deadline() {
    let (router, _guard, mut vehicle) = setup();
    let (task, set) = start_set(&router, &mut vehicle, 1.5).await;

    vehicle.send(&value_of(&set, 1.0)).await;

    assert_eq!(task.await.unwrap().unwrap(), Value::F32(1.0));
    let resent = tokio::time::timeout(Duration::from_secs(5), vehicle.recv()).await;
    assert!(
        resent.is_err(),
        "a clamped set is not sent again: {resent:?}"
    );
}

#[tokio::test]
async fn param_set_ignores_a_late_echo_of_a_previous_write() {
    let (router, _guard, mut vehicle) = setup();
    let (first, set) = start_set(&router, &mut vehicle, 1.5).await;
    vehicle.send(&value_of(&set, 1.5)).await;
    assert_eq!(first.await.unwrap().unwrap(), Value::F32(1.5));

    // PX4 echoes again on every link; that copy lands in the next set.
    let (second, set) = start_set(&router, &mut vehicle, 2.5).await;
    vehicle.send(&value_of(&set, 1.5)).await;
    vehicle.send(&value_of(&set, 2.5)).await;

    assert_eq!(second.await.unwrap().unwrap(), Value::F32(2.5));
}

// ----- mission transfer harness -----

fn mission_item(seq: u16, x: i32) -> MissionItemInt {
    MissionItemInt {
        seq,
        x,
        y: 0,
        z: 0.0,
        frame: MavFrame::GlobalInt,
        command: MavCmd::NavWaypoint,
        mission_type: MavMissionType::Mission,
        ..Default::default()
    }
}

/// The same item in the raw form an upload takes.
fn wire_item(seq: u16, x: i32) -> RawMissionItem {
    RawMissionItem {
        seq,
        x,
        y: 0,
        z: 0.0,
        frame: MavFrame::GlobalInt as u8,
        command: MavCmd::NavWaypoint as u16,
        mission_type: MavMissionType::Mission as u8,
        ..Default::default()
    }
}

async fn recv_request_list(vehicle: &mut MockVehicle) -> MissionRequestList {
    let DefaultDialect::MissionRequestList(request) = vehicle.recv().await else {
        panic!("expected MISSION_REQUEST_LIST")
    };
    request
}

async fn recv_request_int(vehicle: &mut MockVehicle) -> u16 {
    let DefaultDialect::MissionRequestInt(request) = vehicle.recv().await else {
        panic!("expected MISSION_REQUEST_INT")
    };
    request.seq
}

async fn recv_ack(vehicle: &mut MockVehicle) -> MissionAck {
    let DefaultDialect::MissionAck(ack) = vehicle.recv().await else {
        panic!("expected MISSION_ACK")
    };
    ack
}

#[tokio::test]
async fn mission_download_transfers_all_items() {
    let (router, _guard, mut vehicle) = setup();

    let task = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .mission_download(VEHICLE, MavMissionType::Mission, &|_| {})
                .await
        }
    });

    let request = recv_request_list(&mut vehicle).await;
    assert_eq!(request.mission_type, MavMissionType::Mission);

    vehicle
        .send(&MissionCount {
            count: 3,
            mission_type: MavMissionType::Mission,
            opaque_id: 7,
            ..Default::default()
        })
        .await;

    for seq in 0..3u16 {
        assert_eq!(recv_request_int(&mut vehicle).await, seq);
        vehicle.send(&mission_item(seq, 100 + seq as i32)).await;
    }

    let ack = recv_ack(&mut vehicle).await;
    assert_eq!(ack.type_, MavMissionResult::MavMissionAccepted);

    let downloaded = task.await.unwrap().expect("download should succeed");
    assert_eq!(downloaded.opaque_id, 7);
    assert_eq!(downloaded.items.len(), 3);
    for (seq, item) in downloaded.items.iter().enumerate() {
        assert_eq!(item.seq, seq as u16);
        assert_eq!(item.x, 100 + seq as i32);
    }
}

#[tokio::test]
async fn mission_upload_sends_items_and_returns_id() {
    let (router, _guard, mut vehicle) = setup();
    let items = vec![wire_item(0, 11), wire_item(1, 22)];

    let task = tokio::spawn({
        let router = router.clone();
        let items = items.clone();
        async move {
            router
                .mission_upload(VEHICLE, MavMissionType::Mission, &items, &|_| {})
                .await
        }
    });

    let DefaultDialect::MissionCount(count) = vehicle.recv().await else {
        panic!("expected MISSION_COUNT")
    };
    assert_eq!(count.count, 2);

    for seq in 0..2u16 {
        vehicle
            .send(&MissionRequestInt {
                seq,
                mission_type: MavMissionType::Mission,
                ..Default::default()
            })
            .await;

        let DefaultDialect::MissionItemInt(item) = vehicle.recv().await else {
            panic!("expected MISSION_ITEM_INT")
        };
        assert_eq!(item.seq, seq);
        assert_eq!(item.x, items[seq as usize].x);
        assert_eq!(item.target_system, VEHICLE.system);
    }

    vehicle
        .send(&MissionAck {
            type_: MavMissionResult::MavMissionAccepted,
            mission_type: MavMissionType::Mission,
            opaque_id: 99,
            ..Default::default()
        })
        .await;

    assert_eq!(task.await.unwrap().expect("upload should succeed"), 99);
}

/// A vehicle may hold an item whose command or frame is newer than this
/// build's dialect. Downloading it must keep the whole item, and uploading it
/// again must send the same bytes.
#[tokio::test]
async fn mission_items_newer_than_the_dialect_round_trip() {
    let (router, _guard, mut vehicle) = setup();
    let served = RawMissionItem {
        seq: 0,
        frame: 255,
        command: 31_050,
        params: [1.0, 2.0, 3.0, 4.0],
        x: 11,
        y: 22,
        z: 33.0,
        mission_type: MavMissionType::Mission as u8,
        ..Default::default()
    };

    // Download: the vehicle serves the item, which the dialect cannot name.
    let download = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .mission_download(VEHICLE, MavMissionType::Mission, &|_| {})
                .await
        }
    });

    recv_request_list(&mut vehicle).await;
    vehicle
        .send(&MissionCount {
            count: 1,
            mission_type: MavMissionType::Mission,
            opaque_id: 4,
            ..Default::default()
        })
        .await;
    assert_eq!(recv_request_int(&mut vehicle).await, 0);
    vehicle.send_raw(&served).await;
    recv_ack(&mut vehicle).await;

    let downloaded = download.await.unwrap().expect("the item is tolerated");
    assert_eq!(downloaded.items, vec![served]);

    // Upload the same plan: the vehicle receives the item unchanged, apart
    // from the target fields the router fills in.
    let upload = tokio::spawn({
        let router = router.clone();
        let items = downloaded.items.clone();
        async move {
            router
                .mission_upload(VEHICLE, MavMissionType::Mission, &items, &|_| {})
                .await
        }
    });

    let DefaultDialect::MissionCount(count) = vehicle.recv().await else {
        panic!("expected MISSION_COUNT")
    };
    assert_eq!(count.count, 1);

    vehicle
        .send(&MissionRequestInt {
            seq: 0,
            mission_type: MavMissionType::Mission,
            ..Default::default()
        })
        .await;

    let expected = RawMissionItem {
        target_system: VEHICLE.system,
        target_component: VEHICLE.component,
        ..served
    };
    let frame = vehicle.recv_frame().await;
    assert_eq!(frame.message_id(), 73);
    assert_eq!(
        frame.payload().bytes(),
        expected
            .encode(MavLinkVersion::V2)
            .expect("encodable")
            .bytes(),
        "the unknown command and frame travel back unchanged"
    );

    vehicle
        .send(&MissionAck {
            type_: MavMissionResult::MavMissionAccepted,
            mission_type: MavMissionType::Mission,
            ..Default::default()
        })
        .await;
    assert!(upload.await.unwrap().is_ok());
}

#[tokio::test]
async fn mission_upload_answers_legacy_mission_request() {
    let (router, _guard, mut vehicle) = setup();
    let items = vec![wire_item(0, 11), wire_item(1, 22)];

    let task = tokio::spawn({
        let router = router.clone();
        let items = items.clone();
        async move {
            router
                .mission_upload(VEHICLE, MavMissionType::Mission, &items, &|_| {})
                .await
        }
    });

    let DefaultDialect::MissionCount(count) = vehicle.recv().await else {
        panic!("expected MISSION_COUNT")
    };
    assert_eq!(count.count, 2);

    // ArduPilot requests items with the deprecated MISSION_REQUEST (40).
    for seq in 0..2u16 {
        vehicle
            .send(&MissionRequest {
                seq,
                mission_type: MavMissionType::Mission,
                ..Default::default()
            })
            .await;

        let DefaultDialect::MissionItemInt(item) = vehicle.recv().await else {
            panic!("expected MISSION_ITEM_INT")
        };
        assert_eq!(item.seq, seq);
        assert_eq!(item.x, items[seq as usize].x);
    }

    vehicle
        .send(&MissionAck {
            type_: MavMissionResult::MavMissionAccepted,
            mission_type: MavMissionType::Mission,
            opaque_id: 42,
            ..Default::default()
        })
        .await;

    assert_eq!(task.await.unwrap().expect("upload should succeed"), 42);
}

#[tokio::test]
async fn mission_download_retransmits_a_dropped_item_request() {
    let (router, _guard, mut vehicle) = setup();

    let task = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .mission_download(VEHICLE, MavMissionType::Mission, &|_| {})
                .await
        }
    });

    recv_request_list(&mut vehicle).await;
    vehicle
        .send(&MissionCount {
            count: 1,
            mission_type: MavMissionType::Mission,
            opaque_id: 1,
            ..Default::default()
        })
        .await;

    // Drop the first request for item 0 without answering.
    assert_eq!(recv_request_int(&mut vehicle).await, 0);

    // The retransmission arrives after MISSION_ITEM_TIMEOUT and is answered.
    assert_eq!(recv_request_int(&mut vehicle).await, 0);
    vehicle.send(&mission_item(0, 5)).await;

    let ack = recv_ack(&mut vehicle).await;
    assert_eq!(ack.type_, MavMissionResult::MavMissionAccepted);

    let downloaded = task.await.unwrap().expect("download should recover");
    assert_eq!(downloaded.items.len(), 1);
    assert_eq!(downloaded.items[0].x, 5);
}

#[tokio::test]
async fn mission_clear_reports_nack() {
    let (router, _guard, mut vehicle) = setup();

    let task = tokio::spawn({
        let router = router.clone();
        async move { router.mission_clear(VEHICLE, MavMissionType::Mission).await }
    });

    let DefaultDialect::MissionClearAll(_) = vehicle.recv().await else {
        panic!("expected MISSION_CLEAR_ALL")
    };

    vehicle
        .send(&MissionAck {
            type_: MavMissionResult::MavMissionDenied,
            mission_type: MavMissionType::Mission,
            ..Default::default()
        })
        .await;

    let result = task.await.unwrap();
    assert!(matches!(
        result,
        Err(MavError::MissionFailed(MavMissionResult::MavMissionDenied))
    ));
}

/// Another GCS on the network runs its own transfer with the same vehicle:
/// replies addressed to it must not be taken as ours.
#[tokio::test]
async fn mission_download_ignores_traffic_for_another_gcs() {
    let (router, _guard, mut vehicle) = setup();

    let task = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .mission_download(VEHICLE, MavMissionType::Mission, &|_| {})
                .await
        }
    });

    recv_request_list(&mut vehicle).await;

    // A count for a different GCS (system 200) arrives first and is ignored.
    vehicle
        .send(&MissionCount {
            target_system: 200,
            target_component: 190,
            count: 9,
            mission_type: MavMissionType::Mission,
            ..Default::default()
        })
        .await;
    vehicle
        .send(&MissionCount {
            target_system: DEFAULT_GCS_ID.system,
            target_component: DEFAULT_GCS_ID.component,
            count: 1,
            mission_type: MavMissionType::Mission,
            opaque_id: 3,
        })
        .await;

    assert_eq!(recv_request_int(&mut vehicle).await, 0);

    // The other GCS's item (x = 999) is ignored; ours (x = 5) is taken.
    vehicle
        .send(&MissionItemInt {
            target_system: 200,
            ..mission_item(0, 999)
        })
        .await;
    vehicle.send(&mission_item(0, 5)).await;

    recv_ack(&mut vehicle).await;
    let downloaded = task.await.unwrap().expect("download succeeds");
    assert_eq!(downloaded.items.len(), 1);
    assert_eq!(downloaded.items[0].x, 5);
}

/// A fence transfer finishing alongside a mission transfer must not end the
/// mission transfer.
#[tokio::test]
async fn mission_upload_ignores_acks_for_another_mission_type() {
    let (router, _guard, mut vehicle) = setup();
    let items = vec![wire_item(0, 11)];

    let task = tokio::spawn({
        let router = router.clone();
        let items = items.clone();
        async move {
            router
                .mission_upload(VEHICLE, MavMissionType::Mission, &items, &|_| {})
                .await
        }
    });

    let DefaultDialect::MissionCount(_) = vehicle.recv().await else {
        panic!("expected MISSION_COUNT")
    };

    vehicle
        .send(&MissionAck {
            type_: MavMissionResult::MavMissionDenied,
            mission_type: MavMissionType::Fence,
            ..Default::default()
        })
        .await;

    vehicle
        .send(&MissionRequestInt {
            seq: 0,
            mission_type: MavMissionType::Mission,
            ..Default::default()
        })
        .await;
    let DefaultDialect::MissionItemInt(_) = vehicle.recv().await else {
        panic!("expected MISSION_ITEM_INT")
    };

    vehicle
        .send(&MissionAck {
            type_: MavMissionResult::MavMissionAccepted,
            mission_type: MavMissionType::Mission,
            opaque_id: 5,
            ..Default::default()
        })
        .await;

    assert_eq!(task.await.unwrap().expect("the fence ack is ignored"), 5);
}

/// Abandoning a download midway tells the vehicle, so its own transfer state
/// machine does not wait for a GCS that has left.
#[tokio::test]
async fn an_abandoned_download_is_cancelled_on_the_vehicle() {
    let (router, _guard, mut vehicle) = setup();

    let task = tokio::spawn({
        let router = router.clone();
        async move {
            router
                .mission_download(VEHICLE, MavMissionType::Mission, &|_| {})
                .await
        }
    });

    recv_request_list(&mut vehicle).await;
    vehicle
        .send(&MissionCount {
            count: 4,
            mission_type: MavMissionType::Mission,
            ..Default::default()
        })
        .await;
    assert_eq!(recv_request_int(&mut vehicle).await, 0);

    task.abort();
    drop(task.await);

    let ack = recv_ack(&mut vehicle).await;
    assert_eq!(ack.type_, MavMissionResult::MavMissionOperationCancelled);
}

#[tokio::test]
async fn download_reports_progress_per_item() {
    use std::sync::{Arc, Mutex};

    let (router, _guard, mut vehicle) = setup();
    let seen = Arc::new(Mutex::new(Vec::new()));

    let task = tokio::spawn({
        let router = router.clone();
        let seen = seen.clone();
        async move {
            router
                .mission_download(VEHICLE, MavMissionType::Mission, &move |progress| {
                    seen.lock().unwrap().push((progress.done, progress.total))
                })
                .await
        }
    });

    recv_request_list(&mut vehicle).await;
    vehicle
        .send(&MissionCount {
            count: 2,
            mission_type: MavMissionType::Mission,
            ..Default::default()
        })
        .await;
    for seq in 0..2 {
        assert_eq!(recv_request_int(&mut vehicle).await, seq);
        vehicle.send(&mission_item(seq, 1)).await;
    }
    recv_ack(&mut vehicle).await;
    task.await.unwrap().unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![(0, 2), (1, 2), (2, 2)]);
}

// ----- frame replay harness -----

/// Encode a message into a versionless v2 frame's bytes, as if sent by `vehicle`
/// with the given sequence number.
fn encode_frame(vehicle: MavlinkId, sequence: u8, message: &dyn mavio::Message) -> Vec<u8> {
    let frame = FrameBuilder::new()
        .system_id(vehicle.system)
        .component_id(vehicle.component)
        .version(V2)
        .sequence(sequence)
        .message(message)
        .unwrap()
        .build()
        .into_versionless();

    let mut buf = vec![0u8; frame.size()];
    let written = frame.serialize(&mut buf).unwrap();
    buf.truncate(written);
    buf
}

/// An in-memory reader over concatenated raw frames. Yields everything
/// eagerly, then EOFs.
struct BytesReader {
    bytes: Vec<u8>,
    pos: usize,
}

impl AsyncRead for BytesReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos >= self.bytes.len() {
            return Poll::Ready(Ok(())); // 0 bytes filled => EOF
        }

        let remaining = &self.bytes[self.pos..];
        let n = remaining.len().min(buf.remaining());
        buf.put_slice(&remaining[..n]);
        self.pos += n;
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn replay_frames_drive_fleet_state() {
    // A scripted flight: heartbeat, a position fix, battery, and a status text.
    let heartbeat = Heartbeat {
        type_: MavType::Quadrotor,
        autopilot: MavAutopilot::Ardupilotmega,
        base_mode: MavModeFlag::SAFETY_ARMED,
        custom_mode: 5,
        ..Default::default()
    };

    let position = GlobalPositionInt {
        time_boot_ms: 0,
        // Raw scaled ints: 474_000_000 -> 47.4 deg lat, 85_000_000 -> 8.5 deg lon.
        lat: 474_000_000,
        lon: 85_000_000,
        alt: 150_000,
        relative_alt: 100_000,
        vx: 0,
        vy: 0,
        vz: 0,
        hdg: 0,
    };

    let battery = SysStatus {
        voltage_battery: 12_000,
        current_battery: 500,
        battery_remaining: 80,
        ..Default::default()
    };

    let mut status_bytes = [0u8; 50];
    status_bytes[..9].copy_from_slice(b"REPLAY_OK");
    let status = Statustext {
        severity: MavSeverity::Info,
        text: status_bytes,
        ..Default::default()
    };

    let script: [Box<dyn mavio::Message>; 4] = [
        Box::new(heartbeat),
        Box::new(position),
        Box::new(battery),
        Box::new(status),
    ];

    let mut bytes = Vec::new();
    for (sequence, message) in script.iter().enumerate() {
        bytes.extend(encode_frame(VEHICLE, sequence as u8, message.as_ref()));
    }

    let (router, events) = MavlinkRouter::new(DEFAULT_GCS_ID);
    let mut events = events;
    let _guard = router.attach_reader_writer(
        LinkId::new("replay"),
        BytesReader { bytes, pos: 0 },
        tokio::io::sink(),
        false,
    );

    // Fold the replayed frames into a fleet, bounded so the test cannot hang.
    let mut fleet = Fleet::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut folded = 0;

    while folded < script.len() {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Event::Message(inbound))) => {
                let Ok(message) = inbound.frame.decode::<DefaultDialect>() else {
                    continue;
                };
                fleet.observe(inbound.source_mav, inbound.received_at, Some(&message));
                folded += 1;
            }
            Ok(Some(Event::LinkClosed(_))) => {}
            Ok(None) | Err(_) => break,
        }
    }

    assert_eq!(
        folded,
        script.len(),
        "all scripted frames should be observed"
    );

    let vehicle = fleet
        .vehicle(SystemId::from(VEHICLE.system))
        .expect("vehicle should be registered");
    assert_eq!(vehicle.identity.is_armed(), Some(true));

    let pos = vehicle
        .position
        .current
        .as_ref()
        .expect("position should be recorded")
        .pos;
    assert!((pos.lat - 47.4).abs() < 1e-7, "lat was {}", pos.lat);
    assert!((pos.lon - 8.5).abs() < 1e-7, "lon was {}", pos.lon);

    let battery = vehicle
        .battery
        .primary()
        .expect("battery should be recorded");
    let voltage = battery.voltage.expect("voltage is known");
    assert!((voltage - 12.0).abs() < 1e-6, "voltage was {voltage}");

    assert!(
        vehicle
            .status
            .texts
            .iter()
            .any(|text| text.text == "REPLAY_OK"),
        "the status text should be recorded"
    );
}
