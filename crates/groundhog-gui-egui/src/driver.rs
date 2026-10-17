//! Bridging the async backend to the immediate-mode UI.
//!
//! The backend's event streams run as tokio tasks. A forwarder for each
//! channel pushes its items into a plain queue that the application drains
//! at the start of every frame, and asks egui for a repaint as they arrive.

use futures::{StreamExt, pin_mut};
use groundhog_logic::services::{BackendEvents, FleetUpdate, LinkEvent};
use tokio::sync::mpsc;

/// What arrives from the backend between frames.
#[derive(Debug)]
pub enum AppEvent {
    Fleet(FleetUpdate),
    Link(LinkEvent),
    ConfigurationSaved(Result<(), String>),
}

/// Start the forwarders for everything the backend publishes.
pub fn forward(
    ctx: egui::Context,
    events: BackendEvents,
    config_saves: mpsc::UnboundedReceiver<Result<(), String>>,
) -> mpsc::UnboundedReceiver<AppEvent> {
    let (sender, receiver) = mpsc::unbounded_channel::<AppEvent>();

    tokio::spawn({
        let sender = sender.clone();
        let ctx = ctx.clone();
        async move {
            let updates = events.fleet.into_stream();
            pin_mut!(updates);

            while let Some(update) = updates.next().await {
                let _ = sender.send(AppEvent::Fleet(update));
                ctx.request_repaint();
            }
        }
    });

    tokio::spawn({
        let sender = sender.clone();
        let ctx = ctx.clone();
        async move {
            let links = events.links.into_stream();
            pin_mut!(links);

            while let Some(event) = links.next().await {
                let _ = sender.send(AppEvent::Link(event));
                ctx.request_repaint();
            }
        }
    });

    tokio::spawn({
        let sender = sender;
        let ctx = ctx;
        async move {
            let mut saves = config_saves;
            while let Some(outcome) = saves.recv().await {
                let _ = sender.send(AppEvent::ConfigurationSaved(outcome));
                ctx.request_repaint();
            }
        }
    });

    receiver
}
