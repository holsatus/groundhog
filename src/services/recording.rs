//! Telemetry recording in the `.tlog` format.
//!
//! A tlog is the de-facto MAVLink flight recording (QGroundControl, Mission
//! Planner and pymavlink read it): every frame prefixed with its reception
//! time as a big-endian `u64` of microseconds since the Unix epoch. The
//! recorder taps the router like any transaction does, so recording sees
//! every inbound frame exactly once, independent of the UI and of the fleet
//! runtime.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::io::{AsyncWriteExt, BufWriter};

use crate::mavlink::MavlinkRouter;

/// How often buffered frames are flushed to disk, bounding what a crash loses.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// A running recording. Stops (and flushes) when dropped.
pub struct TlogRecorder {
    path: PathBuf,
    _task: tokio::task::JoinHandle<()>,
    stop: tokio_util::sync::DropGuard,
}

impl TlogRecorder {
    /// Start recording inbound frames from `router` into a new file in
    /// `directory`, named after the current UTC time.
    ///
    /// Must be called within an async runtime.
    pub fn start(router: &MavlinkRouter, directory: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("{}.tlog", file_stamp(SystemTime::now())));
        let file = std::fs::File::create(&path)?;

        let mut frames = router.receive_filtered(|_| true);
        let cancel = tokio_util::sync::CancellationToken::new();
        let stopped = cancel.clone();
        let log_path = path.clone();

        let task = tokio::spawn(async move {
            let mut writer = BufWriter::new(tokio::fs::File::from_std(file));
            let mut flush = tokio::time::interval(FLUSH_INTERVAL);
            let mut buffer = vec![0u8; 512];

            loop {
                tokio::select! {
                    _ = stopped.cancelled() => break,
                    _ = flush.tick() => {
                        if let Err(error) = writer.flush().await {
                            log::error!("Telemetry log {}: {error}", log_path.display());
                            return;
                        }
                    }
                    inbound = frames.next() => {
                        let Some(inbound) = inbound else { break };

                        let micros = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|elapsed| elapsed.as_micros() as u64)
                            .unwrap_or(0);

                        if buffer.len() < inbound.frame.size() {
                            buffer.resize(inbound.frame.size(), 0);
                        }
                        let Ok(length) = inbound.frame.serialize(&mut buffer) else {
                            continue;
                        };

                        let written = async {
                            writer.write_all(&micros.to_be_bytes()).await?;
                            writer.write_all(&buffer[..length]).await
                        };
                        if let Err(error) = written.await {
                            log::error!("Telemetry log {}: {error}", log_path.display());
                            return;
                        }
                    }
                }
            }

            let _ = writer.flush().await;
        });

        log::info!("Recording telemetry to {}", path.display());

        Ok(Self {
            path,
            _task: task,
            stop: cancel.drop_guard(),
        })
    }

    /// The file being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stop recording, flushing what was buffered.
    pub fn stop(self) {
        drop(self.stop);
    }
}

/// `YYYY-MM-DD_HH-MM-SS` in UTC, for file names that sort chronologically.
fn file_stamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);

    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let of_day = seconds % 86_400;

    format!(
        "{year:04}-{month:02}-{day:02}_{:02}-{:02}-{:02}",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{link::LinkId, mavlink::DEFAULT_GCS_ID};
    use mavio::{dialects::minimal::messages::Heartbeat, prelude::V2, protocol::FrameBuilder};

    #[test]
    fn stamps_are_utc_calendar_times() {
        assert_eq!(file_stamp(UNIX_EPOCH), "1970-01-01_00-00-00");
        // 2024-02-29T12:34:56Z, a leap day.
        let leap = UNIX_EPOCH + Duration::from_secs(1_709_210_096);
        assert_eq!(file_stamp(leap), "2024-02-29_12-34-56");
    }

    #[tokio::test]
    async fn inbound_frames_are_written_with_timestamps() {
        let directory = std::env::temp_dir().join(format!("groundhog-tlog-{}", std::process::id()));
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let recorder = TlogRecorder::start(&router, &directory).unwrap();
        let path = recorder.path().to_path_buf();

        let frame = FrameBuilder::new()
            .system_id(1)
            .component_id(1)
            .version(V2)
            .sequence(0)
            .message(&Heartbeat::default())
            .unwrap()
            .build();
        let mut bytes = vec![0u8; frame.size()];
        let length = frame.serialize(&mut bytes).unwrap();
        bytes.truncate(length);

        let (mut peer, gcs) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(gcs);
        let _guard = router.attach_reader_writer(LinkId::new("test"), reader, writer, false);
        peer.write_all(&bytes).await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        recorder.stop();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let written = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_dir_all(&directory);

        assert_eq!(written.len(), 8 + bytes.len());
        assert_eq!(&written[8..], bytes.as_slice());
        let micros = u64::from_be_bytes(written[..8].try_into().unwrap());
        assert!(
            micros > 1_600_000_000_000_000,
            "a plausible wall-clock time"
        );
    }
}
