//! Parameters: what a component reports, and the operator's pending edits.
//!
//! [`Parameters`] is vehicle state: the values a component reported, kept with
//! their raw wire form so they can be re-decoded if the component's encoding
//! only becomes known later. [`ParamEdit`] is operator state: a typed-in value
//! and the fate of its upload, collected per component in [`ParamEdits`]. The
//! two live in different places — the first in the fleet, the second in the
//! frontend — and meet only where an edit is compared with the reported value
//! or an upload result is folded in.

use std::collections::{BTreeMap, HashSet};

use mav_param::{Ident, Value};
use mavio::default_dialect::enums::MavParamType;

use crate::Progress;

/// How a component encodes parameter values on the wire: vocabulary for
/// [`Vehicle::param_encoding`](super::vehicle::Vehicle::param_encoding) and
/// the firmware adapters. Encoding and decoding stay inside the crate.
pub use crate::wire::param::ParamEncoding;

/// A plain set of parameter values, as read from or written to a file.
pub type ParamValues = BTreeMap<Ident, Value>;

/// One parameter as its component reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportedParam {
    value: Value,
    raw: f32,
    raw_type: MavParamType,
    /// The download generation this value was last reported in.
    generation: u64,
}

impl ReportedParam {
    /// The value, decoded with the component's encoding.
    pub fn value(&self) -> Value {
        self.value
    }
}

/// How a component's parameter download ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadOutcome {
    /// Every parameter the component advertised arrived.
    Complete,
    /// The download ended short of the advertised count.
    Incomplete(Progress),
    /// Nothing arrived. The component may report no parameters, or may not
    /// have answered; the protocol cannot tell them apart, so the panel gives
    /// a reason and offers a retry rather than calling it a failure.
    NoParameters,
    /// The transfer failed; values received so far are kept.
    Failed(String),
    /// The operator stopped it; values received so far are kept.
    Cancelled,
}

/// A component's parameter list download: how far it has come, and how it
/// ended.
///
/// Every download has a generation, and every value remembers the one it was
/// last reported in. A download that completes removes the values it did not
/// report; one that fails, is cancelled, or brings nothing keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamDownload {
    generation: u64,
    running: bool,
    expected: u16,
    received: HashSet<u16>,
    refused: u32,
    outcome: Option<DownloadOutcome>,
}

impl ParamDownload {
    /// Whether a download is in flight.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// How many parameters the target advertised, or 0 while unknown.
    pub fn expected(&self) -> u16 {
        self.expected
    }

    /// How many distinct list indices arrived in this download.
    pub fn received(&self) -> usize {
        self.received.len()
    }

    /// How many of them this build could not decode.
    pub fn refused(&self) -> u32 {
        self.refused
    }

    /// How the last download ended, if it did.
    pub fn outcome(&self) -> Option<&DownloadOutcome> {
        self.outcome.as_ref()
    }

    /// The generation values are stamped with.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    fn start(&mut self) {
        self.generation += 1;
        self.running = true;
        self.expected = 0;
        self.received.clear();
        self.refused = 0;
        self.outcome = None;
    }

    /// Note that list index `index` of `count` arrived, refused or not.
    fn note(&mut self, index: u16, count: u16, refused: bool) {
        if count == 0 || index >= count {
            return;
        }

        self.received.insert(index);
        self.expected = count;
        if refused {
            self.refused += 1;
        }
    }

    fn finish(&mut self, outcome: DownloadOutcome) {
        self.running = false;
        self.outcome = Some(outcome);
    }
}

/// A change to a component's download, reported by the service running it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadUpdate {
    /// A download started: a new generation begins.
    Start,
    /// A download ended.
    Finish(DownloadOutcome),
}

/// The parameters of one component.
#[derive(Debug, Clone, Default)]
pub struct Parameters {
    map: BTreeMap<Ident, ReportedParam>,
    /// The encoding values are decoded with, once known (advertised by the
    /// component, or implied by its firmware). Until then values are decoded
    /// bytewise for display, and writes are refused.
    encoding: Option<ParamEncoding>,
    download: ParamDownload,
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

    pub fn download(&self) -> &ParamDownload {
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

        let decoded = encoding
            .unwrap_or(ParamEncoding::Bytewise)
            .decode(raw, raw_type);

        // Unsolicited echoes (of a set) carry index 65535 and say nothing
        // about the list. A list index counts as arrived even when its value
        // cannot be decoded: the download did receive it.
        if index != u16::MAX {
            self.download.note(index, count, decoded.is_none());
        }

        let value = decoded.ok_or(UnsupportedType(raw_type))?;
        self.map.insert(
            ident,
            ReportedParam {
                value,
                raw,
                raw_type,
                generation: self.download.generation(),
            },
        );

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

    /// Fold a download's start or outcome in. A completed download drops the
    /// values it did not report: the component no longer has them.
    pub(crate) fn set_download(&mut self, update: DownloadUpdate) {
        match update {
            DownloadUpdate::Start => self.download.start(),
            DownloadUpdate::Finish(outcome) => {
                let generation = self.download.generation;
                let complete = outcome == DownloadOutcome::Complete;

                self.download.finish(outcome);
                if complete {
                    self.map.retain(|_, param| param.generation == generation);
                }
            }
        }
    }

    /// Whether reporting this value would change nothing: an unsolicited echo
    /// of the value and type already stored, under the same encoding.
    pub(crate) fn reports_nothing_new(
        &self,
        ident: &Ident,
        raw: f32,
        raw_type: MavParamType,
        index: u16,
        encoding: Option<ParamEncoding>,
    ) -> bool {
        index == u16::MAX
            && self.encoding == encoding
            && self.map.get(ident).is_some_and(|param| {
                param.generation == self.download.generation
                    && param.raw.to_bits() == raw.to_bits()
                    && param.raw_type == raw_type
            })
    }
}

/// A parameter type that cannot be represented, such as a 64-bit integer.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("unsupported parameter type {0:?}")]
pub struct UnsupportedType(pub MavParamType);

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
///
/// The typed text is kept verbatim, even where it parses to the value the
/// edit was made against (`1.` while the parameter is `1`): the operator is
/// still typing, and taking the text back would swallow what they type next.
/// Such an edit changes nothing, so it is neither uploadable nor a change.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamEdit {
    /// The text as typed, kept verbatim so partial input survives.
    pub text: String,
    /// The text parsed as the parameter's type, if it parses.
    pub value: Option<Value>,
    pub status: EditStatus,
    /// The reported value the edit was made against.
    base: Value,
    /// The upload of this edit in flight, if any.
    upload: Option<Upload>,
}

/// One upload of one edit: handed out by [`ParamEdits::begin_upload`], and
/// given back to [`ParamEdits::finish_upload`] with its result. A result whose
/// upload is no longer the edit's (the edit was replaced, reset or uploaded
/// again meanwhile) is ignored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Upload {
    /// The value sent.
    pub value: Value,
    id: u64,
}

impl ParamEdit {
    /// An edit of a parameter currently at `current` to `text`, or `None` when
    /// `text` is the current value as it is shown (the operator typed it
    /// back).
    pub fn new(current: Value, text: String) -> Option<ParamEdit> {
        if text.trim() == value_to_string(current) {
            return None;
        }

        Some(ParamEdit {
            value: value_parse_as(current, &text),
            text,
            status: EditStatus::Pending,
            base: current,
            upload: None,
        })
    }

    /// An edit to `value`, as loaded from a file, of a parameter reported at
    /// `current`.
    pub fn to_value(value: Value, current: Value) -> ParamEdit {
        ParamEdit {
            text: value_to_string(value),
            value: Some(value),
            status: EditStatus::Pending,
            base: current,
            upload: None,
        }
    }

    /// The value to upload, if the edit is ready to be uploaded: it parses,
    /// differs from the value it was made against, and is not uploading.
    pub fn uploadable(&self) -> Option<Value> {
        match self.status {
            EditStatus::Uploading => None,
            _ => self.value.filter(|value| *value != self.base),
        }
    }

    /// Whether the operator would call this parameter changed: its text does
    /// not parse, parses to another value, or an upload of it is in flight or
    /// did not succeed.
    pub fn is_change(&self) -> bool {
        self.status != EditStatus::Pending || self.value != Some(self.base)
    }

    pub fn is_uploading(&self) -> bool {
        self.upload.is_some()
    }

    /// The upload in flight, if any.
    pub fn upload(&self) -> Option<Upload> {
        self.upload
    }

    /// Fold an upload result in. Returns `true` when the edit is done (the
    /// component now holds the requested value) and should be dropped.
    fn finish_upload(&mut self, requested: Value, result: Result<Value, String>) -> bool {
        self.upload = None;
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

/// The operator's pending edits to one component's parameters.
///
/// Uploads are the frontend's to run: [`ParamEdits::begin_upload`] hands out
/// an [`Upload`] with the value to send, and [`ParamEdits::finish_upload`]
/// folds its result in. Editing, loading or resetting a parameter replaces its
/// edit, and a late result of the old edit's upload is then ignored, so a
/// frontend cancels superseded uploads only to stop sending them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamEdits {
    edits: BTreeMap<Ident, ParamEdit>,
    /// The id of the last upload handed out.
    last_upload: u64,
}

/// How many entries of a parameter file became edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadSummary {
    /// Values that differ from the reported ones, now pending edits.
    pub changed: usize,
    /// Values for parameters the component does not report, or of another
    /// type than it reports.
    pub skipped: usize,
}

/// How an upload ended, as folded into the edits.
#[derive(Debug, Clone, PartialEq)]
pub enum UploadOutcome {
    /// The component holds the requested value; the edit is done and gone.
    Set,
    /// The component holds `reported` instead (clamped or refused). The edit
    /// stays, marked [`EditStatus::Rejected`].
    Rejected { requested: Value, reported: Value },
    /// The upload did not complete. The edit stays, marked
    /// [`EditStatus::Failed`], and can be retried.
    Failed(String),
}

impl ParamEdits {
    pub fn get(&self, ident: &Ident) -> Option<&ParamEdit> {
        self.edits.get(ident)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Ident, &ParamEdit)> {
        self.edits.iter()
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// The operator typed `text` for `ident`, which the component reports at
    /// `current`. The current value typed back as shown, or a parameter the
    /// component does not report, leaves no edit.
    pub fn edit(&mut self, ident: Ident, current: Option<Value>, text: String) {
        match current.and_then(|current| ParamEdit::new(current, text)) {
            Some(edit) => {
                self.edits.insert(ident, edit);
            }
            None => {
                self.edits.remove(&ident);
            }
        }
    }

    /// Drop the edit of `ident`. Returns whether there was one.
    pub fn reset(&mut self, ident: &Ident) -> bool {
        self.edits.remove(ident).is_some()
    }

    /// Turn values loaded from a file into edits, where they differ from what
    /// the component reports (`None`: it reports nothing yet). Values of
    /// parameters it does not report, or of another type, are skipped: a file
    /// cannot add parameters, nor change their type.
    pub fn load(&mut self, reported: Option<&Parameters>, loaded: &ParamValues) -> LoadSummary {
        let mut summary = LoadSummary {
            changed: 0,
            skipped: 0,
        };

        for (ident, value) in loaded {
            match reported.and_then(|reported| reported.value(ident)) {
                Some(current) if value_type_matches(current, *value) => {
                    if current != *value {
                        self.edits
                            .insert(ident.clone(), ParamEdit::to_value(*value, current));
                        summary.changed += 1;
                    }
                }
                _ => summary.skipped += 1,
            }
        }

        summary
    }

    /// How many parameters the operator changed ([`ParamEdit::is_change`]).
    pub fn changes(&self) -> usize {
        self.edits.values().filter(|edit| edit.is_change()).count()
    }

    /// The edits that can be uploaded now: parsed, changed, and not
    /// uploading.
    pub fn uploadable(&self) -> impl Iterator<Item = (&Ident, Value)> {
        self.edits
            .iter()
            .filter_map(|(ident, edit)| Some((ident, edit.uploadable()?)))
    }

    /// Mark the edit of `ident` as uploading and return the upload to run,
    /// or `None` when there is nothing to upload (no edit, text that does not
    /// parse or changes nothing, or an upload already in flight).
    pub fn begin_upload(&mut self, ident: &Ident) -> Option<Upload> {
        let edit = self.edits.get_mut(ident)?;
        let value = edit.uploadable()?;
        self.last_upload += 1;
        let upload = Upload {
            value,
            id: self.last_upload,
        };
        edit.status = EditStatus::Uploading;
        edit.upload = Some(upload);
        Some(upload)
    }

    /// Fold the result of `upload` of `ident` in: what the component reported
    /// holding afterwards, or why the upload failed. `None`, changing
    /// nothing, when `upload` is not the edit's upload in flight: the edit
    /// was replaced or reset meanwhile.
    pub fn finish_upload<E: std::fmt::Display>(
        &mut self,
        ident: &Ident,
        upload: Upload,
        result: Result<Value, E>,
    ) -> Option<UploadOutcome> {
        let edit = self
            .edits
            .get_mut(ident)
            .filter(|edit| edit.upload == Some(upload))?;
        let requested = upload.value;
        let outcome = match &result {
            Ok(reported) if *reported == requested => UploadOutcome::Set,
            Ok(reported) => UploadOutcome::Rejected {
                requested,
                reported: *reported,
            },
            Err(error) => UploadOutcome::Failed(error.to_string()),
        };

        if edit.finish_upload(requested, result.map_err(|error| error.to_string())) {
            self.edits.remove(ident);
        }
        Some(outcome)
    }
}

fn value_type_matches(lhs: Value, rhs: Value) -> bool {
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
        assert_eq!(parameters.download().received(), 0);
        assert_eq!(parameters.value(&ident()), Some(Value::F32(2.0)));
    }

    #[test]
    fn a_download_tracks_what_arrived() {
        let mut parameters = Parameters::default();
        parameters.set_download(DownloadUpdate::Start);

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

        let download = parameters.download();
        assert!(download.is_running());
        assert_eq!(download.received(), 3);
        assert_eq!(download.expected(), 3);
        assert_eq!(download.refused(), 0);

        parameters.set_download(DownloadUpdate::Finish(DownloadOutcome::Complete));
        assert!(!parameters.download().is_running());
        assert_eq!(
            parameters.download().outcome(),
            Some(&DownloadOutcome::Complete)
        );
    }

    #[test]
    fn a_completed_download_drops_the_parameters_it_did_not_report() {
        let mut parameters = Parameters::default();
        let gone = Ident::from_str_truncated("PARAM_GONE");
        parameters
            .observe(ident(), 1.0, MavParamType::Real32, 0, 2, None)
            .unwrap();
        parameters
            .observe(gone.clone(), 2.0, MavParamType::Real32, 1, 2, None)
            .unwrap();

        parameters.set_download(DownloadUpdate::Start);
        parameters
            .observe(ident(), 3.0, MavParamType::Real32, 0, 1, None)
            .unwrap();
        parameters.set_download(DownloadUpdate::Finish(DownloadOutcome::Complete));

        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters.value(&ident()), Some(Value::F32(3.0)));
        assert!(parameters.value(&gone).is_none());
    }

    #[test]
    fn an_incomplete_download_keeps_the_parameters_it_did_not_report() {
        let mut parameters = Parameters::default();
        let kept = Ident::from_str_truncated("PARAM_KEPT");
        parameters
            .observe(kept.clone(), 2.0, MavParamType::Real32, 0, 2, None)
            .unwrap();

        parameters.set_download(DownloadUpdate::Start);
        parameters
            .observe(ident(), 3.0, MavParamType::Real32, 0, 2, None)
            .unwrap();
        parameters.set_download(DownloadUpdate::Finish(DownloadOutcome::Incomplete(
            crate::Progress { done: 1, total: 2 },
        )));

        assert_eq!(parameters.len(), 2);
        assert!(parameters.value(&kept).is_some());
    }

    #[test]
    fn a_refused_value_still_counts_as_arrived() {
        let mut parameters = Parameters::default();
        parameters.set_download(DownloadUpdate::Start);

        let error = parameters
            .observe(ident(), 1.0, MavParamType::Uint64, 0, 1, None)
            .expect_err("64-bit types are not representable");
        assert_eq!(error.0, MavParamType::Uint64);

        let download = parameters.download();
        assert_eq!(download.received(), 1);
        assert_eq!(download.refused(), 1);

        parameters.set_download(DownloadUpdate::Finish(DownloadOutcome::Complete));
        assert!(parameters.is_empty(), "the refused value is not stored");
    }

    #[test]
    fn an_edit_to_the_current_value_is_no_edit() {
        assert!(ParamEdit::new(Value::F32(1.0), "1".into()).is_none());
        assert!(ParamEdit::new(Value::F32(1.0), " 1 ".into()).is_none());
        let edit = ParamEdit::new(Value::F32(1.0), "2".into()).unwrap();
        assert_eq!(edit.value, Some(Value::F32(2.0)));
        let partial = ParamEdit::new(Value::U8(1), "30".into()).unwrap();
        assert_eq!(partial.value, Some(Value::U8(30)));
        let invalid = ParamEdit::new(Value::U8(1), "300".into()).unwrap();
        assert_eq!(invalid.value, None, "out of range keeps the text only");
        assert_eq!(invalid.uploadable(), None);
    }

    fn reported(params: &[(&str, f32)]) -> Parameters {
        let mut parameters = Parameters::default();
        for (index, (name, value)) in params.iter().enumerate() {
            parameters
                .observe(
                    Ident::from_str_truncated(name),
                    *value,
                    MavParamType::Real32,
                    index as u16,
                    params.len() as u16,
                    Some(ParamEncoding::CCast),
                )
                .unwrap();
        }
        parameters
    }

    #[test]
    fn editing_back_to_the_reported_value_drops_the_edit() {
        let mut edits = ParamEdits::default();
        edits.edit(ident(), Some(Value::F32(1.0)), "2".into());
        assert_eq!(edits.len(), 1);

        edits.edit(ident(), Some(Value::F32(1.0)), "1".into());
        assert!(edits.is_empty());

        edits.edit(ident(), None, "2".into());
        assert!(edits.is_empty(), "an unreported parameter takes no edit");
    }

    #[test]
    fn typed_text_that_parses_to_the_current_value_survives() {
        let mut edits = ParamEdits::default();
        edits.edit(ident(), Some(Value::F32(1.0)), "1.".into());

        let edit = edits.get(&ident()).expect("the text is kept");
        assert_eq!(edit.text, "1.");
        assert_eq!(edit.uploadable(), None, "it changes nothing");
        assert!(!edit.is_change());
        assert_eq!(edits.changes(), 0);

        edits.edit(ident(), Some(Value::F32(1.0)), "1.5".into());
        let edit = edits.get(&ident()).unwrap();
        assert_eq!(edit.uploadable(), Some(Value::F32(1.5)));
        assert_eq!(edits.changes(), 1);
    }

    #[test]
    fn changes_count_unparseable_and_uploading_edits() {
        let mut edits = ParamEdits::default();
        edits.edit(ident(), Some(Value::F32(1.0)), "abc".into());
        let other = Ident::from_str_truncated("PARAM_TWO");
        edits.edit(other.clone(), Some(Value::F32(1.0)), "2".into());
        edits.begin_upload(&other).unwrap();

        assert_eq!(edits.uploadable().count(), 0);
        assert_eq!(edits.changes(), 2);
    }

    #[test]
    fn unparseable_text_is_kept_but_not_uploadable() {
        let mut edits = ParamEdits::default();
        edits.edit(ident(), Some(Value::F32(1.0)), "abc".into());

        assert_eq!(edits.get(&ident()).unwrap().text, "abc");
        assert_eq!(edits.uploadable().count(), 0);
        assert_eq!(edits.begin_upload(&ident()), None);
    }

    #[test]
    fn uploads_fold_their_results_into_the_edits() {
        let mut edits = ParamEdits::default();
        edits.edit(ident(), Some(Value::F32(1.0)), "5".into());

        let upload = edits.begin_upload(&ident()).unwrap();
        assert_eq!(upload.value, Value::F32(5.0));
        assert_eq!(edits.begin_upload(&ident()), None, "already uploading");

        // The vehicle clamps the value: the edit stays, marked rejected.
        let outcome = edits.finish_upload(&ident(), upload, Ok::<_, String>(Value::F32(3.0)));
        assert_eq!(
            outcome,
            Some(UploadOutcome::Rejected {
                requested: upload.value,
                reported: Value::F32(3.0)
            })
        );
        assert!(matches!(
            edits.get(&ident()).unwrap().status,
            EditStatus::Rejected { .. }
        ));

        // A failure keeps the edit too, ready for a retry.
        let upload = edits.begin_upload(&ident()).unwrap();
        let outcome = edits.finish_upload(&ident(), upload, Err("timed out"));
        assert_eq!(outcome, Some(UploadOutcome::Failed("timed out".into())));

        // Retried and accepted: the edit is done.
        let upload = edits.begin_upload(&ident()).unwrap();
        let outcome = edits.finish_upload(&ident(), upload, Ok::<_, String>(upload.value));
        assert_eq!(outcome, Some(UploadOutcome::Set));
        assert!(edits.is_empty());

        // A result for an edit that is gone changes nothing.
        let late = edits.finish_upload(&ident(), upload, Ok::<_, String>(upload.value));
        assert_eq!(late, None);
    }

    /// Replacing an edit by `replace` while its upload runs leaves the new
    /// edit alone whatever the old upload's result.
    fn assert_replaced_edit_ignores_old_results(replace: impl Fn(&mut ParamEdits)) {
        let reported = reported(&[("PARAM_ONE", 1.0)]);
        for result in [Ok(Value::F32(5.0)), Err("timed out")] {
            let mut edits = ParamEdits::default();
            edits.edit(ident(), reported.value(&ident()), "5".into());
            let upload = edits.begin_upload(&ident()).unwrap();

            replace(&mut edits);
            let replaced = edits.get(&ident()).cloned();

            assert_eq!(edits.finish_upload(&ident(), upload, result), None);
            assert_eq!(edits.get(&ident()).cloned(), replaced);
        }
    }

    #[test]
    fn a_result_for_a_loaded_over_edit_is_ignored() {
        let reported = reported(&[("PARAM_ONE", 1.0)]);
        let loaded: ParamValues = [(ident(), Value::F32(7.0))].into_iter().collect();
        assert_replaced_edit_ignores_old_results(|edits| {
            edits.load(Some(&reported), &loaded);
        });
    }

    #[test]
    fn a_result_for_a_retyped_edit_is_ignored() {
        assert_replaced_edit_ignores_old_results(|edits| {
            edits.edit(ident(), Some(Value::F32(1.0)), "5".into());
        });
    }

    #[test]
    fn a_result_for_a_reset_and_retyped_edit_is_ignored() {
        assert_replaced_edit_ignores_old_results(|edits| {
            edits.reset(&ident());
            edits.edit(ident(), Some(Value::F32(1.0)), "6".into());
        });
    }

    #[test]
    fn loading_a_file_creates_edits_for_differences_only() {
        let reported = reported(&[("PARAM_ONE", 1.0), ("PARAM_TWO", 2.0), ("PARAM_THREE", 3.0)]);
        let loaded: ParamValues = [
            ("PARAM_ONE", Value::F32(1.0)),
            ("PARAM_TWO", Value::F32(4.0)),
            // Another type than the component reports.
            ("PARAM_THREE", Value::U8(1)),
            ("UNKNOWN", Value::F32(0.0)),
        ]
        .into_iter()
        .map(|(name, value)| (Ident::from_str_truncated(name), value))
        .collect();

        let mut edits = ParamEdits::default();
        let summary = edits.load(Some(&reported), &loaded);

        assert_eq!(
            summary,
            LoadSummary {
                changed: 1,
                skipped: 2
            }
        );
        let two = edits.get(&Ident::from_str_truncated("PARAM_TWO")).unwrap();
        assert_eq!(two.value, Some(Value::F32(4.0)));

        let mut none = ParamEdits::default();
        let summary = none.load(None, &loaded);
        assert_eq!(
            summary.skipped,
            loaded.len(),
            "nothing reported, nothing to change"
        );
        assert!(none.is_empty());
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
