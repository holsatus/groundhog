//! The UI panels, and the contract between them and the application.
//!
//! A panel owns its view state and operator state (drafts, edits, running
//! operations) and nothing else. Vehicle state reaches it as an immutable
//! [`FleetSnapshot`]; operations go through [`FleetHandle`]. Everything a
//! panel wants the *application* to do — ask for confirmation, change the
//! selected vehicle, move the map, remember a directory, persist what it
//! holds, change a link — it requests through its [`Context`] as an
//! [`Effect`], so panels never reach into each other or into the
//! application's state.
//!
//! Every panel implements [`Panel`]: one interface for its messages, its
//! view, and what it draws on the map. The application lists
//! its panels once (`app/panels.rs`) and routes to them through it.

pub mod calibration;
pub mod confirm;
pub mod links;
pub mod logs;
pub mod mission;
pub mod notifications;
pub mod operations;
pub mod parameters;
pub mod settings;
#[cfg(test)]
mod test_support;
pub mod vehicles;
pub mod video;

use std::{
    fmt,
    path::{Path, PathBuf},
    time::Instant,
};

use iced::{Element, Task, theme::Palette};

use groundhog_logic::{
    SystemId,
    domain::{fleet::FleetSnapshot, geo::GeoPosition},
    link::{LinkConfig, LinkId},
    services::{FleetHandle, LinkManager},
};

use self::{confirm::Confirmation, notifications::Notifications};
use crate::map::LayerSet;

/// A panel the operator opens in a pane or a sheet.
///
/// The application owns one of each, routes its messages to
/// [`Panel::update`] with a [`Context`], shows [`Panel::view`] wherever the
/// shell puts it, and draws its [`Panel::layers`] on the map.
pub trait Panel {
    type Message: Clone + fmt::Debug + Send + 'static;

    /// Handle one of the panel's messages.
    fn update(
        &mut self,
        message: Self::Message,
        cx: &mut Context<'_, Self::Message>,
    ) -> Task<Self::Message>;

    /// The panel's content, the same in every shell.
    fn view<'a>(&'a self, cx: ViewContext<'a>) -> Element<'a, Self::Message>;

    /// The layers the panel draws on the map, if it draws any.
    fn layers<'a>(&'a self, _map: MapContext<'a>, _layers: &mut LayerSet<'a, Self::Message>) {}

    /// The fleet published a new snapshot (`cx.fleet`).
    fn on_fleet(&mut self, _cx: &mut Context<'_, Self::Message>) -> Task<Self::Message> {
        Task::none()
    }

    /// Whether the panel must hear [`Panel::on_tick`] as time passes, with
    /// nothing else changing.
    fn wants_ticks(&self) -> bool {
        false
    }

    /// Time passed, or what the panel reads from the application changed
    /// (the settings, the shell): bring the panel up to date as of `now`.
    fn on_tick(
        &mut self,
        _now: Instant,
        _cx: &mut Context<'_, Self::Message>,
    ) -> Task<Self::Message> {
        Task::none()
    }
}

/// What a panel's view may read: the application's state as it is now.
#[derive(Clone, Copy)]
pub struct ViewContext<'a> {
    /// The fleet as last published.
    pub fleet: &'a FleetSnapshot,
    /// The vehicle the UI shows.
    pub selected: Option<SystemId>,
    /// Whether the map follows the selected vehicle.
    pub following: bool,
    /// The operator's notices, as the log shows them.
    pub notices: &'a Notifications,
    /// The links and their status.
    pub links: &'a LinkManager,
    /// The application's settings.
    pub settings: &'a crate::config::Settings,
    /// The file telemetry is being recorded to, if any.
    pub recorder: Option<&'a Path>,
    /// Where videos are recorded to, if anywhere.
    pub recordings: Option<&'a Path>,
}

/// Something a panel asks the application to do.
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
    /// Save what a panel keeps in the configuration file.
    Persist(Persisted),
    /// Change the application's settings to these.
    Settings(crate::config::Settings),
    /// Change the links. The application makes the change, tells the
    /// operator what came of it, and saves the links if they changed.
    Link(LinkRequest),
}

/// A change to the links, which only the application makes: a panel reads
/// the links, but a mistake in one cannot drop a vehicle's connection.
#[derive(Debug, Clone)]
pub enum LinkRequest {
    /// Add this link, or update the one of the same name.
    Save(LinkConfig),
    Connect(LinkId),
    Disconnect(LinkId),
    /// Remove a link, dropping any live connection with it.
    Delete(LinkId),
}

/// What a panel keeps in the configuration file, which the application
/// owns and writes.
#[derive(Debug, Clone)]
pub enum Persisted {
    /// The video's source and options.
    Video(crate::config::VideoSettings),
}

impl<M> Effect<M> {
    pub fn map<N>(self, map: impl FnOnce(M) -> N) -> Effect<N> {
        match self {
            Effect::Confirm(confirmation) => Effect::Confirm(confirmation.map(map)),
            Effect::SelectVehicle(system) => Effect::SelectVehicle(system),
            Effect::CenterMap(position) => Effect::CenterMap(position),
            Effect::FollowVehicle(follow) => Effect::FollowVehicle(follow),
            Effect::RememberDirectory(path) => Effect::RememberDirectory(path),
            Effect::Persist(persisted) => Effect::Persist(persisted),
            Effect::Settings(settings) => Effect::Settings(settings),
            Effect::Link(request) => Effect::Link(request),
        }
    }
}

/// What a panel's `update` may use and request.
pub struct Context<'a, M> {
    /// The fleet as last published.
    pub fleet: &'a FleetSnapshot,
    pub services: &'a FleetHandle,
    /// The links and their status. A change is asked with
    /// [`Effect::Link`].
    pub links: &'a LinkManager,
    pub notices: &'a mut Notifications,
    /// Where file dialogs open.
    pub directory: Option<&'a Path>,
    /// Application settings panels consult.
    pub settings: &'a crate::config::Settings,
    effects: Vec<Effect<M>>,
}

impl<'a, M> Context<'a, M> {
    pub fn new(
        fleet: &'a FleetSnapshot,
        services: &'a FleetHandle,
        links: &'a LinkManager,
        notices: &'a mut Notifications,
        directory: Option<&'a Path>,
        settings: &'a crate::config::Settings,
    ) -> Self {
        Self {
            fleet,
            services,
            links,
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
