//! Tests of the vehicles panel's commands: which ask the operator first,
//! driven through a [`Context`] over a synthetic fleet.

use groundhog_logic::MavlinkId;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavAutopilot, MavModeFlag, MavType},
        messages::{Heartbeat, SysStatus, VfrHud},
    },
};

use super::*;
use crate::features::test_support::Env;

const A: SystemId = SystemId(1);

const ADDRESS: MavlinkId = MavlinkId {
    system: A.0,
    component: 1,
};

/// An ArduPilot copter in Stabilize, armed or not.
fn env(armed: bool) -> Env {
    let mut env = Env::new();
    env.observe_vehicle(ADDRESS, armed, 0);
    env
}

impl Env {
    fn mode(&self, name: &str) -> FlightMode {
        let snapshot = self.fleet.snapshot();
        let vehicle = snapshot.vehicle(A).expect("the vehicle is registered");
        vehicle
            .modes()
            .find(|mode| mode.name == name)
            .expect("the mode exists")
    }
}

fn changing_mode(panel: &VehiclePanel) -> bool {
    panel.commands.is_running(&(A, Command::SetMode))
}

#[tokio::test]
async fn an_armed_vehicle_switches_mode_only_once_confirmed() {
    let mut env = env(true);
    let mut panel = VehiclePanel::default();
    let acro = env.mode("Acro");

    let effects = env.step(&mut panel, Message::RequestMode(A, acro));

    assert!(
        matches!(effects.as_slice(), [Effect::Confirm(confirmation)]
            if matches!(confirmation.accept, Message::SetMode(A, mode) if mode == acro)),
        "a confirmation that leads to the switch"
    );
    assert!(
        !changing_mode(&panel),
        "nothing is sent before it is accepted"
    );
}

#[tokio::test]
async fn a_disarmed_vehicle_switches_mode_at_once() {
    let mut env = env(false);
    let mut panel = VehiclePanel::default();
    let acro = env.mode("Acro");

    let effects = env.step(&mut panel, Message::RequestMode(A, acro));

    assert!(effects.is_empty(), "no confirmation");
    assert!(changing_mode(&panel));
}

#[tokio::test]
async fn a_vehicle_of_unknown_arming_asks_first() {
    let mut env = env(false);
    let mut panel = VehiclePanel::default();
    let acro = env.mode("Acro");
    let unknown = SystemId(7);

    let effects = env.step(&mut panel, Message::RequestMode(unknown, acro));

    assert!(matches!(effects.as_slice(), [Effect::Confirm(_)]));
}

#[tokio::test]
async fn return_asks_only_while_armed() {
    let mut panel = VehiclePanel::default();

    let mut armed = env(true);
    let effects = armed.step(
        &mut panel,
        Message::RequestModeKind(A, ModeKind::ReturnToLaunch),
    );
    assert!(matches!(effects.as_slice(), [Effect::Confirm(_)]));
    assert!(!changing_mode(&panel));

    let mut disarmed = env(false);
    let effects = disarmed.step(
        &mut panel,
        Message::RequestModeKind(A, ModeKind::ReturnToLaunch),
    );
    assert!(effects.is_empty(), "no confirmation");
    assert!(changing_mode(&panel));
}

#[tokio::test]
async fn choosing_the_current_mode_does_nothing() {
    let mut env = env(true);
    let mut panel = VehiclePanel::default();
    let stabilize = env.mode("Stabilize");

    let effects = env.step(&mut panel, Message::RequestMode(A, stabilize));

    assert!(effects.is_empty());
    assert!(!changing_mode(&panel));
}

#[tokio::test]
async fn a_reboot_is_sent_only_once_confirmed() {
    let mut env = env(false);
    let mut panel = VehiclePanel::default();

    let effects = env.step(&mut panel, Message::RequestReboot(A));

    assert!(
        matches!(effects.as_slice(), [Effect::Confirm(confirmation)]
            if matches!(confirmation.accept, Message::Reboot(A))),
        "a confirmation that leads to the reboot"
    );
    assert!(!panel.commands.is_running(&(A, Command::Reboot)));

    env.step(&mut panel, Message::Reboot(A));
    assert!(panel.commands.is_running(&(A, Command::Reboot)));
}

/// Whether the panel of the vehicle at `ADDRESS` shows `text`.
fn shows(env: &Env, text: &str) -> bool {
    let panel = VehiclePanel::default();
    let snapshot = env.fleet.snapshot();
    let mut simulator = iced_test::Simulator::new(panel.view(env.view_context(&snapshot, Some(A))));
    simulator.find(text).is_ok()
}

/// A vehicle of `type_` that reported its airspeed and throttle, and has an
/// airspeed sensor in use or not.
fn reporting(type_: MavType, sensor: bool) -> Env {
    use mavio::default_dialect::enums::MavSysStatusSensor as F;

    let mut env = Env::new();
    env.observe(
        ADDRESS,
        &DefaultDialect::Heartbeat(Heartbeat {
            type_,
            autopilot: MavAutopilot::Ardupilotmega,
            base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
            ..Default::default()
        }),
    );
    let sensors = if sensor {
        F::DIFFERENTIAL_PRESSURE
    } else {
        F::empty()
    };
    env.observe(
        ADDRESS,
        &DefaultDialect::SysStatus(SysStatus {
            onboard_control_sensors_present: sensors,
            onboard_control_sensors_enabled: sensors,
            onboard_control_sensors_health: sensors,
            ..Default::default()
        }),
    );
    env.observe(
        ADDRESS,
        &DefaultDialect::VfrHud(VfrHud {
            airspeed: 17.4,
            throttle: 64,
            ..Default::default()
        }),
    );
    env
}

#[tokio::test]
async fn a_plane_shows_its_airspeed_and_whether_it_is_estimated() {
    let measured = reporting(MavType::FixedWing, true);
    assert!(shows(&measured, "Airspeed"));
    assert!(shows(&measured, "17.4 m/s"));
    assert!(shows(&measured, "64 %"), "the throttle");

    let estimated = reporting(MavType::FixedWing, false);
    assert!(shows(&estimated, "Airspeed (est.)"));
    assert!(shows(&estimated, "17.4 m/s"));
}

#[tokio::test]
async fn a_copter_shows_its_throttle_but_no_airspeed() {
    // Without a sensor, whose health badge would read "Airspeed".
    let copter = reporting(MavType::Quadrotor, false);
    assert!(shows(&copter, "64 %"));
    assert!(!shows(&copter, "Airspeed"));
    assert!(!shows(&copter, "Airspeed (est.)"));
    assert!(!shows(&copter, "17.4 m/s"));
}

/// A failed command is announced, naming what failed and the vehicle.
#[tokio::test]
async fn a_failed_command_is_announced() {
    let mut env = env(false);
    let mut panel = VehiclePanel::default();
    env.step(&mut panel, Message::Arm(A, true));
    let finished = panel.commands.finished(
        &(A, Command::Arm),
        Err(groundhog_logic::services::Error::Timeout),
    );

    env.step(&mut panel, Message::Finished(finished));

    let notice = env.notices.history().last().expect("a notice");
    let name = env.fleet.snapshot().vehicle_name(A);
    assert_eq!(notice.title, format!("Arming on {name} failed"));
    assert_eq!(notice.system, Some(A));
    assert!(!panel.commands.is_running(&(A, Command::Arm)));
}
