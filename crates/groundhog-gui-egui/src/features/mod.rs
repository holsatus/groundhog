//! The UI features, and the contract between them and the application.
//!
//! A feature owns its view state and operator state (drafts, edits, running
//! operations) and nothing else. Vehicle state reaches it as an immutable
//! [`FleetSnapshot`]; operations go through [`FleetHandle`]. Everything a
//! feature wants the *application* to do — ask for confirmation, change the
//! selected vehicle, move the map, remember a directory — it requests through
//! its [`Context`] as an [`Effect`], so features never reach into each other
//! or into the application's state.
//!
//! egui is immediate mode, so features have no `update`/`view` split: their
//! state changes whenever the operator interacts, during drawing, and when
//! operation results arrive. The rest of the contract is the same as the
//! iced frontend's.

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

use groundhog_logic::{
    SystemId,
    domain::{fleet::FleetSnapshot, geo::GeoPosition},
    services::FleetHandle,
};

use self::{confirm::Confirmation, notifications::Notifications};
/// Something a feature asks the application to do.
#[derive(Debug)]
pub enum Effect {
    /// Ask the operator before doing `action`.
    Confirm(Confirmation),
    /// Make this the vehicle the UI shows.
    SelectVehicle(SystemId),
    /// Centre the map here (and stop following).
    CenterMap(GeoPosition),
    /// Follow the selected vehicle on the map, or stop.
    FollowVehicle(bool),
    /// Remember the directory of the last used file dialog.
    RememberDirectory(PathBuf),
    /// The configured links changed; persist them.
    SaveLinks(Vec<groundhog_logic::link::LinkConfig>),
    /// The recording switch moved: start or stop the recorder.
    RecordTelemetry(bool),
    /// The configuration was mutated by the settings panel; persist it.
    SaveSettings,
}

/// What a feature's drawing and result handling may use and request.
pub struct Context<'a> {
    /// The fleet as last published.
    pub fleet: &'a FleetSnapshot,
    pub services: &'a FleetHandle,
    pub notices: &'a mut Notifications,
    /// Where file dialogs open.
    pub directory: Option<&'a Path>,
    /// Application settings features consult.
    pub settings: &'a crate::config::Settings,
    effects: Vec<Effect>,
}

impl<'a> Context<'a> {
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

    pub fn request(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    pub fn confirm(&mut self, confirmation: Confirmation) {
        self.request(Effect::Confirm(confirmation));
    }

    /// The effects requested while the context was held.
    pub fn into_effects(self) -> Vec<Effect> {
        self.effects
    }
}

/// A file dialog opening in `directory`, if known.
pub fn file_dialog(directory: Option<&Path>) -> rfd::AsyncFileDialog {
    let dialog = rfd::AsyncFileDialog::new();
    match directory {
        Some(directory) => dialog.set_directory(directory),
        None => dialog,
    }
}

/// A failed operation, ready to become a notice. Every panel's `Finished`
/// handler goes through this so failures read the same way everywhere.
pub fn failure_notice(title: String, error: impl std::fmt::Display) -> notifications::Notice {
    notifications::Notice::error(title).detail(error.to_string())
}
