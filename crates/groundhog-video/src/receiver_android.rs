//! The receiver on Android: not there yet. Android will receive video
//! through MediaCodec or the GStreamer Android SDK (see `docs/video.md`);
//! until then a receiver says so, with the same interface as on the
//! desktop, so a frontend needs no platform code of its own.

use tokio::sync::watch;

use crate::{Config, FrameSlot, State, Status, source::Source};

/// Receives one stream into a [`FrameSlot`] until dropped.
pub struct Receiver {
    slot: FrameSlot,
    state: watch::Receiver<State>,
}

impl Receiver {
    /// A receiver that reports why it cannot receive `config`'s stream.
    pub fn start(config: Config) -> Self {
        let reason = match Source::parse(&config.uri) {
            Ok(_) => "Video is not supported on Android yet".to_string(),
            Err(reason) => reason,
        };
        let (_, state) = watch::channel(State {
            status: Status::Failed(reason),
            frames: 0,
        });

        Self {
            slot: FrameSlot::new(),
            state,
        }
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
