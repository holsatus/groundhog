//! Tests of the vehicles panel's commands: which ask the operator first,
//! driven through a [`Context`] over a synthetic fleet.

use std::time::Instant;

use groundhog_logic::{
    Backend, BackendOptions, MavlinkId, domain::fleet::Fleet, services::FleetHandle,
};
use mavio::default_dialect::{
    enums::{MavAutopilot, MavModeFlag, MavType},
    messages,
};

use super::*;
use crate::{config::Settings, features::notifications::Notifications};

const A: SystemId = SystemId(1);

struct Env {
    fleet: Fleet,
    handle: FleetHandle,
    notices: Notifications,
    settings: Settings,
}

impl Env {
    /// An ArduPilot copter in Stabilize, armed or not.
    fn new(armed: bool) -> Self {
        let (backend, _events) = Backend::start(BackendOptions {
            bootstrap: false,
            ..BackendOptions::default()
        });
        let handle = backend.fleet().clone();

        let mut base_mode = MavModeFlag::CUSTOM_MODE_ENABLED;
        if armed {
            base_mode |= MavModeFlag::SAFETY_ARMED;
        }
        let mut fleet = Fleet::default();
        fleet.observe(
            MavlinkId {
                system: A.0,
                component: 1,
            },
            Instant::now(),
            Some(&mavio::DefaultDialect::Heartbeat(messages::Heartbeat {
                type_: MavType::Quadrotor,
                autopilot: MavAutopilot::Ardupilotmega,
                base_mode,
                custom_mode: 0,
                ..Default::default()
            })),
        );

        Self {
            fleet,
            handle,
            notices: Notifications::default(),
            settings: Settings::default(),
        }
    }

    fn step(&mut self, panel: &mut VehiclePanel, message: Message) -> Vec<Effect<Message>> {
        let snapshot = self.fleet.snapshot();
        let mut cx = Context::new(
            &snapshot,
            &self.handle,
            &mut self.notices,
            None,
            &self.settings,
        );
        let _task = panel.update(message, &mut cx);
        cx.into_effects()
    }

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
    let mut env = Env::new(true);
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
    let mut env = Env::new(false);
    let mut panel = VehiclePanel::default();
    let acro = env.mode("Acro");

    let effects = env.step(&mut panel, Message::RequestMode(A, acro));

    assert!(effects.is_empty(), "no confirmation");
    assert!(changing_mode(&panel));
}

#[tokio::test]
async fn a_vehicle_of_unknown_arming_asks_first() {
    let mut env = Env::new(false);
    let mut panel = VehiclePanel::default();
    let acro = env.mode("Acro");
    let unknown = SystemId(7);

    let effects = env.step(&mut panel, Message::RequestMode(unknown, acro));

    assert!(matches!(effects.as_slice(), [Effect::Confirm(_)]));
}

#[tokio::test]
async fn return_asks_only_while_armed() {
    let mut panel = VehiclePanel::default();

    let mut armed = Env::new(true);
    let effects = armed.step(
        &mut panel,
        Message::RequestModeKind(A, ModeKind::ReturnToLaunch),
    );
    assert!(matches!(effects.as_slice(), [Effect::Confirm(_)]));
    assert!(!changing_mode(&panel));

    let mut disarmed = Env::new(false);
    let effects = disarmed.step(
        &mut panel,
        Message::RequestModeKind(A, ModeKind::ReturnToLaunch),
    );
    assert!(effects.is_empty(), "no confirmation");
    assert!(changing_mode(&panel));
}

#[tokio::test]
async fn choosing_the_current_mode_does_nothing() {
    let mut env = Env::new(true);
    let mut panel = VehiclePanel::default();
    let stabilize = env.mode("Stabilize");

    let effects = env.step(&mut panel, Message::RequestMode(A, stabilize));

    assert!(effects.is_empty());
    assert!(!changing_mode(&panel));
}
