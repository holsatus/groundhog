//! The application root: composition, message routing, and policy.
//!
//! The root wires the layers together at boot, routes messages to panels,
//! applies the [`Effect`]s panels request, and decides which fleet and link
//! events become operator notices. It holds the latest fleet snapshot, which
//! the fleet runtime publishes from off the UI thread.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{StreamExt, stream};
use groundhog_logic::{
    Backend, BackendOptions, SystemId,
    domain::{
        calibration::Outcome,
        fleet::{FleetEvent, FleetSnapshot},
        vehicle::{Liveness, status},
    },
    services::{FleetUpdate, LinkEvent, LinkManager, LinkStatus, Saved, TlogRecorder},
};
use iced::{
    Element, Length, Size, Subscription, Task, Theme, Widget, keyboard,
    widget::{container, sensor, stack},
};
use mavio::default_dialect::enums::MavSeverity;

use self::panels::{PanelEnv, PanelMessage, Panels};

use crate::{
    config::{self, ConfigWriter, Configuration, Loaded, Settings},
    layout::{self, Layout},
    map::{self, LayerSet, Map, Placement},
    panels::{
        Effect, LinkRequest, MapContext, Persisted, ViewContext,
        calibration::CalibrationPanel,
        confirm::{self, Answer, Confirmation},
        links::LinkPanel,
        logs::LogPanel,
        mission::MissionPanel,
        notifications::{self, Level, Notice, Notifications},
        parameters::{self, ParameterPanel},
        settings::SettingsPanel,
        vehicles::VehiclePanel,
        video::{self, VideoPanel},
    },
    platform::{self, Insets},
    shell::{self, Floating, Shell},
    ui,
};

pub struct Application {
    layout: Layout,
    map: Map,
    configuration: Configuration,
    /// Saves the configuration; `None` when the file could not be read, so
    /// it is never overwritten with what this session made up instead.
    config_writer: Option<ConfigWriter>,
    backend: Backend,
    /// The fleet as last published by the runtime.
    fleet: Arc<FleetSnapshot>,
    /// The vehicle the UI shows.
    selected: Option<SystemId>,
    /// The keyboard modifiers held, for the rail's clicks.
    modifiers: keyboard::Modifiers,
    /// The panel whose rail button is being dragged onto the grid.
    rail_drag: Option<layout::PaneKind>,
    recorder: Option<TlogRecorder>,
    notices: Notifications,
    confirmation: Option<Confirmation<Message>>,
    panels: Panels,
    /// How much of the window the platform's own chrome covers.
    insets: Insets,
    /// The theme whose system-bar icons were requested, if any.
    system_bars: Option<bool>,
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
    /// The fleet runtime stopped: vehicle state no longer advances.
    FleetStopped,
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
    Rail(layout::PaneKind),
    /// A rail button is being dragged; the grid shows where its panel
    /// would land.
    RailDragged(layout::PaneKind),
    /// The rail button's drag ended: dropped (the grid reports where, as a
    /// layout message) or cancelled.
    RailDropped,
    /// The keyboard modifiers changed.
    Modifiers(keyboard::Modifiers),
    Map(map::Message),
    /// A message to one of the panels.
    Panel(PanelMessage),
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
    /// The operator asked to close the window.
    CloseRequested,
    /// Unsaved changes are saved (or could not be in time): leave.
    Exit,
    /// The operator went back (see [`back_event`]); `captured` when a widget
    /// took the key, as a text input takes Escape.
    Back {
        captured: bool,
    },
}

/// Idle time after the last layout change before the layout is persisted.
const LAYOUT_SAVE_DEBOUNCE: Duration = Duration::from_millis(800);
/// The longest closing waits for the last save, so a stuck disk cannot hold
/// the application open.
const CLOSE_SAVE_TIMEOUT: Duration = Duration::from_secs(2);

impl Application {
    pub fn boot() -> (Self, Task<Message>) {
        let mut notices = Notifications::default();

        // A file that exists but cannot be read is left alone: this session
        // runs on defaults and saves nothing over it.
        let (configuration, saving) = match Configuration::initialize() {
            Ok(Loaded {
                configuration,
                writable: true,
            }) => (configuration, Some(config::spawn_writer())),
            Ok(Loaded {
                configuration,
                writable: false,
            }) => {
                notices.push(
                    Notice::warning("The configuration is from a newer Groundhog")
                        .detail("It is used as it is, and changes in this session are not saved."),
                );
                (configuration, None)
            }
            Err(error) => {
                notices.push(
                    Notice::error("Could not read the configuration")
                        .detail(format!("Changes in this session are not saved. {error:#}")),
                );
                (Configuration::default(), None)
            }
        };
        let (config_writer, saved) = saving.unzip();

        // The backend spawns its tasks here: boot runs inside the
        // application's async runtime.
        let (backend, events) = Backend::start(BackendOptions {
            links: configuration.links.clone(),
            ..BackendOptions::default()
        });

        let layout = configuration
            .workspace
            .clone()
            .map_or_else(Layout::new, Layout::from_workspace);

        // The video comes back on if it was on when the application closed,
        // once something shows it (see `sync_panels`, below).
        let video = VideoPanel::new(
            configuration.video.clone(),
            config::data_dir().map(|directory| directory.join("videos")),
        );

        let mut application = Application {
            layout,
            map: Map::new(platform::cache_dir().map(|directory| directory.join("tiles"))),
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
            panels: Panels {
                vehicles: VehiclePanel::default(),
                mission: MissionPanel::default(),
                parameters: ParameterPanel::default(),
                calibration: CalibrationPanel::default(),
                links: LinkPanel::new(),
                video,
                logs: LogPanel::new(),
                settings: SettingsPanel::default(),
            },
            insets: platform::insets(),
            system_bars: None,
            full_screen: FullScreen::default(),
            floating: Floating::default(),
            window: None,
        };

        if application.configuration.settings.record_telemetry {
            application.start_recording();
        }

        application.sync_system_bars();
        let video_task = application.sync_panels();

        let saved = stream::unfold(saved, |saved| async move {
            let mut saved = saved?;
            saved.recv().await.map(|outcome| (outcome, Some(saved)))
        });

        let tasks = Task::batch([
            // The runtime stops only if its task dies: say so, rather than
            // show the last snapshot as if it were current.
            Task::stream(
                events
                    .fleet
                    .into_stream()
                    .map(Message::Fleet)
                    .chain(stream::once(async { Message::FleetStopped })),
            ),
            Task::stream(events.links.into_stream()).map(Message::Link),
            Task::stream(saved).map(Message::ConfigurationSaved),
            video_task,
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
        if let Some(writer) = &self.config_writer {
            writer.save(self.configuration.clone());
        }
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
                let task = self.on_fleet(update);
                Task::batch([task, self.sync_full_screen()])
            }
            Message::FleetStopped => {
                self.notices.push(
                    Notice::error("Vehicle state stopped updating")
                        .detail("What is shown is no longer current. Restart Groundhog."),
                );
                Task::none()
            }
            Message::Link(event) => {
                self.on_link(event);
                Task::none()
            }
            Message::CloseRequested => {
                // A layout change still in its debounce would be lost with
                // the window: save it now, and leave once it is written.
                if self.layout.is_dirty() {
                    self.layout.clear_dirty();
                    self.save_configuration();
                }
                match &self.config_writer {
                    Some(writer) => Task::perform(
                        tokio::time::timeout(CLOSE_SAVE_TIMEOUT, writer.flushed()),
                        |_| Message::Exit,
                    ),
                    None => iced::exit(),
                }
            }
            Message::Exit => iced::exit(),
            Message::ConfigurationSaved(result) => {
                if let Err(error) = result {
                    self.notices
                        .push(Notice::error("Could not save the configuration").detail(error));
                }
                Task::none()
            }
            Message::Window(size) => {
                // The size can change the shell, and with it whether the
                // video shows.
                self.window = Some(size);
                self.sync_panels()
            }
            Message::Back { captured } => {
                let target = back_target(
                    captured,
                    self.confirmation.is_some(),
                    self.panels.video.is_full_screen(),
                    self.shell() == Shell::Floating,
                );
                match target {
                    Some(BackTarget::Dismiss) => self.update(Message::Confirm(Answer::Dismiss)),
                    Some(BackTarget::LeaveVideo) => {
                        self.update(video::Message::FullScreen(false).into())
                    }
                    Some(BackTarget::StepBack) => {
                        self.update(Message::Floating(shell::Message::Back))
                    }
                    None => Task::none(),
                }
            }
            Message::Floating(message) => {
                // Back with nowhere left to go leaves, as a phone app does.
                if !self.floating.update(message) {
                    platform::leave();
                }
                Task::none()
            }
            Message::Layout(message) => {
                // A panel opens where a stack has room; say so where none has.
                if let layout::Message::Toggle(panel, _) = message
                    && !self.layout.has_room(panel)
                {
                    self.notices.push(
                        Notice::warning(format!("No room for {}", panel.title()))
                            .detail("Close a panel or enlarge the window."),
                    );
                }
                if self.layout.update(message) {
                    self.configuration.workspace = Some(self.layout.workspace().clone());
                }
                self.sync_panels()
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
            Message::Panel(message) => {
                let task = self.with_panels(|panels, env| panels.update(message, env));
                // The video's own full screen is the window's.
                Task::batch([task, self.sync_full_screen()])
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
                self.sync_panels()
            }
        }
    }

    /// Run `run` on the panels with what their updates are given, then
    /// apply the effects they requested.
    fn with_panels(
        &mut self,
        run: impl FnOnce(&mut Panels, &mut PanelEnv<'_>) -> Task<PanelMessage>,
    ) -> Task<Message> {
        let fleet = self.fleet.clone();
        let mut env = PanelEnv {
            fleet: &fleet,
            handle: self.backend.fleet(),
            links: self.backend.links(),
            notices: &mut self.notices,
            directory: self.configuration.file_picker_path.as_deref(),
            settings: &self.configuration.settings,
            effects: Vec::new(),
        };

        let task = run(&mut self.panels, &mut env).map(Message::Panel);
        let effects = env.effects;

        let applied: Vec<_> = effects
            .into_iter()
            .map(|effect| self.apply(effect.map(Message::Panel)))
            .collect();

        Task::batch([task].into_iter().chain(applied))
    }

    /// Do what a panel asked, returning any work it starts.
    fn apply(&mut self, effect: Effect<Message>) -> Task<Message> {
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
            Effect::Persist(persisted) => {
                match persisted {
                    Persisted::Video(settings) => self.configuration.video = settings,
                }
                self.save_configuration();
            }
            Effect::Settings(settings) => return self.change_settings(settings),
            Effect::Link(request) => {
                if change_links(self.backend.links_mut(), request, &mut self.notices) {
                    self.configuration.links = self.backend.links().configs();
                    self.save_configuration();
                }
            }
        }
        Task::none()
    }

    /// Take `settings` as the application's: persist them, and apply what
    /// changed.
    fn change_settings(&mut self, settings: Settings) -> Task<Message> {
        let before = self.configuration.settings;
        if settings == before {
            return Task::none();
        }
        self.configuration.settings = settings;
        self.save_configuration();

        // Only a change of the setting: a recording that could not start is
        // not retried (and its failure not announced again) on every other
        // settings message, such as each step of a scale slider.
        if settings.record_telemetry != before.record_telemetry {
            self.sync_recording();
        }
        self.sync_system_bars();
        // The panels may follow a setting (recording video while armed), and
        // the shell may have changed.
        self.sync_panels()
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

    fn on_fleet(&mut self, update: FleetUpdate) -> Task<Message> {
        self.fleet = update.snapshot;

        // The fleet's own news first: the panels may tell of what follows
        // from it (a recording that starts as a vehicle arms).
        for event in update.events {
            self.on_fleet_event(event);
        }
        let task = self.with_panels(|panels, env| panels.on_fleet(env));

        self.follow_selected();
        task
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
            FleetEvent::Rebooted(system) => {
                Notice::warning(format!("{} rebooted", name(system))).vehicle(system)
            }
            FleetEvent::CalibrationFinished(system, kind, outcome) => {
                let subject = format!("{kind} calibration of {}", name(system));
                match outcome {
                    Outcome::Succeeded => Notice::success(format!("{subject} succeeded")),
                    Outcome::Failed(reason) => {
                        Notice::error(format!("{subject} failed")).detail(reason)
                    }
                    Outcome::Cancelled => Notice::info(format!("{subject} cancelled")),
                }
                .vehicle(system)
            }
            FleetEvent::StatedEncodingContradicted {
                address,
                stated,
                inferred,
            } => Notice::warning(format!(
                "Component {} of {} encodes parameters {}, not {}",
                address.component,
                name(address.system_id()),
                parameters::encoding_name(inferred),
                parameters::encoding_name(stated),
            ))
            .detail(
                "Values written since its encoding was stated may have been stored wrong. \
                 Download its parameters and check them.",
            )
            .vehicle(address.system_id()),
            // The backend asks the camera to describe itself; the cameras
            // it describes show where they are used.
            FleetEvent::CameraAppeared(_) => return,
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

    /// Starts or stops recording to match the setting.
    fn sync_recording(&mut self) {
        let record = self.configuration.settings.record_telemetry;
        if record && self.recorder.is_none() {
            self.start_recording();
        } else if !record && let Some(recorder) = self.recorder.take() {
            recorder.stop();
        }
    }

    /// Tells the video whether anything can show it (see [`video_shown`]),
    /// so a stream nothing shows pauses instead of decoding unseen, then
    /// brings every panel up to date ([`Panel::on_tick`]): the settings or
    /// the shell may have changed, or time passed.
    ///
    /// [`Panel::on_tick`]: crate::panels::Panel::on_tick
    fn sync_panels(&mut self) -> Task<Message> {
        self.panels
            .video
            .set_shown(video_shown(self.shell(), &self.layout));
        let now = Instant::now();
        let task = self.with_panels(|panels, env| panels.on_tick(now, env));
        Task::batch([task, self.sync_full_screen()])
    }

    /// Puts the window in the mode the application and the video ask for,
    /// if it is not in it already.
    fn sync_full_screen(&mut self) -> Task<Message> {
        match self.full_screen.sync(self.panels.video.is_full_screen()) {
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
        let insets = self.insets.padding(self.scale_factor());

        // The full-screen video still shows the notices: an alert must not
        // go unseen for watching the video.
        let base = if self.panels.video.is_full_screen() {
            stack![
                self.panels
                    .video
                    .view_full_screen(insets, &self.fleet, self.selected)
                    .map(Message::from),
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
                insets,
            ))
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .on_show(Message::Window)
        .on_resize(Message::Window)
    }

    /// The toasts, in the bottom-right corner of the space they are given,
    /// `inset` from its edges and clear of the platform's chrome.
    fn view_toasts(&self, inset: f32) -> impl Widget<Message> + '_ {
        let chrome = self.insets.padding(self.scale_factor());

        ui::toast::layer(
            self.notices.view().map(Message::Notifications),
            iced::Padding {
                top: chrome.top + inset,
                right: chrome.right + inset,
                bottom: chrome.bottom + inset,
                left: chrome.left + inset,
            },
        )
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
        self.panels.video.slot().is_some() && self.floating.is_video_main()
    }

    /// The map with the panels' layers, placed as `placement`.
    fn view_map(&self, placement: Placement) -> impl Widget<Message> + '_ {
        let theme = self.theme();
        let context = MapContext {
            fleet: &self.fleet,
            selected: self.selected,
            palette: *theme.palette(),
        };

        let mut panels = LayerSet::new();
        self.panels.layers(context, &mut panels);
        let mut layers = LayerSet::new();
        layers.extend(panels, Message::Panel);

        self.map.view(layers, Message::Map, &theme, placement)
    }

    /// A panel's content, the same in every shell.
    fn pane_body(&self, panel: layout::PaneKind) -> Element<'_, Message> {
        match panel {
            // A `Stack` hides the pointer from the map only where an upper
            // layer shows one: here, over the toasts, which are meant to
            // hide it.
            layout::PaneKind::Map => stack![
                self.view_map(Placement::Pane),
                self.view_toasts(ui::space::M)
            ]
            .boxed(),
            panel => match self.panels.view(panel, self.view_context()) {
                Some(view) => view.map(Message::Panel).boxed(),
                None => ui::empty(format!("{} is not available.", panel.title())).boxed(),
            },
        }
    }

    /// What the panels' views read.
    fn view_context(&self) -> ViewContext<'_> {
        ViewContext {
            fleet: &self.fleet,
            selected: self.selected,
            following: self.map.is_following(),
            notices: &self.notices,
            links: self.backend.links(),
            settings: &self.configuration.settings,
            recorder: self.recorder.as_ref().map(TlogRecorder::path),
            recordings: self.panels.video.recordings(),
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
        let refresh = if self.notices.has_toasts() || active_links || self.panels.wants_ticks() {
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

        let back = iced::event::listen_with(back_event);

        Subscription::batch([
            layout_tick,
            refresh,
            modifiers,
            back,
            platform::insets_subscription().map(Message::Insets),
            iced::window::close_requests().map(|_| Message::CloseRequested),
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

/// A back gesture, if `event` is one: Escape, the system's back (a phone's
/// back gesture arrives as BrowserBack), or a mouse's back button. The one
/// definition of "back", answered by [`back_target`].
fn back_event(
    event: iced::Event,
    status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            ..
        }) => Some(Message::Back {
            captured: status == iced::event::Status::Captured,
        }),
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::BrowserBack),
            ..
        })
        | iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Back)) => {
            Some(Message::Back { captured: false })
        }
        _ => None,
    }
}

/// What a back gesture steps out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackTarget {
    /// The open confirmation, as its Dismiss would.
    Dismiss,
    /// The full-screen video.
    LeaveVideo,
    /// One level of the floating shell (from the bare map: the application).
    StepBack,
}

/// Who answers a back gesture, in order: an open confirmation, so back
/// never acts behind the question; the full-screen video; the floating
/// shell. A `captured` gesture (Escape in a text input) answers nothing but
/// the full-screen video, which nothing else on screen could leave.
fn back_target(
    captured: bool,
    confirming: bool,
    video_full_screen: bool,
    floating: bool,
) -> Option<BackTarget> {
    if confirming && !captured {
        Some(BackTarget::Dismiss)
    } else if video_full_screen {
        Some(BackTarget::LeaveVideo)
    } else if floating && !captured {
        Some(BackTarget::StepBack)
    } else {
        None
    }
}

/// Make the change to `links` a panel asked for, and tell the operator
/// what came of it. Returns whether the configured links changed, to be
/// saved; connecting and disconnecting change only what runs.
fn change_links(
    links: &mut LinkManager,
    request: LinkRequest,
    notices: &mut Notifications,
) -> bool {
    match request {
        LinkRequest::Save(config) => {
            let name = config.name.trim().to_string();
            match links.save(config) {
                Ok(saved) => {
                    // Keeping its id, and its connection unless the
                    // transport changed.
                    if let Saved::Reconnected(_) = saved {
                        notices.push(Notice::info(format!(
                            "Reconnecting {name} with the new settings"
                        )));
                    }
                    true
                }
                Err(error) => {
                    notices.push(Notice::warning("Could not save the link").detail(error));
                    false
                }
            }
        }
        LinkRequest::Connect(id) => {
            // The manager refuses a second supervisor on an active link.
            links.connect(&id);
            false
        }
        LinkRequest::Disconnect(id) => {
            links.disconnect(&id);
            false
        }
        LinkRequest::Delete(id) => links.remove(&id),
    }
}

/// Whether anything can show the video: the floating shell always can
/// (the map's corner, or the whole screen, with a tap), the tiled shell
/// while the Video pane is open.
fn video_shown(shell: Shell, layout: &Layout) -> bool {
    shell == Shell::Floating || layout.is_open(layout::PaneKind::Video)
}

mod floating;
#[cfg(test)]
mod gallery;
mod panels;
mod tiled;

#[cfg(test)]
mod tests {
    use super::{BackTarget, FullScreen, Layout, Shell, back_target, layout, video_shown};
    use iced::window::Mode;

    /// The tiled shell shows the video only in its pane; the floating shell
    /// shows it with the map, the pane or not.
    #[test]
    fn the_video_shows_in_its_pane_or_the_floating_shell() {
        let mut layout = Layout::new();
        assert!(!video_shown(Shell::Tiled, &layout));
        assert!(video_shown(Shell::Floating, &layout), "in the map's corner");

        layout.update(layout::Message::Toggle(
            layout::PaneKind::Video,
            layout::Slot::Left,
        ));
        assert!(video_shown(Shell::Tiled, &layout));

        layout.update(layout::Message::Toggle(
            layout::PaneKind::Video,
            layout::Slot::Left,
        ));
        assert!(!video_shown(Shell::Tiled, &layout), "closed again");
    }

    #[test]
    fn back_dismisses_a_confirmation_before_anything_else() {
        for (video, floating) in [(false, false), (false, true), (true, true)] {
            assert_eq!(
                back_target(false, true, video, floating),
                Some(BackTarget::Dismiss)
            );
        }
    }

    #[test]
    fn back_leaves_the_video_then_steps_the_floating_shell() {
        assert_eq!(
            back_target(false, false, true, true),
            Some(BackTarget::LeaveVideo)
        );
        assert_eq!(
            back_target(false, false, false, true),
            Some(BackTarget::StepBack)
        );
        assert_eq!(back_target(false, false, false, false), None, "tiled");
    }

    #[test]
    fn a_captured_escape_only_leaves_the_video() {
        assert_eq!(back_target(true, true, false, true), None);
        assert_eq!(
            back_target(true, true, true, true),
            Some(BackTarget::LeaveVideo)
        );
    }

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

    /// What the application makes of the panels' changes to the links.
    mod links {
        use std::net::{Ipv4Addr, SocketAddrV4};

        use groundhog_logic::{
            Backend, BackendOptions,
            link::{LinkBuild, LinkConfig, LinkId},
        };

        use super::super::{LinkRequest, Notifications, change_links};

        fn config(name: &str) -> LinkConfig {
            LinkConfig {
                name: name.to_string(),
                auto_connect: false,
                high_latency: false,
                auto_reconnect: true,
                build: LinkBuild::Udp {
                    // Any free port: a test may connect it.
                    sock_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
                    peer: None,
                },
            }
        }

        fn titles(notices: &Notifications) -> Vec<&str> {
            notices
                .history()
                .map(|notice| notice.title.as_str())
                .collect()
        }

        #[tokio::test]
        async fn saving_and_deleting_change_the_saved_links() {
            let (mut backend, _events) = Backend::start(BackendOptions::default());
            let mut notices = Notifications::default();

            let saved = change_links(
                backend.links_mut(),
                LinkRequest::Save(config("A")),
                &mut notices,
            );
            assert!(saved);
            let id = backend.links().entries().next().expect("saved").0.clone();

            let unknown = LinkRequest::Delete(LinkId::new("does-not-exist"));
            assert!(!change_links(backend.links_mut(), unknown, &mut notices));
            assert_eq!(backend.links().configs().len(), 1);

            let deleted = change_links(backend.links_mut(), LinkRequest::Delete(id), &mut notices);
            assert!(deleted);
            assert!(backend.links().configs().is_empty());
            assert!(titles(&notices).is_empty());
        }

        #[tokio::test]
        async fn a_link_without_a_name_is_not_saved() {
            let (mut backend, _events) = Backend::start(BackendOptions::default());
            let mut notices = Notifications::default();

            let saved = change_links(
                backend.links_mut(),
                LinkRequest::Save(config("  ")),
                &mut notices,
            );

            assert!(!saved);
            assert!(backend.links().configs().is_empty());
            assert_eq!(titles(&notices), ["Could not save the link"]);
        }

        /// Connecting and disconnecting change what runs, not what is
        /// saved; a new transport for a running link reconnects it, and
        /// says so.
        #[tokio::test]
        async fn connecting_runs_a_link_without_changing_what_is_saved() {
            let (mut backend, _events) = Backend::start(BackendOptions::default());
            let mut notices = Notifications::default();
            let links = backend.links_mut();
            change_links(links, LinkRequest::Save(config("A")), &mut notices);
            let id = links.entries().next().expect("saved").0.clone();

            assert!(!change_links(
                links,
                LinkRequest::Connect(id.clone()),
                &mut notices
            ));
            assert!(links.status(&id).is_active());

            let high_latency = LinkConfig {
                high_latency: true,
                ..config("A")
            };
            assert!(change_links(
                links,
                LinkRequest::Save(high_latency),
                &mut notices
            ));
            assert!(links.status(&id).is_active(), "reconnected");
            assert_eq!(titles(&notices), ["Reconnecting A with the new settings"]);

            assert!(!change_links(
                links,
                LinkRequest::Disconnect(id.clone()),
                &mut notices
            ));
            assert!(!links.status(&id).is_active());
        }
    }
}
