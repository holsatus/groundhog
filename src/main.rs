use std::{
    collections::BTreeMap,
    error::Error,
    path::Path,
    sync::{Arc, LazyLock, atomic::AtomicU16},
    time::{Duration, Instant},
};

use iced::{
    Alignment, Color, Element, Font,
    Length::{self},
    Subscription, Task, Theme,
    alignment::Vertical,
    widget::{
        Button, Column, ProgressBar, Space, Text, TextInput, button, canvas::Stroke, container,
        image::Handle, pick_list, progress_bar, row, rule, space, stack, text::Ellipsis,
        text_input,
    },
};
use mavio::{
    Frame,
    default_dialect::{
        enums::{MavCmd, MavSeverity},
        messages::{CommandLong, ComponentInformationBasic, Heartbeat, ParamRequestList},
    },
    prelude::Versionless,
};
use rfd::AsyncFileDialog;
use slippery::{CacheMessage, Geodetic, Mercator, Projector, TileCache, Viewpoint, Zoom, location};

use crate::{
    connection::builder::{
        ConnectionHandle, LinkBuild, LinkBuilder, LinkConfig, LinkId, LinkVariant,
        WeakConnectionHandle,
    },
    parameter::base::{MavlinkId, ParamState, Parameters, value_as_string, value_type_name},
    vehicle::Vehicle,
};

mod parameter;

mod config;
mod connection;
mod vehicle;

fn main() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("groundhog", log::LevelFilter::Debug)
        .init();

    iced::application(Application::boot, Application::update, Application::view)
        .subscription(Application::subscription)
        .title("Holsatus Groundhog")
        .theme(Theme::GruvboxDark)
        .run()
        .expect("Groundhog dead");
}

type ArcError = Arc<dyn Error + Send + Sync + 'static>;
type BoxError = Box<dyn Error + Send + Sync + 'static>;
static ARROW_HANDLE: LazyLock<Handle> =
    LazyLock::new(|| Handle::from_bytes(include_bytes!("../assets/pointer.png").as_slice()));

struct Application {
    viewpoint: Viewpoint,
    projector: Option<Projector>,
    tile_cache: TileCache,
    link_config: LinkConfig,
    connection: Option<ConnectionHandle>,
    configuration: config::Configuration,
    vehicles: BTreeMap<MavlinkId, Vehicle>,
    parameter_filter: String,
    parameter_filtered: Option<Parameters>,
    primary_vehicle: Option<MavlinkId>,
    follow_primary_vehicle: bool,
}

#[derive(Debug)]
struct AtomicMavlinkId(AtomicU16);

impl AtomicMavlinkId {
    pub const fn new(sys: u8, com: u8) -> Self {
        Self(AtomicU16::new(u16::from_le_bytes([sys, com])))
    }

    pub fn load(&self) -> MavlinkId {
        let inner = self.0.load(std::sync::atomic::Ordering::Relaxed);
        let [system, component] = inner.to_le_bytes();
        MavlinkId { system, component }
    }

    #[allow(unused)] // TODO
    pub fn store(&self, mav_id: MavlinkId) {
        let inner = u16::from_le_bytes([mav_id.system, mav_id.component]);
        self.0.store(inner, std::sync::atomic::Ordering::Relaxed);
    }
}

static GCS_MAVLINK_ID: AtomicMavlinkId = AtomicMavlinkId::new(255, 1);

#[derive(Debug, Clone)]
enum Message {
    MapProjector(Projector),
    SetViewPosition(Mercator),
    MapCache(CacheMessage),
    Connection(ConnectionMessage),
    Parameter(parameter::Message),

    SaveConfigurationToFile,

    #[allow(unused)] // TODO
    UpdateAndSaveConfiguration(config::Configuration),

    SetPrimaryVehicle(MavlinkId),
    SetFollowPrimaryVehicle(bool),
}

impl From<parameter::Message> for Message {
    fn from(value: parameter::Message) -> Self {
        Message::Parameter(value)
    }
}

#[derive(Debug, Clone)]
enum ConnectionMessage {
    ConnectFailed(ArcError),
    ConnectSuccess(ConnectionHandle),
    RecvFrame(Frame<Versionless>, LinkId),
    RecvError(mavio::Error, LinkId),
    ConnectToLink(LinkBuild),
    DisconnectLink,
    ChangeLinkVariant(LinkVariant),
    UpdateLinkBuilder(LinkBuilder),
    DetectSerialPorts,
}

impl From<ConnectionMessage> for Message {
    fn from(value: ConnectionMessage) -> Self {
        Message::Connection(value)
    }
}

impl Application {
    fn boot() -> Self {
        let config = config::Configuration::initialize().unwrap_or_else(|e| {
            log::error!("Unable to initialize configuration file: {}", e);
            config::Configuration::default()
        });

        let link_builder = config
            .link_config
            .as_ref()
            .map(|cfg| cfg.to_builder())
            .unwrap_or_else(LinkBuilder::default_udp);

        Application {
            viewpoint: Viewpoint {
                position: location::paris().as_mercator(),
                zoom: Zoom::try_from(8.0).unwrap(),
            },
            projector: None,
            tile_cache: TileCache::new(slippery::sources::ArcGisWorldMap),
            link_config: LinkConfig::new(link_builder, "Default".to_owned()),
            connection: None,
            configuration: config.clone(),
            vehicles: BTreeMap::new(),
            parameter_filter: String::new(),
            parameter_filtered: None,
            primary_vehicle: None,
            follow_primary_vehicle: false,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
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
            Message::MapProjector(projector) => {
                // Only update the zoom if we are following the vehicle

                self.viewpoint = projector.viewpoint;
                self.follow_primary_vehicle = false;
                self.projector = Some(projector);
            }
            Message::SetViewPosition(mercator) => {
                self.viewpoint.position = mercator;
            }
            Message::MapCache(message) => {
                let map_task = self.tile_cache.update(message);
                return Some(map_task.map(Message::MapCache));
            }
            Message::Parameter(message) => return self.parameter_message_update(message),
            Message::Connection(message) => return Some(self.connection_message_update(message)),

            Message::SetPrimaryVehicle(mav_id) => {
                self.primary_vehicle = Some(mav_id);
            }
            Message::SetFollowPrimaryVehicle(follow) => {
                self.follow_primary_vehicle = follow;
            }
        }

        None
    }

    fn get_connection_handle(&self) -> Option<WeakConnectionHandle> {
        self.connection.as_ref().map(|handle| handle.downgrade())
    }

    fn set_file_picker_path_config(&mut self, path: &Path) -> Option<()> {
        let file_picker_path = path.is_dir().then_some(path).or(path.parent())?;

        self.configuration.file_picker_path = Some(file_picker_path.to_path_buf());
        self.save_configuration_to_file();

        Some(())
    }

    fn new_file_dialog(&self) -> AsyncFileDialog {
        let mut file_dialog = rfd::AsyncFileDialog::new();

        if let Some(path) = self.configuration.file_picker_path.as_ref() {
            file_dialog = file_dialog.set_directory(path)
        }

        file_dialog
    }

    fn get_vehicle_or_insert(&mut self, mav_id: MavlinkId) -> &mut Vehicle {
        self.vehicles.entry(mav_id).or_insert_with(|| {
            log::info!("New vehicle detected: {mav_id:?}");
            if let Some(handle) = self.connection.as_ref() {
                Self::request_vehicle_info(handle, mav_id);
            }
            Vehicle::new(mav_id)
        })
    }

    fn request_vehicle_info(handle: &ConnectionHandle, mav_id: MavlinkId) {
        let handle = handle.downgrade();
        tokio::spawn(async move {
            // Request parameters
            handle
                .send_message(ParamRequestList {
                    target_system: mav_id.system,
                    target_component: mav_id.component,
                    ..ParamRequestList::default()
                })
                .await;

            // Request basic component information
            let command = MavCmd::RequestMessage;
            handle
                .send_message(CommandLong {
                    target_system: mav_id.system,
                    target_component: mav_id.component,
                    command,
                    param1: ComponentInformationBasic::ID as f32,
                    ..CommandLong::default()
                })
                .await;

            // Wait for ack/nack to request command
            let ack = handle
                .await_messages(|message| match message {
                    mavio::DefaultDialect::CommandAck(ack) if ack.command == command => {
                        let target = MavlinkId {
                            system: ack.target_system,
                            component: ack.target_component,
                        };

                        (GCS_MAVLINK_ID.load() == target).then(|| ack.clone())
                    }
                    _ => None,
                })
                .await;

            match ack {
                Some(ack) => log::debug!("ACK for {:?} - {:?}", ack.command, ack.result),
                None => log::warn!("No response to command"),
            };
        });
    }

    fn connection_message_update(&mut self, message: ConnectionMessage) -> Task<Message> {
        let time_now = Instant::now();
        match message {
            ConnectionMessage::RecvFrame(frame, link_id) => {
                let message = match frame.decode::<mavio::DefaultDialect>() {
                    Ok(message) => message,
                    Err(mavio::Error::Frame(mavio::error::FrameError::NotInDialect(id))) => {
                        log::trace!("Message with id: {id} is not in dialect");
                        return Task::none();
                    }
                    Err(error) => {
                        log::error!("Mavlink decode error: {error:?}");
                        return Task::none();
                    }
                };

                let mav_id = MavlinkId {
                    system: frame.system_id(),
                    component: frame.component_id(),
                };

                // This will be the first vehicle, set it as the primary
                if self.vehicles.is_empty() {
                    self.primary_vehicle = Some(mav_id);
                }

                let vehicle = self.get_vehicle_or_insert(mav_id);

                vehicle.link_info_mut(link_id).last_message = Some(time_now);
                vehicle.handle_message(time_now, message);

                if self.primary_vehicle.is_some_and(|id| id == mav_id) {
                    self.refresh_filtered_parameter();
                }

                if self.follow_primary_vehicle {
                    if let Some(prim_id) = self.primary_vehicle
                        && prim_id == mav_id
                    {
                        if let Some(primary_vehicle) = self.vehicles.get(&prim_id) {
                            if let Some(position) = &primary_vehicle.global_position {
                                return Task::done(Message::SetViewPosition(
                                    Geodetic::new(position.lon, position.lat).as_mercator(),
                                ));
                            }
                        }
                    }
                }
            }
            ConnectionMessage::RecvError(error, link_id) => {
                log::error!("Error receiving on {link_id:?}: {error:?}");
            }
            ConnectionMessage::ChangeLinkVariant(variant) => {
                if self.link_config.builder.to_variant() == variant {
                    return Task::none();
                }

                self.link_config.builder = variant.to_default_builder();
                self.link_config.build = self.link_config.builder.try_build();
                if let Some(link_config) = self.link_config.build.clone() {
                    self.configuration.link_config = Some(link_config);
                    return Task::done(Message::SaveConfigurationToFile);
                }
            }
            ConnectionMessage::UpdateLinkBuilder(link_builder) => {
                self.link_config.builder = link_builder;
                self.link_config.build = self.link_config.builder.try_build();
                if let Some(link_config) = self.link_config.build.clone() {
                    self.configuration.link_config = Some(link_config);
                    return Task::done(Message::SaveConfigurationToFile);
                }
            }
            ConnectionMessage::DetectSerialPorts => {
                if let LinkBuilder::Serial {
                    available_ports, ..
                } = &mut self.link_config.builder
                {
                    match serialport::available_ports() {
                        Ok(ports) => {
                            *available_ports = ports
                                .into_iter()
                                .filter(|p| {
                                    matches!(p.port_type, serialport::SerialPortType::UsbPort(_))
                                })
                                .map(|p| p.port_name)
                                .collect();

                            available_ports.sort();
                        }
                        Err(error) => {
                            log::error!("Unable to fetch serial ports: {error}");
                        }
                    }

                    self.link_config.build = self.link_config.builder.try_build();
                }
            }
            ConnectionMessage::ConnectToLink(config) => {
                self.configuration.link_config = Some(config.clone());
                log::info!("Connecting to: {config:?}");
                return config.connect();
            }
            ConnectionMessage::DisconnectLink => {
                if let Some(connection) = self.connection.take() {
                    connection.close();
                    log::info!("Disconnected");
                }
            }
            ConnectionMessage::ConnectFailed(error) => {
                log::error!("Connect failed: {error:?}");
            }
            ConnectionMessage::ConnectSuccess(connection) => {
                log::info!("Connection established");
                let weak_connection = connection.downgrade();
                self.connection = Some(connection);
                tokio::spawn(async move {
                    loop {
                        let heartbeat = Heartbeat::default();
                        if !weak_connection.send_message(heartbeat).await {
                            return;
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                });
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let map = slippery::MapProgram::new(&self.tile_cache)
            .on_cache(Message::MapCache)
            .on_update(Message::MapProjector)
            .with_draw_layer(move |projector, frame| {
                for (_, vehicle) in &self.vehicles {
                    if let Some(pos) = &vehicle.global_position {
                        let center = projector.mercator_into_screen_space(
                            Geodetic::new(pos.lon, pos.lat).as_mercator(),
                        );

                        let yaw_angle = vehicle
                            .attitude
                            .as_ref()
                            .map(|att| att.attitude.euler_angles().2)
                            .unwrap_or_default();

                        let mut last_position = vehicle.global_positions.front().unwrap();
                        for this_position in vehicle.global_positions.iter().skip(1) {
                            let from = projector.geodetic_into_screen_space(Geodetic::new(
                                last_position.lon,
                                last_position.lat,
                            ));
                            let to = projector.geodetic_into_screen_space(Geodetic::new(
                                this_position.lon,
                                this_position.lat,
                            ));

                            last_position = this_position;

                            frame.stroke(
                                &iced::widget::canvas::Path::line(from, to),
                                Stroke::default().with_color(Color::BLACK).with_width(2.0),
                            );
                            frame.stroke(
                                &iced::widget::canvas::Path::circle(from, 1.0),
                                Stroke::default().with_color(Color::BLACK).with_width(3.0),
                            );
                        }

                        frame.with_save(|frame| {
                            frame.translate(iced::Vector::new(center.x, center.y));
                            frame.rotate(yaw_angle);

                            const WIDTH: f32 = 60.0;
                            frame.draw_image(
                                iced::Rectangle {
                                    x: -WIDTH / 2.0,
                                    y: -WIDTH / 2.0,
                                    width: WIDTH,
                                    height: WIDTH,
                                },
                                &*ARROW_HANDLE,
                            );
                        });
                    }
                }
            })
            .build(self.viewpoint);

        let overlay = iced::widget::column![
            iced::widget::opaque(self.view_top_panel()),
            iced::widget::row![
                iced::widget::opaque(self.view_param_list_scrollable()),
                iced::widget::space::Space::new().width(Length::Fill),
                iced::widget::opaque(self.view_right_side_panel()),
            ]
            .spacing(10.0),
        ]
        .padding(10.0)
        .spacing(10.0);

        stack!(map, overlay).into()
    }

    fn view_top_panel(&self) -> Element<'_, Message> {
        let mut row_contents = Vec::<Element<'_, Message>>::with_capacity(16);

        let selector = iced::widget::pick_list(
            Some(self.link_config.builder.to_variant()),
            LinkVariant::list(),
            ToString::to_string,
        )
        .placeholder("Pick one")
        .on_select(|v| Message::Connection(ConnectionMessage::ChangeLinkVariant(v)))
        .into();

        row_contents.push(selector);
        row_contents.push(iced::widget::rule::vertical(1.0).into());

        match &self.link_config.builder {
            LinkBuilder::Tcp { addr, port } => {
                let addr_input = text_input("address", addr)
                    .on_input(|addr| {
                        Message::Connection(ConnectionMessage::UpdateLinkBuilder(
                            LinkBuilder::Tcp {
                                addr,
                                port: port.clone(),
                            },
                        ))
                    })
                    .width(160.0)
                    .into();

                let port_input = text_input("port", port)
                    .on_input(|port| {
                        Message::Connection(ConnectionMessage::UpdateLinkBuilder(
                            LinkBuilder::Tcp {
                                addr: addr.clone(),
                                port,
                            },
                        ))
                    })
                    .width(100.0)
                    .into();

                row_contents.push(addr_input);
                row_contents.push(port_input);
            }
            LinkBuilder::Udp { addr, port } => {
                let addr_input = text_input("address", addr)
                    .on_input(|addr| {
                        Message::Connection(ConnectionMessage::UpdateLinkBuilder(
                            LinkBuilder::Udp {
                                addr,
                                port: port.clone(),
                            },
                        ))
                    })
                    .width(160.0)
                    .into();

                let port_input = text_input("port", port)
                    .on_input(|port| {
                        Message::Connection(ConnectionMessage::UpdateLinkBuilder(
                            LinkBuilder::Udp {
                                addr: addr.clone(),
                                port,
                            },
                        ))
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
                        .placeholder(if available_ports.is_empty() {
                            "No connection"
                        } else {
                            "Select a port"
                        })
                        .ellipsis(Ellipsis::Start)
                        .on_open(Message::Connection(ConnectionMessage::DetectSerialPorts))
                        .on_select(|selected_port| {
                            Message::Connection(ConnectionMessage::UpdateLinkBuilder(
                                LinkBuilder::Serial {
                                    port: Some(selected_port),
                                    baud: *baud,
                                    available_ports: available_ports.clone(),
                                },
                            ))
                        })
                        .width(160.0)
                        .into();

                let baud_picker = pick_list(Some(baud), connection::builder::BAUDRATES, |x| {
                    x.to_string()
                })
                .on_open(Message::Connection(ConnectionMessage::DetectSerialPorts))
                .on_select(|selected_baud| {
                    Message::Connection(ConnectionMessage::UpdateLinkBuilder(LinkBuilder::Serial {
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

        row_contents.push(iced::widget::rule::vertical(1.0).into());
        row_contents.push(iced::widget::text("Follow primary vehicle").into());
        row_contents.push(
            iced::widget::checkbox(self.follow_primary_vehicle)
                .on_toggle(Message::SetFollowPrimaryVehicle)
                .into(),
        );

        row_contents.push(iced::widget::space::Space::new().width(Length::Fill).into());

        if self.vehicles.len() > 1 {
            // TODO: We should use vendor and model name here instead
            let mut vehicle_ids = self.vehicles.keys().cloned().collect::<Vec<_>>();
            vehicle_ids.sort();
            let vehicle_picker = pick_list(self.primary_vehicle, vehicle_ids, |id| {
                self.vehicles
                    .get(id)
                    .map_or_else(|| "Invalid vehicle".to_string(), |v| v.pretty_name())
            })
            .on_select(Message::SetPrimaryVehicle);
            row_contents.push(vehicle_picker.into());
        }

        if self.connection.is_none() {
            let connect_button = Button::new(
                container("Connect")
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .on_press_maybe(self.link_config.build.as_ref().map(|config| {
                Message::Connection(ConnectionMessage::ConnectToLink(config.clone()))
            }))
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
            .on_press(Message::Connection(ConnectionMessage::DisconnectLink))
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
        .style(shaded_bordered_box)
        .width(Length::Fill)
        .padding(10.0)
        .into()
    }

    fn view_right_side_panel(&self) -> Element<'_, Message> {
        let mut row_contents = Vec::<Element<'_, Message>>::with_capacity(16);

        let mut roll = 0.0;
        let mut pitch = 0.0;
        let mut yaw = 0.0;

        if let Some(mav_id) = self.primary_vehicle {
            if let Some(vehicle) = self.vehicles.get(&mav_id) {
                if let Some(attitude) = vehicle.attitude.as_ref() {
                    (roll, pitch, yaw) = attitude.attitude.euler_angles();
                }
            }
        }

        row_contents.push(
            iced::widget::container(
                iced::widget::column![
                    iced::widget::text(format!("R: {roll}")),
                    iced::widget::text(format!("P: {pitch}")),
                    iced::widget::text(format!("Y: {yaw}"))
                ]
                .spacing(10.0),
            )
            .width(250.0)
            .into(),
        );

        if let Some(mav_id) = self.primary_vehicle {
            if let Some(vehicle) = self.vehicles.get(&mav_id) {
                use iced::font;
                use iced::widget::{rich_text, span};
                use iced::{Font, color, never};

                row_contents.push(iced::widget::rule::horizontal(1.0).into());
                row_contents.push(
                    iced::widget::scrollable(row![
                        iced::widget::Column::from_iter(
                            vehicle
                                .status_texts
                                .iter()
                                .enumerate()
                                .map(|(idx, status)| {
                                    let color = match status.severity {
                                        MavSeverity::Emergency => color!(0xff0000),
                                        MavSeverity::Alert => color!(0xff0000),
                                        MavSeverity::Critical => color!(0xff0000),
                                        MavSeverity::Error => color!(0xff0000),
                                        MavSeverity::Warning => color!(0xff8800),
                                        MavSeverity::Notice => color!(0xdddd00),
                                        MavSeverity::Info => color!(0x6666ff),
                                        MavSeverity::Debug => color!(0xaaaaaa),
                                    };
                                    iced::widget::container(row![
                                        rich_text![
                                            span(format!("[{:?}]", status.severity))
                                                .color(color)
                                                .font(Font {
                                                    weight: font::Weight::Bold,
                                                    ..Font::default()
                                                }),
                                            span(" "),
                                            span(&status.text),
                                        ]
                                        .on_link_click(never),
                                        iced::widget::Space::new().width(Length::Fill)
                                    ])
                                    .style(if idx % 2 == 0 {
                                        list_default
                                    } else {
                                        list_brighter
                                    })
                                    .padding(5.0)
                                    .into()
                                })
                        )
                        .spacing(10.0),
                    ])
                    .anchor_bottom()
                    .into(),
                );
            }
        }

        iced::widget::container(
            iced::widget::Column::from_vec(row_contents)
                .width(Length::Fill)
                .align_x(Alignment::Start)
                .spacing(10.0),
        )
        .style(shaded_bordered_box)
        .width(500.0)
        .padding(10.0)
        .into()
    }

    fn view_param_list_scrollable(&self) -> Element<'_, Message> {
        if self.vehicles.is_empty() {
            return space::Space::new().into();
        }

        iced::widget::container(
            iced::widget::scrollable(self.view_param_list())
                .style(iced::widget::scrollable::default)
                .spacing(0.0),
        )
        .style(shaded_bordered_box)
        .into()
    }

    fn view_param_list(&self) -> Element<'_, Message> {
        let Some(mav_id) = self.primary_vehicle else {
            return space::Space::new().into();
        };

        let Some(vehicle) = self.vehicles.get(&mav_id) else {
            return space::Space::new().into();
        };

        let mut entries = Vec::with_capacity(128);

        let reload_button = Button::new("Reload parameters")
            .on_press(parameter::Message::ListReload(mav_id).into())
            .into();

        let upload_button = Button::new("Upload changed parameters")
            .on_press(parameter::Message::UploadAll(mav_id).into())
            .into();

        let save_button = Button::new("Save parameters to file")
            .on_press_with(|| parameter::Message::SaveDialog(vehicle.parameters.clone()).into())
            .into();

        let load_button = Button::new("Load parameters from file")
            .on_press(parameter::Message::LoadDialog(mav_id).into())
            .into();

        let filter_field = TextInput::new("Filter parameters", &self.parameter_filter)
            .on_input(|buf| parameter::Message::FilterBuf(buf).into())
            .into();

        entries.push(reload_button);
        entries.push(upload_button);
        entries.push(save_button);
        entries.push(load_button);
        entries.push(filter_field);

        let got = vehicle.parameters.loading_state.has_loaded.len();
        let exp = vehicle.parameters.loading_state.expected_count;

        let style = if got < exp as usize {
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

        let parameters = self
            .parameter_filtered
            .as_ref()
            .unwrap_or(&vehicle.parameters);

        for (ident, param) in &parameters.map {
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

            let ident_owned = ident.clone();
            let text_input = iced::widget::TextInput::new("Write value", &value_string)
                .on_input(move |string| {
                    parameter::Message::BufferEdit(mav_id, ident_owned.clone(), string).into()
                })
                .style(move |theme, status| {
                    let mut style = iced::widget::text_input::default(theme, status);
                    match param.state {
                        ParamState::Unchanged => {}
                        ParamState::Changed(..) => {
                            style.background =
                                iced::Background::Color(Color::from_rgb8(30, 120, 60));
                        }
                        ParamState::Uploading(..) => {
                            style.background =
                                iced::Background::Color(Color::from_rgb8(180, 150, 0));
                        }
                    }
                    style
                });

            let ident_owned = ident.clone();
            let restore_button = Button::new("Restore")
                .on_press_maybe(match param.state {
                    ParamState::Changed(..) => {
                        Some(parameter::Message::ValueReset(mav_id, ident_owned.clone()).into())
                    }
                    _ => None,
                })
                .width(80.0);

            let commit_button = Button::new("Upload")
                .on_press_maybe(match param.state {
                    ParamState::Changed(value) => Some(
                        parameter::Message::ValueUpload(mav_id, ident_owned.clone(), value).into(),
                    ),
                    _ => None,
                })
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

        Column::from_vec(entries).spacing(5.0).padding(10.0).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
}

pub fn shaded_bordered_box(theme: &iced::Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weakest.color.into()),
        text_color: Some(palette.background.weakest.text),
        border: iced::Border {
            width: 1.0,
            radius: 6.0.into(),
            color: palette.background.strongest.color,
        },
        shadow: iced::Shadow {
            color: iced::Color::BLACK.scale_alpha(0.5),
            offset: iced::Vector::new(0.0, 1.0),
            blur_radius: 6.0,
        },
        ..container::Style::default()
    }
}

pub fn list_brighter(theme: &iced::Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weaker.color.into()),
        text_color: Some(palette.background.weakest.text),
        ..container::Style::default()
    }
}

pub fn list_default(theme: &iced::Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weakest.color.into()),
        text_color: Some(palette.background.weakest.text),
        ..container::Style::default()
    }
}
