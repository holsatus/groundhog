//! Groundhog's iced frontend: the desktop ground control station.
//!
//! Everything here is presentation and operator state. Vehicle state and
//! every rule about it live in `groundhog-logic`, started once at boot as a
//! [`groundhog_logic::Backend`].

mod app;
mod config;
mod features;
mod layout;
mod map;
mod ui;
mod widget;

use app::Application;

fn main() -> iced::Result {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Error)
        .filter_module("groundhog_logic", log::LevelFilter::Debug)
        .filter_module("groundhog_gui_iced", log::LevelFilter::Debug)
        .init();

    iced::application(Application::boot, Application::update, Application::view)
        .subscription(Application::subscription)
        .scale_factor(Application::scale_factor)
        .title(Application::title)
        .theme(Application::theme)
        .run()
}
