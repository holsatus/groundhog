//! A vehicle's calibration: the latest run, from its start until the operator
//! dismisses it, and every rule about how a run lives and ends.
//!
//! A run is driven from four directions. The operation running it reports
//! its start, acknowledgement, confirmations and cancel
//! ([`CalibrationUpdate`]). The vehicle's feedback advances it, through the
//! firmware's [`CalibrationProtocol`]. The fleet tells it what happened to the
//! vehicle (it rebooted, or went silent), and ages it so a stalled run fails.
//! [`Calibration`] takes all four and answers with an [`Ended`] whenever the
//! run finishes, so the fleet announces each run once without comparing the
//! run before and after every change.
//!
//! It also decides what the operator may do: start a run (only on a vehicle
//! known to be disarmed and still answering), confirm a step, cancel. A
//! frontend shows these through
//! [`Vehicle::calibration_controls`](crate::domain::vehicle::Vehicle::calibration_controls),
//! and the client refuses what they rule out.

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use mavio::default_dialect::messages::CommandLong;

use super::{
    CalibrationControls, CalibrationKind, CalibrationProtocol, CalibrationUpdate, Feedback,
    Instruction, Outcome, Phase, Session, Side,
};
use crate::domain::vehicle::Liveness;

/// How long the vehicle may say nothing about a run it is working on before
/// the run is failed.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// One run of a calibration, so that what the operation running it reports
/// cannot land on another run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunId(u64);

/// The last run id handed out; ids only need to be unique within the
/// process.
static LAST_RUN: AtomicU64 = AtomicU64::new(0);

impl RunId {
    /// A run id no other run in this process has.
    pub(crate) fn next() -> Self {
        RunId(LAST_RUN.fetch_add(1, Ordering::Relaxed) + 1)
    }
}

/// Why a calibration cannot be started now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartBlocked {
    /// The vehicle is armed, or its armed state is not known.
    NotDisarmed,
    /// The vehicle stopped answering.
    Lost,
    /// A run is already going.
    Running,
}

impl fmt::Display for StartBlocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StartBlocked::NotDisarmed => "the vehicle is not known to be disarmed",
            StartBlocked::Lost => "the vehicle is not responding",
            StartBlocked::Running => "a calibration is already running",
        })
    }
}

/// A firmware's command that stops a run on the vehicle.
#[derive(Debug, Clone)]
pub(crate) struct Cancel {
    pub(crate) command: CommandLong,
    /// How many times it is repeated: some firmwares acknowledge it twice.
    pub(crate) retries: u8,
}

/// The confirmation the vehicle waits for in the running run.
#[derive(Debug, Clone)]
pub(crate) struct Confirmation {
    pub(crate) command: CommandLong,
    pub(crate) run: RunId,
    /// The side the operator placed the vehicle on.
    pub(crate) side: Side,
}

/// A run that just finished, to be announced once.
#[derive(Debug, Clone)]
pub(crate) struct Ended {
    pub(crate) kind: CalibrationKind,
    pub(crate) outcome: Outcome,
    /// What stops the run on the vehicle, when Groundhog ended the run
    /// itself while the vehicle may still be calibrating.
    pub(crate) cancel: Option<Cancel>,
}

/// The run being followed, with what is needed to follow it.
#[derive(Clone)]
struct Run {
    id: RunId,
    session: Session,
    /// The protocol the run started under; a later heartbeat naming another
    /// autopilot does not change how this run is spoken to.
    protocol: &'static dyn CalibrationProtocol,
    /// When the run last advanced, for the stall timeout.
    progress_at: Instant,
}

/// Why Groundhog ended a run itself.
#[derive(Debug, Clone, Copy)]
enum EndReason {
    Rebooted,
    Lost,
    Stalled,
}

impl EndReason {
    fn text(self) -> &'static str {
        match self {
            EndReason::Rebooted => "the autopilot rebooted",
            EndReason::Lost => "contact with the vehicle was lost",
            EndReason::Stalled => "the calibration made no progress",
        }
    }

    /// Whether the vehicle must be told to stop: a rebooted autopilot has
    /// already dropped the run, but a lost or silent one may come back still
    /// calibrating.
    fn tells_the_vehicle(self) -> bool {
        match self {
            EndReason::Rebooted => false,
            EndReason::Lost | EndReason::Stalled => true,
        }
    }
}

/// A vehicle's latest calibration run, until dismissed.
#[derive(Clone, Default)]
pub(crate) struct Calibration {
    run: Option<Run>,
}

impl fmt::Debug for Calibration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Calibration")
            .field("session", &self.session())
            .finish()
    }
}

impl Calibration {
    /// The latest run as the operator sees it, until dismissed.
    pub(crate) fn session(&self) -> Option<&Session> {
        self.run.as_ref().map(|run| &run.session)
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running().is_some()
    }

    fn running(&self) -> Option<&Run> {
        self.run.as_ref().filter(|run| run.session.is_running())
    }

    /// What the operator can do, on a vehicle whose armed state and liveness
    /// are `armed` and `liveness`.
    pub(crate) fn controls(&self, armed: Option<bool>, liveness: Liveness) -> CalibrationControls {
        let start_blocked = if self.is_running() {
            Some(StartBlocked::Running)
        } else if armed != Some(false) {
            Some(StartBlocked::NotDisarmed)
        } else if liveness == Liveness::Lost {
            Some(StartBlocked::Lost)
        } else {
            None
        };

        let Some(run) = self.running() else {
            return CalibrationControls {
                start_blocked,
                ..CalibrationControls::default()
            };
        };
        let cancel = run.protocol.cancel(&run.session).is_some();
        CalibrationControls {
            start_blocked,
            confirm: run.protocol.confirm(&run.session).is_some(),
            cancel,
            cancel_note: (!cancel && run.session.phase == Phase::Starting)
                .then(|| run.protocol.cancel_note(run.session.kind))
                .flatten(),
        }
    }

    /// The confirmation the running run waits for, if it waits for one.
    pub(crate) fn confirmation(&self) -> Option<Confirmation> {
        let run = self.running()?;
        let Instruction::Place(side) = run.session.instruction else {
            return None;
        };
        Some(Confirmation {
            command: run.protocol.confirm(&run.session)?,
            run: run.id,
            side,
        })
    }

    /// The command that stops the running run on the vehicle, when its
    /// firmware has one.
    pub(crate) fn cancel_command(&self) -> Option<Cancel> {
        self.running().and_then(cancel_of)
    }

    /// Apply what the operation running a calibration reports. `protocol` is
    /// the vehicle's firmware's, taken by a run that begins; without one, no
    /// run can begin.
    pub(crate) fn apply(
        &mut self,
        now: Instant,
        update: CalibrationUpdate,
        protocol: Option<&'static dyn CalibrationProtocol>,
    ) -> Option<Ended> {
        if let CalibrationUpdate::Begin { kind, run } = update {
            // A run replaced by a new one is not announced as finished.
            if let Some(protocol) = protocol {
                self.run = Some(Run {
                    id: run,
                    session: Session::new(kind, protocol.completes_on_ack(kind)),
                    protocol,
                    progress_at: now,
                });
            }
            return None;
        }

        if matches!(update, CalibrationUpdate::Dismiss) {
            if !self.is_running() {
                self.run = None;
            }
            return None;
        }

        let run = self.run.as_mut()?;
        let was_running = run.session.is_running();
        let before = run.session.clone();
        match update {
            CalibrationUpdate::Begin { .. } | CalibrationUpdate::Dismiss => {}
            // What a run reports applies to that run only.
            CalibrationUpdate::Acknowledged { run: id, result } if id == run.id => {
                run.session.acknowledged(result);
            }
            CalibrationUpdate::Abandoned { run: id } if id == run.id => {
                run.session
                    .fail("The start was abandoned before the vehicle answered");
            }
            CalibrationUpdate::Confirmed { run: id, side } if id == run.id => {
                run.session.confirmed(side);
            }
            CalibrationUpdate::ConfirmFailed { run: id, side } if id == run.id => {
                run.session.confirm_failed(side);
            }
            CalibrationUpdate::Cancel => run.session.cancel(),
            CalibrationUpdate::Acknowledged { .. }
            | CalibrationUpdate::Abandoned { .. }
            | CalibrationUpdate::Confirmed { .. }
            | CalibrationUpdate::ConfirmFailed { .. } => return None,
        }

        if run.session != before {
            run.progress_at = now;
        }
        // The operation that cancelled tells the vehicle itself.
        ended(run, was_running, None)
    }

    /// Fold the vehicle's feedback into the running run.
    pub(crate) fn observe(&mut self, at: Instant, feedback: Feedback<'_>) -> Option<Ended> {
        let run = self.run.as_mut().filter(|run| run.session.is_running())?;

        // Any feedback that belongs to the run keeps it alive, even when it
        // repeats what the session already shows: a vehicle can spend a
        // minute on the same percentage while the operator turns it.
        if run.protocol.observe(&mut run.session, feedback) {
            run.progress_at = at;
        }
        ended(run, true, None)
    }

    /// The autopilot rebooted: a running run ended with it, and a finished
    /// one that waited for a reboot is now applied.
    pub(crate) fn rebooted(&mut self) -> Option<Ended> {
        if let Some(run) = self.run.as_mut()
            && !run.session.is_running()
        {
            run.session.needs_reboot = false;
            return None;
        }
        self.end(EndReason::Rebooted)
    }

    /// The vehicle stopped answering.
    pub(crate) fn lost(&mut self) -> Option<Ended> {
        self.end(EndReason::Lost)
    }

    /// Whether the running run has made no progress for too long, while the
    /// vehicle (not the operator) is the one to act. Cheap, so the fleet can
    /// ask every vehicle on every tick without copying it.
    pub(crate) fn is_stalled(&self, now: Instant) -> bool {
        self.running().is_some_and(|run| {
            !run.session.instruction.waits_for_operator()
                && now.saturating_duration_since(run.progress_at) > STALL_TIMEOUT
        })
    }

    /// Age the run: one that stalled fails.
    pub(crate) fn tick(&mut self, now: Instant) -> Option<Ended> {
        if !self.is_stalled(now) {
            return None;
        }
        self.end(EndReason::Stalled)
    }

    fn end(&mut self, reason: EndReason) -> Option<Ended> {
        let run = self.run.as_mut().filter(|run| run.session.is_running())?;

        // Read the cancel before failing the session: the protocol decides it
        // from the run that is ending.
        let cancel = reason.tells_the_vehicle().then(|| cancel_of(run)).flatten();
        run.session.fail(reason.text());
        ended(run, true, cancel)
    }
}

fn cancel_of(run: &Run) -> Option<Cancel> {
    Some(Cancel {
        command: run.protocol.cancel(&run.session)?,
        retries: run.protocol.cancel_retries(),
    })
}

/// The run's end, if it was running and no longer is.
fn ended(run: &Run, was_running: bool, cancel: Option<Cancel>) -> Option<Ended> {
    if !was_running {
        return None;
    }
    Some(Ended {
        kind: run.session.kind,
        outcome: run.session.phase.outcome()?,
        cancel,
    })
}

#[cfg(test)]
mod tests;
