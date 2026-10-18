//! Groundhog's iced frontend: the ground control station.
//!
//! Everything here is presentation and operator state. Vehicle state and
//! every rule about it live in `groundhog-logic`, started once at boot as a
//! [`groundhog_logic::Backend`].
//!
//! This is a library as well as a binary because Android has no `main`: the
//! platform loads the `cdylib` built from this crate and calls the exported
//! `android_main`, which starts the same `application` as the desktop binary.

mod app;
mod config;
mod features;
mod layout;
mod map;
mod platform;
mod ui;
mod widget;

use app::Application;

/// The application definition shared by every platform entry point.
fn application()
-> iced::Application<impl iced::Program<Message = app::Message, Theme = iced::Theme>> {
    iced::application(Application::boot, Application::update, Application::view)
        .subscription(Application::subscription)
        .scale_factor(Application::scale_factor)
        .title(Application::title)
        .theme(Application::theme)
}

/// Runs the desktop application.
#[cfg(not(target_os = "android"))]
pub fn run() -> iced::Result {
    platform::init_logging();
    application().run()
}

/// Runs the application on Android.
///
/// `android-activity` calls this from the `NativeActivity` glue once the
/// activity is created. The [`AndroidApp`] is the handle to the parts of the
/// platform that exist only there: the app's data paths, and the association
/// winit's event loop needs.
///
/// [`AndroidApp`]: android_activity::AndroidApp
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: android_activity::AndroidApp) {
    platform::android::init(app.clone());
    let _ = application().run_android(app);
}
