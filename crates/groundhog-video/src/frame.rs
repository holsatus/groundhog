//! Decoded frames, and the slot that hands the newest to a viewer.

use std::{
    fmt,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

/// How a frame's YUV samples are laid out in its planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelLayout {
    /// A full-size luma plane, then one half-size plane of interleaved
    /// chroma (U, V): what hardware decoders produce.
    Nv12,
    /// A full-size luma plane, then half-size U and V planes: what software
    /// decoders often produce.
    I420,
}

/// The YUV-to-RGB matrix a frame was encoded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    /// Standard definition.
    Bt601,
    /// High definition, and what a stream that does not say is taken to be.
    Bt709,
}

/// The range of a frame's sample values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// Luma 16–235 and chroma 16–240, as video usually is.
    Limited,
    /// 0–255.
    Full,
}

/// How a frame's samples become colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub matrix: Matrix,
    pub range: Range,
}

/// One plane of samples: `rows` rows of `stride` bytes, of which the
/// plane's width in samples are used.
#[derive(Clone, PartialEq, Eq)]
pub struct Plane {
    pub data: Vec<u8>,
    pub stride: u32,
    pub rows: u32,
}

impl fmt::Debug for Plane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Plane({} rows of {} bytes)", self.rows, self.stride)
    }
}

/// A decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub layout: PixelLayout,
    /// Luma first, then chroma: two planes for NV12, three for I420.
    pub planes: Vec<Plane>,
    pub colour: Colour,
}

impl Frame {
    /// The width and height of the chroma planes: half the frame's,
    /// rounded up.
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }
}

/// The newest decoded frame of a stream, shared between the receiver,
/// which replaces it, and a viewer, which shows it.
///
/// A frame no one has taken is replaced by the next one, so a viewer that
/// falls behind skips frames rather than showing old ones. Each frame has
/// a generation, so a viewer can tell a frame it has already shown.
#[derive(Clone)]
pub struct FrameSlot {
    inner: Arc<Inner>,
}

struct Inner {
    id: u64,
    latest: Mutex<Option<(u64, Arc<Frame>)>>,
    generation: AtomicU64,
}

impl FrameSlot {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        Self {
            inner: Arc::new(Inner {
                id: NEXT.fetch_add(1, Ordering::Relaxed),
                latest: Mutex::new(None),
                generation: AtomicU64::new(0),
            }),
        }
    }

    /// An identity unique to this slot (and its clones), for a viewer that
    /// keeps resources per stream.
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// Make `frame` the newest, replacing the one before it.
    pub fn publish(&self, frame: Frame) {
        let generation = self.inner.generation.fetch_add(1, Ordering::Relaxed) + 1;
        *self
            .inner
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((generation, Arc::new(frame)));
    }

    /// The newest frame and its generation, if any has arrived.
    pub fn latest(&self) -> Option<(u64, Arc<Frame>)> {
        self.inner
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Forget the frame shown, as when the stream stops.
    pub fn clear(&self) {
        *self
            .inner
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl Default for FrameSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for FrameSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FrameSlot({})", self.inner.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32) -> Frame {
        Frame {
            width,
            height: 2,
            layout: PixelLayout::Nv12,
            planes: Vec::new(),
            colour: Colour {
                matrix: Matrix::Bt709,
                range: Range::Limited,
            },
        }
    }

    #[test]
    fn the_newest_frame_replaces_one_not_taken() {
        let slot = FrameSlot::new();
        assert!(slot.latest().is_none());

        slot.publish(frame(1));
        slot.publish(frame(2));

        let (generation, latest) = slot.latest().expect("a frame");
        assert_eq!(latest.width, 2);
        assert_eq!(generation, 2, "the skipped frame still counts");

        slot.clear();
        assert!(slot.latest().is_none());
    }

    #[test]
    fn clones_share_a_slot_and_its_identity() {
        let slot = FrameSlot::new();
        let viewer = slot.clone();
        assert_eq!(slot.id(), viewer.id());
        assert_ne!(slot.id(), FrameSlot::new().id());

        slot.publish(frame(4));
        assert_eq!(viewer.latest().map(|(_, frame)| frame.width), Some(4));
    }

    #[test]
    fn chroma_is_half_the_frame_rounded_up() {
        let mut odd = frame(5);
        odd.height = 3;
        assert_eq!(odd.chroma_size(), (3, 2));
    }
}
