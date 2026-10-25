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
    let mut simulator = iced_test::Simulator::new(panel.view(&snapshot, Some(A)));
    simulator.find(text).is_ok()
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
    let ticket = panel.operations.ticket_of(&(A, Action::Start)).unwrap();
    env.step(
        &mut panel,
        Message::Finished((A, Action::Start), ticket, Ok(())),
    );

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
            .calibration
            .as_ref()
            .unwrap()
            .instruction;
        assert_eq!(instruction, Instruction::Place(side));

        env.step(&mut panel, Message::Confirm(A));
        assert!(panel.operations.is_running(&(A, Action::Confirm)));
        let ticket = panel.operations.ticket_of(&(A, Action::Confirm)).unwrap();
        env.step(
            &mut panel,
            Message::Finished((A, Action::Confirm), ticket, Ok(())),
        );
    }

    position(&mut env, SUCCESS);
    assert!(shows(&panel, &env, "Done"));
    assert!(shows(&panel, &env, "Dismiss"));
    assert!(
        !shows(&panel, &env, "In position"),
        "nothing left to confirm"
    );
}
