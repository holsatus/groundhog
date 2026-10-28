//! Recording a stream as it arrives: a branch off the pipeline's compressed
//! stream into a Matroska file, with no decoding and no encoding again.
//!
//! Every pipeline but the test pattern's has a `tee` where the stream is
//! still compressed ([`source::ENCODED`], after the parser, before the
//! decoder). A recording is a branch added to it while the pipeline plays:
//!
//! - a leaky queue, so a slow disk loses recorded frames, never live ones;
//! - the codec's parser, into the form Matroska stores (with the decoder's
//!   setup in the file's header);
//! - `matroskamux`, which writes as it goes and its index last, so a file
//!   cut off by a crash or a dead battery still plays to near its end (an
//!   MP4 cut off does not play at all);
//! - the file.
//!
//! The branch passes nothing until a keyframe, so the file starts with a
//! frame that decodes, at time zero. Its `DateUTC` is the wall-clock time
//! that keyframe arrived, the clock a telemetry log stamps its frames with.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gstreamer::{self as gst, glib, prelude::*};

use crate::source;

/// How much the branch's queue holds before it drops the oldest frames.
const QUEUE: Duration = Duration::from_secs(5);

/// How long finishing a file may take: the frames queued, then the index.
const FINISH_TIMEOUT: Duration = Duration::from_secs(3);

/// A recording branch of a running pipeline.
pub(crate) struct Branch {
    tee: gst::Element,
    tee_pad: gst::Pad,
    /// The queue, the parser, the muxer and the file's sink, in order.
    elements: [gst::Element; 4],
    directory: PathBuf,
    path: PathBuf,
    /// Whether a keyframe went in, so the file holds video.
    started: Arc<AtomicBool>,
}

impl Branch {
    /// Start recording `pipeline`'s stream into a new file in `directory`.
    /// `on_start` is told the file's path when its first keyframe goes in,
    /// on a streaming thread.
    ///
    /// `Ok(None)` while the stream is not known yet (no video has come);
    /// try again later.
    ///
    /// # Errors
    ///
    /// Why the stream cannot be recorded: it is the test pattern, its codec
    /// cannot be stored, a GStreamer element is missing, or the file
    /// cannot be created.
    pub(crate) fn attach(
        pipeline: &gst::Pipeline,
        directory: &Path,
        on_start: impl Fn(&Path) + Send + Sync + 'static,
    ) -> Result<Option<Self>, String> {
        let tee = pipeline
            .by_name(source::ENCODED)
            .ok_or_else(|| "The test pattern is not recorded".to_string())?;
        let Some(caps) = tee.static_pad("sink").and_then(|pad| pad.current_caps()) else {
            return Ok(None);
        };
        let codec = caps
            .structure(0)
            .map(|structure| structure.name().to_string())
            .unwrap_or_default();
        let parser = match codec.as_str() {
            "video/x-h264" => "h264parse",
            "video/x-h265" => "h265parse",
            _ => return Err(format!("A {codec} stream cannot be recorded")),
        };

        std::fs::create_dir_all(directory)
            .map_err(|error| format!("Cannot create {}: {error}", directory.display()))?;
        let path = new_file(directory)?;
        match Self::build(pipeline, tee, parser, directory, path.clone(), on_start) {
            Ok(branch) => Ok(Some(branch)),
            Err(error) => {
                drop(std::fs::remove_file(&path));
                Err(error)
            }
        }
    }

    fn build(
        pipeline: &gst::Pipeline,
        tee: gst::Element,
        parser: &str,
        directory: &Path,
        path: PathBuf,
        on_start: impl Fn(&Path) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let missing = |factory: &str| format!("Cannot record: GStreamer's {factory} is missing");
        let location = path
            .to_str()
            .ok_or_else(|| format!("Cannot record into {}", path.display()))?;

        let queue = gst::ElementFactory::make("queue")
            .property("max-size-buffers", 0u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", QUEUE.as_nanos() as u64)
            .property_from_str("leaky", "downstream")
            .build()
            .map_err(|_| missing("queue"))?;
        let parse = gst::ElementFactory::make(parser)
            .build()
            .map_err(|_| missing(parser))?;
        let mux = gst::ElementFactory::make("matroskamux")
            .property("offset-to-zero", true)
            .property("writing-app", "Groundhog")
            .build()
            .map_err(|_| missing("matroskamux"))?;
        let sink = gst::ElementFactory::make("filesink")
            .property("location", location)
            .property("sync", false)
            .property("async", false)
            .build()
            .map_err(|_| missing("filesink"))?;
        let elements = [queue, parse, mux.clone(), sink];

        let added = pipeline
            .add_many(&elements)
            .map_err(|error| error.to_string());
        if let Err(error) = added {
            return Err(format!("Cannot record: {error}"));
        }
        let linked = gst::Element::link_many(&elements)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                elements.iter().rev().try_for_each(|element| {
                    element
                        .sync_state_with_parent()
                        .map_err(|error| error.to_string())
                })
            });
        let tee_pad = linked.and_then(|()| {
            tee.request_pad_simple("src_%u")
                .ok_or_else(|| "no pad on the stream's tee".to_string())
        });
        let tee_pad = match tee_pad {
            Ok(pad) => pad,
            Err(error) => {
                remove(pipeline, &elements);
                return Err(format!("Cannot record: {error}"));
            }
        };

        // Nothing goes in before a keyframe. The parser marks a keyframe
        // that carries the decoder's setup as a header too, and setup sent
        // on its own comes just before a keyframe: either starts the file.
        let started = Arc::new(AtomicBool::new(false));
        let first = started.clone();
        let target = path.clone();
        let weak = pipeline.downgrade();
        tee_pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            if buffer.flags().contains(gst::BufferFlags::DELTA_UNIT) {
                return gst::PadProbeReturn::Drop;
            }

            let arrived = weak
                .upgrade()
                .and_then(|pipeline| arrival(&pipeline, pad, buffer.pts()));
            if let Some(date) = arrived.and_then(date_time) {
                mux.set_property("creation-time", &date);
            }
            first.store(true, Ordering::Relaxed);
            on_start(&target);
            gst::PadProbeReturn::Remove
        });

        let linked = elements[0]
            .static_pad("sink")
            .ok_or_else(|| "the queue has no sink".to_string())
            .and_then(|sink| {
                tee_pad
                    .link(&sink)
                    .map(drop)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = linked {
            tee.release_request_pad(&tee_pad);
            remove(pipeline, &elements);
            return Err(format!("Cannot record: {error}"));
        }

        Ok(Self {
            tee,
            tee_pad,
            elements,
            directory: directory.to_path_buf(),
            path,
            started,
        })
    }

    /// The directory the file is in.
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    /// Whether `object` is one of the branch's elements, so its error is
    /// the recording's, not the stream's.
    pub(crate) fn owns(&self, object: &gst::Object) -> bool {
        self.elements
            .iter()
            .any(|element| element.upcast_ref::<gst::Object>() == object)
    }

    /// Stop recording and finish the file: what is queued is written, then
    /// the index. A file that never got a keyframe is deleted.
    pub(crate) fn finish(self, pipeline: &gst::Pipeline) {
        let [queue, .., sink] = &self.elements;

        let (finished, written) = mpsc::sync_channel(1);
        if let Some(pad) = sink.static_pad("sink") {
            pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
                if info
                    .event()
                    .is_some_and(|event| event.type_() == gst::EventType::Eos)
                {
                    let _ = finished.try_send(());
                }
                gst::PadProbeReturn::Ok
            });
        }

        // Unlink from the tee between two buffers, then end the branch's
        // stream, which the muxer answers by writing its index.
        let queue_sink = queue.static_pad("sink");
        self.tee_pad
            .add_probe(gst::PadProbeType::IDLE, move |pad, _| {
                if let Some(queue_sink) = &queue_sink {
                    drop(pad.unlink(queue_sink));
                    queue_sink.send_event(gst::event::Eos::new());
                }
                gst::PadProbeReturn::Remove
            });

        if written.recv_timeout(FINISH_TIMEOUT).is_err() {
            log::warn!(
                "Video: {} may not be finished properly",
                self.path.display()
            );
        }
        self.tee.release_request_pad(&self.tee_pad);
        remove(pipeline, &self.elements);

        if !self.started.load(Ordering::Relaxed) {
            drop(std::fs::remove_file(&self.path));
        }
    }
}

/// Take `elements` out of `pipeline`, stopped.
fn remove(pipeline: &gst::Pipeline, elements: &[gst::Element]) {
    for element in elements {
        let _ = element.set_state(gst::State::Null);
        drop(pipeline.remove(element));
    }
}

/// The wall-clock time a buffer timestamped `pts` arrived on `pad`: now,
/// less how long ago (in the pipeline's running time) that was. A jitter
/// buffer that held the frame does not make it later.
fn arrival(
    pipeline: &gst::Pipeline,
    pad: &gst::Pad,
    pts: Option<gst::ClockTime>,
) -> Option<SystemTime> {
    let now = SystemTime::now();
    let running_now = pipeline.current_running_time()?;
    let segment = pad.sticky_event::<gst::event::Segment>(0)?;
    let running_then = segment
        .segment()
        .downcast_ref::<gst::format::Time>()?
        .to_running_time(pts?)?;
    let ago = running_now.saturating_sub(running_then);
    now.checked_sub(Duration::from_nanos(ago.nseconds()))
}

/// `time` as GLib's date, in UTC, to the microsecond.
fn date_time(time: SystemTime) -> Option<glib::DateTime> {
    let since_epoch = time.duration_since(UNIX_EPOCH).ok()?;
    let seconds = i64::try_from(since_epoch.as_secs()).ok()?;
    glib::DateTime::from_unix_utc(seconds)
        .and_then(|date| date.add_seconds(f64::from(since_epoch.subsec_micros()) / 1e6))
        .ok()
}

/// A new, empty file named after the time in `directory`
/// (`YYYY-MM-DD_HH-MM-SS.mkv`, in UTC, as telemetry logs are), or with
/// `_2` and on if a recording that second exists: one is never truncated.
fn new_file(directory: &Path) -> Result<PathBuf, String> {
    let stamp = glib::DateTime::now_utc()
        .and_then(|now| now.format("%Y-%m-%d_%H-%M-%S"))
        .map_err(|error| format!("Cannot name the recording: {error}"))?;

    let mut number = 1;
    loop {
        let path = match number {
            1 => directory.join(format!("{stamp}.mkv")),
            _ => directory.join(format!("{stamp}_{number}.mkv")),
        };
        match std::fs::File::create_new(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && number < 100 => {
                number += 1;
            }
            Err(error) => {
                return Err(format!("Cannot create {}: {error}", path.display()));
            }
        }
    }
}
