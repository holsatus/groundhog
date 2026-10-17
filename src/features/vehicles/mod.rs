//! The vehicles feature: the selected vehicle's state and controls, the
//! vehicle picker, and the vehicles' map layers.

mod map;

use iced::{
    Alignment, Element, Length, Task, Theme, Widget,
    widget::{button, column, container, pick_list, row, rule, scrollable, text, text::Wrapping},
};

use super::{
    Context, Effect, confirm::Confirmation, notifications::Notice, operations::Operations,
};
use crate::{
    domain::{
        SystemId,
        firmware::{FlightMode, ModeKind},
        fleet::FleetSnapshot,
        vehicle::{Liveness, Vehicle},
    },
    services::Result,
    style,
    widget::virtualized::virtual_list,
};

/// Height of a single status row: one line of text plus the container padding.
const STATUS_ROW_HEIGHT: f32 = 26.0;

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
                let name = vehicle_name(cx.fleet, system);
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
                let name = vehicle_name(cx.fleet, system);
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
                            vehicle_name(cx.fleet, system)
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
        theme: &Theme,
    ) -> Element<'a, Message> {
        let mut header = row![
            iced::widget::checkbox(follow)
                .label("Follow")
                .on_toggle(Message::Follow)
        ]
        .spacing(10.0)
        .align_y(Alignment::Center);

        if !fleet.is_empty() {
            let picker = pick_list(selected, fleet.ids().collect::<Vec<_>>(), move |id| {
                fleet.vehicle(*id).map_or_else(
                    || "Unknown vehicle".to_string(),
                    |vehicle| {
                        format!(
                            "{} ({})",
                            vehicle.display_name(),
                            liveness_label(vehicle.liveness)
                        )
                    },
                )
            })
            .on_select(Message::Select)
            .style(style::pick_list::default)
            .width(Length::Fill);
            header = header.push(picker.boxed());
        }

        let Some(vehicle) = selected.and_then(|id| fleet.vehicle(id)) else {
            return container(
                column![
                    header,
                    rule::horizontal(1.0),
                    text("No vehicle. Connect a link to begin.")
                ]
                .spacing(10.0),
            )
            .padding(8.0)
            .width(Length::Fill)
            .boxed();
        };

        let mut contents = column![
            header,
            rule::horizontal(1.0),
            self.view_status(vehicle, theme),
            rule::horizontal(1.0),
            self.view_controls(vehicle),
            rule::horizontal(1.0),
            view_telemetry(vehicle),
            rule::horizontal(1.0),
        ]
        .spacing(10.0)
        .width(Length::Fill);

        contents = contents.push(view_status_texts(vehicle, theme));

        container(contents).padding(8.0).width(Length::Fill).boxed()
    }

    fn view_status<'a>(&self, vehicle: &'a Vehicle, theme: &Theme) -> Element<'a, Message> {
        let palette = theme.palette();
        let (liveness, liveness_color) = match vehicle.liveness {
            Liveness::Active => ("active", palette.success.base.color),
            Liveness::Stale => ("stale", palette.warning.base.color),
            Liveness::Lost => ("lost", palette.danger.base.color),
        };

        let armed = match vehicle.armed() {
            Some(true) => text("ARMED").color(palette.danger.base.color),
            Some(false) => text("disarmed"),
            None => text("unknown"),
        };

        let mode = vehicle
            .flight_mode()
            .map_or_else(|| "unknown".to_string(), |mode| mode.name.to_string());

        let state = vehicle
            .identity
            .mav_state
            .map_or_else(|| "unknown".to_string(), |state| format!("{state:?}"));

        column![
            row![
                cell("Link", text(liveness).color(liveness_color)),
                cell("Armed", armed),
                cell("Mode", text(mode)),
            ]
            .spacing(10.0),
            row![
                cell("Firmware", text(vehicle.firmware().name())),
                cell("Type", text(vehicle.class().name())),
                cell("State", text(state)),
            ]
            .spacing(10.0),
        ]
        .spacing(6.0)
        .boxed()
    }

    fn view_controls<'a>(&'a self, vehicle: &'a Vehicle) -> Element<'a, Message> {
        let system = vehicle.system;
        let busy = |command| self.commands.is_running(&(system, command));
        let firmware = vehicle.firmware();
        let class = vehicle.class();

        let arm = match vehicle.armed() {
            Some(true) => button(text(if busy(Command::Disarm) {
                "Disarming…"
            } else {
                "Disarm"
            }))
            .style(button::danger)
            .on_press_maybe((!busy(Command::Disarm)).then_some(Message::RequestArm(system, false))),
            _ => button(text(if busy(Command::Arm) {
                "Arming…"
            } else {
                "Arm"
            }))
            .style(button::primary)
            .on_press_maybe((!busy(Command::Arm)).then_some(Message::RequestArm(system, true))),
        };

        let modes: Vec<FlightMode> = firmware
            .modes(class)
            .iter()
            .filter(|mode| mode.selectable)
            .copied()
            .collect();

        let mode_picker = pick_list(vehicle.flight_mode(), modes, |mode| mode.name.to_string())
            .placeholder("Mode")
            .on_select(move |mode| Message::SetMode(system, mode))
            .style(style::pick_list::default)
            .width(Length::Fill);

        let quick = |kind: ModeKind, label: &'static str| {
            button(text(label))
                .style(style::button::transparent)
                .on_press_maybe(
                    (firmware.mode_of_kind(class, kind).is_some() && !busy(Command::SetMode))
                        .then_some(Message::RequestModeKind(system, kind)),
                )
        };

        column![
            row![arm, mode_picker]
                .spacing(6.0)
                .align_y(Alignment::Center),
            row![
                quick(ModeKind::ReturnToLaunch, "Return"),
                quick(ModeKind::Land, "Land"),
                button(text("Locate"))
                    .style(style::button::transparent)
                    .on_press(Message::Locate(system)),
            ]
            .spacing(6.0)
            .wrap(),
        ]
        .spacing(6.0)
        .boxed()
    }
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

    column![
        row![
            cell("Roll", text(roll)),
            cell("Pitch", text(pitch)),
            cell("Heading", text(yaw)),
        ]
        .spacing(10.0),
        row![
            cell("Altitude", text(altitude)),
            cell("Speed", text(speed)),
            cell("Battery", text(remaining)),
        ]
        .spacing(10.0),
        row![
            cell("Voltage", text(voltage)),
            cell("Current", text(current)),
            cell("", text("")),
        ]
        .spacing(10.0),
    ]
    .spacing(6.0)
    .boxed()
}

fn view_status_texts<'a>(vehicle: &'a Vehicle, theme: &Theme) -> Element<'a, Message> {
    let palette = *theme.palette();
    let entries = vehicle.status.texts.iter().enumerate().collect::<Vec<_>>();

    let list = virtual_list(entries, move |&(index, status)| {
        let color = style::severity_color(palette, status.severity);

        container(
            row![
                text(format!("{:?}", status.severity))
                    .color(color)
                    .width(Length::Fixed(96.0)),
                text(format!("[c{}]", status.component)).width(Length::Fixed(56.0)),
                text(&status.text)
                    .width(Length::Fill)
                    .wrapping(Wrapping::None),
            ]
            .spacing(8.0),
        )
        .style(if index % 2 == 0 {
            style::list_default
        } else {
            style::list_brighter
        })
        .padding(2)
        .boxed()
    })
    .item_height(|_| STATUS_ROW_HEIGHT)
    .key(|(_, status)| (status.component, status.at))
    .spacing(0.0);

    scrollable(list)
        .anchor_bottom()
        .height(Length::Fill)
        .width(Length::Fill)
        .boxed()
}

/// A labelled value, sharing a row equally with its siblings.
fn cell<'a>(label: &'a str, value: iced::widget::Text<'a>) -> Element<'a, Message> {
    let value = value.wrapping(Wrapping::None).ellipsis(text::Ellipsis::End);
    container(column![text(label).size(11), value].spacing(1))
        .width(Length::FillPortion(1))
        .boxed()
}

/// A short, legible marker for a vehicle's liveness.
fn liveness_label(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::Active => "active",
        Liveness::Stale => "stale",
        Liveness::Lost => "lost",
    }
}

/// How notices and dialogs name a vehicle.
pub fn vehicle_name(fleet: &FleetSnapshot, system: SystemId) -> String {
    fleet
        .vehicle(system)
        .map_or_else(|| format!("vehicle {}", system.0), Vehicle::display_name)
}
