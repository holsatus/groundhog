use std::collections::{BTreeMap, HashSet};

use mav_param::{Ident, Value};
use mavio::default_dialect::enums::{MavParamType, MavProtocolCapability};

#[derive(Clone, Debug)]
pub struct Parameter {
    value: Value,
    state: ParamState,
}

#[derive(Clone, Debug)]
pub enum ParamState {
    Unchanged,
    Changed { value: Value, status: ChangedStatus },
    Uploading { value: Value },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangedStatus {
    None,
    Failed,
    Rejected,
}

impl Parameter {
    pub fn new(value: Value) -> Self {
        Parameter {
            value,
            state: ParamState::Unchanged,
        }
    }

    /// The last value reported by (or accepted for) the target.
    pub fn value(&self) -> Value {
        self.value
    }

    /// The local edit state relative to [`Parameter::value`].
    pub fn state(&self) -> &ParamState {
        &self.state
    }

    /// Fold a freshly parsed edit buffer into the state. An unparseable buffer
    /// or a value equal to the current one clears any pending change.
    pub fn set_edited(&mut self, parsed: Option<Value>) {
        match parsed {
            Some(value) if value != self.value => {
                self.state = ParamState::Changed {
                    value,
                    status: ChangedStatus::None,
                };
            }
            _ => self.state = ParamState::Unchanged,
        }
    }

    /// Discard a pending change. The reported value is left untouched.
    pub fn reset(&mut self) {
        self.state = ParamState::Unchanged;
    }

    /// Mark this parameter as being uploaded with `value`.
    pub fn begin_upload(&mut self, value: Value) {
        self.state = ParamState::Uploading { value };
    }

    /// The target accepted the upload and now reports `value`.
    pub fn upload_succeeded(&mut self, value: Value) {
        self.value = value;
        self.state = ParamState::Unchanged;
    }

    /// The target rejected the upload, reporting `reported`; keep `requested`.
    pub fn upload_rejected(&mut self, reported: Value, requested: Value) {
        self.value = reported;
        self.state = ParamState::Changed {
            value: requested,
            status: ChangedStatus::Rejected,
        };
    }

    /// The upload transport failed; keep `requested`.
    pub fn upload_failed(&mut self, requested: Value) {
        self.state = ParamState::Changed {
            value: requested,
            status: ChangedStatus::Failed,
        };
    }

    /// Record the value the target currently reports, without touching the
    /// local edit state.
    fn observe_value(&mut self, value: Value) {
        self.value = value;
    }

    /// Mark a different `value` as pending, leaving the reported value in place.
    pub fn mark_changed(&mut self, value: Value) {
        self.state = ParamState::Changed {
            value,
            status: ChangedStatus::None,
        };
    }
}

#[derive(Debug, Clone, Default)]
pub struct Parameters {
    map: BTreeMap<Ident, Parameter>,
    loading_state: LoadingState,
}

impl Parameters {
    pub fn iter(&self) -> impl Iterator<Item = (&Ident, &Parameter)> {
        self.map.iter()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn get(&self, id: &Ident) -> Option<&Parameter> {
        self.map.get(id)
    }

    pub fn get_mut(&mut self, id: &Ident) -> Option<&mut Parameter> {
        self.map.get_mut(id)
    }

    /// Insert a parameter with an initial value, replacing any existing one.
    pub fn insert(&mut self, id: Ident, value: Value) -> Option<Parameter> {
        self.map.insert(id, Parameter::new(value))
    }

    pub fn loading(&self) -> &LoadingState {
        &self.loading_state
    }

    /// Clear the set of received parameter indices, matching the state before a
    /// fresh list download. The expected count is left in place.
    pub fn reset_loading(&mut self) {
        self.loading_state.has_loaded.clear();
    }

    /// Insert a parameter if it is unseen, otherwise just refresh its reported
    /// value.
    pub fn observe_value(&mut self, id: Ident, value: Value) {
        match self.map.entry(id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                _ = entry.insert(Parameter::new(value));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().observe_value(value);
            }
        }
    }

    /// Record that parameter `index` of an expected `count` has arrived, and log
    /// completion once the final index is seen.
    pub fn note_loaded_index(&mut self, index: u16, count: u16) {
        if count == 0 {
            return;
        }

        self.loading_state.has_loaded.insert(index);
        self.loading_state.expected_count = count;

        if index + 1 == count {
            let got = self.loading_state.has_loaded.len();
            let exp = count;
            if got >= exp as usize {
                log::info!("Loaded total of {got} parameters");
            } else {
                log::warn!("Expected {exp} paramaters, got {got}");
            }
        }
    }
}

/// Used to track which parameters in a continuous sequence have been received.
/// Any that are missing can be re-requested afterwards
#[derive(Debug, Clone, Default)]
pub struct LoadingState {
    has_loaded: HashSet<u16>,
    expected_count: u16,
}

impl LoadingState {
    /// How many parameters have been received so far.
    pub fn loaded(&self) -> usize {
        self.has_loaded.len()
    }

    /// How many parameters the target advertised.
    pub fn expected(&self) -> u16 {
        self.expected_count
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ParamEncoding {
    CCast,
    Bytewise,
    #[allow(unused)]
    Unspecified,
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
            // ParamEncoding::Unspecified
            ParamEncoding::Bytewise
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
    let mut parameters = Parameters::default();

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
                Some(value) => _ = parameters.insert(ident, value),
                None => log::warn!("client: Could not parse value ({val}) as type ({ty})"),
            }
        }
    }

    Ok(parameters)
}

pub fn value_from_bytewise(param_value: f32, param_type: MavParamType) -> Option<Value> {
    use mav_param::{value::from_bytewise, Value};
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
    use mav_param::{value::into_bytewise, Value};
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

pub fn mavio_from_value(cap: MavProtocolCapability, value: Value) -> Option<(f32, MavParamType)> {
    match ParamEncoding::from(cap) {
        ParamEncoding::CCast => Some(value_into_c_cast(value)),
        ParamEncoding::Bytewise => Some(value_into_bytewise(value)),
        ParamEncoding::Unspecified => None,
    }
}

pub fn mavio_into_value(
    cap: MavProtocolCapability,
    param_value: f32,
    param_type: MavParamType,
) -> Option<Value> {
    match ParamEncoding::from(cap) {
        ParamEncoding::CCast => value_from_c_cast(param_value, param_type),
        ParamEncoding::Bytewise => value_from_bytewise(param_value, param_type),
        ParamEncoding::Unspecified => None,
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
