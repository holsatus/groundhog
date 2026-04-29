use std::{
    collections::{HashMap, hash_map::Entry},
    time::Instant,
};

use mavio::{
    DefaultDialect,
    default_dialect::{enums::MavProtocolCapability, messages::Heartbeat},
};

use crate::{connection::LinkId, parameters::Parameters};

#[derive(Clone, Debug)]
pub struct Vehicle {
    pub capabilities: Option<MavProtocolCapability>,
    pub params: Parameters,
    pub link_info: HashMap<LinkId, VehicleLinkInfo>,
    pub message_history: Vec<(Instant, DefaultDialect)>,
    pub last_heartbeat: Option<(Instant, Heartbeat)>,
    pub gyroscope: Option<[f32; 3]>,
    pub accelerometer: Option<[f32; 3]>,
}

impl Vehicle {
    pub fn new() -> Self {
        Self {
            capabilities: None,
            params: Parameters::new(),
            link_info: HashMap::new(),
            message_history: Vec::with_capacity(100),
            last_heartbeat: None,
            gyroscope: None,
            accelerometer: None,
        }
    }

    pub fn link_info_mut(&'_ mut self, link_id: LinkId) -> &mut VehicleLinkInfo {
        match self.link_info.entry(link_id) {
            Entry::Occupied(occupied_entry) => occupied_entry.into_mut(),
            Entry::Vacant(vacant_entry) => vacant_entry.insert(VehicleLinkInfo::default()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VehicleLinkInfo {
    pub last_message: Option<Instant>,
    // Some stuff about reliability?
}
