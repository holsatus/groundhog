//! The vehicles feature: the selected vehicle's state, health and controls,
//! the vehicle picker, and the vehicles' map layers.
//!
//! The panel shows what an operator glances at in flight. Status texts are
//! the log panel's; only the latest alert is repeated here.

mod map;

use std::time::{Duration, Instant};

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
use crate::ui::{self, Icon, TextExt, Tone};

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
    /// Switch the vehicle to a mode, asking first while it may be flying
    /// (see [`mode_change_asks`]).
    RequestMode(SystemId, FlightMode),
    SetMode(SystemId, FlightMode),
    /// Command the vehicle's mode of a kind (return, land, ...), asking
    /// first while it may be flying.
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
            Message::RequestMode(system, mode) => {
                let vehicle = cx.fleet.vehicle(system);
                if vehicle.and_then(Vehicle::flight_mode) == Some(mode) {
                    return Task::none();
                }
                if !mode_change_asks(vehicle) {
                    return self.update(Message::SetMode(system, mode), cx);
                }
                let name = cx.fleet.vehicle_name(system);
                cx.confirm(
                    Confirmation::new(
                        format!("Switch {name} to {mode}?"),
                        "The vehicle leaves its current mode immediately.",
                        Message::SetMode(system, mode),
                    )
                    .accept_label("Switch"),
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
                if !mode_change_asks(cx.fleet.vehicle(system)) {
                    return self.update(Message::ModeKind(system, kind), cx);
                }
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

    /// The panel, most urgent first: which vehicle, how it is (mode,
    /// arming, a recent alert), the commands, then the read-outs.
    pub fn view<'a>(
        &'a self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
        follow: bool,
    ) -> Element<'a, Message> {
        let picker = (!fleet.is_empty()).then(|| {
            ui::pick_list(selected, fleet.ids().collect::<Vec<_>>(), move |id| {
                fleet.vehicle_name(*id)
            })
            .on_select(Message::Select)
        });
        let vehicle = selected.and_then(|id| fleet.vehicle(id));

        let header = ui::header(
            column![picker],
            [
                vehicle.map(|vehicle| {
                    let (label, tone) = liveness(vehicle.liveness);
                    ui::badge(label, tone).boxed()
                }),
                Some(
                    ui::checkbox("Follow", follow)
                        .on_toggle(Message::Follow)
                        .boxed(),
                ),
            ]
            .into_iter()
            .flatten(),
            [],
        );

        let Some(vehicle) = vehicle else {
            return ui::panel_with(
                header,
                ui::text("No vehicle. Connect a link to begin.").tone(Tone::Muted),
            )
            .boxed();
        };

        let contents = column![
            view_status(vehicle),
            view_alert(vehicle),
            self.view_controls(vehicle),
            ui::section("Flight", [view_telemetry(vehicle).boxed()]),
            ui::section("Health", [view_health(vehicle).boxed()]),
            ui::section("Navigation", [view_navigation(vehicle)]),
        ]
        .spacing(ui::space::L);

        ui::panel_with(header, scrollable(contents).height(Length::Fill)).boxed()
    }

    fn view_controls<'a>(&'a self, vehicle: &'a Vehicle) -> impl Widget<Message> + 'a {
        let system = vehicle.system;
        let busy = |command| self.commands.is_running(&(system, command));

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

        let modes: Vec<FlightMode> = vehicle.modes().collect();

        let mut mode_picker =
            ui::pick_list(vehicle.flight_mode(), modes, |mode| mode.name.to_string())
                .placeholder("Mode")
                .width(Length::Fixed(160.0));
        if !busy(Command::SetMode) {
            mode_picker = mode_picker.on_select(move |mode| Message::RequestMode(system, mode));
        }

        let quick = |kind: ModeKind, label: &'static str, hint: &'static str| {
            ui::button(label)
                .hint(hint)
                .on_press_maybe(
                    (vehicle.mode_of_kind(kind).is_some() && !busy(Command::SetMode))
                        .then_some(Message::RequestModeKind(system, kind)),
                )
                .boxed()
        };

        ui::actions([
            vec![arm.width(Length::Fixed(96.0)).boxed(), mode_picker.boxed()],
            vec![
                quick(ModeKind::ReturnToLaunch, "Return", "Fly home and land"),
                quick(ModeKind::Land, "Land", "Land where the vehicle is"),
                ui::icon_button(Icon::Place)
                    .hint("Show the vehicle on the map")
                    .on_press(Message::Locate(system))
                    .boxed(),
            ],
        ])
    }
}

/// Whether a mode change asks the operator first: while the vehicle is
/// armed, or its armed state is unknown. A disarmed vehicle does not act on
/// its mode until it is armed, and arming asks.
fn mode_change_asks(vehicle: Option<&Vehicle>) -> bool {
    vehicle.and_then(Vehicle::armed) != Some(false)
}

/// Values in columns that line up: three to a row where they fit.
fn stats<'a>(
    cells: impl IntoIterator<Item = impl Widget<Message> + 'a>,
) -> impl Widget<Message> + 'a {
    ui::stats(cells, 3)
}

/// What the vehicle is doing, at a glance: its mode, large, whether it
/// is armed, and what it is.
fn view_status(vehicle: &Vehicle) -> impl Widget<Message> + '_ {
    let armed = match vehicle.armed() {
        Some(true) => ui::badge("Armed", Tone::Danger),
        Some(false) => ui::badge("Disarmed", Tone::Muted),
        None => ui::badge("Arming unknown", Tone::Muted),
    };

    let mode = vehicle
        .flight_mode()
        .map_or_else(|| "Unknown mode".to_string(), |mode| mode.name.to_string());

    let mut identity = vec![
        vehicle.firmware().name().to_string(),
        vehicle.class().name().to_string(),
    ];
    identity.extend(vehicle.identity.mav_state.map(|state| format!("{state:?}")));

    column![
        row![ui::title(mode), armed]
            .spacing(ui::space::M)
            .align_y(Alignment::Center),
        ui::caption(identity.join(" · ")).tone(Tone::Muted),
    ]
    .spacing(ui::space::XXS)
}

fn view_telemetry(vehicle: &Vehicle) -> impl Widget<Message> + '_ {
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

    stats([
        ui::stat("Roll", ui::text(roll)),
        ui::stat("Pitch", ui::text(pitch)),
        ui::stat("Heading", ui::text(yaw)),
        ui::stat("Altitude", ui::text(altitude)),
        ui::stat("Climb", ui::text(climb)),
        ui::stat("Speed", ui::text(speed)),
    ])
}

/// The battery, GPS, link and pre-arm checks, and the sensors as badges.
fn view_health(vehicle: &Vehicle) -> impl Widget<Message> + '_ {
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
            ui::caption("Battery").tone(Tone::Muted).width(Length::Fill),
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

    let checks = stats([
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
            let tone = match (status.enabled, status.healthy) {
                (false, _) => Tone::Muted,
                (true, true) => Tone::Success,
                (true, false) => Tone::Danger,
            };
            ui::badge(status.sensor.name(), tone)
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
        return stats([ui::stat("Home", home), ui::stat("Mission", ui::text("–"))]).boxed();
    };

    let (state, tone) = match mission.state {
        MissionState::Active => ("Active", Tone::Success),
        MissionState::Paused => ("Paused", Tone::Warning),
        MissionState::Complete => ("Complete", Tone::Muted),
        MissionState::NotStarted => ("Not started", Tone::Muted),
        _ => ("Unknown", Tone::Muted),
    };
    let item = match mission.count {
        Some(count) => format!("Item {} of {count}", mission.current + 1),
        None => format!("Item {}", mission.current + 1),
    };
    let to_item = mission
        .to_current
        .map_or_else(|| "–".to_string(), |offset| distance(offset.distance_m));

    column![
        stats([
            ui::stat("Home", home),
            ui::stat("Mission", ui::text(item)),
            ui::stat("To item", ui::text(to_item)),
        ]),
        row![
            ui::progress(
                (mission.current + 1) as f32,
                mission.count.unwrap_or(mission.current + 1) as f32
            ),
            ui::badge(state, tone),
        ]
        .spacing(ui::space::M)
        .align_y(Alignment::Center),
    ]
    .spacing(ui::space::S)
    .boxed()
}

/// How long an alert stays on the panel. Older ones are in the log.
const ALERT_SHOWN_FOR: Duration = Duration::from_secs(60);

/// The latest warning or worse the vehicle sent, while it is recent.
fn view_alert(vehicle: &Vehicle) -> Option<impl Widget<Message> + '_> {
    let alert = vehicle
        .status
        .latest_alert()
        .filter(|alert| alert.at.elapsed() < ALERT_SHOWN_FOR)?;
    Some(ui::banner(&alert.text, Tone::of_severity(alert.severity)))
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
        Liveness::Active => ("Active", Tone::Success),
        Liveness::Stale => ("Stale", Tone::Warning),
        Liveness::Lost => ("Lost", Tone::Danger),
    }
}

#[cfg(test)]
mod tests;
