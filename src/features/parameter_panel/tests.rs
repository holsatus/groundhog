use std::time::Instant;

use mavio::default_dialect::{
    enums::{MavAutopilot, MavModeFlag, MavParamType, MavType},
    messages,
};

use super::*;
use crate::{
    config::Settings,
    domain::fleet::Fleet,
    features::notifications::{Level, Notifications},
    mavlink::{DEFAULT_GCS_ID, MavlinkRouter},
    services::{FleetHandle, RuntimeOptions, runtime},
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
        let (router, events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (handle, _updates) =
            runtime::spawn(router, events, RuntimeOptions { bootstrap: false });

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
async fn editing_back_to_the_reported_value_drops_the_edit() {
    let mut env = Env::new(&[("GAIN", 1.0)]);
    let mut panel = ParameterPanel::default();

    env.step(
        &mut panel,
        Message::Edit(ADDRESS, ident("GAIN"), "2.5".into()),
    );
    let edit = panel.edit(ADDRESS, &ident("GAIN")).unwrap();
    assert_eq!(edit.value, Some(Value::F32(2.5)));

    env.step(
        &mut panel,
        Message::Edit(ADDRESS, ident("GAIN"), "1".into()),
    );
    assert!(panel.edit(ADDRESS, &ident("GAIN")).is_none());
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
async fn loading_a_file_creates_edits_for_differences_only() {
    let env = Env::new(&[("GAIN", 1.0), ("RATE", 2.0)]);
    let mut panel = ParameterPanel::default();

    let mut loaded = ParamValues::new();
    loaded.insert(ident("GAIN"), Value::F32(1.0)); // unchanged
    loaded.insert(ident("RATE"), Value::F32(4.0)); // changed
    loaded.insert(ident("MISSING"), Value::F32(1.0)); // unknown
    loaded.insert(ident("GAIN"), Value::F32(1.0));

    let snapshot = env.fleet.snapshot();
    let (changed, skipped) = panel.load_edits(&snapshot, ADDRESS, &loaded);
    assert_eq!((changed, skipped), (1, 1));
    assert_eq!(
        panel.edit(ADDRESS, &ident("RATE")).unwrap().value,
        Some(Value::F32(4.0))
    );
    assert!(panel.edit(ADDRESS, &ident("GAIN")).is_none());
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
