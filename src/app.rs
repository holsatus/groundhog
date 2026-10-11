use std::{
    path::Path,
    time::{Duration, Instant},
};

use iced::futures::{SinkExt, Stream, StreamExt};
use iced::{Element, Length, Subscription, Task, Theme, Widget};
use mavio::{DefaultDialect, error::SpecError};

use crate::{
    config,
    domain::fleet::{Fleet, FleetEvent},
    domain::vehicle::Liveness,
    features::link_panel::{self, LinkPanel},
    features::log_panel::{self, LogPanel},
    features::map::{self, Map},
    features::mission_panel::{self, MissionPanel},
    features::parameter_panel::{self, ParameterPanel},
    features::settings_panel,
    features::vehicle_panel,
    foundation::{DEFAULT_GCS_ID, SystemId},
    layout::{self, Layout},
    mav,
    mav::MavlinkRouter,
    services::FleetServices,
    style,
};

pub struct Application {
    pub(crate) layout: Layout,
    pub(crate) map: Map,
    pub(crate) link_panel: LinkPanel,
    pub(crate) log_panel: LogPanel,
    pub(crate) mission_panel: MissionPanel,
    pub(crate) parameter_panel: ParameterPanel,
    pub(crate) router: MavlinkRouter,
    /// Handle to the vehicle operations the feature panels drive.
    pub(crate) services: FleetServices,
    pub(crate) configuration: config::Configuration,
    /// Cached iced theme, kept in sync with `configuration.settings.theme`.
    ///
    /// Regenerating the theme runs real palette colour math and allocates, so
    /// it is built once at boot and whenever the setting changes rather than
    /// on every render.
    pub(crate) theme: Theme,
    pub(crate) fleet: Fleet,
    /// Set when the layout changed and needs to be persisted (debounced).
    pub(crate) layout_dirty: bool,
    /// Bumped on every layout change so the debounce timer is restarted.
    pub(crate) layout_revision: u64,
}

#[derive(Debug, Clone)]
pub enum Message {
    Layout(layout::Message),
    Map(map::Message),
    Link(link_panel::Message),
    Logs(log_panel::Message),
    /// A batch of MAVLink events, collected at most once per telemetry frame.
    MavEvents(Vec<mav::Event>),
    Mission(mission_panel::Message),
    Parameter(parameter_panel::Message),
    Vehicles(vehicle_panel::Message),
    Settings(settings_panel::Message),

    SaveConfigurationToFile,

    /// Debounce tick that flushes a pending layout save.
    Tick,

    /// Periodic liveness check for every vehicle in the fleet.
    FleetTick(Instant),

    /// Result of an asynchronous configuration write.
    ConfigurationSaved(Result<(), String>),
}

impl From<parameter_panel::Message> for Message {
    fn from(value: parameter_panel::Message) -> Self {
        Message::Parameter(value)
    }
}

impl From<mission_panel::Message> for Message {
    fn from(value: mission_panel::Message) -> Self {
        Message::Mission(value)
    }
}

impl From<settings_panel::Message> for Message {
    fn from(value: settings_panel::Message) -> Self {
        Message::Settings(value)
    }
}

/// How often telemetry is allowed to reach `update` and rebuild the view/map.
///
/// This caps the redraw cadence, not the data rate: the router still folds every
/// inbound frame into vehicle state, but `update` — and therefore the view and
/// the map overlay, which re-projects and re-strokes every trail point — runs at
/// most this often. 30 Hz is smooth for a GCS HUD and roughly halves overlay
/// rasterisation work compared with 60 Hz. Map panning and zooming are
/// input-driven inside `slippery` and are not affected by this throttle.
const TELEMETRY_FRAME: Duration = Duration::from_millis(33);

/// Idle time after the last layout change before the layout is persisted.
const LAYOUT_SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

/// Cap the rate at which inbound MAVLink events reach `update`.
///
/// The router emits one event per received frame. Without this, a busy link
/// would rebuild the whole view once per frame; instead events are collected
/// and delivered in order in a single batch at most once per `frame`.
fn coalesce(
    events: impl Stream<Item = mav::Event> + Send + 'static,
    frame: Duration,
) -> impl Stream<Item = Vec<mav::Event>> + Send + 'static {
    iced::stream::channel(1, async move |mut output| {
        let mut events = Box::pin(events);
        let mut tick = tokio::time::interval(frame);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await; // consume the immediate first tick

        let mut batch = Vec::new();
        loop {
            tokio::select! {
                biased;
                _ = tick.tick() => {
                    if !batch.is_empty()
                        && output.send(std::mem::take(&mut batch)).await.is_err()
                    {
                        break;
                    }
                }
                event = events.next() => match event {
                    Some(event) => batch.push(event),
                    None => break,
                },
            }
        }
    })
}

impl Application {
    pub fn boot() -> (Self, Task<Message>) {
        let config = config::Configuration::initialize().unwrap_or_else(|e| {
            log::error!("Unable to initialize configuration file: {}", e);
            config::Configuration::default()
        });

        let (link_panel, auto_connect) = LinkPanel::from_config(config.links.clone());

        let (client, events) = mav::MavlinkRouter::new(DEFAULT_GCS_ID);
        let services = FleetServices::new(&client);
        let events = Task::stream(coalesce(events, TELEMETRY_FRAME).map(Message::MavEvents));

        let auto_connect = Task::batch(
            auto_connect
                .into_iter()
                .map(|id| Task::done(Message::Link(link_panel::Message::Connect(id)))),
        );

        let layout = config
            .layout
            .clone()
            .map_or_else(Layout::new, Layout::from_configuration);

        let application = Application {
            layout,
            map: Map::new(),
            link_panel,
            log_panel: LogPanel::new(),
            router: client,
            services,
            configuration: config.clone(),
            theme: config.settings.theme.theme(),
            fleet: Fleet::default(),
            mission_panel: MissionPanel::new(),
            parameter_panel: ParameterPanel::new(),
            layout_dirty: false,
            layout_revision: 0,
        };

        (application, Task::batch([events, auto_connect]))
    }

    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    pub fn title(&self) -> String {
        "Holsatus Groundhog".to_string()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        self.maybe_update(message).unwrap_or_default()
    }

    /// Write the current configuration to disk off the UI thread.
    fn save_configuration(&self) -> Task<Message> {
        let configuration = self.configuration.clone();
        iced_runtime::task::blocking(move |mut sender| {
            let result = configuration
                .write_to_file()
                .map_err(|error| error.to_string());
            let _ = sender.try_send(Message::ConfigurationSaved(result));
        })
    }

    fn maybe_update(&mut self, message: Message) -> Option<Task<Message>> {
        match message {
            Message::SaveConfigurationToFile => return Some(self.save_configuration()),
            Message::ConfigurationSaved(result) => {
                if let Err(error) = result {
                    log::error!("Unable to save configuration file: {error}");
                }
            }
            Message::Layout(message) => self.layout_message_update(message),
            Message::Map(message) => return Some(self.map_message_update(message)),
            Message::Link(message) => return Some(self.link_message_update(message)),
            Message::Logs(message) => self.log_panel.update(message),
            Message::MavEvents(events) => return Some(self.handle_mav_events(events)),
            Message::Mission(message) => return Some(self.mission_message_update(message)),
            Message::Parameter(message) => return Some(self.parameter_message_update(message)),
            Message::Vehicles(message) => return Some(self.vehicle_message_update(message)),
            Message::Settings(message) => return Some(self.settings_message_update(message)),
            Message::Tick => {
                if self.layout_dirty {
                    self.layout_dirty = false;
                    return Some(self.save_configuration());
                }
            }
            Message::FleetTick(now) => {
                let events = self.fleet.tick(now);
                return Some(Task::batch(
                    events
                        .into_iter()
                        .map(|event| self.handle_fleet_event(event))
                        .collect::<Vec<_>>(),
                ));
            }
        }

        None
    }

    fn layout_message_update(&mut self, message: layout::Message) {
        let changed = message.changes_layout();
        self.layout.update(message);
        if changed {
            self.configuration.layout = self.layout.configuration();
            self.layout_revision = self.layout_revision.wrapping_add(1);
            self.layout_dirty = true;
        }
    }

    fn vehicle_message_update(&mut self, message: vehicle_panel::Message) -> Task<Message> {
        match message {
            vehicle_panel::Message::SetPrimary(system) => {
                self.fleet.set_primary(system);
                Task::none()
            }
            vehicle_panel::Message::Follow(follow) => self
                .map
                .update(map::Message::Follow(follow))
                .map(Message::Map),
        }
    }

    pub(crate) fn set_file_picker_path_config(&mut self, path: &Path) -> Task<Message> {
        let Some(file_picker_path) = path.is_dir().then_some(path).or(path.parent()) else {
            return Task::none();
        };

        self.configuration.file_picker_path = Some(file_picker_path.to_path_buf());
        self.save_configuration()
    }

    fn map_message_update(&mut self, message: map::Message) -> Task<Message> {
        let map_task = self.map.update(message.clone()).map(Message::Map);

        let mission_task = match message {
            map::Message::WaypointPressed(index) => {
                self.mission_message_update(mission_panel::Message::BeginDrag(index))
            }
            map::Message::WaypointDragged(index, position) => {
                self.mission_message_update(mission_panel::Message::MoveWaypointTo(index, position))
            }
            map::Message::WaypointReleased => {
                self.mission_message_update(mission_panel::Message::EndDrag)
            }
            map::Message::PlaceAt(position) => {
                self.mission_message_update(mission_panel::Message::AddWaypointAt(position))
            }
            _ => Task::none(),
        };

        Task::batch([map_task, mission_task])
    }

    fn mission_message_update(&mut self, message: mission_panel::Message) -> Task<Message> {
        let (task, event) = {
            let mut ctx = mission_panel::Context {
                services: &self.services,
                fleet: &mut self.fleet,
                file_picker_path: self.configuration.file_picker_path.as_deref(),
            };

            self.mission_panel.update(message, &mut ctx)
        };

        let task = task.map(Message::Mission);

        match event {
            Some(mission_panel::Event::SetFilePickerPath(path)) => {
                Task::batch([task, self.set_file_picker_path_config(&path)])
            }
            None => task,
        }
    }

    fn parameter_message_update(&mut self, message: parameter_panel::Message) -> Task<Message> {
        let (task, event) = {
            let mut ctx = parameter_panel::Context {
                services: &self.services,
                fleet: &mut self.fleet,
                file_picker_path: self.configuration.file_picker_path.as_deref(),
            };
            self.parameter_panel.update(message, &mut ctx)
        };

        let task = task.map(Message::Parameter);

        match event {
            Some(parameter_panel::Event::SetFilePickerPath(path)) => {
                Task::batch([task, self.set_file_picker_path_config(&path)])
            }
            None => task,
        }
    }

    /// Fetch basic information and the parameter list from a newly detected vehicle.
    fn initialize_vehicle(&self, system: SystemId) -> Task<Message> {
        let Some(target) = self.fleet.vehicle(system).map(|vehicle| vehicle.target()) else {
            return Task::none();
        };
        let client = self.services.vehicle(target);
        let request = Task::perform(
            async move {
                if let Err(error) = client.component_information().await {
                    log::warn!("No COMPONENT_INFORMATION_BASIC from {target:?}: {error}");
                }

                // This requests AUTOPILOT_VERSION to get capabilities for param encoding
                client.fetch_parameters(true).await
            },
            move |result| parameter_panel::Message::ListReloadResult(target, result).into(),
        );

        // Mark the implicit list download as running so the reload button is
        // disabled while the automatic initialisation is in flight.
        Task::batch([
            Task::done(Message::Parameter(
                parameter_panel::Message::ListReloadStarted(target),
            )),
            request,
        ])
    }

    fn link_message_update(&mut self, message: link_panel::Message) -> Task<Message> {
        let (task, event) = self.link_panel.update(message, &self.router);
        let task = task.map(Message::Link);

        match event {
            Some(link_panel::Event::LinksChanged(links)) => {
                self.configuration.links = links;
                Task::batch([task, Task::done(Message::SaveConfigurationToFile)])
            }
            None => task,
        }
    }

    /// Apply an application setting and persist it. The cached theme is
    /// refreshed here alongside the setting so it takes effect immediately.
    fn settings_message_update(&mut self, message: settings_panel::Message) -> Task<Message> {
        match message {
            settings_panel::Message::SetTheme(mode) => {
                if self.configuration.settings.theme != mode {
                    self.configuration.settings.theme = mode;
                    self.theme = mode.theme();
                    return Task::done(Message::SaveConfigurationToFile);
                }
            }
        }

        Task::none()
    }

    /// Feed a batch of MAVLink events into the fleet, in order, and turn what
    /// the fleet reports back into application effects.
    fn handle_mav_events(&mut self, events: Vec<mav::Event>) -> Task<Message> {
        let mut tasks = Vec::new();

        for event in events {
            match event {
                mav::Event::Message(inbound) => {
                    let source = inbound.source_mav;

                    let decoded = match inbound.frame.decode::<DefaultDialect>() {
                        Ok(message) => Some(message),
                        // Downgrade these to trace to avoid spamming the logs
                        Err(mavio::Error::Spec(SpecError::NotInDialect(id))) => {
                            log::trace!("mavlink message id {id} is not in default dialect");
                            None
                        }
                        Err(error) => {
                            log::trace!("could not decode mavlink frame: {error}");
                            None
                        }
                    };

                    for fleet_event in self.fleet.observe(source, inbound.received_at, decoded) {
                        tasks.push(self.handle_fleet_event(fleet_event));
                    }
                }
                mav::Event::LinkClosed(link_id) => {
                    log::info!("Link {link_id:?} closed");
                    tasks.push(self.link_message_update(link_panel::Message::Closed(link_id)));
                }
            }
        }

        Task::batch(tasks)
    }

    /// Translate one [`FleetEvent`] into the application's effects. Both the
    /// telemetry path and the periodic [`Message::FleetTick`] use this, so an
    /// event means the same thing regardless of which produced it.
    fn handle_fleet_event(&self, event: FleetEvent) -> Task<Message> {
        match event {
            FleetEvent::Registered(system) => {
                log::info!("New vehicle detected: {system:?}");
                self.initialize_vehicle(system)
            }
            FleetEvent::LivenessChanged(system, liveness) => {
                log_liveness(system, liveness);
                Task::none()
            }
            FleetEvent::PrimaryMoved(position) => {
                if self.map.follow {
                    Task::done(Message::Map(map::Message::SetViewPosition(position)))
                } else {
                    Task::none()
                }
            }
        }
    }

    pub fn view(&self) -> impl Widget<Message> + '_ {
        iced::widget::row![
            self.view_rail(),
            iced::widget::rule::vertical(1.0),
            self.view_panes()
        ]
        .spacing(0)
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

    fn view_panes(&self) -> Element<'_, Message> {
        use crate::widget::pane_grid::{self, PaneGrid};

        PaneGrid::new(self.layout.panes(), |pane, panel, is_maximized| {
            let content = pane_grid::Content::new(self.pane_body(*panel));

            if panel.is_fixed() {
                // Zero-height title bar: keeps the `Content` type uniform while
                // leaving nothing to grab, so the map cannot be moved or closed.
                let empty = iced::widget::Space::new()
                    .height(Length::Fixed(0.0))
                    .boxed();
                content.title_bar(pane_grid::TitleBar::new(empty).padding(0))
            } else {
                let focused = self.layout.is_focused(pane);
                let title: Element<'_, Message> = iced::widget::text(panel.title()).boxed();

                let maximize = if is_maximized {
                    iced::widget::button(iced::widget::text("v"))
                        .padding(2)
                        .on_press(Message::Layout(layout::Message::Restore))
                } else {
                    iced::widget::button(iced::widget::text("^"))
                        .padding(2)
                        .on_press(Message::Layout(layout::Message::Maximize(pane)))
                };

                let close = iced::widget::button(iced::widget::text("x"))
                    .style(iced::widget::button::danger)
                    .padding(2)
                    .on_press(Message::Layout(layout::Message::Close(pane)));

                let controls = iced::widget::row![maximize, close].spacing(4);

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

                content.title_bar(title_bar).style(style::pane)
            }
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .spacing(1)
        .on_click(|pane| Message::Layout(layout::Message::Clicked(pane)))
        .on_drag(|event| Message::Layout(layout::Message::Dragged(event)))
        .on_resize(9, |event| Message::Layout(layout::Message::Resized(event)))
        .boxed()
    }

    fn pane_body(&self, panel: layout::Panel) -> Element<'_, Message> {
        match panel {
            layout::Panel::Map => {
                let primary = self.fleet.primary();
                let interaction = map::Interaction {
                    placing: self.mission_panel.is_placing(primary),
                    dragging: self.mission_panel.dragging(primary),
                };
                self.map
                    .view(self.fleet.vehicles(), primary, interaction)
                    .map(Message::Map)
                    .boxed()
            }
            layout::Panel::Vehicles => {
                let theme = &self.theme;
                vehicle_panel::view(
                    self.fleet.vehicles(),
                    self.fleet.primary(),
                    self.map.follow,
                    theme,
                )
                .map(Message::Vehicles)
                .boxed()
            }
            layout::Panel::Parameters => {
                let theme = &self.theme;
                self.parameter_panel
                    .view(self.fleet.vehicles(), self.fleet.primary(), theme)
                    .map(Message::Parameter)
                    .boxed()
            }
            layout::Panel::Mission => self
                .mission_panel
                .view(self.fleet.vehicles(), self.fleet.primary(), &self.theme)
                .map(Message::Mission)
                .boxed(),
            layout::Panel::Links => iced::widget::column![
                self.link_panel.view_editor().map(Message::Link),
                self.link_panel.view_status().map(Message::Link),
            ]
            .spacing(10.0)
            .boxed(),
            layout::Panel::Logs => {
                let theme = &self.theme;
                self.log_panel
                    .view(self.fleet.vehicles(), self.fleet.primary(), theme)
                    .map(Message::Logs)
                    .boxed()
            }
            layout::Panel::Settings => settings_panel::view(&self.configuration.settings)
                .map(Message::Settings)
                .boxed(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let fleet_tick = iced::time::every(Duration::from_secs(1)).map(Message::FleetTick);

        // Trailing debounce for layout persistence: the timer is restarted on
        // every layout change (the revision is part of the subscription
        // identity), so the write happens `LAYOUT_SAVE_DEBOUNCE` after the last
        // change rather than repeatedly throughout a drag/resize.
        let layout_tick = if self.layout_dirty {
            iced::time::every(LAYOUT_SAVE_DEBOUNCE)
                .with(self.layout_revision)
                .map(|_| Message::Tick)
        } else {
            Subscription::none()
        };

        Subscription::batch([fleet_tick, layout_tick])
    }
}

/// Log a liveness transition, louder the further a vehicle drifts.
fn log_liveness(system: SystemId, liveness: Liveness) {
    match liveness {
        Liveness::Lost => log::warn!("Vehicle {system:?} lost"),
        Liveness::Stale => log::info!("Vehicle {system:?} is stale"),
        Liveness::Active => log::info!("Vehicle {system:?} is active"),
    }
}
