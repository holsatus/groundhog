//! Desktop paths and logging.

use std::{path::PathBuf, sync::LazyLock};

use directories::ProjectDirs;

use super::Insets;

static PROJECT_DIRS: LazyLock<Option<ProjectDirs>> =
    LazyLock::new(|| ProjectDirs::from("org", "Holsatus", "Groundhog"));

/// Where the configuration is written.
pub fn config_dir() -> Option<PathBuf> {
    PROJECT_DIRS
        .as_ref()
        .map(|dirs| dirs.config_dir().to_path_buf())
}

/// Where application data (telemetry logs) is kept.
pub fn data_dir() -> Option<PathBuf> {
    PROJECT_DIRS
        .as_ref()
        .map(|dirs| dirs.data_dir().to_path_buf())
}

/// Initialises logging to stderr.
pub fn init_logging() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Error)
        .filter_module("groundhog_logic", log::LevelFilter::Debug)
        .filter_module("groundhog_gui_iced", log::LevelFilter::Debug)
        .init();
}

/// Nothing of the platform's chrome covers the application.
pub fn insets() -> Insets {
    Insets::ZERO
}

/// No insets change.
pub fn insets_subscription() -> iced::Subscription<Insets> {
    iced::Subscription::none()
}

/// Back with nowhere left to go does nothing on the desktop: a window is
/// closed by the window manager.
pub fn leave() {}

/// The desktop draws no system bars over the application.
pub fn set_system_bars(_dark: bool) {}
