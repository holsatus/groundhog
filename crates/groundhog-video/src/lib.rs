//! Video from the vehicle: a stream's URI in, decoded frames out, with as
//! little latency as the stream allows.
//!
//! A [`Receiver`] opens a stream (see [`Source`] for the URIs it takes),
//! decodes it on its own threads, and puts every decoded frame in a
//! [`FrameSlot`], replacing the frame before it if no one has taken that
//! one yet: a slow viewer drops frames and never queues them. It reports
//! what it is doing through a [`State`] that changes with every frame, so
//! a frontend can wake when a frame arrives instead of polling.
//!
//! On the desktop the receiver is a GStreamer pipeline, configured as
//! QGroundControl configures its own (see `docs/video.md`): no jitter
//! buffer in low-latency mode, a decoder chosen by GStreamer (hardware
//! where one is available) with QoS off, and a sink that neither waits for
//! the clock nor holds more than one frame. Frames keep the decoder's YUV
//! layout ([`PixelLayout`]), for the frontend to convert on the GPU.
//!
//! The receiver reconnects by itself, a second after the stream fails or
//! stalls.
//!
//! It also records the stream, as it arrives, into Matroska files
//! ([`Receiver::record`]): the compressed stream is written as it is, never
//! decoded and encoded again, a file per stretch of video between
//! reconnections. Each file starts at a keyframe, at time zero, and carries
//! the wall-clock time that keyframe arrived (Matroska's `DateUTC`), so a
//! frame's arrival is the file's date plus its timestamp: the same clock a
//! telemetry log stamps its frames with.

// Public items in private modules are invisible to the dead-code and
// visibility checks the seam relies on; this makes them visible.
#![warn(unreachable_pub)]

mod frame;
#[cfg(not(target_os = "android"))]
mod record;
mod source;

#[cfg(not(target_os = "android"))]
mod receiver;
#[cfg(target_os = "android")]
#[path = "receiver_android.rs"]
mod receiver;

pub use frame::{Colour, Frame, FrameSlot, Matrix, PixelLayout, Plane, Range};
pub use receiver::Receiver;
pub use source::{Codec, Source};

use std::path::PathBuf;

/// How a stream is received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Where the stream comes from; see [`Source::parse`].
    pub uri: String,
    /// Show every frame as soon as it is decoded: no jitter buffer (so no
    /// reordering or retransmission of lost packets), and no waiting for a
    /// frame's presentation time. Lowest latency; on a lossy link, more
    /// frames show damaged.
    pub low_latency: bool,
    /// Outside low-latency mode, how long packets are held to put them in
    /// order and ask for lost ones again. Retransmission needs at least
    /// [`MIN_RETRANSMISSION_LATENCY_MS`].
    pub jitter_buffer_ms: u32,
}

/// QGroundControl's default jitter buffer: room for one retransmission of
/// a lost packet before its frame is due.
pub const DEFAULT_JITTER_BUFFER_MS: u32 = 80;

/// The least jitter buffer in which asking for a lost packet again can
/// still get it in time.
pub const MIN_RETRANSMISSION_LATENCY_MS: u32 = 40;

impl Config {
    /// `uri`, at the lowest latency.
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            low_latency: true,
            jitter_buffer_ms: DEFAULT_JITTER_BUFFER_MS,
        }
    }
}

/// What the receiver is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Opening the stream, or opening it again.
    Connecting,
    /// Frames arrive.
    Streaming {
        width: u32,
        height: u32,
        /// The decoder GStreamer chose, if it said.
        decoder: Option<String>,
    },
    /// The stream failed, ended or stalled; it is opened again in a
    /// second.
    Retrying { reason: String },
    /// The stream cannot be received (an unknown URI, a missing GStreamer
    /// plugin): trying again would not help.
    Failed(String),
}

/// The receiver's state: its status, and how many frames it has decoded.
/// The count changes with every frame, so watching the state wakes a
/// viewer for each one.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub status: Status,
    pub frames: u64,
    pub recording: Recording,
}

/// Whether the stream is being recorded.
#[derive(Debug, Clone, PartialEq)]
pub enum Recording {
    Off,
    /// Asked to record: a file opens at the next keyframe, once there is
    /// video.
    Waiting,
    /// Writing into this file.
    Writing(PathBuf),
    /// The stream cannot be recorded, and why. It is tried again once the
    /// stream is opened again.
    Failed(String),
}
