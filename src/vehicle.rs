use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
};

use mav_param::Ident;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavProtocolCapability, MavSeverity, MavState, MavType},
        messages,
    },
    protocol::MessageSpec,
};
use nalgebra::{Quaternion, UnitQuaternion, Vector3};

use crate::{
    connection::builder::LinkId,
    parameter::base::{self, MavlinkId, Parameter, Parameters},
};

#[derive(Debug, Clone, Default)]
pub struct Vehicle {
    pub mav_id: MavlinkId,
    pub mav_state: Option<MavState>,
    pub mav_type: Option<MavType>,
    pub capabilities: Option<MavProtocolCapability>,
    pub model_name: Option<String>,
    pub vendor_name: Option<String>,
    pub parameters: Parameters,
    pub link_info: HashMap<LinkId, VehicleLinkInfo>,
    pub message_history: VecDeque<MavMessage>,
    pub latest_message: HashMap<u32, MavMessage>,
    pub attitude: Option<Attitude>,
    pub angular_rates: Option<AngularRate>,
    pub velocity_ned: Option<Vector3<f32>>,
    pub altitude_msl: Option<f32>,
    pub global_position: Option<GlobalPosition>,
    pub global_positions: VecDeque<GlobalPosition>,
    pub battery_state: Option<BatteryState>,
    pub status_texts: Vec<StatusText>,
}

fn string_from_cstring(cstring: &[u8]) -> Option<String> {
    let null = cstring
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(cstring.len());

    str::from_utf8(&cstring[..null])
        .map(|str| str.to_string())
        .ok()
}

#[derive(Clone, Debug)]
pub struct Attitude {
    pub at: Instant,
    pub attitude: UnitQuaternion<f32>,
}

#[derive(Clone, Debug)]
pub struct AngularRate {
    pub at: Instant,
    pub rates: Vector3<f32>,
}

#[derive(Clone, Debug)]
pub struct MavMessage {
    pub at: Instant,
    pub message: DefaultDialect,
}

#[derive(Clone, Debug)]
pub struct BatteryState {
    pub at: Instant,
    pub voltage: f32,
    pub current: f32,
    pub charge: f32,
}

#[derive(Clone, Debug)]
pub struct GlobalPosition {
    pub at: Instant,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug)]
pub struct StatusText {
    pub at: Instant,
    pub severity: MavSeverity,
    pub text: String,
}

impl Vehicle {
    pub fn new(mav_id: MavlinkId) -> Self {
        Self {
            mav_id,
            ..Vehicle::default()
        }
    }

    pub fn on_component_information_basic(
        &mut self,
        message: &messages::ComponentInformationBasic,
    ) {
        if let Some(model_name) = string_from_cstring(&message.model_name)
            && !model_name.is_empty()
        {
            self.model_name = Some(String::from(model_name))
        }
        if let Some(vendor_name) = string_from_cstring(&message.vendor_name)
            && !vendor_name.is_empty()
        {
            self.vendor_name = Some(String::from(vendor_name))
        }
        self.set_mav_capability(message.capabilities);
    }

    pub fn on_param_value(&mut self, message: &messages::ParamValue) {
        let cap = self.capabilities.unwrap_or_default();

        let maybe_value = base::mavio_into_value(cap, message.param_value, message.param_type);

        let Some(value) = maybe_value else {
            log::error!("Unsupported type of parameter: {:?}", message.param_type);
            return;
        };

        let Ok(ident) = Ident::try_from(&message.param_id) else {
            log::error!("Invalid parameter identifier: {:?}", message.param_id);
            return;
        };

        self.parameters.map.insert(ident, Parameter::new(value));

        // Keep track of how many we expect
        if message.param_count > 0 {
            self.parameters
                .loading_state
                .has_loaded
                .insert(message.param_index);

            self.parameters.loading_state.expected_count = message.param_count;

            if message.param_index + 1 == message.param_count {
                let got = self.parameters.loading_state.has_loaded.len();
                let exp = message.param_count;
                if got >= exp as usize {
                    log::info!("Loaded total of {got} parameters");
                } else {
                    log::warn!("Expected {exp} paramaters, got {got}");
                }
            }
        }
    }

    fn on_heartbeat(&mut self, message: &messages::Heartbeat) {
        if self.mav_type != Some(message.type_) {
            log::debug!(
                "Received new MAV type for {:?}: {:?}",
                self.mav_id,
                message.type_
            );
            self.mav_type = Some(message.type_);
        }

        if self.mav_state != Some(message.system_status) {
            log::debug!(
                "Received new MAV state for {:?}: {:?}",
                self.mav_id,
                message.system_status
            );
            self.mav_state = Some(message.system_status);
        }
    }

    fn on_autopilot_version(&mut self, message: &messages::AutopilotVersion) {
        self.set_mav_capability(message.capabilities);
    }

    pub fn register_global_position(&mut self, position: GlobalPosition) {
        self.global_position = Some(position.clone());
        if self.global_positions.len() >= 500 {
            self.global_positions.pop_front();
        }
        self.global_positions.push_back(position);
    }

    fn on_sys_status(&mut self, message: &messages::SysStatus, time: Instant) {
        self.battery_state = Some(BatteryState {
            at: time,
            voltage: message.voltage_battery as f32 * 1e-3,
            current: message.current_battery as f32 * 1e-2,
            charge: message.battery_remaining as f32,
        });
    }

    fn on_battery_status(&mut self, message: &messages::BatteryStatus, time: Instant) {
        self.battery_state = Some(BatteryState {
            at: time,
            voltage: message
                .voltages
                .iter()
                .cloned()
                .filter_map(|v| (v != u16::MAX).then_some(v as f32))
                .sum::<f32>() as f32
                * 1e-3,
            current: message.current_battery as f32 * 1e-2,
            charge: message.battery_remaining as f32,
        });
    }

    fn on_attitude(&mut self, message: &messages::Attitude, time: Instant) {
        self.angular_rates = Some(AngularRate {
            at: time,
            rates: Vector3::new(message.rollspeed, message.pitchspeed, message.yawspeed),
        });

        // TODO: Check order of rotation angles
        self.attitude = Some(Attitude {
            at: time,
            attitude: UnitQuaternion::from_euler_angles(message.roll, message.pitch, message.yaw),
        });
    }

    fn on_attitude_quaternion(&mut self, message: &messages::AttitudeQuaternion, time: Instant) {
        self.angular_rates = Some(AngularRate {
            at: time,
            rates: Vector3::new(message.rollspeed, message.pitchspeed, message.yawspeed),
        });

        // TODO: Check order quaternion scalars
        self.attitude = Some(Attitude {
            at: time,
            attitude: UnitQuaternion::from_quaternion(Quaternion::from_vector(
                [message.q1, message.q2, message.q3, message.q4].into(),
            )),
        });
    }

    fn on_global_position_int(&mut self, message: &messages::GlobalPositionInt, time: Instant) {
        self.register_global_position(GlobalPosition {
            at: time,
            lat: message.lat as f64 * 1e-7,
            lon: message.lon as f64 * 1e-7,
        });
        self.velocity_ned = Some(Vector3::new(
            message.vx as f32 / 100.,
            message.vy as f32 / 100.,
            message.vz as f32 / 100.,
        )); // [cm/s]
        self.altitude_msl = Some(message.alt as f32 / 1000.); // [mm]
    }

    fn on_global_position_int_cov(
        &mut self,
        message: &messages::GlobalPositionIntCov,
        time: Instant,
    ) {
        self.register_global_position(GlobalPosition {
            at: time,
            lat: message.lat as f64 * 1e-7,
            lon: message.lon as f64 * 1e-7,
        });
        self.velocity_ned = Some(Vector3::new(
            message.vx as f32 / 100.,
            message.vy as f32 / 100.,
            message.vz as f32 / 100.,
        )); // [cm/s]
        self.altitude_msl = Some(message.alt as f32 / 1000.); // [mm]
    }

    pub fn on_status_text(&mut self, message: &messages::Statustext, time: Instant) {
        let null = message
            .text
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(message.text.len());

        let text_string = match str::from_utf8(&message.text[..null]) {
            Ok(text) => text.to_string(),
            Err(error) => {
                log::warn!(
                    "Statustext from '{}' not valid utf8: {}",
                    self.pretty_name(),
                    error
                );
                return;
            }
        };

        self.status_texts.push(StatusText {
            at: time,
            severity: message.severity,
            text: text_string,
        });
    }

    pub fn handle_message(&mut self, time: Instant, message: DefaultDialect) {
        match &message {
            DefaultDialect::Heartbeat(message) => {
                self.on_heartbeat(message);
            }
            DefaultDialect::AutopilotVersion(message) => {
                self.on_autopilot_version(message);
            }
            DefaultDialect::ComponentInformationBasic(message) => {
                self.on_component_information_basic(message);
            }
            DefaultDialect::ParamValue(message) => {
                self.on_param_value(message);
            }
            DefaultDialect::SysStatus(message) => {
                self.on_sys_status(message, time);
            }
            DefaultDialect::BatteryStatus(message) => {
                self.on_battery_status(message, time);
            }
            DefaultDialect::Attitude(message) => {
                self.on_attitude(message, time);
            }
            DefaultDialect::AttitudeQuaternion(message) => {
                self.on_attitude_quaternion(message, time);
            }
            DefaultDialect::GlobalPositionInt(message) => {
                self.on_global_position_int(message, time)
            }
            DefaultDialect::GlobalPositionIntCov(message) => {
                self.on_global_position_int_cov(message, time)
            }
            DefaultDialect::Statustext(message) => {
                self.on_status_text(message, time);
            }
            _ => (),
        }

        while self.message_history.len() >= 1000 {
            self.message_history.pop_back();
        }

        self.message_history.push_front(MavMessage {
            at: time,
            message: message.clone(),
        });

        self.latest_message
            .insert(message.id(), MavMessage { at: time, message });
    }

    pub fn link_info_mut(&mut self, link_id: LinkId) -> &mut VehicleLinkInfo {
        self.link_info.entry(link_id).or_default()
    }

    pub fn set_mav_capability(&mut self, capab: MavProtocolCapability) {
        if self.capabilities != Some(capab) {
            log::debug!("Received capabilities for {:?}: {:b}", self.mav_id, capab);
            self.capabilities = Some(capab);
        }
    }

    pub fn pretty_name(&self) -> String {
        format!(
            "{} {} (mav {}:{})",
            self.vendor_name.as_deref().unwrap_or("Unknown"),
            self.model_name.as_deref().unwrap_or("Unknown"),
            self.mav_id.system,
            self.mav_id.component
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct VehicleLinkInfo {
    pub last_message: Option<Instant>,
    // Some stuff about reliability?
}
