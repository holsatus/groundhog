//! Parameter value codecs: typed values <-> MAVLink wire parameters.
//!
//! A parameter travels as a `f32` plus a `MAV_PARAM_TYPE`; how the typed
//! [`Value`] maps to that pair depends on the target's advertised encoding
//! capability (C-cast or bytewise). These functions are pure: they touch no
//! application state, so both the domain and the middleware share them.
//!
//! The 64-bit types (`Uint64`, `Int64`) are deliberately unsupported:
//! `mav_param::Value` cannot hold them, and a wire `f32` cannot represent them.
//! They decode to `None`, so their values are left out rather than truncated.

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
    pub(crate) fn advertised(capabilities: MavProtocolCapability) -> Option<Self> {
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
    pub(crate) fn encode(self, value: Value) -> (f32, MavParamType) {
        match self {
            ParamEncoding::CCast => value_into_c_cast(value),
            ParamEncoding::Bytewise => value_into_bytewise(value),
        }
    }

    /// Decode a wire value, or `None` for a type `mav_param` cannot hold.
    pub(crate) fn decode(self, param_value: f32, param_type: MavParamType) -> Option<Value> {
        match self {
            ParamEncoding::CCast => value_from_c_cast(param_value, param_type),
            ParamEncoding::Bytewise => value_from_bytewise(param_value, param_type),
        }
    }
}

pub(crate) fn value_from_bytewise(param_value: f32, param_type: MavParamType) -> Option<Value> {
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

pub(crate) fn value_into_bytewise(value: Value) -> (f32, MavParamType) {
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

/// A C-cast value, or `None` for a non-finite value or one outside the type's
/// range: a sentinel the autopilot sends (`-1`, `UINT16_MAX`, NaN) must not be
/// presented as an editable value. Fractional values truncate, as a C cast
/// does.
pub(crate) fn value_from_c_cast(param_value: f32, param_type: MavParamType) -> Option<Value> {
    let cast = |min: f64, max: f64| {
        let value = f64::from(param_value);
        (value.is_finite() && (min..=max).contains(&value)).then_some(value)
    };

    Some(match param_type {
        MavParamType::Uint8 => Value::U8(cast(0.0, 255.0)? as u8),
        MavParamType::Int8 => Value::I8(cast(-128.0, 127.0)? as i8),
        MavParamType::Uint16 => Value::U16(cast(0.0, 65_535.0)? as u16),
        MavParamType::Int16 => Value::I16(cast(-32_768.0, 32_767.0)? as i16),
        MavParamType::Uint32 => Value::U32(cast(0.0, f64::from(u32::MAX))? as u32),
        MavParamType::Int32 => Value::I32(cast(f64::from(i32::MIN), f64::from(i32::MAX))? as i32),
        MavParamType::Real32 => {
            if !param_value.is_finite() {
                return None;
            }
            Value::F32(param_value)
        }
        // 64-bit integers and every other type cannot be held by `Value`.
        _ => return None,
    })
}

pub(crate) fn value_into_c_cast(value: Value) -> (f32, MavParamType) {
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

    /// A sentinel or an out-of-range value must not become an editable
    /// number: the C cast would truncate it silently.
    #[test]
    fn c_cast_refuses_values_the_type_cannot_hold() {
        assert_eq!(value_from_c_cast(f32::NAN, MavParamType::Uint8), None);
        assert_eq!(value_from_c_cast(f32::INFINITY, MavParamType::Int32), None);
        assert_eq!(value_from_c_cast(-1.0, MavParamType::Uint8), None);
        assert_eq!(value_from_c_cast(300.0, MavParamType::Uint8), None);
        assert_eq!(value_from_c_cast(65_536.0, MavParamType::Uint16), None);
        assert_eq!(
            value_from_c_cast(1.9, MavParamType::Uint8),
            Some(Value::U8(1))
        );
        assert_eq!(
            value_from_c_cast(-1.0, MavParamType::Int8),
            Some(Value::I8(-1))
        );
    }

    #[test]
    fn unsupported_types_decode_to_none() {
        for ty in [MavParamType::Uint64, MavParamType::Int64] {
            assert_eq!(value_from_c_cast(1.0, ty), None);
            assert_eq!(value_from_bytewise(1.0, ty), None);
        }
    }
}
