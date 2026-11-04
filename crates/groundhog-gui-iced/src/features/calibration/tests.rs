//! Tests of the calibration panel: a whole six-sided accelerometer run,
//! driven through the shared feature harness.

use std::time::Instant;

use groundhog_logic::{
    MavlinkId,
    domain::calibration::{CalibrationKind, Instruction, Side},
};
use mavio::{
    DefaultDialect,
    default_dialect::{enums::MavCmd, messages::CommandLong},
};

use super::*;
use crate::features::test_support::Env;

const A: SystemId = SystemId(1);
const ADDRESS: MavlinkId = MavlinkId {
    system: A.0,
    component: 1,
};

/// `ACCELCAL_VEHICLE_POS_SUCCESS`.
const SUCCESS: f32 = 16_777_215.0;

fn env() -> Env {
    let mut env = Env::new();
    env.observe_vehicle(ADDRESS, false, 0);
    env
}

/// The vehicle asks for the orientation at `position` (1 to 6), or reports
/// success.
fn position(env: &mut Env, position: f32) {
    env.observe(
        ADDRESS,
        &DefaultDialect::CommandLong(CommandLong {
            command: MavCmd::AccelcalVehiclePos,
            param1: position,
            ..Default::default()
        }),
    );
}

fn shows(panel: &CalibrationPanel, env: &Env, text: &str) -> bool {
    let snapshot = env.fleet.snapshot();
    let mut simulator = iced_test::Simulator::new(panel.view(env.view_context(&snapshot, Some(A))));
    simulator.find(text).is_ok()
}

/// The backend says when a calibration may start; the panel says why not.
#[tokio::test]
async fn an_armed_vehicle_is_asked_to_disarm_first() {
    let mut env = Env::new();
    env.observe_vehicle(ADDRESS, true, 0);
    let panel = CalibrationPanel::default();

    assert!(shows(&panel, &env, "Disarm the vehicle to calibrate."));

    env.observe_vehicle(ADDRESS, false, 0);
    assert!(!shows(&panel, &env, "Disarm the vehicle to calibrate."));
}

#[tokio::test]
async fn a_whole_accelerometer_run_walks_through_every_side() {
    let mut env = env();
    let mut panel = CalibrationPanel::default();

    env.step(
        &mut panel,
        Message::Start(A, CalibrationKind::Accelerometer),
    );
    assert!(panel.operations.is_running(&(A, Action::Start)));
    let finished = panel.operations.finished(&(A, Action::Start), Ok(()));
    env.step(&mut panel, Message::Finished(finished));

    // The service reports the run to the fleet runtime; the harness has no
    // runtime, so begin it as the service would.
    env.fleet
        .test_begin_calibration(A, CalibrationKind::Accelerometer, Instant::now());

    for (asked, side) in Side::ALL.into_iter().enumerate() {
        position(&mut env, (asked + 1) as f32);
        assert!(
            shows(&panel, &env, "In position"),
            "{side:?} asks for a confirmation"
        );
        let snapshot = env.fleet.snapshot();
        let instruction = snapshot
            .vehicle(A)
            .unwrap()
            .calibration()
            .unwrap()
            .instruction;
        assert_eq!(instruction, Instruction::Place(side));

        env.step(&mut panel, Message::Confirm(A));
        assert!(panel.operations.is_running(&(A, Action::Confirm)));
        let finished = panel.operations.finished(&(A, Action::Confirm), Ok(()));
        env.step(&mut panel, Message::Finished(finished));
    }

    position(&mut env, SUCCESS);
    assert!(shows(&panel, &env, "Done"));
    assert!(shows(&panel, &env, "Dismiss"));
    assert!(
        !shows(&panel, &env, "In position"),
        "nothing left to confirm"
    );
}

/// A compass run on ArduPilot applies only after a reboot: the panel offers
/// one, asks before sending it, and stops offering it once the autopilot has
/// rebooted.
#[tokio::test]
async fn a_calibration_that_waits_for_a_reboot_offers_one() {
    use mavio::default_dialect::{
        enums::MagCalStatus,
        messages::{Attitude, MagCalProgress, MagCalReport},
    };

    let mut env = env();
    let mut panel = CalibrationPanel::default();
    env.fleet
        .test_begin_calibration(A, CalibrationKind::Compass, Instant::now());
    env.observe(
        ADDRESS,
        &DefaultDialect::MagCalProgress(MagCalProgress {
            compass_id: 0,
            completion_pct: 10,
            cal_status: MagCalStatus::MagCalRunningStepOne,
            ..Default::default()
        }),
    );
    env.observe(
        ADDRESS,
        &DefaultDialect::MagCalReport(MagCalReport {
            compass_id: 0,
            cal_status: MagCalStatus::MagCalSuccess,
            ..Default::default()
        }),
    );
    assert!(shows(&panel, &env, "Done"));
    assert!(shows(&panel, &env, "Reboot now"));

    let effects = env.step(&mut panel, Message::RequestReboot(A));
    assert!(
        matches!(effects.as_slice(), [crate::features::Effect::Confirm(confirmation)]
            if matches!(confirmation.accept, Message::Reboot(A))),
        "a confirmation that leads to the reboot"
    );
    assert!(
        !panel.operations.is_running(&(A, Action::Reboot)),
        "nothing is sent before it is accepted"
    );

    env.step(&mut panel, Message::Reboot(A));
    assert!(panel.operations.is_running(&(A, Action::Reboot)));

    for time_boot_ms in [600_000, 1_500] {
        env.observe(
            ADDRESS,
            &DefaultDialect::Attitude(Attitude {
                time_boot_ms,
                ..Default::default()
            }),
        );
    }
    assert!(shows(&panel, &env, "Done"), "the run still succeeded");
    assert!(
        !shows(&panel, &env, "Reboot now"),
        "the calibration is applied"
    );
}

/// A start that fails after the session began is told by the session; one
/// refused before there is a session is announced.
#[tokio::test]
async fn only_a_start_refused_before_the_session_is_announced() {
    let mut env = env();
    let mut panel = CalibrationPanel::default();
    let start = Message::Start(A, CalibrationKind::Gyroscope);

    env.step(&mut panel, start.clone());
    let finished = panel
        .operations
        .finished(&(A, Action::Start), Err(Error::Timeout));
    env.step(&mut panel, Message::Finished(finished));
    assert_eq!(env.notices.history().count(), 0, "the session says it");

    env.step(&mut panel, start);
    let blocked =
        Error::CalibrationBlocked(groundhog_logic::domain::calibration::StartBlocked::NotDisarmed);
    let finished = panel.operations.finished(&(A, Action::Start), Err(blocked));
    env.step(&mut panel, Message::Finished(finished));
    let notice = env.notices.history().last().expect("announced");
    let name = env.fleet.snapshot().vehicle_name(A);
    assert_eq!(
        notice.title,
        format!("Starting the calibration on {name} failed")
    );
}
