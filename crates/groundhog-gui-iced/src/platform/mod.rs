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

/// Whether the platform is a touch screen: no hover, and fingers rather
/// than a pointer, so the interface floats over the map by default and
/// shows no tooltips.
pub const TOUCH: bool = cfg!(target_os = "android");

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
    ///
    /// `scale` is the application scale factor from [`Settings`]. The insets
    /// are logical pixels (dp), while iced lays the user interface out in
    /// pixels that the application scale factor has already enlarged, so the
    /// padding must shrink by the same factor to keep covering the same part
    /// of the screen.
    ///
    /// [`Settings`]: crate::config::Settings
    pub fn padding(self, scale: f32) -> iced::Padding {
        let scale = if scale > 0.0 { scale } else { 1.0 };

        iced::Padding {
            top: self.top / scale,
            right: self.right / scale,
            bottom: self.bottom / scale,
            left: self.left / scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_shrinks_by_the_application_scale() {
        let insets = Insets {
            top: 24.0,
            right: 0.0,
            bottom: 48.0,
            left: 0.0,
        };

        let padding = insets.padding(2.0);

        assert_eq!(padding.top, 12.0);
        assert_eq!(padding.bottom, 24.0);
        assert_eq!(Insets::ZERO.padding(2.0), iced::Padding::ZERO);
    }
}
