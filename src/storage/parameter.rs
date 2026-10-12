//! The INI read/write grammar for a [`Parameters`] set.
//!
//! The format groups parameters by the first segment of their identifier and
//! stores each value tagged with its MAVLink type (for example `f32 = 1.5`).
//! Reading and writing go exclusively through the public [`Parameters`] API.

use mav_param::{Ident, Value};

use crate::domain::parameter::Parameters;

/// Serialize `parameters` as an INI document.
pub fn parameters_to_ini_string(parameters: &Parameters) -> String {
    let mut config = ini::Ini::new();

    // Group parameters by their first segment
    for (key, param) in parameters.iter() {
        let parts: Vec<&str> = key.as_str().splitn(2, mav_param::SEP).collect();

        let (section, property) = if parts.len() > 1 {
            (Some(parts[0]), parts[1])
        } else {
            (None, parts[0])
        };

        // Convert value to string
        let type_value_str = match param.value() {
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

/// Parse an INI document into a [`Parameters`] set.
pub fn parameters_from_ini_str(text: &str) -> Result<Parameters, ini::Error> {
    let mut reader = text.as_bytes();
    let conf = ini::Ini::read_from(&mut reader)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_round_trips_typed_values_and_sections() {
        let mut parameters = Parameters::default();
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
        for (ident, param) in parameters.iter() {
            let parsed_param = parsed.get(ident).expect("ident survives the round trip");
            assert_eq!(
                parsed_param.value(),
                param.value(),
                "value of {}",
                ident.as_str()
            );
        }
    }
}
