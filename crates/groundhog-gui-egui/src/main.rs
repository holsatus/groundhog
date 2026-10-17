//! Groundhog's egui frontend: the desktop ground control station.
//!
//! Everything here is presentation and operator state. Vehicle state and
//! every rule about it live in `groundhog-logic`, started once at boot as a
//! [`groundhog_logic::Backend`] inside the application's async runtime, the
//! same way the iced frontend does it.

mod app;
mod config;
mod driver;
mod features;
mod map;
mod style;

use eframe::{NativeOptions, egui};

use app::Application;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Error)
        .filter_module("groundhog_logic", log::LevelFilter::Debug)
        .filter_module("groundhog_gui_egui", log::LevelFilter::Debug)
        .init();

    // The application runs inside the async runtime, so `tokio::spawn` works
    // from anywhere on the UI thread, as it does in iced: operation tasks and
    // the event forwarders live on the runtime's workers.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime
        .block_on(async {
            let options = NativeOptions {
                viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
                ..Default::default()
            };

            eframe::run_native(
                "Holsatus Groundhog",
                options,
                Box::new(|creation| Ok(Box::new(Application::new(creation)))),
            )
        })
        .map_err(|error| anyhow::anyhow!("the event loop stopped: {error}"))
}
