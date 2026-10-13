//! Parameter value codecs: typed values <-> MAVLink wire parameters.
//!
//! A parameter travels as a `f32` plus a `MAV_PARAM_TYPE`; how the typed
//! [`Value`] maps to that pair depends on the target's advertised encoding
//! capability (C-cast or bytewise). These functions are pure: they touch no
//! application state, so both the domain and the middleware share them.

use mav_param::Value;
use mavio::default_dialect::enums::{MavParamType, MavProtocolCapability};

/// How a parameter value is encoded on the wire, per the target's capabilities.
#[derive(Debug, Clone, Copy)]
pub enum ParamEncoding {
    CCast,
    Bytewise,
}

impl From<MavProtocolCapability> for ParamEncoding {
    fn from(value: MavProtocolCapability) -> Self {
        if value.intersects(
            MavProtocolCapability::PARAM_ENCODE_C_CAST | MavProtocolCapability::PARAM_FLOAT,
        ) {
            ParamEncoding::CCast
        } else if value.intersects(MavProtocolCapability::PARAM_ENCODE_BYTEWISE) {
            ParamEncoding::Bytewise
        } else {
            // MAVLink's "unspecified" encoding is treated as bytewise.
            ParamEncoding::Bytewise
        }
    }
}

pub fn value_from_bytewise(param_value: f32, param_type: MavParamType) -> Option<Value> {
    use mav_param::value::from_bytewise;

    let value = match param_type {
        MavParamType::Uint8 => Value::U8(from_bytewise(param_value)),
        MavParamType::Int8 => Value::I8(from_bytewise(param_value)),
        MavParamType::Uint16 => Value::U16(from_bytewise(param_value)),
        MavParamType::Int16 => Value::I16(from_bytewise(param_value)),
        MavParamType::Uint32 => Value::U32(from_bytewise(param_value)),
        MavParamType::Int32 => Value::I32(from_bytewise(param_value)),
        MavParamType::Real32 => Value::F32(from_bytewise(param_value)),
        _ => return None,
    };

    Some(value)
}

pub fn value_into_bytewise(value: Value) -> (f32, MavParamType) {
    use mav_param::value::into_bytewise;

    match value {
        Value::U8(v) => (into_bytewise(v), MavParamType::Uint8),
        Value::I8(v) => (into_bytewise(v), MavParamType::Int8),
        Value::U16(v) => (into_bytewise(v), MavParamType::Uint16),
        Value::I16(v) => (into_bytewise(v), MavParamType::Int16),
        Value::U32(v) => (into_bytewise(v), MavParamType::Uint32),
        Value::I32(v) => (into_bytewise(v), MavParamType::Int32),
        Value::F32(v) => (into_bytewise(v), MavParamType::Real32),
    }
}

pub fn value_from_c_cast(param_value: f32, param_type: MavParamType) -> Option<Value> {
    let value = match param_type {
        MavParamType::Uint8 => Value::U8(param_value as u8),
        MavParamType::Int8 => Value::I8(param_value as i8),
        MavParamType::Uint16 => Value::U16(param_value as u16),
        MavParamType::Int16 => Value::I16(param_value as i16),
        MavParamType::Uint32 => Value::U32(param_value as u32),
        MavParamType::Int32 => Value::I32(param_value as i32),
        MavParamType::Real32 => Value::F32(param_value),
        _ => return None,
    };

    Some(value)
}

pub fn value_into_c_cast(value: Value) -> (f32, MavParamType) {
    match value {
        Value::U8(v) => (v as f32, MavParamType::Uint8),
        Value::I8(v) => (v as f32, MavParamType::Int8),
        Value::U16(v) => (v as f32, MavParamType::Uint16),
        Value::I16(v) => (v as f32, MavParamType::Int16),
        Value::U32(v) => (v as f32, MavParamType::Uint32),
        Value::I32(v) => (v as f32, MavParamType::Int32),
        Value::F32(v) => (v, MavParamType::Real32),
    }
}

pub fn to_wire(cap: MavProtocolCapability, value: Value) -> Option<(f32, MavParamType)> {
    match ParamEncoding::from(cap) {
        ParamEncoding::CCast => Some(value_into_c_cast(value)),
        ParamEncoding::Bytewise => Some(value_into_bytewise(value)),
    }
}

pub fn from_wire(
    cap: MavProtocolCapability,
    param_value: f32,
    param_type: MavParamType,
) -> Option<Value> {
    match ParamEncoding::from(cap) {
        ParamEncoding::CCast => value_from_c_cast(param_value, param_type),
        ParamEncoding::Bytewise => value_from_bytewise(param_value, param_type),
    }
}
