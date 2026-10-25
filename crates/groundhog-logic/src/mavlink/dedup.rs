//! Cross-link duplicate suppression.
//!
//! The same packet can reach the router on more than one link (a relay, a
//! shared upstream, a retransmitting modem). Folding it twice would
//! double-count into vehicle state (duplicated status texts, doubled trail
//! points), so the first arriving copy of a packet from a source wins and the
//! rest are dropped.
//!
//! A packet is identified by its [`PacketKey`]: the sequence number *and* the
//! message id and frame checksum. The sequence alone is not enough: autopilots
//! keep one sequence counter per channel, so two links to the same vehicle
//! carry different packets whose sequence numbers collide all the time. The
//! checksum covers the sequence, message id and payload, so a forwarded copy of
//! a packet matches it exactly while an unrelated packet practically never does.
//! What is caught is therefore byte-identical copies, not repeated content: an
//! autopilot sending the same text on two channels sends two packets.
//!
//! A packet inside the window is always a duplicate, however many arrive in a
//! row: a link that stalls and then delivers its backlog (a store-and-forward
//! radio, a TCP connection catching up) delivers nothing but copies, and
//! accepting any of them would fold them again and make the lagging link the
//! preferred route. A sender that reboots and starts its sequence again is
//! not mistaken for its old packets, because its new packets differ in
//! content. The exception is one that replays exactly the same bytes (a
//! deterministic simulator restarted at once): its first packets drop until
//! [`SEQUENCE_RESET_GAP`] after the last accepted one, when the window is
//! flushed, a bounded loss at a moment the vehicle reads as lost anyway.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};

use super::lock;
use crate::MavlinkId;

/// Size of the inbound duplicate-suppression window, per source.
///
/// Sender sequence numbers are `u8` and wrap around at 256. A window
/// *strictly below* 256 has a precise property: a `(source, sequence)`
/// reappearing inside the window can only be a multipath duplicate of an
/// unexpired packet, since a legitimate wrap-around reappear of the same
/// sequence would require more than 192 intervening packets from that
/// sender. Under sender sequence aliasing (two hosts sharing one address)
/// both readings are pathological, and dropping the first copy is the
/// consistent choice either way. The oldest entry is evicted to make room.
const DUPLICATE_WINDOW: usize = 192;

/// Silence from a source at which its duplicate window is flushed.
///
/// Matches the fleet's presumed-lost window: a sender that reboots and resets
/// its sequence counter takes longer than this before it speaks again, while
/// genuine multipath duplicates of a live packet do not. After the gap a
/// fresh window cannot mistake a reboot's first frames for old duplicates.
const SEQUENCE_RESET_GAP: Duration = Duration::from_secs(10);

/// What identifies one packet from a source, for duplicate detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PacketKey {
    pub sequence: u8,
    pub message_id: u32,
    pub checksum: u16,
}

/// The duplicate-suppression state of one source: its most recent inbound
/// packets, plus when a frame from it was last accepted into state.
#[derive(Debug)]
struct SequenceWindow {
    sequences: VecDeque<PacketKey>,
    /// When a frame from this source was last accepted (as opposed to
    /// dropped): a long silence flushes the window for a sequence reset.
    last_accepted: Instant,
}

impl SequenceWindow {
    fn new(now: Instant) -> Self {
        Self {
            sequences: VecDeque::new(),
            last_accepted: now,
        }
    }
}

/// The per-source sequence windows behind cross-link duplicate suppression.
#[derive(Default)]
pub(super) struct Dedup {
    seen: Mutex<BTreeMap<MavlinkId, SequenceWindow>>,
}

impl Dedup {
    /// Record a packet of a source and report whether an identical packet is
    /// already inside [`DUPLICATE_WINDOW`], i.e. the frame is a cross-link
    /// duplicate.
    ///
    /// A [`SEQUENCE_RESET_GAP`]-sized silence since the last accepted frame
    /// flushes the window, so a sender that rebooted is not held to the
    /// packets of its previous run.
    pub(super) fn seen_before(&self, source: MavlinkId, packet: PacketKey, now: Instant) -> bool {
        let mut seen = lock(&self.seen);
        let window = seen
            .entry(source)
            .or_insert_with(|| SequenceWindow::new(now));

        // Flush a window that may belong to a previous run of the sender: if
        // the gap is at least the reset gap, a fresh sequence cannot be an
        // unexpired duplicate.
        if now.duration_since(window.last_accepted) >= SEQUENCE_RESET_GAP {
            window.sequences.clear();
        }

        if window.sequences.contains(&packet) {
            return true;
        }

        window.sequences.push_back(packet);
        while window.sequences.len() > DUPLICATE_WINDOW {
            window.sequences.pop_front();
        }
        window.last_accepted = now;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: MavlinkId = MavlinkId {
        system: 5,
        component: 1,
    };

    fn packet(sequence: u8) -> PacketKey {
        PacketKey {
            sequence,
            message_id: 0,
            checksum: 0xbeef,
        }
    }

    #[test]
    fn a_long_gap_flushes_the_window_for_a_sequence_reset() {
        let dedup = Dedup::default();
        let t0 = Instant::now();

        // Warm the window with sequence 0; a repeat inside the window drops.
        assert!(!dedup.seen_before(SOURCE, packet(0), t0));
        assert!(dedup.seen_before(SOURCE, packet(0), t0));

        // After a presumed-lost-sized silence the same sequence passes: the
        // sender's counter has reset (reboot), so it is not an old packet.
        let after_gap = t0 + SEQUENCE_RESET_GAP;
        assert!(
            !dedup.seen_before(SOURCE, packet(0), after_gap),
            "the long gap must flush the window"
        );

        // Immediately after, the normal duplicate rule applies again.
        assert!(
            dedup.seen_before(SOURCE, packet(0), after_gap),
            "the freshly accepted sequence drops if repeated normally"
        );
    }

    #[test]
    fn a_backlog_of_copies_is_dropped_however_long() {
        let dedup = Dedup::default();
        let mut now = Instant::now();

        // One link delivers 40 packets; the other then flushes its backlog
        // of the same 40, as a stalled radio or TCP connection does.
        for sequence in 0..40 {
            assert!(!dedup.seen_before(SOURCE, packet(sequence), now));
        }
        for sequence in 0..40 {
            now += Duration::from_millis(1);
            assert!(
                dedup.seen_before(SOURCE, packet(sequence), now),
                "copy {sequence} of the backlog must drop"
            );
        }
    }

    #[test]
    fn colliding_sequences_of_different_packets_are_not_duplicates() {
        let dedup = Dedup::default();
        let now = Instant::now();

        // Two channels of one autopilot: same sequence, different packets.
        let heartbeat = PacketKey {
            sequence: 42,
            message_id: 0,
            checksum: 0x1111,
        };
        let attitude = PacketKey {
            sequence: 42,
            message_id: 30,
            checksum: 0x2222,
        };

        assert!(!dedup.seen_before(SOURCE, heartbeat, now));
        assert!(!dedup.seen_before(SOURCE, attitude, now));
        // The forwarded copy of either still drops.
        assert!(dedup.seen_before(SOURCE, attitude, now));
    }
}
