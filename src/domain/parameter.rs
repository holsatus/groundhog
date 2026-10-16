use std::collections::{BTreeMap, HashSet};

use mav_param::{Ident, Value};

use crate::foundation::BoxError;

#[derive(Clone, Debug)]
pub struct Parameter {
    value: Value,
    state: ParamState,
}

#[derive(Clone, Debug)]
pub enum ParamState {
    Unchanged,
    Changed {
        value: Option<Value>,
        status: ChangedStatus,
    },
    Uploading {
        value: Value,
    },
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
    pub fn set_edited(&mut self, value: Option<Value>) {
        if value != Some(self.value) {
            self.state = ParamState::Changed {
                value,
                status: ChangedStatus::None,
            };
        } else {
            self.state = ParamState::Unchanged
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
            value: Some(requested),
            status: ChangedStatus::Rejected,
        };
    }

    /// The upload transport failed; keep `requested`.
    pub fn upload_failed(&mut self, requested: Value) {
        self.state = ParamState::Changed {
            value: Some(requested),
            status: ChangedStatus::Failed,
        };
    }

    /// Record the value the target currently reports, without touching the
    /// local edit state.
    fn observe_value(&mut self, value: Value) {
        self.value = value;
    }
}

/// The outcome of folding an upload result onto a parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum UploadOutcome {
    /// No upload of the same requested value was in flight; ignored.
    Ignored,
    /// The target reports the requested value: accepted.
    Accepted,
    /// The target set a different value than requested (clamped/rejected).
    Clamped(Value),
    /// The transport did not deliver a verdict.
    Failed,
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
                log::warn!("Expected {exp} parameters, got {got}");
            }
        }
    }

    /// Fold an upload result onto the in-flight upload of `ident`.
    ///
    /// Only an upload that is in flight with the *same* `requested` value may
    /// land: a result for an unknown ident or a stale, mismatched, or absent
    /// upload returns [`UploadOutcome::Ignored`] and touches nothing. The
    /// result is only read: the caller keeps ownership, so diagnostics can
    /// still inspect the error.
    pub fn apply_upload_result(
        &mut self,
        ident: &Ident,
        requested: Value,
        result: &Result<Value, BoxError>,
    ) -> UploadOutcome {
        let Some(param) = self.map.get_mut(ident) else {
            return UploadOutcome::Ignored;
        };

        if !matches!(param.state(), ParamState::Uploading { value } if *value == requested) {
            return UploadOutcome::Ignored;
        }

        match result {
            Ok(value) if *value == requested => {
                param.upload_succeeded(*value);
                UploadOutcome::Accepted
            }
            Ok(reported) => {
                param.upload_rejected(*reported, requested);
                UploadOutcome::Clamped(*reported)
            }
            Err(_) => {
                param.upload_failed(requested);
                UploadOutcome::Failed
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

/// The namespace a parameter name belongs to: the prefix before the first
/// `'_'`, if any.
pub fn group_of(ident: &Ident) -> Option<&str> {
    ident.as_str().split_once('_').map(|(group, _)| group)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ident every test below uses for its in-flight parameter.
    fn ident() -> Ident {
        Ident::from_str_truncated("PARAM_ONE")
    }

    /// A set holding one parameter registered at `initial`, left `Uploading`
    /// the given `requested` value.
    fn uploading(requested: Value) -> (Parameters, Ident) {
        let ident = ident();
        let mut parameters = Parameters::default();
        parameters.insert(ident.clone(), Value::F32(1.0));
        parameters
            .get_mut(&ident)
            .expect("just inserted")
            .begin_upload(requested);
        (parameters, ident)
    }

    #[test]
    fn upload_result_echoed_is_accepted() {
        let (mut parameters, ident) = uploading(Value::F32(2.0));

        let outcome = parameters.apply_upload_result(&ident, Value::F32(2.0), &Ok(Value::F32(2.0)));

        assert_eq!(outcome, UploadOutcome::Accepted);
        let param = parameters.get(&ident).unwrap();
        assert_eq!(param.value(), Value::F32(2.0));
        assert!(matches!(param.state(), ParamState::Unchanged));
    }

    #[test]
    fn upload_result_reported_value_clamped_is_rejected() {
        let (mut parameters, ident) = uploading(Value::F32(5.0));

        // The target clamped the request to `3.0`.
        let outcome = parameters.apply_upload_result(&ident, Value::F32(5.0), &Ok(Value::F32(3.0)));

        assert_eq!(outcome, UploadOutcome::Clamped(Value::F32(3.0)));
        let param = parameters.get(&ident).unwrap();
        assert_eq!(param.value(), Value::F32(3.0), "the reported value is kept");
        match param.state() {
            ParamState::Changed { value, status } => {
                assert_eq!(*value, Some(Value::F32(5.0)), "the request stays pending");
                assert_eq!(*status, ChangedStatus::Rejected);
            }
            state => panic!("expected Changed(Rejected), got {state:?}"),
        }
    }

    #[test]
    fn upload_result_transport_error_fails() {
        let (mut parameters, ident) = uploading(Value::F32(7.0));

        let outcome =
            parameters.apply_upload_result(&ident, Value::F32(7.0), &Err("timeout".into()));

        assert_eq!(outcome, UploadOutcome::Failed);
        let param = parameters.get(&ident).unwrap();
        assert_eq!(param.value(), Value::F32(1.0), "the target set nothing");
        match param.state() {
            ParamState::Changed { value, status } => {
                assert_eq!(*value, Some(Value::F32(7.0)), "the request stays pending");
                assert_eq!(*status, ChangedStatus::Failed);
            }
            state => panic!("expected Changed(Failed), got {state:?}"),
        }
    }

    #[test]
    fn upload_result_without_an_in_flight_upload_is_ignored() {
        let mut parameters = Parameters::default();
        let ident = ident();
        parameters.insert(ident.clone(), Value::F32(1.0));

        let outcome = parameters.apply_upload_result(&ident, Value::F32(9.0), &Ok(Value::F32(9.0)));

        assert_eq!(outcome, UploadOutcome::Ignored);
        // Ignored means untouched: no verdict may be recorded.
        let param = parameters.get(&ident).unwrap();
        assert_eq!(param.value(), Value::F32(1.0));
        assert!(matches!(param.state(), ParamState::Unchanged));
    }

    #[test]
    fn upload_result_for_a_mismatched_in_flight_value_is_ignored() {
        let (mut parameters, ident) = uploading(Value::F32(5.0));

        // Only `5.0` is in flight; a result claimed for `9.0` is stale.
        let outcome = parameters.apply_upload_result(&ident, Value::F32(9.0), &Ok(Value::F32(9.0)));

        assert_eq!(outcome, UploadOutcome::Ignored);
        let param = parameters.get(&ident).unwrap();
        assert_eq!(param.value(), Value::F32(1.0));
        assert!(matches!(
            param.state(),
            ParamState::Uploading { value } if *value == Value::F32(5.0)
        ));
    }

    #[test]
    fn upload_result_for_an_unknown_ident_is_ignored() {
        let (mut parameters, word) = uploading(Value::F32(2.0));

        let missing = Ident::from_str_truncated("PARAM_MISSING");
        let outcome =
            parameters.apply_upload_result(&missing, Value::F32(2.0), &Ok(Value::F32(2.0)));

        assert_eq!(outcome, UploadOutcome::Ignored);
        // The in-flight upload of the known ident keeps its state.
        let param = parameters.get(&word).unwrap();
        assert!(matches!(
            param.state(),
            ParamState::Uploading { value } if *value == Value::F32(2.0)
        ));
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
