//! Vehicle status text log.

use std::{collections::VecDeque, time::Instant};

use mavio::default_dialect::{enums::MavSeverity, messages};

/// The most status texts retained per vehicle; older ones are dropped.
const MAX_STATUS_TEXTS: usize = 1000;

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
}

#[derive(Clone, Debug)]
pub struct StatusText {
    /// The component that emitted the text. `STATUSTEXT` is accepted from any
    /// component, so each message is attributed to its source.
    pub component: u8,
    pub at: Instant,
    pub severity: MavSeverity,
    pub text: String,
}

impl StatusLog {
    /// Append a decoded `STATUSTEXT` from `component`, returning the UTF-8 error
    /// (without logging) if the payload is not valid.
    pub fn observe_statustext(
        &mut self,
        component: u8,
        message: &messages::Statustext,
        time: Instant,
    ) -> Result<(), std::str::Utf8Error> {
        let null = message
            .text
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(message.text.len());

        let text_string = str::from_utf8(&message.text[..null])?.to_string();

        self.texts.push_back(StatusText {
            component,
            at: time,
            severity: message.severity,
            text: text_string,
        });

        if self.texts.len() > MAX_STATUS_TEXTS {
            self.texts.pop_front();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> messages::Statustext {
        let mut bytes = [0u8; 50];
        let raw = text.as_bytes();
        bytes[..raw.len()].copy_from_slice(raw);

        messages::Statustext {
            severity: MavSeverity::Info,
            text: bytes,
            id: 0,
            chunk_seq: 0,
        }
    }

    #[test]
    fn status_log_keeps_newest_texts_within_cap() {
        let mut log = StatusLog::default();

        for i in 0..MAX_STATUS_TEXTS + 10 {
            log.observe_statustext(1, &message(&i.to_string()), Instant::now())
                .expect("valid utf8");
        }

        assert_eq!(log.texts.len(), MAX_STATUS_TEXTS);
        assert_eq!(log.texts.front().unwrap().text, "10");
        assert_eq!(
            log.texts.back().unwrap().text,
            (MAX_STATUS_TEXTS + 9).to_string()
        );
    }

    #[test]
    fn status_texts_are_attributed_to_their_component() {
        let mut log = StatusLog::default();

        log.observe_statustext(1, &message("from autopilot"), Instant::now())
            .expect("valid utf8");
        log.observe_statustext(154, &message("from gimbal"), Instant::now())
            .expect("valid utf8");

        assert_eq!(log.texts.len(), 2);
        assert_eq!(log.texts[0].component, 1);
        assert_eq!(log.texts[0].text, "from autopilot");
        assert_eq!(log.texts[1].component, 154);
        assert_eq!(log.texts[1].text, "from gimbal");
    }

    #[test]
    fn severity_rank_orders_most_to_least_severe() {
        assert!(rank(MavSeverity::Emergency) < rank(MavSeverity::Warning));
        assert!(rank(MavSeverity::Warning) < rank(MavSeverity::Debug));
        assert_eq!(rank(MavSeverity::Emergency), 0);
        assert_eq!(rank(MavSeverity::Debug), 7);

        // `ORDERED` lists the severities exactly most-to-least severe.
        assert_eq!(ORDERED.map(rank), [0, 1, 2, 3, 4, 5, 6, 7]);
    }
}
