//! Tests of the mission editor: drafts, transfers and their confirmations,
//! driven through a [`Context`] over a synthetic fleet.

use std::time::Instant;

use groundhog_logic::{
    Backend, BackendOptions, MavlinkId,
    domain::{
        fleet::Fleet,
        mission::{MissionItem, Position},
    },
    services::FleetHandle,
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
    fn new() -> Self {
        let (backend, _events) = Backend::start(BackendOptions {
            bootstrap: false,
            ..BackendOptions::default()
        });
        let handle = backend.fleet().clone();

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
                base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
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

    /// Put the vehicle on the ground at a known spot (altitude zero).
    fn place_vehicle(&mut self) {
        self.fleet.observe(
            MavlinkId {
                system: A.0,
                component: 1,
            },
            Instant::now(),
            Some(&mavio::DefaultDialect::GlobalPositionInt(
                messages::GlobalPositionInt {
                    lat: 474_000_000,
                    lon: 85_000_000,
                    relative_alt: 0,
                    ..Default::default()
                },
            )),
        );
    }

    fn step(
        &mut self,
        editor: &mut MissionEditor,
        plan: PlanKey,
        kind: MessageKind,
    ) -> Vec<Effect<Message>> {
        let snapshot = self.fleet.snapshot();
        let mut cx = Context::new(
            &snapshot,
            &self.handle,
            &mut self.notices,
            None,
            &self.settings,
        );
        let _task = editor.update(Message { plan, kind }, &mut cx);
        cx.into_effects()
    }

    fn set_onboard(&mut self, editor: &mut MissionEditor, items: Vec<MissionItem>, opaque_id: u32) {
        self.fleet.set_onboard_mission(
            A,
            OnboardMission {
                items,
                home: None,
                opaque_id,
            },
        );
        editor.sync(&self.fleet.snapshot());
    }
}

fn draft(editor: &MissionEditor, plan: PlanKey) -> &Draft {
    editor.drafts.get(&plan).expect("draft exists")
}

/// Place a waypoint on the map at `lat`, as the operator clicks it there.
fn place(env: &mut Env, editor: &mut MissionEditor, plan: PlanKey, lat: f64) {
    if !editor.drafts.get(&plan).is_some_and(|draft| draft.placing) {
        env.step(editor, plan, MessageKind::TogglePlacing);
    }
    env.step(
        editor,
        plan,
        MessageKind::PlaceAt(GeoPosition::new(lat, 8.0)),
    );
}

fn waypoint(lat: f64) -> MissionItem {
    MissionItem::waypoint(Position {
        lat,
        lon: 8.0,
        alt_m: 40.0,
    })
}

#[tokio::test]
async fn a_first_waypoint_with_nowhere_to_go_asks_to_be_placed() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());

    env.step(&mut editor, None, MessageKind::AddWaypoint);

    let offline = draft(&editor, None);
    assert!(offline.plan().is_empty(), "nothing at 0, 0");
    assert!(offline.placing, "the map takes the next click");
    assert_eq!(
        env.notices.history().last().unwrap().level,
        crate::features::notifications::Level::Info
    );
}

#[tokio::test]
async fn a_command_that_needs_a_location_the_plan_lacks_is_refused() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());
    editor
        .draft_mut(None)
        .plan_mut()
        .push(MissionItem::new(MavCmd::NavDelay, None));
    let delay = draft(&editor, None).plan().items()[0].id();

    env.step(
        &mut editor,
        None,
        MessageKind::ChangeCommand(delay, MavCmd::NavWaypoint as u16),
    );

    let item = draft(&editor, None).plan().get(delay).unwrap();
    assert_eq!(item.known_command(), Some(MavCmd::NavDelay), "unchanged");
    assert_eq!(
        env.notices.history().last().unwrap().level,
        crate::features::notifications::Level::Warning
    );
}

#[tokio::test]
async fn new_waypoints_never_take_the_vehicles_ground_altitude() {
    let mut env = Env::new();
    env.place_vehicle();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());

    env.step(&mut editor, Some(A), MessageKind::AddWaypoint);

    let plan = &draft(&editor, Some(A)).plan();
    let position = plan.items()[0].item.position().unwrap();
    assert!((position.lat - 47.4).abs() < 1e-6, "placed at the vehicle");
    assert_eq!(
        position.alt_m, env.settings.default_waypoint_altitude_m,
        "at the default altitude, not the vehicle's zero"
    );

    // Further waypoints take the previous waypoint's altitude.
    let first = plan.items()[0].id();
    env.step(
        &mut editor,
        Some(A),
        MessageKind::EditField(first, Field::Altitude, "120".into()),
    );
    env.step(&mut editor, Some(A), MessageKind::TogglePlacing);
    env.step(
        &mut editor,
        Some(A),
        MessageKind::PlaceAt(GeoPosition::new(47.5, 8.5)),
    );
    let plan = &draft(&editor, Some(A)).plan();
    assert_eq!(plan.items()[1].item.position().unwrap().alt_m, 120.0);
}

#[tokio::test]
async fn map_clicks_only_place_while_placing() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());

    env.step(
        &mut editor,
        Some(A),
        MessageKind::PlaceAt(GeoPosition::new(1.0, 1.0)),
    );
    assert!(draft(&editor, Some(A)).plan().is_empty());

    env.step(&mut editor, Some(A), MessageKind::TogglePlacing);
    env.step(
        &mut editor,
        Some(A),
        MessageKind::PlaceAt(GeoPosition::new(1.0, 1.0)),
    );
    assert_eq!(draft(&editor, Some(A)).plan().len(), 1);
}

#[tokio::test]
async fn an_unmodified_draft_follows_the_onboard_mission() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());
    assert_eq!(
        editor.status(Some(A), &env.fleet.snapshot()),
        SyncStatus::Unknown
    );

    env.set_onboard(&mut editor, vec![waypoint(1.0)], 1);
    assert!(draft(&editor, Some(A)).plan().matches(&[waypoint(1.0)]));
    assert_eq!(
        editor.status(Some(A), &env.fleet.snapshot()),
        SyncStatus::Synced
    );

    // Another GCS uploads a different mission: still unmodified here, so the
    // draft follows.
    env.set_onboard(&mut editor, vec![waypoint(2.0), waypoint(3.0)], 2);
    assert_eq!(draft(&editor, Some(A)).plan().len(), 2);
}

#[tokio::test]
async fn a_modified_draft_is_never_overwritten() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    env.set_onboard(&mut editor, vec![waypoint(1.0)], 1);

    env.step(&mut editor, Some(A), MessageKind::AddWaypoint);
    assert_eq!(
        editor.status(Some(A), &env.fleet.snapshot()),
        SyncStatus::Modified
    );

    env.set_onboard(&mut editor, vec![waypoint(9.0)], 2);
    let plan = &draft(&editor, Some(A)).plan();
    assert_eq!(plan.len(), 2, "the operator's edit survives");
    assert_eq!(
        editor.status(Some(A), &env.fleet.snapshot()),
        SyncStatus::Modified
    );
}

#[tokio::test]
async fn downloading_over_changes_asks_first() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());
    place(&mut env, &mut editor, Some(A), 1.0);

    let effects = env.step(
        &mut editor,
        Some(A),
        MessageKind::Download { confirmed: false },
    );
    assert!(
        matches!(effects.as_slice(), [Effect::Confirm(confirmation)]
            if matches!(confirmation.accept.kind, MessageKind::Download { confirmed: true })),
        "a confirmation that leads to the real download"
    );
    assert!(!editor.transferring(A), "nothing started yet");

    env.step(
        &mut editor,
        Some(A),
        MessageKind::Download { confirmed: true },
    );
    assert!(editor.transferring(A));
}

#[tokio::test]
async fn edits_wait_for_a_running_transfer() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());

    env.step(
        &mut editor,
        Some(A),
        MessageKind::Download { confirmed: false },
    );
    assert!(editor.transferring(A));

    place(&mut env, &mut editor, Some(A), 1.0);
    assert!(
        draft(&editor, Some(A)).plan().is_empty(),
        "refused during the download"
    );

    // The offline plan is unaffected by a vehicle's transfer.
    place(&mut env, &mut editor, None, 1.0);
    assert_eq!(draft(&editor, None).plan().len(), 1);
}

#[tokio::test]
async fn a_finished_download_replaces_the_draft_and_a_stale_one_is_ignored() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());

    env.step(
        &mut editor,
        Some(A),
        MessageKind::Download { confirmed: true },
    );
    let ticket = editor
        .operations
        .ticket_of(&(A, Operation::Download))
        .unwrap();

    let mission = OnboardMission {
        items: vec![waypoint(5.0)],
        home: None,
        opaque_id: 3,
    };
    env.step(
        &mut editor,
        Some(A),
        MessageKind::Finished(Operation::Download, ticket, Ok(Some(mission.clone()))),
    );
    assert!(draft(&editor, Some(A)).plan().matches(&mission.items));
    assert!(!editor.transferring(A));

    // The same result again (a stale ticket) changes nothing.
    env.step(&mut editor, Some(A), MessageKind::Discard);
    env.step(
        &mut editor,
        Some(A),
        MessageKind::Finished(Operation::Download, ticket, Ok(Some(mission))),
    );
    assert!(draft(&editor, Some(A)).plan().is_empty());
}

#[tokio::test]
async fn uploading_an_empty_plan_asks_first() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    env.set_onboard(&mut editor, vec![waypoint(1.0)], 1);
    env.step(&mut editor, Some(A), MessageKind::Discard);

    let effects = env.step(
        &mut editor,
        Some(A),
        MessageKind::Upload { confirmed: false },
    );
    assert!(
        matches!(effects.as_slice(), [Effect::Confirm(confirmation)]
            if confirmation.body.contains("deletes the vehicle's mission")),
        "an empty upload says what it deletes"
    );
    assert!(!editor.transferring(A), "nothing sent before the answer");
}

#[tokio::test]
async fn starting_says_when_the_vehicle_flies_another_mission() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    env.set_onboard(&mut editor, vec![waypoint(1.0)], 1);
    place(&mut env, &mut editor, Some(A), 2.0);

    let effects = env.step(
        &mut editor,
        Some(A),
        MessageKind::Start { confirmed: false },
    );
    assert!(
        matches!(effects.as_slice(), [Effect::Confirm(confirmation)]
            if confirmation.body.contains("not the plan shown")),
        "the confirmation says the plan shown is not what flies"
    );
    assert!(!editor.operations.is_running(&(A, Operation::Start)));
}

#[tokio::test]
async fn resuming_asks_and_pausing_does_not() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    env.set_onboard(&mut editor, vec![waypoint(1.0)], 1);

    let effects = env.step(
        &mut editor,
        Some(A),
        MessageKind::Resume { confirmed: false },
    );
    assert!(matches!(effects.as_slice(), [Effect::Confirm(_)]));
    assert!(!editor.operations.is_running(&(A, Operation::Resume)));

    let effects = env.step(&mut editor, Some(A), MessageKind::Pause);
    assert!(effects.is_empty(), "a safety stop never asks");
    assert!(editor.operations.is_running(&(A, Operation::Pause)));
}

#[tokio::test]
async fn failures_are_shown_to_the_operator() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());

    // An empty plan asks first; this one was confirmed.
    env.step(
        &mut editor,
        Some(A),
        MessageKind::Upload { confirmed: true },
    );
    let ticket = editor
        .operations
        .ticket_of(&(A, Operation::Upload))
        .unwrap();
    env.step(
        &mut editor,
        Some(A),
        MessageKind::Finished(
            Operation::Upload,
            ticket,
            Err(groundhog_logic::services::Error::Timeout),
        ),
    );

    let notice = env.notices.history().last().expect("a notice");
    assert_eq!(notice.level, crate::features::notifications::Level::Error);
    assert!(notice.title.contains("upload"), "{}", notice.title);
}

#[tokio::test]
async fn drags_address_items_by_id() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());
    place(&mut env, &mut editor, None, 1.0);
    place(&mut env, &mut editor, None, 2.0);

    let first = draft(&editor, None).plan().items()[0].id();
    env.step(&mut editor, None, MessageKind::MoveDown(first));
    env.step(
        &mut editor,
        None,
        MessageKind::Drop(first, GeoPosition::new(10.0, 20.0)),
    );

    let plan = &draft(&editor, None).plan();
    assert_eq!(plan.index_of(first), Some(1));
    let moved = plan.get(first).unwrap().position().unwrap();
    assert!(
        (moved.lat - 10.0).abs() < 1e-6,
        "the moved item, not whatever is first"
    );

    // An id from before the plan was replaced resolves to nothing.
    env.step(&mut editor, None, MessageKind::Discard);
    place(&mut env, &mut editor, None, 1.0);
    env.step(
        &mut editor,
        None,
        MessageKind::Drop(first, GeoPosition::new(-5.0, -5.0)),
    );
    let plan = &draft(&editor, None).plan();
    assert!(plan.items()[0].item.position().unwrap().lat > -1.0);
}

#[tokio::test]
async fn jumping_needs_a_synced_plan_and_a_confirmation() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    env.set_onboard(&mut editor, vec![waypoint(1.0), waypoint(2.0)], 1);
    let second = draft(&editor, Some(A)).plan().items()[1].id();

    let effects = env.step(
        &mut editor,
        Some(A),
        MessageKind::SetCurrent {
            item: second,
            confirmed: false,
        },
    );
    assert!(matches!(effects.as_slice(), [Effect::Confirm(_)]));

    // Once the plan is modified, item numbers no longer match the vehicle.
    env.step(&mut editor, Some(A), MessageKind::AddWaypoint);
    let effects = env.step(
        &mut editor,
        Some(A),
        MessageKind::SetCurrent {
            item: second,
            confirmed: true,
        },
    );
    assert!(effects.is_empty());
    assert!(!editor.operations.is_running(&(A, Operation::SetCurrent)));
    assert_eq!(
        env.notices.history().last().unwrap().level,
        crate::features::notifications::Level::Warning
    );
}

#[tokio::test]
async fn clearing_the_vehicle_always_asks() {
    let mut env = Env::new();
    let mut editor = MissionEditor::default();
    editor.sync(&env.fleet.snapshot());

    let effects = env.step(
        &mut editor,
        Some(A),
        MessageKind::Clear { confirmed: false },
    );
    assert!(matches!(effects.as_slice(), [Effect::Confirm(confirmation)] if confirmation.danger));
}
