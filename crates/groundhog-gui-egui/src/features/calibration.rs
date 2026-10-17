//! The calibration panel: run the selected vehicle's sensor calibrations and
//! follow them step by step.
//!
//! Ported from the iced frontend. The running calibration is vehicle state (a
//! [`Session`] in the snapshot, advanced by the fleet runtime as the vehicle
//! reports); this panel only starts, confirms and cancels, and shows what the
//! session says to do.

use groundhog_logic::{
    SystemId,
    domain::{
        calibration::{CalibrationKind, CompassStatus, Instruction, Phase, Session, Side},
        fleet::FleetSnapshot,
        vehicle::{Liveness, Vehicle},
    },
    services::{Error, Result},
};
use tokio::sync::mpsc;

use super::{
    Context,
    operations::{Operations, Ticket},
};
use crate::style;

/// How many of the vehicle's calibration texts the panel shows.
const MESSAGES: usize = 6;

/// What the panel asks of a vehicle, one of each at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Start,
    Confirm,
    Cancel,
}

type Key = (SystemId, Action);

/// What the panel's operations report back.
struct RunFinished {
    system: SystemId,
    action: Action,
    ticket: Ticket,
    result: Result<()>,
}

/// The state: operations in flight, and the channel their results arrive on.
pub struct CalibrationPanel {
    runs: Operations<Key>,
    sender: mpsc::UnboundedSender<RunFinished>,
    results: mpsc::UnboundedReceiver<RunFinished>,
}

impl Default for CalibrationPanel {
    fn default() -> Self {
        let (sender, results) = mpsc::unbounded_channel();
        Self {
            runs: Operations::default(),
            sender,
            results,
        }
    }
}

impl CalibrationPanel {
    /// Drain the runs' results: stale results are dropped, failures become
    /// notices. A refused or unanswered start announces itself through the
    /// session's own finish event, so only the earliest failures are news.
    pub fn collect(&mut self, cx: &mut Context) {
        while let Ok(outcome) = self.results.try_recv() {
            let RunFinished {
                system,
                action,
                ticket,
                result,
            } = outcome;
            if !self.runs.finish(&(system, action), ticket) {
                continue;
            }

            let Err(error) = result else { return };

            let what = match action {
                Action::Start => match error {
                    Error::CalibrationUnsupported | Error::UnknownVehicle => {
                        "Starting the calibration"
                    }
                    // The session announces any other start failure.
                    _ => continue,
                },
                Action::Confirm => "Confirming the orientation",
                Action::Cancel => "Cancelling the calibration",
            };

            cx.notices.push(
                super::failure_notice(
                    format!("{what} on {} failed", cx.fleet.vehicle_name(system)),
                    error,
                )
                .vehicle(system),
            );
        }
    }

    /// The panel: blockers, the running session, and the kinds to start.
    pub fn view(
        &mut self,
        ui: &mut egui::Ui,
        fleet: &FleetSnapshot,
        selected: Option<SystemId>,
        cx: &mut Context,
    ) {
        let Some(vehicle) = selected.and_then(|id| fleet.vehicle(id)) else {
            ui.add_space(2.0);
            ui.label("No vehicle. Connect a link to begin.");
            return;
        };

        let firmware = vehicle.firmware();
        let kinds = vehicle.calibration_kinds();
        if kinds.is_empty() {
            ui.label(format!(
                "Calibration is not available for {} vehicles.",
                firmware.name()
            ));
            return;
        }

        ui.horizontal(|ui| {
            ui.strong(vehicle.display_name());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |row| {
                row.label(egui::RichText::new(firmware.name()).small().weak());
            });
        });

        let blocker = if vehicle.armed() == Some(true) {
            Some("Disarm the vehicle to calibrate.")
        } else if vehicle.liveness == Liveness::Lost {
            Some("The vehicle is not responding.")
        } else {
            None
        };
        if let Some(blocker) = blocker {
            ui.label(egui::RichText::new(blocker).color(style::severity::WARNING));
        }

        if let Some(session) = &vehicle.calibration {
            self.view_session(ui, vehicle, session, cx);
        }

        let running = vehicle
            .calibration
            .as_ref()
            .is_some_and(Session::is_running);
        let can_start =
            blocker.is_none() && !running && !self.is_busy(vehicle.system, Action::Start);

        ui.separator();
        for kind in kinds {
            // A copy for the async closure: the start command must escape
            // this frame's draw.
            let started = move |client: groundhog_logic::services::VehicleClient| async move {
                client.start_calibration(kind).await
            };
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.strong(kind.title());
                    ui.small(kind.preparation());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |row| {
                    if row
                        .add_enabled(can_start, egui::Button::new("Start"))
                        .clicked()
                    {
                        self.start_run(vehicle.system, Action::Start, cx, started);
                    }
                });
            });
            ui.add_space(2.0);
        }
    }

    fn view_session(
        &mut self,
        ui: &mut egui::Ui,
        vehicle: &Vehicle,
        session: &Session,
        cx: &mut Context,
    ) {
        let system = vehicle.system;
        let controls = vehicle.calibration_controls();

        let (phase, color) = match &session.phase {
            Phase::Starting => ("Starting…", ui.visuals().text_color()),
            Phase::Running => ("Running", style::map::SELECTED),
            Phase::Succeeded => ("Done", style::map::HOME),
            Phase::Failed(_) => ("Failed", style::severity::CRITICAL),
            Phase::Cancelled => ("Cancelled", style::severity::WARNING),
        };

        ui.add_space(4.0);
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .inner_margin(10.0)
            .show(ui, |card| {
                card.horizontal(|ui| {
                    ui.strong(session.kind.title());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |row| {
                        row.label(egui::RichText::new(phase).color(color));
                    });
                });

                if session.is_running() || session.progress.is_some() {
                    let progress = session.progress.map_or(0.0, f32::from);
                    card.add(egui::ProgressBar::new(progress / 100.0).desired_height(6.0));
                }

                if let Phase::Failed(reason) = &session.phase {
                    card.label(egui::RichText::new(reason).color(style::severity::CRITICAL));
                }

                if session.is_running() {
                    card.label(egui::RichText::new(session.instruction.text()).size(14.0));

                    if controls.confirm {
                        let busy = self.is_busy(system, Action::Confirm);
                        let label = if busy { "Confirming…" } else { "In position" };
                        if card.add_enabled(!busy, egui::Button::new(label)).clicked() {
                            self.start_run(system, Action::Confirm, cx, |client| async move {
                                client.confirm_calibration_step().await
                            });
                        }
                    }
                }

                if matches!(session.kind, CalibrationKind::Accelerometer)
                    || (session.kind == CalibrationKind::Compass && session.sides.count() > 0)
                {
                    view_sides(card, session);
                }

                for compass in &session.compasses {
                    let state = match compass.status {
                        CompassStatus::Running => format!("{}%", compass.completion),
                        CompassStatus::Succeeded => match compass.fitness {
                            Some(fitness) => format!("done (fitness {fitness:.1})"),
                            None => "done".to_owned(),
                        },
                        CompassStatus::Failed(ref reason) => format!("failed: {reason}"),
                    };
                    card.label(
                        egui::RichText::new(format!("Compass {}: {state}", compass.id + 1))
                            .size(12.0),
                    );
                }

                if session.needs_reboot && session.phase == Phase::Succeeded {
                    card.label(
                        egui::RichText::new("Reboot the autopilot to apply the new calibration.")
                            .color(style::severity::WARNING),
                    );
                }

                let skip = session.messages.len().saturating_sub(MESSAGES);
                for message in session.messages.iter().skip(skip) {
                    card.label(egui::RichText::new(message).size(11.0).weak());
                }

                // Cancel while running, dismiss afterwards. A calibration the
                // vehicle cannot stop waits for a reboot or a new run.
                if session.is_running() {
                    let stoppable = controls.cancel;
                    card.horizontal(|row| {
                        if row
                            .add_enabled(
                                !self.is_busy(system, Action::Cancel),
                                egui::Button::new(if stoppable { "Cancel" } else { "Abandon" }),
                            )
                            .clicked()
                        {
                            self.cancel_run(system, cx);
                        }
                        if !stoppable {
                            row.label(
                            egui::RichText::new(
                                "The vehicle keeps waiting until it reboots or a new run starts.",
                            )
                            .small(),
                        );
                        }
                    });
                } else if card.button("Dismiss").clicked() {
                    cx.services.vehicle(system).dismiss_calibration();
                }
            });
    }

    fn is_busy(&self, system: SystemId, action: Action) -> bool {
        self.runs.is_running(&(system, action))
    }

    /// Run one panel action, one per `action` kind per vehicle.
    fn start_run<Run, Fut>(&mut self, system: SystemId, action: Action, cx: &mut Context, run: Run)
    where
        Run: FnOnce(groundhog_logic::services::VehicleClient) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let client = cx.services.vehicle(system);
        let sender = self.sender.clone();
        self.runs.start(
            (system, action),
            run(client),
            sender,
            move |ticket, result| RunFinished {
                system,
                action,
                ticket,
                result,
            },
        );
    }

    /// Cancel the running calibration. One that runs inside its start
    /// command (ArduPilot airspeed) stops with it.
    fn cancel_run(&mut self, system: SystemId, cx: &mut Context) {
        self.runs.cancel(&(system, Action::Start));
        self.start_run(system, Action::Cancel, cx, |client| async move {
            client.cancel_calibration().await
        });
    }
}

/// The six orientations: done, current, or still to do.
fn view_sides(ui: &mut egui::Ui, session: &Session) {
    let current = match session.instruction {
        Instruction::Place(side) | Instruction::Measuring(side) => Some(side),
        Instruction::Rotate(side) => side,
        _ => None,
    };

    ui.horizontal_wrapped(|ui| {
        for side in Side::ALL {
            let done = session.sides.is_done(side);
            let text_color = if done {
                style::map::HOME
            } else if current == Some(side) && session.is_running() {
                style::map::SELECTED
            } else {
                ui.visuals().text_color()
            };

            let chip = egui::RichText::new(side.label()).color(text_color).small();
            let fill = if done {
                egui::Color32::from_rgba_premultiplied(0x6f, 0xce, 0x94, 46)
            } else if current == Some(side) && session.is_running() {
                egui::Color32::from_rgba_premultiplied(0x4c, 0xb2, 0xff, 46)
            } else {
                egui::Color32::TRANSPARENT
            };
            ui.add(egui::Button::new(chip).fill(fill));
        }
    });
}
