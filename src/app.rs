use std::{collections::BTreeMap, path::Path};

use iced::{
    Length, Subscription, Task, Theme, Widget,
    futures::channel::mpsc::SendError,
    theme,
};
use mavio::{
    DefaultDialect,
    default_dialect::messages::ComponentInformationBasic,
    error::SpecError,
};

use crate::{
    config, mav,
    domain::vehicle::Vehicle,
    features::link_panel::{self, LinkPanel},
    features::map::{self, Map},
    features::parameter_panel::{self, ParameterPanel},
    features::top_bar,
    features::vehicle_panel,
    foundation::MavlinkId,
    link::LinkBuilder,
    mav::MavlinkRouter,
};

pub struct Application {
    pub(crate) theme: Theme,
    pub(crate) map: Map,
    pub(crate) link_panel: LinkPanel,
    pub(crate) parameter_panel: ParameterPanel,
    pub(crate) router: MavlinkRouter,
    pub(crate) configuration: config::Configuration,
    pub(crate) vehicles: BTreeMap<MavlinkId, Vehicle>,
    pub(crate) primary_vehicle: Option<MavlinkId>,
}

impl From<SendError> for Message {
    fn from(_: SendError) -> Self {
        unreachable!("Why are we here ya dingus?")
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Map(map::Message),
    Link(link_panel::Message),
    MavEvent(mav::Event),
    Parameter(parameter_panel::Message),

    SaveConfigurationToFile,

    #[allow(unused)] // TODO
    UpdateAndSaveConfiguration(config::Configuration),

    SetPrimaryVehicle(MavlinkId),
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

        let application = Application {
            theme: theme::Theme::GruvboxDark,
            map: Map::new(),
            link_panel: LinkPanel::new(link_builder, "Default".to_owned()),
            router: client,
            configuration: config.clone(),
            vehicles: BTreeMap::new(),
            parameter_panel: ParameterPanel::new(),
            primary_vehicle: None,
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

    fn save_configuration_to_file(&mut self) {
        if let Err(error) = self.configuration.write_to_file() {
            log::error!("Unable to save configuration file: {}", error);
        }
    }

    fn maybe_update(&mut self, message: Message) -> Option<Task<Message>> {
        match message {
            Message::SaveConfigurationToFile => self.save_configuration_to_file(),
            Message::UpdateAndSaveConfiguration(config) => {
                self.configuration = config;
                self.save_configuration_to_file()
            }
            Message::Map(message) => return Some(self.map.update(message).map(Message::Map)),
            Message::Link(message) => return Some(self.link_message_update(message)),
            Message::MavEvent(event) => return Some(self.handle_mav_event(event)),
            Message::Parameter(message) => return Some(self.parameter_message_update(message)),
            Message::SetPrimaryVehicle(mav_id) => {
                self.primary_vehicle = Some(mav_id);
            }
        }

        None
    }

    pub(crate) fn set_file_picker_path_config(&mut self, path: &Path) -> Option<()> {
        let file_picker_path = path.is_dir().then_some(path).or(path.parent())?;

        self.configuration.file_picker_path = Some(file_picker_path.to_path_buf());
        self.save_configuration_to_file();

        Some(())
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
                self.set_file_picker_path_config(&path);
                task
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
                if !self.vehicles.contains_key(&mav_id) {
                    log::info!("New vehicle detected: {mav_id:?}");
                    self.vehicles.insert(mav_id, Vehicle::new(mav_id));
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
        let overlay = iced::widget::column![
            iced::widget::opaque(
                top_bar::view(
                    &self.link_panel,
                    self.map.follow,
                    &self.vehicles,
                    self.primary_vehicle,
                )
                .map(Self::top_bar_message)
                .boxed(),
            ),
            iced::widget::row![
                iced::widget::opaque(
                    self.parameter_panel
                        .view(&self.vehicles, self.primary_vehicle, &self.router)
                        .map(Message::Parameter),
                ),
                iced::widget::opaque(self.link_panel.view_editor().map(Message::Link)),
                iced::widget::opaque(self.link_panel.view_status().map(Message::Link)),
                iced::widget::space().width(Length::Fill),
                iced::widget::opaque(vehicle_panel::view(
                    &self.vehicles,
                    self.primary_vehicle,
                    &self.theme,
                )),
            ]
            .spacing(10.0),
        ]
        .padding(10.0)
        .spacing(10.0);

        iced::widget::stack!(self.map.view(&self.vehicles).map(Message::Map), overlay)
    }

    /// Translate a top-bar message into a root message.
    fn top_bar_message(message: top_bar::Message) -> Message {
        match message {
            top_bar::Message::Link(message) => Message::Link(message),
            top_bar::Message::Follow(value) => Message::Map(map::Message::Follow(value)),
            top_bar::Message::SetPrimary(mav_id) => Message::SetPrimaryVehicle(mav_id),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
}
