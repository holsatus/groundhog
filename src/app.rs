//! The application root: composition, message routing, and policy.
//!
//! The root wires the layers together at boot, routes messages to features,
//! applies the [`Effect`]s features request, and decides which fleet and link
//! events become operator notices. It holds the latest fleet snapshot, which
//! the fleet runtime publishes from off the UI thread.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use iced::{Element, Length, Subscription, Task, Theme, Widget};
use mavio::default_dialect::enums::MavSeverity;

use crate::{
    config::{self, ConfigWriter, Configuration},
    domain::{
        SystemId,
        calibration::Phase,
        fleet::{FleetEvent, FleetSnapshot},
        vehicle::{Liveness, status},
    },
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
        vehicles::{self, VehiclePanel, vehicle_name},
    },
    layout::{self, Layout},
    map::{self, LayerSet, Map},
    mavlink::{self, MavlinkRouter},
    services::{
        FleetHandle, FleetUpdate, LinkEvent, LinkId, LinkManager, LinkStatus, RuntimeOptions,
        TlogRecorder, runtime,
    },
    style,
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
    router: MavlinkRouter,
    services: FleetHandle,
    /// The fleet as last published by the runtime.
    fleet: Arc<FleetSnapshot>,
    /// The vehicle the UI shows.
    selected: Option<SystemId>,
    link_manager: LinkManager,
    /// Whether each link was last known connected, to word link notices.
    link_connected: BTreeMap<LinkId, bool>,
    /// Vehicles announced as lost, so only their recovery is announced.
    lost: std::collections::BTreeSet<SystemId>,
    recorder: Option<TlogRecorder>,
    notices: Notifications,
    confirmation: Option<Confirmation<Message>>,
    features: Features,
    link_panel: LinkPanel,
    log_panel: LogPanel,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The fleet runtime published.
    Fleet(FleetUpdate),
    /// A link's supervisor changed status.
    Link(LinkEvent),
    /// A configuration write finished.
    ConfigurationSaved(Result<(), String>),

    Layout(layout::Message),
    Map(map::Message),
    Vehicles(vehicles::Message),
    Mission(mission::Message),
    Parameters(parameter_panel::Message),
    Calibration(calibration::Message),
    Links(link_panel::Message),
    Logs(log_panel::Message),
    Settings(settings_panel::Message),
    Notifications(notifications::Message),
    Confirm(Answer),

    /// Debounce tick that flushes a pending layout save.
    LayoutSaveTick,
    /// Refreshes time-dependent views (toast expiry, link countdowns).
    Tick(Instant),
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

        // The runtime and supervisors are spawned here: boot runs inside the
        // application's async runtime.
        let (router, events) = MavlinkRouter::new(mavlink::DEFAULT_GCS_ID);
        let (services, updates) = runtime::spawn(router.clone(), events, RuntimeOptions::default());
        let (mut link_manager, auto_connect, link_events) =
            LinkManager::new(&router, configuration.links.clone());
        for id in &auto_connect {
            link_manager.connect(id);
        }

        let (config_writer, saved) = ConfigWriter::spawn();

        let layout = configuration
            .layout
            .clone()
            .map_or_else(Layout::new, Layout::from_configuration);

        let mut application = Application {
            layout,
            map: Map::new(),
            configuration,
            config_writer,
            router,
            services,
            fleet: Arc::default(),
            selected: None,
            link_manager,
            link_connected: BTreeMap::new(),
            lost: Default::default(),
            recorder: None,
            notices,
            confirmation: None,
            features: Features::default(),
            link_panel: LinkPanel::new(),
            log_panel: LogPanel::new(),
        };

        if application.configuration.settings.record_telemetry {
            application.start_recording();
        }

        let saved = futures::stream::unfold(saved, |mut saved| async move {
            saved.recv().await.map(|outcome| (outcome, saved))
        });

        let tasks = Task::batch([
            Task::stream(updates.into_stream()).map(Message::Fleet),
            Task::stream(link_events.into_stream()).map(Message::Link),
            Task::stream(saved).map(Message::ConfigurationSaved),
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

        match TlogRecorder::start(&self.router, &directory) {
            Ok(recorder) => self.recorder = Some(recorder),
            Err(error) => self
                .notices
                .push(Notice::error("Could not start telemetry recording").detail(error)),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
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
            Message::Layout(message) => {
                if self.layout.update(message) {
                    self.configuration.layout = Some(self.layout.configuration());
                }
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
                        .update(message, &mut self.link_manager, &mut self.notices);

                if let Some(link_panel::Event::LinksChanged(links)) = event {
                    self.configuration.links = links;
                    self.save_configuration();
                }
                task.map(Message::Links)
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
            &self.services,
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
                self.map.center_on(position);
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
            self.map.follow_to(position.pos);
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
        let name = |system| vehicle_name(&self.fleet, system);

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
            FleetEvent::LivenessChanged(system, liveness) => match liveness {
                Liveness::Stale => return,
                Liveness::Lost => {
                    self.lost.insert(system);
                    Notice::error(format!("Lost contact with {}", name(system))).vehicle(system)
                }
                Liveness::Active => {
                    if !self.lost.remove(&system) {
                        return;
                    }
                    Notice::success(format!("Contact with {} restored", name(system)))
                        .vehicle(system)
                }
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
        let was_connected = self.link_connected.get(&event.id).copied().unwrap_or(false);
        let connected = event.status.is_connected();
        self.link_connected.insert(event.id.clone(), connected);

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
            settings_panel::Message::ScaleFactor(scale) => settings.scale_factor = scale,
            settings_panel::Message::DefaultAltitude(altitude) => {
                settings.default_waypoint_altitude_m = altitude;
            }
            settings_panel::Message::RecordTelemetry(record) => {
                settings.record_telemetry = record;
                if record && self.recorder.is_none() {
                    self.start_recording();
                } else if !record && let Some(recorder) = self.recorder.take() {
                    recorder.stop();
                }
            }
        }

        if self.configuration.settings != before {
            self.save_configuration();
        }
    }

    pub fn view(&self) -> impl Widget<Message> + '_ {
        let base: Element<'_, Message> = iced::widget::row![
            self.view_rail(),
            iced::widget::rule::vertical(1.0),
            self.view_panes()
        ]
        .spacing(0)
        .boxed();

        let toasts = iced::widget::container(self.notices.view().map(Message::Notifications))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(16);

        let with_toasts: Element<'_, Message> = iced::widget::stack![base, toasts].boxed();

        iced::widget::container(confirm::view(
            with_toasts,
            self.confirmation.as_ref(),
            Message::Confirm,
        ))
    }

    fn view_rail(&self) -> Element<'_, Message> {
        let mut buttons = iced::widget::Column::new().spacing(4);

        for panel in layout::Panel::DOCKABLE {
            if self.layout.is_open(panel) {
                continue;
            }

            let button = iced::widget::button(
                iced::widget::container(iced::widget::text(panel.badge()))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fixed(36.0))
            .on_press(Message::Layout(layout::Message::Open(panel)));

            buttons = buttons.push(button);
        }

        iced::widget::container(buttons)
            .width(Length::Fixed(44.0))
            .height(Length::Fill)
            .padding(4)
            .style(style::rail)
            .boxed()
    }

    fn view_panes(&self) -> impl Widget<Message> + '_ {
        use crate::widget::pane_grid::{self, PaneGrid};

        let panes = PaneGrid::new(self.layout.panes(), |pane, panel, is_maximized| {
            // The map paints opaque tiles across its whole pane, which a rounded
            // container cannot clip; it draws its own frame instead (see
            // `map::Map::view`). Every other pane is a rounded card split by a
            // rule.
            let body: Element<'_, Message> = if panel.is_fixed() {
                self.pane_body(*panel)
            } else {
                iced::widget::column![
                    iced::widget::rule::horizontal(style::PANE_BORDER_WIDTH).style(style::divider),
                    self.pane_body(*panel),
                ]
                .height(Length::Fill)
                .boxed()
            };

            let mut content = pane_grid::Content::new(body);

            if !panel.is_fixed() {
                content = content.style(style::pane);
            }

            if panel.is_fixed() {
                // Zero-height title bar: keeps the `Content` type uniform while
                // leaving nothing to grab, so the map cannot be moved or closed.
                let empty = iced::widget::Space::new()
                    .height(Length::Fixed(0.0))
                    .boxed();
                content.title_bar(pane_grid::TitleBar::new(empty).padding(0))
            } else {
                let focused = self.layout.is_focused(pane);
                let title = iced::widget::container(iced::widget::text(panel.title()))
                    .padding(5.0)
                    .boxed();

                let maximize = if is_maximized {
                    iced::widget::button(
                        iced::widget::container(iced::widget::text("v")).center(20),
                    )
                    .padding(2)
                    .on_press(Message::Layout(layout::Message::Restore))
                } else {
                    iced::widget::button(
                        iced::widget::container(iced::widget::text("^")).center(20),
                    )
                    .padding(2)
                    .on_press(Message::Layout(layout::Message::Maximize(pane)))
                };

                let close = iced::widget::button(
                    iced::widget::container(iced::widget::text("x")).center(20),
                )
                .style(iced::widget::button::danger)
                .padding(2)
                .on_press(Message::Layout(layout::Message::Close(pane)));

                let controls =
                    iced::widget::container(iced::widget::row![maximize, close].spacing(4))
                        .center_y(32);

                let title_bar = pane_grid::TitleBar::new(title)
                    .controls(pane_grid::Controls::new(controls))
                    .padding(4)
                    .style(move |theme| {
                        if focused {
                            style::title_bar_focused(theme)
                        } else {
                            style::title_bar(theme)
                        }
                    });

                content.title_bar(title_bar)
            }
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .spacing(5)
        .on_click(|pane| Message::Layout(layout::Message::Clicked(pane)))
        .on_drag(|event| Message::Layout(layout::Message::Dragged(event)))
        .on_resize(7, |event| Message::Layout(layout::Message::Resized(event)));

        iced::widget::container(panes)
            .padding(5.0)
            .style(move |theme: &Theme| {
                let palette = theme.palette();

                iced::widget::container::Style {
                    background: Some(palette.background.base.color.into()),
                    ..iced::widget::container::Style::default()
                }
            })
    }

    fn pane_body(&self, panel: layout::Panel) -> Element<'_, Message> {
        let theme = self.theme();
        let fleet: &FleetSnapshot = &self.fleet;

        match panel {
            layout::Panel::Map => {
                let context = MapContext {
                    fleet,
                    selected: self.selected,
                    palette: *theme.palette(),
                };

                let mut layers = LayerSet::new();
                layers.plugin(&self.features.vehicles, context, Message::Vehicles);
                layers.plugin(&self.features.mission, context, Message::Mission);

                self.map.view(layers, Message::Map, &theme)
            }
            layout::Panel::Vehicles => self
                .features
                .vehicles
                .view(fleet, self.selected, self.map.is_following(), &theme)
                .map(Message::Vehicles)
                .boxed(),
            layout::Panel::Parameters => self
                .features
                .parameters
                .view(fleet, self.selected, &theme)
                .map(Message::Parameters)
                .boxed(),
            layout::Panel::Calibration => self
                .features
                .calibration
                .view(fleet, self.selected, &theme)
                .map(Message::Calibration)
                .boxed(),
            layout::Panel::Mission => self
                .features
                .mission
                .view(fleet, self.selected, &theme)
                .map(Message::Mission)
                .boxed(),
            layout::Panel::Links => iced::widget::column![
                self.link_panel.view_editor().map(Message::Links),
                self.link_panel
                    .view_status(&self.link_manager)
                    .map(Message::Links),
            ]
            .spacing(10.0)
            .boxed(),
            layout::Panel::Logs => self
                .log_panel
                .view(fleet, self.selected, &self.notices, &theme)
                .map(Message::Logs)
                .boxed(),
            layout::Panel::Settings => settings_panel::view(
                &self.configuration.settings,
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
            .link_manager
            .entries()
            .any(|(_, _, status)| status.is_active());
        let refresh = if self.notices.has_toasts() || active_links {
            iced::time::every(Duration::from_secs(1)).map(Message::Tick)
        } else {
            Subscription::none()
        };

        Subscription::batch([layout_tick, refresh])
    }
}
