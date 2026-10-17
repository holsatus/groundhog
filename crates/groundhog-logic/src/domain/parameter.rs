//! Parameters: what a component reports, and the operator's pending edits.
//!
//! [`Parameters`] is vehicle state: the values a component reported, kept with
//! their raw wire form so they can be re-decoded if the component's encoding
//! only becomes known later. [`ParamEdit`] is operator state: a typed-in value
//! and the fate of its upload. The two live in different places — the first in
//! the fleet, the second in the parameter feature — and meet only when an
//! upload result is folded in.

use std::collections::{BTreeMap, HashSet};

use mav_param::{Ident, Value};
use mavio::default_dialect::enums::MavParamType;

use crate::wire::param::ParamEncoding;

/// A plain set of parameter values, as read from or written to a file.
pub type ParamValues = BTreeMap<Ident, Value>;

/// One parameter as its component reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportedParam {
    value: Value,
    raw: f32,
    raw_type: MavParamType,
}

impl ReportedParam {
    /// The value, decoded with the component's encoding.
    pub fn value(&self) -> Value {
        self.value
    }
}

/// Where a component's parameter list download stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DownloadState {
    #[default]
    Idle,
    InProgress,
    Complete,
    Failed(String),
}

/// The parameters of one component.
#[derive(Debug, Clone, Default)]
pub struct Parameters {
    map: BTreeMap<Ident, ReportedParam>,
    loading: LoadingState,
    /// The encoding values are decoded with, once known (advertised by the
    /// component, or implied by its firmware). Until then values are decoded
    /// bytewise for display, and writes are refused.
    encoding: Option<ParamEncoding>,
    download: DownloadState,
}

impl Parameters {
    pub fn iter(&self) -> impl Iterator<Item = (&Ident, &ReportedParam)> {
        self.map.iter()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, id: &Ident) -> Option<&ReportedParam> {
        self.map.get(id)
    }

    pub fn value(&self, id: &Ident) -> Option<Value> {
        self.map.get(id).map(ReportedParam::value)
    }

    pub fn loading(&self) -> &LoadingState {
        &self.loading
    }

    pub fn download(&self) -> &DownloadState {
        &self.download
    }

    /// Every value, for saving to a file.
    pub fn values(&self) -> ParamValues {
        self.map
            .iter()
            .map(|(ident, param)| (ident.clone(), param.value))
            .collect()
    }

    /// Record a reported value, decoded with `encoding` (or bytewise for
    /// display while the encoding is unknown).
    pub(crate) fn observe(
        &mut self,
        ident: Ident,
        raw: f32,
        raw_type: MavParamType,
        index: u16,
        count: u16,
        encoding: Option<ParamEncoding>,
    ) -> Result<(), UnsupportedType> {
        // Keep every stored value decoded the same way as this one.
        self.set_encoding(encoding);

        let value = encoding
            .unwrap_or(ParamEncoding::Bytewise)
            .decode(raw, raw_type)
            .ok_or(UnsupportedType(raw_type))?;

        self.map.insert(
            ident,
            ReportedParam {
                value,
                raw,
                raw_type,
            },
        );

        // Unsolicited echoes (of a set) carry index 65535 and say nothing
        // about the list.
        if index != u16::MAX {
            self.loading.note(index, count);
        }

        Ok(())
    }

    /// Re-decode every value under `encoding`, now that it is known. A no-op
    /// when it matches the current one.
    pub(crate) fn set_encoding(&mut self, encoding: Option<ParamEncoding>) {
        if encoding == self.encoding {
            return;
        }

        self.encoding = encoding;
        let decoder = encoding.unwrap_or(ParamEncoding::Bytewise);
        for param in self.map.values_mut() {
            if let Some(value) = decoder.decode(param.raw, param.raw_type) {
                param.value = value;
            }
        }
    }

    /// Note that a list download started or ended.
    pub(crate) fn set_download(&mut self, state: DownloadState) {
        if state == DownloadState::InProgress {
            self.loading.has_loaded.clear();
        }
        self.download = state;
    }
}

/// A parameter type that cannot be represented, such as a 64-bit integer.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("unsupported parameter type {0:?}")]
pub struct UnsupportedType(pub MavParamType);

/// Which parameters of an advertised list have arrived.
#[derive(Debug, Clone, Default)]
pub struct LoadingState {
    has_loaded: HashSet<u16>,
    expected_count: u16,
}

impl LoadingState {
    fn note(&mut self, index: u16, count: u16) {
        if count == 0 || index >= count {
            return;
        }
        self.has_loaded.insert(index);
        self.expected_count = count;
    }

    /// How many parameters have been received so far.
    pub fn loaded(&self) -> usize {
        self.has_loaded.len()
    }

    /// How many parameters the target advertised.
    pub fn expected(&self) -> u16 {
        self.expected_count
    }

    pub fn is_complete(&self) -> bool {
        self.expected_count > 0 && self.has_loaded.len() >= self.expected_count as usize
    }
}

/// What became of an upload, once its result is folded in.
#[derive(Debug, Clone, PartialEq)]
pub enum EditStatus {
    /// Typed in, not uploaded.
    Pending,
    /// An upload of the value is in flight.
    Uploading,
    /// The component set a different value than requested (clamped or
    /// refused).
    Rejected { reported: Value },
    /// The upload did not complete.
    Failed(String),
}

/// The operator's pending change to one parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamEdit {
    /// The text as typed, kept verbatim so partial input survives.
    pub text: String,
    /// The text parsed as the parameter's type, if it parses.
    pub value: Option<Value>,
    pub status: EditStatus,
}

impl ParamEdit {
    /// An edit of a parameter currently at `current` to `text`, or `None` when
    /// `text` parses to the current value (nothing to change).
    pub fn new(current: Value, text: String) -> Option<ParamEdit> {
        let value = value_parse_as(current, &text);

        if value == Some(current) {
            return None;
        }

        Some(ParamEdit {
            text,
            value,
            status: EditStatus::Pending,
        })
    }

    /// An edit to `value`, as loaded from a file.
    pub fn to_value(value: Value) -> ParamEdit {
        ParamEdit {
            text: value_to_string(value),
            value: Some(value),
            status: EditStatus::Pending,
        }
    }

    /// The value to upload, if the edit is ready to be uploaded.
    pub fn uploadable(&self) -> Option<Value> {
        match self.status {
            EditStatus::Uploading => None,
            _ => self.value,
        }
    }

    pub fn is_uploading(&self) -> bool {
        self.status == EditStatus::Uploading
    }

    /// Fold an upload result in. Returns `true` when the edit is done (the
    /// component now holds the requested value) and should be dropped.
    pub fn finish_upload(&mut self, requested: Value, result: Result<Value, String>) -> bool {
        match result {
            Ok(reported) if reported == requested => true,
            Ok(reported) => {
                self.status = EditStatus::Rejected { reported };
                false
            }
            Err(error) => {
                self.status = EditStatus::Failed(error);
                false
            }
        }
    }
}

pub fn value_type_matches(lhs: Value, rhs: Value) -> bool {
    std::mem::discriminant(&lhs) == std::mem::discriminant(&rhs)
}

/// Parse `text` as a value of the same type as `kind`.
pub fn value_parse_as(kind: Value, text: &str) -> Option<Value> {
    let text = text.trim();
    let value = match kind {
        Value::U8(_) => Value::U8(text.parse::<u8>().ok()?),
        Value::I8(_) => Value::I8(text.parse::<i8>().ok()?),
        Value::U16(_) => Value::U16(text.parse::<u16>().ok()?),
        Value::I16(_) => Value::I16(text.parse::<i16>().ok()?),
        Value::U32(_) => Value::U32(text.parse::<u32>().ok()?),
        Value::I32(_) => Value::I32(text.parse::<i32>().ok()?),
        Value::F32(_) => Value::F32(text.parse::<f32>().ok().filter(|value| value.is_finite())?),
    };

    Some(value)
}

/// A value formatted for display and editing.
pub fn value_to_string(value: Value) -> String {
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

/// The display name of a value's type, e.g. `"f32"`.
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

/// The namespace a parameter name belongs to: the prefix before the first
/// `'_'`, if any.
pub fn group_of(ident: &Ident) -> Option<&str> {
    ident.as_str().split_once('_').map(|(group, _)| group)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident() -> Ident {
        Ident::from_str_truncated("PARAM_ONE")
    }

    #[test]
    fn a_late_encoding_re_decodes_every_value() {
        let mut parameters = Parameters::default();
        // ArduPilot sends INT32 1 as the float 1.0 (C cast).
        parameters
            .observe(ident(), 1.0, MavParamType::Int32, 0, 1, None)
            .unwrap();
        assert_eq!(
            parameters.value(&ident()),
            Some(Value::I32(1.0f32.to_bits() as i32)),
            "decoded bytewise while the encoding is unknown"
        );
        assert_eq!(parameters.encoding, None);

        parameters.set_encoding(Some(ParamEncoding::CCast));
        assert_eq!(parameters.value(&ident()), Some(Value::I32(1)));
    }

    #[test]
    fn echoes_do_not_count_towards_the_list() {
        let mut parameters = Parameters::default();
        parameters
            .observe(
                ident(),
                2.0,
                MavParamType::Real32,
                u16::MAX,
                5,
                Some(ParamEncoding::CCast),
            )
            .unwrap();
        assert_eq!(parameters.loading().loaded(), 0);
        assert_eq!(parameters.value(&ident()), Some(Value::F32(2.0)));
    }

    #[test]
    fn loading_tracks_completion() {
        let mut parameters = Parameters::default();
        for index in 0..3 {
            let name = Ident::from_str_truncated(&format!("P{index}"));
            parameters
                .observe(
                    name,
                    0.0,
                    MavParamType::Real32,
                    index,
                    3,
                    Some(ParamEncoding::CCast),
                )
                .unwrap();
        }
        assert!(parameters.loading().is_complete());

        parameters.set_download(DownloadState::InProgress);
        assert!(
            !parameters.loading().is_complete(),
            "a new download starts over"
        );
    }

    #[test]
    fn an_edit_to_the_current_value_is_no_edit() {
        assert!(ParamEdit::new(Value::F32(1.0), "1.0".into()).is_none());
        let edit = ParamEdit::new(Value::F32(1.0), "2".into()).unwrap();
        assert_eq!(edit.value, Some(Value::F32(2.0)));
        let partial = ParamEdit::new(Value::U8(1), "30".into()).unwrap();
        assert_eq!(partial.value, Some(Value::U8(30)));
        let invalid = ParamEdit::new(Value::U8(1), "300".into()).unwrap();
        assert_eq!(invalid.value, None, "out of range keeps the text only");
        assert_eq!(invalid.uploadable(), None);
    }

    #[test]
    fn upload_results_fold_into_the_edit() {
        let mut edit = ParamEdit::new(Value::F32(1.0), "5".into()).unwrap();
        edit.status = EditStatus::Uploading;
        assert_eq!(edit.uploadable(), None, "no double upload");

        assert!(!edit.finish_upload(Value::F32(5.0), Ok(Value::F32(3.0))));
        assert_eq!(
            edit.status,
            EditStatus::Rejected {
                reported: Value::F32(3.0)
            }
        );

        assert!(!edit.finish_upload(Value::F32(5.0), Err("timed out".into())));
        assert!(matches!(edit.status, EditStatus::Failed(_)));
        assert_eq!(edit.uploadable(), Some(Value::F32(5.0)), "can retry");

        assert!(edit.finish_upload(Value::F32(5.0), Ok(Value::F32(5.0))));
    }

    #[test]
    fn group_of_splits_at_the_first_underscore() {
        assert_eq!(
            group_of(&Ident::from_str_truncated("BAT_CAPACITY")),
            Some("BAT")
        );
        assert_eq!(group_of(&Ident::from_str_truncated("A_B_C")), Some("A"));
        assert_eq!(group_of(&Ident::from_str_truncated("NOUNDERSCORE")), None);
    }
}
