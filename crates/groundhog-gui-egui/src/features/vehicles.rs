//! The vehicles feature: the selected vehicle's state and controls.
//!
//! Ported from the iced frontend. Operator state is just the commands in
//! flight; everything else is read from the snapshot. Effects (select, follow,
//! locate) go through the [`Context`]; vehicle commands go through
//! [`VehicleClient`](groundhog_logic::services::VehicleClient).

use groundhog_logic::{
    SystemId,
    domain::{
        firmware::{FlightMode, ModeKind},
        fleet::FleetSnapshot,
        vehicle::{Liveness, Vehicle},
    },
    services::Result,
};
use tokio::sync::mpsc;

use super::{
    Context, Effect, failure_notice,
    operations::{Operations, Ticket},
};
use crate::style;

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

/// What the panel's operations report back.
enum CommandEvent {
    Finished(SystemId, Command, Ticket, Result<()>),
}

/// The state: commands in flight, and the channel their results arrive on.
pub struct VehiclePanel {
    commands: Operations<Key>,
    sender: mpsc::UnboundedSender<CommandEvent>,
    results: mpsc::UnboundedReceiver<CommandEvent>,
}

impl Default for VehiclePanel {
    fn default() -> Self {
        Self::new()
    }
}

impl VehiclePanel {
    pub fn new() -> Self {
        let (sender, results) = mpsc::unbounded_channel();
        Self {
            commands: Operations::default(),
            sender,
            results,
        }
    }

    /// Drain the commands' results: stale results are dropped, failures
    /// become notices.
    pub fn collect(&mut self, cx: &mut Context) {
        while let Ok(event) = self.results.try_recv() {
            let CommandEvent::Finished(system, command, ticket, result) = event;
            if self.commands.finish(&(system, command), ticket)
                && let Err(error) = result
            {
                cx.notices.push(
                    failure_notice(
                        format!(
                            "{} {} failed",
                            command.verb(),
                            cx.fleet.vehicle_name(system)
                        ),
                        error,
                    )
                    .vehicle(system),
                );
            }
        }
    }

    /// Run an already-confirmed arm (or disarm).
    pub fn perform_arm(&mut self, cx: &Context, system: SystemId, arm: bool) {
        let command = if arm { Command::Arm } else { Command::Disarm };
        let client = cx.services.vehicle(system);
        let sender = self.sender.clone();
        self.commands.start(
            (system, command),
            async move { client.arm(arm).await },
            sender,
            move |ticket, result| CommandEvent::Finished(system, command, ticket, result),
        );
    }

    /// Run an already-confirmed mode-of-kind command (return, land, ...).
    pub fn perform_mode_kind(&mut self, cx: &Context, system: SystemId, kind: ModeKind) {
        let client = cx.services.vehicle(system);
        let sender = self.sender.clone();
        self.commands.start(
            (system, Command::SetMode),
            async move { client.set_mode_kind(kind).await.map(|_| ()) },
            sender,
            move |ticket, result| CommandEvent::Finished(system, Command::SetMode, ticket, result),
        );
    }

    /// The panel: picker, status, controls, telemetry and status texts.
    pub fn view(
        &mut self,
        ui: &mut egui::Ui,
        fleet: &FleetSnapshot,
        selected: Option<SystemId>,
        follow: bool,
        cx: &mut Context,
    ) {
        // The picker and the follow toggle: which vehicle the UI shows, and
        // whether the map follows it.
        ui.horizontal(|ui| {
            let picker_text = selected.and_then(|id| fleet.vehicle(id)).map_or_else(
                || "No vehicle selected".to_owned(),
                |vehicle| {
                    format!(
                        "{} ({})",
                        vehicle.display_name(),
                        liveness_label(vehicle.liveness)
                    )
                },
            );

            egui::ComboBox::from_id_salt("vehicles.picker")
                .selected_text(egui::RichText::new(picker_text).weak())
                .width(ui.available_width() - 90.0)
                .show_ui(ui, |ui| {
                    let mut picked = None;
                    for id in fleet.ids() {
                        let label = fleet.vehicle(id).map_or_else(
                            || "unknown".to_owned(),
                            |vehicle| {
                                format!(
                                    "{} ({})",
                                    vehicle.display_name(),
                                    liveness_label(vehicle.liveness)
                                )
                            },
                        );
                        if ui.selectable_label(selected == Some(id), label).clicked() {
                            picked = Some(id);
                        }
                    }

                    if let Some(id) = picked {
                        cx.request(Effect::SelectVehicle(id));
                    }
                });

            let toggled = ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::selectable(follow, "Follow"),
                )
                .clicked();
            if toggled {
                cx.request(Effect::FollowVehicle(!follow));
            }
        });

        let Some(vehicle) = selected.and_then(|id| fleet.vehicle(id)) else {
            ui.separator();
            ui.label(match fleet.is_empty() {
                true => "No vehicle. Connect a link to begin.",
                false => "No vehicle selected.",
            });
            return;
        };

        ui.separator();
        view_status(ui, vehicle);
        ui.separator();
        self.view_controls(ui, vehicle, cx);
        ui.separator();
        view_telemetry(ui, vehicle);
        ui.separator();
        view_status_texts(ui, vehicle);
    }

    /// Arm / disarm, the mode picker, quick mode kinds and locate.
    fn view_controls(&mut self, ui: &mut egui::Ui, vehicle: &Vehicle, cx: &mut Context) {
        let system = vehicle.system;
        let firmware = vehicle.firmware();
        let class = vehicle.class();

        let disarming = self.control_commands(system, Command::Disarm);
        let arming = self.control_commands(system, Command::Arm);
        let armed = vehicle.armed() == Some(true);
        let (arm_label, arm_busy, danger) = if armed {
            ("Disarm", disarming, true)
        } else {
            ("Arm", arming, false)
        };

        let mode_busy = self.control_commands(system, Command::SetMode);
        let modes: Vec<FlightMode> = firmware
            .modes(class)
            .iter()
            .filter(|mode| mode.selectable)
            .copied()
            .collect();

        // The arm button and the mode picker share the first row.
        ui.horizontal_wrapped(|ui| {
            let fill = if danger {
                style::severity::CRITICAL.linear_multiply(0.75)
            } else {
                style::map::SELECTED.linear_multiply(0.6)
            };
            let arm = ui
                .add_enabled(
                    !arm_busy,
                    egui::Button::new(egui::RichText::new(arm_label).strong()).fill(fill),
                )
                .clicked();
            if arm && !arm_busy {
                request_arm(cx, system, !armed);
            }

            let mut picked_mode = None;
            egui::ComboBox::from_id_salt("vehicles.mode")
                .selected_text(
                    vehicle
                        .flight_mode()
                        .map_or_else(|| "Mode".to_owned(), |mode| mode.name.to_owned()),
                )
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    for mode in &modes {
                        if ui
                            .add_enabled(!mode_busy, egui::Button::new(mode.name))
                            .clicked()
                        {
                            picked_mode = Some(*mode);
                        }
                    }
                });
            if let Some(mode) = picked_mode {
                let client = cx.services.vehicle(system);
                let sender = self.sender.clone();
                self.commands.start(
                    (system, Command::SetMode),
                    async move { client.set_mode(mode).await },
                    sender,
                    move |ticket, result| {
                        CommandEvent::Finished(system, Command::SetMode, ticket, result)
                    },
                );
            }
        });

        // Quick mode kinds and locating.
        ui.horizontal_wrapped(|ui| {
            for (kind, label) in [
                (ModeKind::ReturnToLaunch, "Return"),
                (ModeKind::Land, "Land"),
            ] {
                let response = ui.add_enabled(
                    firmware.mode_of_kind(class, kind).is_some() && !mode_busy,
                    egui::Button::new(label),
                );
                if response.clicked() {
                    request_mode_kind(cx, system, kind);
                }
            }

            if ui.button("Locate").clicked()
                && let Some(position) = vehicle.position.current.as_ref()
            {
                cx.request(Effect::CenterMap(position.pos));
            }
        });
    }

    /// Whether a command of this kind is in flight for the vehicle.
    fn control_commands(&self, system: SystemId, command: Command) -> bool {
        self.commands.is_running(&(system, command))
    }
}

/// Ask before arming: motors may spin up, or the vehicle may drop out of
/// flight. Never sent on a single click.
fn request_arm(cx: &mut Context, system: SystemId, arm: bool) {
    let name = cx.fleet.vehicle_name(system);
    let (title, body, label) = if arm {
        (
            format!("Arm {name}?"),
            "Motors may start spinning. Make sure the area around the vehicle is clear.",
            "Arm",
        )
    } else {
        (
            format!("Disarm {name}?"),
            "Disarming in flight stops the motors and the vehicle will fall.",
            "Disarm",
        )
    };

    cx.confirm(
        super::confirm::Confirmation::new(title, body, super::confirm::Action::Arm { system, arm })
            .accept_label(label)
            .danger(),
    );
}

/// Ask before commanding a consequential mode kind (return, land, ...).
fn request_mode_kind(cx: &mut Context, system: SystemId, kind: ModeKind) {
    let name = cx.fleet.vehicle_name(system);
    let (title, accept) = match kind {
        ModeKind::ReturnToLaunch => (format!("Return {name} to launch?"), "Return"),
        ModeKind::Land => (format!("Land {name} here?"), "Land"),
        _ => (format!("Change the mode of {name}?"), "Change"),
    };

    cx.confirm(
        super::confirm::Confirmation::new(
            title,
            "The vehicle leaves its current mode immediately.",
            super::confirm::Action::ModeKind { system, kind },
        )
        .accept_label(accept),
    );
}

/// The vehicle's liveness, firmware, class, arm state and mode.
fn view_status(ui: &mut egui::Ui, vehicle: &Vehicle) {
    let (liveness, liveness_color) = match vehicle.liveness {
        Liveness::Active => ("active", style::map::HOME),
        Liveness::Stale => ("stale", style::severity::WARNING),
        Liveness::Lost => ("lost", style::severity::CRITICAL),
    };

    let armed = match vehicle.armed() {
        Some(true) => egui::RichText::new("ARMED").color(style::severity::CRITICAL),
        Some(false) => egui::RichText::new("disarmed"),
        None => egui::RichText::new("unknown").weak(),
    };

    let mode = vehicle
        .flight_mode()
        .map_or_else(|| "unknown".to_owned(), |mode| mode.name.to_owned());

    let state = vehicle
        .identity
        .mav_state
        .map_or_else(|| "unknown".to_owned(), |state| format!("{state:?}"));

    egui::Grid::new("vehicle.status")
        .num_columns(6)
        .min_col_width(64.0)
        .show(ui, |ui| {
            cell(
                ui,
                "Link",
                egui::RichText::new(liveness).color(liveness_color),
            );
            cell(ui, "Armed", armed);
            cell(ui, "Mode", mode);
            ui.end_row();
            cell(ui, "Firmware", vehicle.firmware().name());
            cell(ui, "Type", vehicle.class().name());
            cell(ui, "State", state);
            ui.end_row();
        });
}

/// One labelled value cell: label above, value below, as the iced panel had.
fn cell(ui: &mut egui::Ui, label: &str, value: impl Into<egui::RichText>) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small().weak());
        ui.add(egui::Label::new(value.into()).truncate());
    });
}

/// Attitude, altitude, speed and battery.
fn view_telemetry(ui: &mut egui::Ui, vehicle: &Vehicle) {
    let unknown = || "–".to_owned();

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

    egui::Grid::new("vehicle.telemetry")
        .num_columns(6)
        .min_col_width(64.0)
        .show(ui, |ui| {
            cell(ui, "Roll", roll);
            cell(ui, "Pitch", pitch);
            cell(ui, "Heading", yaw);
            ui.end_row();
            cell(ui, "Altitude", altitude);
            cell(ui, "Speed", speed);
            cell(ui, "Battery", remaining);
            ui.end_row();
            cell(ui, "Voltage", voltage);
            cell(ui, "Current", current);
            ui.end_row();
        });
}

/// The vehicle's status texts, newest last.
///
/// Height of a single status row: one line of text plus padding.
const STATUS_ROW_HEIGHT: f32 = 22.0;

fn view_status_texts(ui: &mut egui::Ui, vehicle: &Vehicle) {
    let texts = &vehicle.status.texts;
    if texts.is_empty() {
        return;
    }

    let count = texts.len();
    ui.add_space(2.0);
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .show_rows(ui, STATUS_ROW_HEIGHT, count, |ui, range| {
            egui::Grid::new("vehicle.status_texts")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    for index in range {
                        let status = &texts[index];
                        let severity = format!("{:?}", status.severity);
                        let color = style::severity::color(status.severity);

                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(severity).color(color).monospace(),
                            )
                            .truncate(),
                        );
                        ui.add(egui::Label::new(
                            egui::RichText::new(format!("[c{}]", status.component))
                                .monospace()
                                .weak(),
                        ));
                        ui.add(egui::Label::new(&status.text).truncate());
                        ui.end_row();
                    }
                });
        });
}

/// A short, legible marker for a vehicle's liveness.
fn liveness_label(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::Active => "active",
        Liveness::Stale => "stale",
        Liveness::Lost => "lost",
    }
}
