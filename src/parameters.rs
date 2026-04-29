use std::collections::{BTreeMap, HashSet};

use iced::task::Handle;
use mav_param::{Ident, Value};
use mavio::default_dialect::enums::MavParamType;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MavlinkId {
    pub(crate) system: u8,
    pub(crate) component: u8,
}

#[derive(Clone, Debug)]
pub struct Parameter {
    pub(crate) value: Value,
    pub(crate) state: ParamState,
    pub(crate) editing: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ParamState {
    Unchanged,
    Changed(Value),
    Uploading(Handle, Value),
}

impl Parameter {
    pub fn new(value: Value) -> Self {
        Parameter {
            value,
            state: ParamState::Unchanged,
            editing: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Parameters {
    pub(crate) map: BTreeMap<Ident, Parameter>,
    pub(crate) loading_state: LoadingState,
}

#[derive(Clone, Debug)]
pub struct LoadingState {
    pub(crate) has_loaded: HashSet<u16>,
    pub(crate) expected_count: u16,
}

impl LoadingState {
    pub fn new() -> LoadingState {
        LoadingState {
            has_loaded: HashSet::new(),
            expected_count: 0,
        }
    }
}

impl Parameters {
    pub fn new() -> Self {
        Parameters {
            map: BTreeMap::new(),
            loading_state: LoadingState::new(),
        }
    }
}

pub(crate) fn save_parameters_to_ini(
    path: impl AsRef<std::path::Path>,
    parameters: Parameters,
) -> std::io::Result<()> {
    let mut config = ini::Ini::new();

    // Group parameters by their first segment
    for (key, param) in parameters.map.iter() {
        let parts: Vec<&str> = key.as_str().splitn(2, mav_param::SEP).collect();

        let (section, property) = if parts.len() > 1 {
            (Some(parts[0]), parts[1])
        } else {
            (None, parts[0])
        };

        // Convert value to string
        let type_value_str = match param.value {
            Value::U8(v) => format!("u8 = {v}"),
            Value::I8(v) => format!("i8 = {v}"),
            Value::U16(v) => format!("u16 = {v}"),
            Value::I16(v) => format!("i16 = {v}"),
            Value::U32(v) => format!("u32 = {v}"),
            Value::I32(v) => format!("i32 = {v}"),
            Value::F32(v) => format!("f32 = {v}"),
        };

        // Set the property in the appropriate section
        config.with_section(section).set(property, type_value_str);
    }

    // Write to file
    config.write_to_file_opt(
        path,
        ini::WriteOption {
            kv_separator: ": ",
            ..Default::default()
        },
    )
}

pub(crate) fn load_parameters_from_ini(
    path: impl AsRef<std::path::Path>,
) -> Result<Parameters, ini::Error> {
    let conf = ini::Ini::load_from_file(path)?;
    let mut parameter_map = BTreeMap::new();

    for (section, properties) in conf.iter() {
        for (key, ty_val) in properties.iter() {
            // Concatenate section and key if required
            let ident = if let Some(section_name) = section {
                format!("{}.{}", section_name, key)
            } else {
                key.to_string()
            };

            let ident = Ident::from_str_truncated(&ident);

            // Split type and value and trim white spaces
            let Some((ty, val)) = ty_val.split_once('=') else {
                log::error!("Missing '=' delimiter: {key}: {ty_val}");
                continue;
            };
            let (ty, val) = (ty.trim(), val.trim());

            // Try to parse into the expected type
            let parse = || {
                Some(match ty {
                    "u8" => mav_param::Value::U8(val.parse::<u8>().ok()?),
                    "i8" => mav_param::Value::I8(val.parse::<i8>().ok()?),
                    "u16" => mav_param::Value::U16(val.parse::<u16>().ok()?),
                    "i16" => mav_param::Value::I16(val.parse::<i16>().ok()?),
                    "u32" => mav_param::Value::U32(val.parse::<u32>().ok()?),
                    "i32" => mav_param::Value::I32(val.parse::<i32>().ok()?),
                    "f32" => mav_param::Value::F32(val.parse::<f32>().ok()?),
                    _ => return None,
                })
            };

            match parse() {
                Some(value) => _ = parameter_map.insert(ident, Parameter::new(value)),
                None => println!("client: Could not parse value ({val}) as type ({ty})"),
            }
        }
    }

    Ok(Parameters {
        map: parameter_map,
        loading_state: LoadingState::new(),
    })
}

pub fn value_from_bytewise(param_value: f32, param_type: MavParamType) -> Option<Value> {
    use mav_param::{Value, value::from_bytewise};
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
    use mav_param::{Value, value::into_bytewise};
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
    use mav_param::Value;
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

#[allow(unused)]
pub fn value_into_c_cast(value: Value) -> (f32, MavParamType) {
    use mav_param::Value;
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

pub fn value_type_matches(lhs: Value, rhs: Value) -> bool {
    use mav_param::Value;

    matches!(
        (lhs, rhs),
        (Value::U8(_), Value::U8(_))
            | (Value::I8(_), Value::I8(_))
            | (Value::U16(_), Value::U16(_))
            | (Value::I16(_), Value::I16(_))
            | (Value::U32(_), Value::U32(_))
            | (Value::I32(_), Value::I32(_))
            | (Value::F32(_), Value::F32(_))
    )
}

pub fn value_parse_as(kind: Value, string: &str) -> Option<Value> {
    use mav_param::Value;
    let value = match kind {
        Value::U8(_) => Value::U8(string.parse::<u8>().ok()?),
        Value::I8(_) => Value::I8(string.parse::<i8>().ok()?),
        Value::U16(_) => Value::U16(string.parse::<u16>().ok()?),
        Value::I16(_) => Value::I16(string.parse::<i16>().ok()?),
        Value::U32(_) => Value::U32(string.parse::<u32>().ok()?),
        Value::I32(_) => Value::I32(string.parse::<i32>().ok()?),
        Value::F32(_) => Value::F32(string.parse::<f32>().ok()?),
    };

    Some(value)
}

pub fn value_type_name(value: Value) -> &'static str {
    match value {
        Value::U8(_) => "u8",
        Value::I8(_) => "i8",
        Value::U16(_) => "u16",
        Value::I16(_) => "i16",
        Value::U32(_) => "u32",
        Value::I32(_) => "i32",
        Value::F32(_) => "f32",
    }
}

pub fn value_as_string(value: Value) -> String {
    match value {
        Value::U8(v) => v.to_string(),
        Value::I8(v) => v.to_string(),
        Value::U16(v) => v.to_string(),
        Value::I16(v) => v.to_string(),
        Value::U32(v) => v.to_string(),
        Value::I32(v) => v.to_string(),
        Value::F32(v) => v.to_string(),
    }
}
