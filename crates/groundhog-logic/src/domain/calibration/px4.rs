//! PX4 calibrations.
//!
//! Every calibration starts with `PREFLIGHT_CALIBRATION`: gyroscope
//! `param1 = 1`, compass `param2 = 1`, accelerometer `param5 = 1`, level
//! `param5 = 2`, airspeed `param6 = 2`. The command is acknowledged at once;
//! the calibration then runs on the vehicle and narrates itself in status
//! texts prefixed `[cal]`, the protocol QGroundControl follows:
//!
//! ```text
//! [cal] calibration started: 2 accel
//! [cal] pending: back front left right up down
//! [cal] down orientation detected
//! [cal] down side done, rotate to a different side
//! [cal] progress <17>
//! [cal] calibration done: accel
//! ```
//!
//! The vehicle detects orientations itself, so nothing needs confirming. A
//! `PREFLIGHT_CALIBRATION` with every parameter zero cancels.
//!
//! See `src/modules/commander/calibration_messages.h` and
//! `calibration_routines.cpp` in the PX4 tree.

use mavio::default_dialect::messages::CommandLong;

use super::{
    CalibrationKind, CalibrationProtocol, Feedback, Instruction, Session, Side,
    mentions_calibration, preflight_calibration,
};
use crate::domain::vehicle::class::VehicleClass;

/// The PX4 calibration protocol.
pub(crate) struct Px4Calibration;

const PREFIX: &str = "[cal] ";

impl CalibrationProtocol for Px4Calibration {
    fn supports(&self, kind: CalibrationKind, class: VehicleClass) -> bool {
        kind != CalibrationKind::Airspeed || class.has_wings()
    }

    fn start(&self, kind: CalibrationKind) -> CommandLong {
        match kind {
            CalibrationKind::Gyroscope => preflight_calibration([1., 0., 0., 0., 0., 0., 0.]),
            CalibrationKind::Compass => preflight_calibration([0., 1., 0., 0., 0., 0., 0.]),
            CalibrationKind::Accelerometer => preflight_calibration([0., 0., 0., 0., 1., 0., 0.]),
            CalibrationKind::Level => preflight_calibration([0., 0., 0., 0., 2., 0., 0.]),
            CalibrationKind::Airspeed => preflight_calibration([0., 0., 0., 0., 0., 2., 0.]),
        }
    }

    fn cancel(&self, _session: &Session) -> Option<CommandLong> {
        Some(preflight_calibration([0.0; 7]))
    }

    fn observe(&self, session: &mut Session, feedback: Feedback<'_>) {
        let Feedback::Text(text) = feedback else {
            return;
        };

        match text.text.strip_prefix(PREFIX) {
            Some(narration) => observe_narration(session, narration.trim()),
            None if mentions_calibration(&text.text) => session.note(&text.text),
            None => {}
        }
    }
}

fn observe_narration(session: &mut Session, text: &str) {
    if let Some(percent) = text
        .strip_prefix("progress <")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        // Too frequent to keep in the log.
        if let Ok(percent) = percent.parse::<u16>() {
            session.progress = Some(percent.min(100) as u8);
        }
        return;
    }

    session.observe_running();

    if text.starts_with("calibration started") {
        // Running, as any narration shows.
    } else if text.starts_with("calibration done") {
        if needs_sides(session.kind) {
            for side in Side::ALL {
                session.sides.mark(side);
            }
        }
        session.succeed();
    } else if let Some(reason) = text.strip_prefix("calibration failed: ") {
        session.fail(reason);
    } else if text.starts_with("calibration cancelled") {
        session.cancel();
    } else if let Some(pending) = text.strip_prefix("pending:") {
        let pending: Vec<Side> = pending.split_whitespace().filter_map(side_named).collect();
        for side in Side::ALL {
            if !pending.contains(&side) {
                session.sides.mark(side);
            }
        }
        session.instruction = Instruction::HoldOnPendingSide;
    } else if text.starts_with("hold vehicle still on a pending side") {
        session.instruction = Instruction::HoldOnPendingSide;
    } else if let Some(side) = text
        .strip_suffix(" orientation detected")
        .and_then(side_named)
    {
        session.instruction = match session.kind {
            CalibrationKind::Compass => Instruction::Rotate(Some(side)),
            _ => Instruction::Measuring(side),
        };
    } else if let Some(side) = text
        .strip_prefix("Hold still, measuring ")
        .and_then(|rest| rest.strip_suffix(" side"))
        .and_then(side_named)
    {
        session.instruction = Instruction::Measuring(side);
    } else if text.starts_with("Rotate vehicle") {
        let side = match session.instruction {
            Instruction::Rotate(side) => side,
            Instruction::Measuring(side) => Some(side),
            _ => None,
        };
        session.instruction = Instruction::Rotate(side);
    } else if let Some(side) = text
        .split_once(" side done")
        .and_then(|(side, _)| side_named(side))
    {
        session.sides.mark(side);
        session.instruction = Instruction::HoldOnPendingSide;
    } else if text.starts_with("Ensure sensor is not measuring wind") {
        session.instruction = Instruction::ShieldPitot;
    } else if text.starts_with("Blow into front of pitot")
        || text.starts_with("Create air pressure")
    {
        session.instruction = Instruction::BlowIntoPitot;
    } else {
        // What the session does not already show: warnings ("detected
        // motion, hold still..."), sensor results, retries.
        session.note(text);
    }
}

fn needs_sides(kind: CalibrationKind) -> bool {
    matches!(
        kind,
        CalibrationKind::Accelerometer | CalibrationKind::Compass
    )
}

/// PX4 names an orientation by the face of the vehicle that points down.
fn side_named(name: &str) -> Option<Side> {
    Some(match name {
        "down" => Side::Level,
        "left" => Side::Left,
        "right" => Side::Right,
        "front" => Side::NoseDown,
        "back" => Side::NoseUp,
        "up" => Side::Back,
        _ => return None,
    })
}
