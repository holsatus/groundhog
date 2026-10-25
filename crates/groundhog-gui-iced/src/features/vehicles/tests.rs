//! Tests of the vehicles panel's commands: which ask the operator first,
//! driven through a [`Context`] over a synthetic fleet.

use groundhog_logic::MavlinkId;

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
