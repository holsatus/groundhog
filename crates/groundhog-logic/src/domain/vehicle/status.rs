//! Vehicle status text log.
//!
//! `STATUSTEXT` carries at most 50 characters. Longer texts are sent as chunks
//! sharing a non-zero `id`, numbered by `chunk_seq`; the text ends with the
//! first chunk that is not completely filled. Chunks are reassembled here, so a
//! long pre-arm failure reads as one message.

use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

use mavio::default_dialect::{enums::MavSeverity, messages};

/// The most status texts retained per vehicle; older ones are dropped.
const MAX_STATUS_TEXTS: usize = 1000;

/// Chunks of an unfinished text older than this are published as they are.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(2);

/// The payload size of one `STATUSTEXT`.
const CHUNK_LEN: usize = 50;

/// Severity ordering, most severe (`0`) to least severe (`7`).
///
/// MAVLink defines the discriminants in this order, but deriving a total order
/// from them is a domain rule, not an implementation detail to leave to the UI.
pub fn rank(severity: MavSeverity) -> u8 {
    match severity {
        MavSeverity::Emergency => 0,
        MavSeverity::Alert => 1,
        MavSeverity::Critical => 2,
        MavSeverity::Error => 3,
        MavSeverity::Warning => 4,
        MavSeverity::Notice => 5,
        MavSeverity::Info => 6,
        MavSeverity::Debug => 7,
    }
}

/// Severities from most to least severe, for a picker.
pub const ORDERED: [MavSeverity; 8] = [
    MavSeverity::Emergency,
    MavSeverity::Alert,
    MavSeverity::Critical,
    MavSeverity::Error,
    MavSeverity::Warning,
    MavSeverity::Notice,
    MavSeverity::Info,
    MavSeverity::Debug,
];

#[derive(Debug, Clone, Default)]
pub struct StatusLog {
    pub texts: VecDeque<StatusText>,
    /// Texts being reassembled, by `(component, id)`.
    pending: BTreeMap<(u8, u16), Pending>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusText {
    /// The component that emitted the text. `STATUSTEXT` is accepted from any
    /// component, so each message is attributed to its source.
    pub component: u8,
    pub at: Instant,
    pub severity: MavSeverity,
    pub text: String,
}

#[derive(Clone, Debug)]
struct Pending {
    severity: MavSeverity,
    started: Instant,
    next_chunk: u8,
    text: String,
}

impl StatusLog {
    /// Fold in a `STATUSTEXT` from `component`. Returns the text it
    /// completed, if any (a single-chunk text completes immediately).
    pub(crate) fn observe_statustext(
        &mut self,
        component: u8,
        message: &messages::Statustext,
        time: Instant,
    ) -> Option<StatusText> {
        self.flush_stale(time);

        let null = message.text.iter().position(|c| *c == 0);
        let chunk =
            String::from_utf8_lossy(&message.text[..null.unwrap_or(CHUNK_LEN)]).into_owned();

        if message.id == 0 {
            return Some(self.push(StatusText {
                component,
                at: time,
                severity: message.severity,
                text: chunk,
            }));
        }

        let key = (component, message.id);
        let pending = self.pending.entry(key).or_insert_with(|| Pending {
            severity: message.severity,
            started: time,
            next_chunk: 0,
            text: String::new(),
        });

        if message.chunk_seq != pending.next_chunk {
            // A chunk went missing; mark the gap rather than splice words.
            pending.text.push('…');
        }
        pending.text.push_str(&chunk);
        pending.next_chunk = message.chunk_seq.wrapping_add(1);

        // A chunk that does not fill the payload ends the text.
        if null.is_some() {
            let pending = self.pending.remove(&key).expect("just inserted");
            return Some(self.push(StatusText {
                component,
                at: pending.started,
                severity: pending.severity,
                text: pending.text,
            }));
        }

        None
    }

    fn push(&mut self, text: StatusText) -> StatusText {
        self.texts.push_back(text.clone());
        if self.texts.len() > MAX_STATUS_TEXTS {
            self.texts.pop_front();
        }
        text
    }

    /// Publish unfinished texts whose last chunk never came.
    fn flush_stale(&mut self, now: Instant) {
        let stale: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| now.saturating_duration_since(pending.started) > CHUNK_TIMEOUT)
            .map(|(key, _)| *key)
            .collect();

        for key in stale {
            let pending = self.pending.remove(&key).expect("listed above");
            self.push(StatusText {
                component: key.0,
                at: pending.started,
                severity: pending.severity,
                text: pending.text + "…",
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(text: &str, id: u16, chunk_seq: u8) -> messages::Statustext {
        let mut bytes = [0u8; CHUNK_LEN];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        messages::Statustext {
            severity: MavSeverity::Warning,
            text: bytes,
            id,
            chunk_seq,
        }
    }

    #[test]
    fn a_single_text_completes_at_once() {
        let mut log = StatusLog::default();
        let done = log.observe_statustext(1, &chunk("Armed", 0, 0), Instant::now());
        assert_eq!(done.unwrap().text, "Armed");
        assert_eq!(log.texts.len(), 1);
    }

    #[test]
    fn chunks_reassemble_into_one_text() {
        let mut log = StatusLog::default();
        let now = Instant::now();
        let full = "x".repeat(CHUNK_LEN);

        assert!(
            log.observe_statustext(1, &chunk(&full, 7, 0), now)
                .is_none()
        );
        let done = log
            .observe_statustext(1, &chunk("tail", 7, 1), now)
            .unwrap();

        assert_eq!(done.text, format!("{full}tail"));
        assert_eq!(log.texts.len(), 1);
    }

    #[test]
    fn an_unfinished_text_is_flushed_after_a_while() {
        let mut log = StatusLog::default();
        let now = Instant::now();
        let full = "y".repeat(CHUNK_LEN);

        log.observe_statustext(1, &chunk(&full, 3, 0), now);
        log.observe_statustext(1, &chunk("later", 0, 0), now + CHUNK_TIMEOUT * 2);

        assert_eq!(log.texts.len(), 2);
        assert!(log.texts[0].text.ends_with('…'));
    }

    #[test]
    fn invalid_utf8_is_kept_lossily() {
        let mut log = StatusLog::default();
        let mut message = chunk("ok", 0, 0);
        message.text[2] = 0xff;
        let done = log.observe_statustext(1, &message, Instant::now()).unwrap();
        assert!(done.text.starts_with("ok"));
    }
}
