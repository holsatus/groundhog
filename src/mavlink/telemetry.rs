//! The telemetry stream adapter: decodes and coalesces router events.
//!
//! The router emits one [`Event`] per received frame. Decoding a frame into the
//! default dialect is middleware work, so it lives here rather than in the UI
//! root: [`coalesce`] decodes off the UI thread and delivers ready batches.
//!
//! This module depends only on `futures`/`tokio`/`mavio`; the middleware layer
//! must not pull in the GUI framework.

use std::time::{Duration, Instant};

use futures::{Stream, StreamExt, stream};
use mavio::{DefaultDialect, error::SpecError};
use tokio::time::{Interval, MissedTickBehavior, interval};

use crate::{foundation::MavlinkId, link::LinkId};

use super::Event;

/// An inbound router event with its frame already decoded.
///
/// Decoding runs on the coalescing task, off the UI thread, so `update` only
/// has to fold ready messages into the fleet.
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
///
/// Built from `futures`/`tokio` primitives only. The stream state is the events
/// stream, the ticker, and the pending batch; each tick yields the batch
/// accumulated so far. The ticker is created on first poll so constructing the
/// stream needs no runtime context.
pub fn coalesce(
    events: impl Stream<Item = Event> + Send + 'static,
    frame: Duration,
) -> impl Stream<Item = Vec<Telemetry>> + Send + 'static {
    stream::unfold(
        (Box::pin(events), None::<Interval>, Vec::new()),
        move |(mut events, tick, mut batch)| async move {
            let mut tick = match tick {
                Some(tick) => tick,
                None => {
                    let mut tick = interval(frame);
                    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    tick.tick().await; // consume the immediate first tick
                    tick
                }
            };

            loop {
                // `biased` polls the ticker first, so a ready tick flushes the
                // batch before another event is folded in.
                let ready = tokio::select! {
                    biased;
                    _ = tick.tick() => !batch.is_empty(),
                    event = events.next() => match event {
                        Some(event) => {
                            batch.push(Telemetry::from(event));
                            false
                        }
                        None => return None,
                    },
                };

                if ready {
                    let batch_out = std::mem::take(&mut batch);
                    return Some((batch_out, (events, Some(tick), batch)));
                }
            }
        },
    )
}
