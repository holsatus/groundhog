use std::{
    path::Path,
    time::{Duration, Instant},
};

use iced::futures::{SinkExt, Stream, StreamExt};
use iced::{Element, Length, Subscription, Task, Theme, Widget};
use mavio::{
    DefaultDialect, default_dialect::messages::ComponentInformationBasic, error::SpecError,
};

use crate::{
    config,
    domain::fleet::{Fleet, FleetEvent},
    domain::vehicle::Liveness,
    features::link_panel::{self, LinkPanel},
    features::log_panel::{self, LogPanel},
    features::map::{self, Map},
    features::parameter_panel::{self, ParameterPanel},
    features::settings_panel,
    features::vehicle_panel,
    foundation::MavlinkId,
    layout::{self, Layout},
    link::LinkBuilder,
    mav,
    mav::MavlinkRouter,
    style,
};

pub struct Application {
    pub(crate) layout: Layout,
    pub(crate) map: Map,
    pub(crate) link_panel: LinkPanel,
    pub(crate) log_panel: LogPanel,
    pub(crate) parameter_panel: ParameterPanel,
    pub(crate) router: MavlinkRouter,
    pub(crate) configuration: config::Configuration,
    pub(crate) fleet: Fleet,
    /// Set when the layout changed and needs to be persisted (debounced).
    pub(crate) layout_dirty: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Layout(layout::Message),
    Map(map::Message),
    Link(link_panel::Message),
    Logs(log_panel::Message),
    /// A batch of MAVLink events, collected at most once per telemetry frame.
    MavEvents(Vec<mav::Event>),
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

        let link_builder = config
            .link_config
            .as_ref()
            .map(|cfg| cfg.to_builder())
            .unwrap_or_else(LinkBuilder::default_udp);

        let (client, events) = mav::MavlinkRouter::new();
        let events = Task::stream(coalesce(events, TELEMETRY_FRAME).map(Message::MavEvents));

        let layout = config
            .layout
            .clone()
            .map_or_else(Layout::new, Layout::from_configuration);

        let application = Application {
            layout,
            map: Map::new(),
            link_panel: LinkPanel::new(link_builder, "Default".to_owned()),
            log_panel: LogPanel::new(),
            router: client,
            configuration: config.clone(),
            fleet: Fleet::default(),
            parameter_panel: ParameterPanel::new(),
            layout_dirty: false,
        };

        (application, events)
    }

    pub fn theme(&self) -> Theme {
        self.configuration.settings.theme.theme()
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
            Message::Layout(message) => {
                let changed = message.changes_layout();
                self.layout.update(message);
                if changed {
                    self.configuration.layout = self.layout.configuration();
                    self.layout_dirty = true;
                }
            }
            Message::Map(message) => return Some(self.map.update(message).map(Message::Map)),
            Message::Link(message) => return Some(self.link_message_update(message)),
            Message::Logs(message) => self.log_panel.update(message),
            Message::MavEvents(events) => return Some(self.handle_mav_events(events)),
            Message::Parameter(message) => return Some(self.parameter_message_update(message)),
            Message::Vehicles(message) => match message {
                vehicle_panel::Message::SetPrimary(mav_id) => {
                    self.fleet.set_primary(mav_id);
                }
                vehicle_panel::Message::Follow(follow) => {
                    return Some(
                        self.map
                            .update(map::Message::Follow(follow))
                            .map(Message::Map),
                    );
                }
            },
            Message::Settings(message) => return Some(self.settings_message_update(message)),
            Message::Tick => {
                if self.layout_dirty {
                    self.layout_dirty = false;
                    return Some(self.save_configuration());
                }
            }
            Message::FleetTick(now) => {
                for event in self.fleet.tick(now) {
                    match event {
                        // `tick` only ages vehicles today, so liveness is the
                        // only event it can produce.
                        FleetEvent::LivenessChanged(mav_id, liveness) => {
                            log_liveness(mav_id, liveness);
                        }
                        FleetEvent::Registered(mav_id) => {
                            log::debug!("Fleet tick registered vehicle {mav_id:?}");
                        }
                        FleetEvent::PrimaryMoved(..) => {}
                    }
                }
            }
        }

        None
    }

    pub(crate) fn set_file_picker_path_config(&mut self, path: &Path) -> Task<Message> {
        let Some(file_picker_path) = path.is_dir().then_some(path).or(path.parent()) else {
            return Task::none();
        };

        self.configuration.file_picker_path = Some(file_picker_path.to_path_buf());
        self.save_configuration()
    }

    fn parameter_message_update(&mut self, message: parameter_panel::Message) -> Task<Message> {
        let (task, event) = {
            let mut ctx = parameter_panel::Context {
                router: &self.router,
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
    fn initialize_vehicle(&self, mav_id: MavlinkId) -> Task<Message> {
        let router = self.router.clone();
        Task::perform(
            async move {
                if let Err(error) = router
                    .request_message::<ComponentInformationBasic>(mav_id)
                    .await
                {
                    log::warn!("No COMPONENT_INFORMATION_BASIC from {mav_id:?}: {error}");
                }

                // This requests AUTOPILOT_VERSION to get capabilities for param encoding
                router.param_list(mav_id, true).await
            },
            move |result| parameter_panel::Message::ListReloadResult(mav_id, result).into(),
        )
    }

    fn link_message_update(&mut self, message: link_panel::Message) -> Task<Message> {
        let (task, event) = self.link_panel.update(message, &self.router);
        let task = task.map(Message::Link);

        match event {
            Some(link_panel::Event::SetConfig(build)) => {
                self.configuration.link_config = Some(build);
                task
            }
            Some(link_panel::Event::SaveConfig(build)) => {
                self.configuration.link_config = Some(build);
                Task::batch([task, Task::done(Message::SaveConfigurationToFile)])
            }
            None => task,
        }
    }

    /// Apply an application setting and persist it. The theme takes effect
    /// immediately because [`Application::theme`] reads it on every render.
    fn settings_message_update(&mut self, message: settings_panel::Message) -> Task<Message> {
        match message {
            settings_panel::Message::SetTheme(mode) => {
                if self.configuration.settings.theme != mode {
                    self.configuration.settings.theme = mode;
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
                        match fleet_event {
                            FleetEvent::Registered(mav_id) => {
                                log::info!("New vehicle detected: {mav_id:?}");
                                tasks.push(self.initialize_vehicle(mav_id));
                            }
                            FleetEvent::LivenessChanged(mav_id, liveness) => {
                                log_liveness(mav_id, liveness);
                            }
                            FleetEvent::PrimaryMoved(position) if self.map.follow => {
                                tasks.push(Task::done(Message::Map(
                                    map::Message::SetViewPosition(position),
                                )));
                            }
                            FleetEvent::PrimaryMoved(..) => {}
                        }
                    }
                }
                mav::Event::LinkClosed(link_id) => log::info!("Link {link_id:?} closed"),
            }
        }

        Task::batch(tasks)
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
            layout::Panel::Map => self
                .map
                .view(self.fleet.vehicles())
                .map(Message::Map)
                .boxed(),
            layout::Panel::Vehicles => {
                let theme = self.theme();
                vehicle_panel::view(
                    self.fleet.vehicles(),
                    self.fleet.primary(),
                    self.map.follow,
                    &theme,
                )
                .map(Message::Vehicles)
                .boxed()
            }
            layout::Panel::Parameters => self
                .parameter_panel
                .view(self.fleet.vehicles(), self.fleet.primary(), &self.router)
                .map(Message::Parameter)
                .boxed(),
            layout::Panel::Links => iced::widget::column![
                self.link_panel.view_editor().map(Message::Link),
                self.link_panel.view_status().map(Message::Link),
            ]
            .spacing(10.0)
            .boxed(),
            layout::Panel::Logs => {
                let theme = self.theme();
                self.log_panel
                    .view(self.fleet.vehicles(), self.fleet.primary(), &theme)
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

        // While the layout is dirty, tick to debounce a configuration write.
        let layout_tick = if self.layout_dirty {
            iced::time::every(Duration::from_millis(800)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        };

        Subscription::batch([fleet_tick, layout_tick])
    }
}

/// Log a liveness transition, louder the further a vehicle drifts.
fn log_liveness(mav_id: MavlinkId, liveness: Liveness) {
    match liveness {
        Liveness::Lost => log::warn!("Vehicle {mav_id:?} lost"),
        Liveness::Stale => log::info!("Vehicle {mav_id:?} is stale"),
        Liveness::Active => log::info!("Vehicle {mav_id:?} is active"),
    }
}
