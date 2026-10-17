//! The fleet runtime: the task that owns vehicle state.
//!
//! The runtime drains the router's lossless event channel, decodes each frame,
//! folds it into the [`Fleet`], ages vehicles on a timer, and bootstraps newly
//! registered vehicles (capabilities, home, parameters, mission). It runs on
//! the async runtime, independent of the UI: vehicle state, liveness and the
//! events derived from them keep advancing while the UI is busy or blocked.
//!
//! Readers never borrow the fleet. The runtime publishes an immutable
//! [`FleetSnapshot`] at most every [`PUBLISH_INTERVAL`] when something
//! changed, along with the [`FleetEvent`]s produced since the last
//! publication. Snapshots share vehicle data with the fleet, so publishing
//! copies only what changed since the previous snapshot.

use std::{sync::Arc, time::Duration};

use mavio::{
    DefaultDialect,
    default_dialect::{enums::MagCalStatus, messages::MagCalReport},
    error::SpecError,
    mavspec::rust::spec::Dialect,
};
use tokio::{
    sync::{mpsc, watch},
    time::{MissedTickBehavior, interval},
};

use super::fleet::FleetHandle;
use crate::{
    domain::{
        SystemId,
        calibration::CalibrationUpdate,
        fleet::{Fleet, FleetEvent, FleetSnapshot},
        mission::OnboardMission,
        parameter::DownloadState,
    },
    foundation::MavlinkId,
    mavlink::{Event, EventReceiver, MavlinkRouter},
};

/// The most often a snapshot is published: the UI's redraw cadence for
/// telemetry. 30 Hz is smooth for a HUD and keeps redraw work bounded.
pub const PUBLISH_INTERVAL: Duration = Duration::from_millis(33);

/// How often vehicles are aged for liveness.
const LIVENESS_INTERVAL: Duration = Duration::from_millis(250);

/// State changes reported to the runtime by operations running elsewhere.
#[derive(Debug)]
pub(super) enum Input {
    OnboardMission {
        system: SystemId,
        mission: OnboardMission,
    },
    ParamDownload {
        address: MavlinkId,
        state: DownloadState,
    },
    Calibration {
        system: SystemId,
        update: CalibrationUpdate,
    },
}

/// What the runtime publishes to its reader, at most every
/// [`PUBLISH_INTERVAL`].
#[derive(Clone)]
pub struct FleetUpdate {
    pub snapshot: Arc<FleetSnapshot>,
    /// Everything that happened since the previous update, in order.
    pub events: Vec<FleetEvent>,
}

/// A summary, not the snapshot: UI runtimes format messages for diagnostics
/// (iced's `debug` feature pretty-prints every message on the UI thread), and
/// a full fleet dump costs tens of milliseconds per update.
impl std::fmt::Debug for FleetUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FleetUpdate")
            .field("vehicles", &self.snapshot.ids().collect::<Vec<_>>())
            .field("events", &self.events)
            .finish()
    }
}

/// Options for [`spawn`].
#[derive(Debug, Clone, Copy)]
pub struct RuntimeOptions {
    /// Bootstrap newly registered vehicles (request capabilities and home,
    /// download parameters and mission).
    pub bootstrap: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self { bootstrap: true }
    }
}

/// Start the runtime on the current async runtime, consuming `events` from
/// `router`. Returns the handle operations go through, and the receiving end
/// of the update channel for the (single) reader.
pub fn spawn(
    router: MavlinkRouter,
    events: EventReceiver,
    options: RuntimeOptions,
) -> (FleetHandle, FleetUpdates) {
    let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(FleetSnapshot::default()));
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    let handle = FleetHandle::new(router, input_tx, snapshot_rx.clone());

    tokio::spawn(run(
        handle.clone(),
        events,
        input_rx,
        snapshot_tx,
        event_tx,
        options,
    ));

    (
        handle,
        FleetUpdates {
            snapshot: snapshot_rx,
            events: event_rx,
        },
    )
}

async fn run(
    handle: FleetHandle,
    mut router_events: EventReceiver,
    mut inputs: mpsc::UnboundedReceiver<Input>,
    snapshot: watch::Sender<Arc<FleetSnapshot>>,
    updates: mpsc::UnboundedSender<FleetEvent>,
    options: RuntimeOptions,
) {
    let mut fleet = Fleet::default();
    let mut pending: Vec<FleetEvent> = Vec::new();
    let mut dirty = false;

    let mut publish = interval(PUBLISH_INTERVAL);
    publish.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut liveness = interval(LIVENESS_INTERVAL);
    liveness.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        // Ticks first: a busy event stream must not starve publication.
        tokio::select! {
            biased;

            _ = publish.tick() => {
                if dirty || !pending.is_empty() {
                    // Snapshot before events, so a reader woken by an event
                    // already sees the state it describes.
                    snapshot.send_replace(Arc::new(fleet.snapshot()));
                    for event in pending.drain(..) {
                        if updates.send(event).is_err() {
                            log::debug!("fleet update reader is gone");
                        }
                    }
                    dirty = false;
                }
            }

            now = liveness.tick() => {
                let events = fleet.tick(now.into_std());
                dirty |= !events.is_empty();
                pending.extend(events);
            }

            input = inputs.recv() => match input {
                Some(input) => {
                    pending.extend(apply(&mut fleet, input));
                    dirty = true;
                }
                // Every handle, including our own, is gone.
                None => break,
            },

            event = router_events.recv() => match event {
                Some(Event::Message(inbound)) => {
                    let decoded = decode(&inbound.frame);
                    let events = fleet.observe(inbound.source_mav, inbound.received_at, decoded.as_ref());

                    for event in &events {
                        if let FleetEvent::Registered(system) = event
                            && options.bootstrap
                        {
                            let client = handle.vehicle(*system);
                            tokio::spawn(async move { client.bootstrap().await });
                        }
                    }

                    pending.extend(events);
                    dirty = true;
                }
                Some(Event::LinkClosed(link)) => log::info!("Link {link:?} closed"),
                None => break,
            },
        }
    }

    log::info!("Fleet runtime stopped");
}

fn apply(fleet: &mut Fleet, input: Input) -> Vec<FleetEvent> {
    match input {
        Input::OnboardMission { system, mission } => {
            fleet.set_onboard_mission(system, mission);
        }
        Input::ParamDownload { address, state } => {
            fleet.set_param_download(address, state);
        }
        Input::Calibration { system, update } => {
            return fleet.update_calibration(system, update);
        }
    }
    Vec::new()
}

/// Decode a frame into the default dialect, quietly skipping messages from
/// other dialects.
fn decode(frame: &mavio::Frame<mavio::prelude::Versionless>) -> Option<DefaultDialect> {
    match frame.decode::<DefaultDialect>() {
        Ok(message) => Some(message),
        Err(mavio::Error::Spec(SpecError::InvalidEnumValue { .. }))
            if frame.message_id() == MagCalReport::message_id() =>
        {
            decode_mag_cal_report(frame)
        }
        Err(mavio::Error::Spec(SpecError::NotInDialect(id))) => {
            log::trace!("mavlink message id {id} is not in the default dialect");
            None
        }
        Err(error) => {
            log::trace!("could not decode mavlink frame: {error}");
            None
        }
    }
}

/// The offset of `cal_status` in the `MAG_CAL_REPORT` payload: after ten
/// `f32`s, `compass_id` and `cal_mask`.
const MAG_CAL_REPORT_STATUS: usize = 42;

/// Decode a `MAG_CAL_REPORT` whose status the bundled dialect does not know.
///
/// The message definitions mavio generates from predate the failure reasons
/// ArduPilot reports since (`MAG_CAL_FAILED_OFFSETS` and later), and a report
/// carrying one would not decode at all: the calibration would never be seen
/// to fail. Such a status is read as the generic `MAG_CAL_FAILED`.
fn decode_mag_cal_report(
    frame: &mavio::Frame<mavio::prelude::Versionless>,
) -> Option<DefaultDialect> {
    frame
        .validate_checksum_with_crc_extra(MagCalReport::crc_extra())
        .ok()?;

    let payload = frame.payload();
    let mut bytes = payload.bytes().to_vec();
    // MAVLink 2 truncates trailing zeroes; the status is never trailing.
    let status = bytes.get_mut(MAG_CAL_REPORT_STATUS)?;
    log::debug!("MAG_CAL_REPORT with unknown status {status}; reading it as failed");
    *status = MagCalStatus::MagCalFailed as u8;

    let patched = mavio::protocol::Payload::new(payload.id(), &bytes, payload.version());
    DefaultDialect::decode(&patched).ok()
}

/// The reading end of the runtime's publications.
pub struct FleetUpdates {
    snapshot: watch::Receiver<Arc<FleetSnapshot>>,
    events: mpsc::UnboundedReceiver<FleetEvent>,
}

impl FleetUpdates {
    /// Wait for the next publication. Returns `None` once the runtime stopped.
    pub async fn next(&mut self) -> Option<FleetUpdate> {
        let mut events = Vec::new();

        tokio::select! {
            changed = self.snapshot.changed() => {
                if changed.is_err() {
                    return None;
                }
            }
            event = self.events.recv() => match event {
                Some(event) => events.push(event),
                None => return None,
            },
        }

        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }

        Some(FleetUpdate {
            snapshot: self.snapshot.borrow_and_update().clone(),
            events,
        })
    }

    /// The publications as a stream, for a UI runtime to drive.
    pub fn into_stream(self) -> impl futures::Stream<Item = FleetUpdate> + Send + 'static {
        futures::stream::unfold(self, |mut updates| async move {
            updates.next().await.map(|update| (update, updates))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{domain::fleet::Fleet, foundation::MavlinkId};
    use mavio::default_dialect::{
        enums::{MavAutopilot, MavParamType, MavType},
        messages,
    };
    use std::time::Instant;

    /// Messages are formatted on the UI thread by iced's debug tooling; an
    /// update must not dump the fleet.
    #[test]
    fn updates_format_as_a_summary() {
        let mut fleet = Fleet::default();
        let source = MavlinkId {
            system: 1,
            component: 1,
        };
        let now = Instant::now();
        fleet.observe(
            source,
            now,
            Some(&DefaultDialect::Heartbeat(messages::Heartbeat {
                type_: MavType::Quadrotor,
                autopilot: MavAutopilot::Ardupilotmega,
                ..Default::default()
            })),
        );
        for index in 0..500u16 {
            fleet.observe(
                source,
                now,
                Some(&DefaultDialect::GlobalPositionInt(
                    messages::GlobalPositionInt::default(),
                )),
            );
            let mut param_id = [0u8; 16];
            param_id[..4].copy_from_slice(&index.to_le_bytes().repeat(2));
            fleet.observe(
                source,
                now,
                Some(&DefaultDialect::ParamValue(messages::ParamValue {
                    param_id,
                    param_type: MavParamType::Real32,
                    param_count: 500,
                    param_index: index,
                    ..Default::default()
                })),
            );
        }

        let update = FleetUpdate {
            snapshot: Arc::new(fleet.snapshot()),
            events: Vec::new(),
        };

        assert!(format!("{update:#?}").len() < 200);
    }
}
