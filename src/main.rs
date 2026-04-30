use std::{
    collections::{
        BTreeMap,
        btree_map::{self},
    },
    error::Error,
    path::PathBuf,
    sync::{Arc, atomic::AtomicU16},
    time::{Duration, Instant},
};

use iced::{
    Alignment, Color, Element, Font, Length, Task,
    alignment::Vertical,
    widget::{
        Button, Column, ProgressBar, Space, Text, TextInput, button, column, container, pick_list,
        progress_bar, row, rule, space, stack, text::Ellipsis, text_input,
    },
};
use mav_param::{Ident, Value};
use mavio::{
    Frame,
    default_dialect::{
        enums::{MavCmd, MavProtocolCapability},
        messages::{AutopilotVersion, CommandInt, Heartbeat, ParamRequestList, ParamSet},
    },
    prelude::Versionless,
};
use rfd::AsyncFileDialog;
use slippery::{
    CacheMessage, Projector, TileCache, Viewpoint, Zoom, location, sources::OpenStreetMap,
};

mod parameters;
use crate::{
    connection::{ConnectionHandle, LinkBuilder, LinkConfig, LinkId, LinkVariant},
    parameters::{MavlinkId, ParamState, Parameter, Parameters, value_as_string, value_type_name},
    vehicle::Vehicle,
};

mod connection;
mod vehicle;
mod config;


fn main() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("groundhog", log::LevelFilter::Debug)
        .init();

    iced::application(Application::boot, Application::update, Application::view)
        .title("Holsatus Groundhog")
        .scale_factor(|_| 1.0)
        .run()
        .expect("Groundhog died");
}

type ArcError = Arc<dyn Error + Send + Sync + 'static>;
type BoxError = Box<dyn Error + Send + Sync + 'static>;

struct Application {
    viewpoint: Viewpoint,
    projector: Option<Projector>,
    tile_cache: TileCache,
    link_builder: LinkBuilder,
    link_config: Option<LinkConfig>,
    connection: Option<ConnectionHandle>,
    configuration: config::Configuration,
    vehicles: BTreeMap<MavlinkId, Vehicle>,
    parameter_filter: String,
    parameter_filtered: Option<Parameters>,
    primary_vehicle: Option<MavlinkId>,
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
    Noop,

    MapProjector(Projector),
    MapCache(CacheMessage),
    Conn(ConnMessage),

    SaveConfigurationToFile,

    #[allow(unused)] // TODO
    UpdateAndSaveConfiguration(config::Configuration),

    SetPrimaryVehicle(MavlinkId),

    /// Filter for only certain parameters
    ParamFilterBuf(String),

    /// Initiate a paramter list request to the target
    ParamListReload(MavlinkId),

    /// Upload all modified parameters to the target
    ParamUploadAll(MavlinkId),

    /// Reset a local parameter to its target-default
    ParamValueReset(MavlinkId, Ident),

    /// Upload a local parameter to the target
    ParamValueUpload(MavlinkId, Ident, Value),

    /// Upload a local parameter to the target
    ParamValueUploadTimeout(MavlinkId, Ident),

    /// Edit the text-value buffer of the local parameter
    ParamBufferEdit(MavlinkId, Ident, String),

    ParamSaveDialog(Parameters),
    ParamLoadDialog(MavlinkId),

    ParamSaveToFile(PathBuf, Parameters),
    ParamLoadFromFile(PathBuf, MavlinkId),
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

impl From<ConnMessage> for Message {
    fn from(value: ConnMessage) -> Self {
        Message::Conn(value)
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
            tile_cache: TileCache::new(OpenStreetMap),
            link_builder: link_builder.clone(),
            link_config: link_builder.try_build(),
            connection: None,
            configuration: config.clone(),
            vehicles: BTreeMap::new(),
            parameter_filter: String::new(),
            parameter_filtered: None,

            primary_vehicle: None,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        self.maybe_update(message).unwrap_or_default()
    }

    fn maybe_update(&mut self, message: Message) -> Option<Task<Message>> {
        match message {
            Message::Noop => (),
            Message::SaveConfigurationToFile => {
                if let Err(error) = self.configuration.write_to_file() {
                    log::error!("Unable to save configuration file: {}", error);
                }
            }
            Message::UpdateAndSaveConfiguration(config) => {
                self.configuration = config;
                return Some(Task::done(Message::SaveConfigurationToFile));
            }
            Message::MapProjector(projector) => {
                self.viewpoint = projector.viewpoint;
                self.projector = Some(projector);
            }
            Message::MapCache(message) => {
                let map_task = self.tile_cache.update(message);
                return Some(map_task.map(Message::MapCache));
            }
            Message::ParamFilterBuf(buffer) => {
                self.parameter_filter = buffer;

                if self.parameter_filter.is_empty() {
                    self.parameter_filtered = None;
                    return None;
                }

                // TODO: Current filtering assumes we have a single vehicle
                let vehicle = self.vehicles.first_key_value()?.1;

                let param_map = vehicle.params.map.iter().filter_map(|(ident, param)| {
                    ident
                        .as_str()
                        .contains(&self.parameter_filter)
                        .then_some((ident.clone(), param.clone()))
                });

                self.parameter_filtered = Some(Parameters {
                    map: param_map.collect(),
                    loading_state: vehicle.params.loading_state.clone(),
                });
            }
            Message::Conn(message) => return Some(self.update_conn(message)),
            Message::ParamListReload(mav_id) => {
                let connection = self.connection.as_ref()?.downgrade();
                let vehicle = self.vehicles.get_mut(&mav_id)?;

                let get_capabilities = vehicle.capabilities.is_none();
                vehicle.params.loading_state.has_loaded.clear();

                return Some(Task::future(async move {
                    if get_capabilities {
                        connection
                            .send_message(CommandInt {
                                target_system: mav_id.system,
                                target_component: mav_id.component,
                                command: MavCmd::RequestMessage,
                                param1: AutopilotVersion::ID as f32,
                                ..Default::default()
                            })
                            .await;

                        // Give MAV some time to respond in right order
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }

                    connection
                        .send_message(ParamRequestList {
                            target_system: mav_id.system,
                            target_component: mav_id.component,
                        })
                        .await;

                    Message::Noop
                }));
            }
            Message::ParamUploadAll(mav_id) => {
                let connection = self.connection.as_ref()?.downgrade();
                let vehicle = self.vehicles.get_mut(&mav_id)?;

                let mut timeout_tasks = Vec::new();

                // Loop over all changed parameters and do a param set request.
                // Mark the parameter as uploading to keep track of things.
                for (ident, param, value) in
                    vehicle
                        .params
                        .map
                        .iter_mut()
                        .filter_map(|(ident, param)| match param.state {
                            ParamState::Changed(value) => Some((ident, param, value)),
                            _ => None,
                        })
                {
                    let ident_cloned = ident.clone();
                    let (task, handle) = Task::future(async move {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        Message::ParamValueUploadTimeout(mav_id, ident_cloned)
                    })
                    .abortable();

                    param.state = ParamState::Uploading(handle, param.value);

                    let cap = vehicle.capabilities?;
                    let (param_value, param_type) =
                        if cap.contains(MavProtocolCapability::PARAM_ENCODE_BYTEWISE) {
                            parameters::value_into_bytewise(value)
                        } else if cap.contains(MavProtocolCapability::PARAM_ENCODE_C_CAST) {
                            parameters::value_into_c_cast(value)
                        } else {
                            log::error!("Parameter encoding type not known for vehicle");
                            return None;
                        };

                    let param_set = ParamSet {
                        target_system: mav_id.system,
                        target_component: mav_id.component,
                        param_id: *ident.as_raw(),
                        param_value,
                        param_type,
                    };

                    connection.spawn_send_message(param_set);

                    timeout_tasks.push(task);
                }

                return Some(Task::batch(timeout_tasks));
            }
            Message::ParamBufferEdit(mav_id, ident, buffer) => {
                let entry = self.vehicles.get_mut(&mav_id)?;
                let param = entry.params.map.get_mut(&ident)?;

                param.state = ParamState::Unchanged;

                if let Some(new_value) = parameters::value_parse_as(param.value, &buffer)
                    && new_value != param.value
                {
                    param.state = ParamState::Changed(new_value);
                }

                param.editing = Some(buffer);
            }
            Message::ParamValueReset(mav_id, ident) => {
                let entry = self.vehicles.get_mut(&mav_id)?;
                let param = entry.params.map.get_mut(&ident)?;

                param.editing = None;
                param.state = ParamState::Unchanged;
            }
            Message::ParamValueUpload(mav_id, ident, value) => {
                let connection = self.connection.as_ref()?.downgrade();
                let vehicle = self.vehicles.get_mut(&mav_id)?;
                let param = vehicle.params.map.get_mut(&ident)?;

                let ident_cloned = ident.clone();
                let (timeout_task, handle) = Task::future(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Message::ParamValueUploadTimeout(mav_id, ident_cloned)
                })
                .abortable();

                param.state = ParamState::Uploading(handle, param.value);

                let cap = vehicle.capabilities?;
                let (param_value, param_type) =
                    if cap.contains(MavProtocolCapability::PARAM_ENCODE_BYTEWISE) {
                        parameters::value_into_bytewise(value)
                    } else if cap.contains(MavProtocolCapability::PARAM_ENCODE_C_CAST) {
                        parameters::value_into_c_cast(value)
                    } else {
                        log::error!("Parameter encoding type not known for vehicle");
                        return None;
                    };

                let param_set = ParamSet {
                    target_system: mav_id.system,
                    target_component: mav_id.component,
                    param_id: *ident.as_raw(),
                    param_value,
                    param_type,
                };

                connection.spawn_send_message(param_set);

                return Some(timeout_task);
            }
            Message::ParamValueUploadTimeout(mav_id, ident) => {
                let entry = self.vehicles.get_mut(&mav_id)?;
                let param = entry.params.map.get_mut(&ident)?;

                if let ParamState::Uploading(handle, value) = param.state.clone() {
                    log::warn!("Parameter upload for '{}' timed out", ident.as_str());
                    param.state = ParamState::Changed(value);
                    handle.abort();
                }
            }
            Message::ParamSaveDialog(parameters) => {
                let file_dialog = self.new_file_dialog();
                return Some(Task::future(async move {
                    match file_dialog.save_file().await {
                        Some(path) => Message::ParamSaveToFile(path.path().to_owned(), parameters),
                        None => Message::Noop,
                    }
                }));
            }
            Message::ParamSaveToFile(save_path, parameters) => {
                log::info!("Saving parameters to file: {save_path:?}");

                let result = parameters::save_parameters_to_ini(save_path.clone(), parameters);
                if let Err(error) = result {
                    log::error!("Error saving to file: {}", error);
                }

                return self.set_file_picker_path_config(save_path);
            }
            Message::ParamLoadDialog(mav_id) => {
                let file_dialog = self.new_file_dialog();
                return Some(Task::future(async move {
                    match file_dialog.pick_file().await {
                        Some(path) => Message::ParamLoadFromFile(path.path().to_owned(), mav_id),
                        None => Message::Noop,
                    }
                }));
            }
            Message::ParamLoadFromFile(load_path, mav_id) => {
                log::info!("Loading parameters from file: {load_path:?}");

                let vehicle = self.vehicles.get_mut(&mav_id)?;
                let loaded_params = match parameters::load_parameters_from_ini(load_path.clone()) {
                    Ok(loaded_params) => loaded_params,
                    Err(error) => {
                        log::error!("Could not load parameter file: {}", error);
                        return None;
                    }
                };

                // Set the parameters of the vehicle as modified if that is the case
                for (ident, new_param) in loaded_params.map.iter() {
                    if let Some(old_param) = vehicle.params.map.get_mut(ident)
                        && parameters::value_type_matches(old_param.value, new_param.value)
                        && old_param.value != new_param.value
                    {
                        old_param.state = ParamState::Changed(new_param.value);
                        old_param.editing = Some(parameters::value_as_string(new_param.value));
                    }
                }

                return self.set_file_picker_path_config(load_path);
            }
            Message::SetPrimaryVehicle(mav_id) => {
                self.primary_vehicle = Some(mav_id);
            }
        }

        None
    }

    fn set_file_picker_path_config(&mut self, path: PathBuf) -> Option<Task<Message>> {
        let mut file_picker_path = None;
        if path.is_dir() {
            file_picker_path = Some(path);
        } else {
            if let Some(parent) = path.parent() {
                file_picker_path = Some(parent.to_path_buf());
            }
        }

        if let Some(path) = file_picker_path.take() {
            self.configuration.file_picker_path = Some(path);
            Some(Task::done(Message::SaveConfigurationToFile))
        } else {
            None
        }
    }

    fn new_file_dialog(&self) -> AsyncFileDialog {
        let mut file_dialog = rfd::AsyncFileDialog::new();

        if let Some(path) = self.configuration.file_picker_path.as_ref() {
            file_dialog = file_dialog.set_directory(path)
        }

        file_dialog
    }

    fn update_conn(&mut self, message: ConnMessage) -> Task<Message> {
        let time_now = Instant::now();
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

                let vehicle = match self.vehicles.entry(mav_id) {
                    btree_map::Entry::Vacant(vacant_entry) => {
                        log::info!("New vehicle detected: {mav_id:?}");
                        vacant_entry.insert(Vehicle::new())
                    }
                    btree_map::Entry::Occupied(occupied_entry) => occupied_entry.into_mut(),
                };

                vehicle.link_info_mut(link_id).last_message = Some(time_now);
                vehicle.message_history.push((time_now, message.clone()));

                match message {
                    mavio::DefaultDialect::Heartbeat(msg) => {
                        vehicle.last_heartbeat = Some((time_now, msg))
                    }
                    mavio::DefaultDialect::AutopilotVersion(msg) => {
                        if let Some(vehicle) = self.vehicles.get_mut(&mav_id) {
                            if vehicle.capabilities.is_none() {
                                log::debug!(
                                    "Received capabilities for {:?}: {:?}",
                                    mav_id,
                                    msg.capabilities
                                )
                            }
                            vehicle.capabilities = Some(msg.capabilities);
                        }
                    }
                    mavio::DefaultDialect::ScaledImu(msg) => {
                        if let Some(vehicle) = self.vehicles.get_mut(&mav_id) {
                            vehicle.gyroscope = Some([
                                msg.xgyro as f32 * 1e-3,
                                msg.ygyro as f32 * 1e-3,
                                msg.zgyro as f32 * 1e-3,
                            ]);

                            vehicle.accelerometer = Some([
                                msg.xacc as f32 * 1e-3,
                                msg.yacc as f32 * 1e-3,
                                msg.zacc as f32 * 1e-3,
                            ]);
                        }
                    }
                    mavio::DefaultDialect::ParamValue(msg) => {
                        use mavio::default_dialect::enums::MavProtocolCapability;
                        let Some(proto_capabilities) = vehicle.capabilities else {
                            log::error!(
                                "Cannot handle parameters before knowing vehicle capabilities"
                            );
                            return Task::none();
                        };

                        // Decode the parameter according to capabilities flag
                        let maybe_value = if proto_capabilities
                            .contains(MavProtocolCapability::PARAM_ENCODE_BYTEWISE)
                        {
                            parameters::value_from_bytewise(msg.param_value, msg.param_type)
                        } else if proto_capabilities
                            .contains(MavProtocolCapability::PARAM_ENCODE_C_CAST)
                        {
                            parameters::value_from_c_cast(msg.param_value, msg.param_type)
                        } else {
                            log::error!("Parameter encoding type not known for vehicle");
                            return Task::none();
                        };

                        let Some(value) = maybe_value else {
                            log::error!("Unsupported numeric type of parameter");
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
            ConnMessage::RecvError(error, link_id) => {
                log::error!("Error receiving on {link_id:?}: {error:?}");
            }
            ConnMessage::ChangeLinkVariant(variant) => {
                if self.link_builder.to_variant() == variant {
                    return Task::none();
                }

                self.link_builder = variant.to_default_builder();
                self.link_config = self.link_builder.try_build();
                if let Some(link_config) = self.link_config.clone() {
                    self.configuration.link_config = Some(link_config);
                    return Task::done(Message::SaveConfigurationToFile);
                }
            }
            ConnMessage::UpdateLinkBuilder(link_builder) => {
                self.link_builder = link_builder;
                self.link_config = self.link_builder.try_build();
                if let Some(link_config) = self.link_config.clone() {
                    self.configuration.link_config = Some(link_config);
                    return Task::done(Message::SaveConfigurationToFile);
                }
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
                self.configuration.link_config = Some(config.clone());
                log::info!("Connecting to: {config:?}");
                return config.connect();
            }
            ConnMessage::DisconnectLink => {
                if let Some(connection) = self.connection.take() {
                    connection.close();
                }
            }
            ConnMessage::ConnectFailed(error) => {
                log::error!("Connect failed: {error:?}");
            }
            ConnMessage::ConnectSuccess(connection) => {
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
        stack!(
            slippery::MapWidget::new(&self.tile_cache, Message::MapCache, self.viewpoint)
                .on_update(Message::MapProjector),
            column![
                iced::widget::container(self.view_top_panel()),
                row![
                    self.view_param_list_scrollable(),
                    self.view_vehicle_information(),
                ]
                .spacing(10.0)
            ]
            .spacing(10.0)
            .padding(10.0)
        )
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
                            addr,
                            port: port.clone(),
                        }))
                    })
                    .width(160.0)
                    .into();

                let port_input = text_input("port", port)
                    .on_input(|port| {
                        Message::Conn(ConnMessage::UpdateLinkBuilder(LinkBuilder::Tcp {
                            addr: addr.clone(),
                            port,
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
                            addr,
                            port: port.clone(),
                        }))
                    })
                    .width(160.0)
                    .into();

                let port_input = text_input("port", port)
                    .on_input(|port| {
                        Message::Conn(ConnMessage::UpdateLinkBuilder(LinkBuilder::Udp {
                            addr: addr.clone(),
                            port,
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

        if self.vehicles.len() > 1 {
            let mut vehicle_ids = self.vehicles.keys().cloned().collect::<Vec<_>>();
            vehicle_ids.sort();
            let vehicle_picker = pick_list(self.primary_vehicle, vehicle_ids, |v| {
                format!("sys: {} - com: {}", v.system, v.component)
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
        if self.vehicles.is_empty() {
            return space::Space::new().into();
        }

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
                Text::new(format!("Heartbeat: {:#?}", vehicle.last_heartbeat))
                    .color_maybe(
                        vehicle
                            .last_heartbeat
                            .as_ref()
                            .is_none_or(|(hb, _)| hb.elapsed() > Duration::from_secs(2))
                            .then(|| Color::from_rgb8(255, 100, 100)),
                    )
                    .into(),
            );

            entries.push(Text::new(format!("Gyros: {:?}", vehicle.gyroscope)).into());
            entries.push(Text::new(format!("Accel: {:?}", vehicle.accelerometer)).into());
        }

        iced::widget::container(Column::from_vec(entries))
            .style(iced::widget::container::bordered_box)
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
        .style(iced::widget::container::bordered_box)
        .into()
    }

    fn view_param_list(&self) -> Element<'_, Message> {
        let mut entries = Vec::with_capacity(128);

        for (identity, vehicle) in &self.vehicles {
            let reload_button = Button::new("Reload parameters")
                .on_press(Message::ParamListReload(*identity))
                .into();

            let upload_button = Button::new("Upload changed parameters")
                .on_press(Message::ParamUploadAll(*identity))
                .into();

            let save_button = Button::new("Save parameters to file")
                .on_press_with(|| Message::ParamSaveDialog(vehicle.params.clone()))
                .into();

            let load_button = Button::new("Load parameters from file")
                .on_press(Message::ParamLoadDialog(*identity))
                .into();

            let filter_field = TextInput::new("Filter parameters", &self.parameter_filter)
                .on_input(Message::ParamFilterBuf)
                .into();

            entries.push(reload_button);
            entries.push(upload_button);
            entries.push(save_button);
            entries.push(load_button);
            entries.push(filter_field);

            let identity = *identity;

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

            let parameters = self.parameter_filtered.as_ref().unwrap_or(&vehicle.params);

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
                        Message::ParamBufferEdit(identity, ident_owned.clone(), string)
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

                // let text_input = if !matches!(param.state, ParamState::Unchanged) {
                //     let tooltip = container(Text::new(format!("Was: {:?}", param.value))).style(shaded_bordered_box).padding(10.0);
                //     let content = iced::widget::tooltip(text_input, tooltip, iced::widget::tooltip::Position::Left);
                //     container(content)
                // } else {
                //     container(text_input)
                // };

                let ident_owned = ident.clone();
                let restore_button = Button::new("Restore")
                    .on_press_maybe(match param.state {
                        ParamState::Changed(..) => {
                            Some(Message::ParamValueReset(identity, ident_owned.clone()))
                        }
                        _ => None,
                    })
                    .width(80.0);

                let commit_button = Button::new("Upload")
                    .on_press_maybe(match param.state {
                        ParamState::Changed(value) => Some(Message::ParamValueUpload(
                            identity,
                            ident_owned.clone(),
                            value,
                        )),
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
        }

        Column::from_vec(entries).spacing(5.0).padding(10.0).into()
    }
}

pub fn shaded_bordered_box(theme: &iced::Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weaker.color.into()),
        text_color: Some(palette.background.weakest.text),
        border: iced::Border {
            width: 1.0,
            radius: 5.0.into(),
            color: palette.background.weak.color,
        },
        shadow: iced::Shadow {
            color: iced::Color::BLACK.scale_alpha(0.5),
            offset: iced::Vector::new(0.0, 1.0),
            blur_radius: 6.0,
        },
        ..container::Style::default()
    }
}
