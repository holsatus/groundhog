//! Vehicle status text log.

use std::time::Instant;

use mavio::default_dialect::{enums::MavSeverity, messages};

#[derive(Debug, Clone, Default)]
pub struct StatusLog {
    pub texts: Vec<StatusText>,
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

        self.texts.push(StatusText {
            at: time,
            severity: message.severity,
            text: text_string,
        });

        Ok(())
    }
}
