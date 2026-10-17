//! The calibration panel: run the selected vehicle's sensor calibrations and
//! follow them step by step.
//!
//! The running calibration is vehicle state (a [`Session`] in the snapshot,
//! advanced by the fleet runtime as the vehicle reports); this panel only
//! starts, confirms and cancels, and shows what the session says to do.

use groundhog_logic::{
    SystemId,
    domain::{
        calibration::{CalibrationKind, CompassStatus, Instruction, Phase, Session, Side},
        fleet::FleetSnapshot,
        vehicle::{Liveness, Vehicle},
    },
    services::{Error, Result},
};
use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{Column, column, row, scrollable, space},
};

use super::{
    Context,
    notifications::Notice,
    operations::{Operations, Ticket},
};
use crate::ui::{self, Chip, TextExt, Tone};

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

#[derive(Default)]
pub struct CalibrationPanel {
    operations: Operations<Key>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Start(SystemId, CalibrationKind),
    /// The vehicle is where the calibration asked for it.
    Confirm(SystemId),
    Cancel(SystemId),
    /// Clear a finished calibration.
    Dismiss(SystemId),
    Finished(Key, Ticket, Result<()>),
}

impl CalibrationPanel {
    pub fn update(&mut self, message: Message, cx: &mut Context<Message>) -> Task<Message> {
        match message {
            Message::Start(system, kind) => {
                let client = cx.services.vehicle(system);
                return self.run((system, Action::Start), async move {
                    client.start_calibration(kind).await
                });
            }
            Message::Confirm(system) => {
                let client = cx.services.vehicle(system);
                return self.run((system, Action::Confirm), async move {
                    client.confirm_calibration_step().await
                });
            }
            Message::Cancel(system) => {
                // A calibration that runs inside its start command (ArduPilot
                // airspeed) stops with it.
                self.operations.cancel(&(system, Action::Start));
                let client = cx.services.vehicle(system);
                return self.run((system, Action::Cancel), async move {
                    client.cancel_calibration().await
                });
            }
            Message::Dismiss(system) => cx.services.vehicle(system).dismiss_calibration(),
            Message::Finished(key, ticket, result) => {
                if self.operations.finish(&key, ticket)
                    && let Err(error) = result
                {
                    notify(cx, key, error);
                }
            }
        }

        Task::none()
    }

    fn run(
        &mut self,
        key: Key,
        operation: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Task<Message> {
        self.operations
            .start(key, operation, move |ticket, result| {
                Message::Finished(key, ticket, result)
            })
    }

    fn is_busy(&self, system: SystemId, action: Action) -> bool {
        self.operations.is_running(&(system, action))
    }

    pub fn view<'a>(
        &'a self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
    ) -> Element<'a, Message> {
        let Some(vehicle) = selected.and_then(|id| fleet.vehicle(id)) else {
            return ui::empty("No vehicle. Connect a link to begin.");
        };

        let firmware = vehicle.firmware();
        let kinds = vehicle.calibration_kinds();
        if kinds.is_empty() {
            return ui::empty(format!(
                "Calibration is not available for {} vehicles.",
                firmware.name()
            ));
        }

        let blocker = if vehicle.armed() == Some(true) {
            Some("Disarm the vehicle to calibrate.")
        } else if vehicle.liveness == Liveness::Lost {
            Some("The vehicle is not responding.")
        } else {
            None
        };

        let running = vehicle
            .calibration
            .as_ref()
            .is_some_and(Session::is_running);
        let can_start =
            blocker.is_none() && !running && !self.is_busy(vehicle.system, Action::Start);

        let calibrations = kinds.into_iter().map(|kind| {
            row![
                column![ui::text(kind.title()), ui::caption(kind.preparation())]
                    .spacing(ui::space::XXS)
                    .width(Length::Fill),
                ui::button("Start")
                    .primary()
                    .on_press_maybe(can_start.then_some(Message::Start(vehicle.system, kind)))
                    .boxed(),
            ]
            .spacing(ui::space::M)
            .align_y(Alignment::Center)
            .boxed()
        });

        let contents = column![
            row![
                ui::title(vehicle.display_name()),
                space::horizontal(),
                ui::caption(firmware.name()).tone(Tone::Muted),
            ]
            .align_y(Alignment::Center),
            blocker.map(|blocker| ui::text(blocker).tone(Tone::Warning)),
            vehicle
                .calibration
                .as_ref()
                .map(|session| self.view_session(vehicle, session)),
            ui::divider(),
            Column::with_children(calibrations).spacing(ui::space::M),
        ]
        .spacing(ui::space::M);

        scrollable(ui::panel(contents)).height(Length::Fill).boxed()
    }

    fn view_session<'a>(&self, vehicle: &'a Vehicle, session: &'a Session) -> Element<'a, Message> {
        let system = vehicle.system;
        let controls = vehicle.calibration_controls();
        let running = session.is_running();

        let (phase, tone) = match &session.phase {
            Phase::Starting => ("Starting…", Tone::Muted),
            Phase::Running => ("Running", Tone::Accent),
            Phase::Succeeded => ("Done", Tone::Success),
            Phase::Failed(_) => ("Failed", Tone::Danger),
            Phase::Cancelled => ("Cancelled", Tone::Warning),
        };

        let mut card = column![
            row![
                ui::title(session.kind.title()),
                space::horizontal(),
                ui::text(phase).tone(tone),
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(ui::space::M);

        if running || session.progress.is_some() {
            let progress = session.progress.map_or(0.0, f32::from);
            card = card.push(ui::progress(progress, 100.0).boxed());
        }

        if let Phase::Failed(reason) = &session.phase {
            card = card.push(ui::text(reason).tone(Tone::Danger).boxed());
        }

        if running {
            card = card.push(ui::text(session.instruction.text()).boxed());

            if controls.confirm {
                card = card.push(
                    ui::button("In position")
                        .primary()
                        .busy(self.is_busy(system, Action::Confirm), "Confirming…")
                        .on_press(Message::Confirm(system))
                        .boxed(),
                );
            }
        }

        if matches!(session.kind, CalibrationKind::Accelerometer)
            || (session.kind == CalibrationKind::Compass && session.sides.count() > 0)
        {
            card = card.push(view_sides(session));
        }

        for compass in &session.compasses {
            let state = match compass.status {
                CompassStatus::Running => format!("{}%", compass.completion),
                CompassStatus::Succeeded => match compass.fitness {
                    Some(fitness) => format!("done (fitness {fitness:.1})"),
                    None => "done".to_string(),
                },
                CompassStatus::Failed(reason) => format!("failed: {reason}"),
            };
            card = card.push(ui::caption(format!("Compass {}: {state}", compass.id + 1)).boxed());
        }

        if session.needs_reboot && session.phase == Phase::Succeeded {
            card = card.push(
                ui::text("Reboot the autopilot to apply the new calibration.")
                    .tone(Tone::Warning)
                    .boxed(),
            );
        }

        let skip = session.messages.len().saturating_sub(MESSAGES);
        for message in session.messages.iter().skip(skip) {
            card = card.push(ui::caption(message).boxed());
        }

        let buttons = if running {
            let stoppable = controls.cancel;
            row![
                ui::button(if stoppable { "Cancel" } else { "Abandon" })
                    .danger()
                    .on_press_maybe(
                        (!self.is_busy(system, Action::Cancel)).then_some(Message::Cancel(system))
                    )
                    .boxed(),
                (!stoppable).then(|| {
                    ui::caption("The vehicle keeps waiting until it reboots or a new run starts.")
                }),
            ]
        } else {
            row![
                ui::button("Dismiss")
                    .on_press(Message::Dismiss(system))
                    .boxed()
            ]
        };

        ui::card(
            card.push(
                buttons
                    .spacing(ui::space::M)
                    .align_y(Alignment::Center)
                    .boxed(),
            ),
        )
    }
}

/// The six orientations: done, current, or still to do.
fn view_sides(session: &Session) -> Element<'_, Message> {
    let current = match session.instruction {
        Instruction::Place(side) | Instruction::Measuring(side) => Some(side),
        Instruction::Rotate(side) => side,
        _ => None,
    };

    let chips = Side::ALL.into_iter().map(|side| {
        let state = if session.sides.is_done(side) {
            Chip::Done
        } else if current == Some(side) && session.is_running() {
            Chip::Current
        } else {
            Chip::Pending
        };
        ui::chip(side.label(), state)
    });

    row(chips).spacing(ui::space::XS).wrap().boxed()
}

fn notify(cx: &mut Context<Message>, (system, action): Key, error: Error) {
    let what = match action {
        // A refused or unanswered start ends the session, which announces
        // itself; only failures before there is a session are news.
        Action::Start => match error {
            Error::CalibrationUnsupported | Error::UnknownVehicle => "Starting the calibration",
            _ => return,
        },
        Action::Confirm => "Confirming the orientation",
        Action::Cancel => "Cancelling the calibration",
    };

    cx.notices.push(
        Notice::error(format!(
            "{what} on {} failed",
            cx.fleet.vehicle_name(system)
        ))
        .detail(error)
        .vehicle(system),
    );
}
