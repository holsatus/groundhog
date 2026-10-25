//! End-to-end tests of the services: a mock vehicle on an in-memory link,
//! the real router, the fleet runtime and the vehicle client.

use std::time::Duration;

use mav_param::{Ident, Value};
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{
            MavAutopilot, MavCmd, MavFrame, MavMissionResult, MavMissionType, MavModeFlag, MavType,
        },
        messages::{
            Heartbeat, HomePosition, MissionAck, MissionCount, MissionItemInt, MissionRequestInt,
        },
    },
    prelude::V2,
    protocol::FrameBuilder,
};

use super::{
    runtime::{FleetUpdates, RuntimeOptions},
    *,
};
use crate::{
    MavlinkId, SystemId,
    domain::{
        fleet::{FleetEvent, FleetSnapshot},
        mission::{MissionItem, Position},
        parameter::DownloadState,
    },
    link::LinkId,
    mavlink::{
        DEFAULT_GCS_ID, LinkGuard, MavlinkRouter,
        io::{AsyncReceiver, AsyncSender, new_async_receiver, new_async_sender},
    },
};

const VEHICLE: MavlinkId = MavlinkId {
    system: 7,
    component: 1,
};
const SYSTEM: SystemId = SystemId(7);

struct MockVehicle {
    rx: AsyncReceiver,
    tx: AsyncSender,
    sequence: u8,
}

impl MockVehicle {
    async fn send(&mut self, message: &dyn mavio::Message) {
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

    /// The next message from the GCS that is not a heartbeat.
    async fn recv(&mut self) -> DefaultDialect {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), self.rx.recv())
                .await
                .expect("the GCS sends something")
                .expect("transport open");
            match frame.decode::<DefaultDialect>() {
                Ok(DefaultDialect::Heartbeat(_)) | Err(_) => continue,
                Ok(message) => return message,
            }
        }
    }

    async fn heartbeat(&mut self, autopilot: MavAutopilot) {
        self.heartbeat_of(autopilot, MavType::Quadrotor).await;
    }

    async fn heartbeat_of(&mut self, autopilot: MavAutopilot, type_: MavType) {
        self.send(&Heartbeat {
            type_,
            autopilot,
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
            ..Default::default()
        })
        .await;
    }
}

struct Harness {
    fleet: FleetHandle,
    updates: FleetUpdates,
    vehicle: MockVehicle,
    _guard: LinkGuard,
    _router: MavlinkRouter,
    _runtime: tokio_util::task::AbortOnDropHandle<()>,
}

fn harness() -> Harness {
    harness_with(RuntimeOptions { bootstrap: false })
}

fn harness_with(options: RuntimeOptions) -> Harness {
    let (router, events) = MavlinkRouter::new(DEFAULT_GCS_ID);
    let (fleet, updates, runtime) = runtime::spawn(router.clone(), events, options);

    let (gcs, vehicle) = tokio::io::duplex(1 << 16);
    let (gcs_reader, gcs_writer) = tokio::io::split(gcs);
    let (vehicle_reader, vehicle_writer) = tokio::io::split(vehicle);
    let guard = router.attach_reader_writer(LinkId::new("test"), gcs_reader, gcs_writer, false);

    Harness {
        fleet,
        updates,
        vehicle: MockVehicle {
            rx: new_async_receiver(vehicle_reader),
            tx: new_async_sender(vehicle_writer),
            sequence: 0,
        },
        _guard: guard,
        _router: router,
        _runtime: runtime,
    }
}

/// Wait for a published snapshot satisfying `done`.
async fn until(updates: &mut FleetUpdates, done: impl Fn(&FleetSnapshot) -> bool) -> FleetSnapshot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let update = tokio::time::timeout_at(deadline, updates.next())
            .await
            .expect("the condition is reached in time")
            .expect("the runtime runs");
        if done(&update.snapshot) {
            return (*update.snapshot).clone();
        }
    }
}

async fn register(harness: &mut Harness, autopilot: MavAutopilot) {
    register_as(harness, autopilot, MavType::Quadrotor).await;
}

async fn register_as(harness: &mut Harness, autopilot: MavAutopilot, type_: MavType) {
    harness.vehicle.heartbeat_of(autopilot, type_).await;
    until(&mut harness.updates, |fleet| fleet.contains(SYSTEM)).await;
}

#[tokio::test]
async fn a_heartbeat_registers_the_vehicle_and_publishes_it() {
    let mut harness = harness();
    harness.vehicle.heartbeat(MavAutopilot::Ardupilotmega).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut registered = false;
    while !registered {
        let update = tokio::time::timeout_at(deadline, harness.updates.next())
            .await
            .expect("an update arrives")
            .unwrap();
        registered = update.events.contains(&FleetEvent::Registered(SYSTEM));
        if registered {
            assert!(
                update.snapshot.contains(SYSTEM),
                "the snapshot already has it"
            );
        }
    }

    assert_eq!(
        harness
            .fleet
            .snapshot()
            .vehicle(SYSTEM)
            .unwrap()
            .firmware()
            .name(),
        "ArduPilot"
    );
}

/// ArduPilot keeps home in mission slot 0: the upload must put home there and
/// the plan after it, and report the vehicle's mission back to the fleet.
#[tokio::test]
async fn uploads_to_ardupilot_fill_the_home_slot() {
    let mut harness = harness();
    register(&mut harness, MavAutopilot::Ardupilotmega).await;

    harness
        .vehicle
        .send(&HomePosition {
            latitude: 471_000_000,
            longitude: 81_000_000,
            altitude: 420_000,
            ..Default::default()
        })
        .await;
    until(&mut harness.updates, |fleet| {
        fleet
            .vehicle(SYSTEM)
            .is_some_and(|vehicle| vehicle.home().is_some())
    })
    .await;

    let waypoint = MissionItem::waypoint(Position {
        lat: 47.2,
        lon: 8.2,
        alt_m: 60.0,
    });
    let client = harness.fleet.vehicle(SYSTEM);
    let items = vec![waypoint.clone()];
    let upload = tokio::spawn(async move { client.upload_mission(items, &|_| {}).await });

    let DefaultDialect::MissionCount(count) = harness.vehicle.recv().await else {
        panic!("expected MISSION_COUNT")
    };
    assert_eq!(count.count, 2, "home plus one waypoint");

    let mut received = Vec::new();
    for seq in 0..2 {
        harness
            .vehicle
            .send(&MissionRequestInt {
                seq,
                mission_type: MavMissionType::Mission,
                target_system: DEFAULT_GCS_ID.system,
                target_component: DEFAULT_GCS_ID.component,
            })
            .await;
        let DefaultDialect::MissionItemInt(item) = harness.vehicle.recv().await else {
            panic!("expected MISSION_ITEM_INT")
        };
        received.push(item);
    }

    assert_eq!(received[0].x, 471_000_000, "home in slot 0");
    assert_eq!(received[0].z, 420.0);
    assert_eq!(
        MissionItem::from_wire(&received[1]),
        waypoint,
        "the plan from slot 1"
    );

    harness
        .vehicle
        .send(&MissionAck {
            type_: MavMissionResult::MavMissionAccepted,
            mission_type: MavMissionType::Mission,
            opaque_id: 77,
            ..Default::default()
        })
        .await;

    let onboard = upload.await.unwrap().expect("the upload succeeds");
    assert_eq!(onboard.items, vec![waypoint]);
    assert_eq!(onboard.opaque_id, 77);

    let fleet = until(&mut harness.updates, |fleet| {
        fleet
            .vehicle(SYSTEM)
            .and_then(|vehicle| vehicle.mission.onboard.as_ref())
            .is_some_and(|onboard| onboard.opaque_id == 77)
    })
    .await;
    assert_eq!(
        fleet
            .vehicle(SYSTEM)
            .unwrap()
            .mission
            .onboard
            .as_ref()
            .unwrap()
            .items
            .len(),
        1,
        "the fleet holds the plan, home excluded"
    );
}

#[tokio::test]
async fn downloads_from_ardupilot_set_the_home_slot_apart() {
    let mut harness = harness();
    register(&mut harness, MavAutopilot::Ardupilotmega).await;

    let client = harness.fleet.vehicle(SYSTEM);
    let download = tokio::spawn(async move { client.download_mission(&|_| {}).await });

    let DefaultDialect::MissionRequestList(_) = harness.vehicle.recv().await else {
        panic!("expected MISSION_REQUEST_LIST")
    };
    harness
        .vehicle
        .send(&MissionCount {
            count: 2,
            mission_type: MavMissionType::Mission,
            opaque_id: 5,
            ..Default::default()
        })
        .await;

    for seq in 0..2u16 {
        let DefaultDialect::MissionRequestInt(request) = harness.vehicle.recv().await else {
            panic!("expected MISSION_REQUEST_INT")
        };
        assert_eq!(request.seq, seq);
        harness
            .vehicle
            .send(&MissionItemInt {
                seq,
                frame: MavFrame::GlobalRelativeAltInt,
                command: MavCmd::NavWaypoint,
                x: 470_000_000 + i32::from(seq),
                y: 80_000_000,
                z: 30.0,
                autocontinue: 1,
                mission_type: MavMissionType::Mission,
                ..Default::default()
            })
            .await;
    }

    let onboard = download.await.unwrap().expect("the download succeeds");
    assert_eq!(onboard.items.len(), 1, "item 0 is home");
    assert_eq!(onboard.items[0].x, 470_000_001);
    assert_eq!(onboard.home.as_ref().map(|home| home.x), Some(470_000_000));
}

/// Without an advertised or firmware-implied encoding, a write could store a
/// different value than intended, so it is refused before anything is sent.
#[tokio::test]
async fn parameter_writes_need_a_known_encoding() {
    let mut harness = harness();
    register(&mut harness, MavAutopilot::Generic).await;

    let result = harness
        .fleet
        .vehicle(SYSTEM)
        .set_parameter(Ident::from_str_truncated("SOME_PARAM"), Value::I32(1))
        .await;

    assert_eq!(result, Err(Error::ParamEncodingUnknown));
}

/// Dropping a download (the operator cancelled it) must not leave it in
/// progress in the snapshot.
#[tokio::test]
async fn a_dropped_parameter_download_reports_itself_cancelled() {
    let mut harness = harness();
    register(&mut harness, MavAutopilot::Ardupilotmega).await;

    let client = harness.fleet.vehicle(SYSTEM);
    let download = tokio::spawn(async move { client.fetch_parameters().await });
    let DefaultDialect::ParamRequestList(_) = harness.vehicle.recv().await else {
        panic!("expected PARAM_REQUEST_LIST")
    };
    let state = |fleet: &FleetSnapshot| {
        let parameters = fleet.vehicle(SYSTEM)?.parameters(VEHICLE.component)?;
        Some(parameters.download().clone())
    };
    until(&mut harness.updates, |fleet| {
        state(fleet) == Some(DownloadState::InProgress)
    })
    .await;

    download.abort();

    until(&mut harness.updates, |fleet| {
        state(fleet) == Some(DownloadState::Cancelled)
    })
    .await;
}

#[tokio::test]
async fn operations_on_unknown_vehicles_fail_cleanly() {
    let harness = harness();
    let client = harness.fleet.vehicle(SystemId(99));

    assert_eq!(client.arm(true).await, Err(Error::UnknownVehicle));
    assert!(matches!(
        client.download_mission(&|_| {}).await,
        Err(Error::UnknownVehicle)
    ));
}

/// Dropping the backend stops the fleet runtime: no task outlives it, and a
/// handle kept past it sees the runtime gone rather than hanging.
#[tokio::test]
async fn dropping_the_backend_stops_the_runtime() {
    let (backend, _events) = crate::Backend::start(crate::BackendOptions {
        bootstrap: false,
        ..crate::BackendOptions::default()
    });
    let fleet = backend.fleet().clone();
    drop(backend);

    let stopped = fleet.wait_for(SYSTEM, Duration::from_secs(5));
    let stopped = tokio::time::timeout(Duration::from_secs(1), stopped).await;
    assert_eq!(stopped, Ok(false), "wait_for returns at once, unanswered");
}

/// The runtime bootstraps a vehicle as it registers it, before the snapshot
/// containing the vehicle is published: the bootstrap must wait for it rather
/// than give up on an "unknown" vehicle.
#[tokio::test]
async fn a_registered_vehicle_is_bootstrapped() {
    let mut harness = harness_with(RuntimeOptions { bootstrap: true });
    harness.vehicle.heartbeat(MavAutopilot::Ardupilotmega).await;

    let DefaultDialect::CommandLong(request) = harness.vehicle.recv().await else {
        panic!("expected the capabilities request")
    };
    assert_eq!(request.command, MavCmd::RequestMessage);
    assert_eq!(request.param1, 148.0, "AUTOPILOT_VERSION");
}

mod calibration {
    use mavio::default_dialect::{
        enums::{MagCalStatus, MavResult},
        messages::{CommandAck, CommandLong, MagCalProgress, MagCalReport},
    };

    use super::*;
    use crate::domain::calibration::{CalibrationKind, Instruction, Outcome, Phase, Session, Side};

    fn session(fleet: &FleetSnapshot) -> Option<&Session> {
        fleet.vehicle(SYSTEM)?.calibration.as_ref()
    }

    impl MockVehicle {
        /// The next command from the GCS.
        async fn command(&mut self) -> CommandLong {
            match self.recv().await {
                DefaultDialect::CommandLong(command) => command,
                other => panic!("expected a command, got {other:?}"),
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

        /// ArduPilot asking the GCS for an accelerometer orientation.
        async fn ask_position(&mut self, position: f32) {
            self.send(&CommandLong {
                command: MavCmd::AccelcalVehiclePos,
                param1: position,
                ..Default::default()
            })
            .await;
        }
    }

    /// The whole ArduPilot handshake: start, the vehicle asks for a side, the
    /// operator confirms it, the vehicle reports the result.
    #[tokio::test]
    async fn ardupilot_accelerometer_waits_for_each_confirmation() {
        let mut harness = harness();
        register(&mut harness, MavAutopilot::Ardupilotmega).await;

        let client = harness.fleet.vehicle(SYSTEM);
        let start = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .start_calibration(CalibrationKind::Accelerometer)
                    .await
            }
        });

        let command = harness.vehicle.command().await;
        assert_eq!(command.command, MavCmd::PreflightCalibration);
        assert_eq!(command.param5, 1.0);
        harness
            .vehicle
            .ack(MavCmd::PreflightCalibration, MavResult::Accepted)
            .await;
        start.await.unwrap().expect("accepted");

        harness.vehicle.ask_position(1.0).await;
        until(&mut harness.updates, |fleet| {
            session(fleet)
                .is_some_and(|session| session.instruction == Instruction::Place(Side::Level))
        })
        .await;

        let confirm = tokio::spawn({
            let client = client.clone();
            async move { client.confirm_calibration_step().await }
        });
        let command = harness.vehicle.command().await;
        assert_eq!(command.command, MavCmd::AccelcalVehiclePos);
        assert_eq!(command.param1, 1.0);
        harness
            .vehicle
            .ack(MavCmd::AccelcalVehiclePos, MavResult::Accepted)
            .await;
        confirm.await.unwrap().expect("accepted");

        harness.vehicle.ask_position(16_777_215.0).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let update = tokio::time::timeout_at(deadline, harness.updates.next())
                .await
                .expect("the calibration finishes")
                .unwrap();
            if update.events.contains(&FleetEvent::CalibrationFinished(
                SYSTEM,
                CalibrationKind::Accelerometer,
                Outcome::Succeeded,
            )) {
                break;
            }
        }
    }

    /// ArduPilot runs the airspeed calibration inside the command: the
    /// `IN_PROGRESS` acknowledgements keep it open, the final one ends it.
    #[tokio::test]
    async fn ardupilot_airspeed_completes_with_the_command() {
        let mut harness = harness();
        register_as(
            &mut harness,
            MavAutopilot::Ardupilotmega,
            MavType::FixedWing,
        )
        .await;

        let client = harness.fleet.vehicle(SYSTEM);
        let start =
            tokio::spawn(async move { client.start_calibration(CalibrationKind::Airspeed).await });

        let command = harness.vehicle.command().await;
        assert_eq!(command.param3, 1.0, "ArduPilot: ground pressure");
        for _ in 0..3 {
            harness
                .vehicle
                .ack(MavCmd::PreflightCalibration, MavResult::InProgress)
                .await;
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let running = harness.fleet.snapshot();
        assert!(session(&running).is_some_and(Session::is_running));
        harness
            .vehicle
            .ack(MavCmd::PreflightCalibration, MavResult::Accepted)
            .await;

        start.await.unwrap().expect("succeeded");
        until(&mut harness.updates, |fleet| {
            session(fleet).is_some_and(|session| session.phase == Phase::Succeeded)
        })
        .await;
    }

    /// A copter has no airspeed sensor to calibrate.
    #[tokio::test]
    async fn airspeed_is_refused_without_wings() {
        let mut harness = harness();
        register(&mut harness, MavAutopilot::Ardupilotmega).await;

        let client = harness.fleet.vehicle(SYSTEM);
        let result = client.start_calibration(CalibrationKind::Airspeed).await;
        assert!(matches!(result, Err(Error::CalibrationUnsupported)));
    }

    /// A start dropped before the vehicle answers must not leave the session
    /// starting: nothing else would ever end it.
    #[tokio::test]
    async fn an_abandoned_start_ends_the_session() {
        let mut harness = harness();
        register(&mut harness, MavAutopilot::Ardupilotmega).await;

        let client = harness.fleet.vehicle(SYSTEM);
        let start = tokio::spawn(async move {
            client
                .start_calibration(CalibrationKind::Accelerometer)
                .await
        });
        harness.vehicle.command().await;
        until(&mut harness.updates, |fleet| {
            session(fleet).is_some_and(Session::is_running)
        })
        .await;

        start.abort();
        until(&mut harness.updates, |fleet| {
            session(fleet).is_some_and(|session| matches!(session.phase, Phase::Failed(_)))
        })
        .await;
    }

    /// A start the vehicle refuses ends the session with the reason.
    #[tokio::test]
    async fn a_refused_start_fails_the_session() {
        let mut harness = harness();
        register(&mut harness, MavAutopilot::Px4).await;

        let client = harness.fleet.vehicle(SYSTEM);
        let start =
            tokio::spawn(async move { client.start_calibration(CalibrationKind::Compass).await });
        assert_eq!(harness.vehicle.command().await.param2, 1.0);
        harness
            .vehicle
            .ack(MavCmd::PreflightCalibration, MavResult::TemporarilyRejected)
            .await;

        assert!(start.await.unwrap().is_err());
        until(&mut harness.updates, |fleet| {
            session(fleet).is_some_and(|session| matches!(session.phase, Phase::Failed(_)))
        })
        .await;
    }

    /// ArduPilot reports compass failures the bundled message definitions
    /// do not know yet. They must still end the calibration.
    #[tokio::test]
    async fn an_unknown_compass_failure_still_fails_the_calibration() {
        let mut harness = harness();
        register(&mut harness, MavAutopilot::Ardupilotmega).await;

        let client = harness.fleet.vehicle(SYSTEM);
        let start =
            tokio::spawn(async move { client.start_calibration(CalibrationKind::Compass).await });
        assert_eq!(
            harness.vehicle.command().await.command,
            MavCmd::DoStartMagCal
        );
        harness
            .vehicle
            .ack(MavCmd::DoStartMagCal, MavResult::Accepted)
            .await;
        start.await.unwrap().expect("accepted");

        harness
            .vehicle
            .send(&MagCalProgress {
                compass_id: 0,
                completion_pct: 50,
                cal_status: MagCalStatus::MagCalRunningStepTwo,
                ..Default::default()
            })
            .await;

        // MAG_CAL_FAILED_RESIDUALS_HIGH (10), written over a known status.
        let report = MagCalReport {
            compass_id: 0,
            cal_status: MagCalStatus::MagCalFailed,
            fitness: 120.0,
            ..Default::default()
        };
        let frame = |payload: Option<&[u8]>| {
            let builder = FrameBuilder::new()
                .system_id(VEHICLE.system)
                .component_id(VEHICLE.component)
                .version(V2)
                .sequence(200);
            match payload {
                None => builder.message(&report).unwrap().build(),
                Some(bytes) => builder
                    .message_id(MagCalReport::message_id())
                    .payload(bytes)
                    .crc_extra(MagCalReport::crc_extra())
                    .build(),
            }
            .into_versionless()
        };
        let mut bytes = frame(None).payload().bytes().to_vec();
        bytes[42] = 10;
        let frame = frame(Some(&bytes));
        assert!(
            frame.decode::<DefaultDialect>().is_err(),
            "unknown to mavio"
        );
        harness.vehicle.tx.send(&frame).await.unwrap();

        let fleet = until(&mut harness.updates, |fleet| {
            session(fleet).is_some_and(|session| !session.is_running())
        })
        .await;
        let session = session(&fleet).unwrap();
        assert!(matches!(&session.phase, Phase::Failed(reason) if reason.starts_with("compass 1")));
        assert_eq!(session.compasses[0].fitness, Some(120.0));
    }
}
