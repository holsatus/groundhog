//! Parameter files.
//!
//! Three formats are read and written, chosen by file extension
//! ([`ParamFileFormat::of`]):
//! - **INI** (`.ini`, the default): parameters grouped by the first segment
//!   of their identifier, each value tagged with its type (`f32 = 1.5`);
//! - **Mission Planner** (`.param`, `.parm`): `NAME,VALUE` per line, without
//!   types — values are read as the type the vehicle reports;
//! - **QGroundControl** (`.params`): `system component NAME VALUE TYPE`,
//!   tab-separated, with `MAV_PARAM_TYPE` numbers.
//!
//! Lines starting with `#` are comments in the latter two.

use std::{fmt::Write as _, path::Path};

use mav_param::{Ident, Value};

use crate::{MavlinkId, domain::parameter::ParamValues};

/// A parameter file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamFileFormat {
    GroundhogIni,
    MissionPlanner,
    QGroundControl,
}

impl ParamFileFormat {
    /// The format of a file, by its extension.
    pub fn of_file_extension(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("param" | "parm") => ParamFileFormat::MissionPlanner,
            Some("params") => ParamFileFormat::QGroundControl,
            _ => ParamFileFormat::GroundhogIni,
        }
    }
}

/// Serialize `parameters` of the component at `address` in `format`.
pub fn write_parameters(
    format: ParamFileFormat,
    parameters: &ParamValues,
    address: MavlinkId,
) -> String {
    match format {
        ParamFileFormat::GroundhogIni => parameters_to_ini_string(parameters),
        ParamFileFormat::MissionPlanner => {
            let mut out = String::new();
            for (ident, value) in parameters {
                let _ = writeln!(out, "{},{}", ident.as_str(), value_text(*value));
            }
            out
        }
        ParamFileFormat::QGroundControl => {
            let mut out = String::from("# Vehicle-Id Component-Id Name Value Type\n");
            for (ident, value) in parameters {
                let _ = writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{}",
                    address.system,
                    address.component,
                    ident.as_str(),
                    value_text(*value),
                    type_number(*value)
                );
            }
            out
        }
    }
}

/// Parse a parameter file in `format`. `reported` gives the type of a
/// parameter as the vehicle reports it, for formats that carry no types;
/// parameters it does not know are skipped there.
///
/// # Errors
///
/// A message naming the line that could not be read: a row with too few
/// fields, or a value that does not parse as its type. For an INI file it is
/// the INI parser's own description.
pub fn read_parameters(
    format: ParamFileFormat,
    text: &str,
    reported: impl Fn(&Ident) -> Option<Value>,
) -> Result<ParamValues, String> {
    match format {
        ParamFileFormat::GroundhogIni => {
            parameters_from_ini_str(text).map_err(|error| error.to_string())
        }
        ParamFileFormat::MissionPlanner => {
            let mut values = ParamValues::new();
            for (number, line) in data_lines(text) {
                let mut fields = line
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|field| !field.is_empty());
                let (Some(name), Some(value)) = (fields.next(), fields.next()) else {
                    return Err(format!("line {number}: expected NAME,VALUE"));
                };
                let ident = Ident::from_str_truncated(name);
                let Some(kind) = reported(&ident) else {
                    continue;
                };
                let value = parse_as(kind, value)
                    .ok_or_else(|| format!("line {number}: '{value}' does not fit {name}"))?;
                values.insert(ident, value);
            }
            Ok(values)
        }
        ParamFileFormat::QGroundControl => {
            let mut values = ParamValues::new();
            for (number, line) in data_lines(text) {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let [_, _, name, value, kind] = fields[..] else {
                    return Err(format!("line {number}: expected 5 fields"));
                };
                let kind = kind
                    .parse::<u8>()
                    .ok()
                    .and_then(type_of_number)
                    .ok_or_else(|| format!("line {number}: unknown type '{kind}'"))?;
                let value = parse_as(kind, value)
                    .ok_or_else(|| format!("line {number}: '{value}' does not fit {name}"))?;
                values.insert(Ident::from_str_truncated(name), value);
            }
            Ok(values)
        }
    }
}

/// Non-blank, non-comment lines with their 1-based numbers.
fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

fn value_text(value: Value) -> String {
    crate::domain::parameter::value_to_string(value)
}

/// `MAV_PARAM_TYPE` of a value.
fn type_number(value: Value) -> u8 {
    match value {
        Value::U8(_) => 1,
        Value::I8(_) => 2,
        Value::U16(_) => 3,
        Value::I16(_) => 4,
        Value::U32(_) => 5,
        Value::I32(_) => 6,
        Value::F32(_) => 9,
    }
}

/// A zero value of the type `MAV_PARAM_TYPE` `number` names.
fn type_of_number(number: u8) -> Option<Value> {
    Some(match number {
        1 => Value::U8(0),
        2 => Value::I8(0),
        3 => Value::U16(0),
        4 => Value::I16(0),
        5 => Value::U32(0),
        6 => Value::I32(0),
        9 => Value::F32(0.0),
        _ => return None,
    })
}

/// Parse `text` as a value of `kind`'s type. Integer types accept decimal
/// notation (`1.000000`, as QGC writes) when the value is integral.
fn parse_as(kind: Value, text: &str) -> Option<Value> {
    if let Value::F32(_) = kind {
        return text
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Value::F32);
    }

    let number = text
        .parse::<f64>()
        .ok()
        .filter(|value| value.fract() == 0.0)?;
    crate::domain::parameter::value_parse_as(kind, &format!("{number:.0}"))
}

/// Serialize `parameters` as an INI document.
fn parameters_to_ini_string(parameters: &ParamValues) -> String {
    let mut config = ini::Ini::new();

    // Group parameters by their first segment
    for (key, value) in parameters.iter() {
        let parts: Vec<&str> = key.as_str().splitn(2, mav_param::SEP).collect();

        let (section, property) = if parts.len() > 1 {
            (Some(parts[0]), parts[1])
        } else {
            (None, parts[0])
        };

        // Convert value to string
        let type_value_str = match value {
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

    let mut buffer = Vec::new();
    config
        .write_to_opt(
            &mut buffer,
            ini::WriteOption {
                kv_separator: ": ",
                ..Default::default()
            },
        )
        .expect("writing an INI to a byte buffer is infallible");

    String::from_utf8(buffer).expect("the INI writer emits valid UTF-8")
}

/// Parse an INI document into a set of parameter values.
fn parameters_from_ini_str(text: &str) -> Result<ParamValues, ini::Error> {
    let mut reader = text.as_bytes();
    let conf = ini::Ini::read_from(&mut reader)?;
    let mut parameters = ParamValues::new();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_round_trips_typed_values_and_sections() {
        let mut parameters = ParamValues::new();
        parameters.insert(Ident::from_str_truncated("PARAM_U8"), Value::U8(7));
        parameters.insert(Ident::from_str_truncated("PARAM_F32"), Value::F32(1.5));
        parameters.insert(Ident::from_str_truncated("PARAM_I16"), Value::I16(-3));
        // A dotted ident exercises the section path: `GROUP` + `ITEM`.
        parameters.insert(Ident::from_str_truncated("GROUP.ITEM"), Value::U16(42));

        let text = parameters_to_ini_string(&parameters);
        assert!(text.contains("[GROUP]"), "dotted idents form a section");
        assert!(text.contains("ITEM"), "the dotted ident's property is kept");

        let parsed = parameters_from_ini_str(&text).expect("round trip should parse");

        assert_eq!(parsed.len(), parameters.len());
        for (ident, value) in parameters.iter() {
            let parsed_value = parsed.get(ident).expect("ident survives the round trip");
            assert_eq!(parsed_value, value, "value of {}", ident.as_str());
        }
    }

    const ADDRESS: MavlinkId = MavlinkId {
        system: 1,
        component: 1,
    };

    fn sample() -> ParamValues {
        let mut values = ParamValues::new();
        values.insert(Ident::from_str_truncated("ARMING_CHECK"), Value::I32(1));
        values.insert(Ident::from_str_truncated("WPNAV_SPEED"), Value::F32(500.5));
        values.insert(Ident::from_str_truncated("SERIAL1_BAUD"), Value::I16(57));
        values
    }

    #[test]
    fn formats_follow_the_extension() {
        assert_eq!(
            ParamFileFormat::of_file_extension(Path::new("a.param")),
            ParamFileFormat::MissionPlanner
        );
        assert_eq!(
            ParamFileFormat::of_file_extension(Path::new("a.PARM")),
            ParamFileFormat::MissionPlanner
        );
        assert_eq!(
            ParamFileFormat::of_file_extension(Path::new("a.params")),
            ParamFileFormat::QGroundControl
        );
        assert_eq!(
            ParamFileFormat::of_file_extension(Path::new("a.ini")),
            ParamFileFormat::GroundhogIni
        );
        assert_eq!(
            ParamFileFormat::of_file_extension(Path::new("a")),
            ParamFileFormat::GroundhogIni
        );
    }

    #[test]
    fn every_format_round_trips() {
        let values = sample();
        let reported = |ident: &Ident| values.get(ident).copied();

        for format in [
            ParamFileFormat::GroundhogIni,
            ParamFileFormat::MissionPlanner,
            ParamFileFormat::QGroundControl,
        ] {
            let text = write_parameters(format, &values, ADDRESS);
            let parsed = read_parameters(format, &text, reported).expect("round trip parses");
            assert_eq!(parsed, values, "{format:?}");
        }
    }

    #[test]
    fn mission_planner_files_take_the_reported_types() {
        let text = "# comment\nARMING_CHECK,1\nUNKNOWN,3\nWPNAV_SPEED 250\n";
        let reported = |ident: &Ident| sample().get(ident).copied();

        let parsed = read_parameters(ParamFileFormat::MissionPlanner, text, reported).unwrap();
        assert_eq!(parsed.len(), 2, "unknown parameters are skipped");
        assert_eq!(
            parsed[&Ident::from_str_truncated("ARMING_CHECK")],
            Value::I32(1)
        );
        assert_eq!(
            parsed[&Ident::from_str_truncated("WPNAV_SPEED")],
            Value::F32(250.0)
        );

        let error = read_parameters(
            ParamFileFormat::MissionPlanner,
            "ARMING_CHECK,1.5\n",
            reported,
        )
        .unwrap_err();
        assert!(error.contains("line 1"), "{error}");
    }

    #[test]
    fn qgc_files_carry_their_own_types() {
        let text = "# Vehicle-Id Component-Id Name Value Type\n1\t1\tRTL_ALT\t1500.000000000000000000\t6\n";
        let parsed = read_parameters(ParamFileFormat::QGroundControl, text, |_| None).unwrap();
        assert_eq!(
            parsed[&Ident::from_str_truncated("RTL_ALT")],
            Value::I32(1500)
        );
    }
}
