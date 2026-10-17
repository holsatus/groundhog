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
        enums::{
            GpsFixType, MavAutopilot, MavModeFlag, MavParamType, MavSeverity, MavState,
            MavSysStatusSensor, MavType, MissionState,
        },
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
    // Every usual sensor present, the airspeed sensor unused, the compass
    // failing, and so the pre-arm checks too.
    let present = MavSysStatusSensor::_3D_GYRO
        | MavSysStatusSensor::_3D_ACCEL
        | MavSysStatusSensor::_3D_MAG
        | MavSysStatusSensor::ABSOLUTE_PRESSURE
        | MavSysStatusSensor::DIFFERENTIAL_PRESSURE
        | MavSysStatusSensor::GPS
        | MavSysStatusSensor::RC_RECEIVER
        | MavSysStatusSensor::MAV_SYS_STATUS_AHRS
        | MavSysStatusSensor::MOTOR_OUTPUTS
        | MavSysStatusSensor::BATTERY
        | MavSysStatusSensor::MAV_SYS_STATUS_PREARM_CHECK;
    let enabled = present.difference(MavSysStatusSensor::DIFFERENTIAL_PRESSURE);
    let healthy = present
        .difference(MavSysStatusSensor::_3D_MAG)
        .difference(MavSysStatusSensor::MAV_SYS_STATUS_PREARM_CHECK);
    observe(DefaultDialect::SysStatus(messages::SysStatus {
        voltage_battery: 15_820,
        current_battery: 1_240,
        battery_remaining: 76,
        onboard_control_sensors_present: present,
        onboard_control_sensors_enabled: enabled,
        onboard_control_sensors_health: healthy,
        ..Default::default()
    }));
    observe(DefaultDialect::GpsRawInt(messages::GpsRawInt {
        fix_type: GpsFixType::_3dFix,
        satellites_visible: 14,
        eph: 80,
        ..Default::default()
    }));
    observe(DefaultDialect::HomePosition(messages::HomePosition {
        latitude: 554_650_000,
        longitude: 104_100_000,
        ..Default::default()
    }));
    // ArduPilot numbers the home slot 0: sequence 2 is the second item.
    observe(DefaultDialect::MissionCurrent(messages::MissionCurrent {
        seq: 2,
        total: 4,
        mission_state: MissionState::Active,
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

/// One step of the pointer, before a render.
enum Pointer {
    Move(iced::Point),
    Press,
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
        let moves = cursor.map(Pointer::Move);
        self.render_after(name, size, theme, element, moves);
    }

    /// Render `element` after the pointer did `steps`: shows what only
    /// shows on hover, or in the middle of a drag.
    fn render_after(
        &self,
        name: &str,
        size: Size,
        theme: &Theme,
        element: Element<'_, Message>,
        steps: impl IntoIterator<Item = Pointer>,
    ) {
        use iced::mouse;

        let path = self.directory.join(name);
        let mut simulator = iced_test::Simulator::with_size(Default::default(), size, element);
        for step in steps {
            let event = match step {
                Pointer::Move(position) => {
                    simulator.point_at(position);
                    mouse::Event::CursorMoved { position }
                }
                Pointer::Press => mouse::Event::ButtonPressed(mouse::Button::Left),
            };
            let _ = simulator.simulate([iced::Event::Mouse(event)]);
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
            // Any free port, so the gallery never collides with a real SITL.
            sock_addr: std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 0),
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
    // A panel in each of the three places.
    for (panel, slot) in [
        (layout::Panel::Mission, layout::Slot::Left),
        (layout::Panel::Parameters, layout::Slot::Right),
        (layout::Panel::Logs, layout::Slot::Middle),
        (layout::Panel::Links, layout::Slot::Middle),
    ] {
        let _ = app.update(Message::Layout(layout::Message::Toggle(panel, slot)));
    }

    for (mode, suffix) in [(ThemeMode::Dark, "dark"), (ThemeMode::Light, "light")] {
        let _ = app.update(Message::Settings(settings_panel::Message::Theme(mode)));
        let theme = app.theme();

        // What the grid's sensor reports in the application: the window,
        // less its padding, the rail, and the gap beside the rail.
        let grid = Size::new(
            1600.0 - 3.0 * ui::space::S - ui::size::RAIL_BUTTON,
            1000.0 - 2.0 * ui::space::S,
        );
        let _ = app.update(Message::Layout(layout::Message::Sized(grid)));
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

    // Dragging the parameters (right column) by their title bar, to show
    // where they would land: the bottom half of the vehicles pane (splitting
    // it), the divider beneath it (an even share), its middle (a swap), and
    // the right half of the log, in the row beneath the map (splitting it).
    let _ = app.update(Message::Settings(settings_panel::Message::Theme(
        ThemeMode::Dark,
    )));
    let grid = Size::new(
        1600.0 - 3.0 * ui::space::S - ui::size::RAIL_BUTTON,
        1000.0 - 2.0 * ui::space::S,
    );
    let _ = app.update(Message::Layout(layout::Message::Sized(grid)));
    let origin = iced::Point::new(1600.0 - ui::space::S - 360.0 / 2.0, ui::space::S + 18.0);
    let regions: Vec<(layout::Panel, iced::Rectangle)> = app
        .layout
        .panes()
        .layout()
        .pane_regions(layout::SPACING, layout::MIN_PANE, grid)
        .into_iter()
        .map(|(pane, region)| {
            // The grid sits right of the rail, inside the window's padding.
            let offset =
                iced::Vector::new(2.0 * ui::space::S + ui::size::RAIL_BUTTON, ui::space::S);
            (*app.layout.panes().get(pane).unwrap(), region + offset)
        })
        .collect();
    let region = |panel| regions.iter().find(|(p, _)| *p == panel).unwrap().1;
    let vehicles = region(layout::Panel::Vehicles);
    for (name, at) in [
        (
            "drag-split",
            iced::Point::new(vehicles.center_x(), vehicles.y + vehicles.height * 0.75),
        ),
        (
            "drag-between",
            iced::Point::new(vehicles.center_x(), vehicles.y + vehicles.height + 2.0),
        ),
        ("drag-swap", vehicles.center()),
        ("drag-row", {
            let logs = region(layout::Panel::Logs);
            iced::Point::new(logs.x + logs.width * 0.75, logs.center_y())
        }),
    ] {
        gallery.render_after(
            name,
            Size::new(1600.0, 1000.0),
            &app.theme(),
            app.view().boxed(),
            [
                Pointer::Move(origin),
                Pointer::Press,
                Pointer::Move(origin + iced::Vector::new(-40.0, 40.0)),
                Pointer::Move(at),
            ],
        );
    }

    // Dragging a closed panel's rail button onto the middle of the
    // vehicles pane: it has nothing to swap with, so it splits the half
    // the cursor is on.
    let (index, closed) = layout::Panel::DOCKABLE
        .into_iter()
        .enumerate()
        .find(|(_, panel)| !app.layout.is_open(*panel))
        .expect("a closed panel");
    let button = iced::Point::new(
        ui::space::S + ui::size::RAIL_BUTTON / 2.0,
        ui::space::S
            + index as f32 * (ui::size::RAIL_BUTTON + ui::space::S)
            + ui::size::RAIL_BUTTON / 2.0,
    );
    let _ = app.update(Message::RailDragged(closed));
    gallery.render_after(
        "drag-rail",
        Size::new(1600.0, 1000.0),
        &app.theme(),
        app.view().boxed(),
        [
            Pointer::Move(button),
            Pointer::Press,
            Pointer::Move(button + iced::Vector::new(40.0, 20.0)),
            Pointer::Move(vehicles.center() + iced::Vector::new(0.0, 20.0)),
        ],
    );
    let _ = app.update(Message::RailDropped);

    // A saved link loaded into the editor, which would overwrite it.
    let sitl = app
        .backend
        .links()
        .entries()
        .find(|(_, config, _)| config.name == "SITL")
        .map(|(id, ..)| id.clone())
        .expect("the SITL link is saved");
    let _ = app.update(Message::Links(link_panel::Message::EditLink(sitl.clone())));
    gallery.render(
        "panel-links-overwrite-dark",
        Size::new(560.0, 820.0),
        &ThemeMode::Dark.theme(),
        iced::widget::container(app.pane_body(layout::Panel::Links))
            .style(style::pane)
            .boxed(),
    );

    // The link connected. (Its details are in a tooltip, which shows only
    // after a hover delay that a snapshot does not wait for.)
    let _ = app.update(Message::Links(link_panel::Message::Connect(sitl.clone())));
    for _ in 0..100 {
        if app.backend.links().status(&sitl).is_connected() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    gallery.render(
        "panel-links-connected-dark",
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
