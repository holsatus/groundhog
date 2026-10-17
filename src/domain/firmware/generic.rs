//! The fallback for autopilots without a dedicated adapter: no named modes,
//! and nothing assumed about the parameter encoding.

use super::{Firmware, FlightMode};
use crate::{domain::vehicle::class::VehicleClass, wire::param::ParamEncoding};

pub struct Generic;

impl Firmware for Generic {
    fn name(&self) -> &'static str {
        "Generic"
    }

    fn modes(&self, _class: VehicleClass) -> &'static [FlightMode] {
        &[]
    }

    fn param_encoding(&self) -> Option<ParamEncoding> {
        None
    }
}
