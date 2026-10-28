//! The receiver on the desktop: a GStreamer pipeline per attempt, run and
//! restarted by a supervisor thread of its own.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_video::{self as gst_video, prelude::*};
use tokio::sync::watch;

use crate::{
    Colour, Config, Frame, FrameSlot, Matrix, PixelLayout, Plane, Range, Recording, State, Status,
    record::Branch,
    source::{self, Source},
};

/// How long a stream may go without a frame, once frames came, before it
/// is opened again.
const STALL: Duration = Duration::from_secs(3);
/// How long a stream may take to show its first frame.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// The wait before the stream is opened again. Short and fixed: while no
/// pipeline runs, a UDP stream's packets are lost, so a longer wait only
/// delays the picture, and one attempt a second costs a server nothing.
const RETRY: Duration = Duration::from_secs(1);
/// How often the supervisor looks at the bus, the clock and the stop flag.
const POLL: Duration = Duration::from_millis(100);

/// Receives one stream into a [`FrameSlot`] until dropped.
pub struct Receiver {
    slot: FrameSlot,
    state: watch::Receiver<State>,
    stop: Arc<AtomicBool>,
    record: Arc<Mutex<Option<PathBuf>>>,
}

impl Receiver {
    /// Start receiving as `config` asks. Failures are reported through
    /// [`Receiver::state`], never returned: a stream that cannot be opened
    /// now may open later.
    pub fn start(config: Config) -> Self {
        let slot = FrameSlot::new();
        let (state, watcher) = watch::channel(State {
            status: Status::Connecting,
            frames: 0,
            recording: Recording::Off,
        });
        let stop = Arc::new(AtomicBool::new(false));
        let record = Arc::new(Mutex::new(None));

        let supervisor = Supervisor {
            config,
            slot: slot.clone(),
            state,
            stop: stop.clone(),
            record: record.clone(),
        };
        let spawned = thread::Builder::new()
            .name("video".to_string())
            .spawn(move || supervisor.run());
        if let Err(error) = spawned {
            log::error!("Could not start the video receiver: {error}");
        }

        Self {
            slot,
            state: watcher,
            stop,
            record,
        }
    }

    /// Record the stream into `directory`, or stop recording (`None`). A
    /// file is written per stretch of video: from a keyframe, until the
    /// stream is lost or recording stops. Files are named after the time
    /// they start (see [`Recording`] for how it stands).
    pub fn record(&self, directory: Option<PathBuf>) {
        *self.record.lock().unwrap_or_else(PoisonError::into_inner) = directory;
    }

    /// Where the decoded frames arrive.
    pub fn slot(&self) -> &FrameSlot {
        &self.slot
    }

    /// The receiver's state, changing with every frame.
    pub fn state(&self) -> watch::Receiver<State> {
        self.state.clone()
    }
}

impl Drop for Receiver {
    /// The supervisor stops the pipeline on its own thread: closing an RTSP
    /// session can take a moment, and the caller should not wait for it.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct Supervisor {
    config: Config,
    slot: FrameSlot,
    state: watch::Sender<State>,
    stop: Arc<AtomicBool>,
    /// Where to record, if anywhere.
    record: Arc<Mutex<Option<PathBuf>>>,
}

/// One attempt's recording.
#[derive(Default)]
struct Recorder {
    branch: Option<Branch>,
    /// The stream could not be recorded; not tried again until the next
    /// attempt, or until recording is asked for anew.
    refused: bool,
}

/// What ended an attempt.
enum Ended {
    Stopped,
    /// Worth trying again, after [`RETRY`].
    Retry(String),
    /// Not worth trying again.
    Failed(String),
}

impl Supervisor {
    fn run(self) {
        let source = match Source::parse(&self.config.uri) {
            Ok(source) => source,
            Err(reason) => return self.set_status(Status::Failed(reason)),
        };
        if let Err(error) = initialise() {
            return self.set_status(Status::Failed(error));
        }

        loop {
            self.set_status(Status::Connecting);
            match self.attempt(&source) {
                Ended::Stopped => break,
                Ended::Failed(reason) => {
                    self.set_status(Status::Failed(reason));
                    break;
                }
                Ended::Retry(reason) => {
                    log::info!("Video: {reason}; retrying in {} s", RETRY.as_secs());
                    self.set_status(Status::Retrying { reason });
                    if self.wait(RETRY) {
                        break;
                    }
                }
            }
        }
        self.slot.clear();
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Wait `duration`, or less if stopped; whether stopped.
    fn wait(&self, duration: Duration) -> bool {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            if self.stopped() {
                return true;
            }
            thread::sleep(POLL);
        }
        self.stopped()
    }

    fn set_status(&self, status: Status) {
        self.state.send_if_modified(|state| {
            let changed = state.status != status;
            state.status = status;
            changed
        });
    }

    fn set_recording(&self, recording: Recording) {
        set_recording(&self.state, recording);
    }

    /// Where to record, if anywhere.
    fn record_into(&self) -> Option<PathBuf> {
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Bring the recording in line with what is asked: start a branch where
    /// a stream has come, finish one no longer wanted.
    fn sync_recording(&self, pipeline: &gst::Pipeline, recorder: &mut Recorder) {
        let wanted = self.record_into();
        if let Some(branch) = recorder
            .branch
            .take_if(|branch| wanted.as_deref() != Some(branch.directory()))
        {
            branch.finish(pipeline);
        }

        let Some(directory) = wanted else {
            recorder.refused = false;
            self.set_recording(Recording::Off);
            return;
        };
        if recorder.branch.is_some() || recorder.refused {
            return;
        }

        let state = self.state.clone();
        let started = move |path: &std::path::Path| {
            log::info!("Video: recording into {}", path.display());
            set_recording(&state, Recording::Writing(path.to_path_buf()));
        };
        match Branch::attach(pipeline, &directory, started) {
            Ok(Some(branch)) => {
                recorder.branch = Some(branch);
                self.set_recording(Recording::Waiting);
            }
            Ok(None) => self.set_recording(Recording::Waiting),
            Err(reason) => {
                log::warn!("Video: {reason}");
                recorder.refused = true;
                self.set_recording(Recording::Failed(reason));
            }
        }
    }

    /// One run of the pipeline, from opening the stream until it fails,
    /// stalls or is stopped.
    fn attempt(&self, source: &Source) -> Ended {
        let description = source::pipeline_description(source, &self.config);
        log::debug!("Video pipeline: {description}");

        let pipeline = match gst::parse::launch(&description)
            .map_err(|error| error.to_string())
            .and_then(|element| {
                element
                    .downcast::<gst::Pipeline>()
                    .map_err(|_| "not a pipeline".to_string())
            }) {
            Ok(pipeline) => pipeline,
            // A missing element is a missing plugin: retrying cannot help.
            Err(error) => {
                return Ended::Failed(format!("Cannot build the video pipeline: {error}"));
            }
        };

        let running = Running::default();
        if let Err(reason) = self.wire(&pipeline, &running) {
            return Ended::Failed(reason);
        }

        let mut recorder = Recorder::default();
        let ended = if pipeline.set_state(gst::State::Playing).is_err() {
            Ended::Retry("The stream could not be opened".to_string())
        } else {
            self.watch(&pipeline, &running, &mut recorder)
        };

        // The file is finished before the pipeline stops, so it keeps its
        // index. Until the next stretch of video, a recording waits; one
        // that failed says so until it is tried again.
        if let Some(branch) = recorder.branch.take() {
            branch.finish(&pipeline);
        }
        self.state.send_if_modified(|state| {
            let waiting = match state.recording {
                Recording::Writing(_) => Recording::Waiting,
                _ => return false,
            };
            state.recording = waiting;
            true
        });

        let _ = pipeline.set_state(gst::State::Null);
        ended
    }

    /// Connect the pipeline's elements to the slot and the state.
    fn wire(&self, pipeline: &gst::Pipeline, running: &Running) -> Result<(), String> {
        let sink = pipeline
            .by_name(source::SINK)
            .and_then(|sink| sink.downcast::<gst_app::AppSink>().ok())
            .ok_or_else(|| "The video pipeline has no sink".to_string())?;

        // RTSP: set up only the video streams, so audio costs nothing and
        // leaves no pad unlinked.
        if let Some(rtsp) = pipeline.by_name(source::RTSP_SOURCE) {
            rtsp.connect("select-stream", false, |values| {
                let video = values
                    .get(2)
                    .and_then(|caps| caps.get::<gst::Caps>().ok())
                    .and_then(|caps| {
                        caps.structure(0)
                            .and_then(|structure| structure.get::<&str>("media").ok())
                            .map(|media| media == "video")
                    })
                    .unwrap_or(false);
                Some(video.to_value())
            });
        }

        // Where a parsebin finds the stream, its video goes on to the tee;
        // anything else (an MPEG-TS's audio) is left unlinked, which the
        // demuxer tolerates while one stream is linked.
        if let Some(parse) = pipeline.by_name(source::PARSE) {
            let tee = pipeline
                .by_name(source::ENCODED)
                .and_then(|tee| tee.static_pad("sink"))
                .ok_or_else(|| "The video pipeline has no tee".to_string())?;
            parse.connect_pad_added(move |_, pad| {
                let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
                let video = caps
                    .structure(0)
                    .is_some_and(|structure| structure.name().starts_with("video/"));
                if video
                    && !tee.is_linked()
                    && let Err(error) = pad.link(&tee)
                {
                    log::warn!("Video: cannot link the stream ({error:?})");
                }
            });
        }

        // On a live stream, a decoder with QoS on drops every frame after
        // the first late one; keep the name of the decoder chosen.
        let decoder = running.decoder.clone();
        pipeline.connect_deep_element_added(move |_, _, element| {
            let Some(factory) = element.factory() else {
                return;
            };
            if !factory.has_type(gst::ElementFactoryType::DECODER) {
                return;
            }
            if element.has_property("qos") {
                element.set_property("qos", false);
            }
            *decoder.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(factory.name().to_string());
        });

        let slot = self.slot.clone();
        let state = self.state.clone();
        let decoder = running.decoder.clone();
        let last_frame = running.last_frame.clone();
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let Some(frame) = frame_of(&sample) else {
                        return Ok(gst::FlowSuccess::Ok);
                    };

                    *last_frame.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(Instant::now());
                    let streaming = Status::Streaming {
                        width: frame.width,
                        height: frame.height,
                        decoder: decoder
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone(),
                    };
                    slot.publish(frame);
                    state.send_modify(|state| {
                        state.frames += 1;
                        if state.status != streaming {
                            state.status = streaming;
                        }
                    });
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        Ok(())
    }

    /// Run the pipeline until it fails, stalls or is stopped, recording
    /// into `recorder` as asked.
    fn watch(&self, pipeline: &gst::Pipeline, running: &Running, recorder: &mut Recorder) -> Ended {
        let Some(bus) = pipeline.bus() else {
            return Ended::Failed("The video pipeline has no bus".to_string());
        };
        let opened = Instant::now();

        loop {
            if self.stopped() {
                return Ended::Stopped;
            }

            let last_frame = *running
                .last_frame
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match last_frame {
                Some(at) if at.elapsed() > STALL => {
                    return Ended::Retry("The video stopped".to_string());
                }
                None if opened.elapsed() > OPEN_TIMEOUT => {
                    return Ended::Retry("No video arrives".to_string());
                }
                _ => {}
            }

            self.sync_recording(pipeline, recorder);

            let Some(message) =
                bus.timed_pop(gst::ClockTime::from_nseconds(POLL.as_nanos() as u64))
            else {
                continue;
            };
            match message.view() {
                gst::MessageView::Error(error) => {
                    let detail = error.debug().map(|debug| debug.to_string());
                    log::warn!("Video: {} ({detail:?})", error.error());

                    // A recording that fails (a full disk) ends; the
                    // stream does not.
                    let source = message.src();
                    if let Some(branch) = recorder
                        .branch
                        .take_if(|branch| source.is_some_and(|source| branch.owns(source)))
                    {
                        branch.finish(pipeline);
                        recorder.refused = true;
                        self.set_recording(Recording::Failed(format!(
                            "Recording stopped: {}",
                            error.error()
                        )));
                        continue;
                    }
                    return Ended::Retry(error.error().to_string());
                }
                gst::MessageView::Eos(_) => {
                    return Ended::Retry("The stream ended".to_string());
                }
                gst::MessageView::Latency(_) => {
                    drop(pipeline.recalculate_latency());
                }
                _ => {}
            }
        }
    }
}

/// Set the recording's state, waking the viewer only on a change.
fn set_recording(state: &watch::Sender<State>, recording: Recording) {
    state.send_if_modified(|state| {
        let changed = state.recording != recording;
        state.recording = recording;
        changed
    });
}

/// What one attempt's callbacks share with its supervisor.
#[derive(Default)]
struct Running {
    decoder: Arc<Mutex<Option<String>>>,
    last_frame: Arc<Mutex<Option<Instant>>>,
}

/// Initialise GStreamer once per process; the error, if it cannot be.
fn initialise() -> Result<(), String> {
    static INITIALISED: OnceLock<Result<(), String>> = OnceLock::new();
    INITIALISED
        .get_or_init(|| gst::init().map_err(|error| format!("GStreamer is not available: {error}")))
        .clone()
}

/// The frame in `sample`, copied out of GStreamer's buffer, if it is one
/// the viewer can draw.
fn frame_of(sample: &gst::Sample) -> Option<Frame> {
    let info = gst_video::VideoInfo::from_caps(sample.caps()?).ok()?;
    let layout = match info.format() {
        gst_video::VideoFormat::Nv12 => PixelLayout::Nv12,
        gst_video::VideoFormat::I420 => PixelLayout::I420,
        _ => return None,
    };
    let video = gst_video::VideoFrameRef::from_buffer_ref_readable(sample.buffer()?, &info).ok()?;

    let height = info.height();
    let chroma_rows = height.div_ceil(2);
    let planes = (0..video.n_planes())
        .map(|index| {
            let rows = if index == 0 { height } else { chroma_rows };
            let stride = video.plane_stride()[index as usize] as u32;
            let data = video.plane_data(index).ok()?;
            let length = (stride as usize * rows as usize).min(data.len());
            Some(Plane {
                data: data[..length].to_vec(),
                stride,
                rows,
            })
        })
        .collect::<Option<Vec<_>>>()?;

    let colorimetry = info.colorimetry();
    let colour = Colour {
        matrix: match colorimetry.matrix() {
            gst_video::VideoColorMatrix::Bt601 => Matrix::Bt601,
            _ => Matrix::Bt709,
        },
        range: match colorimetry.range() {
            gst_video::VideoColorRange::Range0_255 => Range::Full,
            _ => Range::Limited,
        },
    };

    Some(Frame {
        width: info.width(),
        height,
        layout,
        planes,
        colour,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait up to `timeout` for `condition` on the receiver's state.
    fn until(receiver: &Receiver, timeout: Duration, condition: impl Fn(&State) -> bool) -> State {
        let state = receiver.state();
        let until = Instant::now() + timeout;
        loop {
            let now = state.borrow().clone();
            if condition(&now) || Instant::now() > until {
                return now;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn gstreamer_has(elements: &[&str]) -> bool {
        initialise().is_ok()
            && elements
                .iter()
                .all(|element| gst::ElementFactory::find(element).is_some())
    }

    #[test]
    fn an_unknown_uri_fails_without_retrying() {
        let receiver = Receiver::start(Config::new("http://camera"));
        let state = until(&receiver, Duration::from_secs(2), |state| {
            matches!(state.status, Status::Failed(_))
        });
        assert!(matches!(state.status, Status::Failed(_)), "{state:?}");
    }

    /// A source that refuses is tried again every [`RETRY`], however often
    /// it has refused: the wait does not grow while the camera is away.
    #[test]
    fn a_refusing_source_is_retried_at_a_steady_pace() {
        if !gstreamer_has(&["tcpclientsrc", "decodebin3", "appsink"]) {
            eprintln!("skipped: GStreamer's base plugins are not installed");
            return;
        }

        // A port nobody listens on: every open is refused at once.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        let receiver = Receiver::start(Config::new(format!("tcp://127.0.0.1:{port}")));

        // Count the times the receiver is seen waiting to retry. A change
        // seen while waiting is a new wait, even if the attempt between the
        // two came and went unseen.
        let mut state = receiver.state();
        let mut retries = 0;
        let until = Instant::now() + Duration::from_millis(5500);
        while Instant::now() < until {
            if state.has_changed().unwrap_or(false)
                && matches!(state.borrow_and_update().status, Status::Retrying { .. })
            {
                retries += 1;
            }
            thread::sleep(Duration::from_millis(20));
        }

        // A doubling wait (1, 2, 4 s) would have retried three times by now.
        assert!(retries >= 5, "{retries} retries in 5.5 s");
    }

    #[test]
    fn the_test_pattern_streams_frames() {
        if !gstreamer_has(&["videotestsrc", "videoconvert", "appsink"]) {
            eprintln!("skipped: GStreamer's base plugins are not installed");
            return;
        }

        let receiver = Receiver::start(Config::new("test://"));
        let state = until(&receiver, Duration::from_secs(5), |state| state.frames >= 3);

        assert!(state.frames >= 3, "{state:?}");
        assert!(
            matches!(
                state.status,
                Status::Streaming {
                    width: 1280,
                    height: 720,
                    ..
                }
            ),
            "{state:?}"
        );
        let (_, frame) = receiver.slot().latest().expect("a frame in the slot");
        assert_eq!(frame.planes.len(), frame_planes(frame.layout));
        assert!(frame.planes[0].stride >= 1280);
        assert_eq!(frame.planes[0].rows, 720);
    }

    fn frame_planes(layout: PixelLayout) -> usize {
        match layout {
            PixelLayout::Nv12 => 2,
            PixelLayout::I420 => 3,
        }
    }

    /// The low-latency path end to end: an H.264 stream over RTP/UDP on
    /// loopback, as a companion computer would send it.
    #[test]
    fn h264_over_udp_is_received() {
        let encoder = ["x264enc", "openh264enc"]
            .into_iter()
            .find(|encoder| gstreamer_has(&[encoder]));
        let Some(encoder) = encoder.filter(|_| {
            gstreamer_has(&[
                "rtph264pay",
                "rtph264depay",
                "h264parse",
                "udpsink",
                "decodebin3",
            ])
        }) else {
            eprintln!("skipped: no H.264 encoder, or GStreamer's RTP plugins are not installed");
            return;
        };

        // Any free port: bind one, then let it go for the receiver.
        let port = std::net::UdpSocket::bind("127.0.0.1:0")
            .and_then(|socket| socket.local_addr())
            .expect("a free port")
            .port();
        let receiver = Receiver::start(Config::new(format!("udp://127.0.0.1:{port}")));

        let tune = if encoder == "x264enc" {
            " tune=zerolatency"
        } else {
            ""
        };
        let sender = gst::parse::launch(&format!(
            "videotestsrc is-live=true ! video/x-raw,width=320,height=240,framerate=30/1 \
             ! {encoder}{tune} ! rtph264pay config-interval=1 ! udpsink host=127.0.0.1 port={port}"
        ))
        .expect("the sender pipeline");
        sender
            .set_state(gst::State::Playing)
            .expect("the sender plays");

        let state = until(&receiver, Duration::from_secs(10), |state| {
            state.frames >= 5
        });
        let _ = sender.set_state(gst::State::Null);

        assert!(state.frames >= 5, "{state:?}");
        assert!(
            matches!(
                state.status,
                Status::Streaming {
                    width: 320,
                    height: 240,
                    ..
                }
            ),
            "{state:?}"
        );
    }

    /// An H.264 encoder GStreamer has, if any, with what keeps it from
    /// buffering.
    fn h264_encoder() -> Option<&'static str> {
        if gstreamer_has(&["x264enc"]) {
            Some("x264enc tune=zerolatency key-int-max=15")
        } else if gstreamer_has(&["openh264enc"]) {
            Some("openh264enc gop-size=15")
        } else {
            None
        }
    }

    /// A free UDP port on loopback.
    fn free_port() -> u16 {
        std::net::UdpSocket::bind("127.0.0.1:0")
            .and_then(|socket| socket.local_addr())
            .expect("a free port")
            .port()
    }

    /// A playing sender of colour bars, encoded in H.264, then `tail`.
    fn sender(encoder: &str, tail: &str) -> gst::Element {
        let sender = gst::parse::launch(&format!(
            "videotestsrc is-live=true ! video/x-raw,width=320,height=240,framerate=30/1 \
             ! {encoder} ! {tail}"
        ))
        .expect("the sender pipeline");
        sender
            .set_state(gst::State::Playing)
            .expect("the sender plays");
        sender
    }

    /// A directory of its own for a test's recordings, empty.
    fn recordings(test: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("groundhog-video-{test}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&directory));
        directory
    }

    /// What a recorded file holds: its first frame's flags and timestamp,
    /// how many frames, and the date it says it starts at.
    struct Recorded {
        first_is_keyframe: bool,
        first_pts: Option<gst::ClockTime>,
        frames: usize,
        date: Option<gst::DateTime>,
    }

    fn read_recording(path: &std::path::Path) -> Recorded {
        let reader = gst::parse::launch(&format!(
            "filesrc location=\"{}\" ! matroskademux ! appsink name=out sync=false",
            path.display()
        ))
        .expect("the reader pipeline")
        .downcast::<gst::Pipeline>()
        .expect("a pipeline");
        let out = reader
            .by_name("out")
            .and_then(|out| out.downcast::<gst_app::AppSink>().ok())
            .expect("the reader's sink");
        reader
            .set_state(gst::State::Playing)
            .expect("the reader plays");

        let mut recorded = Recorded {
            first_is_keyframe: false,
            first_pts: None,
            frames: 0,
            date: None,
        };
        while let Some(sample) = out.try_pull_sample(gst::ClockTime::from_seconds(2)) {
            let buffer = sample.buffer().expect("a buffer");
            if recorded.frames == 0 {
                recorded.first_is_keyframe = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);
                recorded.first_pts = buffer.pts();
            }
            recorded.frames += 1;
        }
        let bus = reader.bus().expect("a bus");
        while let Some(message) = bus.pop() {
            if let gst::MessageView::Tag(tag) = message.view()
                && let Some(date) = tag.tags().get::<gst::tags::DateTime>()
            {
                recorded.date = Some(date.get().clone());
            }
        }
        let _ = reader.set_state(gst::State::Null);
        recorded
    }

    /// Record `uri` (fed by a running sender) for a while, stop, and
    /// return the one file written, checked to start on a keyframe at zero
    /// and to say when it began.
    fn record_a_while(uri: &str, test: &str) -> Recorded {
        let directory = recordings(test);
        let receiver = Receiver::start(Config::new(uri));
        receiver.record(Some(directory.clone()));

        let state = until(&receiver, Duration::from_secs(10), |state| {
            matches!(state.recording, Recording::Writing(_))
        });
        let Recording::Writing(path) = state.recording.clone() else {
            panic!("not recording: {state:?}");
        };
        assert_eq!(path.parent(), Some(directory.as_path()));
        // A second of video into the file, counted from when it opened.
        let opened = state.frames;
        until(&receiver, Duration::from_secs(5), |state| {
            state.frames >= opened + 30
        });

        let before_stop = std::time::SystemTime::now();
        receiver.record(None);
        let state = until(&receiver, Duration::from_secs(5), |state| {
            state.recording == Recording::Off
        });
        assert_eq!(state.recording, Recording::Off, "{state:?}");

        let recorded = read_recording(&path);
        assert!(recorded.first_is_keyframe, "the file starts on a keyframe");
        assert!(
            recorded
                .first_pts
                .is_some_and(|pts| pts < gst::ClockTime::from_mseconds(100)),
            "the file starts at zero: {:?}",
            recorded.first_pts
        );
        assert!(recorded.frames >= 10, "{} frames", recorded.frames);

        // Its date is when its first frame arrived: within the recording.
        let date = recorded.date.as_ref().expect("the file has a date");
        let started = date.to_g_date_time().expect("a full date").to_unix();
        let stopped = before_stop
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs() as i64;
        assert!(
            (stopped - 20..=stopped).contains(&started),
            "started {started}, stopped {stopped}"
        );

        drop(receiver);
        drop(std::fs::remove_dir_all(&directory));
        recorded
    }

    /// An RTP stream is recorded as it comes, without encoding it again.
    #[test]
    fn h264_over_udp_is_recorded() {
        let Some(encoder) = h264_encoder().filter(|_| {
            gstreamer_has(&[
                "rtph264pay",
                "rtph264depay",
                "h264parse",
                "matroskamux",
                "matroskademux",
            ])
        }) else {
            eprintln!(
                "skipped: no H.264 encoder, or GStreamer's RTP or Matroska plugins are missing"
            );
            return;
        };
        let port = free_port();
        let sender = sender(
            encoder,
            &format!("rtph264pay config-interval=1 ! udpsink host=127.0.0.1 port={port}"),
        );

        record_a_while(&format!("udp://127.0.0.1:{port}"), "rtp");

        let _ = sender.set_state(gst::State::Null);
    }

    /// An MPEG-TS stream, found by the parsebin, is shown and recorded.
    #[test]
    fn mpegts_over_udp_is_recorded() {
        let Some(encoder) = h264_encoder().filter(|_| {
            gstreamer_has(&[
                "mpegtsmux",
                "tsdemux",
                "h264parse",
                "matroskamux",
                "matroskademux",
            ])
        }) else {
            eprintln!(
                "skipped: no H.264 encoder, or GStreamer's MPEG-TS or Matroska plugins are missing"
            );
            return;
        };
        let port = free_port();
        let sender = sender(
            encoder,
            &format!("h264parse ! mpegtsmux ! udpsink host=127.0.0.1 port={port}"),
        );

        record_a_while(&format!("mpegts://127.0.0.1:{port}"), "mpegts");

        let _ = sender.set_state(gst::State::Null);
    }

    /// A stream that is lost ends its file; when video comes back, the next
    /// stretch goes into a file of its own, and both play.
    #[test]
    fn each_stretch_of_video_is_a_file() {
        let Some(encoder) = h264_encoder().filter(|_| {
            gstreamer_has(&[
                "rtph264pay",
                "rtph264depay",
                "h264parse",
                "matroskamux",
                "matroskademux",
            ])
        }) else {
            eprintln!(
                "skipped: no H.264 encoder, or GStreamer's RTP or Matroska plugins are missing"
            );
            return;
        };
        let directory = recordings("stretches");
        let port = free_port();
        let tail = format!("rtph264pay config-interval=1 ! udpsink host=127.0.0.1 port={port}");
        let receiver = Receiver::start(Config::new(format!("udp://127.0.0.1:{port}")));
        receiver.record(Some(directory.clone()));

        let mut files = Vec::new();
        for _ in 0..2 {
            let sending = sender(encoder, &tail);
            let state = until(
                &receiver,
                Duration::from_secs(10),
                |state| matches!(&state.recording, Recording::Writing(path) if !files.contains(path)),
            );
            let Recording::Writing(path) = state.recording else {
                panic!("not recording a new file: {state:?}");
            };
            files.push(path);
            thread::sleep(Duration::from_millis(500));

            // The camera goes away: the receiver notices the stall.
            let _ = sending.set_state(gst::State::Null);
            let state = until(&receiver, STALL * 2, |state| {
                state.recording == Recording::Waiting
            });
            assert_eq!(state.recording, Recording::Waiting, "{state:?}");
        }

        assert_ne!(files[0], files[1]);
        for file in &files {
            let recorded = read_recording(file);
            assert!(recorded.frames > 0, "{} is empty", file.display());
        }
        drop(receiver);
        drop(std::fs::remove_dir_all(&directory));
    }

    /// Asked to record before any video, then stopped: no file is left.
    #[test]
    fn a_recording_without_video_leaves_nothing() {
        if !gstreamer_has(&["udpsrc", "rtph264depay", "h264parse"]) {
            eprintln!("skipped: GStreamer's RTP plugins are missing");
            return;
        }
        let directory = recordings("silent");
        let receiver = Receiver::start(Config::new(format!("udp://127.0.0.1:{}", free_port())));
        receiver.record(Some(directory.clone()));

        let state = until(&receiver, Duration::from_secs(2), |state| {
            state.recording == Recording::Waiting
        });
        assert_eq!(state.recording, Recording::Waiting, "{state:?}");
        receiver.record(None);
        let state = until(&receiver, Duration::from_secs(2), |state| {
            state.recording == Recording::Off
        });
        assert_eq!(state.recording, Recording::Off, "{state:?}");

        let files = std::fs::read_dir(&directory)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(files, 0, "nothing in {}", directory.display());
        drop(std::fs::remove_dir_all(&directory));
    }

    /// The test pattern is no stream: it says it cannot be recorded.
    #[test]
    fn the_test_pattern_is_not_recorded() {
        if !gstreamer_has(&["videotestsrc", "videoconvert", "appsink"]) {
            eprintln!("skipped: GStreamer's base plugins are not installed");
            return;
        }
        let receiver = Receiver::start(Config::new("test://"));
        receiver.record(Some(recordings("pattern")));

        let state = until(&receiver, Duration::from_secs(5), |state| {
            matches!(state.recording, Recording::Failed(_))
        });
        assert!(matches!(state.recording, Recording::Failed(_)), "{state:?}");
    }
}
