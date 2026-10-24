//! The application root: composition, message routing, and policy.
//!
//! The root wires the layers together at boot, routes messages to features,
//! applies the [`Effect`]s features request, and decides which fleet and link
//! events become operator notices. It holds the latest fleet snapshot, which
//! the fleet runtime publishes from off the UI thread.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use groundhog_logic::{
    Backend, BackendOptions, SystemId,
    domain::{
        calibration::Phase,
        fleet::{FleetEvent, FleetSnapshot},
        vehicle::{Liveness, status},
    },
    services::{FleetUpdate, LinkEvent, LinkStatus, TlogRecorder},
};
use iced::{
    Element, Length, Size, Subscription, Task, Theme, Widget, keyboard,
    widget::{container, sensor, stack},
};
use mavio::default_dialect::enums::MavSeverity;

use crate::{
    config::{self, ConfigWriter, Configuration, Settings},
    features::{
        Context, Effect, MapContext,
        calibration::{self, CalibrationPanel},
        confirm::{self, Answer, Confirmation},
        link_panel::{self, LinkPanel},
        log_panel::{self, LogPanel},
        mission::{self, MissionEditor},
        notifications::{self, Level, Notice, Notifications},
        parameter_panel::{self, ParameterPanel},
        settings_panel,
        vehicles::{self, VehiclePanel},
        video::{self, VideoPanel},
    },
    layout::{self, Layout},
    map::{self, LayerSet, Map, Placement},
    platform::{self, Insets},
    shell::{self, Floating, Shell},
    ui,
};

/// The features that take a [`Context`], grouped so their state can be
/// borrowed apart from the context's.
#[derive(Default)]
struct Features {
    vehicles: VehiclePanel,
    mission: MissionEditor,
    parameters: ParameterPanel,
    calibration: CalibrationPanel,
}

pub struct Application {
    layout: Layout,
    map: Map,
    configuration: Configuration,
    config_writer: ConfigWriter,
    backend: Backend,
    /// The fleet as last published by the runtime.
    fleet: Arc<FleetSnapshot>,
    /// The vehicle the UI shows.
    selected: Option<SystemId>,
    /// The keyboard modifiers held, for the rail's clicks.
    modifiers: keyboard::Modifiers,
    /// The panel whose rail button is being dragged onto the grid.
    rail_drag: Option<layout::Panel>,
    recorder: Option<TlogRecorder>,
    notices: Notifications,
    confirmation: Option<Confirmation<Message>>,
    features: Features,
    link_panel: LinkPanel,
    video: VideoPanel,
    log_panel: LogPanel,
    /// How much of the window the platform's own chrome covers.
    insets: Insets,
    /// The theme whose system-bar icons were requested, if any.
    system_bars: Option<bool>,
    /// The scale the settings' slider is being dragged to, applied on
    /// release.
    scale_preview: Option<f32>,
    /// The window's full screen: the application's own (F11), apart from
    /// the video's.
    full_screen: FullScreen,
    /// The floating shell's sheet.
    floating: Floating,
    /// The window's size, once measured; it decides the shell.
    window: Option<Size>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The fleet runtime published.
    Fleet(FleetUpdate),
    /// A link's supervisor changed status.
    Link(LinkEvent),
    /// A configuration write finished.
    ConfigurationSaved(Result<(), String>),

    /// The window was laid out at this size.
    Window(Size),
    Layout(layout::Message),
    Floating(shell::Message),
    /// A rail button was clicked with the primary button; the modifiers
    /// held decide where its panel opens.
    Rail(layout::Panel),
    /// A rail button is being dragged; the grid shows where its panel
    /// would land.
    RailDragged(layout::Panel),
    /// The rail button's drag ended: dropped (the grid reports where, as a
    /// layout message) or cancelled.
    RailDropped,
    /// The keyboard modifiers changed.
    Modifiers(keyboard::Modifiers),
    Map(map::Message),
    Vehicles(vehicles::Message),
    Mission(mission::Message),
    Parameters(parameter_panel::Message),
    Calibration(calibration::Message),
    Links(link_panel::Message),
    Logs(log_panel::Message),
    Video(video::Message),
    Settings(settings_panel::Message),
    Notifications(notifications::Message),
    Confirm(Answer),

    /// Debounce tick that flushes a pending layout save.
    LayoutSaveTick,
    /// Refreshes time-dependent views (toast expiry, link countdowns).
    Tick(Instant),
    /// The platform's chrome moved, appeared or disappeared (the status and
    /// navigation bars, the soft keyboard, a rotation).
    Insets(Insets),
    /// Put the whole application in full screen, or take it out (F11).
    ToggleFullScreen,
}

/// Idle time after the last layout change before the layout is persisted.
const LAYOUT_SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

impl Application {
    pub fn boot() -> (Self, Task<Message>) {
        let mut notices = Notifications::default();

        let configuration = Configuration::initialize().unwrap_or_else(|error| {
            notices.push(Notice::error("Could not read the configuration").detail(error));
            Configuration::default()
        });

        // The backend spawns its tasks here: boot runs inside the
        // application's async runtime.
        let (backend, events) = Backend::start(BackendOptions {
            links: configuration.links.clone(),
            ..BackendOptions::default()
        });

        let (config_writer, saved) = config::spawn_writer();

        let layout = configuration
            .workspace
            .clone()
            .map_or_else(Layout::new, Layout::from_workspace);

        // The video comes back on if it was on when the application closed.
        let (video, video_task) = VideoPanel::new(configuration.video.clone());

        let mut application = Application {
            layout,
            map: Map::new(),
            configuration,
            config_writer,
            backend,
            fleet: Arc::default(),
            selected: None,
            modifiers: keyboard::Modifiers::default(),
            rail_drag: None,
            recorder: None,
            notices,
            confirmation: None,
            features: Features::default(),
            link_panel: LinkPanel::new(),
            video,
            log_panel: LogPanel::new(),
            insets: platform::insets(),
            system_bars: None,
            scale_preview: None,
            full_screen: FullScreen::default(),
            floating: Floating::default(),
            window: None,
        };

        if application.configuration.settings.record_telemetry {
            application.start_recording();
        }

        application.sync_system_bars();

        let saved = futures::stream::unfold(saved, |mut saved| async move {
            saved.recv().await.map(|outcome| (outcome, saved))
        });

        let tasks = Task::batch([
            Task::stream(events.fleet.into_stream()).map(Message::Fleet),
            Task::stream(events.links.into_stream()).map(Message::Link),
            Task::stream(saved).map(Message::ConfigurationSaved),
            video_task.map(Message::Video),
        ]);

        (application, tasks)
    }

    pub fn theme(&self) -> Theme {
        self.configuration.settings.theme.theme()
    }

    pub fn title(&self) -> String {
        "Holsatus Groundhog".to_string()
    }

    pub fn scale_factor(&self) -> f32 {
        self.configuration.settings.scale_factor
    }

    fn save_configuration(&self) {
        self.config_writer.save(self.configuration.clone());
    }

    fn start_recording(&mut self) {
        let Some(directory) = config::data_dir().map(|directory| directory.join("tlogs")) else {
            self.notices
                .push(Notice::error("No data directory to record telemetry into"));
            return;
        };

        match self.backend.record(&directory) {
            Ok(recorder) => self.recorder = Some(recorder),
            Err(error) => self
                .notices
                .push(Notice::error("Could not start telemetry recording").detail(error)),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ToggleFullScreen => {
                self.full_screen.toggle_app();
                self.sync_full_screen()
            }
            Message::Insets(insets) => {
                self.insets = insets;
                self.sync_system_bars();
                Task::none()
            }
            Message::Fleet(update) => {
                self.on_fleet(update);
                Task::none()
            }
            Message::Link(event) => {
                self.on_link(event);
                Task::none()
            }
            Message::ConfigurationSaved(result) => {
                if let Err(error) = result {
                    self.notices
                        .push(Notice::error("Could not save the configuration").detail(error));
                }
                Task::none()
            }
            Message::Window(size) => {
                self.window = Some(size);
                Task::none()
            }
            Message::Floating(message) => {
                // Back with nowhere left to go leaves, as a phone app does.
                if !self.floating.update(message) {
                    platform::leave();
                }
                Task::none()
            }
            Message::Layout(message) => {
                if self.layout.update(message) {
                    self.configuration.workspace = Some(self.layout.workspace().clone());
                }
                Task::none()
            }
            Message::Rail(panel) => {
                // A plain click opens on the left; the modifiers stand in
                // for the middle and right buttons a trackpad lacks.
                let slot = if self.modifiers.command() {
                    layout::Slot::Middle
                } else if self.modifiers.shift() {
                    layout::Slot::Right
                } else {
                    layout::Slot::Left
                };
                self.update(Message::Layout(layout::Message::Toggle(panel, slot)))
            }
            Message::RailDragged(panel) => {
                self.rail_drag = Some(panel);
                Task::none()
            }
            Message::RailDropped => {
                self.rail_drag = None;
                Task::none()
            }
            Message::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
            Message::Map(message) => self.map.update(message).map(Message::Map),
            Message::Vehicles(message) => self.dispatch(Message::Vehicles, |features, cx| {
                features.vehicles.update(message, cx)
            }),
            Message::Mission(message) => self.dispatch(Message::Mission, |features, cx| {
                features.mission.update(message, cx)
            }),
            Message::Parameters(message) => self.dispatch(Message::Parameters, |features, cx| {
                features.parameters.update(message, cx)
            }),
            Message::Calibration(message) => self.dispatch(Message::Calibration, |features, cx| {
                features.calibration.update(message, cx)
            }),
            Message::Links(message) => {
                let (task, event) =
                    self.link_panel
                        .update(message, self.backend.links_mut(), &mut self.notices);

                match event {
                    Some(link_panel::Event::LinksChanged(links)) => {
                        self.configuration.links = links;
                        self.save_configuration();
                    }
                    Some(link_panel::Event::Confirm(confirmation)) => {
                        self.confirmation = Some(confirmation.map(Message::Links));
                    }
                    None => {}
                }
                task.map(Message::Links)
            }
            Message::Video(message) => {
                let (task, event) = self.video.update(message);
                if let Some(video::Event::Changed(settings)) = event {
                    self.configuration.video = settings;
                    self.save_configuration();
                }
                Task::batch([task.map(Message::Video), self.sync_full_screen()])
            }
            Message::Logs(message) => {
                self.log_panel.update(message);
                Task::none()
            }
            Message::Settings(message) => {
                self.on_settings(message);
                Task::none()
            }
            Message::Notifications(message) => {
                self.notices.update(message);
                Task::none()
            }
            Message::Confirm(answer) => match (answer, self.confirmation.take()) {
                (Answer::Accept, Some(confirmation)) => Task::done(confirmation.accept),
                _ => Task::none(),
            },
            Message::LayoutSaveTick => {
                if self.layout.is_dirty() {
                    self.layout.clear_dirty();
                    self.save_configuration();
                }
                Task::none()
            }
            Message::Tick(now) => {
                self.notices.update(notifications::Message::Tick(now));
                Task::none()
            }
        }
    }

    /// Run a feature update with a fresh [`Context`], then apply the effects
    /// it requested.
    fn dispatch<M>(
        &mut self,
        wrap: fn(M) -> Message,
        update: impl FnOnce(&mut Features, &mut Context<M>) -> Task<M>,
    ) -> Task<Message>
    where
        M: Send + 'static,
    {
        let fleet = self.fleet.clone();
        let mut context = Context::new(
            &fleet,
            self.backend.fleet(),
            &mut self.notices,
            self.configuration.file_picker_path.as_deref(),
            &self.configuration.settings,
        );

        let task = update(&mut self.features, &mut context).map(wrap);
        let effects = context.into_effects();

        for effect in effects {
            self.apply(effect.map(wrap));
        }

        task
    }

    fn apply(&mut self, effect: Effect<Message>) {
        match effect {
            Effect::Confirm(confirmation) => self.confirmation = Some(confirmation),
            Effect::SelectVehicle(system) => {
                if self.fleet.contains(system) {
                    self.selected = Some(system);
                    self.follow_selected();
                }
            }
            Effect::CenterMap(position) => {
                self.map.set_following(false);
                self.map.center_on(position, self.map_placement());
            }
            Effect::FollowVehicle(follow) => {
                self.map.set_following(follow);
                self.follow_selected();
            }
            Effect::RememberDirectory(path) => self.remember_directory(&path),
        }
    }

    fn remember_directory(&mut self, path: &Path) {
        let Some(directory) = path.is_dir().then_some(path).or(path.parent()) else {
            return;
        };

        if self.configuration.file_picker_path.as_deref() != Some(directory) {
            self.configuration.file_picker_path = Some(directory.to_path_buf());
            self.save_configuration();
        }
    }

    /// Keep the map on the selected vehicle, if following.
    fn follow_selected(&mut self) {
        if let Some(position) = self
            .selected
            .and_then(|system| self.fleet.vehicle(system))
            .and_then(|vehicle| vehicle.position.current.as_ref())
        {
            self.map.follow_to(position.pos, self.map_placement());
        }
    }

    fn on_fleet(&mut self, update: FleetUpdate) {
        self.fleet = update.snapshot;
        self.features.mission.sync(&self.fleet);

        for event in update.events {
            self.on_fleet_event(event);
        }

        self.follow_selected();
    }

    /// Fleet events become selection changes and operator notices.
    fn on_fleet_event(&mut self, event: FleetEvent) {
        let name = |system| self.fleet.vehicle_name(system);

        let notice = match event {
            FleetEvent::Registered(system) => {
                if self.selected.is_none() {
                    self.selected = Some(system);
                }
                Notice::info(format!("{} connected", name(system))).vehicle(system)
            }
            // A late vehicle shows in the vehicle panel; only losing and
            // regaining contact are announced, so a flaky link does not
            // flood the operator.
            FleetEvent::LivenessChanged { system, from, to } => match (from, to) {
                (_, Liveness::Lost) => {
                    Notice::error(format!("Lost contact with {}", name(system))).vehicle(system)
                }
                (Liveness::Lost, Liveness::Active) => {
                    Notice::success(format!("Contact with {} restored", name(system)))
                        .vehicle(system)
                }
                _ => return,
            },
            FleetEvent::ArmedChanged(system, true) => {
                Notice::warning(format!("{} armed", name(system))).vehicle(system)
            }
            FleetEvent::ArmedChanged(system, false) => {
                Notice::info(format!("{} disarmed", name(system))).vehicle(system)
            }
            FleetEvent::ModeChanged(system, mode) => Notice::info(format!(
                "{}: {} mode",
                name(system),
                mode.map_or("unknown", |mode| mode.name)
            ))
            .vehicle(system),
            FleetEvent::StatusText(system, text) => {
                // Status texts are in the vehicle's own log; only the serious
                // ones are toasted, and not duplicated into the history.
                let level = match status::rank(text.severity) {
                    rank if rank <= status::rank(MavSeverity::Error) => Level::Error,
                    rank if rank <= status::rank(MavSeverity::Warning) => Level::Warning,
                    _ => return,
                };
                self.notices.toast(
                    Notice::new(level, format!("{}: {}", name(system), text.text)).vehicle(system),
                );
                return;
            }
            FleetEvent::OnboardMissionStale(system) => Notice::warning(format!(
                "The mission on {} was changed elsewhere",
                name(system)
            ))
            .detail("Download it to see the vehicle's current mission.")
            .vehicle(system),
            FleetEvent::CalibrationFinished(system, kind, phase) => {
                let subject = format!("{kind} calibration of {}", name(system));
                match phase {
                    Phase::Succeeded => Notice::success(format!("{subject} succeeded")),
                    Phase::Failed(reason) => {
                        Notice::error(format!("{subject} failed")).detail(reason)
                    }
                    Phase::Cancelled => Notice::info(format!("{subject} cancelled")),
                    Phase::Starting | Phase::Running => return,
                }
                .vehicle(system)
            }
        };

        self.notices.push(notice);
    }

    fn on_link(&mut self, event: LinkEvent) {
        let was_connected = event.previous.is_connected();
        let notice = match &event.status {
            LinkStatus::Connected { .. } if !was_connected => {
                Some(Notice::success(format!("Link {} connected", event.name)))
            }
            LinkStatus::Reconnecting { error, .. } if was_connected => Some(
                Notice::warning(format!("Link {} lost; reconnecting", event.name)).detail(error),
            ),
            LinkStatus::Failed(error) => {
                Some(Notice::error(format!("Link {} failed", event.name)).detail(error))
            }
            _ => None,
        };

        if let Some(notice) = notice {
            self.notices.push(notice);
        }
    }

    fn on_settings(&mut self, message: settings_panel::Message) {
        let settings = &mut self.configuration.settings;
        let before = *settings;

        match message {
            settings_panel::Message::Theme(mode) => settings.theme = mode,
            settings_panel::Message::Shell(mode) => settings.shell = mode,
            settings_panel::Message::TouchControls(touch) => settings.touch_controls = touch,
            settings_panel::Message::PreviewScaleFactor(scale) => self.scale_preview = Some(scale),
            settings_panel::Message::CommitScaleFactor => {
                if let Some(scale) = self.scale_preview.take() {
                    settings.scale_factor = scale;
                }
            }
            settings_panel::Message::DefaultAltitude(altitude) => {
                settings.default_waypoint_altitude_m = altitude;
            }
            settings_panel::Message::RecordTelemetry(record) => settings.record_telemetry = record,
            settings_panel::Message::RestoreDefaults => {
                self.confirmation = Some(
                    Confirmation::new(
                        "Restore the default settings?",
                        "Every setting goes back to its default. Links and the panel layout are kept.",
                        Message::Settings(settings_panel::Message::DefaultsConfirmed),
                    )
                    .accept_label("Restore"),
                );
            }
            settings_panel::Message::DefaultsConfirmed => *settings = Settings::default(),
        }

        if self.configuration.settings != before {
            self.save_configuration();
        }

        self.sync_recording();
        self.sync_system_bars();
    }

    /// Starts or stops recording to match the setting.
    fn sync_recording(&mut self) {
        let record = self.configuration.settings.record_telemetry;
        if record && self.recorder.is_none() {
            self.start_recording();
        } else if !record && let Some(recorder) = self.recorder.take() {
            recorder.stop();
        }
    }

    /// Puts the window in the mode the application and the video ask for,
    /// if it is not in it already.
    fn sync_full_screen(&mut self) -> Task<Message> {
        match self.full_screen.sync(self.video.is_full_screen()) {
            Some(mode) => {
                iced::window::latest().and_then(move |window| iced::window::set_mode(window, mode))
            }
            None => Task::none(),
        }
    }

    /// Matches the status- and navigation-bar icons to the theme: dark icons
    /// on a light palette, light ones on a dark palette.
    fn sync_system_bars(&mut self) {
        let dark = self.theme().palette().is_dark;

        if self.system_bars != Some(dark) {
            self.system_bars = Some(dark);
            platform::set_system_bars(dark);
        }
    }

    /// The shell the window is laid out in.
    fn shell(&self) -> Shell {
        self.configuration
            .settings
            .shell
            .resolve(self.window, platform::TOUCH)
    }

    /// How large controls are drawn: for a finger in the floating shell,
    /// and everywhere when the operator asks for it.
    fn density(&self) -> ui::Density {
        if self.configuration.settings.touch_controls || self.shell() == Shell::Floating {
            ui::Density::Touch
        } else {
            ui::Density::Pointer
        }
    }

    pub fn view(&self) -> impl Widget<Message> + '_ {
        ui::set_density(self.density());

        // The full-screen video still shows the notices: an alert must not
        // go unseen for watching the video.
        let base = if self.video.is_full_screen() {
            let insets = self.insets.padding(self.scale_factor());
            stack![
                self.video.view_full_screen(insets).map(Message::Video),
                self.view_toasts(ui::space::XL),
            ]
            .boxed()
        } else {
            match self.shell() {
                Shell::Tiled => self.view_tiled(),
                Shell::Floating => self.view_floating(),
            }
        };

        // The window is measured by what fills it, whatever the shell puts
        // inside, so the measure follows every resize.
        sensor(
            container(confirm::view(
                base,
                self.confirmation.as_ref(),
                Message::Confirm,
            ))
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .on_show(Message::Window)
        .on_resize(Message::Window)
    }

    /// The toasts, in the bottom-right corner of the space they are given,
    /// `inset` from its edges.
    fn view_toasts(&self, inset: f32) -> Element<'_, Message> {
        container(self.notices.view().map(Message::Notifications))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(inset)
            .boxed()
    }

    /// How the map is placed in the shell on screen.
    fn map_placement(&self) -> Placement {
        match self.shell() {
            Shell::Tiled => Placement::Pane,
            // In the corner, while the video fills the screen.
            Shell::Floating if self.video_is_main() => Placement::Pane,
            Shell::Floating => Placement::Window {
                covered: self.floating_covered(),
            },
        }
    }

    /// Whether the floating shell's video fills the map's screen.
    fn video_is_main(&self) -> bool {
        self.video.slot().is_some() && self.floating.is_video_main()
    }

    /// The map with the features' layers, placed as `placement`.
    fn view_map(&self, placement: Placement) -> Element<'_, Message> {
        let theme = self.theme();
        let context = MapContext {
            fleet: &self.fleet,
            selected: self.selected,
            palette: *theme.palette(),
        };

        let mut layers = LayerSet::new();
        layers.plugin(&self.features.vehicles, context, Message::Vehicles);
        layers.plugin(&self.features.mission, context, Message::Mission);

        self.map.view(layers, Message::Map, &theme, placement)
    }

    /// A panel's content, the same in every shell.
    fn pane_body(&self, panel: layout::Panel) -> Element<'_, Message> {
        let fleet: &FleetSnapshot = &self.fleet;

        match panel {
            // A `Stack` hides the pointer from the map only where an upper
            // layer shows one: here, over the toasts, which are meant to
            // hide it.
            layout::Panel::Map => stack![
                self.view_map(Placement::Pane),
                self.view_toasts(ui::space::M)
            ]
            .boxed(),
            layout::Panel::Vehicles => self
                .features
                .vehicles
                .view(fleet, self.selected, self.map.is_following())
                .map(Message::Vehicles)
                .boxed(),
            layout::Panel::Parameters => self
                .features
                .parameters
                .view(fleet, self.selected)
                .map(Message::Parameters)
                .boxed(),
            layout::Panel::Calibration => self
                .features
                .calibration
                .view(fleet, self.selected)
                .map(Message::Calibration)
                .boxed(),
            layout::Panel::Mission => self
                .features
                .mission
                .view(fleet, self.selected)
                .map(Message::Mission)
                .boxed(),
            layout::Panel::Links => self
                .link_panel
                .view(self.backend.links())
                .map(Message::Links)
                .boxed(),
            layout::Panel::Video => self.video.view().map(Message::Video).boxed(),
            layout::Panel::Logs => self
                .log_panel
                .view(fleet, self.selected, &self.notices)
                .map(Message::Logs)
                .boxed(),
            layout::Panel::Settings => settings_panel::view(
                &self.configuration.settings,
                self.scale_preview,
                self.recorder.as_ref().map(TlogRecorder::path),
            )
            .map(Message::Settings)
            .boxed(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // Trailing debounce for layout persistence: the timer is restarted on
        // every layout change (the revision is part of the subscription
        // identity), so the write happens `LAYOUT_SAVE_DEBOUNCE` after the last
        // change rather than repeatedly throughout a drag/resize.
        let layout_tick = if self.layout.is_dirty() {
            iced::time::every(LAYOUT_SAVE_DEBOUNCE)
                .with(self.layout.revision())
                .map(|_| Message::LayoutSaveTick)
        } else {
            Subscription::none()
        };

        // Time-dependent views only need refreshing while they show something.
        let active_links = self
            .backend
            .links()
            .entries()
            .any(|(_, _, status)| status.is_active());
        let refresh = if self.notices.has_toasts() || active_links {
            iced::time::every(Duration::from_secs(1)).map(Message::Tick)
        } else {
            Subscription::none()
        };

        let modifiers = iced::event::listen_with(|event, _, _| match event {
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::Modifiers(modifiers))
            }
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::F11),
                repeat: false,
                ..
            }) => Some(Message::ToggleFullScreen),
            _ => None,
        });

        // Escape, or the system's back (a phone's gesture, a mouse's back
        // button), leaves the full-screen video; elsewhere in the floating
        // shell, back and Escape where nothing else took it step back.
        let back = if self.video.is_full_screen() {
            iced::event::listen_with(|event, _, _| {
                let leave = match event {
                    iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => matches!(
                        key,
                        keyboard::Key::Named(
                            keyboard::key::Named::Escape | keyboard::key::Named::BrowserBack
                        )
                    ),
                    iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                        iced::mouse::Button::Back,
                    )) => true,
                    _ => false,
                };
                leave.then_some(Message::Video(video::Message::FullScreen(false)))
            })
        } else if self.shell() == Shell::Floating {
            iced::event::listen_with(|event, status, _| {
                let back = match event {
                    iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
                        matches!(key, keyboard::Key::Named(keyboard::key::Named::BrowserBack))
                            || (status == iced::event::Status::Ignored
                                && key == keyboard::Key::Named(keyboard::key::Named::Escape))
                    }
                    iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                        iced::mouse::Button::Back,
                    )) => true,
                    _ => false,
                };
                back.then_some(Message::Floating(shell::Message::Back))
            })
        } else {
            Subscription::none()
        };

        Subscription::batch([
            layout_tick,
            refresh,
            modifiers,
            back,
            platform::insets_subscription().map(Message::Insets),
        ])
    }
}

/// The window's full screen. The operator asks for it twice over: for the
/// whole application (F11), and for the video (its own button, left with
/// Escape). The window is full screen while either asks, so leaving one
/// leaves the other as it was: the video, left, does not take a full-screen
/// application out of full screen.
#[derive(Debug, Default)]
struct FullScreen {
    /// Whether the application is full screen (F11).
    app: bool,
    /// Whether the window is, as last set.
    window: bool,
}

impl FullScreen {
    fn toggle_app(&mut self) {
        self.app = !self.app;
    }

    /// The mode the window must change to, now that the video is full
    /// screen or not, if it must change.
    fn sync(&mut self, video: bool) -> Option<iced::window::Mode> {
        let full_screen = self.app || video;
        if full_screen == self.window {
            return None;
        }
        self.window = full_screen;
        Some(if full_screen {
            iced::window::Mode::Fullscreen
        } else {
            iced::window::Mode::Windowed
        })
    }
}

mod floating;
#[cfg(test)]
mod gallery;
mod tiled;

#[cfg(test)]
mod tests {
    use super::FullScreen;
    use iced::window::Mode;

    #[test]
    fn leaving_the_video_keeps_the_application_full_screen() {
        let mut full_screen = FullScreen::default();

        // F11, then the video's full screen, then leaving the video: the
        // window goes full screen once, and stays.
        full_screen.toggle_app();
        assert_eq!(full_screen.sync(false), Some(Mode::Fullscreen));
        assert_eq!(full_screen.sync(true), None);
        assert_eq!(full_screen.sync(false), None, "still the application's");

        // F11 again, outside the video: windowed.
        full_screen.toggle_app();
        assert_eq!(full_screen.sync(false), Some(Mode::Windowed));
    }

    #[test]
    fn the_video_alone_comes_and_goes() {
        let mut full_screen = FullScreen::default();

        assert_eq!(full_screen.sync(true), Some(Mode::Fullscreen));
        assert_eq!(full_screen.sync(false), Some(Mode::Windowed));
    }

    #[test]
    fn f11_inside_the_video_is_kept_for_after_it() {
        let mut full_screen = FullScreen::default();
        assert_eq!(full_screen.sync(true), Some(Mode::Fullscreen));

        // F11 while the video fills the screen changes nothing yet...
        full_screen.toggle_app();
        assert_eq!(full_screen.sync(true), None);
        // ...and leaving the video leaves the application full screen.
        assert_eq!(full_screen.sync(false), None);
    }
}
