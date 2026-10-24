//! Intent-level operations on a single vehicle.
//!
//! A [`VehicleClient`] resolves everything wire-level from the latest fleet
//! snapshot at the moment an operation starts: which component to address,
//! how the firmware lays out missions, how parameters are encoded, where home
//! is. Callers speak in domain terms — plans, flight modes, parameter values —
//! and results that change what the vehicle holds (a transferred mission, a
//! parameter download) are reported back to the fleet runtime, so every
//! reader sees them.

use std::time::Duration;

use mavio::default_dialect::messages::CommandLong;

use mav_param::{Ident, Value};
use mavio::default_dialect::{
    enums::{MavMissionType, MavResult},
    messages::{AutopilotVersion, HomePosition},
};

use super::{Error, FleetHandle, Result, runtime::Input};
use crate::{
    MavlinkId, SystemId,
    domain::{
        calibration::{CalibrationKind, CalibrationProtocol, CalibrationUpdate, Session},
        firmware::{FlightMode, ModeKind},
        mission::{MissionItem, OnboardMission, codec},
        parameter::DownloadState,
        vehicle::Vehicle,
    },
    mavlink::{MavError, commands},
};

pub use crate::mavlink::TransferProgress;

/// How long a calibration command may take to be acknowledged. Firmwares run
/// some calibrations (ArduPilot's gyroscope and level) before acknowledging.
const CALIBRATION_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a calibration step confirmation may take to be acknowledged.
const CALIBRATION_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);

/// A summary of a parameter-list download: how many parameters arrived and how
/// many the target advertised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamListSummary {
    pub received: usize,
    pub expected: u16,
}

/// Reports a parameter download to the runtime: in progress from the start,
/// then its outcome, or cancelled if the download is dropped first. A dropped
/// download must not stay in progress in every reader's snapshot.
struct DownloadReport<'a> {
    fleet: &'a FleetHandle,
    address: MavlinkId,
    finished: bool,
}

impl<'a> DownloadReport<'a> {
    fn start(fleet: &'a FleetHandle, address: MavlinkId) -> Self {
        fleet.report(Input::ParamDownload {
            address,
            state: DownloadState::InProgress,
        });
        Self {
            fleet,
            address,
            finished: false,
        }
    }

    fn finish(mut self, state: DownloadState) {
        self.finished = true;
        self.fleet.report(Input::ParamDownload {
            address: self.address,
            state,
        });
    }
}

impl Drop for DownloadReport<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.fleet.report(Input::ParamDownload {
                address: self.address,
                state: DownloadState::Cancelled,
            });
        }
    }
}

/// An intent-level handle for operating one vehicle (or one component of it).
/// Cheap to clone.
#[derive(Clone)]
pub struct VehicleClient {
    fleet: FleetHandle,
    system: SystemId,
    /// The addressed component; the autopilot when `None`.
    component: Option<u8>,
}

impl VehicleClient {
    pub(super) fn new(fleet: FleetHandle, system: SystemId, component: Option<u8>) -> Self {
        Self {
            fleet,
            system,
            component,
        }
    }

    /// Read the vehicle from the latest snapshot.
    fn with_vehicle<T>(&self, read: impl FnOnce(&Vehicle) -> T) -> Result<T> {
        let snapshot = self.fleet.snapshot();
        let vehicle = snapshot.vehicle(self.system).ok_or(Error::UnknownVehicle)?;
        Ok(read(vehicle))
    }

    /// The addressed component of `vehicle`.
    fn component_of(&self, vehicle: &Vehicle) -> u8 {
        self.component
            .unwrap_or_else(|| vehicle.default_parameter_component())
    }

    fn target(&self) -> Result<MavlinkId> {
        self.with_vehicle(|vehicle| vehicle.target_for(self.component_of(vehicle)))
    }

    /// Bring a newly registered vehicle up to date: capabilities (which settle
    /// the parameter encoding), home, the parameter list and the mission.
    /// Every step is best effort; failures are logged and the next step runs.
    pub async fn bootstrap(&self) {
        // The runtime starts this as it registers the vehicle, before the
        // snapshot that contains it is published.
        if !self
            .fleet
            .wait_for(self.system, Duration::from_secs(5))
            .await
        {
            log::warn!(
                "Vehicle {} never appeared; not bootstrapping it",
                self.system
            );
            return;
        }

        let Ok(target) = self.target() else {
            return;
        };
        let router = self.fleet.router();

        if let Err(error) = router.request_message::<AutopilotVersion>(target).await {
            log::info!("No AUTOPILOT_VERSION from {target:?}: {error}");
        }

        if let Err(error) = router.request_message::<HomePosition>(target).await {
            log::info!("No HOME_POSITION from {target:?}: {error}");
        }

        if let Err(error) = self.fetch_parameters().await {
            log::warn!("Parameter download from {target:?} failed: {error}");
        }

        if let Err(error) = self.download_mission(&|_| {}).await {
            log::warn!("Mission download from {target:?} failed: {error}");
        }
    }

    /// Download the component's parameter list. Progress and the outcome are
    /// visible in the fleet snapshot (see [`DownloadState`]).
    pub async fn fetch_parameters(&self) -> Result<ParamListSummary> {
        // Capabilities are only worth a round trip when they would decide
        // the encoding: a known firmware already implies it.
        let (target, encoding_known) = self.with_vehicle(|vehicle| {
            let component = self.component_of(vehicle);
            (
                vehicle.target_for(component),
                vehicle.param_encoding(component).is_some(),
            )
        })?;

        let report = DownloadReport::start(&self.fleet, target);

        let result = self
            .fleet
            .router()
            .param_list(target, !encoding_known)
            .await
            .map_err(Error::from);

        let state = match &result {
            Ok((received, expected)) if *received >= usize::from(*expected) => {
                DownloadState::Complete
            }
            Ok((received, expected)) => {
                DownloadState::Failed(format!("received {received} of {expected} parameters"))
            }
            Err(error) => DownloadState::Failed(error.to_string()),
        };
        report.finish(state);

        result.map(|(received, expected)| ParamListSummary { received, expected })
    }

    /// Set one parameter on the component, returning the value it reports
    /// back. Refused while the component's encoding is unknown: writing with
    /// a guessed encoding can store a different value than intended.
    pub async fn set_parameter(&self, ident: Ident, value: Value) -> Result<Value> {
        let (target, encoding) = self.with_vehicle(|vehicle| {
            let component = self.component_of(vehicle);
            (
                vehicle.target_for(component),
                vehicle.param_encoding(component),
            )
        })?;

        let encoding = encoding.ok_or(Error::ParamEncodingUnknown)?;

        self.fleet
            .router()
            .param_set(target, ident, value, encoding)
            .await
            .map_err(Error::from)
    }

    /// Download the vehicle's mission.
    pub async fn download_mission(
        &self,
        progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<OnboardMission> {
        let (target, layout) =
            self.with_vehicle(|vehicle| (vehicle.target(), vehicle.mission_layout()))?;

        let downloaded = self
            .fleet
            .router()
            .mission_download(target, MavMissionType::Mission, progress)
            .await?;

        let mission = codec::decode(&downloaded.items, downloaded.opaque_id, layout);
        self.fleet.report(Input::OnboardMission {
            system: self.system,
            mission: mission.clone(),
        });

        Ok(mission)
    }

    /// Upload `items` as the vehicle's mission, returning what it now holds.
    pub async fn upload_mission(
        &self,
        items: Vec<MissionItem>,
        progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<OnboardMission> {
        let (target, layout, home) = self.with_vehicle(|vehicle| {
            (
                vehicle.target(),
                vehicle.mission_layout(),
                vehicle.home().map(|home| home.mission_item()),
            )
        })?;

        let wire = codec::encode(&items, home.as_ref(), layout, MavMissionType::Mission);
        let opaque_id = self
            .fleet
            .router()
            .mission_upload(target, MavMissionType::Mission, &wire, progress)
            .await?;

        let mission = OnboardMission {
            items,
            home: layout.home_slot.then_some(home).flatten(),
            opaque_id,
        };
        self.fleet.report(Input::OnboardMission {
            system: self.system,
            mission: mission.clone(),
        });

        Ok(mission)
    }

    /// Clear the vehicle's mission.
    pub async fn clear_mission(&self) -> Result<()> {
        let target = self.with_vehicle(Vehicle::target)?;

        self.fleet
            .router()
            .mission_clear(target, MavMissionType::Mission)
            .await?;

        self.fleet.report(Input::OnboardMission {
            system: self.system,
            mission: OnboardMission::empty(),
        });
        Ok(())
    }

    /// Continue the mission at plan item `index`.
    ///
    /// Uses the acknowledged `MAV_CMD_DO_SET_MISSION_CURRENT`, falling back to
    /// the legacy message for vehicles that do not support the command.
    pub async fn set_current_mission_item(&self, index: usize) -> Result<()> {
        let (target, layout) =
            self.with_vehicle(|vehicle| (vehicle.target(), vehicle.mission_layout()))?;
        let seq = layout.wire_seq(index);
        let router = self.fleet.router();

        match router
            .command(target, commands::set_mission_current(seq))
            .await
        {
            Ok(_) => Ok(()),
            Err(MavError::Rejected(MavResult::Unsupported)) => router
                .mission_set_current(target, seq)
                .await
                .map_err(Error::from),
            Err(error) => Err(error.into()),
        }
    }

    /// Run a command against the autopilot, discarding the final ACK.
    async fn run(&self, command: crate::mavlink::Command) -> Result<()> {
        let target = self.with_vehicle(Vehicle::target)?;
        self.fleet
            .router()
            .command(target, command)
            .await
            .map(|_| ())
            .map_err(Error::from)
    }

    /// Start executing the stored mission.
    pub async fn start_mission(&self) -> Result<()> {
        self.run(commands::mission_start()).await
    }

    /// Pause (`true`) or resume (`false`) the running mission.
    pub async fn pause_mission(&self, pause: bool) -> Result<()> {
        self.run(commands::pause_continue(pause)).await
    }

    /// Arm or disarm, honouring the vehicle's arming checks.
    pub async fn arm(&self, arm: bool) -> Result<()> {
        self.run(commands::arm_disarm(arm, false)).await
    }

    /// Command a flight mode.
    pub async fn set_mode(&self, mode: FlightMode) -> Result<()> {
        let command = self.with_vehicle(|vehicle| vehicle.firmware().set_mode(&mode))?;
        self.run(commands::set_mode(
            command.base_mode,
            command.custom_mode,
            command.custom_sub_mode,
        ))
        .await
    }

    /// Run a calibration. Returns once the vehicle acknowledged the start (or,
    /// for calibrations the firmware completes before acknowledging, the
    /// outcome). The run itself is followed by the fleet runtime and visible
    /// as the vehicle's [`Session`] in the snapshot.
    pub async fn start_calibration(&self, kind: CalibrationKind) -> Result<()> {
        let (target, protocol) =
            self.with_vehicle(|vehicle| (vehicle.target(), vehicle.firmware().calibration()))?;
        let protocol = protocol
            .filter(|protocol| protocol.supports(kind))
            .ok_or(Error::CalibrationUnsupported)?;

        // Before the command: feedback can arrive before the acknowledgement.
        self.report_calibration(CalibrationUpdate::Begin(kind));

        // Never resent: a duplicate would restart the calibration, or run a
        // second one after the first.
        let result = self
            .fleet
            .router()
            .command(target, protocol.start(kind))
            .timeout(CALIBRATION_ACK_TIMEOUT)
            .retries(0)
            .await
            .map(|_| ())
            .map_err(Error::from);

        self.report_calibration(CalibrationUpdate::Acknowledged(
            result.as_ref().map(|_| ()).map_err(ToString::to_string),
        ));
        result
    }

    /// Tell the vehicle the operator did what the running calibration asks
    /// (placed it in the requested orientation).
    pub async fn confirm_calibration_step(&self) -> Result<()> {
        let (target, command) =
            self.with_calibration(|protocol, session| protocol.confirm(session))?;
        let command = command.ok_or(Error::NotCalibrating)?;

        // Not resent either: once the vehicle measures, a repeat is refused.
        self.fleet
            .router()
            .command(target, command)
            .timeout(CALIBRATION_CONFIRM_TIMEOUT)
            .retries(0)
            .await
            .map(|_| ())
            .map_err(Error::from)
    }

    /// Cancel the running calibration. The session ends at once; the vehicle
    /// is told to stop when its firmware has a way to.
    pub async fn cancel_calibration(&self) -> Result<()> {
        let (target, command) =
            self.with_calibration(|protocol, session| protocol.cancel(session))?;
        self.report_calibration(CalibrationUpdate::Cancel);

        if let Some(command) = command {
            // PX4 acknowledges a cancel twice, once refusing it as a new
            // command while busy: the outcome shows in its texts instead.
            if let Err(error) = self
                .fleet
                .router()
                .command(target, command)
                .retries(1)
                .await
            {
                log::debug!("Calibration cancel on {target:?}: {error}");
            }
        }
        Ok(())
    }

    /// Forget a finished calibration.
    pub fn dismiss_calibration(&self) {
        self.report_calibration(CalibrationUpdate::Dismiss);
    }

    fn report_calibration(&self, update: CalibrationUpdate) {
        self.fleet.report(Input::Calibration {
            system: self.system,
            update,
        });
    }

    /// Read the running calibration and its protocol.
    fn with_calibration(
        &self,
        read: impl FnOnce(&dyn CalibrationProtocol, &Session) -> Option<CommandLong>,
    ) -> Result<(MavlinkId, Option<CommandLong>)> {
        self.with_vehicle(|vehicle| {
            let protocol = vehicle.firmware().calibration()?;
            let session = vehicle
                .calibration
                .as_ref()
                .filter(|session| session.is_running())?;
            Some((vehicle.target(), read(protocol, session)))
        })?
        .ok_or(Error::NotCalibrating)
    }

    /// Command the vehicle's mode of `kind` (return, land, ...), whatever its
    /// firmware calls it. Returns the mode commanded.
    pub async fn set_mode_kind(&self, kind: ModeKind) -> Result<FlightMode> {
        let mode = self
            .with_vehicle(|vehicle| vehicle.mode_of_kind(kind))?
            .ok_or(Error::NoSuchMode)?;

        self.set_mode(mode).await.map(|()| mode)
    }
}
