use std::{
    collections::{HashMap, hash_map::Entry},
    time::Instant,
};

use mavio::{
    DefaultDialect,
    default_dialect::{enums::MavProtocolCapability, messages::Heartbeat},
};

use crate::{
    connection::LinkId,
    parameters::{MavlinkId, Parameters},
};

#[derive(Clone, Debug)]
pub struct Vehicle {
    pub mav_id: MavlinkId,
    pub capabilities: Option<MavProtocolCapability>,
    pub model_name: Option<Box<str>>,
    pub vendor_name: Option<Box<str>>,
    pub params: Parameters,
    pub link_info: HashMap<LinkId, VehicleLinkInfo>,
    pub message_history: Vec<(Instant, DefaultDialect)>,
    pub last_heartbeat: Option<(Instant, Heartbeat)>,
    pub gyroscope: Option<[f32; 3]>,
    pub accelerometer: Option<[f32; 3]>,
}

impl Vehicle {
    pub fn new(mav_id: MavlinkId) -> Self {
        Self {
            mav_id,
            capabilities: None,
            model_name: None,
            vendor_name: None,
            params: Parameters::new(),
            link_info: HashMap::new(),
            message_history: Vec::with_capacity(100),
            last_heartbeat: None,
            gyroscope: None,
            accelerometer: None,
        }
    }

    pub fn link_info_mut(&'_ mut self, link_id: LinkId) -> &mut VehicleLinkInfo {
        self.link_info.entry(link_id).or_default()
    }

    pub fn set_mav_capability(&mut self, cap: MavProtocolCapability) {
        if self.capabilities != Some(cap) {
            log::debug!("Received capabilities for {:?}: {:b}", self.mav_id, cap)
        }
        self.capabilities = Some(cap);
    }
}

#[derive(Clone, Debug, Default)]
pub struct VehicleLinkInfo {
    pub last_message: Option<Instant>,
    // Some stuff about reliability?
}
