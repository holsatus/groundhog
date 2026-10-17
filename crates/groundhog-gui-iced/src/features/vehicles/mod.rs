//! The vehicles feature: the selected vehicle's state and controls, the
//! vehicle picker, and the vehicles' map layers.

mod map;

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

use super::{
    Context, Effect, confirm::Confirmation, log_panel::timeline_row, notifications::Notice,
    operations::Operations,
};
use crate::{
    ui::{self, TextExt, Tone},
    widget::virtualized::virtual_list,
};

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

        ui::panel(
            column![
                header,
                ui::divider(),
                view_status(vehicle),
                ui::divider(),
                self.view_controls(vehicle),
                ui::divider(),
                view_telemetry(vehicle),
                ui::divider(),
                view_status_texts(vehicle),
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
    let speed = vehicle
        .position
        .ground_speed()
        .map_or_else(unknown, |speed| format!("{speed:.1} m/s"));

    let battery = vehicle.battery.primary();
    let voltage = battery
        .and_then(|battery| battery.voltage)
        .map_or_else(unknown, |volts| format!("{volts:.2} V"));
    let current = battery
        .and_then(|battery| battery.current)
        .map_or_else(unknown, |amps| format!("{amps:.1} A"));
    let remaining = battery
        .and_then(|battery| battery.remaining)
        .map_or_else(unknown, |percent| format!("{percent:.0} %"));

    stats(vec![
        ui::stat("Roll", ui::text(roll)),
        ui::stat("Pitch", ui::text(pitch)),
        ui::stat("Heading", ui::text(yaw)),
        ui::stat("Altitude", ui::text(altitude)),
        ui::stat("Speed", ui::text(speed)),
        ui::stat("Battery", ui::text(remaining)),
        ui::stat("Voltage", ui::text(voltage)),
        ui::stat("Current", ui::text(current)),
    ])
}

fn view_status_texts(vehicle: &Vehicle) -> Element<'_, Message> {
    let entries = vehicle.status.texts.iter().enumerate().collect::<Vec<_>>();

    let list = virtual_list(entries, |&(index, status)| {
        timeline_row(
            index,
            format!("c{}", status.component),
            status.severity,
            status.text.clone(),
        )
    })
    .item_height(|_| ui::size::LIST_ROW)
    .key(|(_, status)| (status.component, status.at));

    scrollable(list)
        .anchor_bottom()
        .height(Length::Fill)
        .width(Length::Fill)
        .boxed()
}

/// A short, legible marker for a vehicle's liveness, and its tone.
fn liveness(liveness: Liveness) -> (&'static str, Tone) {
    match liveness {
        Liveness::Active => ("active", Tone::Success),
        Liveness::Stale => ("stale", Tone::Warning),
        Liveness::Lost => ("lost", Tone::Danger),
    }
}
