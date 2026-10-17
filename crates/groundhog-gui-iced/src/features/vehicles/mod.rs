//! The vehicles feature: the selected vehicle's state, health and controls,
//! the vehicle picker, and the vehicles' map layers.
//!
//! The panel shows what an operator glances at in flight. Status texts are
//! the log panel's; only the latest alert is repeated here.

mod map;

use std::time::Instant;

use groundhog_logic::{
    SystemId,
    domain::{
        firmware::{FlightMode, ModeKind},
        fleet::FleetSnapshot,
        vehicle::{Liveness, Vehicle},
    },
    services::Result,
};
use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{column, row, scrollable},
};
use mavio::default_dialect::enums::{GpsFixType, MissionState};

use super::{
    Context, Effect, confirm::Confirmation, notifications::Notice, operations::Operations,
};
use crate::ui::{self, Chip, TextExt, Tone};

/// The commands the panel runs, one of each kind per vehicle at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Command {
    Arm,
    Disarm,
    SetMode,
}

impl Command {
    fn verb(self) -> &'static str {
        match self {
            Command::Arm => "Arming",
            Command::Disarm => "Disarming",
            Command::SetMode => "Changing mode",
        }
    }
}

type Key = (SystemId, Command);

#[derive(Default)]
pub struct VehiclePanel {
    commands: Operations<Key>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Make a vehicle the one the UI shows.
    Select(SystemId),
    /// Follow the selected vehicle on the map.
    Follow(bool),
    /// Centre the map on a vehicle.
    Locate(SystemId),
    /// Ask for confirmation, then arm (`true`) or disarm.
    RequestArm(SystemId, bool),
    Arm(SystemId, bool),
    SetMode(SystemId, FlightMode),
    /// Command the vehicle's mode of a kind (return, land, ...).
    RequestModeKind(SystemId, ModeKind),
    ModeKind(SystemId, ModeKind),
    Finished(Key, super::operations::Ticket, Result<()>),
}

impl VehiclePanel {
    pub fn update(&mut self, message: Message, cx: &mut Context<Message>) -> Task<Message> {
        match message {
            Message::Select(system) => cx.request(Effect::SelectVehicle(system)),
            Message::Follow(follow) => cx.request(Effect::FollowVehicle(follow)),
            Message::Locate(system) => {
                if let Some(position) = cx
                    .fleet
                    .vehicle(system)
                    .and_then(|vehicle| vehicle.position.current.as_ref())
                {
                    cx.request(Effect::CenterMap(position.pos));
                }
            }
            Message::RequestArm(system, arm) => {
                let name = cx.fleet.vehicle_name(system);
                let confirmation = if arm {
                    Confirmation::new(
                        format!("Arm {name}?"),
                        "Motors may start spinning. Make sure the area around the vehicle is clear.",
                        Message::Arm(system, true),
                    )
                    .accept_label("Arm")
                    .danger()
                } else {
                    Confirmation::new(
                        format!("Disarm {name}?"),
                        "Disarming in flight stops the motors and the vehicle will fall.",
                        Message::Arm(system, false),
                    )
                    .accept_label("Disarm")
                    .danger()
                };
                cx.confirm(confirmation);
            }
            Message::Arm(system, arm) => {
                let command = if arm { Command::Arm } else { Command::Disarm };
                let client = cx.services.vehicle(system);
                return self.commands.start(
                    (system, command),
                    async move { client.arm(arm).await },
                    move |ticket, result| Message::Finished((system, command), ticket, result),
                );
            }
            Message::SetMode(system, mode) => {
                let client = cx.services.vehicle(system);
                return self.commands.start(
                    (system, Command::SetMode),
                    async move { client.set_mode(mode).await },
                    move |ticket, result| {
                        Message::Finished((system, Command::SetMode), ticket, result)
                    },
                );
            }
            Message::RequestModeKind(system, kind) => {
                let name = cx.fleet.vehicle_name(system);
                let (title, accept) = match kind {
                    ModeKind::ReturnToLaunch => (format!("Return {name} to launch?"), "Return"),
                    ModeKind::Land => (format!("Land {name} here?"), "Land"),
                    _ => (format!("Change the mode of {name}?"), "Change"),
                };
                cx.confirm(
                    Confirmation::new(
                        title,
                        "The vehicle leaves its current mode immediately.",
                        Message::ModeKind(system, kind),
                    )
                    .accept_label(accept),
                );
            }
            Message::ModeKind(system, kind) => {
                let client = cx.services.vehicle(system);
                return self.commands.start(
                    (system, Command::SetMode),
                    async move { client.set_mode_kind(kind).await.map(|_| ()) },
                    move |ticket, result| {
                        Message::Finished((system, Command::SetMode), ticket, result)
                    },
                );
            }
            Message::Finished(key, ticket, result) => {
                if self.commands.finish(&key, ticket)
                    && let Err(error) = result
                {
                    let (system, command) = key;
                    cx.notices.push(
                        Notice::error(format!(
                            "{} {} failed",
                            command.verb(),
                            cx.fleet.vehicle_name(system)
                        ))
                        .detail(error)
                        .vehicle(system),
                    );
                }
            }
        }

        Task::none()
    }

    pub fn view<'a>(
        &'a self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
        follow: bool,
    ) -> Element<'a, Message> {
        let picker = (!fleet.is_empty()).then(|| {
            ui::pick_list(selected, fleet.ids().collect::<Vec<_>>(), move |id| {
                fleet.vehicle(*id).map_or_else(
                    || fleet.vehicle_name(*id),
                    |vehicle| {
                        format!(
                            "{} ({})",
                            vehicle.display_name(),
                            liveness(vehicle.liveness).0
                        )
                    },
                )
            })
            .on_select(Message::Select)
        });

        let header = row![
            ui::checkbox("Follow", follow).on_toggle(Message::Follow),
            picker
        ]
        .spacing(ui::space::M)
        .align_y(Alignment::Center);

        let Some(vehicle) = selected.and_then(|id| fleet.vehicle(id)) else {
            return ui::panel(
                column![
                    header,
                    ui::divider(),
                    ui::text("No vehicle. Connect a link to begin.").tone(Tone::Muted)
                ]
                .spacing(ui::space::M),
            );
        };

        let contents = column![
            view_status(vehicle),
            ui::divider(),
            self.view_controls(vehicle),
            ui::section("Flight", [view_telemetry(vehicle)]),
            ui::section("Health", [view_health(vehicle)]),
            ui::section("Navigation", [view_navigation(vehicle)]),
            view_alert(vehicle),
        ]
        .spacing(ui::space::L);

        ui::panel(
            column![
                header,
                ui::divider(),
                scrollable(contents).height(Length::Fill)
            ]
            .spacing(ui::space::M),
        )
    }

    fn view_controls<'a>(&'a self, vehicle: &'a Vehicle) -> Element<'a, Message> {
        let system = vehicle.system;
        let busy = |command| self.commands.is_running(&(system, command));
        let firmware = vehicle.firmware();
        let class = vehicle.class();

        let arm = match vehicle.armed() {
            Some(true) => ui::button("Disarm")
                .danger()
                .busy(busy(Command::Disarm), "Disarming…")
                .on_press(Message::RequestArm(system, false)),
            _ => ui::button("Arm")
                .primary()
                .busy(busy(Command::Arm), "Arming…")
                .on_press(Message::RequestArm(system, true)),
        };

        let modes: Vec<FlightMode> = firmware
            .modes(class)
            .iter()
            .filter(|mode| mode.selectable)
            .copied()
            .collect();

        let mode_picker = ui::pick_list(vehicle.flight_mode(), modes, |mode| mode.name.to_string())
            .placeholder("Mode")
            .on_select(move |mode| Message::SetMode(system, mode));

        let quick = |kind: ModeKind, label: &'static str| {
            ui::button(label)
                .on_press_maybe(
                    (firmware.mode_of_kind(class, kind).is_some() && !busy(Command::SetMode))
                        .then_some(Message::RequestModeKind(system, kind)),
                )
                .boxed()
        };

        column![
            row![arm.boxed(), mode_picker]
                .spacing(ui::space::S)
                .align_y(Alignment::Center),
            ui::toolbar([
                quick(ModeKind::ReturnToLaunch, "Return"),
                quick(ModeKind::Land, "Land"),
                ui::button("Locate")
                    .on_press(Message::Locate(system))
                    .boxed(),
            ]),
        ]
        .spacing(ui::space::S)
        .boxed()
    }
}

/// Values shown three to a row, so they line up in columns.
fn stats<'a>(cells: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut rows = column![].spacing(ui::space::S);
    let mut cells = cells.into_iter().peekable();
    while cells.peek().is_some() {
        let mut line: Vec<Element<'a, Message>> = cells.by_ref().take(3).collect();
        // Short rows keep the columns' widths.
        while line.len() < 3 {
            line.push(ui::stat("", ui::text("")));
        }
        rows = rows.push(row(line).spacing(ui::space::M));
    }
    rows.boxed()
}

fn view_status(vehicle: &Vehicle) -> Element<'_, Message> {
    let (liveness, tone) = liveness(vehicle.liveness);

    let armed = match vehicle.armed() {
        Some(true) => ui::text("ARMED").tone(Tone::Danger),
        Some(false) => ui::text("disarmed"),
        None => ui::text("unknown").tone(Tone::Muted),
    };

    let mode = vehicle
        .flight_mode()
        .map_or_else(|| "unknown".to_string(), |mode| mode.name.to_string());

    let state = vehicle
        .identity
        .mav_state
        .map_or_else(|| "unknown".to_string(), |state| format!("{state:?}"));

    stats(vec![
        ui::stat("Link", ui::text(liveness).tone(tone)),
        ui::stat("Armed", armed),
        ui::stat("Mode", ui::text(mode)),
        ui::stat("Firmware", ui::text(vehicle.firmware().name())),
        ui::stat("Type", ui::text(vehicle.class().name())),
        ui::stat("State", ui::text(state)),
    ])
}

fn view_telemetry(vehicle: &Vehicle) -> Element<'_, Message> {
    let unknown = || "–".to_string();

    let (roll, pitch, yaw) = vehicle
        .attitude
        .current
        .as_ref()
        .map(|attitude| attitude.attitude.euler_angles())
        .map(|(roll, pitch, yaw)| {
            (
                format!("{:.1}°", roll.to_degrees()),
                format!("{:.1}°", pitch.to_degrees()),
                format!("{:.0}°", yaw.to_degrees().rem_euclid(360.0)),
            )
        })
        .unwrap_or_else(|| (unknown(), unknown(), unknown()));

    let altitude = vehicle
        .position
        .current
        .as_ref()
        .map_or_else(unknown, |position| format!("{:.1} m", position.alt_agl));
    let climb = vehicle
        .position
        .climb_rate()
        // Adding zero turns a negative zero into a positive one, so a
        // level vehicle does not read as sinking.
        .map_or_else(unknown, |climb| format!("{:+.1} m/s", climb + 0.0));
    let speed = vehicle
        .position
        .ground_speed()
        .map_or_else(unknown, |speed| format!("{speed:.1} m/s"));

    stats(vec![
        ui::stat("Roll", ui::text(roll)),
        ui::stat("Pitch", ui::text(pitch)),
        ui::stat("Heading", ui::text(yaw)),
        ui::stat("Altitude", ui::text(altitude)),
        ui::stat("Climb", ui::text(climb)),
        ui::stat("Speed", ui::text(speed)),
    ])
}

/// The battery, GPS, link and pre-arm checks, and the sensors as chips.
fn view_health(vehicle: &Vehicle) -> Element<'_, Message> {
    let health = &vehicle.health;

    // The battery: a gauge, with what is known beside it.
    let battery = vehicle.battery.primary();
    let remaining = battery.and_then(|battery| battery.remaining);
    let tone = match remaining {
        None => Tone::Muted,
        Some(percent) if percent < 20.0 => Tone::Danger,
        Some(percent) if percent < 35.0 => Tone::Warning,
        Some(_) => Tone::Success,
    };
    let readings: Vec<String> = [
        remaining.map(|percent| format!("{percent:.0} %")),
        battery
            .and_then(|battery| battery.voltage)
            .map(|volts| format!("{volts:.2} V")),
        battery
            .and_then(|battery| battery.current)
            .map(|amps| format!("{amps:.1} A")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let battery = column![
        row![
            ui::caption("Battery").width(Length::Fill),
            ui::caption(if readings.is_empty() {
                "–".to_string()
            } else {
                readings.join(" · ")
            })
        ],
        ui::meter(remaining.unwrap_or(0.0), 100.0, tone),
    ]
    .spacing(ui::space::XS);

    // The GPS: the fix in its tone, satellites and precision in the caption.
    let gps = match health.gps() {
        None => ui::text("–"),
        Some(gps) => {
            let tone = match gps.fix {
                GpsFixType::NoGps => Tone::Muted,
                _ if !gps.has_position() => Tone::Danger,
                _ if gps.hdop.is_some_and(|hdop| hdop > 2.0) => Tone::Warning,
                _ => Tone::Success,
            };
            ui::text(gps.fix_name()).tone(tone)
        }
    };
    let precision = health
        .gps()
        .map(|gps| {
            [
                gps.satellites.map(|count| format!("{count} sats")),
                gps.hdop.map(|hdop| format!("HDOP {hdop:.1}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
        })
        .unwrap_or_default();

    let heard = vehicle.last_seen.map_or_else(
        || ui::text("–"),
        |at| {
            let ago = Instant::now().saturating_duration_since(at).as_secs_f32();
            ui::text(format!("{ago:.1} s ago")).tone(liveness(vehicle.liveness).1)
        },
    );

    let prearm = match health.prearm_passes() {
        Some(true) => ui::text("Ready").tone(Tone::Success),
        Some(false) => ui::text("Not ready").tone(Tone::Danger),
        None => ui::text("–").tone(Tone::Muted),
    };

    let checks = stats(vec![
        ui::stat("GPS", gps),
        ui::stat("Last heard", heard),
        ui::stat("Pre-arm", prearm),
    ]);

    // Healthy sensors in green, failing ones in red, those not in use dim.
    let sensors = health.sensors();
    let sensors = if sensors.is_empty() {
        ui::caption("No sensor health reported")
            .tone(Tone::Muted)
            .boxed()
    } else {
        row(sensors.into_iter().map(|status| {
            let state = match (status.enabled, status.healthy) {
                (false, _) => Chip::Pending,
                (true, true) => Chip::Done,
                (true, false) => Chip::Failed,
            };
            ui::chip(status.sensor.name(), state)
        }))
        .spacing(ui::space::XS)
        .wrap()
        .boxed()
    };

    column![
        battery,
        checks,
        (!precision.is_empty()).then(|| ui::caption(precision).tone(Tone::Muted)),
        sensors,
    ]
    .spacing(ui::space::S)
    .boxed()
}

/// Home, and how far the mission has come.
fn view_navigation(vehicle: &Vehicle) -> Element<'_, Message> {
    let home = vehicle.to_home().map_or_else(
        || ui::text("–"),
        |home| {
            ui::text(format!(
                "{} · {:.0}°",
                distance(home.distance_m),
                home.bearing_deg
            ))
        },
    );

    let Some(mission) = vehicle.mission_status() else {
        return stats(vec![
            ui::stat("Home", home),
            ui::stat("Mission", ui::text("–")),
        ]);
    };

    let (state, tone) = match mission.state {
        MissionState::Active => ("active", Tone::Success),
        MissionState::Paused => ("paused", Tone::Warning),
        MissionState::Complete => ("complete", Tone::Muted),
        MissionState::NotStarted => ("not started", Tone::Muted),
        _ => ("", Tone::Muted),
    };
    let item = match mission.count {
        Some(count) => format!("Item {} of {count}", mission.current + 1),
        None => format!("Item {}", mission.current + 1),
    };
    let to_item = mission
        .to_current
        .map_or_else(|| "–".to_string(), |offset| distance(offset.distance_m));

    column![
        stats(vec![
            ui::stat("Home", home),
            ui::stat("Mission", ui::text(item)),
            ui::stat("To item", ui::text(to_item)),
        ]),
        row![
            ui::progress(
                (mission.current + 1) as f32,
                mission.count.unwrap_or(mission.current + 1) as f32
            ),
            ui::caption(state).tone(tone),
        ]
        .spacing(ui::space::M)
        .align_y(Alignment::Center),
    ]
    .spacing(ui::space::S)
    .boxed()
}

/// The latest warning or worse the vehicle sent, if any; the log panel has
/// them all.
fn view_alert(vehicle: &Vehicle) -> Element<'_, Message> {
    match vehicle.status.latest_alert() {
        Some(alert) => column![
            ui::caption("Latest alert"),
            ui::text(&alert.text)
                .tone(Tone::of_severity(alert.severity))
                .single_line(),
        ]
        .spacing(ui::space::XXS)
        .boxed(),
        None => ui::caption("No alerts").tone(Tone::Muted).boxed(),
    }
}

/// A distance in metres, or kilometres from one up.
fn distance(metres: f64) -> String {
    if metres < 1000.0 {
        format!("{metres:.0} m")
    } else {
        format!("{:.2} km", metres / 1000.0)
    }
}

/// A short, legible marker for a vehicle's liveness, and its tone.
fn liveness(liveness: Liveness) -> (&'static str, Tone) {
    match liveness {
        Liveness::Active => ("active", Tone::Success),
        Liveness::Stale => ("stale", Tone::Warning),
        Liveness::Lost => ("lost", Tone::Danger),
    }
}
