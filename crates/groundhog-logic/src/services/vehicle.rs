//! Intent-level operations on a single vehicle.
//!
//! A [`VehicleClient`] resolves everything wire-level from the latest fleet
//! snapshot at the moment an operation starts: which component to address,
//! how the firmware lays out missions, how parameters are encoded, where home
//! is. Callers speak in domain terms — plans, flight modes, parameter values —
//! and results that change what the vehicle holds (a transferred mission, a
//! parameter download) are reported back to the fleet runtime, so every
//! reader sees them.

use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use mav_param::{Ident, Value};
use mavio::default_dialect::{
    enums::{MavMissionType, MavResult},
    messages::{AutopilotVersion, CameraInformation, CameraTrackingImageStatus, HomePosition},
};

use super::{Error, FleetHandle, Result, runtime::Input};
use crate::{
    MavlinkId, SystemId,
    domain::{
        calibration::{CalibrationKind, CalibrationUpdate, Confirmation, RunId},
        firmware::{FlightMode, ModeKind},
        mission::{MissionItem, OnboardMission, codec},
        parameter::{DownloadId, DownloadOutcome, DownloadUpdate, ParamEncoding},
        vehicle::{
            Vehicle,
            camera::{CameraId, ImagePoint, ImageRect, TrackingSupport},
            mission::{TransferId, TransferKind, TransferUpdate},
        },
    },
    mavlink::{MavError, ParamListResult, TransferReport, commands},
};

/// How often a camera is asked to report what it tracks, once it tracks.
const TRACKING_REPORT_INTERVAL: Duration = Duration::from_millis(100);

/// How long a calibration command may take to be acknowledged. Firmwares run
/// some calibrations (ArduPilot's gyroscope and level) before acknowledging.
const CALIBRATION_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a reboot may take to be acknowledged. An autopilot answers before
/// it goes down, but the answer may be lost as it does.
const REBOOT_ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// How long an unanswered reboot may take to show: the autopilot going down,
/// starting, and reporting its boot time again, perhaps over a link that
/// reconnects (a USB port that comes and goes with it).
const REBOOT_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a calibration step confirmation may take to be acknowledged.
const CALIBRATION_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);

/// Reports a parameter download to the runtime: in progress once it holds the
/// target's reply channel, then its outcome, or cancelled if the download is
/// dropped first. A dropped download must not stay in progress in every
/// reader's snapshot.
///
/// A download that is still queued for the channel has not started: reporting
/// it would begin a new generation, and the running download's completion
/// would then drop every value it received before this one was requested.
/// Its end, reported after the channel is released, names its own id, so it
/// cannot finish a download that took the channel in the meantime.
struct DownloadReport<'a> {
    fleet: &'a FleetHandle,
    address: MavlinkId,
    run: DownloadId,
    /// Atomic because `started` runs inside the router's download future,
    /// which must be `Send`, through a shared borrow: a `Cell` is not `Sync`.
    started: AtomicBool,
    finished: bool,
}

impl<'a> DownloadReport<'a> {
    fn new(fleet: &'a FleetHandle, address: MavlinkId) -> Self {
        Self {
            fleet,
            address,
            run: DownloadId(LAST_DOWNLOAD.fetch_add(1, Ordering::Relaxed) + 1),
            started: AtomicBool::new(false),
            finished: false,
        }
    }

    /// Report the start, once: the caller runs this when it holds the target's
    /// reply channel, from inside the download's future.
    fn started(&self) {
        if !self.started.swap(true, Ordering::Relaxed) {
            self.fleet.report(Input::ParamDownload {
                address: self.address,
                update: DownloadUpdate::Start(self.run),
            });
        }
    }

    fn finish(mut self, outcome: DownloadOutcome) {
        // A download that ends without having reported its start cannot
        // happen (the callback runs first), but the outcome must not be
        // folded into the generation before it either.
        self.started();
        self.finished = true;
        self.fleet.report(Input::ParamDownload {
            address: self.address,
            update: DownloadUpdate::Finish(self.run, outcome),
        });
    }
}

impl Drop for DownloadReport<'_> {
    fn drop(&mut self) {
        if !self.finished && self.started.load(Ordering::Relaxed) {
            self.fleet.report(Input::ParamDownload {
                address: self.address,
                update: DownloadUpdate::Finish(self.run, DownloadOutcome::Cancelled),
            });
        }
    }
}

/// The last parameter download handed out; download ids only need to be
/// unique within the process.
static LAST_DOWNLOAD: AtomicU64 = AtomicU64::new(0);

/// Reports a mission transfer to the runtime: its start once the router
/// holds the vehicle's mission channel (before anything is sent), its
/// progress, and its end when dropped. The operation reports its result
/// before this drops, so the runtime sees the result first. While a transfer
/// runs, the fleet does not judge the onboard mission from the vehicle's
/// reports: they may already name the mission being transferred.
struct MissionReport<'a> {
    fleet: &'a FleetHandle,
    system: SystemId,
    kind: TransferKind,
    id: TransferId,
    /// Atomic for the same reason as [`DownloadReport::started`].
    started: AtomicBool,
}

impl<'a> MissionReport<'a> {
    fn new(fleet: &'a FleetHandle, system: SystemId, kind: TransferKind) -> Self {
        Self {
            fleet,
            system,
            kind,
            id: TransferId::next(),
            started: AtomicBool::new(false),
        }
    }

    /// The router's report of the transfer, passed on to the runtime.
    fn report(&self, report: TransferReport) {
        let update = match report {
            TransferReport::Started => {
                if self.started.swap(true, Ordering::Relaxed) {
                    return;
                }
                TransferUpdate::Start(self.id, self.kind)
            }
            TransferReport::Progress(progress) => TransferUpdate::Progress(self.id, progress),
        };
        self.fleet.report(Input::MissionTransfer {
            system: self.system,
            update,
        });
    }
}

impl Drop for MissionReport<'_> {
    fn drop(&mut self) {
        if self.started.load(Ordering::Relaxed) {
            self.fleet.report(Input::MissionTransfer {
                system: self.system,
                update: TransferUpdate::End(self.id),
            });
        }
    }
}

/// Reports a calibration start to the runtime, naming its run: begun when
/// created, then acknowledged, or abandoned if dropped first. A dropped start
/// must not leave the session starting in every reader's snapshot, and the
/// run id keeps a late report from landing on a newer run.
struct CalibrationStart<'a> {
    client: &'a VehicleClient,
    run: RunId,
    finished: bool,
}

impl<'a> CalibrationStart<'a> {
    fn begin(client: &'a VehicleClient, kind: CalibrationKind) -> Self {
        let run = RunId::next();
        client.report_calibration(CalibrationUpdate::Begin { kind, run });
        Self {
            client,
            run,
            finished: false,
        }
    }

    fn acknowledged(mut self, result: std::result::Result<(), String>) {
        self.finished = true;
        self.client
            .report_calibration(CalibrationUpdate::Acknowledged {
                run: self.run,
                result,
            });
    }
}

impl Drop for CalibrationStart<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.client
                .report_calibration(CalibrationUpdate::Abandoned { run: self.run });
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

        if let Err(error) = self.download_mission().await {
            log::warn!("Mission download from {target:?} failed: {error}");
        }

        // Last: most autopilots have no camera to describe, and the request
        // waits out its retries before it gives up.
        self.describe_camera(target.component).await;
    }

    /// Download the component's parameter list. Progress and the outcome are
    /// visible in the fleet snapshot (see [`ParamDownload`]); the returned
    /// outcome is the same one the snapshot holds.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet, or the reason
    /// the transfer failed: [`Error::Timeout`] if it stopped answering,
    /// [`Error::NoLink`] if nothing reaches it. A download that ends short of
    /// the count, or with nothing at all, is not an error: it returns
    /// [`DownloadOutcome::Incomplete`] or [`DownloadOutcome::NoParameters`].
    ///
    /// [`ParamDownload`]: crate::domain::parameter::ParamDownload
    pub async fn fetch_parameters(&self) -> Result<DownloadOutcome> {
        // Capabilities are only worth a round trip when they would decide
        // the encoding: a known firmware already implies it.
        let (target, encoding_known) = self.with_vehicle(|vehicle| {
            let component = self.component_of(vehicle);
            (
                vehicle.target_for(component),
                vehicle.param_encoding(component).is_some(),
            )
        })?;

        let report = DownloadReport::new(&self.fleet, target);

        let result = self
            .fleet
            .router()
            .param_list(target, !encoding_known, || report.started())
            .await;

        let outcome = match &result {
            Ok(ParamListResult::Nothing) => DownloadOutcome::NoParameters,
            Ok(ParamListResult::Received(progress)) if progress.done >= progress.total => {
                DownloadOutcome::Complete
            }
            Ok(ParamListResult::Received(progress)) => DownloadOutcome::Incomplete(*progress),
            Err(error) => DownloadOutcome::Failed(error.to_string()),
        };
        report.finish(outcome.clone());

        match result {
            Ok(_) => Ok(outcome),
            Err(error) => Err(Error::from(error)),
        }
    }

    /// Set one parameter on the component, returning the value it reports
    /// back. Refused while the component's encoding is unknown: writing with
    /// a guessed encoding can store a different value than intended.
    ///
    /// # Errors
    ///
    /// [`Error::ParamEncodingUnknown`] while the component's encoding is not
    /// known, [`Error::ParamEncoding`] if `value` cannot be represented in it,
    /// [`Error::Rejected`] if the vehicle refuses the write,
    /// [`Error::UnknownVehicle`] if it left the fleet, or [`Error::Timeout`] /
    /// [`Error::NoLink`].
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

    /// Take the operator's word for how the component encodes parameter
    /// values, for a component that neither advertises an encoding nor names
    /// a firmware that implies one; `None` takes it back. Its values are
    /// re-decoded with it, and [`Self::set_parameter`] writes with it, until
    /// the component makes its own encoding known
    /// ([`FleetEvent::StatedEncodingContradicted`] if they differ).
    ///
    /// Ignored for a component whose encoding can be inferred: the
    /// operator's word is the last resort, not an override. Not kept beyond
    /// the session, since a system id may name another vehicle next time.
    ///
    /// [`FleetEvent::StatedEncodingContradicted`]: crate::domain::fleet::FleetEvent::StatedEncodingContradicted
    pub fn state_param_encoding(&self, encoding: Option<ParamEncoding>) {
        match self.target() {
            Ok(address) => self
                .fleet
                .report(Input::ParamEncoding { address, encoding }),
            Err(error) => log::warn!(
                "Not stating the parameter encoding of {}: {error}",
                self.system
            ),
        }
    }

    /// Download the vehicle's mission. Its progress is visible in the fleet
    /// snapshot, as the vehicle's
    /// [`MissionTransfer`](crate::domain::vehicle::mission::MissionTransfer).
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet,
    /// [`Error::MissionRejected`] if it refuses the transfer,
    /// [`Error::Protocol`] if it sends a mission this build cannot decode, or
    /// [`Error::Timeout`] / [`Error::NoLink`].
    pub async fn download_mission(&self) -> Result<OnboardMission> {
        let (target, layout) =
            self.with_vehicle(|vehicle| (vehicle.target(), vehicle.mission_layout()))?;

        let report = MissionReport::new(&self.fleet, self.system, TransferKind::Download);
        let downloaded = self
            .fleet
            .router()
            .mission_download(target, MavMissionType::Mission, &|update| {
                report.report(update)
            })
            .await?;

        let mission = codec::decode(&downloaded.items, downloaded.opaque_id, layout);
        self.fleet.report(Input::OnboardMission {
            system: self.system,
            mission: mission.clone(),
        });

        Ok(mission)
    }

    /// Upload `items` as the vehicle's mission, returning what it now holds.
    /// Its progress is visible in the fleet snapshot, as the vehicle's
    /// [`MissionTransfer`](crate::domain::vehicle::mission::MissionTransfer).
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet,
    /// [`Error::MissionRejected`] if it refuses the plan (no space for it, an
    /// item it cannot fly, ...), or [`Error::Timeout`] / [`Error::NoLink`].
    pub async fn upload_mission(&self, items: Vec<MissionItem>) -> Result<OnboardMission> {
        let (target, layout, home) = self.with_vehicle(|vehicle| {
            (
                vehicle.target(),
                vehicle.mission_layout(),
                vehicle.home().map(|home| home.mission_item()),
            )
        })?;

        let wire = codec::encode(&items, home.as_ref(), layout, MavMissionType::Mission)
            .map_err(|_| Error::PlanTooLong)?;
        let report = MissionReport::new(&self.fleet, self.system, TransferKind::Upload);
        let opaque_id = self
            .fleet
            .router()
            .mission_upload(target, MavMissionType::Mission, &wire, &|update| {
                report.report(update)
            })
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
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet,
    /// [`Error::MissionRejected`] if it refuses, or [`Error::Timeout`] /
    /// [`Error::NoLink`].
    pub async fn clear_mission(&self) -> Result<()> {
        let target = self.with_vehicle(Vehicle::target)?;

        let report = MissionReport::new(&self.fleet, self.system, TransferKind::Clear);
        self.fleet
            .router()
            .mission_clear(target, MavMissionType::Mission, &|update| {
                report.report(update)
            })
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
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet,
    /// [`Error::Rejected`] if it refuses both forms, or [`Error::Timeout`] /
    /// [`Error::NoLink`].
    pub async fn set_current_mission_item(&self, index: usize) -> Result<()> {
        let (target, layout) =
            self.with_vehicle(|vehicle| (vehicle.target(), vehicle.mission_layout()))?;
        let seq = layout
            .wire_seq(index)
            .ok_or_else(|| Error::Protocol(format!("mission item {index} is out of range")))?;
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

    /// Ask `component` to describe its cameras (`CAMERA_INFORMATION`):
    /// the camera itself, or, from an autopilot, the cameras attached to
    /// it. Best effort, as during the bootstrap: a component with no camera
    /// does not answer.
    pub(crate) async fn describe_camera(&self, component: u8) {
        let target = MavlinkId {
            system: self.system.0,
            component,
        };
        if let Err(error) = self
            .fleet
            .router()
            .request_message::<CameraInformation>(target)
            .await
        {
            log::info!("No CAMERA_INFORMATION from {target:?}: {error}");
        }
    }

    /// Have `camera` track the point `at` in its picture, `radius` across
    /// (a share of the picture's width). What it then tracks is in the
    /// snapshot, as the camera reports it.
    ///
    /// # Errors
    ///
    /// [`Error::TrackingRadius`] if `radius` is NaN (clamping cannot make it
    /// a size), [`Error::TrackingUnsupported`] if the camera has not said it
    /// tracks points, [`Error::UnknownVehicle`] if the vehicle left the
    /// fleet, [`Error::Rejected`] if the camera refuses, or
    /// [`Error::Timeout`] / [`Error::NoLink`].
    pub async fn track_point(&self, camera: CameraId, at: ImagePoint, radius: f32) -> Result<()> {
        if radius.is_nan() {
            return Err(Error::TrackingRadius);
        }
        self.check_tracking(camera, |support| support.point)?;
        let command =
            commands::camera_track_point(at.x(), at.y(), radius.clamp(0.0, 1.0), camera.device);
        self.run_on_camera(camera, command).await?;
        self.request_tracking_reports(camera).await;
        Ok(())
    }

    /// Have `camera` track what lies within `rect` in its picture.
    ///
    /// # Errors
    ///
    /// As [`Self::track_point`], [`Error::TrackingUnsupported`] if the
    /// camera has not said it tracks rectangles.
    pub async fn track_rectangle(&self, camera: CameraId, rect: ImageRect) -> Result<()> {
        self.check_tracking(camera, |support| support.rectangle)?;
        let command = commands::camera_track_rectangle(
            (rect.top_left.x(), rect.top_left.y()),
            (rect.bottom_right.x(), rect.bottom_right.y()),
            camera.device,
        );
        self.run_on_camera(camera, command).await?;
        self.request_tracking_reports(camera).await;
        Ok(())
    }

    /// Have `camera` stop tracking.
    ///
    /// # Errors
    ///
    /// As [`Self::track_point`], [`Error::TrackingUnsupported`] if the
    /// camera has not said it tracks at all.
    pub async fn stop_tracking(&self, camera: CameraId) -> Result<()> {
        self.check_tracking(camera, TrackingSupport::any)?;
        self.run_on_camera(camera, commands::camera_stop_tracking(camera.device))
            .await
    }

    /// Refuse to command tracking `camera` has not said it can do: what it
    /// would do with the command is not known.
    fn check_tracking(
        &self,
        camera: CameraId,
        can: impl Fn(TrackingSupport) -> bool,
    ) -> Result<()> {
        let supported = self.with_vehicle(|vehicle| {
            vehicle
                .cameras
                .get(camera)
                .is_some_and(|it| can(it.tracking))
        })?;
        if supported {
            Ok(())
        } else {
            Err(Error::TrackingUnsupported)
        }
    }

    async fn run_on_camera(
        &self,
        camera: CameraId,
        command: crate::mavlink::Command,
    ) -> Result<()> {
        let target = self.with_vehicle(|vehicle| vehicle.target_for(camera.component))?;
        self.fleet
            .router()
            .command(target, command)
            .await
            .map(drop)
            .map_err(Error::from)
    }

    /// Ask `camera` to report what it tracks, which it sends only at the
    /// rate it is asked for. Best effort: tracking goes on without reports,
    /// only unseen.
    async fn request_tracking_reports(&self, camera: CameraId) {
        let command = commands::set_message_interval(
            CameraTrackingImageStatus::message_id(),
            TRACKING_REPORT_INTERVAL,
        );
        if let Err(error) = self.run_on_camera(camera, command).await {
            log::info!("{camera:?} will not report its tracking: {error}");
        }
    }

    /// Start executing the stored mission.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet,
    /// [`Error::Rejected`] if it refuses to start, or [`Error::Timeout`] /
    /// [`Error::NoLink`].
    pub async fn start_mission(&self) -> Result<()> {
        self.run(commands::mission_start()).await
    }

    /// Pause (`true`) or resume (`false`) the running mission.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet,
    /// [`Error::Rejected`] if it refuses, or [`Error::Timeout`] /
    /// [`Error::NoLink`].
    pub async fn pause_mission(&self, pause: bool) -> Result<()> {
        self.run(commands::pause_continue(pause)).await
    }

    /// Arm or disarm, honouring the vehicle's arming checks.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] if the vehicle refuses (typically a failed arming
    /// check), [`Error::UnknownVehicle`] if it left the fleet, or
    /// [`Error::Timeout`] / [`Error::NoLink`].
    pub async fn arm(&self, arm: bool) -> Result<()> {
        self.run(commands::arm_disarm(arm, false)).await
    }

    /// Reboot the autopilot. Returns once the vehicle accepted the reboot,
    /// or, if its answer was lost to the reboot itself, once it is seen
    /// starting again ([`FleetEvent::Rebooted`] announces that either way).
    ///
    /// The command is sent once: a copy resent because the answer was lost
    /// would reach the freshly started autopilot and reboot it again.
    ///
    /// # Errors
    ///
    /// [`Error::NotDisarmed`] unless the vehicle is known to be disarmed
    /// ([`Vehicle::can_reboot`]), [`Error::UnknownVehicle`] if it left the
    /// fleet, [`Error::Rejected`] if it refuses, [`Error::Timeout`] if it
    /// neither answered nor rebooted, or [`Error::NoLink`].
    ///
    /// [`FleetEvent::Rebooted`]: crate::domain::fleet::FleetEvent::Rebooted
    pub async fn reboot(&self) -> Result<()> {
        let (target, can_reboot, boots) = self.with_vehicle(|vehicle| {
            (vehicle.target(), vehicle.can_reboot(), vehicle.boot_count())
        })?;
        if !can_reboot {
            return Err(Error::NotDisarmed);
        }

        let result = self
            .fleet
            .router()
            .command(target, commands::reboot_autopilot())
            .timeout(REBOOT_ACK_TIMEOUT)
            .retries(0)
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(MavError::Timeout) => {
                let system = self.system;
                let rebooted = self
                    .fleet
                    .wait_until(REBOOT_TIMEOUT, |fleet| {
                        fleet
                            .vehicle(system)
                            .is_some_and(|vehicle| vehicle.boot_count() != boots)
                    })
                    .await;
                if rebooted {
                    Ok(())
                } else {
                    Err(Error::Timeout)
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Command a flight mode.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet,
    /// [`Error::Rejected`] if it refuses the mode, or [`Error::Timeout`] /
    /// [`Error::NoLink`].
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
    /// as the vehicle's [`Session`](crate::domain::calibration::Session) in
    /// the snapshot.
    ///
    /// # Errors
    ///
    /// [`Error::CalibrationUnsupported`] if the vehicle's firmware does not
    /// offer `kind` on this vehicle's class, [`Error::CalibrationBlocked`]
    /// if the vehicle is not known to be disarmed, does not answer, or
    /// already calibrates, [`Error::UnknownVehicle`] if it left the fleet,
    /// [`Error::Rejected`] if it refuses to start, or [`Error::Timeout`] /
    /// [`Error::NoLink`]. The command is never resent.
    pub async fn start_calibration(&self, kind: CalibrationKind) -> Result<()> {
        let (target, protocol, class, blocked) = self.with_vehicle(|vehicle| {
            (
                vehicle.target(),
                vehicle.firmware().calibration(),
                vehicle.class(),
                vehicle.calibration_controls().start_blocked,
            )
        })?;
        let protocol = protocol
            .filter(|protocol| protocol.supports(kind, class))
            .ok_or(Error::CalibrationUnsupported)?;
        if let Some(blocked) = blocked {
            return Err(Error::CalibrationBlocked(blocked));
        }

        // Before the command: feedback can arrive before the acknowledgement.
        let start = CalibrationStart::begin(self, kind);

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

        start.acknowledged(result.as_ref().map(|_| ()).map_err(ToString::to_string));
        result
    }

    /// Tell the vehicle the operator did what the running calibration asks
    /// (placed it in the requested orientation).
    ///
    /// # Errors
    ///
    /// [`Error::NotCalibrating`] if no run is waiting for a confirmation,
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet, or
    /// [`Error::Rejected`] / [`Error::Timeout`] / [`Error::NoLink`]. The
    /// command is never resent.
    pub async fn confirm_calibration_step(&self) -> Result<()> {
        let (target, confirmation) = self.with_vehicle(|vehicle| {
            (vehicle.target(), vehicle.calibration_state().confirmation())
        })?;
        let Confirmation { command, run, side } = confirmation.ok_or(Error::NotCalibrating)?;

        // The operator is done; the vehicle takes over. Until its next
        // progress the session must not ask again: a repeat would be refused
        // once measuring has begun.
        self.report_calibration(CalibrationUpdate::Confirmed { run, side });

        // Not resent either: once the vehicle measures, a repeat is refused.
        let result = self
            .fleet
            .router()
            .command(target, command)
            .timeout(CALIBRATION_CONFIRM_TIMEOUT)
            .retries(0)
            .await
            .map(|_| ())
            .map_err(Error::from);

        if result.is_err() {
            self.report_calibration(CalibrationUpdate::ConfirmFailed { run, side });
        }

        result
    }

    /// Cancel the running calibration. The session ends at once; the vehicle
    /// is told to stop when its firmware has a way to.
    ///
    /// # Errors
    ///
    /// [`Error::NotCalibrating`] if no run is going, or
    /// [`Error::UnknownVehicle`] if the vehicle left the fleet. Failing to
    /// tell the vehicle is logged rather than returned: the session has ended
    /// either way.
    pub async fn cancel_calibration(&self) -> Result<()> {
        let (target, running, cancel) = self.with_vehicle(|vehicle| {
            let calibration = vehicle.calibration_state();
            (
                vehicle.target(),
                calibration.is_running(),
                calibration.cancel_command(),
            )
        })?;
        if !running {
            return Err(Error::NotCalibrating);
        }
        self.report_calibration(CalibrationUpdate::Cancel);

        if let Some(cancel) = cancel {
            // The retry count is the firmware's: PX4 acknowledges a cancel
            // twice, once refusing it as a new command while busy, and the
            // outcome shows in its texts instead.
            if let Err(error) = self
                .fleet
                .router()
                .command(target, cancel.command)
                .retries(cancel.retries)
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

    /// Command the vehicle's mode of `kind` (return, land, ...), whatever its
    /// firmware calls it. Returns the mode commanded.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchMode`] if the vehicle has no mode of `kind`,
    /// [`Error::UnknownVehicle`] if it left the fleet, or whatever
    /// [`Self::set_mode`] fails with.
    pub async fn set_mode_kind(&self, kind: ModeKind) -> Result<FlightMode> {
        let mode = self
            .with_vehicle(|vehicle| vehicle.mode_of_kind(kind))?
            .ok_or(Error::NoSuchMode)?;

        self.set_mode(mode).await.map(|()| mode)
    }
}
