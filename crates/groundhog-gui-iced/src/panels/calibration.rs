//! The calibration panel: run the selected vehicle's sensor calibrations and
//! follow them step by step.
//!
//! The running calibration is vehicle state (a [`Session`] in the snapshot,
//! advanced by the fleet runtime as the vehicle reports); this panel only
//! starts, confirms and cancels, and shows what the session says to do. When
//! a start, a confirmation or a cancel is possible is the backend's to say
//! (`Vehicle::calibration_controls`).

use std::borrow::Cow;

use groundhog_logic::{
    SystemId,
    domain::{
        calibration::{
            CalibrationKind, CompassStatus, Instruction, Phase, Session, Side, StartBlocked,
        },
        vehicle::Vehicle,
    },
    services::{Error, Result},
};
use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{column, row, scrollable, space},
};

use super::{
    Context, Panel, ViewContext,
    confirm::Confirmation,
    operations::{Activity, Finished, Operations},
};
use crate::ui::{self, Icon, TextExt, Tone};

/// How many of the vehicle's calibration texts the panel shows.
const MESSAGES: usize = 6;

/// What the panel asks of a vehicle, one of each at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Start,
    Confirm,
    Cancel,
    Reboot,
}

type Key = (SystemId, Action);

impl Activity for Key {
    fn activity(&self, (): &()) -> Cow<'static, str> {
        match self.1 {
            Action::Start => "Starting the calibration",
            Action::Confirm => "Confirming the orientation",
            Action::Cancel => "Cancelling the calibration",
            Action::Reboot => "Rebooting the autopilot",
        }
        .into()
    }

    fn system(&self) -> SystemId {
        self.0
    }

    /// A refused or unanswered start ends the session, which announces
    /// itself; only failures before there is a session are news.
    fn announces(&self, (): &(), error: &Error) -> bool {
        self.1 != Action::Start
            || matches!(
                error,
                Error::CalibrationUnsupported
                    | Error::CalibrationBlocked(_)
                    | Error::UnknownVehicle
            )
    }
}

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
    /// Ask for confirmation, then reboot the autopilot to apply a
    /// calibration that waits for it.
    RequestReboot(SystemId),
    Reboot(SystemId),
    Finished(Finished<Key, ()>),
}

impl Panel for CalibrationPanel {
    type Message = Message;

    fn update(&mut self, message: Message, cx: &mut Context<Message>) -> Task<Message> {
        match message {
            Message::Start(system, kind) => {
                let client = cx.vehicle_client(system);
                return self.run((system, Action::Start), async move {
                    client.start_calibration(kind).await
                });
            }
            Message::Confirm(system) => {
                let client = cx.vehicle_client(system);
                return self.run((system, Action::Confirm), async move {
                    client.confirm_calibration_step().await
                });
            }
            Message::Cancel(system) => {
                // A calibration that runs inside its start command (ArduPilot
                // airspeed) stops with it.
                self.operations.cancel(&(system, Action::Start));
                let client = cx.vehicle_client(system);
                return self.run((system, Action::Cancel), async move {
                    client.cancel_calibration().await
                });
            }
            Message::Dismiss(system) => cx.vehicle_client(system).dismiss_calibration(),
            Message::RequestReboot(system) => {
                let name = cx.fleet.vehicle_name(system);
                cx.confirm(Confirmation::reboot(&name, Message::Reboot(system)));
            }
            Message::Reboot(system) => {
                let client = cx.vehicle_client(system);
                return self.run(
                    (system, Action::Reboot),
                    async move { client.reboot().await },
                );
            }
            Message::Finished(finished) => {
                // A failure is announced; the run shows in the snapshot.
                self.operations.settle(finished, cx);
            }
        }

        Task::none()
    }

    fn view<'a>(&'a self, cx: ViewContext<'a>) -> Element<'a, Message> {
        let ViewContext {
            fleet, selected, ..
        } = cx;
        let Some(vehicle) = selected.and_then(|id| fleet.vehicle(id)) else {
            return ui::empty("No vehicle. Connect a link to begin.").boxed();
        };

        let firmware = vehicle.firmware();
        let kinds = vehicle.calibration_kinds();
        if kinds.is_empty() {
            return ui::empty(format!(
                "Calibration is not available for {} vehicles.",
                firmware.name()
            ))
            .boxed();
        }

        let start_blocked = vehicle.calibration_controls().start_blocked;
        // A running calibration shows itself; only the other reasons need
        // saying.
        let blocker = start_blocked.and_then(|blocked| match blocked {
            StartBlocked::NotDisarmed => Some("Disarm the vehicle to calibrate."),
            StartBlocked::Lost => Some("The vehicle is not responding."),
            StartBlocked::Running => None,
        });
        let can_start = start_blocked.is_none() && !self.is_busy(vehicle.system, Action::Start);

        let calibrations = kinds.into_iter().map(|kind| {
            row![
                column![
                    ui::text(kind.title()),
                    ui::caption(kind.preparation()).tone(Tone::Muted)
                ]
                .spacing(ui::space::XXS)
                .width(Length::Fill),
                ui::button("Start")
                    .icon(Icon::Play)
                    .on_press_maybe(can_start.then_some(Message::Start(vehicle.system, kind)))
                    .boxed(),
            ]
            .spacing(ui::space::M)
            .align_y(Alignment::Center)
            .boxed()
        });

        let header = ui::header(
            row![
                ui::text(vehicle.display_name()),
                ui::caption(firmware.name()).tone(Tone::Muted),
            ]
            .spacing(ui::space::S)
            .align_y(Alignment::Center),
            [],
            [],
        );

        // A run under way comes first: it is what the operator is doing.
        let contents = column![
            blocker.map(|blocker| ui::banner(blocker, Tone::Warning)),
            vehicle
                .calibration()
                .map(|session| self.view_session(vehicle, session)),
            ui::section("Sensors", [ui::card_list(calibrations).boxed()]),
        ]
        .spacing(ui::space::L);

        ui::panel_with(header, scrollable(contents).height(Length::Fill)).boxed()
    }
}

impl CalibrationPanel {
    fn run(
        &mut self,
        key: Key,
        operation: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Task<Message> {
        self.operations.start(key, operation, Message::Finished)
    }

    fn is_busy(&self, system: SystemId, action: Action) -> bool {
        self.operations.is_running(&(system, action))
    }

    fn view_session<'a>(
        &self,
        vehicle: &'a Vehicle,
        session: &'a Session,
    ) -> impl Widget<Message> + 'a {
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
                ui::badge(phase, tone),
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
            card = card.push(view_sides(session).boxed());
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
                ui::caption(format!("Compass {}: {state}", u16::from(compass.id) + 1)).boxed(),
            );
        }

        if session.needs_reboot && session.phase == Phase::Succeeded {
            card = card.push(
                row![
                    ui::text("Reboot the autopilot to apply the new calibration.")
                        .tone(Tone::Warning)
                        .width(Length::Fill),
                    ui::button("Reboot now")
                        .icon(Icon::Reboot)
                        .primary()
                        .busy(self.is_busy(system, Action::Reboot), "Rebooting…")
                        .on_press_maybe(
                            vehicle
                                .can_reboot()
                                .then_some(Message::RequestReboot(system))
                        )
                        .boxed(),
                ]
                .spacing(ui::space::M)
                .align_y(Alignment::Center)
                .boxed(),
            );
        }

        let skip = session.messages.len().saturating_sub(MESSAGES);
        for message in session.messages.iter().skip(skip) {
            card = card.push(ui::caption(message).boxed());
        }

        let buttons = if running {
            row![
                ui::button(if controls.cancel { "Cancel" } else { "Abandon" })
                    .danger()
                    .on_press_maybe(
                        (!self.is_busy(system, Action::Cancel)).then_some(Message::Cancel(system))
                    )
                    .boxed(),
                controls.cancel_note.map(ui::caption),
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
fn view_sides(session: &Session) -> impl Widget<Message> + '_ {
    let current = match session.instruction {
        Instruction::Place(side) | Instruction::Confirming(side) | Instruction::Measuring(side) => {
            Some(side)
        }
        Instruction::Rotate(side) => side,
        _ => None,
    };

    let sides = Side::ALL.into_iter().map(|side| {
        let tone = if session.sides.is_done(side) {
            Tone::Success
        } else if current == Some(side) && session.is_running() {
            Tone::Accent
        } else {
            Tone::Muted
        };
        ui::badge(side.label(), tone)
    });

    row(sides).spacing(ui::space::XS).wrap()
}

#[cfg(test)]
mod tests;
