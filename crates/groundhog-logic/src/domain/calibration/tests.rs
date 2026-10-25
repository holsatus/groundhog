//! Tests of the calibration protocols: the commands each firmware is sent,
//! and how its feedback advances a session.

use std::time::Instant;

use mavio::default_dialect::{
    enums::{MagCalStatus, MavSeverity},
    messages::{MagCalProgress, MagCalReport},
};

use super::{ardupilot::ArduPilotCalibration, px4::Px4Calibration, *};

fn session(protocol: &dyn CalibrationProtocol, kind: CalibrationKind) -> Session {
    Session::new(kind, protocol.completes_on_ack(kind), RunId(1))
}

#[test]
fn airspeed_is_offered_only_to_winged_vehicles() {
    let protocols: [&dyn CalibrationProtocol; 2] = [&ArduPilotCalibration, &Px4Calibration];
    for protocol in protocols {
        for class in [
            VehicleClass::Copter,
            VehicleClass::Rover,
            VehicleClass::Submarine,
        ] {
            assert!(!protocol.kinds(class).contains(&CalibrationKind::Airspeed));
            assert!(
                protocol
                    .kinds(class)
                    .contains(&CalibrationKind::Accelerometer)
            );
        }
        for class in [VehicleClass::FixedWing, VehicleClass::Vtol] {
            assert!(protocol.kinds(class).contains(&CalibrationKind::Airspeed));
        }
    }
}

fn text(protocol: &dyn CalibrationProtocol, session: &mut Session, text: &str) {
    let text = StatusText {
        component: 1,
        at: Instant::now(),
        severity: MavSeverity::Info,
        text: text.to_string(),
    };
    protocol.observe(session, Feedback::Text(&text));
}

fn message(protocol: &dyn CalibrationProtocol, session: &mut Session, message: DefaultDialect) {
    protocol.observe(session, Feedback::Message(&message));
}

fn accelcal_position(position: f32) -> DefaultDialect {
    DefaultDialect::CommandLong(CommandLong {
        command: MavCmd::AccelcalVehiclePos,
        param1: position,
        ..Default::default()
    })
}

fn params(command: &CommandLong) -> [f32; 7] {
    [
        command.param1,
        command.param2,
        command.param3,
        command.param4,
        command.param5,
        command.param6,
        command.param7,
    ]
}

/// The two firmwares agree on the accelerometer, and disagree on airspeed
/// and compass: the reason calibration is a firmware adapter concern.
#[test]
fn start_commands_follow_each_firmware() {
    let ardupilot = ArduPilotCalibration;
    let px4 = Px4Calibration;

    for protocol in [&ardupilot as &dyn CalibrationProtocol, &px4] {
        let accel = protocol.start(CalibrationKind::Accelerometer);
        assert_eq!(accel.command, MavCmd::PreflightCalibration);
        assert_eq!(params(&accel), [0., 0., 0., 0., 1., 0., 0.]);
    }

    assert_eq!(
        params(&ardupilot.start(CalibrationKind::Airspeed)),
        [0., 0., 1., 0., 0., 0., 0.],
        "ArduPilot: ground pressure"
    );
    assert_eq!(
        params(&px4.start(CalibrationKind::Airspeed)),
        [0., 0., 0., 0., 0., 2., 0.],
        "PX4: param6"
    );

    assert_eq!(
        ardupilot.start(CalibrationKind::Compass).command,
        MavCmd::DoStartMagCal
    );
    assert_eq!(params(&px4.start(CalibrationKind::Compass))[1], 1.0);
}

#[test]
fn ardupilot_accelerometer_asks_for_each_side_and_waits_for_confirmation() {
    let protocol = ArduPilotCalibration;
    let mut session = session(&protocol, CalibrationKind::Accelerometer);
    session.acknowledged(Ok(()));
    assert_eq!(session.phase, Phase::Running, "accepted is only the start");
    assert_eq!(protocol.confirm(&session), None, "nothing asked yet");

    message(&protocol, &mut session, accelcal_position(1.0));
    assert_eq!(session.instruction, Instruction::Place(Side::Level));
    let confirm = protocol.confirm(&session).expect("waits for the operator");
    assert_eq!(confirm.command, MavCmd::AccelcalVehiclePos);
    assert_eq!(confirm.param1, 1.0);

    // Repeated requests for the same side change nothing.
    message(&protocol, &mut session, accelcal_position(1.0));
    assert_eq!(session.sides.count(), 0);

    message(&protocol, &mut session, accelcal_position(3.0));
    assert_eq!(session.instruction, Instruction::Place(Side::Right));
    assert!(session.sides.is_done(Side::Level) && session.sides.is_done(Side::Left));
    assert_eq!(session.progress, Some(33));

    message(&protocol, &mut session, accelcal_position(16_777_215.0));
    assert_eq!(session.phase, Phase::Succeeded);
    assert_eq!(session.sides.count(), 6);
}

/// ArduPilot repeats the previous run's result every second; one arriving
/// as a new run starts must not end it.
#[test]
fn ardupilot_accelerometer_ignores_an_earlier_result() {
    let protocol = ArduPilotCalibration;
    let mut session = session(&protocol, CalibrationKind::Accelerometer);
    session.acknowledged(Ok(()));

    message(&protocol, &mut session, accelcal_position(16_777_215.0));
    assert_eq!(session.phase, Phase::Running);

    message(&protocol, &mut session, accelcal_position(1.0));
    message(&protocol, &mut session, accelcal_position(16_777_216.0));
    assert!(matches!(session.phase, Phase::Failed(_)));
}

#[test]
fn ardupilot_level_completes_with_its_acknowledgement() {
    let protocol = ArduPilotCalibration;
    let mut session = session(&protocol, CalibrationKind::Level);
    session.acknowledged(Ok(()));
    assert_eq!(session.phase, Phase::Succeeded);

    let mut session = self::session(&protocol, CalibrationKind::Airspeed);
    session.acknowledged(Err("the vehicle refused: failed".into()));
    assert_eq!(
        session.phase,
        Phase::Failed("the vehicle refused: failed".into())
    );
}

fn mag_progress(compass_id: u8, completion_pct: u8) -> DefaultDialect {
    DefaultDialect::MagCalProgress(MagCalProgress {
        compass_id,
        completion_pct,
        cal_status: MagCalStatus::MagCalRunningStepOne,
        ..Default::default()
    })
}

fn mag_report(compass_id: u8, cal_status: MagCalStatus) -> DefaultDialect {
    DefaultDialect::MagCalReport(MagCalReport {
        compass_id,
        cal_status,
        fitness: 8.5,
        ..Default::default()
    })
}

#[test]
fn ardupilot_compass_succeeds_once_every_compass_reports() {
    let protocol = ArduPilotCalibration;
    let mut session = session(&protocol, CalibrationKind::Compass);
    session.acknowledged(Ok(()));

    // A report left over from an earlier run, before this one's progress.
    message(
        &protocol,
        &mut session,
        mag_report(2, MagCalStatus::MagCalSuccess),
    );
    assert!(session.compasses.is_empty());

    message(&protocol, &mut session, mag_progress(1, 40));
    message(&protocol, &mut session, mag_progress(0, 60));
    assert_eq!(session.progress, Some(40), "the slowest compass");
    assert_eq!(
        session.compasses.iter().map(|c| c.id).collect::<Vec<_>>(),
        [0, 1]
    );

    message(
        &protocol,
        &mut session,
        mag_report(0, MagCalStatus::MagCalSuccess),
    );
    assert_eq!(session.phase, Phase::Running, "compass 1 is still running");

    message(
        &protocol,
        &mut session,
        mag_report(1, MagCalStatus::MagCalSuccess),
    );
    assert_eq!(session.phase, Phase::Succeeded);
    assert!(session.needs_reboot);
    assert_eq!(session.compasses[1].fitness, Some(8.5));
}

#[test]
fn ardupilot_compass_fails_when_any_compass_does() {
    let protocol = ArduPilotCalibration;
    let mut session = session(&protocol, CalibrationKind::Compass);
    message(&protocol, &mut session, mag_progress(0, 10));
    message(&protocol, &mut session, mag_progress(1, 10));
    message(
        &protocol,
        &mut session,
        mag_report(0, MagCalStatus::MagCalSuccess),
    );
    message(
        &protocol,
        &mut session,
        mag_report(1, MagCalStatus::MagCalBadRadius),
    );

    let Phase::Failed(reason) = &session.phase else {
        panic!("expected a failure, got {:?}", session.phase);
    };
    assert!(reason.starts_with("compass 2"), "{reason}");
    assert!(!session.needs_reboot);
    assert!(
        protocol.cancel(&session).is_some(),
        "compass runs can be cancelled"
    );
}

/// The compass number comes from the wire: the last one must not overflow.
#[test]
fn the_last_compass_id_is_numbered_without_overflow() {
    let protocol = ArduPilotCalibration;
    let mut session = session(&protocol, CalibrationKind::Compass);
    message(&protocol, &mut session, mag_progress(255, 10));
    message(
        &protocol,
        &mut session,
        mag_report(255, MagCalStatus::MagCalBadRadius),
    );

    let Phase::Failed(reason) = &session.phase else {
        panic!("expected a failure, got {:?}", session.phase);
    };
    assert!(reason.starts_with("compass 256"), "{reason}");
}

#[test]
fn ardupilot_keeps_calibration_texts_only() {
    let protocol = ArduPilotCalibration;
    let mut session = session(&protocol, CalibrationKind::Accelerometer);
    text(&protocol, &mut session, "PreArm: Throttle below failsafe");
    text(
        &protocol,
        &mut session,
        "Place vehicle level and press any key.",
    );
    text(&protocol, &mut session, "Calibration successful");
    assert_eq!(session.messages, ["Calibration successful"]);
}

#[test]
fn px4_accelerometer_follows_the_narration() {
    let protocol = Px4Calibration;
    let mut session = session(&protocol, CalibrationKind::Accelerometer);
    session.acknowledged(Ok(()));

    text(
        &protocol,
        &mut session,
        "[cal] calibration started: 2 accel",
    );
    text(
        &protocol,
        &mut session,
        "[cal] pending: back front left right up down",
    );
    assert_eq!(session.instruction, Instruction::HoldOnPendingSide);
    assert_eq!(session.sides.count(), 0);

    text(
        &protocol,
        &mut session,
        "[cal] detected motion, hold still...",
    );
    text(
        &protocol,
        &mut session,
        "[cal] detected motion, hold still...",
    );
    text(&protocol, &mut session, "[cal] down orientation detected");
    assert_eq!(session.instruction, Instruction::Measuring(Side::Level));
    assert_eq!(protocol.confirm(&session), None, "PX4 detects sides itself");

    text(&protocol, &mut session, "[cal] progress <17>");
    text(
        &protocol,
        &mut session,
        "[cal] down side done, rotate to a different side",
    );
    assert!(session.sides.is_done(Side::Level));
    assert_eq!(session.progress, Some(17));

    text(
        &protocol,
        &mut session,
        "[cal] pending: back front right up",
    );
    assert!(
        session.sides.is_done(Side::Left),
        "left is no longer pending"
    );
    assert!(!session.sides.is_done(Side::NoseUp));

    text(&protocol, &mut session, "[cal] calibration done: accel");
    assert_eq!(session.phase, Phase::Succeeded);
    assert_eq!(session.sides.count(), 6);
    assert_eq!(
        session.messages,
        ["detected motion, hold still..."],
        "only what the session does not show, repeats collapsed"
    );
}

#[test]
fn px4_compass_rotates_on_each_side() {
    let protocol = Px4Calibration;
    let mut session = session(&protocol, CalibrationKind::Compass);
    text(&protocol, &mut session, "[cal] calibration started: 2 mag");
    text(&protocol, &mut session, "[cal] front orientation detected");
    assert_eq!(
        session.instruction,
        Instruction::Rotate(Some(Side::NoseDown))
    );
    text(&protocol, &mut session, "[cal] Rotate vehicle");
    assert_eq!(
        session.instruction,
        Instruction::Rotate(Some(Side::NoseDown))
    );

    text(&protocol, &mut session, "[cal] calibration failed: mag");
    assert_eq!(session.phase, Phase::Failed("mag".into()));
}

#[test]
fn px4_airspeed_asks_for_airflow() {
    let protocol = Px4Calibration;
    let mut session = session(&protocol, CalibrationKind::Airspeed);
    assert_eq!(session.instruction, Instruction::ShieldPitot);
    text(
        &protocol,
        &mut session,
        "[cal] Ensure sensor is not measuring wind",
    );
    text(
        &protocol,
        &mut session,
        "[cal] Blow into front of pitot without touching",
    );
    assert_eq!(session.instruction, Instruction::BlowIntoPitot);
    text(&protocol, &mut session, "[cal] calibration cancelled");
    assert_eq!(session.phase, Phase::Cancelled);
}

#[test]
fn px4_cancels_with_an_empty_preflight_calibration() {
    let protocol = Px4Calibration;
    let session = session(&protocol, CalibrationKind::Gyroscope);
    let cancel = protocol.cancel(&session).expect("PX4 can cancel");
    assert_eq!(cancel.command, MavCmd::PreflightCalibration);
    assert_eq!(params(&cancel), [0.0; 7]);
}

#[test]
fn finished_sessions_ignore_late_acknowledgements() {
    let protocol = Px4Calibration;
    let mut session = session(&protocol, CalibrationKind::Gyroscope);
    text(&protocol, &mut session, "[cal] calibration done: gyro");
    session.acknowledged(Err("timeout".into()));
    assert_eq!(session.phase, Phase::Succeeded);
    session.cancel();
    assert_eq!(session.phase, Phase::Succeeded);
}

/// Once the operator says the vehicle is in place, the session waits for the
/// vehicle's next progress instead of asking again (a repeat is refused).
#[test]
fn a_sent_confirmation_waits_for_the_vehicle() {
    let protocol = ArduPilotCalibration;
    let mut session = session(&protocol, CalibrationKind::Accelerometer);
    session.acknowledged(Ok(()));
    message(&protocol, &mut session, accelcal_position(1.0));
    assert_eq!(session.instruction, Instruction::Place(Side::Level));

    session.confirmed(Side::Level);
    assert_eq!(session.instruction, Instruction::Confirming(Side::Level));
    assert_eq!(protocol.confirm(&session), None, "not asked twice");

    // A failed send asks the operator again.
    session.confirm_failed(Side::Level);
    assert_eq!(session.instruction, Instruction::Place(Side::Level));

    session.confirmed(Side::Level);
    message(&protocol, &mut session, accelcal_position(2.0));
    assert_eq!(session.instruction, Instruction::Place(Side::Left));
}

/// The cancel caption is the firmware's (and the kind's) to word: some kinds
/// finish before acknowledging, the accelerometer keeps waiting.
#[test]
fn cancelling_is_explained_per_firmware_and_kind() {
    let ardupilot = ArduPilotCalibration;

    assert_eq!(
        ardupilot.cancel_note(CalibrationKind::Compass),
        None,
        "a compass run can be stopped"
    );
    assert!(
        ardupilot
            .cancel_note(CalibrationKind::Accelerometer)
            .is_some_and(|note| note.contains("orientation"))
    );
    assert!(
        ardupilot
            .cancel_note(CalibrationKind::Gyroscope)
            .is_some_and(|note| note.contains("on its own"))
    );

    assert_eq!(ardupilot.cancel_retries(), 0);
    assert_eq!(Px4Calibration.cancel_retries(), 1, "PX4 acknowledges twice");
}

#[test]
fn px4_sides_move_on_with_the_pending_list() {
    let protocol = Px4Calibration;
    let mut session = session(&protocol, CalibrationKind::Accelerometer);
    session.acknowledged(Ok(()));

    text(
        &protocol,
        &mut session,
        "[cal] calibration started: 2 accel",
    );
    text(
        &protocol,
        &mut session,
        "[cal] pending: back front left right up down",
    );
    assert_eq!(session.sides.count(), 0, "nothing is done yet");

    text(
        &protocol,
        &mut session,
        "[cal] down side done, rotate to a different side",
    );
    assert!(session.sides.is_done(Side::Level));

    text(
        &protocol,
        &mut session,
        "[cal] pending: back front left right up",
    );
    assert_eq!(session.sides.count(), 1, "only the level side is done");
}

/// The per-side progress form is progress, not a log line.
#[test]
fn px4_parses_per_side_progress() {
    let protocol = Px4Calibration;
    let mut session = session(&protocol, CalibrationKind::Compass);
    session.acknowledged(Ok(()));

    text(
        &protocol,
        &mut session,
        "[cal] left side calibration: progress <42>",
    );
    assert_eq!(session.progress, Some(42));
    assert!(session.messages.is_empty(), "not kept in the log");
}

/// A pending list this build cannot name must not mark the unnamed sides
/// done.
#[test]
fn px4_does_not_guess_an_unknown_pending_side() {
    let protocol = Px4Calibration;
    let mut session = session(&protocol, CalibrationKind::Accelerometer);
    session.acknowledged(Ok(()));

    text(
        &protocol,
        &mut session,
        "[cal] pending: back wibble front left right up",
    );
    assert_eq!(session.sides.count(), 0);
    assert!(
        session
            .messages
            .back()
            .is_some_and(|note| note.contains("wibble")),
        "the unknown orientation is reported"
    );
}

#[test]
fn px4_notes_an_unexpected_protocol_version() {
    let protocol = Px4Calibration;
    let mut session = session(&protocol, CalibrationKind::Gyroscope);
    session.acknowledged(Ok(()));

    text(&protocol, &mut session, "[cal] calibration started: 2 gyro");
    assert!(session.messages.is_empty(), "the expected version is quiet");

    text(&protocol, &mut session, "[cal] calibration started: 3 gyro");
    assert!(
        session
            .messages
            .back()
            .is_some_and(|note| note.contains("protocol version 3"))
    );
}
