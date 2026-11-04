//! Tests of a vehicle's calibration through its interface: what the
//! operation running a run reports, the vehicle's feedback, the fleet's facts
//! and ticks, and what comes out (the session, the controls, the run's end).

use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MagCalStatus, MavCmd, MavSeverity},
        messages,
    },
};

use super::*;
use crate::domain::{
    calibration::{ardupilot::ArduPilotCalibration, px4::Px4Calibration},
    vehicle::StatusText,
};

const ARDUPILOT: &dyn CalibrationProtocol = &ArduPilotCalibration;
const PX4: &dyn CalibrationProtocol = &Px4Calibration;

/// A run of `kind` under `protocol`, begun at `at`, not yet acknowledged.
fn begun(
    protocol: &'static dyn CalibrationProtocol,
    kind: CalibrationKind,
    at: Instant,
) -> (Calibration, RunId) {
    let mut calibration = Calibration::default();
    let run = RunId::next();
    assert!(
        calibration
            .apply(at, CalibrationUpdate::Begin { kind, run }, Some(protocol))
            .is_none()
    );
    (calibration, run)
}

/// A run of `kind` under `protocol`, begun and acknowledged at `at`.
fn started(
    protocol: &'static dyn CalibrationProtocol,
    kind: CalibrationKind,
    at: Instant,
) -> (Calibration, RunId) {
    let (mut calibration, run) = begun(protocol, kind, at);
    calibration.apply(
        at,
        CalibrationUpdate::Acknowledged {
            run,
            result: Ok(()),
        },
        None,
    );
    (calibration, run)
}

fn message(calibration: &mut Calibration, at: Instant, message: &DefaultDialect) -> Option<Ended> {
    calibration.observe(at, Feedback::Message(message))
}

fn text(calibration: &mut Calibration, at: Instant, text: &str) -> Option<Ended> {
    let text = StatusText {
        component: 1,
        at,
        severity: MavSeverity::Info,
        text: text.to_string(),
    };
    calibration.observe(at, Feedback::Text(&text))
}

fn accelcal_position(position: f32) -> DefaultDialect {
    DefaultDialect::CommandLong(messages::CommandLong {
        command: MavCmd::AccelcalVehiclePos,
        param1: position,
        ..Default::default()
    })
}

fn mag_progress(completion_pct: u8) -> DefaultDialect {
    DefaultDialect::MagCalProgress(messages::MagCalProgress {
        compass_id: 0,
        completion_pct,
        cal_status: MagCalStatus::MagCalRunningStepOne,
        ..Default::default()
    })
}

fn mag_success() -> DefaultDialect {
    DefaultDialect::MagCalReport(messages::MagCalReport {
        compass_id: 0,
        cal_status: MagCalStatus::MagCalSuccess,
        ..Default::default()
    })
}

fn instruction(calibration: &Calibration) -> Instruction {
    calibration.session().expect("a run").instruction
}

fn failure(ended: Option<Ended>) -> String {
    match ended.map(|ended| ended.outcome) {
        Some(Outcome::Failed(reason)) => reason,
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// Seconds after `at`.
fn after(at: Instant, seconds: u64) -> Instant {
    at + Duration::from_secs(seconds)
}

const DISARMED: Option<bool> = Some(false);

#[test]
fn a_start_needs_a_vehicle_known_to_be_disarmed_and_answering() {
    let idle = Calibration::default();
    let blocked = |armed, liveness| idle.controls(armed, liveness).start_blocked;

    assert_eq!(blocked(DISARMED, Liveness::Active), None);
    assert_eq!(
        blocked(DISARMED, Liveness::Stale),
        None,
        "a stale vehicle may still answer"
    );
    assert_eq!(
        blocked(Some(true), Liveness::Active),
        Some(StartBlocked::NotDisarmed)
    );
    assert_eq!(
        blocked(None, Liveness::Active),
        Some(StartBlocked::NotDisarmed),
        "an unknown armed state is not disarmed"
    );
    assert_eq!(blocked(DISARMED, Liveness::Lost), Some(StartBlocked::Lost));
}

#[test]
fn a_running_run_blocks_another_and_a_finished_one_does_not() {
    let at = Instant::now();
    let (mut calibration, _) = started(ARDUPILOT, CalibrationKind::Accelerometer, at);
    assert_eq!(
        calibration
            .controls(DISARMED, Liveness::Active)
            .start_blocked,
        Some(StartBlocked::Running)
    );

    calibration.apply(at, CalibrationUpdate::Cancel, None);
    assert_eq!(
        calibration
            .controls(DISARMED, Liveness::Active)
            .start_blocked,
        None
    );
}

/// The controls a frontend offers come from the firmware's protocol and the
/// step the run is at.
#[test]
fn controls_follow_the_protocol_and_the_step() {
    let at = Instant::now();
    let (mut ardupilot, _) = started(ARDUPILOT, CalibrationKind::Accelerometer, at);
    let controls = ardupilot.controls(DISARMED, Liveness::Active);
    assert!(
        !controls.confirm,
        "nothing to confirm before the vehicle asks"
    );
    assert!(
        !controls.cancel,
        "ArduPilot cannot stop an accelerometer run"
    );

    message(&mut ardupilot, at, &accelcal_position(1.0));
    assert!(ardupilot.controls(DISARMED, Liveness::Active).confirm);

    let (mut px4, _) = begun(PX4, CalibrationKind::Gyroscope, at);
    let controls = px4.controls(DISARMED, Liveness::Active);
    assert!(controls.cancel);
    assert!(!controls.confirm);

    px4.apply(at, CalibrationUpdate::Cancel, None);
    let controls = px4.controls(DISARMED, Liveness::Active);
    assert!(
        !controls.cancel && !controls.confirm,
        "a finished run has no controls"
    );
}

/// What cancelling an ArduPilot accelerometer start does is explained only
/// while the start is pending.
#[test]
fn a_cancel_note_shows_while_the_start_is_pending() {
    let at = Instant::now();
    let (mut calibration, run) = begun(ARDUPILOT, CalibrationKind::Accelerometer, at);
    assert!(
        calibration
            .controls(DISARMED, Liveness::Active)
            .cancel_note
            .is_some()
    );

    calibration.apply(
        at,
        CalibrationUpdate::Acknowledged {
            run,
            result: Ok(()),
        },
        None,
    );
    assert!(
        calibration
            .controls(DISARMED, Liveness::Active)
            .cancel_note
            .is_none()
    );
}

#[test]
fn the_confirmation_names_the_run_and_the_side() {
    let at = Instant::now();
    let (mut calibration, run) = started(ARDUPILOT, CalibrationKind::Accelerometer, at);
    assert!(calibration.confirmation().is_none(), "not asked yet");

    message(&mut calibration, at, &accelcal_position(1.0));
    let confirmation = calibration
        .confirmation()
        .expect("asked for the level side");
    assert_eq!(confirmation.run, run);
    assert_eq!(confirmation.side, Side::Level);
    assert_eq!(confirmation.command.command, MavCmd::AccelcalVehiclePos);
}

/// ArduPilot repeats its orientation request until the confirmation arrives;
/// the repeat must not offer Confirm again while it measures.
#[test]
fn a_repeated_orientation_request_does_not_re_offer_confirmation() {
    let at = Instant::now();
    let (mut calibration, run) = started(ARDUPILOT, CalibrationKind::Accelerometer, at);
    message(&mut calibration, at, &accelcal_position(1.0));

    calibration.apply(
        at,
        CalibrationUpdate::Confirmed {
            run,
            side: Side::Level,
        },
        None,
    );
    assert!(calibration.confirmation().is_none());

    message(&mut calibration, at, &accelcal_position(1.0));
    assert_eq!(
        instruction(&calibration),
        Instruction::Confirming(Side::Level)
    );
    assert!(!calibration.controls(DISARMED, Liveness::Active).confirm);
}

/// A failed confirmation puts the operator back on the step; the vehicle is
/// not waiting for a confirmation it never received.
#[test]
fn a_failed_confirmation_asks_again() {
    let at = Instant::now();
    let (mut calibration, run) = started(ARDUPILOT, CalibrationKind::Accelerometer, at);
    message(&mut calibration, at, &accelcal_position(1.0));
    let side = Side::Level;
    calibration.apply(at, CalibrationUpdate::Confirmed { run, side }, None);

    calibration.apply(at, CalibrationUpdate::ConfirmFailed { run, side }, None);

    assert_eq!(instruction(&calibration), Instruction::Place(side));
}

/// What a run reports lands on that run only: a confirmation of an earlier
/// run that fails, or a start that is abandoned, after a newer run began
/// leaves the newer one alone.
#[test]
fn updates_apply_to_their_own_run() {
    let at = Instant::now();
    let (mut calibration, old) = begun(ARDUPILOT, CalibrationKind::Accelerometer, at);
    let new = RunId::next();
    let kind = CalibrationKind::Accelerometer;
    calibration.apply(
        at,
        CalibrationUpdate::Begin { kind, run: new },
        Some(ARDUPILOT),
    );
    calibration.apply(
        at,
        CalibrationUpdate::Acknowledged {
            run: new,
            result: Ok(()),
        },
        None,
    );
    message(&mut calibration, at, &accelcal_position(1.0));
    let side = Side::Level;
    calibration.apply(at, CalibrationUpdate::Confirmed { run: new, side }, None);

    calibration.apply(
        at,
        CalibrationUpdate::ConfirmFailed { run: old, side },
        None,
    );
    assert_eq!(instruction(&calibration), Instruction::Confirming(side));

    assert!(
        calibration
            .apply(at, CalibrationUpdate::Abandoned { run: old }, None)
            .is_none()
    );
    assert!(calibration.is_running());

    let ended = calibration.apply(at, CalibrationUpdate::Abandoned { run: new }, None);
    assert!(failure(ended).contains("abandoned"));
}

/// A run replaced by a new one is not announced as finished.
#[test]
fn a_replaced_run_is_not_announced() {
    let at = Instant::now();
    let (mut calibration, _) = started(ARDUPILOT, CalibrationKind::Accelerometer, at);
    let kind = CalibrationKind::Compass;
    let run = RunId::next();

    let ended = calibration.apply(at, CalibrationUpdate::Begin { kind, run }, Some(ARDUPILOT));

    assert!(ended.is_none());
    assert_eq!(
        calibration.session().map(|session| session.kind),
        Some(kind)
    );
}

#[test]
fn a_firmware_without_calibration_cannot_begin_one() {
    let mut calibration = Calibration::default();
    let begin = CalibrationUpdate::Begin {
        kind: CalibrationKind::Gyroscope,
        run: RunId::next(),
    };

    calibration.apply(Instant::now(), begin, None);

    assert!(calibration.session().is_none());
}

/// A finished run can be forgotten; a running one must not be.
#[test]
fn only_a_finished_run_can_be_dismissed() {
    let at = Instant::now();
    let (mut calibration, run) = begun(ARDUPILOT, CalibrationKind::Gyroscope, at);

    calibration.apply(at, CalibrationUpdate::Dismiss, None);
    assert!(calibration.session().is_some(), "a running run stays");

    calibration.apply(
        at,
        CalibrationUpdate::Acknowledged {
            run,
            result: Ok(()),
        },
        None,
    );
    calibration.apply(at, CalibrationUpdate::Dismiss, None);
    assert!(calibration.session().is_none());
}

/// A run ends once: the update or feedback that finishes it reports its
/// end, and nothing after it does.
#[test]
fn a_run_reports_its_end_once() {
    let at = Instant::now();
    let (mut calibration, run) = begun(ARDUPILOT, CalibrationKind::Gyroscope, at);
    let ended = calibration
        .apply(
            at,
            CalibrationUpdate::Acknowledged {
                run,
                result: Ok(()),
            },
            None,
        )
        .expect("a gyroscope run completes with its acknowledgement");
    assert_eq!(ended.kind, CalibrationKind::Gyroscope);
    assert_eq!(ended.outcome, Outcome::Succeeded);
    assert!(ended.cancel.is_none());

    assert!(
        calibration
            .apply(at, CalibrationUpdate::Cancel, None)
            .is_none()
    );
    assert!(calibration.lost().is_none(), "not again on loss");
    assert!(calibration.rebooted().is_none());
    assert!(calibration.tick(after(at, 120)).is_none());

    let (mut compass, _) = started(ARDUPILOT, CalibrationKind::Compass, at);
    message(&mut compass, at, &mag_progress(10));
    let ended = message(&mut compass, at, &mag_success()).expect("the report ends the run");
    assert_eq!(ended.outcome, Outcome::Succeeded);
    assert!(message(&mut compass, at, &mag_success()).is_none());
}

/// The operator's cancel ends the run; the operation that cancelled tells
/// the vehicle itself.
#[test]
fn a_cancel_ends_the_run_without_a_command() {
    let at = Instant::now();
    let (mut calibration, _) = started(ARDUPILOT, CalibrationKind::Compass, at);
    assert_eq!(
        calibration
            .cancel_command()
            .map(|cancel| cancel.command.command),
        Some(MavCmd::DoCancelMagCal)
    );

    let ended = calibration
        .apply(at, CalibrationUpdate::Cancel, None)
        .expect("the run ends");

    assert_eq!(ended.outcome, Outcome::Cancelled);
    assert!(ended.cancel.is_none());
    assert!(
        calibration.cancel_command().is_none(),
        "nothing left to stop"
    );
}

/// A reboot ends the run on the vehicle's side too, so nothing is sent.
#[test]
fn a_reboot_ends_a_running_run_without_cancelling_it() {
    let at = Instant::now();
    let (mut calibration, _) = started(ARDUPILOT, CalibrationKind::Compass, at);
    message(&mut calibration, at, &mag_progress(10));

    let ended = calibration.rebooted();

    assert!(ended.as_ref().is_some_and(|ended| ended.cancel.is_none()));
    assert!(failure(ended).contains("rebooted"));
}

/// A compass calibration applies only after a reboot; once the autopilot has
/// rebooted, the finished run no longer waits for one.
#[test]
fn a_reboot_applies_a_run_that_waited_for_it() {
    let at = Instant::now();
    let (mut calibration, _) = started(ARDUPILOT, CalibrationKind::Compass, at);
    message(&mut calibration, at, &mag_progress(10));
    message(&mut calibration, at, &mag_success());
    let session = |calibration: &Calibration| calibration.session().cloned().expect("kept");
    assert!(session(&calibration).needs_reboot);

    assert!(
        calibration.rebooted().is_none(),
        "a finished run is not announced again"
    );

    assert_eq!(
        session(&calibration).phase,
        Phase::Succeeded,
        "still a success"
    );
    assert!(!session(&calibration).needs_reboot);
}

/// A lost vehicle may come back still calibrating: Groundhog stops the run
/// on its side too, best effort.
#[test]
fn losing_the_vehicle_ends_the_run_and_stops_it() {
    let at = Instant::now();
    let (mut calibration, _) = started(ARDUPILOT, CalibrationKind::Compass, at);
    message(&mut calibration, at, &mag_progress(10));

    let ended = calibration.lost();

    let cancel = ended.as_ref().and_then(|ended| ended.cancel.clone());
    assert_eq!(
        cancel.map(|cancel| cancel.command.command),
        Some(MavCmd::DoCancelMagCal)
    );
    assert!(failure(ended).contains("lost"));
}

#[test]
fn a_run_stalls_only_while_the_vehicle_is_to_act() {
    let at = Instant::now();
    let (mut calibration, run) = started(ARDUPILOT, CalibrationKind::Accelerometer, at);

    // The vehicle asks for the level side: the operator may take as long as
    // they need.
    message(&mut calibration, at, &accelcal_position(1.0));
    let waited = after(at, 120);
    assert!(!calibration.is_stalled(waited));
    assert!(
        calibration.tick(waited).is_none(),
        "not while waiting for the operator"
    );

    // Confirmed: the vehicle takes over, and owes progress.
    let side = Side::Level;
    calibration.apply(waited, CalibrationUpdate::Confirmed { run, side }, None);
    assert!(calibration.tick(after(waited, 60)).is_none());
    let later = after(waited, 61);
    assert!(calibration.is_stalled(later));

    let ended = calibration.tick(later);

    assert!(
        ended.as_ref().is_some_and(|ended| ended.cancel.is_none()),
        "ArduPilot cannot stop an accelerometer run"
    );
    assert!(failure(ended).contains("no progress"));
}

/// ArduPilot can repeat the same percentage for over a minute while the
/// operator turns the vehicle: that is a live run, not a stalled one.
#[test]
fn repeated_compass_progress_keeps_the_run_alive() {
    let at = Instant::now();
    let (mut calibration, _) = started(ARDUPILOT, CalibrationKind::Compass, at);

    for second in 1..=62 {
        message(&mut calibration, after(at, second), &mag_progress(87));
        assert!(
            calibration.tick(after(at, second)).is_none(),
            "running at {second} s"
        );
    }

    let ended = calibration.tick(after(at, 62 + 61));

    assert_eq!(
        ended
            .as_ref()
            .and_then(|ended| ended.cancel.as_ref())
            .map(|cancel| cancel.command.command),
        Some(MavCmd::DoCancelMagCal)
    );
    assert!(failure(ended).contains("no progress"));
}

/// ArduPilot repeats pre-arm texts while disarmed. They are not calibration
/// feedback, so a stalled run still times out.
#[test]
fn ardupilot_prearm_texts_do_not_keep_a_run_alive() {
    let at = Instant::now();
    let (mut calibration, _) = started(ARDUPILOT, CalibrationKind::Compass, at);
    message(&mut calibration, at, &mag_progress(10));

    for second in [30, 60] {
        text(
            &mut calibration,
            after(at, second),
            "PreArm: Compass not calibrated",
        );
    }

    assert!(failure(calibration.tick(after(at, 61))).contains("no progress"));
}

/// PX4 repeats the same `[cal] progress` text; that too is a live run. It
/// stops a stalled one with an empty preflight calibration, sent twice.
#[test]
fn repeated_px4_progress_keeps_the_run_alive() {
    let at = Instant::now();
    let (mut calibration, _) = started(PX4, CalibrationKind::Compass, at);

    for second in 1..=62 {
        text(&mut calibration, after(at, second), "[cal] progress <42>");
        assert!(
            calibration.tick(after(at, second)).is_none(),
            "running at {second} s"
        );
    }

    let ended = calibration.tick(after(at, 62 + 61));

    let cancel = ended
        .as_ref()
        .and_then(|ended| ended.cancel.clone())
        .expect("PX4 can stop the run");
    assert_eq!(cancel.command.command, MavCmd::PreflightCalibration);
    assert_eq!(cancel.command.param1, 0.0);
    assert_eq!(cancel.retries, 1);
    assert!(failure(ended).contains("no progress"));
}
