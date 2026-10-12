mod app;
mod config;
mod domain;
mod features;
mod foundation;
mod layout;
mod link;
mod mavlink;
mod services;
mod storage;
mod style;
mod theme;
mod widget;
mod wire;

use app::Application;

fn main() -> iced::Result {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Error)
        .filter_module("groundhog", log::LevelFilter::Debug)
        .init();

    iced::application(Application::boot, Application::update, Application::view)
        .subscription(Application::subscription)
        .theme(Application::theme)
        .title(Application::title)
        .run()
}
