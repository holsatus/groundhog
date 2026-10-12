//! Tests of the middleware against a fake vehicle over an in-memory transport.

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures::StreamExt;
use mav_param::{Ident, Value};
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{
            MavAutopilot, MavCmd, MavFrame, MavMissionResult, MavMissionType, MavModeFlag,
            MavProtocolCapability, MavResult, MavSeverity, MavType,
        },
        messages::{
            CommandAck, GlobalPositionInt, Heartbeat, MissionAck, MissionCount, MissionItemInt,
            MissionRequest, MissionRequestInt, MissionRequestList, ParamValue, Statustext,
            SysStatus, SystemTime,
        },
    },
    prelude::V2,
    protocol::FrameBuilder,
};
use tokio::io::{AsyncRead, ReadBuf};

use super::*;
use super::{error::MavError, transaction::Busy};
use crate::{
    domain::fleet::Fleet,
    foundation::{DEFAULT_GCS_ID, SystemId},
    link::LinkId,
    mavlink::io::{AsyncReceiver, AsyncSender, new_async_receiver, new_async_sender},
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

#[tokio::test]
async fn busy_while_command_in_flight() {
    let (router, _guard, mut vehicle) = setup();
    let key = LockKey::command(VEHICLE, MavCmd::RequestMessage);
    assert!(!router.is_busy(&key));

    let command = router.command(VEHICLE, commands::request_message::<SystemTime>());
    let task = tokio::spawn(command.into_future());

    let DefaultDialect::CommandLong(long) = vehicle.recv().await else {
        panic!("expected COMMAND_LONG")
    };
    assert!(router.is_busy(&key));

    let second = router
        .command(VEHICLE, commands::request_message::<SystemTime>())
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
                .mission_download(VEHICLE, MavMissionType::Mission)
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
    assert_eq!(downloaded.vehicle_id, 7);
    assert_eq!(downloaded.items.len(), 3);
    for (seq, item) in downloaded.items.iter().enumerate() {
        assert_eq!(item.seq, seq as u16);
        assert_eq!(item.x, 100 + seq as i32);
    }
}

#[tokio::test]
async fn mission_upload_sends_items_and_returns_id() {
    let (router, _guard, mut vehicle) = setup();
    let items = vec![mission_item(0, 11), mission_item(1, 22)];

    let task = tokio::spawn({
        let router = router.clone();
        let items = items.clone();
        async move {
            router
                .mission_upload(VEHICLE, MavMissionType::Mission, &items)
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

#[tokio::test]
async fn mission_upload_answers_legacy_mission_request() {
    let (router, _guard, mut vehicle) = setup();
    let items = vec![mission_item(0, 11), mission_item(1, 22)];

    let task = tokio::spawn({
        let router = router.clone();
        let items = items.clone();
        async move {
            router
                .mission_upload(VEHICLE, MavMissionType::Mission, &items)
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
                .mission_download(VEHICLE, MavMissionType::Mission)
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
    // `Pin<Box<_>>` is `Unpin`, which `StreamExt::next` needs.
    let mut events = Box::pin(events);
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
        match tokio::time::timeout_at(deadline, events.as_mut().next()).await {
            Ok(Some(Event::Message(inbound))) => {
                let Ok(message) = inbound.frame.decode::<DefaultDialect>() else {
                    continue;
                };
                fleet.observe(inbound.source_mav, inbound.received_at, Some(message));
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
    assert_eq!(vehicle.identity.armed(), Some(true));

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
        .state
        .as_ref()
        .expect("battery should be recorded");
    assert!(
        (battery.voltage - 12.0).abs() < 1e-6,
        "voltage was {}",
        battery.voltage
    );

    assert!(
        vehicle
            .status
            .texts
            .iter()
            .any(|text| text.text == "REPLAY_OK"),
        "the status text should be recorded"
    );
}
