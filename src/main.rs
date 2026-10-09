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
    Subscription, Task, Theme, Widget,
    alignment::Vertical,
    color::Oklch,
    futures::channel::mpsc::SendError,
    theme,
    widget::{
        Button, Column, ProgressBar, Space, Text, TextInput, button,
        canvas::{Gradient, Stroke, Style, gradient::Linear},
        container,
        image::Handle,
        pick_list, progress_bar, row, rule, space,
        text::Ellipsis,
        text_input, tooltip,
    },
};
use mav_param::Ident;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavModeFlag, MavSeverity},
        messages::{ComponentInformationBasic, Heartbeat},
    },
    error::SpecError,
};
use num_enum::FromPrimitive;
use rfd::AsyncFileDialog;
use slippery::{CacheMessage, Geodetic, Mercator, Projector, TileCache, Viewpoint, Zoom, location};

use crate::{
    connection::builder::{LinkBuilder, LinkConfig, LinkConfigurator, LinkId, LinkVariant},
    mav::MavlinkRouter,
    parameter::base::{
        ChangedStatus, MavlinkId, ParamState, Parameter, Parameters, value_as_string,
        value_type_name,
    },
    vehicle::{MavMessage, Vehicle},
    virtualized::virtual_list,
};

mod parameter;

mod config;
mod connection;
mod firmware;
mod mav;
mod vehicle;
mod virtualized;

fn main() -> Result<(), impl std::error::Error> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Error)
        .filter_module("groundhog", log::LevelFilter::Debug)
        .init();

    iced::application(Application::boot, Application::update, Application::view)
        .subscription(Application::subscription)
        .theme(Application::theme)
        .title(Application::title)
        .run()
}

type ArcError = Arc<dyn Error + Send + Sync + 'static>;
type BoxError = Box<dyn Error + Send + Sync + 'static>;
static ARROW_HANDLE: LazyLock<Handle> =
    LazyLock::new(|| Handle::from_bytes(include_bytes!("../assets/pointer.png").as_slice()));

/// Convert an altitude in meters to a corresponding color, for use with position trails.
/// The color sweeps from yellow -> green -> blue -> purple -> red as altitude goes from 0.0 -> 1000.0.
fn altitude_to_color(altitude_m: f32, brightness: f32) -> Color {
    const MIN_ALT_M: f32 = 0.0;
    const MAX_ALT_M: f32 = 1000.0;
    const YELLOW_HUE_DEG: f32 = 90.0;
    const PURPLE_HUE_DEG: f32 = 380.0;

    let t = ((altitude_m - MIN_ALT_M) / (MAX_ALT_M - MIN_ALT_M)).clamp(0.0, 1.0);
    let hue_deg = YELLOW_HUE_DEG + t * (PURPLE_HUE_DEG - YELLOW_HUE_DEG);

    Color::from(Oklch {
        l: brightness,
        c: 0.2 * brightness,
        h: hue_deg.to_radians(),
        a: 1.0,
    })
}

struct Link {
    id: LinkId,
    config: LinkConfig,
    handle: Option<mav::LinkGuard>,
}
struct Application {
    theme: Theme,
    viewpoint: Viewpoint,
    projector: Option<Projector>,
    tile_cache: TileCache,
    link_config: LinkConfigurator,
    router: MavlinkRouter,
    links: BTreeMap<String, Link>,
    configuration: config::Configuration,
    vehicles: BTreeMap<MavlinkId, Vehicle>,
    parameter_filter: parameter::ParameterFilter,
    parameter_filtered: Option<Parameters>,
    primary_vehicle: Option<MavlinkId>,
    follow_primary_vehicle: bool,
    draw_line_segment_dots: bool,
    parameter_namespace_sep: char,
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

impl From<SendError> for Message {
    fn from(_: SendError) -> Self {
        unreachable!("Why are we here ya dingus?")
    }
}

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
    #[allow(unused)] // TODO: Debug view
    DrawLineSegmentDots(bool),
}

impl From<parameter::Message> for Message {
    fn from(value: parameter::Message) -> Self {
        Message::Parameter(value)
    }
}

#[derive(Debug, Clone)]
enum ConnectionMessage {
    ConnectFailed(ArcError),
    ConnectSuccess(LinkId, mav::LinkGuard),
    Event(mav::Event),
    SaveLink(LinkId, LinkConfig),
    Connect(LinkId),
    Disconnect(LinkId),
    ChangeLinkName(String),
    SetLinkHighLatency(bool),
    SetLinkAutoconnect(bool),
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
    fn boot() -> (Self, Task<Message>) {
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
        let events = Task::stream(events).map(|event| ConnectionMessage::Event(event).into());

        let application = Application {
            theme: theme::Theme::GruvboxDark,
            viewpoint: Viewpoint {
                position: location::paris().as_mercator(),
                zoom: Zoom::try_from(8.0).unwrap(),
            },
            projector: None,
            tile_cache: TileCache::new(slippery::sources::OpenStreetMap),
            link_config: LinkConfigurator::new(link_builder, "Default".to_owned()),
            router: client,
            links: BTreeMap::new(),
            configuration: config.clone(),
            vehicles: BTreeMap::new(),
            parameter_filter: parameter::ParameterFilter::default(),
            parameter_filtered: None,
            primary_vehicle: None,
            follow_primary_vehicle: false,
            draw_line_segment_dots: false,
            parameter_namespace_sep: '_',
        };

        (application, events)
    }

    fn theme(&self) -> Theme {
        self.theme.clone()
    }

    fn title(&self) -> String {
        "Holsatus Groundhog".to_string()
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
            Message::SetFollowPrimaryVehicle(value) => {
                self.follow_primary_vehicle = value;
            }
            Message::DrawLineSegmentDots(value) => {
                self.draw_line_segment_dots = value;
            }
        }

        None
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
            move |result| parameter::Message::ListReloadResult(mav_id, result).into(),
        )
    }

    fn connection_message_update(&mut self, message: ConnectionMessage) -> Task<Message> {
        match message {
            ConnectionMessage::Event(mav::Event::Message(inbound)) => {
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
                        Err(error) => {
                            match error {
                                mavio::Error::Spec(SpecError::NotInDialect(id)) => {
                                    // Downgrade this error to trace to avoid spamming the logs
                                    log::trace!("mavlink message id {id} is not in default dialect")
                                }
                                error => {
                                    log::trace!("could not decode mavlink frame: {error}");
                                }
                            }
                        }
                    }
                }

                if self.primary_vehicle.is_some_and(|id| id == mav_id) {
                    self.refresh_filtered_parameter();

                    if self.follow_primary_vehicle
                        && let Some(vehicle) = self.vehicles.get(&mav_id)
                        && let Some(position) = &vehicle.global_position
                    {
                        tasks.push(Task::done(Message::SetViewPosition(position.pos)));
                    }
                }

                return Task::batch(tasks);
            }
            ConnectionMessage::Event(mav::Event::LinkClosed(link_id)) => {
                log::info!("Link {link_id:?} closed");
                // self.links.retain(|link| link.id != link_id);
            }
            ConnectionMessage::ChangeLinkName(name) => {
                self.link_config.name = name;
            }
            ConnectionMessage::SetLinkHighLatency(high_latency) => {
                self.link_config.high_latency = high_latency;
            }
            ConnectionMessage::SetLinkAutoconnect(auto_connect) => {
                self.link_config.auto_connect = auto_connect;
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
            ConnectionMessage::SaveLink(link_id, config) => {
                self.configuration.link_config = Some(config.build.clone());

                self.links.insert(
                    config.name.clone(),
                    Link {
                        id: link_id,
                        config,
                        handle: None,
                    },
                );
            }
            ConnectionMessage::Connect(link_id) => {
                if let Some(link) = self.links.get(link_id.str()) {
                    return link
                        .config
                        .build
                        .connect(link_id.clone(), self.router.clone());
                }
            }
            ConnectionMessage::Disconnect(link_id) => {
                if let Some(link) = self.links.values_mut().find(|this| this.id == link_id) {
                    drop(link.handle.take())
                }
            }
            ConnectionMessage::ConnectFailed(error) => {
                log::error!("Connect failed: {error:?}");
            }
            ConnectionMessage::ConnectSuccess(link_id, handle) => {
                log::info!("Connection established");
                if let Some(link) = self.links.values_mut().find(|link| link.id == link_id) {
                    link.handle = Some(handle)
                }
            }
        }

        Task::none()
    }

    fn view(&self) -> impl Widget<Message> + '_ {
        let overlay = iced::widget::column![
            iced::widget::opaque(self.view_top_panel()),
            iced::widget::row![
                iced::widget::opaque(self.view_parameter_panel()),
                iced::widget::opaque(self.view_links_editor()),
                iced::widget::opaque(self.view_links_status()),
                iced::widget::space().width(Length::Fill),
                iced::widget::opaque(self.view_right_side_panel()),
            ]
            .spacing(10.0),
        ]
        .padding(10.0)
        .spacing(10.0);

        iced::widget::stack!(self.view_osm_map(), overlay)
    }

    fn view_links_editor(&self) -> impl Widget<Message> {
        let link = &self.link_config;

        let name_editor = iced::widget::text_input("Link name", &link.name)
            .on_input(|name| ConnectionMessage::ChangeLinkName(name).into());

        let high_latency = iced::widget::checkbox(link.high_latency)
            .on_toggle(|value| ConnectionMessage::SetLinkHighLatency(value).into())
            .label("High latency");

        let auto_connect = iced::widget::checkbox(link.auto_connect)
            .on_toggle(|value| ConnectionMessage::SetLinkAutoconnect(value).into())
            .label("Auto connect");

        let link_kind = iced::widget::pick_list(
            Some(link.builder.to_variant()),
            LinkVariant::list(),
            LinkVariant::to_string,
        )
        .width(Length::Fill)
        .on_select(|kind| ConnectionMessage::ChangeLinkVariant(kind).into());

        let mut children = Vec::with_capacity(2);

        match &link.builder {
            LinkBuilder::Tcp { addr, port } => {
                let addr_input = text_input("address", addr)
                    .on_input(|addr| {
                        ConnectionMessage::UpdateLinkBuilder(LinkBuilder::Tcp {
                            addr,
                            port: port.clone(),
                        })
                        .into()
                    })
                    .width(Length::Fill)
                    .boxed();

                let port_input = text_input("port", port)
                    .on_input(|port| {
                        ConnectionMessage::UpdateLinkBuilder(LinkBuilder::Tcp {
                            addr: addr.clone(),
                            port,
                        })
                        .into()
                    })
                    .width(Length::Fill)
                    .boxed();

                children.push(addr_input);
                children.push(port_input);
            }
            LinkBuilder::Udp { addr, port } => {
                let addr_input = text_input("address", addr)
                    .on_input(|addr| {
                        ConnectionMessage::UpdateLinkBuilder(LinkBuilder::Udp {
                            addr,
                            port: port.clone(),
                        })
                        .into()
                    })
                    .width(Length::Fill)
                    .boxed();

                let port_input = text_input("port", port)
                    .on_input(|port| {
                        ConnectionMessage::UpdateLinkBuilder(LinkBuilder::Udp {
                            addr: addr.clone(),
                            port,
                        })
                        .into()
                    })
                    .width(Length::Fill)
                    .boxed();

                children.push(addr_input);
                children.push(port_input);
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
                        .on_open(ConnectionMessage::DetectSerialPorts.into())
                        .on_select(|selected_port| {
                            ConnectionMessage::UpdateLinkBuilder(LinkBuilder::Serial {
                                port: Some(selected_port),
                                baud: *baud,
                                available_ports: available_ports.clone(),
                            })
                            .into()
                        })
                        .width(Length::Fill)
                        .boxed();

                let baud_picker =
                    pick_list(Some(baud), connection::builder::BAUDRATES, u32::to_string)
                        .on_select(|selected_baud| {
                            ConnectionMessage::UpdateLinkBuilder(LinkBuilder::Serial {
                                port: port.clone(),
                                baud: selected_baud,
                                available_ports: available_ports.clone(),
                            })
                            .into()
                        })
                        .width(Length::Fill)
                        .boxed();

                children.push(port_picker);
                children.push(baud_picker);
            }
        }

        let button = Button::new(
            container("Save link")
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .on_press_maybe(self.link_config.parse().map(|config| {
            Message::Connection(ConnectionMessage::SaveLink(
                LinkId::new(&self.link_config.name),
                config,
            ))
        }))
        .width(Length::Fill)
        .boxed();

        let contents = iced::widget::column![
            name_editor,
            high_latency,
            auto_connect,
            link_kind,
            iced::widget::Column::from_vec(children).spacing(10.0),
            button,
        ]
        .spacing(10.0);

        container(contents)
            .style(shaded_bordered_box)
            .padding(10.0)
            .width(170.0)
    }

    fn view_links_status<'a>(&'a self) -> impl Widget<Message> + 'a {
        let mut contents = Vec::new();

        for link in self.links.values() {
            let button = if link.handle.is_some() {
                Button::new(
                    container("Disconnect")
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center),
                )
                .style(button::danger)
                .on_press_with(|| {
                    Message::Connection(ConnectionMessage::Disconnect(link.id.clone()))
                })
                .width(100.0)
            } else {
                Button::new(
                    container("Connect")
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center),
                )
                .on_press_with(|| Message::Connection(ConnectionMessage::Connect(link.id.clone())))
                .width(100.0)
            };

            let content = iced::widget::container(iced::widget::row![
                iced::widget::text(&link.config.name),
                iced::widget::text(if link.handle.is_some() {
                    ": connected"
                } else {
                    ": disconnected"
                }),
                button
            ])
            .style(shaded_bordered_box)
            .padding(10.0);
            contents.push(content.boxed());
        }

        iced::widget::Column::from_vec(contents).spacing(10.0)
    }

    fn view_osm_map(&self) -> Element<'_, Message> {
        slippery::MapProgram::new(&self.tile_cache)
            .on_cache(Message::MapCache)
            .on_update(Message::MapProjector)
            .with_draw_layer(|projector, frame| {
                // Darken the map background to make overlay stand out more
                let mut black_fill = iced::widget::canvas::Fill::default();
                black_fill.style = Style::Solid(Color::BLACK.scale_alpha(0.5));
                frame.fill_rectangle([0.0, 0.0].into(), frame.size(), black_fill);

                iced::debug::time_with("position_segments_view", || {
                    for (_, vehicle) in &self.vehicles {
                        if let Some(position) = &vehicle.global_position {
                            let center = projector.mercator_into_screen_space(position.pos);

                            let yaw_angle = vehicle
                                .attitude
                                .as_ref()
                                .map(|att| att.attitude.euler_angles().2)
                                .unwrap_or_default();

                            struct ScreenPosition {
                                point: iced::Point,
                                dark: iced::Color,
                                light: iced::Color,
                                time: Instant,
                            }

                            let screen_points = vehicle
                                .global_positions
                                .iter()
                                .map(|position| ScreenPosition {
                                    point: projector.mercator_into_screen_space(position.pos),
                                    dark: altitude_to_color(position.alt_agl, 0.4),
                                    light: altitude_to_color(position.alt_agl, 0.85),
                                    time: position.at,
                                })
                                .collect::<Vec<_>>();

                            struct DrawLineSegment {
                                from_point: iced::Point,
                                to_point: iced::Point,
                                from_color: iced::Color,
                                to_color: iced::Color,
                                faint: bool,
                                width: f32,
                            }

                            impl DrawLineSegment {
                                fn draw(self, frame: &mut iced::widget::canvas::Frame) {
                                    let alpha = if self.faint { 0.25 } else { 1.0 };
                                    let mut stroke = Stroke::default()
                                        .with_width(self.width)
                                        .with_line_cap(iced::widget::canvas::LineCap::Round);
                                    stroke.style = Style::Gradient(Gradient::Linear(
                                        Linear::new(self.from_point, self.to_point)
                                            .add_stop(0.0, self.from_color.scale_alpha(alpha))
                                            .add_stop(1.0, self.to_color.scale_alpha(alpha)),
                                    ));

                                    frame.stroke(
                                        &iced::widget::canvas::Path::line(
                                            self.from_point,
                                            self.to_point,
                                        ),
                                        stroke,
                                    );
                                }
                            }

                            let mut points_iterator = screen_points.iter();
                            let Some(mut prev) = points_iterator.next() else {
                                return;
                            };

                            // First draw all the dark background sections
                            for next in points_iterator {
                                let long_time = prev.time.saturating_duration_since(next.time)
                                    > Duration::from_secs(30);

                                DrawLineSegment {
                                    from_point: prev.point,
                                    from_color: prev.dark,
                                    to_point: next.point,
                                    to_color: next.dark,
                                    faint: long_time,
                                    width: 10.0,
                                }
                                .draw(frame);

                                prev = next;
                            }

                            let mut points_iterator = screen_points.iter();
                            let Some(mut prev) = points_iterator.next() else {
                                return;
                            };

                            // Then draw all the light foreground sections
                            for next in points_iterator {
                                let long_time = prev.time.saturating_duration_since(next.time)
                                    > Duration::from_secs(30);

                                DrawLineSegment {
                                    from_point: prev.point,
                                    from_color: prev.light,
                                    to_point: next.point,
                                    to_color: next.light,
                                    faint: long_time,
                                    width: 4.0,
                                }
                                .draw(frame);

                                if self.draw_line_segment_dots {
                                    frame.fill(
                                        &iced::widget::canvas::Path::circle(prev.point, 5.0),
                                        iced::widget::canvas::Fill::from(iced::Color::WHITE),
                                    );
                                }

                                prev = next;
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
            })
            .build(self.viewpoint)
    }

    fn view_top_panel(&self) -> impl Widget<Message> + '_ {
        let mut row_contents = Vec::<Element<'_, Message>>::with_capacity(16);

        let selector = iced::widget::pick_list(
            Some(self.link_config.builder.to_variant()),
            LinkVariant::list(),
            ToString::to_string,
        )
        .placeholder("Pick one")
        .on_select(|v| Message::Connection(ConnectionMessage::ChangeLinkVariant(v)))
        .boxed();

        row_contents.push(selector);
        row_contents.push(iced::widget::rule::vertical(1.0).boxed());

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
                    .boxed();

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
                    .boxed();

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
                    .boxed();

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
                    .boxed();

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
                        .boxed();

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
                .boxed();

                row_contents.push(port_picker);
                row_contents.push(baud_picker);
            }
        }

        row_contents.push(iced::widget::rule::vertical(1.0).boxed());
        row_contents.push(iced::widget::text("Follow primary vehicle").boxed());
        row_contents.push(
            iced::widget::checkbox(self.follow_primary_vehicle)
                .on_toggle(Message::SetFollowPrimaryVehicle)
                .boxed(),
        );

        // row_contents.push(iced::widget::rule::vertical(1.0).into());
        // row_contents.push(iced::widget::text("Draw line segment dots").into());
        // row_contents.push(
        //     iced::widget::checkbox(self.draw_line_segment_dots)
        //         .on_toggle(Message::DrawLineSegmentDots)
        //         .into(),
        // );

        row_contents.push(
            iced::widget::space::Space::new()
                .width(Length::Fill)
                .boxed(),
        );

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
            row_contents.push(vehicle_picker.boxed());
        }

        iced::widget::container(
            row::Row::from_vec(row_contents)
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .spacing(10.0)
                .height(32.0),
        )
        .style(shaded_bordered_box)
        .width(Length::Fill)
        .padding(10.0)
    }

    fn view_right_side_panel(&self) -> impl Widget<Message> + '_ {
        let mut row_contents = Vec::<Element<'_, Message>>::with_capacity(16);

        let mut roll = 0.0;
        let mut pitch = 0.0;
        let mut yaw = 0.0;

        let mut voltage = 0.0;
        let mut current = 0.0;
        let mut charge = 0.0;

        let mut flight_mode = "Unknown".to_string();
        let mut mav_state = "Unknown".to_string();
        let mut armed = "Unknown".to_string();

        if let Some(mav_id) = self.primary_vehicle {
            if let Some(vehicle) = self.vehicles.get(&mav_id) {
                if let Some(attitude) = vehicle.attitude.as_ref() {
                    (roll, pitch, yaw) = attitude.attitude.euler_angles();
                }

                if let Some(battery) = vehicle.battery_state.as_ref() {
                    voltage = battery.voltage;
                    current = battery.current;
                    charge = battery.charge;
                }

                if let Some(message) = vehicle.latest_message.get(&Heartbeat::ID) {
                    if let DefaultDialect::Heartbeat(hb) = &message.message {
                        match hb.autopilot {
                            mavio::dialects::minimal::enums::MavAutopilot::Ardupilotmega => {
                                match hb.type_ {
                                    mavio::dialects::minimal::enums::MavType::FixedWing
                                    | mavio::dialects::minimal::enums::MavType::VtolTiltrotor => {
                                        let mode =
                                            firmware::ardupilot::flight_mode::Plane::from_primitive(
                                                hb.custom_mode as u8,
                                            );

                                        flight_mode = format!("{mode:?}");
                                    }
                                    mavio::dialects::minimal::enums::MavType::Quadrotor
                                    | mavio::dialects::minimal::enums::MavType::Helicopter => {
                                        let mode =
                                            firmware::ardupilot::flight_mode::Copter::from_primitive(
                                                hb.custom_mode as u8,
                                            );

                                        flight_mode = format!("{mode:?}");
                                    }
                                    _ => (),
                                }
                            }
                            _ => (),
                        }

                        mav_state = format!("{:?}", hb.system_status);
                        armed = format!("{}", hb.base_mode.intersects(MavModeFlag::SAFETY_ARMED));
                    }
                }
            }
        }

        row_contents.push(
            iced::widget::row![
                iced::widget::container(iced::widget::text(format!("armed: {armed}")),)
                    .width(Length::FillPortion(1)),
                iced::widget::container(iced::widget::text(format!("state: {mav_state}")),)
                    .width(Length::FillPortion(1)),
                iced::widget::container(iced::widget::text(format!("mode: {flight_mode}")),)
                    .width(Length::FillPortion(1)),
            ]
            .width(Length::Fill)
            .spacing(10.0)
            .boxed(),
        );

        row_contents.push(iced::widget::rule::horizontal(1.0).boxed());

        row_contents.push(
            iced::widget::row![
                iced::widget::container(iced::widget::text(format!(
                    "roll: {:.5} deg",
                    roll.to_degrees()
                )),)
                .width(Length::FillPortion(1)),
                iced::widget::container(iced::widget::text(format!(
                    "pitch: {:.5} deg",
                    pitch.to_degrees()
                )),)
                .width(Length::FillPortion(1)),
                iced::widget::container(iced::widget::text(format!(
                    "yaw: {:.5} deg",
                    yaw.to_degrees()
                )),)
                .width(Length::FillPortion(1)),
            ]
            .width(Length::Fill)
            .spacing(10.0)
            .boxed(),
        );

        row_contents.push(
            iced::widget::row![
                iced::widget::container(iced::widget::text(format!("volt: {voltage:.5} V")),)
                    .width(Length::FillPortion(1)),
                iced::widget::container(iced::widget::text(format!("curr: {current:.5} A")),)
                    .width(Length::FillPortion(1)),
                iced::widget::container(iced::widget::text(format!("chrg: {charge:.5} %")),)
                    .width(Length::FillPortion(1)),
            ]
            .width(Length::Fill)
            .spacing(10.0)
            .boxed(),
        );

        if let Some(mav_id) = self.primary_vehicle {
            if let Some(vehicle) = self.vehicles.get(&mav_id) {
                use iced::font;
                use iced::widget::{rich_text, span};
                use iced::{Font, never};

                if let Some(MavMessage {
                    message: DefaultDialect::Heartbeat(heartbeat),
                    ..
                }) = vehicle.latest_message.get(&Heartbeat::ID)
                {
                    row_contents.push(iced::widget::rule::horizontal(1.0).boxed());
                    row_contents.push(
                        iced::widget::container(iced::widget::text(format!("{:?}", heartbeat)))
                            .boxed(),
                    );
                }

                row_contents.push(iced::widget::rule::horizontal(1.0).boxed());
                row_contents.push(
                    iced::widget::scrollable(row![
                        iced::widget::Column::from_iter(
                            vehicle
                                .status_texts
                                .iter()
                                .enumerate()
                                .map(|(idx, status)| {
                                    let palette = self.theme.palette();
                                    let color = match status.severity {
                                        MavSeverity::Emergency => palette.danger.base.color,
                                        MavSeverity::Alert => palette.danger.base.color,
                                        MavSeverity::Critical => palette.danger.base.color,
                                        MavSeverity::Error => palette.danger.base.color,
                                        MavSeverity::Warning => palette.warning.base.color,
                                        MavSeverity::Notice => palette.warning.base.color,
                                        MavSeverity::Info => palette.primary.base.color,
                                        MavSeverity::Debug => palette.secondary.base.color,
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
                                })
                        )
                        .spacing(10.0),
                    ])
                    .anchor_bottom()
                    .boxed(),
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
    }

    fn view_parameter_panel(&self) -> impl Widget<Message> + '_ {
        let Some(mav_id) = self.primary_vehicle else {
            return space::Space::new().boxed();
        };

        let Some(vehicle) = self.vehicles.get(&mav_id) else {
            return space::Space::new().boxed();
        };

        let mut entries: Vec<Element<'_, Message>> = Vec::with_capacity(128);

        // Disabled while a parameter list download is in progress
        let reloading = self.router.is_busy(&mav::LockKey::ParamList(mav_id));
        let reload_button = Button::new("Reload parameters")
            .width(Length::Fill)
            .on_press_maybe((!reloading).then(|| parameter::Message::ListReload(mav_id).into()));

        let upload_button = Button::new("Upload parameters")
            .width(Length::FillPortion(1))
            .on_press(parameter::Message::UploadAll(mav_id).into());

        let save_button = Button::new("Save parameters to file")
            .width(Length::FillPortion(1))
            .on_press_with(|| parameter::Message::SaveDialog(vehicle.parameters.clone()).into());

        let load_button = Button::new("Load parameters from file")
            .width(Length::FillPortion(1))
            .on_press(parameter::Message::LoadDialog(mav_id).into());

        let filter_field =
            TextInput::new("Filter parameters", &self.parameter_filter.ident_pattern)
                .width(Length::FillPortion(1))
                .on_input(|buf| parameter::Message::FilterIdentPattern(buf).into());

        entries.push(
            iced::widget::row![reload_button, upload_button]
                .spacing(5.0)
                .boxed(),
        );
        entries.push(
            iced::widget::row![save_button, load_button]
                .spacing(5.0)
                .boxed(),
        );

        entries.push(filter_field.boxed());

        entries.push(
            iced::widget::checkbox(self.parameter_filter.only_changed)
                .label("Only changed")
                .on_toggle(|changed| {
                    Message::Parameter(parameter::Message::FilterOnlyChanged(changed))
                })
                .boxed(),
        );

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
            .boxed();

        entries.push(progress);

        iced::widget::container(
            iced::widget::column![
                iced::widget::Column::from_iter(entries).spacing(5.0),
                self.view_param_list(mav_id, vehicle),
            ]
            .spacing(5.0),
        )
        .padding(10.0)
        .width(500.0)
        .style(shaded_bordered_box)
        .boxed()
    }

    fn view_param_list<'a>(
        &'a self,
        mav_id: MavlinkId,
        vehicle: &'a Vehicle,
    ) -> impl Widget<Message> + 'a {
        /// Heights must fit the tallest content of each item kind
        const ROW_HEIGHT: f32 = 32.0;
        const HEADER_HEIGHT: f32 = 40.0;
        const SEPARATOR_HEIGHT: f32 = 1.0;

        enum Item<'a> {
            /// `first` is the ident following the section, used as its key
            Section {
                name: Option<&'a str>,
                separator: bool,
                first: &'a Ident,
            },
            Param(&'a Ident, &'a Parameter),
        }

        let parameters = self
            .parameter_filtered
            .as_ref()
            .unwrap_or(&vehicle.parameters);

        let mut items = Vec::with_capacity(parameters.map.len() + 64);
        let mut section = None;

        for (ident, param) in &parameters.map {
            let this_section = ident
                .as_str()
                .split_once(self.parameter_namespace_sep)
                .map(|(sec, _)| sec);

            // Add larger section headers and separators
            if this_section != section {
                let separator = section.is_some();
                if separator || this_section.is_some() {
                    items.push(Item::Section {
                        name: this_section,
                        separator,
                        first: ident,
                    });
                }
                section = this_section;
            }

            items.push(Item::Param(ident, param));
        }

        let list = virtual_list(items, move |item| match item {
            Item::Section {
                name, separator, ..
            } => {
                let header = name.map(|name| -> Text<'_, iced::Theme> {
                    Text::new(format!("[{name}]"))
                        .size(24.0)
                        .font(Font::MONOSPACE)
                });
                Column::<Element<'_, Message>>::new()
                    .push(separator.then(|| rule::horizontal(1.0)).boxed())
                    .push(header.boxed())
                    .spacing(5.0)
                    .boxed()
            }
            Item::Param(ident, param) => self.view_param_row(mav_id, ident, param).boxed(),
        })
        .item_height(|item| match item {
            Item::Section { name: None, .. } => SEPARATOR_HEIGHT,
            Item::Section { .. } => HEADER_HEIGHT,
            Item::Param(..) => ROW_HEIGHT,
        })
        .key(|item| match item {
            Item::Section { first, .. } => ((*first).clone(), true),
            Item::Param(ident, _) => ((*ident).clone(), false),
        })
        .spacing(5.0);

        iced::widget::scrollable(list)
            .style(iced::widget::scrollable::default)
            .spacing(5.0)
            .boxed()
    }

    fn view_param_row<'a>(
        &'a self,
        mav_id: MavlinkId,
        ident: &'a Ident,
        param: &'a Parameter,
    ) -> impl Widget<Message> {
        let type_name = value_type_name(param.value);

        let value_string = match param.editing.clone() {
            Some(buffer) => buffer.clone(),
            None => value_as_string(param.value),
        };

        let text_input = iced::widget::TextInput::new("Write value", value_string)
            .on_input(move |string| {
                parameter::Message::BufferEdit(mav_id, ident.clone(), string).into()
            })
            .style(move |theme, status| {
                let mut style = iced::widget::text_input::default(theme, status);
                let palette = theme.palette();
                match &param.state {
                    ParamState::Unchanged => {}
                    ParamState::Changed { status, .. } => {
                        style.background = match status {
                            ChangedStatus::None => palette.success.base.color,
                            ChangedStatus::Failed => palette.warning.base.color,
                            ChangedStatus::Rejected => palette.danger.base.color,
                        }
                        .into();
                    }
                    ParamState::Uploading { .. } => {
                        style.background = palette.secondary.base.color.into();
                    }
                }
                style
            })
            .width(Length::Fill);

        let restore_button = Button::new("Restore")
            .on_press_maybe(match param.state {
                ParamState::Changed { .. } => {
                    Some(parameter::Message::ValueReset(mav_id, ident.clone()).into())
                }
                _ => None,
            })
            .width(80.0);

        let restore_tooltip = container(Text::new(match param.state {
            ParamState::Changed { .. } => {
                format!(
                    "Restore to: {}",
                    parameter::base::value_as_string(param.value)
                )
            }
            _ => "Nothing to restore".to_string(),
        }))
        .padding(10.0)
        .style(shaded_bordered_box);

        let restore_button = tooltip(restore_button, restore_tooltip, Default::default())
            .delay(Duration::from_millis(250));

        let upload_button = Button::new("Upload")
            .on_press_maybe(match param.state {
                ParamState::Changed { value, .. } => {
                    Some(parameter::Message::ValueUpload(mav_id, ident.clone(), value).into())
                }
                _ => None,
            })
            .width(80.0);

        row![
            Text::new(ident.as_str().to_string())
                .width(160.0)
                .font(Font::MONOSPACE),
            Text::new(type_name)
                .width(50.0)
                .font(Font::MONOSPACE)
                .align_x(Alignment::End)
                .color(Color::from_rgba8(255, 255, 255, 0.5)),
            Space::new().width(10.0),
            text_input,
            restore_button,
            upload_button,
        ]
        .spacing(5.0)
        .align_y(Vertical::Center)
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

pub fn darken(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::color::Color::BLACK.scale_alpha(0.5).into()),
        text_color: None,
        ..container::Style::default()
    }
}

pub fn haversine_distance(p1: Geodetic, p2: Geodetic) -> f64 {
    const R: f64 = 6371000.0;

    // Convert degrees to radians
    let phi1 = p1.latitude().to_radians();
    let phi2 = p2.latitude().to_radians();
    let delta_phi = (p2.latitude() - p1.latitude()).to_radians();
    let delta_lambda = (p2.longitude() - p1.longitude()).to_radians();

    // Haversine core formula
    let a = (delta_phi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (delta_lambda / 2.0).sin().powi(2);

    let c = 2.0 * (a.sqrt()).atan2((1.0 - a).sqrt());

    // Distance in meters
    // THe additional factor is not correct, but works with AP?
    (R * c) * (0.5 + phi1.cos() / 2.0)
}
