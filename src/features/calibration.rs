//! The calibration panel: run the selected vehicle's sensor calibrations and
//! follow them step by step.
//!
//! The running calibration is vehicle state (a [`Session`] in the snapshot,
//! advanced by the fleet runtime as the vehicle reports); this panel only
//! starts, confirms and cancels, and shows what the session says to do.

use iced::{
    Alignment, Element, Length, Task, Theme, Widget,
    widget::{button, column, container, progress_bar, row, rule, scrollable, space, text},
};

use super::{
    Context,
    notifications::Notice,
    operations::{Operations, Ticket},
};
use crate::{
    domain::{
        SystemId,
        calibration::{
            CalibrationKind, CalibrationProtocol, CompassStatus, Instruction, Phase, Session, Side,
        },
        fleet::FleetSnapshot,
        vehicle::{Liveness, Vehicle},
    },
    services::{Error, Result},
    style,
};

use super::vehicles::vehicle_name;

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
        theme: &Theme,
    ) -> Element<'a, Message> {
        let Some(vehicle) = selected.and_then(|id| fleet.vehicle(id)) else {
            return padded(text("No vehicle. Connect a link to begin.").boxed());
        };

        let firmware = vehicle.firmware();
        let Some(protocol) = firmware.calibration() else {
            return padded(
                text(format!(
                    "Calibration is not available for {} vehicles.",
                    firmware.name()
                ))
                .boxed(),
            );
        };

        let palette = theme.palette();
        let mut contents = column![
            row![
                text(vehicle.display_name()).size(16),
                space::horizontal(),
                text(firmware.name()).size(12),
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(10.0);

        let blocker = if vehicle.armed() == Some(true) {
            Some("Disarm the vehicle to calibrate.")
        } else if vehicle.liveness == Liveness::Lost {
            Some("The vehicle is not responding.")
        } else {
            None
        };
        if let Some(blocker) = blocker {
            contents = contents.push(text(blocker).color(palette.warning.base.color).boxed());
        }

        if let Some(session) = &vehicle.calibration {
            contents = contents.push(self.view_session(vehicle, protocol, session, theme));
        }

        let running = vehicle
            .calibration
            .as_ref()
            .is_some_and(Session::is_running);
        let can_start =
            blocker.is_none() && !running && !self.is_busy(vehicle.system, Action::Start);

        contents = contents.push(rule::horizontal(1.0).boxed());
        for kind in protocol.kinds() {
            contents = contents.push(
                row![
                    column![
                        text(kind.title()).size(14),
                        text(kind.preparation()).size(12),
                    ]
                    .spacing(2.0)
                    .width(Length::Fill),
                    button(text("Start"))
                        .style(button::primary)
                        .on_press_maybe(can_start.then_some(Message::Start(vehicle.system, kind))),
                ]
                .spacing(10.0)
                .align_y(Alignment::Center)
                .boxed(),
            );
        }

        scrollable(container(contents).padding(8.0).width(Length::Fill))
            .height(Length::Fill)
            .boxed()
    }

    fn view_session<'a>(
        &self,
        vehicle: &'a Vehicle,
        protocol: &dyn CalibrationProtocol,
        session: &'a Session,
        theme: &Theme,
    ) -> Element<'a, Message> {
        let palette = theme.palette();
        let system = vehicle.system;

        let (phase, color) = match &session.phase {
            Phase::Starting => ("Starting…", palette.background.base.text),
            Phase::Running => ("Running", palette.primary.base.color),
            Phase::Succeeded => ("Done", palette.success.base.color),
            Phase::Failed(_) => ("Failed", palette.danger.base.color),
            Phase::Cancelled => ("Cancelled", palette.warning.base.color),
        };

        let mut card = column![
            row![
                text(session.kind.title()).size(16),
                space::horizontal(),
                text(phase).color(color),
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(8.0);

        if session.is_running() || session.progress.is_some() {
            let progress = session.progress.map_or(0.0, f32::from);
            card = card.push(progress_bar(0.0..=100.0, progress).girth(6.0).boxed());
        }

        if let Phase::Failed(reason) = &session.phase {
            card = card.push(text(reason).color(palette.danger.base.color).boxed());
        }

        if session.is_running() {
            card = card.push(text(session.instruction.text()).size(15).boxed());

            if protocol.confirm(session).is_some() {
                let busy = self.is_busy(system, Action::Confirm);
                card = card.push(
                    button(text(if busy { "Confirming…" } else { "In position" }))
                        .style(button::primary)
                        .on_press_maybe((!busy).then_some(Message::Confirm(system)))
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
            card = card.push(
                text(format!("Compass {}: {state}", compass.id + 1))
                    .size(13)
                    .boxed(),
            );
        }

        if session.needs_reboot && session.phase == Phase::Succeeded {
            card = card.push(
                text("Reboot the autopilot to apply the new calibration.")
                    .color(palette.warning.base.color)
                    .boxed(),
            );
        }

        let skip = session.messages.len().saturating_sub(MESSAGES);
        for message in session.messages.iter().skip(skip) {
            card = card.push(text(message).size(12).boxed());
        }

        let controls = if session.is_running() {
            let stoppable = protocol.cancel(session).is_some();
            let mut controls = row![
                button(text(if stoppable { "Cancel" } else { "Abandon" }))
                    .style(button::danger)
                    .on_press_maybe(
                        (!self.is_busy(system, Action::Cancel)).then_some(Message::Cancel(system))
                    )
            ]
            .spacing(8.0)
            .align_y(Alignment::Center);
            if !stoppable {
                controls = controls.push(
                    text("The vehicle keeps waiting until it reboots or a new run starts.")
                        .size(12)
                        .boxed(),
                );
            }
            controls
        } else {
            row![
                button(text("Dismiss"))
                    .style(button::secondary)
                    .on_press(Message::Dismiss(system))
            ]
        };

        container(card.push(controls.boxed()))
            .padding(10.0)
            .width(Length::Fill)
            .style(style::inner_container)
            .boxed()
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
            style::Chip::Done
        } else if current == Some(side) && session.is_running() {
            style::Chip::Current
        } else {
            style::Chip::Pending
        };

        container(text(side.label()).size(12))
            .padding([2.0, 8.0])
            .style(move |theme: &Theme| style::chip(theme, state))
            .boxed()
    });

    row(chips).spacing(4.0).wrap().boxed()
}

fn padded(content: Element<'_, Message>) -> Element<'_, Message> {
    container(content).padding(8.0).width(Length::Fill).boxed()
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
            vehicle_name(cx.fleet, system)
        ))
        .detail(error)
        .vehicle(system),
    );
}
