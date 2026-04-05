use std::{
    collections::BTreeMap,
    error::Error,
    num::NonZero,
    sync::Arc,
    time::{Duration, Instant},
};

use iced::{
    Alignment, Color, Element, Font, Length, Task,
    alignment::Vertical,
    widget::{
        Button, Column, ProgressBar, Space, Text, button, column, container, pick_list,
        progress_bar, row, rule, space, text::Ellipsis, text_input,
    },
};
use mav_param::{Ident, Value};
use mavio::{
    Frame,
    default_dialect::messages::{ParamRequestList, ParamSet},
    prelude::Versionless,
};

mod parameters;
use crate::{
    connection::{ConnectionHandle, LinkBuilder, LinkConfig, LinkId, LinkVariant},
    parameters::{
        MavlinkId, Parameter, Parameters, Vehicle, load_parameters_from_ini, value_as_string,
        value_parse_as, value_type_name,
    },
};

mod connection;

fn main() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("mav_param_tool", log::LevelFilter::Debug)
        .init();

    iced::application(Application::boot, Application::update, Application::view)
        .run()
        .unwrap();
}

type ArcError = Arc<dyn Error + Send + Sync + 'static>;

struct Configuration;

struct LinkHandle;

struct Application {
    link_builder: LinkBuilder,
    link_config: Option<LinkConfig>,
    connection: Option<ConnectionHandle>,
    configuration: Configuration,
    links: BTreeMap<LinkId, LinkHandle>,
    vehicles: BTreeMap<MavlinkId, Vehicle>,
}

#[derive(Debug, Clone)]
enum Message {
    Param(ParamMessage),
    Conn(ConnMessage),
    ReloadParameters,
}

#[derive(Debug, Clone)]
enum ParamMessage {
    ResetParamValue(MavlinkId, Ident),
    EditParamValue(MavlinkId, Ident, Value),
    UploadParamValue(MavlinkId, Ident, Value),
    EditParamBuffer(MavlinkId, Ident, String),
}

#[derive(Debug, Clone)]
enum ConnMessage {
    ConnectFailed(ArcError),
    ConnectSuccess(ConnectionHandle),
    RecvFrame(Frame<Versionless>, LinkId),
    RecvError(mavio::Error, LinkId),
    ConnectToLink(LinkConfig),
    DisconnectLink,
    ChangeLinkVariant(LinkVariant),
    UpdateLinkBuilder(LinkBuilder),
    DetectSerialPorts,
}

impl Application {
    fn boot() -> Self {
        Application {
            link_builder: LinkBuilder::default_udp(),
            link_config: LinkBuilder::default_udp().try_build(),
            connection: None,
            configuration: Configuration,
            links: BTreeMap::new(),
            vehicles: BTreeMap::from_iter([(
                MavlinkId {
                    system: 1,
                    component: 1,
                },
                Vehicle {
                    params: Parameters::new(),
                    last_heartbeat: None,
                    gyroscope: None,
                },
            )]),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ReloadParameters => {
                if let Some(connection) = self.connection.as_mut() {
                    for (mav_id, vehicle) in &mut self.vehicles {
                        vehicle.params.loading_state.has_loaded.clear();
                        connection.send_message(&ParamRequestList {
                            target_system: mav_id.system,
                            target_component: mav_id.component,
                        });
                    }
                }
                Task::none()
            }
            Message::Param(message) => self.update_param(message),
            Message::Conn(message) => self.update_conn(message),
        }
    }

    fn update_param(&mut self, message: ParamMessage) -> Task<Message> {
        match message {
            ParamMessage::EditParamValue(identity, ident, new_value) => {
                if let Some(entry) = self.vehicles.get_mut(&identity) {
                    if let Some(param) = entry.params.map.get_mut(&ident) {
                        let mut to_edit = param.value.clone();
                        if !parameters::value_set_from(&mut to_edit, new_value) {
                            log::error!("Parameter got set with the wrong type");
                        }

                        if to_edit != param.value {
                            param.changed = Some(to_edit)
                        } else {
                            param.changed = None
                        }
                    }
                }
            }

            ParamMessage::EditParamBuffer(identity, ident, buffer) => {
                let Some(entry) = self.vehicles.get_mut(&identity) else {
                    log::warn!("No set of parameters for vehicle: {:?}", identity);
                    return Task::none();
                };

                let Some(param) = entry.params.map.get_mut(&ident) else {
                    log::warn!("No parameter with identifier: {}", ident.as_str());
                    return Task::none();
                };

                param.changed = None;

                match value_parse_as(param.value, &buffer) {
                    Some(new_value) => {
                        if new_value != param.value {
                            param.changed = Some(new_value);
                        }
                    }
                    None => {
                        log::error!("param: {} !! {buffer:?}", ident.as_str());
                    }
                }

                param.editing = Some(buffer);
            }

            ParamMessage::ResetParamValue(identity, ident) => {
                let Some(entry) = self.vehicles.get_mut(&identity) else {
                    log::warn!("No set of parameters for vehicle: {:?}", identity);
                    return Task::none();
                };

                let Some(param) = entry.params.map.get_mut(&ident) else {
                    log::warn!("No parameter with identifier: {}", ident.as_str());
                    return Task::none();
                };

                param.editing = None;
                param.changed = None;
            }
            ParamMessage::UploadParamValue(identity, ident, value) => {
                if let Some(connection) = self.connection.as_mut() {
                    let (param_value, param_type) = parameters::value_into_bytewise(value);

                    let param_set = ParamSet {
                        target_system: identity.system,
                        target_component: identity.component,
                        param_id: ident.as_raw().clone(),
                        param_value,
                        param_type,
                    };

                    connection.send_message(&param_set);
                }
            }
        }

        Task::none()
    }

    fn update_conn(&mut self, message: ConnMessage) -> Task<Message> {
        match message {
            ConnMessage::RecvFrame(frame, link_id) => {
                let message = match frame.decode::<mavio::DefaultDialect>() {
                    Ok(message) => message,
                    Err(error) => {
                        log::error!("Mavlink decode error: {error:?}");
                        return Task::none();
                    }
                };

                let mav_id = MavlinkId {
                    system: frame.system_id(),
                    component: frame.component_id(),
                };

                match message {
                    mavio::DefaultDialect::Heartbeat(msg) => {
                        if let Some(vehicle) = self.vehicles.get_mut(&mav_id) {
                            vehicle.last_heartbeat = Some(Instant::now())
                        }
                    }

                    mavio::DefaultDialect::ScaledImu(msg) => {
                        if let Some(vehicle) = self.vehicles.get_mut(&mav_id) {
                            vehicle.gyroscope = Some((
                                msg.xgyro as f32 * 1e-3,
                                msg.ygyro as f32 * 1e-3,
                                msg.zgyro as f32 * 1e-3,
                            ))
                        }
                    }
                    mavio::DefaultDialect::ParamValue(msg) => {
                        let Some(value) =
                            parameters::value_from_bytewise(msg.param_value, msg.param_type)
                        else {
                            log::error!("Unsupported numeric type");
                            return Task::none();
                        };

                        let Ok(ident) = Ident::try_from(&msg.param_id) else {
                            log::error!("Invalid parameter identifier");
                            return Task::none();
                        };

                        if let Some(vehicle) = self.vehicles.get_mut(&mav_id) {
                            log::info!("Got parameter: {}: {:?}", ident.as_str(), value);
                            vehicle.params.map.insert(ident, Parameter::new(value));

                            // Keep track of how many we expect
                            if msg.param_count > 0 {
                                vehicle
                                    .params
                                    .loading_state
                                    .has_loaded
                                    .insert(msg.param_index);

                                vehicle.params.loading_state.expected_count = msg.param_count;

                                if msg.param_index + 1 == msg.param_count {
                                    let got = vehicle.params.loading_state.has_loaded.len();
                                    let exp = msg.param_count;
                                    if got == exp as usize {
                                        log::info!("Loaded total of {got} parameters");
                                    } else {
                                        log::warn!("Expected {exp} paramaters, got {got}");
                                    }
                                }
                            }
                        }
                    }
                    _ => log::trace!("Unsupported message type: {}", frame.message_id()),
                }
            }
            ConnMessage::RecvError(error, link_id) => (),
            ConnMessage::ChangeLinkVariant(variant) => {
                if self.link_builder.to_variant() != variant {
                    self.link_builder = variant.to_default_builder();
                    self.link_config = self.link_builder.try_build();
                }
            }
            ConnMessage::UpdateLinkBuilder(link_builder) => {
                self.link_builder = link_builder;
                self.link_config = self.link_builder.try_build();
            }
            ConnMessage::DetectSerialPorts => {
                if let LinkBuilder::Serial {
                    available_ports, ..
                } = &mut self.link_builder
                {
                    match serial2_tokio::SerialPort::available_ports() {
                        Ok(ports) => {
                            *available_ports = ports
                                .iter()
                                .map(|p| p.to_string_lossy().to_string())
                                .collect();
                            available_ports.sort();
                        }
                        Err(error) => {
                            log::error!("Unable to fetch serial ports: {error}");
                        }
                    }

                    self.link_config = self.link_builder.try_build();
                }
            }
            ConnMessage::ConnectToLink(config) => {
                log::info!("Connecting to: {config:?}");
                return config.connect();
            }
            ConnMessage::DisconnectLink => {
                self.connection = None;
            }
            ConnMessage::ConnectFailed(error) => {
                log::error!("Connect failed: {error:?}");
            }
            ConnMessage::ConnectSuccess(connection) => {
                log::info!("Connection established");
                self.connection = Some(connection)
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        column![
            iced::widget::container(self.view_top_panel()),
            row![
                self.view_param_list_scrollable(),
                iced::widget::container(self.view_vehicle_information())
                    .style(iced::widget::container::bordered_box)
                    .padding(10.0),
            ]
            .spacing(10.0)
        ]
        .spacing(10.0)
        .padding(10.0)
        .into()
    }

    fn view_top_panel(&self) -> Element<'_, Message> {
        let mut row_contents = Vec::<Element<'_, Message>>::new();

        let selector = iced::widget::pick_list(
            Some(self.link_builder.to_variant()),
            LinkVariant::list(),
            ToString::to_string,
        )
        .placeholder("Pick one")
        .on_select(|v| Message::Conn(ConnMessage::ChangeLinkVariant(v)))
        .into();

        row_contents.push(selector);
        row_contents.push(iced::widget::rule::vertical(1.0).into());

        match &self.link_builder {
            LinkBuilder::Tcp { addr, port } => {
                let addr_input = text_input("address", addr)
                    .on_input(|addr| {
                        Message::Conn(ConnMessage::UpdateLinkBuilder(LinkBuilder::Tcp {
                            addr: addr,
                            port: port.clone(),
                        }))
                    })
                    .width(160.0)
                    .into();

                let port_input = text_input("port", port)
                    .on_input(|port| {
                        Message::Conn(ConnMessage::UpdateLinkBuilder(LinkBuilder::Tcp {
                            addr: addr.clone(),
                            port: port,
                        }))
                    })
                    .width(100.0)
                    .into();

                row_contents.push(addr_input);
                row_contents.push(port_input);
            }
            LinkBuilder::Udp { addr, port } => {
                let addr_input = text_input("address", addr)
                    .on_input(|addr| {
                        Message::Conn(ConnMessage::UpdateLinkBuilder(LinkBuilder::Udp {
                            addr: addr,
                            port: port.clone(),
                        }))
                    })
                    .width(160.0)
                    .into();

                let port_input = text_input("port", port)
                    .on_input(|port| {
                        Message::Conn(ConnMessage::UpdateLinkBuilder(LinkBuilder::Udp {
                            addr: addr.clone(),
                            port: port,
                        }))
                    })
                    .width(100.0)
                    .into();

                row_contents.push(addr_input);
                row_contents.push(port_input);
            }
            LinkBuilder::Serial {
                port,
                available_ports,
                baud,
            } => {
                let port_picker =
                    pick_list(port.as_ref(), available_ports.as_slice(), |x| x.clone())
                        .placeholder("Select a port")
                        .ellipsis(Ellipsis::Start)
                        .on_open(Message::Conn(ConnMessage::DetectSerialPorts))
                        .on_select(|selected_port| {
                            Message::Conn(ConnMessage::UpdateLinkBuilder(LinkBuilder::Serial {
                                port: Some(selected_port),
                                baud: *baud,
                                available_ports: available_ports.clone(),
                            }))
                        })
                        .width(160.0)
                        .into();

                let baud_picker = pick_list(Some(baud), connection::BAUDRATES, |x| x.to_string())
                    .on_open(Message::Conn(ConnMessage::DetectSerialPorts))
                    .on_select(|selected_baud| {
                        Message::Conn(ConnMessage::UpdateLinkBuilder(LinkBuilder::Serial {
                            port: port.clone(),
                            baud: selected_baud,
                            available_ports: available_ports.clone(),
                        }))
                    })
                    .width(100.0)
                    .into();

                row_contents.push(port_picker);
                row_contents.push(baud_picker);
            }
        }

        row_contents.push(iced::widget::space::Space::new().width(Length::Fill).into());

        if self.connection.is_none() {
            let connect_button = Button::new(
                container("Connect")
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .on_press_maybe(
                self.link_config
                    .as_ref()
                    .map(|config| Message::Conn(ConnMessage::ConnectToLink(config.clone()))),
            )
            .width(100.0)
            .into();
            row_contents.push(connect_button);
        } else {
            let disconnect_button = Button::new(
                container("Disconnect")
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .style(button::danger)
            .on_press(Message::Conn(ConnMessage::DisconnectLink))
            .width(100.0)
            .into();
            row_contents.push(disconnect_button);
        }

        iced::widget::container(
            row::Row::from_vec(row_contents)
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .spacing(10.0),
        )
        .style(iced::widget::container::bordered_box)
        .width(Length::Fill)
        .padding(10.0)
        .into()
    }

    fn view_vehicle_information(&self) -> Element<'_, Message> {
        let mut entries = Vec::new();

        for (identity, vehicle) in &self.vehicles {
            entries.push(
                Text::new(format!(
                    "System: {}, component: {}",
                    identity.system, identity.component
                ))
                .into(),
            );

            entries.push(
                Text::new(format!("Heartbeat: {:?}", vehicle.last_heartbeat))
                    .color_maybe(
                        vehicle
                            .last_heartbeat
                            .is_none_or(|hb| hb.elapsed() > Duration::from_secs(2))
                            .then(|| Color::from_rgb8(255, 100, 100)),
                    )
                    .into(),
            );

            entries.push(Text::new(format!("Gyroscope: {:?}", vehicle.gyroscope)).into());
        }

        Column::from_vec(entries).into()
    }

    fn view_param_list_scrollable(&self) -> Element<'_, Message> {
        iced::widget::container(
            iced::widget::scrollable(self.view_param_list())
                .style(iced::widget::scrollable::default)
                .spacing(0.0),
        )
        .style(iced::widget::container::bordered_box)
        .into()
    }

    fn view_param_list(&self) -> Element<'_, Message> {
        let mut entries = Vec::new();

        let reload_button = Button::new("Reload parameters")
            .on_press(Message::ReloadParameters)
            .into();

        entries.push(reload_button);

        for (identity, vehicle) in &self.vehicles {
            let identity = identity.clone();

            let got = vehicle.params.loading_state.has_loaded.len();
            let exp = vehicle.params.loading_state.expected_count;

            let style = if got != exp as usize {
                progress_bar::primary
            } else {
                progress_bar::success
            };

            let progress = ProgressBar::new((0.0)..=(exp as f32), got as f32)
                .style(style)
                .girth(10.0)
                .into();

            entries.push(progress);

            let mut section = None;

            for (ident, param) in &vehicle.params.map {
                let type_name = value_type_name(param.value);

                let this_section = ident.as_str().split_once('.').map(|(sec, _)| sec);

                // Add larger section headers and separators
                if this_section != section {
                    if section.is_some() {
                        entries.push(space::vertical().height(0.0).into());
                        entries.push(rule::horizontal(1.0).into());
                    }
                    if let Some(section) = this_section {
                        entries.push(
                            Text::new(format!("[{section}]"))
                                .size(24.0)
                                .font(Font::MONOSPACE)
                                .into(),
                        );
                    }
                    section = this_section;
                }

                let value_string = match param.editing.clone() {
                    Some(buffer) => buffer.clone(),
                    None => value_as_string(param.value),
                };

                let is_modified = param.changed.is_some_and(|new| new != param.value);

                let ident_owned = ident.clone();
                let text_input = iced::widget::TextInput::new("Write value", &value_string)
                    .on_input(move |string| {
                        Message::Param(ParamMessage::EditParamBuffer(
                            identity,
                            ident_owned.clone(),
                            string,
                        ))
                    })
                    .style(move |theme, status| {
                        let mut style = iced::widget::text_input::default(theme, status);
                        if is_modified {
                            style.background =
                                iced::Background::Color(Color::from_rgb8(30, 120, 60));
                        }
                        style
                    });

                let ident_owned = ident.clone();
                let restore_button = Button::new("Restore")
                    .on_press_maybe(param.changed.map(|_| {
                        Message::Param(ParamMessage::ResetParamValue(identity, ident_owned.clone()))
                    }))
                    .width(80.0);

                let commit_button = Button::new("Upload")
                    .on_press_maybe(param.changed.map(|value| {
                        Message::Param(ParamMessage::UploadParamValue(
                            identity,
                            ident_owned.clone(),
                            value,
                        ))
                    }))
                    .width(80.0);

                let row = row![
                    Text::new(ident.as_str().to_string())
                        .width(180.0)
                        .font(Font::MONOSPACE),
                    Text::new(type_name)
                        .width(50.0)
                        .font(Font::MONOSPACE)
                        .align_x(Alignment::End)
                        .color(Color::from_rgba8(255, 255, 255, 0.5)),
                    Space::new().width(10.0),
                    text_input.width(100.0),
                    restore_button,
                    commit_button,
                ]
                .spacing(5.0)
                .align_y(Vertical::Center);

                entries.push(row.into());
            }
        }

        Column::from_vec(entries).spacing(5.0).padding(10.0).into()
    }
}
