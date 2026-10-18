//! Android paths, logging and the application handle.
//!
//! Android gives the app no `$HOME` to derive directories from, and app
//! `stderr` goes nowhere, so both come from the [`AndroidApp`] that
//! `android_main` receives.

use std::{path::PathBuf, sync::OnceLock};

use android_activity::AndroidApp;
use log::LevelFilter;

/// The handle Android gives `android_main`, kept for the parts of the
/// application that need platform APIs after boot (the data paths now;
/// insets and lifecycle events later).
static APP: OnceLock<AndroidApp> = OnceLock::new();

/// Prepares logging and remembers the application handle.
///
/// Called first thing in `android_main`, before iced starts.
pub fn init(app: AndroidApp) {
    let _ = APP.set(app);

    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("groundhog")
            .with_filter(
                android_logger::FilterBuilder::new()
                    .filter_level(LevelFilter::Error)
                    .filter_module("groundhog_logic", LevelFilter::Debug)
                    .filter_module("groundhog_gui_iced", LevelFilter::Debug)
                    .build(),
            ),
    );

    // A Rust panic otherwise writes to stderr, which Android discards; log it
    // so `adb logcat -s groundhog` shows what went wrong.
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
    }));
}

/// Where the configuration is written: a private directory inside the app's
/// internal storage.
pub fn config_dir() -> Option<PathBuf> {
    APP.get()?
        .internal_data_path()
        .map(|dir| dir.join("config"))
}

/// Where application data (telemetry logs) is kept.
pub fn data_dir() -> Option<PathBuf> {
    APP.get()?.internal_data_path().map(|dir| dir.join("data"))
}
