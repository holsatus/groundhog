//! Sensor calibration: one model of a running calibration, whatever the
//! firmware.
//!
//! MAVLink standardises only the command that starts most calibrations
//! (`MAV_CMD_PREFLIGHT_CALIBRATION`), and not even consistently: ArduPilot
//! calibrates the airspeed sensor through the "ground pressure" parameter, PX4
//! through its own. How a calibration then talks to the operator is entirely
//! firmware-specific. ArduPilot asks for each accelerometer orientation with a
//! `COMMAND_LONG` and waits for the GCS to confirm it, and streams compass
//! progress as `MAG_CAL_PROGRESS`/`MAG_CAL_REPORT`. PX4 detects orientations
//! itself and narrates everything in `[cal]` status texts.
//!
//! So a [`Session`] is the firmware-neutral view the UI shows (what to do now,
//! progress, which sides are done, per-compass results), and each firmware's
//! [`CalibrationProtocol`] maps it to and from the wire: the command that
//! starts each [`CalibrationKind`], how to confirm a step or cancel, and how
//! the vehicle's feedback advances the session. The fleet runtime owns the
//! session (it lives on the [`Vehicle`](super::vehicle::Vehicle)), so
//! feedback is folded in as it arrives, independent of the UI.

pub(crate) mod ardupilot;
pub(crate) mod px4;

use std::collections::VecDeque;

use mavio::{
    DefaultDialect,
    default_dialect::{enums::MavCmd, messages::CommandLong},
};

use super::vehicle::StatusText;

/// A calibration an operator can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CalibrationKind {
    /// Six-orientation accelerometer calibration.
    Accelerometer,
    /// Level horizon: the attitude the vehicle reports as level.
    Level,
    Gyroscope,
    Compass,
    /// Airspeed sensor zero offset.
    Airspeed,
}

impl CalibrationKind {
    pub const ALL: [CalibrationKind; 5] = [
        CalibrationKind::Accelerometer,
        CalibrationKind::Compass,
        CalibrationKind::Level,
        CalibrationKind::Gyroscope,
        CalibrationKind::Airspeed,
    ];

    pub fn title(self) -> &'static str {
        match self {
            CalibrationKind::Accelerometer => "Accelerometer",
            CalibrationKind::Level => "Level horizon",
            CalibrationKind::Gyroscope => "Gyroscope",
            CalibrationKind::Compass => "Compass",
            CalibrationKind::Airspeed => "Airspeed",
        }
    }

    /// What the operator should know before starting.
    pub fn preparation(self) -> &'static str {
        match self {
            CalibrationKind::Accelerometer => {
                "Hold the vehicle still in six orientations, one after the other."
            }
            CalibrationKind::Level => "Place the vehicle level and keep it still.",
            CalibrationKind::Gyroscope => "Keep the vehicle completely still.",
            CalibrationKind::Compass => {
                "Away from metal and magnets, rotate the vehicle about every axis until done."
            }
            CalibrationKind::Airspeed => "Shield the pitot tube from wind and keep it still.",
        }
    }

    /// What the operator is asked to do first.
    fn first_instruction(self) -> Instruction {
        match self {
            CalibrationKind::Accelerometer => Instruction::Wait,
            CalibrationKind::Level | CalibrationKind::Gyroscope => Instruction::HoldStill,
            CalibrationKind::Compass => Instruction::Rotate(None),
            CalibrationKind::Airspeed => Instruction::ShieldPitot,
        }
    }
}

impl std::fmt::Display for CalibrationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.title())
    }
}

/// An orientation of the vehicle during a six-sided calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Level,
    Left,
    Right,
    NoseDown,
    NoseUp,
    Back,
}

impl Side {
    /// In ArduPilot's order, which is the order it asks for them.
    pub const ALL: [Side; 6] = [
        Side::Level,
        Side::Left,
        Side::Right,
        Side::NoseDown,
        Side::NoseUp,
        Side::Back,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Side::Level => "Level",
            Side::Left => "Left",
            Side::Right => "Right",
            Side::NoseDown => "Nose down",
            Side::NoseUp => "Nose up",
            Side::Back => "Back",
        }
    }

    /// Completes "place the vehicle …".
    pub fn placement(self) -> &'static str {
        match self {
            Side::Level => "level",
            Side::Left => "on its left side",
            Side::Right => "on its right side",
            Side::NoseDown => "nose down",
            Side::NoseUp => "nose up",
            Side::Back => "on its back",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Which of the six sides are done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sides([bool; 6]);

impl Sides {
    pub fn is_done(&self, side: Side) -> bool {
        self.0[side.index()]
    }

    pub(crate) fn mark(&mut self, side: Side) {
        self.0[side.index()] = true;
    }

    pub fn count(&self) -> usize {
        self.0.iter().filter(|done| **done).count()
    }
}

/// What the operator should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    /// Nothing; the vehicle is working or has not answered yet.
    Wait,
    HoldStill,
    /// Place the vehicle on `side`, then confirm (the vehicle waits for it).
    Place(Side),
    /// Hold the vehicle still on any side not yet done; the vehicle detects
    /// which.
    HoldOnPendingSide,
    /// The vehicle is measuring `side`.
    Measuring(Side),
    /// Rotate the vehicle: about every axis, or, with a side, about the
    /// vertical axis while on that side.
    Rotate(Option<Side>),
    ShieldPitot,
    BlowIntoPitot,
}

impl Instruction {
    pub fn text(self) -> String {
        match self {
            Instruction::Wait => "Waiting for the vehicle…".to_string(),
            Instruction::HoldStill => "Keep the vehicle still".to_string(),
            Instruction::Place(side) => {
                format!("Place the vehicle {} and keep it still", side.placement())
            }
            Instruction::HoldOnPendingSide => {
                "Hold the vehicle still on one of the remaining sides".to_string()
            }
            Instruction::Measuring(side) => {
                format!("Measuring {}; keep the vehicle still", side.placement())
            }
            Instruction::Rotate(None) => "Rotate the vehicle slowly about every axis".to_string(),
            Instruction::Rotate(Some(side)) => format!(
                "Keep the vehicle {} and rotate it about the vertical axis",
                side.placement()
            ),
            Instruction::ShieldPitot => "Shield the pitot tube from wind".to_string(),
            Instruction::BlowIntoPitot => "Blow into the front of the pitot tube".to_string(),
        }
    }
}

/// Where a session is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Sent, not yet acknowledged or seen running.
    Starting,
    Running,
    Succeeded,
    Failed(String),
    Cancelled,
}

impl Phase {
    pub fn is_finished(&self) -> bool {
        self.outcome().is_some()
    }

    /// How the session ended, once it has.
    pub fn outcome(&self) -> Option<Outcome> {
        match self {
            Phase::Starting | Phase::Running => None,
            Phase::Succeeded => Some(Outcome::Succeeded),
            Phase::Failed(reason) => Some(Outcome::Failed(reason.clone())),
            Phase::Cancelled => Some(Outcome::Cancelled),
        }
    }
}

/// How a finished session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed(String),
    Cancelled,
}

/// One compass's state in a compass calibration (ArduPilot reports each).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompassProgress {
    /// The vehicle's compass index, from 0.
    pub id: u8,
    pub completion: u8,
    pub status: CompassStatus,
    /// The fit's residual, lower is better; reported with the result.
    pub fitness: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompassStatus {
    Running,
    Succeeded,
    Failed(&'static str),
}

/// What the operator can do about a running calibration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CalibrationControls {
    /// The vehicle waits for the operator to confirm the current step.
    pub confirm: bool,
    /// The vehicle can be told to stop. Without that, cancelling only stops
    /// following the run: the vehicle keeps waiting until it reboots or a new
    /// run starts.
    pub cancel: bool,
}

/// How many vehicle texts a session keeps.
const MESSAGES: usize = 16;

/// A calibration run on one vehicle, as the operator sees it.
#[derive(Debug, Clone)]
pub struct Session {
    pub kind: CalibrationKind,
    pub phase: Phase,
    pub instruction: Instruction,
    /// Percent complete, when the vehicle reports it.
    pub progress: Option<u8>,
    /// For six-sided calibrations.
    pub sides: Sides,
    pub compasses: Vec<CompassProgress>,
    /// The vehicle's calibration texts, oldest first.
    pub messages: VecDeque<String>,
    /// The new calibration only applies after the autopilot reboots.
    pub needs_reboot: bool,
    /// The final `COMMAND_ACK` of the start command is the outcome: the
    /// firmware runs this calibration before acknowledging it.
    completes_on_ack: bool,
    /// The vehicle has shown that *this* run is under way. Firmwares that
    /// keep repeating the last result (ArduPilot) can send one from a
    /// previous run just after a new one starts; results are only trusted
    /// once this is set.
    observed: bool,
}

impl Session {
    pub(crate) fn new(kind: CalibrationKind, completes_on_ack: bool) -> Self {
        Self {
            kind,
            phase: Phase::Starting,
            instruction: kind.first_instruction(),
            progress: None,
            sides: Sides::default(),
            compasses: Vec::new(),
            messages: VecDeque::new(),
            needs_reboot: false,
            completes_on_ack,
            observed: false,
        }
    }

    pub fn is_running(&self) -> bool {
        !self.phase.is_finished()
    }

    /// The start command was acknowledged (`Ok`) or refused.
    pub(crate) fn acknowledged(&mut self, result: Result<(), String>) {
        if self.phase.is_finished() {
            return;
        }

        match result {
            Err(reason) => self.fail(reason),
            Ok(()) if self.completes_on_ack => self.succeed(),
            Ok(()) => self.phase = Phase::Running,
        }
    }

    pub(crate) fn cancel(&mut self) {
        if !self.phase.is_finished() {
            self.phase = Phase::Cancelled;
            self.instruction = Instruction::Wait;
        }
    }

    pub(crate) fn fail(&mut self, reason: impl Into<String>) {
        if !self.phase.is_finished() {
            self.phase = Phase::Failed(reason.into());
            self.instruction = Instruction::Wait;
        }
    }

    fn succeed(&mut self) {
        if !self.phase.is_finished() {
            self.phase = Phase::Succeeded;
            self.progress = Some(100);
            self.instruction = Instruction::Wait;
        }
    }

    /// The vehicle reported this run as under way.
    fn observe_running(&mut self) {
        self.observed = true;
        if self.phase == Phase::Starting {
            self.phase = Phase::Running;
        }
    }

    /// Keep a vehicle text, skipping immediate repeats (PX4 sends some twice).
    fn note(&mut self, text: &str) {
        if self.messages.back().is_some_and(|last| last == text) {
            return;
        }
        if self.messages.len() == MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(text.to_string());
    }
}

/// What the vehicle sent that may concern a calibration.
#[derive(Debug, Clone, Copy)]
pub enum Feedback<'a> {
    /// A complete status text.
    Text(&'a StatusText),
    /// Any other message from the autopilot.
    Message(&'a DefaultDialect),
}

/// How one firmware runs calibrations on the wire.
pub trait CalibrationProtocol: Send + Sync + 'static {
    fn supports(&self, kind: CalibrationKind) -> bool;

    /// The command that starts `kind`.
    fn start(&self, kind: CalibrationKind) -> CommandLong;

    /// Whether the start command's final acknowledgement is the outcome, not
    /// just the start.
    fn completes_on_ack(&self, _kind: CalibrationKind) -> bool {
        false
    }

    /// The command telling the vehicle the operator did what the session's
    /// instruction asks, when the vehicle waits for that.
    fn confirm(&self, _session: &Session) -> Option<CommandLong> {
        None
    }

    /// The command that stops the session on the vehicle, if the firmware
    /// has one. Without it, cancelling only stops following the session.
    fn cancel(&self, _session: &Session) -> Option<CommandLong> {
        None
    }

    /// Fold the vehicle's feedback into a session that is not finished.
    fn observe(&self, session: &mut Session, feedback: Feedback<'_>);

    /// The calibrations this firmware supports, in display order.
    fn kinds(&self) -> Vec<CalibrationKind> {
        CalibrationKind::ALL
            .into_iter()
            .filter(|kind| self.supports(*kind))
            .collect()
    }
}

/// What the runtime is told about a session by the operation running it.
#[derive(Debug, Clone)]
pub enum CalibrationUpdate {
    /// A start command is about to be sent.
    Begin(CalibrationKind),
    /// The start command was acknowledged, or why it failed.
    Acknowledged(Result<(), String>),
    /// The operator cancelled.
    Cancel,
    /// The operator is done looking at a finished session.
    Dismiss,
}

/// `MAV_CMD_PREFLIGHT_CALIBRATION` with `params` as its seven parameters.
fn preflight_calibration(params: [f32; 7]) -> CommandLong {
    let [param1, param2, param3, param4, param5, param6, param7] = params;
    CommandLong {
        command: MavCmd::PreflightCalibration,
        param1,
        param2,
        param3,
        param4,
        param5,
        param6,
        param7,
        ..Default::default()
    }
}

/// Whether a status text is about calibrating, for the session's log.
fn mentions_calibration(text: &str) -> bool {
    const WORDS: [&str; 9] = [
        "calib", "accel", "gyro", "compass", "mag", "airspeed", "arspd", "reboot", "trim",
    ];
    let text = text.to_ascii_lowercase();
    WORDS.iter().any(|word| text.contains(word))
}

#[cfg(test)]
mod tests;
