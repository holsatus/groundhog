//! A gallery of the interface, rendered headlessly to PNG files for review.
//!
//! Not a regression test: it renders the whole window and every panel, with
//! a synthetic vehicle, so a change to the look can be inspected before and
//! after. Run it with an output directory and a configuration directory of
//! its own (the application loads and saves its configuration at boot):
//!
//! ```sh
//! GROUNDHOG_GALLERY=/tmp/gallery XDG_CONFIG_HOME=/tmp/gallery/config \
//!     XDG_DATA_HOME=/tmp/gallery/data \
//!     cargo test -p groundhog-gui-iced gallery -- --ignored
//! ```
//!
//! Existing images are replaced.

use std::{path::PathBuf, sync::Arc, time::Instant};

use groundhog_logic::{
    MavlinkId, SystemId,
    domain::{
        fleet::{Fleet, FleetEvent},
        mission::{MissionItem, OnboardMission, Position},
    },
    link::{LinkBuild, LinkConfig},
    services::FleetUpdate,
};
use iced::{Size, Theme};
use mav_param::Ident;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavAutopilot, MavModeFlag, MavParamType, MavSeverity, MavState, MavType},
        messages,
    },
};

use super::*;
use crate::{features::mission::MessageKind, ui::theme::ThemeMode};

const SYSTEM: SystemId = SystemId(1);
const AUTOPILOT: MavlinkId = MavlinkId {
    system: 1,
    component: 1,
};

/// A vehicle with telemetry, parameters, status texts and a mission.
fn fleet() -> Fleet {
    let mut fleet = Fleet::default();
    let now = Instant::now();
    let mut observe = |message: DefaultDialect| {
        fleet.observe(AUTOPILOT, now, Some(&message));
    };

    observe(DefaultDialect::Heartbeat(messages::Heartbeat {
        type_: MavType::Quadrotor,
        autopilot: MavAutopilot::Ardupilotmega,
        base_mode: MavModeFlag::CUSTOM_MODE_ENABLED,
        custom_mode: 5,
        system_status: MavState::Standby,
        ..Default::default()
    }));
    observe(DefaultDialect::GlobalPositionInt(
        messages::GlobalPositionInt {
            lat: 554_710_000,
            lon: 104_120_000,
            alt: 42_000,
            relative_alt: 12_500,
            vx: 230,
            vy: -110,
            ..Default::default()
        },
    ));
    observe(DefaultDialect::Attitude(messages::Attitude {
        roll: 0.05,
        pitch: -0.12,
        yaw: 1.2,
        ..Default::default()
    }));
    observe(DefaultDialect::SysStatus(messages::SysStatus {
        voltage_battery: 15_820,
        current_battery: 1_240,
        battery_remaining: 76,
        ..Default::default()
    }));

    let params = [
        ("ARMING_CHECK", 1.0),
        ("BATT_CAPACITY", 5200.0),
        ("BATT_MONITOR", 4.0),
        ("COMPASS_USE", 1.0),
        ("WPNAV_SPEED", 500.0),
        ("WPNAV_RADIUS", 200.0),
    ];
    for (index, (name, value)) in params.iter().enumerate() {
        let mut param_id = [0u8; 16];
        param_id[..name.len()].copy_from_slice(name.as_bytes());
        observe(DefaultDialect::ParamValue(messages::ParamValue {
            param_id,
            param_value: *value,
            param_type: MavParamType::Real32,
            param_count: params.len() as u16,
            param_index: index as u16,
        }));
    }

    for (severity, text) in [
        (MavSeverity::Info, "ArduCopter V4.5.7 (2a1fa1b9)"),
        (MavSeverity::Notice, "EKF3 IMU0 is using GPS"),
        (MavSeverity::Warning, "PreArm: Compass not calibrated"),
        (MavSeverity::Error, "Radio failsafe"),
    ] {
        let mut bytes = [0u8; 50];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        observe(DefaultDialect::Statustext(messages::Statustext {
            severity,
            text: bytes,
            ..Default::default()
        }));
    }

    let waypoint = |lat: f64, lon: f64| {
        MissionItem::waypoint(Position {
            lat,
            lon,
            alt_m: 30.0,
        })
    };
    fleet.set_onboard_mission(
        SYSTEM,
        OnboardMission {
            items: vec![
                waypoint(55.4712, 10.4125),
                waypoint(55.4720, 10.4140),
                waypoint(55.4705, 10.4152),
            ],
            home: None,
            opaque_id: 0,
        },
    );

    fleet
}

struct Gallery {
    directory: PathBuf,
}

impl Gallery {
    /// Render `element` at `size` into `<name>.png`.
    fn render(&self, name: &str, size: Size, theme: &Theme, element: Element<'_, Message>) {
        self.render_hovering(name, size, theme, element, None);
    }

    /// Render `element` with the pointer at `cursor`, which shows what only
    /// shows on hover (a pane's controls).
    fn render_hovering(
        &self,
        name: &str,
        size: Size,
        theme: &Theme,
        element: Element<'_, Message>,
        cursor: Option<iced::Point>,
    ) {
        let path = self.directory.join(name);
        let mut simulator = iced_test::Simulator::with_size(Default::default(), size, element);
        if let Some(cursor) = cursor {
            simulator.point_at(cursor);
        }
        let snapshot = simulator.snapshot(theme).expect("the interface renders");

        // `matches_image` writes only when no image exists yet, under a
        // name with the renderer appended.
        for entry in std::fs::read_dir(&self.directory).expect("the directory exists") {
            let entry = entry.expect("a directory entry");
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{name}-"))
            {
                std::fs::remove_file(entry.path()).expect("the old image is removable");
            }
        }
        snapshot.matches_image(path).expect("the image is written");
    }
}

#[tokio::test]
#[ignore = "writes images for review; see the module documentation"]
async fn gallery() {
    let directory = std::env::var_os("GROUNDHOG_GALLERY").expect("GROUNDHOG_GALLERY is set");
    assert!(
        std::env::var_os("XDG_CONFIG_HOME").is_some(),
        "XDG_CONFIG_HOME must point away from the real configuration"
    );
    let gallery = Gallery {
        directory: PathBuf::from(directory),
    };
    std::fs::create_dir_all(&gallery.directory).expect("the directory is creatable");

    let (mut app, _) = Application::boot();
    let _ = app.update(Message::Fleet(FleetUpdate {
        snapshot: Arc::new(fleet().snapshot()),
        events: vec![FleetEvent::Registered(SYSTEM)],
    }));

    // A configured link, so the link list shows.
    let _ = app.update(Message::Links(link_panel::Message::Save(LinkConfig {
        name: "SITL".to_string(),
        auto_connect: false,
        high_latency: false,
        auto_reconnect: true,
        build: LinkBuild::Udp {
            sock_addr: std::net::SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 14550),
            peer: None,
        },
    })));

    let _ = app.update(Message::Links(link_panel::Message::Save(LinkConfig {
        name: "Telemetry radio".to_string(),
        auto_connect: false,
        high_latency: false,
        auto_reconnect: true,
        build: LinkBuild::Serial {
            port: "/dev/ttyUSB0".to_string(),
            baud: 57_600,
        },
    })));

    // An edited parameter and a selected mission item, so their editors show.
    let _ = app.update(Message::Parameters(parameter_panel::Message::Edit(
        AUTOPILOT,
        Ident::from_str_truncated("WPNAV_SPEED"),
        "750".into(),
    )));
    let _ = app.update(Message::Mission(mission::Message {
        plan: Some(SYSTEM),
        kind: MessageKind::AddWaypoint,
    }));
    for panel in [
        layout::Panel::Mission,
        layout::Panel::Parameters,
        layout::Panel::Logs,
    ] {
        let _ = app.update(Message::Layout(layout::Message::Toggle(panel)));
    }

    for (mode, suffix) in [(ThemeMode::Dark, "dark"), (ThemeMode::Light, "light")] {
        let _ = app.update(Message::Settings(settings_panel::Message::Theme(mode)));
        let theme = app.theme();

        gallery.render(
            &format!("window-{suffix}"),
            Size::new(1600.0, 1000.0),
            &theme,
            app.view().boxed(),
        );
        // The pointer over the first pane's title bar, showing its controls.
        gallery.render_hovering(
            &format!("window-hover-{suffix}"),
            Size::new(1600.0, 1000.0),
            &theme,
            app.view().boxed(),
            Some(iced::Point::new(300.0, 18.0)),
        );

        for panel in layout::Panel::DOCKABLE {
            let name = format!("panel-{}-{suffix}", panel.title().to_lowercase());
            gallery.render(
                &name,
                Size::new(560.0, 820.0),
                &theme,
                iced::widget::container(app.pane_body(panel))
                    .style(style::pane)
                    .boxed(),
            );
        }
    }

    // A saved link loaded into the editor, which would overwrite it.
    let sitl = app
        .backend
        .links()
        .entries()
        .find(|(_, config, _)| config.name == "SITL")
        .map(|(id, ..)| id.clone())
        .expect("the SITL link is saved");
    let _ = app.update(Message::Links(link_panel::Message::EditLink(sitl)));
    gallery.render(
        "panel-links-overwrite-dark",
        Size::new(560.0, 820.0),
        &ThemeMode::Dark.theme(),
        iced::widget::container(app.pane_body(layout::Panel::Links))
            .style(style::pane)
            .boxed(),
    );

    // A pending confirmation, over the whole window.
    let _ = app.update(Message::Settings(settings_panel::Message::Theme(
        ThemeMode::Dark,
    )));
    let _ = app.update(Message::Mission(mission::Message {
        plan: Some(SYSTEM),
        kind: MessageKind::Clear { confirmed: false },
    }));
    gallery.render(
        "window-confirmation",
        Size::new(1600.0, 1000.0),
        &app.theme(),
        app.view().boxed(),
    );
}
