//! Recording a stream as it arrives, into a Matroska file, with no
//! decoding and no encoding again.
//!
//! A stream that comes as RTP (`udp://`, `udp265://`, RTSP) is recorded
//! from its packets as they arrive, before the picture's jitter buffer:
//! a [`Tap`] copies them into a branch with a jitter buffer of its own,
//! [`JITTER`] long. The picture waits for nothing, so its jitter buffer
//! drops what comes late (or there is none, and its frames are stamped
//! with when they arrived); the file can wait, so it keeps a packet that
//! a radio delivers late, and stamps each frame from the stream's own
//! clock, so it plays at the camera's pace. The tap stamps the packets
//! with their arrival in the branch's own time, so a restart of the
//! stream's timestamps upstream (an RTSP session that reconnects) never
//! reaches the file. The branch:
//!
//! - an `appsrc` holding up to [`QUEUE`], dropping the oldest packets, so
//!   a slow disk loses recorded frames, never live ones;
//! - `rtpjitterbuffer`, then the codec's depayloader and parser, into the
//!   form Matroska stores (with the decoder's setup in the file's header);
//! - `matroskamux`, which writes as it goes and its index last, so a file
//!   cut off by a crash or a dead battery still plays to near its end (an
//!   MP4 cut off does not play at all);
//! - the file.
//!
//! Any other stream (MPEG-TS, which carries its own timestamps) is
//! recorded from the pipeline's `tee` where it is still compressed
//! ([`source::ENCODED`], after the parser, before the decoder), through a
//! leaky queue instead of the source and the jitter buffer.
//!
//! The branch passes nothing until a keyframe, so the file starts with a
//! frame that decodes, at time zero, and never a frame without a
//! timestamp (what is left of one whose packets were lost), which the
//! file could only put at a nonsense time. Its `DateUTC` is the
//! wall-clock time that keyframe arrived, the clock a telemetry log
//! stamps its frames with.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_app as gst_app;

use crate::source;

/// How much the branch's queue holds before it drops the oldest frames.
const QUEUE: Duration = Duration::from_secs(5);

/// How long the recording's jitter buffer waits for a late packet. Radio
/// links stall for a few hundred milliseconds at a time and then deliver
/// what they held; the file can wait that long.
const JITTER: Duration = Duration::from_secs(2);

/// How long finishing a file may take: the frames queued, then the index.
const FINISH_TIMEOUT: Duration = Duration::from_secs(3);

/// Where an RTP stream's packets are copied into a recording, as they
/// arrive: before the picture's jitter buffer, if it has one.
#[derive(Clone, Default)]
pub(crate) struct Tap(Arc<Mutex<Tapped>>);

#[derive(Default)]
struct Tapped {
    /// The packets' caps, once one has passed the newest tapped pad.
    caps: Option<gst::Caps>,
    /// The recording's source, while one runs.
    into: Option<gst_app::AppSrc>,
}

impl Tap {
    /// Copy the packets that pass `pad` into the recording, from now on,
    /// in place of any pad tapped before: a reconnected session receives
    /// on a pad of its own.
    pub(crate) fn install(&self, pad: &gst::Pad) {
        self.lock().caps = None;
        let tapped = self.0.clone();
        pad.add_probe(
            gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
            move |pad, info| {
                let mut tapped = tapped.lock().unwrap_or_else(PoisonError::into_inner);
                if tapped.caps.is_none() {
                    tapped.caps = pad.current_caps();
                }
                if let Some(into) = &tapped.into {
                    match &info.data {
                        Some(gst::PadProbeData::Buffer(packet)) => push(into, packet),
                        Some(gst::PadProbeData::BufferList(packets)) => {
                            for packet in packets.iter_owned() {
                                push(into, &packet);
                            }
                        }
                        _ => {}
                    }
                }
                gst::PadProbeReturn::Ok
            },
        );
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tapped> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn caps(&self) -> Option<gst::Caps> {
        self.lock().caps.clone()
    }
}

/// Hand `packet` to the recording, stamped with when it arrived in the
/// recording's own time. Dropped if the recording's queue is full or it
/// is finishing.
fn push(into: &gst_app::AppSrc, packet: &gst::Buffer) {
    let arrived = into.current_running_time();
    let mut packet = packet.clone();
    let stamped = packet.make_mut();
    stamped.set_pts(arrived);
    stamped.set_dts(arrived);
    let _ = into.push_buffer(packet);
}

/// Where a branch is to take the stream from.
enum Origin {
    /// The packets `tap` copies, in `caps`, depayloaded by `depay`.
    Packets {
        tap: Tap,
        caps: gst::Caps,
        depay: &'static str,
    },
    /// The pipeline's `tee`.
    Encoded(gst::Element),
}

/// What a branch takes the stream from.
enum Input {
    /// The packets, through a [`Tap`] into the branch's `appsrc`.
    Packets(Tap),
    /// The compressed stream, from the pipeline's `tee`.
    Encoded { tee: gst::Element, pad: gst::Pad },
}

/// A recording branch of a running pipeline.
pub(crate) struct Branch {
    input: Input,
    /// The branch's elements, in order, its first taking the input and its
    /// last the file's sink.
    elements: Vec<gst::Element>,
    directory: PathBuf,
    path: PathBuf,
    /// Whether a keyframe went in, so the file holds video.
    started: Arc<AtomicBool>,
}

impl Branch {
    /// Start recording `pipeline`'s stream into a new file in `directory`,
    /// from the packets `tap` copies where the stream is RTP. `on_start` is
    /// told the file's path when its first keyframe goes in, on a
    /// streaming thread.
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
        tap: Option<&Tap>,
        directory: &Path,
        on_start: impl Fn(&Path) + Send + Sync + 'static,
    ) -> Result<Option<Self>, String> {
        let packets = match tap {
            Some(tap) => {
                let Some(caps) = tap.caps() else {
                    return Ok(None);
                };
                let encoding = caps
                    .structure(0)
                    .and_then(|structure| structure.get::<&str>("encoding-name").ok())
                    .map(str::to_ascii_uppercase);
                match encoding.as_deref() {
                    Some("H264") => Some((tap.clone(), caps, "rtph264depay", "h264parse")),
                    Some("H265") => Some((tap.clone(), caps, "rtph265depay", "h265parse")),
                    // Another payload (MPEG-TS over RTSP) is recorded from
                    // the tee, as it is shown.
                    _ => None,
                }
            }
            None => None,
        };

        let (origin, parser) = match packets {
            Some((tap, caps, depay, parser)) => (Origin::Packets { tap, caps, depay }, parser),
            None => {
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
                (Origin::Encoded(tee), parser)
            }
        };

        std::fs::create_dir_all(directory)
            .map_err(|error| format!("Cannot create {}: {error}", directory.display()))?;
        let path = new_file(directory)?;
        match Self::build(pipeline, origin, parser, directory, path.clone(), on_start) {
            Ok(branch) => Ok(Some(branch)),
            Err(error) => {
                drop(std::fs::remove_file(&path));
                Err(error)
            }
        }
    }

    fn build(
        pipeline: &gst::Pipeline,
        origin: Origin,
        parser: &str,
        directory: &Path,
        path: PathBuf,
        on_start: impl Fn(&Path) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let missing = |factory: &str| format!("Cannot record: GStreamer's {factory} is missing");
        let location = path
            .to_str()
            .ok_or_else(|| format!("Cannot record into {}", path.display()))?;

        let mut elements = match &origin {
            Origin::Encoded(_) => vec![
                gst::ElementFactory::make("queue")
                    .property("max-size-buffers", 0u32)
                    .property("max-size-bytes", 0u32)
                    .property("max-size-time", QUEUE.as_nanos() as u64)
                    .property_from_str("leaky", "downstream")
                    .build()
                    .map_err(|_| missing("queue"))?,
            ],
            Origin::Packets { caps, depay, .. } => vec![
                gst::ElementFactory::make("appsrc")
                    .property("caps", caps)
                    .property("format", gst::Format::Time)
                    .property("is-live", true)
                    .property("max-buffers", 0u64)
                    .property("max-bytes", 0u64)
                    .property("max-time", QUEUE.as_nanos() as u64)
                    .property_from_str("leaky-type", "downstream")
                    .build()
                    .map_err(|_| missing("appsrc"))?,
                gst::ElementFactory::make("rtpjitterbuffer")
                    .property("latency", JITTER.as_millis() as u32)
                    .property_from_str("mode", "none")
                    .build()
                    .map_err(|_| missing("rtpjitterbuffer"))?,
                gst::ElementFactory::make(depay)
                    .build()
                    .map_err(|_| missing(depay))?,
            ],
        };
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
        elements.extend([parse.clone(), mux.clone(), sink]);

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
        if let Err(error) = linked {
            remove(pipeline, &elements);
            return Err(format!("Cannot record: {error}"));
        }

        // Nothing goes in before a keyframe. The parser marks a keyframe
        // that carries the decoder's setup as a header too, and setup sent
        // on its own comes just before a keyframe: either starts the file.
        let started = Arc::new(AtomicBool::new(false));
        let first = started.clone();
        let target = path.clone();
        let weak = pipeline.downgrade();
        let parsed = parse
            .static_pad("src")
            .ok_or_else(|| "the parser has no source".to_string());
        let parsed = match parsed {
            Ok(pad) => pad,
            Err(error) => {
                remove(pipeline, &elements);
                return Err(format!("Cannot record: {error}"));
            }
        };
        parsed.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            if buffer.pts().is_none() {
                return gst::PadProbeReturn::Drop;
            }
            if first.load(Ordering::Relaxed) {
                return gst::PadProbeReturn::Ok;
            }
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
            gst::PadProbeReturn::Ok
        });

        let input = match origin {
            Origin::Encoded(tee) => match link_to_tee(&tee, &elements[0]) {
                Ok(pad) => Input::Encoded { tee, pad },
                Err(error) => {
                    remove(pipeline, &elements);
                    return Err(format!("Cannot record: {error}"));
                }
            },
            Origin::Packets { tap, .. } => {
                tap.lock().into = elements[0].clone().downcast().ok();
                Input::Packets(tap)
            }
        };

        Ok(Self {
            input,
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

    /// Stop recording and finish the file: what is queued (and what the
    /// jitter buffer holds) is written, then the index. A file that never
    /// got a keyframe is deleted.
    pub(crate) fn finish(self, pipeline: &gst::Pipeline) {
        let (finished, written) = mpsc::sync_channel(1);
        let sink = self
            .elements
            .last()
            .and_then(|sink| sink.static_pad("sink"));
        if let Some(pad) = sink {
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

        // End the branch's stream, which the muxer answers by writing its
        // index: no more packets into the source, or, from the tee,
        // unlinked between two buffers.
        match &self.input {
            Input::Packets(tap) => {
                if let Some(source) = tap.lock().into.take() {
                    let _ = source.end_of_stream();
                }
            }
            Input::Encoded { pad, .. } => {
                let queue_sink = self.elements[0].static_pad("sink");
                pad.add_probe(gst::PadProbeType::IDLE, move |pad, _| {
                    if let Some(queue_sink) = &queue_sink {
                        drop(pad.unlink(queue_sink));
                        queue_sink.send_event(gst::event::Eos::new());
                    }
                    gst::PadProbeReturn::Remove
                });
            }
        }

        if written.recv_timeout(FINISH_TIMEOUT).is_err() {
            log::warn!(
                "Video: {} may not be finished properly",
                self.path.display()
            );
        }
        if let Input::Encoded { tee, pad } = &self.input {
            tee.release_request_pad(pad);
        }
        remove(pipeline, &self.elements);

        if !self.started.load(Ordering::Relaxed) {
            drop(std::fs::remove_file(&self.path));
        }
    }
}

/// Link a new pad of `tee` to `first`'s sink; the pad.
fn link_to_tee(tee: &gst::Element, first: &gst::Element) -> Result<gst::Pad, String> {
    let pad = tee
        .request_pad_simple("src_%u")
        .ok_or_else(|| "no pad on the stream's tee".to_string())?;
    let linked = first
        .static_pad("sink")
        .ok_or_else(|| "the queue has no sink".to_string())
        .and_then(|sink| pad.link(&sink).map(drop).map_err(|error| error.to_string()));
    if let Err(error) = linked {
        tee.release_request_pad(&pad);
        return Err(error);
    }
    Ok(pad)
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
