use std::{
    path::Path,
    time::{Duration, Instant},
};

use iced::futures::StreamExt;
use iced::{Element, Length, Subscription, Task, Theme, Widget};

use crate::{
    config,
    domain::SystemId,
    domain::fleet::{Fleet, FleetEvent},
    domain::vehicle::Liveness,
    features::link_panel::{self, LinkPanel},
    features::log_panel::{self, LogPanel},
    features::map::{self, Map},
    features::mission_panel::{self, MissionPanel},
    features::parameter_panel::{self, ParameterPanel},
    features::settings_panel,
    features::vehicle_panel,
    layout::{self, Layout},
    mavlink::{self, telemetry::Telemetry},
    services::{FleetServices, LinkManager},
    style,
};

pub struct Application {
    pub(crate) layout: Layout,
    pub(crate) map: Map,
    pub(crate) link_panel: LinkPanel,
    pub(crate) log_panel: LogPanel,
    pub(crate) mission_panel: MissionPanel,
    pub(crate) parameter_panel: ParameterPanel,
    pub(crate) link_manager: LinkManager,
    pub(crate) services: FleetServices,
    pub(crate) configuration: config::Configuration,
    pub(crate) fleet: Fleet,
}

#[derive(Debug, Clone)]
pub enum Message {
    Layout(layout::Message),
    Map(map::Message),
    Link(link_panel::Message),
    Logs(log_panel::Message),
    /// A batch of MAVLink events, collected at most once per telemetry frame.
    MavEvents(Vec<Telemetry>),
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
/// This caps the redraw cadence, not the data rate: the router still dispatches
/// every frame, the coalescing task only decodes off the UI thread, and
/// `handle_mav_events` folds each batch into vehicle state inside `update`.
/// So `update` — and therefore the view and the map overlay, which re-project
/// and re-stroke every trail point — runs at most this often. 30 Hz is smooth
/// for a GCS HUD and roughly halves overlay rasterisation work compared with
/// 60 Hz. Map panning and zooming are input-driven inside `slippery` and are
/// not affected by this throttle.
const TELEMETRY_FRAME: Duration = Duration::from_millis(33);

/// Idle time after the last layout change before the layout is persisted.
const LAYOUT_SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

impl Application {
    pub fn boot() -> (Self, Task<Message>) {
        let config = config::Configuration::initialize().unwrap_or_else(|e| {
            log::error!("Unable to initialize configuration file: {}", e);
            config::Configuration::default()
        });

        let (client, events) = mavlink::MavlinkRouter::new(mavlink::DEFAULT_GCS_ID);
        let services = FleetServices::new(&client);
        let (link_manager, auto_connect) = LinkManager::from_configs(&client, config.links.clone());
        let events = Task::stream(
            mavlink::telemetry::coalesce(events, TELEMETRY_FRAME).map(Message::MavEvents),
        );

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
            link_panel: LinkPanel::new(),
            log_panel: LogPanel::new(),
            link_manager,
            services,
            configuration: config.clone(),
            fleet: Fleet::default(),
            mission_panel: MissionPanel::new(),
            parameter_panel: ParameterPanel::new(),
        };

        (application, Task::batch([events, auto_connect]))
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

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SaveConfigurationToFile => self.save_configuration(),
            Message::ConfigurationSaved(result) => {
                if let Err(error) = result {
                    log::error!("Unable to save configuration file: {error}");
                }
                Task::none()
            }
            Message::Layout(message) => self.layout_message_update(message),
            Message::Map(message) => self.map_message_update(message),
            Message::Link(message) => self.link_message_update(message),
            Message::Logs(message) => self.log_message_update(message),
            Message::MavEvents(events) => self.handle_mav_events(events),
            Message::Mission(message) => self.mission_message_update(message),
            Message::Parameter(message) => self.parameter_message_update(message),
            Message::Vehicles(message) => self.vehicle_message_update(message),
            Message::Settings(message) => self.settings_message_update(message),
            Message::Tick => {
                if self.layout.is_dirty() {
                    self.layout.clear_dirty();
                    self.save_configuration()
                } else {
                    Task::none()
                }
            }
            Message::FleetTick(now) => {
                let events = self.fleet.tick(now);
                Task::batch(
                    events
                        .into_iter()
                        .map(|event| self.handle_fleet_event(event))
                        .collect::<Vec<_>>(),
                )
            }
        }
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

    /// Apply a layout change, snapshotting the arrangement when it changed.
    ///
    /// `Layout::update` maintains the dirty flag and revision; the root owns
    /// only the snapshot and the debounce policy (see [`Message::Tick`] and
    /// [`Application::subscription`]).
    fn layout_message_update(&mut self, message: layout::Message) -> Task<Message> {
        if self.layout.update(message) {
            self.configuration.layout = Some(self.layout.configuration());
        }

        Task::none()
    }

    fn log_message_update(&mut self, message: log_panel::Message) -> Task<Message> {
        self.log_panel.update(message);
        Task::none()
    }

    fn vehicle_message_update(&mut self, message: vehicle_panel::Message) -> Task<Message> {
        match message {
            vehicle_panel::Message::SetPrimary(system) => {
                let previous = self.fleet.primary();

                if self.fleet.set_primary(system) {
                    // The map's mission interaction derives from the displayed
                    // vehicle: mid-drag, the outgoing vehicle's editor keeps
                    // its drag armed, so a later switch back would teleport the
                    // dragged waypoint on free mouse movement. Ending the
                    // outgoing vehicle's drag keeps the state self-consistent.
                    let drag_end =
                        previous
                            .filter(|previous| previous != &system)
                            .map(|previous| {
                                self.mission_message_update(mission_panel::Message {
                                    system: previous,
                                    kind: mission_panel::MessageKind::EndDrag,
                                })
                            });

                    match drag_end {
                        Some(drag_end) => Task::batch([drag_end]),
                        None => Task::none(),
                    }
                } else {
                    Task::none()
                }
            }
            vehicle_panel::Message::Follow(follow) => {
                let (task, _) = self.map.update(map::Message::Follow(follow));
                task.map(Message::Map)
            }
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
        let (map_task, event) = self.map.update(message);
        let map_task = map_task.map(Message::Map);

        // The map reports neutral interactions; translate each into the mission
        // feature's vocabulary, addressed to the vehicle captured at emission.
        let mission_task = match event {
            Some(map::Event::Picked {
                system,
                marker,
                position,
            }) => self.mission_message_update(mission_panel::Message {
                system,
                kind: mission_panel::MessageKind::MapPicked { marker, position },
            }),
            Some(map::Event::Dragged {
                system,
                marker,
                position,
            }) => self.mission_message_update(mission_panel::Message {
                system,
                kind: mission_panel::MessageKind::MoveWaypointTo(marker, position),
            }),
            Some(map::Event::Released { system }) => {
                self.mission_message_update(mission_panel::Message {
                    system,
                    kind: mission_panel::MessageKind::EndDrag,
                })
            }
            None => Task::none(),
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

    fn link_message_update(&mut self, message: link_panel::Message) -> Task<Message> {
        let (task, event) = self.link_panel.update(message, &mut self.link_manager);
        let task = task.map(Message::Link);

        match event {
            Some(link_panel::Event::LinksChanged(links)) => {
                self.configuration.links = links;
                Task::batch([task, Task::done(Message::SaveConfigurationToFile)])
            }
            None => task,
        }
    }

    /// Apply an application setting and persist it. The theme is derived from
    /// the configuration, so the new setting takes effect on the next frame.
    fn settings_message_update(&mut self, message: settings_panel::Message) -> Task<Message> {
        match message {
            settings_panel::Message::SetTheme(mode) => {
                if self.configuration.settings.theme != mode {
                    self.configuration.settings.theme = mode;
                    return Task::done(Message::SaveConfigurationToFile);
                }
            }
            settings_panel::Message::SetScaleFactor(scale) => {
                if self.configuration.settings.scale_factor != scale {
                    self.configuration.settings.scale_factor = scale;
                    return Task::done(Message::SaveConfigurationToFile);
                }
            }
        }

        Task::none()
    }

    /// Feed a batch of decoded telemetry into the fleet, in order, and turn what
    /// the fleet reports back into application effects.
    fn handle_mav_events(&mut self, events: Vec<Telemetry>) -> Task<Message> {
        let mut tasks = Vec::new();

        for event in events {
            match event {
                Telemetry::Message {
                    source,
                    received_at,
                    decoded,
                } => {
                    for fleet_event in self.fleet.observe(source, received_at, decoded) {
                        tasks.push(self.handle_fleet_event(fleet_event));
                    }
                }
                Telemetry::LinkClosed(link_id) => {
                    // Link closure is reconciled by the link manager, which
                    // drops any handle the router no longer holds on the next
                    // link message; no UI message is needed.
                    log::info!("Link {link_id:?} closed");
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
                Task::done(Message::Parameter(parameter_panel::Message::Initialize(
                    system,
                )))
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

    fn view_panes(&self) -> impl Widget<Message> + '_ {
        use crate::widget::pane_grid::{self, PaneGrid};

        let panes = PaneGrid::new(self.layout.panes(), |pane, panel, is_maximized| {
            // The map paints opaque tiles across its whole pane, which a rounded
            // container cannot clip; it draws its own frame instead (see
            // `map::view`). Every other pane is a rounded card split by a rule.
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

        iced::widget::container(panes).padding(5.0)
    }

    fn pane_body(&self, panel: layout::Panel) -> Element<'_, Message> {
        let theme = self.theme();

        match panel {
            layout::Panel::Map => {
                let primary = self.fleet.primary();
                let dragging = self.mission_panel.dragging(primary);
                self.map
                    .view(self.fleet.vehicles(), primary, dragging, &theme)
                    .map(Message::Map)
                    .boxed()
            }
            layout::Panel::Vehicles => vehicle_panel::view(
                self.fleet.vehicles(),
                self.fleet.primary(),
                self.map.follow,
                &theme,
            )
            .map(Message::Vehicles)
            .boxed(),
            layout::Panel::Parameters => self
                .parameter_panel
                .view(self.fleet.vehicles(), self.fleet.primary(), &theme)
                .map(Message::Parameter)
                .boxed(),
            layout::Panel::Mission => self
                .mission_panel
                .view(self.fleet.vehicles(), self.fleet.primary(), &theme)
                .map(Message::Mission)
                .boxed(),
            layout::Panel::Links => iced::widget::column![
                self.link_panel.view_editor().map(Message::Link),
                self.link_panel
                    .view_status(&self.link_manager)
                    .map(Message::Link),
            ]
            .spacing(10.0)
            .boxed(),
            layout::Panel::Logs => self
                .log_panel
                .view(self.fleet.vehicles(), self.fleet.primary(), &theme)
                .map(Message::Logs)
                .boxed(),
            layout::Panel::Settings => settings_panel::view(&self.configuration.settings)
                .map(Message::Settings)
                .boxed(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let fleet_tick = iced::time::every(Duration::from_millis(500)).map(Message::FleetTick);

        // Trailing debounce for layout persistence: the timer is restarted on
        // every layout change (the revision is part of the subscription
        // identity), so the write happens `LAYOUT_SAVE_DEBOUNCE` after the last
        // change rather than repeatedly throughout a drag/resize.
        let layout_tick = if self.layout.is_dirty() {
            iced::time::every(LAYOUT_SAVE_DEBOUNCE)
                .with(self.layout.revision())
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
