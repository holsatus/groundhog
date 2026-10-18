//! What differs between the desktop and Android builds: where the
//! application's own files live, and how logging is initialised.
//!
//! Everything else in the frontend is platform-neutral. Android has no
//! `$HOME` and no usable `stderr`, so both come from the platform instead of
//! `directories` and `env_logger`.

#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(not(target_os = "android"))]
pub use desktop::{config_dir, data_dir, init_logging};

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android::{config_dir, data_dir};
