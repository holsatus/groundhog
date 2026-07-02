use std::{collections::HashMap, time::Instant};

use mav_param::Ident;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavProtocolCapability, MavSeverity, MavState},
        messages::ParamValue,
    },
    protocol::MessageSpec,
};
use nalgebra::{Quaternion, UnitQuaternion, Vector3};

use crate::{
    connection::builder::LinkId,
    parameter::base::{self, MavlinkId, Parameter, Parameters},
};

#[derive(Clone, Debug)]
pub struct Vehicle {
    pub mav_id: MavlinkId,
    pub mav_state: Option<MavState>,
    pub capabilities: Option<MavProtocolCapability>,
    pub model_name: Option<Box<str>>,
    pub vendor_name: Option<Box<str>>,
    pub params: Parameters,
    pub link_info: HashMap<LinkId, VehicleLinkInfo>,
    pub message_history: Vec<(Instant, DefaultDialect)>,
    pub latest_message: HashMap<u32, (Instant, DefaultDialect)>,
    pub attitude: Option<UnitQuaternion<f32>>,
    pub angular_rates: Option<Vector3<f32>>,
    pub velocity_ned: Option<Vector3<f32>>,
    pub altitude_msl: Option<f32>,
    pub global_position: Option<(f64, f64)>,
    pub status_messages: Vec<(Instant, MavSeverity, String)>,
}

impl Vehicle {
    pub fn new(mav_id: MavlinkId) -> Self {
        Self {
            mav_id,
            mav_state: None,
            capabilities: None,
            model_name: None,
            vendor_name: None,
            params: Parameters::new(),
            link_info: HashMap::new(),
            message_history: Vec::with_capacity(100),
            status_messages: Vec::new(),
            latest_message: HashMap::new(),
            attitude: None,
            angular_rates: None,
            velocity_ned: None,
            altitude_msl: None,
            global_position: None,
        }
    }

    pub fn on_param_value(&mut self, message: &ParamValue) {
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

        log::info!("Got parameter: {}: {:?}", ident.as_str(), value);
        self.params.map.insert(ident, Parameter::new(value));

        // Keep track of how many we expect
        if message.param_count > 0 {
            self.params
                .loading_state
                .has_loaded
                .insert(message.param_index);

            self.params.loading_state.expected_count = message.param_count;

            if message.param_index + 1 == message.param_count {
                let got = self.params.loading_state.has_loaded.len();
                let exp = message.param_count;
                if got >= exp as usize {
                    log::info!("Loaded total of {got} parameters");
                } else {
                    log::warn!("Expected {exp} paramaters, got {got}");
                }
            }
        }
    }

    pub fn register_message(&mut self, time: Instant, message: DefaultDialect) {
        match &message {
            DefaultDialect::Attitude(att) => {
                self.angular_rates =
                    Some(Vector3::new(att.rollspeed, att.pitchspeed, att.yawspeed));

                // TODO: Check order of rotation angles
                self.attitude = Some(UnitQuaternion::from_euler_angles(
                    att.roll, att.pitch, att.yaw,
                ));
            }
            DefaultDialect::AttitudeQuaternion(att) => {
                self.angular_rates =
                    Some(Vector3::new(att.rollspeed, att.pitchspeed, att.yawspeed));

                // TODO: Check order of quaternion values
                self.attitude = Some(UnitQuaternion::from_quaternion(Quaternion::from_vector(
                    [att.q1, att.q2, att.q3, att.q4].into(),
                )));
            }
            DefaultDialect::GlobalPositionInt(pos) => {
                self.global_position = Some((pos.lat as f64 * 1e-7, pos.lon as f64 * 1e-7));
                self.velocity_ned = Some(Vector3::new(
                    pos.vx as f32 / 100.,
                    pos.vy as f32 / 100.,
                    pos.vz as f32 / 100.,
                )); // [cm/s]
                self.altitude_msl = Some(pos.alt as f32 / 1000.); // [mm]
            }
            DefaultDialect::GlobalPositionIntCov(pos) => {
                self.global_position = Some((pos.lat as f64 * 1e-7, pos.lon as f64 * 1e-7));
                self.velocity_ned = Some(Vector3::new(
                    pos.vx as f32 / 100.,
                    pos.vy as f32 / 100.,
                    pos.vz as f32 / 100.,
                )); // [cm/s]
                self.altitude_msl = Some(pos.alt as f32 / 1000.); // [mm]
            }
            _ => (),
        }

        self.latest_message.insert(message.id(), (time, message));
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

    pub fn set_mav_state(&mut self, state: MavState) {
        if self.mav_state != Some(state) {
            log::debug!("Received MAV state for {:?}: {:?}", self.mav_id, state);
            self.mav_state = Some(state);
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
