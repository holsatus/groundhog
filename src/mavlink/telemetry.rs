//! The telemetry stream adapter: decodes and coalesces router events.
//!
//! The router emits one [`Event`] per received frame. Decoding a frame into the
//! default dialect is middleware work, so it lives here rather than in the UI
//! root: [`coalesce`] decodes off the UI thread and delivers ready batches.

use std::time::{Duration, Instant};

use iced::futures::{SinkExt, Stream, StreamExt};
use mavio::{DefaultDialect, error::SpecError};

use crate::{foundation::MavlinkId, link::LinkId};

use super::Event;

/// An inbound router event with its frame already decoded.
///
/// Decoding runs on the coalescing tokio task, off the UI thread, so `update`
/// only has to fold ready messages into the fleet.
// `DefaultDialect` is inherently large but each value is moved straight into
// the fleet; the enum only lives for the span of one coalesced batch.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Telemetry {
    Message {
        source: MavlinkId,
        received_at: Instant,
        decoded: Option<DefaultDialect>,
    },
    LinkClosed(LinkId),
}

impl From<Event> for Telemetry {
    fn from(event: Event) -> Self {
        match event {
            Event::Message(inbound) => {
                let decoded = match inbound.frame.decode::<DefaultDialect>() {
                    Ok(message) => Some(message),
                    // Downgrade these to trace to avoid spamming the logs
                    Err(mavio::Error::Spec(SpecError::NotInDialect(id))) => {
                        log::trace!("mavlink message id {id} is not in default dialect");
                        None
                    }
                    Err(error) => {
                        log::trace!("could not decode mavlink frame: {error}");
                        None
                    }
                };

                Telemetry::Message {
                    source: inbound.source_mav,
                    received_at: inbound.received_at,
                    decoded,
                }
            }
            Event::LinkClosed(link_id) => Telemetry::LinkClosed(link_id),
        }
    }
}

/// Cap the rate at which inbound MAVLink events reach `update`.
///
/// The router emits one event per received frame. Without this, a busy link
/// would rebuild the whole view once per frame; instead events are collected
/// and delivered in order in a single batch at most once per `frame`.
pub fn coalesce(
    events: impl Stream<Item = Event> + Send + 'static,
    frame: Duration,
) -> impl Stream<Item = Vec<Telemetry>> + Send + 'static {
    iced::stream::channel(1, async move |mut output| {
        let mut events = Box::pin(events);
        let mut tick = tokio::time::interval(frame);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await; // consume the immediate first tick

        let mut batch = Vec::new();
        loop {
            tokio::select! {
                biased;
                _ = tick.tick() => {
                    if !batch.is_empty()
                        && output.send(std::mem::take(&mut batch)).await.is_err()
                    {
                        break;
                    }
                }
                event = events.next() => match event {
                    Some(event) => batch.push(Telemetry::from(event)),
                    None => break,
                },
            }
        }
    })
}
