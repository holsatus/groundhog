//! ArduPilot calibrations.
//!
//! - Accelerometer: `PREFLIGHT_CALIBRATION` with `param5 = 1`. The vehicle
//!   then asks for each orientation in turn by sending the GCS
//!   `MAV_CMD_ACCELCAL_VEHICLE_POS` (repeated every second), and waits until
//!   the GCS sends the same command back. The final result arrives the same
//!   way, as the special positions `SUCCESS` and `FAILED`, and is then
//!   repeated for as long as the autopilot runs.
//! - Compass: the ArduPilot dialect's `MAV_CMD_DO_START_MAG_CAL` (ArduPilot
//!   ignores the compass parameter of `PREFLIGHT_CALIBRATION`). Progress and
//!   results stream per compass as `MAG_CAL_PROGRESS` and `MAG_CAL_REPORT`;
//!   new offsets apply after a reboot.
//! - Level (`param5 = 2`), gyroscope (`param1 = 1`) and airspeed (`param3 = 1`,
//!   which also recalibrates the barometer) run before the command is
//!   acknowledged. Airspeed reports `IN_PROGRESS` while it runs.
//!
//! See `libraries/GCS_MAVLink/GCS_Common.cpp` (`handle_command_preflight_calibration`),
//! `libraries/AP_AccelCal/AP_AccelCal.cpp` and
//! `libraries/AP_Compass/AP_Compass_Calibration.cpp` in the ArduPilot tree.

use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MagCalStatus, MavCmd},
        messages::{CommandLong, MagCalProgress, MagCalReport},
    },
};

use super::{
    CalibrationKind, CalibrationProtocol, CompassProgress, CompassStatus, Feedback, Instruction,
    Session, Side, mentions_calibration, preflight_calibration,
};
use crate::domain::vehicle::class::VehicleClass;

/// The ArduPilot calibration protocol.
pub(crate) struct ArduPilotCalibration;

/// `ACCELCAL_VEHICLE_POS_SUCCESS`.
const POSITION_SUCCESS: f32 = 16_777_215.0;
/// `ACCELCAL_VEHICLE_POS_FAILED`.
const POSITION_FAILED: f32 = 16_777_216.0;

/// The `ACCELCAL_VEHICLE_POS` value of `side` (1 to 6).
fn position(side: Side) -> f32 {
    (side.index() + 1) as f32
}

fn side_at(position: f32) -> Option<Side> {
    Side::ALL
        .into_iter()
        .find(|side| self::position(*side) == position)
}

impl CalibrationProtocol for ArduPilotCalibration {
    fn supports(&self, kind: CalibrationKind, class: VehicleClass) -> bool {
        // `param3` calibrates the airspeed sensor only where there is one;
        // elsewhere it calibrates the barometer alone.
        kind != CalibrationKind::Airspeed || class.has_wings()
    }

    fn start(&self, kind: CalibrationKind) -> CommandLong {
        match kind {
            CalibrationKind::Accelerometer => preflight_calibration([0., 0., 0., 0., 1., 0., 0.]),
            CalibrationKind::Level => preflight_calibration([0., 0., 0., 0., 2., 0., 0.]),
            CalibrationKind::Gyroscope => preflight_calibration([1., 0., 0., 0., 0., 0., 0.]),
            CalibrationKind::Airspeed => preflight_calibration([0., 0., 1., 0., 0., 0., 0.]),
            CalibrationKind::Compass => CommandLong {
                command: MavCmd::DoStartMagCal,
                // Every compass, retry failed fits, save good ones, start
                // now, do not reboot by itself.
                param1: 0.0,
                param2: 1.0,
                param3: 1.0,
                param4: 0.0,
                param5: 0.0,
                ..Default::default()
            },
        }
    }

    fn completes_on_ack(&self, kind: CalibrationKind) -> bool {
        matches!(
            kind,
            CalibrationKind::Level | CalibrationKind::Gyroscope | CalibrationKind::Airspeed
        )
    }

    fn confirm(&self, session: &Session) -> Option<CommandLong> {
        match (session.kind, session.instruction) {
            (CalibrationKind::Accelerometer, Instruction::Place(side)) => Some(CommandLong {
                command: MavCmd::AccelcalVehiclePos,
                param1: position(side),
                ..Default::default()
            }),
            _ => None,
        }
    }

    fn cancel(&self, session: &Session) -> Option<CommandLong> {
        // Only the compass calibration can be stopped. The accelerometer
        // calibration waits for its next orientation until restarted or the
        // autopilot reboots; the others finish before acknowledging.
        (session.kind == CalibrationKind::Compass).then(|| CommandLong {
            command: MavCmd::DoCancelMagCal,
            ..Default::default()
        })
    }

    fn cancel_note(&self, kind: CalibrationKind) -> Option<&'static str> {
        match kind {
            CalibrationKind::Compass => None,
            CalibrationKind::Accelerometer => Some(
                "The vehicle keeps waiting for the next orientation until it is restarted or \
                 reboots.",
            ),
            CalibrationKind::Level | CalibrationKind::Gyroscope | CalibrationKind::Airspeed => {
                Some(
                    "The vehicle finishes this calibration on its own; cancelling only stops \
                 waiting for its answer.",
                )
            }
        }
    }

    fn observe(&self, session: &mut Session, feedback: Feedback<'_>) {
        match feedback {
            Feedback::Text(text) => {
                if mentions_calibration(&text.text) {
                    session.note(&text.text);
                }
            }
            Feedback::Message(DefaultDialect::CommandLong(command))
                if command.command == MavCmd::AccelcalVehiclePos
                    && session.kind == CalibrationKind::Accelerometer =>
            {
                observe_position(session, command.param1);
            }
            Feedback::Message(DefaultDialect::MagCalProgress(progress))
                if session.kind == CalibrationKind::Compass =>
            {
                observe_compass_progress(session, progress);
            }
            Feedback::Message(DefaultDialect::MagCalReport(report))
                if session.kind == CalibrationKind::Compass =>
            {
                observe_compass_report(session, report);
            }
            Feedback::Message(_) => {}
        }
    }
}

fn observe_position(session: &mut Session, position: f32) {
    if let Some(side) = side_at(position) {
        session.observe_running();
        // The vehicle asks for the sides in order; asking for this one means
        // every earlier one is done.
        for earlier in Side::ALL.into_iter().take_while(|earlier| *earlier != side) {
            session.sides.mark(earlier);
        }
        session.instruction = Instruction::Place(side);
        session.progress = Some((session.sides.count() * 100 / Side::ALL.len()) as u8);
    } else if session.observed && position == POSITION_SUCCESS {
        for side in Side::ALL {
            session.sides.mark(side);
        }
        session.succeed();
    } else if session.observed && position == POSITION_FAILED {
        session.fail("the vehicle rejected the measurements; try again, holding it still");
    }
}

fn observe_compass_progress(session: &mut Session, progress: &MagCalProgress) {
    session.observe_running();
    let entry = compass(session, progress.compass_id);
    entry.completion = progress.completion_pct.min(100);
    entry.status = CompassStatus::Running;
    entry.fitness = None;

    session.progress = session
        .compasses
        .iter()
        .map(|compass| compass.completion)
        .min();
}

fn observe_compass_report(session: &mut Session, report: &MagCalReport) {
    // Reports repeat until the next run, so one for a compass this run has
    // not shown progress for is from an earlier run.
    let Some(entry) = session
        .compasses
        .iter_mut()
        .find(|compass| compass.id == report.compass_id)
    else {
        return;
    };

    entry.status = compass_status(report.cal_status);
    if entry.status == CompassStatus::Running {
        return;
    }
    entry.completion = 100;
    entry.fitness = Some(report.fitness);

    if session
        .compasses
        .iter()
        .any(|compass| compass.status == CompassStatus::Running)
    {
        return;
    }

    let failed = session
        .compasses
        .iter()
        .find_map(|compass| match compass.status {
            CompassStatus::Failed(reason) => Some((compass.id, reason)),
            _ => None,
        });

    match failed {
        Some((id, reason)) => session.fail(format!("compass {}: {reason}", u16::from(id) + 1)),
        None => {
            session.needs_reboot = true;
            session.succeed();
        }
    }
}

/// The session's entry for compass `id`, added if new.
fn compass(session: &mut Session, id: u8) -> &mut CompassProgress {
    let index = match session
        .compasses
        .iter()
        .position(|compass| compass.id == id)
    {
        Some(index) => index,
        None => {
            session.compasses.push(CompassProgress {
                id,
                completion: 0,
                status: CompassStatus::Running,
                fitness: None,
            });
            session.compasses.sort_by_key(|compass| compass.id);
            session
                .compasses
                .iter()
                .position(|compass| compass.id == id)
                .expect("just inserted")
        }
    };
    &mut session.compasses[index]
}

fn compass_status(status: MagCalStatus) -> CompassStatus {
    match status {
        MagCalStatus::MagCalNotStarted
        | MagCalStatus::MagCalWaitingToStart
        | MagCalStatus::MagCalRunningStepOne
        | MagCalStatus::MagCalRunningStepTwo => CompassStatus::Running,
        MagCalStatus::MagCalSuccess => CompassStatus::Succeeded,
        MagCalStatus::MagCalFailed => CompassStatus::Failed("the fit failed"),
        MagCalStatus::MagCalBadOrientation => {
            CompassStatus::Failed("its orientation could not be determined")
        }
        MagCalStatus::MagCalBadRadius => CompassStatus::Failed("the field strength is implausible"),
    }
}
