//! What differs between the desktop and Android builds: where the
//! application's own files live, how logging is initialised, and how much of
//! the window the platform's own chrome takes.
//!
//! Everything else in the frontend is platform-neutral. Android has no
//! `$HOME`, no usable `stderr`, and draws its status and navigation bars over
//! the application, so all three come from the platform instead of
//! `directories`, `env_logger` and nothing at all.

#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(not(target_os = "android"))]
pub use desktop::{
    config_dir, data_dir, init_logging, insets, insets_subscription, set_system_bars,
};

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android::{config_dir, data_dir, insets, insets_subscription, set_system_bars};

/// The space the platform's own chrome takes around the application's
/// content, in logical pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Insets {
    /// Nothing of the platform's chrome covers the application.
    pub const ZERO: Self = Self {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
    };

    /// The insets as padding, so no content is drawn under them.
    pub fn padding(self) -> iced::Padding {
        iced::Padding {
            top: self.top,
            right: self.right,
            bottom: self.bottom,
            left: self.left,
        }
    }
}
