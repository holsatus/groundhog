//! The UI features, and the contract between them and the application.
//!
//! A feature owns its view state and operator state (drafts, edits, running
//! operations) and nothing else. Vehicle state reaches it as an immutable
//! [`FleetSnapshot`]; operations go through [`FleetHandle`]. Everything a
//! feature wants the *application* to do — ask for confirmation, change the
//! selected vehicle, move the map, remember a directory — it requests through
//! its [`Context`] as an [`Effect`], so features never reach into each other or
//! into the application's state.

pub mod calibration;
pub mod confirm;
pub mod link_panel;
pub mod log_panel;
pub mod mission;
pub mod notifications;
pub mod operations;
pub mod parameter_panel;
pub mod settings_panel;
pub mod vehicles;

use std::path::{Path, PathBuf};

use iced::theme::Palette;

use groundhog_logic::{
    SystemId,
    domain::{fleet::FleetSnapshot, geo::GeoPosition},
    services::FleetHandle,
};

use self::{confirm::Confirmation, notifications::Notifications};

/// Something a feature asks the application to do.
#[derive(Debug)]
pub enum Effect<M> {
    /// Ask the operator before sending `M`.
    Confirm(Confirmation<M>),
    /// Make this the vehicle the UI shows.
    SelectVehicle(SystemId),
    /// Centre the map here (and stop following).
    CenterMap(GeoPosition),
    /// Follow the selected vehicle on the map, or stop.
    FollowVehicle(bool),
    /// Remember the directory of the last used file dialog.
    RememberDirectory(PathBuf),
}

impl<M> Effect<M> {
    pub fn map<N>(self, map: impl FnOnce(M) -> N) -> Effect<N> {
        match self {
            Effect::Confirm(confirmation) => Effect::Confirm(confirmation.map(map)),
            Effect::SelectVehicle(system) => Effect::SelectVehicle(system),
            Effect::CenterMap(position) => Effect::CenterMap(position),
            Effect::FollowVehicle(follow) => Effect::FollowVehicle(follow),
            Effect::RememberDirectory(path) => Effect::RememberDirectory(path),
        }
    }
}

/// What a feature's `update` may use and request.
pub struct Context<'a, M> {
    /// The fleet as last published.
    pub fleet: &'a FleetSnapshot,
    pub services: &'a FleetHandle,
    pub notices: &'a mut Notifications,
    /// Where file dialogs open.
    pub directory: Option<&'a Path>,
    /// Application settings features consult.
    pub settings: &'a crate::config::Settings,
    effects: Vec<Effect<M>>,
}

impl<'a, M> Context<'a, M> {
    pub fn new(
        fleet: &'a FleetSnapshot,
        services: &'a FleetHandle,
        notices: &'a mut Notifications,
        directory: Option<&'a Path>,
        settings: &'a crate::config::Settings,
    ) -> Self {
        Self {
            fleet,
            services,
            notices,
            directory,
            settings,
            effects: Vec::new(),
        }
    }

    pub fn request(&mut self, effect: Effect<M>) {
        self.effects.push(effect);
    }

    pub fn confirm(&mut self, confirmation: Confirmation<M>) {
        self.request(Effect::Confirm(confirmation));
    }

    /// The effects requested during the update.
    pub fn into_effects(self) -> Vec<Effect<M>> {
        self.effects
    }
}

/// What map plugins draw from.
#[derive(Clone, Copy)]
pub struct MapContext<'a> {
    pub fleet: &'a FleetSnapshot,
    pub selected: Option<SystemId>,
    pub palette: Palette,
}

/// A file dialog opening in `directory`, if known.
#[cfg(not(target_os = "android"))]
pub fn file_dialog(directory: Option<&Path>) -> rfd::AsyncFileDialog {
    let dialog = rfd::AsyncFileDialog::new();
    match directory {
        Some(directory) => dialog.set_directory(directory),
        None => dialog,
    }
}

/// A stand-in for `rfd::AsyncFileDialog` on Android.
///
/// Android has no native file dialog through `rfd`; picking and saving files
/// needs the Storage Access Framework, which is a later pass. Until then the
/// dialog reports "cancelled", so the file actions do nothing instead of
/// failing to compile.
#[cfg(target_os = "android")]
pub fn file_dialog(_directory: Option<&Path>) -> AndroidFileDialog {
    AndroidFileDialog
}

#[cfg(target_os = "android")]
pub struct AndroidFileDialog;

#[cfg(target_os = "android")]
impl AndroidFileDialog {
    pub fn set_file_name(self, _name: &str) -> Self {
        self
    }

    pub fn add_filter(self, _name: &str, _extensions: &[&str]) -> Self {
        self
    }

    pub async fn pick_file(self) -> Option<AndroidFileHandle> {
        None
    }

    pub async fn save_file(self) -> Option<AndroidFileHandle> {
        None
    }
}

#[cfg(target_os = "android")]
pub struct AndroidFileHandle(PathBuf);

#[cfg(target_os = "android")]
impl AndroidFileHandle {
    pub fn path(&self) -> &Path {
        &self.0
    }
}
