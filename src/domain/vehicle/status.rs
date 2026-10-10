//! Vehicle status text log.

use std::{collections::VecDeque, time::Instant};

use mavio::default_dialect::{enums::MavSeverity, messages};

/// The most status texts retained per vehicle; older ones are dropped.
const MAX_STATUS_TEXTS: usize = 1000;

#[derive(Debug, Clone, Default)]
pub struct StatusLog {
    pub texts: VecDeque<StatusText>,
}

#[derive(Clone, Debug)]
pub struct StatusText {
    pub at: Instant,
    pub severity: MavSeverity,
    pub text: String,
}

impl StatusLog {
    /// Append a decoded `STATUSTEXT`, returning the UTF-8 error (without
    /// logging) if the payload is not valid.
    pub fn observe_statustext(
        &mut self,
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
            log.observe_statustext(&message(&i.to_string()), Instant::now())
                .expect("valid utf8");
        }

        assert_eq!(log.texts.len(), MAX_STATUS_TEXTS);
        assert_eq!(log.texts.front().unwrap().text, "10");
        assert_eq!(
            log.texts.back().unwrap().text,
            (MAX_STATUS_TEXTS + 9).to_string()
        );
    }
}
