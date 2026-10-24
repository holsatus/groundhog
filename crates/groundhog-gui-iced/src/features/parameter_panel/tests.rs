//! Tests of the parameter panel: edits, uploads, downloads and files,
//! driven through a [`Context`] over a synthetic fleet.

use std::time::Instant;

use groundhog_logic::{Backend, BackendOptions, domain::fleet::Fleet, services::FleetHandle};
use mavio::default_dialect::{
    enums::{MavAutopilot, MavModeFlag, MavParamType, MavType},
    messages,
};

use super::*;
use crate::{
    config::Settings,
    features::notifications::{Level, Notifications},
};

const ADDRESS: MavlinkId = MavlinkId {
    system: 1,
    component: 1,
};

fn ident(name: &str) -> Ident {
    Ident::from_str_truncated(name)
}

struct Env {
    fleet: Fleet,
    handle: FleetHandle,
    notices: Notifications,
    settings: Settings,
}

impl Env {
    /// An ArduPilot vehicle reporting `params` (all `REAL32`).
    fn new(params: &[(&str, f32)]) -> Self {
        let (backend, _events) = Backend::start(BackendOptions {
            bootstrap: false,
            ..BackendOptions::default()
        });
        let handle = backend.fleet().clone();

        let mut fleet = Fleet::default();
        let observe = |fleet: &mut Fleet, message| {
            fleet.observe(ADDRESS, Instant::now(), Some(&message));
        };

        observe(
            &mut fleet,
            mavio::DefaultDialect::Heartbeat(messages::Heartbeat {
                type_: MavType::Quadrotor,
                autopilot: MavAutopilot::Ardupilotmega,
                base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
                ..Default::default()
            }),
        );

        for (index, (name, value)) in params.iter().enumerate() {
            let mut param_id = [0u8; 16];
            param_id[..name.len()].copy_from_slice(name.as_bytes());
            observe(
                &mut fleet,
                mavio::DefaultDialect::ParamValue(messages::ParamValue {
                    param_id,
                    param_value: *value,
                    param_type: MavParamType::Real32,
                    param_count: params.len() as u16,
                    param_index: index as u16,
                }),
            );
        }

        Self {
            fleet,
            handle,
            notices: Notifications::default(),
            settings: Settings::default(),
        }
    }

    fn step(&mut self, panel: &mut ParameterPanel, message: Message) -> Vec<Effect<Message>> {
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
}

#[tokio::test]
async fn unparseable_text_is_kept_but_not_uploadable() {
    let mut env = Env::new(&[("GAIN", 1.0)]);
    let mut panel = ParameterPanel::default();

    env.step(
        &mut panel,
        Message::Edit(ADDRESS, ident("GAIN"), "1.2.3".into()),
    );
    let edit = panel.edit(ADDRESS, &ident("GAIN")).unwrap();
    assert_eq!(edit.text, "1.2.3");
    assert_eq!(edit.uploadable(), None);

    env.step(&mut panel, Message::Upload(ADDRESS, ident("GAIN")));
    assert!(!panel.uploads.is_running(&(ADDRESS, ident("GAIN"))));
}

#[tokio::test]
async fn upload_results_fold_into_the_edit() {
    let mut env = Env::new(&[("GAIN", 1.0)]);
    let mut panel = ParameterPanel::default();
    let key = (ADDRESS, ident("GAIN"));

    env.step(
        &mut panel,
        Message::Edit(ADDRESS, ident("GAIN"), "5".into()),
    );
    env.step(&mut panel, Message::Upload(ADDRESS, ident("GAIN")));
    assert!(panel.edit(ADDRESS, &ident("GAIN")).unwrap().is_uploading());

    // The vehicle clamps the value: the edit stays, marked rejected.
    let ticket = panel.uploads.ticket_of(&key).unwrap();
    env.step(
        &mut panel,
        Message::Uploaded(key.clone(), ticket, Value::F32(5.0), Ok(Value::F32(3.0))),
    );
    assert_eq!(
        panel.edit(ADDRESS, &ident("GAIN")).unwrap().status,
        EditStatus::Rejected {
            reported: Value::F32(3.0)
        }
    );
    assert_eq!(env.notices.history().last().unwrap().level, Level::Warning);

    // Retried and accepted: the edit is done.
    env.step(&mut panel, Message::Upload(ADDRESS, ident("GAIN")));
    let ticket = panel.uploads.ticket_of(&key).unwrap();
    env.step(
        &mut panel,
        Message::Uploaded(key, ticket, Value::F32(5.0), Ok(Value::F32(5.0))),
    );
    assert!(panel.edit(ADDRESS, &ident("GAIN")).is_none());
}

#[tokio::test]
async fn editing_supersedes_an_upload_in_flight() {
    let mut env = Env::new(&[("GAIN", 1.0)]);
    let mut panel = ParameterPanel::default();
    let key = (ADDRESS, ident("GAIN"));

    env.step(
        &mut panel,
        Message::Edit(ADDRESS, ident("GAIN"), "5".into()),
    );
    env.step(&mut panel, Message::Upload(ADDRESS, ident("GAIN")));
    let ticket = panel.uploads.ticket_of(&key).unwrap();

    env.step(
        &mut panel,
        Message::Edit(ADDRESS, ident("GAIN"), "6".into()),
    );
    assert!(!panel.uploads.is_running(&key), "the upload was cancelled");

    // The cancelled upload's late result is ignored.
    env.step(
        &mut panel,
        Message::Uploaded(key, ticket, Value::F32(5.0), Ok(Value::F32(5.0))),
    );
    let edit = panel.edit(ADDRESS, &ident("GAIN")).unwrap();
    assert_eq!(edit.value, Some(Value::F32(6.0)));
    assert_eq!(edit.status, EditStatus::Pending);
}

#[tokio::test]
async fn a_loaded_file_becomes_edits_and_a_notice() {
    let mut env = Env::new(&[("GAIN", 1.0), ("RATE", 2.0)]);
    let mut panel = ParameterPanel::default();

    let mut loaded = ParamValues::new();
    loaded.insert(ident("GAIN"), Value::F32(1.0)); // unchanged
    loaded.insert(ident("RATE"), Value::F32(4.0)); // changed
    loaded.insert(ident("MISSING"), Value::F32(1.0)); // unknown

    let path = PathBuf::from("params.param");
    let effects = env.step(&mut panel, Message::Loaded(path, ADDRESS, Ok(loaded)));

    assert_eq!(
        panel.edit(ADDRESS, &ident("RATE")).unwrap().value,
        Some(Value::F32(4.0))
    );
    assert!(panel.edit(ADDRESS, &ident("GAIN")).is_none());

    let notice = env.notices.history().last().unwrap();
    assert_eq!(notice.level, Level::Success);
    assert_eq!(notice.title, "Loaded 1 changed parameters");
    assert!(matches!(effects.as_slice(), [Effect::RememberDirectory(_)]));
}

#[tokio::test]
async fn upload_all_starts_every_ready_edit() {
    let mut env = Env::new(&[("A_ONE", 1.0), ("A_TWO", 2.0), ("A_THREE", 3.0)]);
    let mut panel = ParameterPanel::default();

    env.step(
        &mut panel,
        Message::Edit(ADDRESS, ident("A_ONE"), "10".into()),
    );
    env.step(
        &mut panel,
        Message::Edit(ADDRESS, ident("A_TWO"), "oops".into()),
    );
    env.step(&mut panel, Message::UploadAll(ADDRESS));

    assert!(panel.uploads.is_running(&(ADDRESS, ident("A_ONE"))));
    assert!(
        !panel.uploads.is_running(&(ADDRESS, ident("A_TWO"))),
        "unparseable"
    );
}

#[test]
fn the_filter_matches_case_insensitively() {
    let filter = ParameterFilter {
        ident_pattern: "gai".into(),
        only_changed: false,
    };
    assert!(filter.matches(&ident("BATT_GAIN"), false, "gai"));
    assert!(!filter.matches(&ident("RATE"), false, "gai"));

    let changed = ParameterFilter {
        ident_pattern: String::new(),
        only_changed: true,
    };
    assert!(!changed.matches(&ident("RATE"), false, ""));
    assert!(changed.matches(&ident("RATE"), true, ""));
}

#[tokio::test]
async fn a_cancelled_download_ignores_its_late_result() {
    let mut env = Env::new(&[("GAIN", 1.0)]);
    let mut panel = ParameterPanel::default();
    env.step(&mut panel, Message::Reload(ADDRESS));
    let ticket = panel.reloads.ticket_of(&ADDRESS).unwrap();

    env.step(&mut panel, Message::CancelReload(ADDRESS));
    assert!(!panel.reloads.is_running(&ADDRESS));
    assert_eq!(env.notices.history().last().unwrap().level, Level::Info);

    let summary = ParamListSummary {
        received: 1,
        expected: 1,
    };
    env.step(&mut panel, Message::Reloaded(ADDRESS, ticket, Ok(summary)));
    assert_eq!(
        env.notices.history().last().unwrap().level,
        Level::Info,
        "no success notice for a download the operator stopped"
    );
}

/// Whether `panel` shows a download's activity.
fn shows_a_download(panel: &ParameterPanel, env: &Env) -> bool {
    let snapshot = env.fleet.snapshot();
    let mut simulator = iced_test::Simulator::new(panel.view(&snapshot, Some(ADDRESS.system_id())));
    simulator
        .find(
            |candidate: iced_test::selector::Candidate<'_>| match candidate {
                iced_test::selector::Candidate::Text { content, .. }
                    if content.starts_with("Downloading") =>
                {
                    Some(())
                }
                _ => None,
            },
        )
        .is_ok()
}

/// An incomplete set is not a download under way: after one fails or is
/// cancelled, the activity goes and the header counts what is missing.
#[tokio::test]
async fn an_incomplete_set_shows_no_download_once_it_stops() {
    let mut env = Env::new(&[]);
    let mut param_id = [0u8; 16];
    param_id[..4].copy_from_slice(b"GAIN");
    env.fleet.observe(
        ADDRESS,
        Instant::now(),
        Some(&mavio::DefaultDialect::ParamValue(messages::ParamValue {
            param_id,
            param_value: 1.0,
            param_type: MavParamType::Real32,
            param_count: 2,
            param_index: 0,
        })),
    );
    let mut panel = ParameterPanel::default();

    env.step(&mut panel, Message::Reload(ADDRESS));
    assert!(shows_a_download(&panel, &env));

    env.step(&mut panel, Message::CancelReload(ADDRESS));
    assert!(!shows_a_download(&panel, &env));

    let snapshot = env.fleet.snapshot();
    let mut simulator = iced_test::Simulator::new(panel.view(&snapshot, Some(ADDRESS.system_id())));
    assert!(
        simulator.find("1 of 2").is_ok(),
        "the header counts what is missing"
    );
}
