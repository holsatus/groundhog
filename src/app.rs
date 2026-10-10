use std::{collections::BTreeMap, path::Path, time::Duration};

use iced::{
    Element, Length, Subscription, Task, Theme, Widget, futures::channel::mpsc::SendError, theme,
};
use mavio::{
    DefaultDialect, default_dialect::messages::ComponentInformationBasic, error::SpecError,
};

use crate::{
    config,
    domain::vehicle::Vehicle,
    features::link_panel::{self, LinkPanel},
    features::log_panel::{self, LogPanel},
    features::map::{self, Map},
    features::parameter_panel::{self, ParameterPanel},
    features::vehicle_panel,
    foundation::MavlinkId,
    layout::{self, Layout},
    link::LinkBuilder,
    mav,
    mav::MavlinkRouter,
    style,
};

pub struct Application {
    pub(crate) theme: Theme,
    pub(crate) layout: Layout,
    pub(crate) map: Map,
    pub(crate) link_panel: LinkPanel,
    pub(crate) log_panel: LogPanel,
    pub(crate) parameter_panel: ParameterPanel,
    pub(crate) router: MavlinkRouter,
    pub(crate) configuration: config::Configuration,
    pub(crate) vehicles: BTreeMap<MavlinkId, Vehicle>,
    pub(crate) primary_vehicle: Option<MavlinkId>,
    /// Set when the layout changed and needs to be persisted (debounced).
    pub(crate) layout_dirty: bool,
}

impl From<SendError> for Message {
    fn from(_: SendError) -> Self {
        unreachable!("Why are we here ya dingus?")
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Layout(layout::Message),
    Map(map::Message),
    Link(link_panel::Message),
    Logs(log_panel::Message),
    MavEvent(mav::Event),
    Parameter(parameter_panel::Message),

    SaveConfigurationToFile,

    #[allow(unused)] // TODO
    UpdateAndSaveConfiguration(config::Configuration),

    SetPrimaryVehicle(MavlinkId),

    /// Debounce tick that flushes a pending layout save.
    Tick,

    /// Result of an asynchronous configuration write.
    ConfigurationSaved(Result<(), String>),
}

impl From<parameter_panel::Message> for Message {
    fn from(value: parameter_panel::Message) -> Self {
        Message::Parameter(value)
    }
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
        let events = Task::stream(events).map(Message::MavEvent);

        let layout = config
            .layout
            .clone()
            .map_or_else(Layout::new, Layout::from_configuration);

        let application = Application {
            theme: theme::Theme::GruvboxDark,
            layout,
            map: Map::new(),
            link_panel: LinkPanel::new(link_builder, "Default".to_owned()),
            log_panel: LogPanel::new(),
            router: client,
            configuration: config.clone(),
            vehicles: BTreeMap::new(),
            parameter_panel: ParameterPanel::new(),
            primary_vehicle: None,
            layout_dirty: false,
        };

        (application, events)
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
            Message::UpdateAndSaveConfiguration(config) => {
                self.configuration = config;
                return Some(self.save_configuration());
            }
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
            Message::MavEvent(event) => return Some(self.handle_mav_event(event)),
            Message::Parameter(message) => return Some(self.parameter_message_update(message)),
            Message::SetPrimaryVehicle(mav_id) => {
                self.primary_vehicle = Some(mav_id);
            }
            Message::Tick => {
                if self.layout_dirty {
                    self.layout_dirty = false;
                    return Some(self.save_configuration());
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
                vehicles: &mut self.vehicles,
                primary: self.primary_vehicle,
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

    /// Handle a MAVLink event from the router: register newly seen vehicles and
    /// feed inbound messages into their state.
    fn handle_mav_event(&mut self, event: mav::Event) -> Task<Message> {
        match event {
            mav::Event::Message(inbound) => {
                let mav_id = inbound.source_mav;

                // This will be the first vehicle, set it as the primary
                if self.vehicles.is_empty() {
                    self.primary_vehicle = Some(mav_id);
                }

                let mut tasks = Vec::new();
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    self.vehicles.entry(mav_id)
                {
                    log::info!("New vehicle detected: {mav_id:?}");
                    entry.insert(Vehicle::new(mav_id));
                    tasks.push(self.initialize_vehicle(mav_id));
                }

                if let Some(vehicle) = self.vehicles.get_mut(&mav_id) {
                    match inbound.frame.decode::<DefaultDialect>() {
                        Ok(message) => vehicle.handle_message(inbound.received_at, message),
                        Err(error) => match error {
                            mavio::Error::Spec(SpecError::NotInDialect(id)) => {
                                // Downgrade this error to trace to avoid spamming the logs
                                log::trace!("mavlink message id {id} is not in default dialect")
                            }
                            error => {
                                log::trace!("could not decode mavlink frame: {error}");
                            }
                        },
                    }
                }

                if self.primary_vehicle.is_some_and(|id| id == mav_id) {
                    self.parameter_panel
                        .refresh(self.primary_vehicle, &self.vehicles);

                    if self.map.follow
                        && let Some(vehicle) = self.vehicles.get(&mav_id)
                        && let Some(position) = &vehicle.global_position
                    {
                        tasks.push(Task::done(Message::Map(map::Message::SetViewPosition(
                            position.pos,
                        ))));
                    }
                }

                Task::batch(tasks)
            }
            mav::Event::LinkClosed(link_id) => {
                log::info!("Link {link_id:?} closed");
                Task::none()
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
        .on_resize(5, |event| Message::Layout(layout::Message::Resized(event)))
        .boxed()
    }

    fn pane_body(&self, panel: layout::Panel) -> Element<'_, Message> {
        match panel {
            layout::Panel::Map => self.map.view(&self.vehicles).map(Message::Map).boxed(),
            layout::Panel::Vehicles => vehicle_panel::view(
                &self.vehicles,
                self.primary_vehicle,
                self.map.follow,
                &self.theme,
            ),
            layout::Panel::Parameters => self
                .parameter_panel
                .view(&self.vehicles, self.primary_vehicle, &self.router)
                .map(Message::Parameter)
                .boxed(),
            layout::Panel::Links => iced::widget::column![
                self.link_panel.view_editor().map(Message::Link),
                self.link_panel.view_status().map(Message::Link),
            ]
            .spacing(10.0)
            .boxed(),
            layout::Panel::Logs => self
                .log_panel
                .view(&self.vehicles, self.primary_vehicle, &self.theme)
                .map(Message::Logs)
                .boxed(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // While the layout is dirty, tick to debounce a configuration write.
        if self.layout_dirty {
            iced::time::every(Duration::from_millis(800)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }
}
