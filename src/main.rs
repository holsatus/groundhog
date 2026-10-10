mod app;
mod config;
mod domain;
mod features;
mod foundation;
mod link;
mod mav;
mod style;
mod widget;

use app::Application;

fn main() -> Result<(), impl std::error::Error> {
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
