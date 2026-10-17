//! Parameter value codecs: typed values <-> MAVLink wire parameters.
//!
//! A parameter travels as a `f32` plus a `MAV_PARAM_TYPE`; how the typed
//! [`Value`] maps to that pair depends on the target's advertised encoding
//! capability (C-cast or bytewise). These functions are pure: they touch no
//! application state, so both the domain and the middleware share them.

use mav_param::Value;
use mavio::default_dialect::enums::{MavParamType, MavProtocolCapability};

/// How a parameter value is encoded on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamEncoding {
    /// The value is cast to `f32` (ArduPilot).
    CCast,
    /// The value's bytes are reinterpreted as an `f32` (PX4).
    Bytewise,
}

impl ParamEncoding {
    /// The encoding a target advertises in its capabilities, if it advertises
    /// one. `PARAM_FLOAT` predates the explicit flags and means a C cast.
    pub fn advertised(capabilities: MavProtocolCapability) -> Option<Self> {
        if capabilities.intersects(
            MavProtocolCapability::PARAM_ENCODE_C_CAST | MavProtocolCapability::PARAM_FLOAT,
        ) {
            Some(ParamEncoding::CCast)
        } else if capabilities.intersects(MavProtocolCapability::PARAM_ENCODE_BYTEWISE) {
            Some(ParamEncoding::Bytewise)
        } else {
            None
        }
    }

    /// Encode `value` for the wire.
    pub fn encode(self, value: Value) -> (f32, MavParamType) {
        match self {
            ParamEncoding::CCast => value_into_c_cast(value),
            ParamEncoding::Bytewise => value_into_bytewise(value),
        }
    }

    /// Decode a wire value, or `None` for a type `mav_param` cannot hold.
    pub fn decode(self, param_value: f32, param_type: MavParamType) -> Option<Value> {
        match self {
            ParamEncoding::CCast => value_from_c_cast(param_value, param_type),
            ParamEncoding::Bytewise => value_from_bytewise(param_value, param_type),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_encodings() {
        assert_eq!(
            ParamEncoding::advertised(MavProtocolCapability::PARAM_ENCODE_C_CAST),
            Some(ParamEncoding::CCast)
        );
        assert_eq!(
            ParamEncoding::advertised(MavProtocolCapability::PARAM_FLOAT),
            Some(ParamEncoding::CCast)
        );
        assert_eq!(
            ParamEncoding::advertised(MavProtocolCapability::PARAM_ENCODE_BYTEWISE),
            Some(ParamEncoding::Bytewise)
        );
        assert_eq!(
            ParamEncoding::advertised(MavProtocolCapability::empty()),
            None
        );
    }

    /// The failure the encoding guards against: an integer written bytewise to
    /// a C-cast autopilot arrives as a denormal float, i.e. zero.
    #[test]
    fn the_encodings_disagree_on_integers() {
        let (bytewise, _) = ParamEncoding::Bytewise.encode(Value::I32(1));
        assert_eq!(bytewise as i32, 0);
        let (c_cast, _) = ParamEncoding::CCast.encode(Value::I32(1));
        assert_eq!(c_cast, 1.0);
    }

    #[test]
    fn both_encodings_round_trip() {
        for encoding in [ParamEncoding::CCast, ParamEncoding::Bytewise] {
            for value in [
                Value::U8(7),
                Value::I16(-3),
                Value::I32(-100_000),
                Value::F32(1.5),
            ] {
                let (raw, ty) = encoding.encode(value);
                assert_eq!(encoding.decode(raw, ty), Some(value), "{encoding:?}");
            }
        }
    }
}
