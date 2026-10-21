//! Window-local clip settings. No source objects, complete payloads or documents
//! are serialized, cloned or mutated by paint. The session worker owns commits.
use crate::arrangement_types::PlacementValue;
use eframe::egui;
use omniusong_core::contracts::{
    EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId, Validate,
};
use omniusong_core::native::{AudioFades, FadeCurve};
use omniusong_core::{
    AudioSourceWindow, ClipPatch, ClipTimelinePolicy, DocumentClipAddress, FieldChange,
    LegacyAudioClipPatch, NativeAudioClipPatch, ProjectDocument, ValueChange,
};
use serde_json::{Number, Value};

const LEGACY_TEXT: usize = 256;
const NATIVE_TEXT: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClipSelection {
    pub project_id: String,
    pub address: DocumentClipAddress,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldMode {
    Keep,
    Set,
    Remove,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueMode {
    Keep,
    Set,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClipField {
    pub mode: FieldMode,
    pub text: String,
}
impl ClipField {
    fn saved(value: Option<&Value>) -> Self {
        Self {
            mode: FieldMode::Keep,
            text: value
                .and_then(Value::as_number)
                .map(Number::as_str)
                .filter(|raw| raw.len() <= LEGACY_TEXT)
                .unwrap_or("")
                .into(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LegacyClipDraft {
    pub start_tick: ClipField,
    pub trim_start_seconds: ClipField,
    pub trim_end_seconds: ClipField,
    pub fade_in_seconds: ClipField,
    pub fade_out_seconds: ClipField,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NativeClipDraft {
    pub position_mode: ValueMode,
    pub position_frame: String,
    pub window_mode: ValueMode,
    pub source_start_frame: String,
    pub frame_count: String,
    pub fades_mode: FieldMode,
    pub fade_in_frames: String,
    pub fade_out_frames: String,
    pub curve: FadeCurve,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ClipDraftFields {
    LegacyAudio(LegacyClipDraft),
    NativeAudio(NativeClipDraft),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ClipSourceIdentity {
    LegacyAudio {
        asset_id: Option<String>,
        source_asset_id: Option<String>,
    },
    NativeAudio {
        take_id: String,
        manifest_sha256: String,
        stream_id: String,
        sample_rate: u32,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClipDraft {
    pub selection: ClipSelection,
    pub base_revision: u64,
    pub fields: ClipDraftFields,
    pub source: ClipSourceIdentity,
    pub extend_loop: bool,
    pub allow_upgrade: bool,
}
fn bounded(raw: &str, limit: usize, label: &str) -> Result<(), String> {
    if raw.chars().take(limit + 1).count() > limit {
        Err(format!("{label} is limited to {limit} characters"))
    } else {
        Ok(())
    }
}
fn integer<T: std::str::FromStr>(raw: &str, signed: bool, label: &str) -> Result<T, String> {
    bounded(raw, NATIVE_TEXT, label)?;
    let digits = if signed {
        raw.strip_prefix('-').unwrap_or(raw)
    } else {
        raw
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "{label} must be a whole {}integer",
            if signed { "signed " } else { "nonnegative " }
        ));
    }
    raw.parse()
        .map_err(|_| format!("{label} is outside its integer range"))
}
fn field<T>(
    draft: &ClipField,
    label: &str,
    parse: impl FnOnce(&str) -> Result<T, String>,
) -> Result<FieldChange<T>, String> {
    bounded(&draft.text, LEGACY_TEXT, label)?;
    Ok(match draft.mode {
        FieldMode::Keep => FieldChange::Keep,
        FieldMode::Remove => FieldChange::Remove,
        FieldMode::Set => FieldChange::Set {
            value: parse(&draft.text)?,
        },
    })
}
fn number(raw: &str, label: &str) -> Result<Number, String> {
    if raw.is_empty() || raw.trim() != raw {
        return Err(format!("{label} must be an exact JSON number"));
    }
    serde_json::from_str::<Number>(raw)
        .map_err(|_| format!("{label} must be a complete JSON number"))
}
impl ClipDraft {
    fn from_document(doc: &ProjectDocument, selection: ClipSelection) -> Result<Self, String> {
        let raw = clip(doc, &selection).ok_or("Selected audio clip is no longer present")?;
        let source =
            source_identity(doc, &selection).ok_or("Selected clip has no saved source identity")?;
        let fields = match &selection.address {
            DocumentClipAddress::LegacyAudio { .. } => {
                ClipDraftFields::LegacyAudio(LegacyClipDraft {
                    start_tick: ClipField::saved(raw.get("startTick")),
                    trim_start_seconds: ClipField::saved(raw.get("trimStartSeconds")),
                    trim_end_seconds: ClipField::saved(raw.get("trimEndSeconds")),
                    fade_in_seconds: ClipField::saved(raw.get("fadeInSeconds")),
                    fade_out_seconds: ClipField::saved(raw.get("fadeOutSeconds")),
                })
            }
            DocumentClipAddress::NativeAudio { .. } => {
                ClipDraftFields::NativeAudio(NativeClipDraft {
                    position_mode: ValueMode::Keep,
                    position_frame: raw["position"]["frame"]
                        .as_i64()
                        .ok_or("Invalid saved placement")?
                        .to_string(),
                    window_mode: ValueMode::Keep,
                    source_start_frame: raw["range"]["start"]
                        .as_u64()
                        .ok_or("Invalid saved source start")?
                        .to_string(),
                    frame_count: raw["range"]["count"]
                        .as_u64()
                        .ok_or("Invalid saved source count")?
                        .to_string(),
                    fades_mode: FieldMode::Keep,
                    fade_in_frames: raw["fades"]["inFrames"].as_u64().unwrap_or(0).to_string(),
                    fade_out_frames: raw["fades"]["outFrames"].as_u64().unwrap_or(0).to_string(),
                    curve: if raw["fades"]["curve"] == "equal_power" {
                        FadeCurve::EqualPower
                    } else {
                        FadeCurve::Linear
                    },
                })
            }
        };
        Ok(Self {
            selection,
            base_revision: doc.revision(),
            fields,
            source,
            extend_loop: false,
            allow_upgrade: false,
        })
    }
    pub fn dirty(&self) -> bool {
        self.extend_loop
            || match &self.fields {
                ClipDraftFields::LegacyAudio(d) => [
                    &d.start_tick,
                    &d.trim_start_seconds,
                    &d.trim_end_seconds,
                    &d.fade_in_seconds,
                    &d.fade_out_seconds,
                ]
                .iter()
                .any(|f| f.mode != FieldMode::Keep),
                ClipDraftFields::NativeAudio(d) => {
                    d.position_mode != ValueMode::Keep
                        || d.window_mode != ValueMode::Keep
                        || d.fades_mode != FieldMode::Keep
                }
            }
    }
    pub fn timeline(&self) -> ClipTimelinePolicy {
        if self.extend_loop {
            ClipTimelinePolicy::ExtendLoop {
                allow_native_schema_upgrade: self.allow_upgrade,
            }
        } else {
            ClipTimelinePolicy::PreserveLoop
        }
    }
    pub fn patch(&self) -> Result<ClipPatch, String> {
        let patch = match &self.fields {
            ClipDraftFields::LegacyAudio(d) => ClipPatch::LegacyAudio(LegacyAudioClipPatch {
                start_tick: field(&d.start_tick, "Clip start", |raw| {
                    integer(raw, false, "Clip start")
                })?,
                trim_start_seconds: field(&d.trim_start_seconds, "Source in", |raw| {
                    number(raw, "Source in")
                })?,
                trim_end_seconds: field(&d.trim_end_seconds, "Source out", |raw| {
                    number(raw, "Source out")
                })?,
                fade_in_seconds: field(&d.fade_in_seconds, "Fade in", |raw| {
                    number(raw, "Fade in")
                })?,
                fade_out_seconds: field(&d.fade_out_seconds, "Fade out", |raw| {
                    number(raw, "Fade out")
                })?,
            }),
            ClipDraftFields::NativeAudio(d) => {
                for (label, raw) in [
                    ("Placement", &d.position_frame),
                    ("Source start", &d.source_start_frame),
                    ("Source length", &d.frame_count),
                    ("Fade in", &d.fade_in_frames),
                    ("Fade out", &d.fade_out_frames),
                ] {
                    bounded(raw, NATIVE_TEXT, label)?;
                }
                ClipPatch::NativeAudio(NativeAudioClipPatch {
                    position_frame: match d.position_mode {
                        ValueMode::Keep => ValueChange::Keep,
                        ValueMode::Set => ValueChange::Set {
                            value: integer(&d.position_frame, true, "Placement")?,
                        },
                    },
                    source_window: match d.window_mode {
                        ValueMode::Keep => ValueChange::Keep,
                        ValueMode::Set => ValueChange::Set {
                            value: AudioSourceWindow {
                                start_frame: integer(&d.source_start_frame, false, "Source start")?,
                                frame_count: integer(&d.frame_count, false, "Source length")?,
                            },
                        },
                    },
                    fades: match d.fades_mode {
                        FieldMode::Keep => FieldChange::Keep,
                        FieldMode::Remove => FieldChange::Remove,
                        FieldMode::Set => FieldChange::Set {
                            value: AudioFades {
                                in_frames: integer(&d.fade_in_frames, false, "Fade in")?,
                                out_frames: integer(&d.fade_out_frames, false, "Fade out")?,
                                curve: d.curve,
                            },
                        },
                    },
                })
            }
        };
        patch.validate().map_err(|error| error.to_string())?;
        if !patch.matches_address(&self.selection.address) {
            return Err("Clip address and draft representations differ".into());
        }
        Ok(patch)
    }
    fn reapply(&mut self, attempted: &Self) -> Result<(), String> {
        match (&mut self.fields, &attempted.fields) {
            (ClipDraftFields::LegacyAudio(fresh), ClipDraftFields::LegacyAudio(old)) => {
                for (fresh, old) in [
                    (&mut fresh.start_tick, &old.start_tick),
                    (&mut fresh.trim_start_seconds, &old.trim_start_seconds),
                    (&mut fresh.trim_end_seconds, &old.trim_end_seconds),
                    (&mut fresh.fade_in_seconds, &old.fade_in_seconds),
                    (&mut fresh.fade_out_seconds, &old.fade_out_seconds),
                ] {
                    if old.mode != FieldMode::Keep {
                        *fresh = old.clone();
                    }
                }
            }
            (ClipDraftFields::NativeAudio(fresh), ClipDraftFields::NativeAudio(old)) => {
                if old.position_mode == ValueMode::Set {
                    fresh.position_mode = old.position_mode;
                    fresh.position_frame = old.position_frame.clone();
                }
                if old.window_mode == ValueMode::Set {
                    fresh.window_mode = old.window_mode;
                    fresh.source_start_frame = old.source_start_frame.clone();
                    fresh.frame_count = old.frame_count.clone();
                }
                if old.fades_mode != FieldMode::Keep {
                    fresh.fades_mode = old.fades_mode;
                    fresh.fade_in_frames = old.fade_in_frames.clone();
                    fresh.fade_out_frames = old.fade_out_frames.clone();
                    fresh.curve = old.curve;
                }
            }
            _ => return Err(
                "Saved clip representation changed; release the rejected choice and select afresh"
                    .into(),
            ),
        }
        self.extend_loop = attempted.extend_loop;
        self.allow_upgrade = attempted.allow_upgrade;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(super) enum ClipAction {
    Select(ClipSelection),
    BeginClip,
    Save,
    Discard,
    Restore(ClipSelection, ClipTimelinePolicy),
    Retry,
    Suspend,
    ReviewLatest,
    NewChoice,
    ReleaseRejected,
}
#[derive(Clone)]
pub(super) struct ClipFailure {
    pub intent: ProjectEditIntent,
    pub draft: Option<ClipDraft>,
    pub message: String,
    pub known_rejected: bool,
    pub suspended: bool,
    reviewed_revision: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClipContext {
    selection: Option<ClipSelection>,
    pub draft: Option<ClipDraft>,
    serial: u64,
}
#[derive(Default)]
pub(super) struct ClipEditor {
    pub selection: Option<ClipSelection>,
    pub draft: Option<ClipDraft>,
    pub failure: Option<ClipFailure>,
    pub notice: Option<String>,
    serial: u64,
    released: Option<ClipFailure>,
}
pub(super) fn track_id(address: &DocumentClipAddress) -> &str {
    match address {
        DocumentClipAddress::LegacyAudio { track_id }
        | DocumentClipAddress::NativeAudio { track_id, .. } => track_id,
    }
}
fn track<'a>(doc: &'a ProjectDocument, id: &str) -> Option<&'a Value> {
    doc.tracks()
        .iter()
        .find(|track| track["id"].as_str() == Some(id))
}
pub(super) fn clip<'a>(doc: &'a ProjectDocument, selection: &ClipSelection) -> Option<&'a Value> {
    if doc.id() != selection.project_id {
        return None;
    }
    let track = track(doc, track_id(&selection.address))?;
    match &selection.address {
        DocumentClipAddress::LegacyAudio { .. } if track["kind"] == "audio" => {
            track.get("audio").filter(|v| v.is_object())
        }
        DocumentClipAddress::NativeAudio { clip_id, .. }
            if doc.schema_version() == 2 && track["kind"] == "recording" =>
        {
            track["recording"]["clips"].as_array()?.iter().find(|clip| {
                clip["id"].as_str() == Some(clip_id) && clip["range"]["unit"] == "audio_frames"
            })
        }
        _ => None,
    }
}
fn source_identity(doc: &ProjectDocument, selection: &ClipSelection) -> Option<ClipSourceIdentity> {
    let raw = clip(doc, selection)?;
    Some(match selection.address {
        DocumentClipAddress::LegacyAudio { .. } => {
            let track = track(doc, track_id(&selection.address))?;
            ClipSourceIdentity::LegacyAudio {
                asset_id: track["assetId"].as_str().map(str::to_owned),
                source_asset_id: track["sourceAssetId"].as_str().map(str::to_owned),
            }
        }
        DocumentClipAddress::NativeAudio { .. } => ClipSourceIdentity::NativeAudio {
            take_id: raw["source"]["takeId"].as_str()?.into(),
            manifest_sha256: raw["source"]["manifestSha256"].as_str()?.into(),
            stream_id: raw["source"]["streamId"].as_str()?.into(),
            sample_rate: u32::try_from(raw["position"]["sampleRate"].as_u64()?).ok()?,
        },
    })
}
fn editable(doc: &ProjectDocument, selection: &ClipSelection) -> Result<(), String> {
    selection
        .address
        .validate()
        .map_err(|error| error.to_string())?;
    clip(doc, selection).ok_or("Selected audio clip is no longer present")?;
    if track(doc, track_id(&selection.address)).is_none_or(|track| track["locked"] != false) {
        return Err("Choose an unlocked audio clip".into());
    }
    Ok(())
}
impl ClipEditor {
    fn changed(&mut self) {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("clip draft serial exhausted");
    }
    pub fn context(&self) -> ClipContext {
        ClipContext {
            selection: self.selection.clone(),
            draft: self.draft.clone(),
            serial: self.serial,
        }
    }
    /// Stage only placement into the existing exact draft. No project/source
    /// mutation or service admission occurs here; Save keeps combined checks.
    pub fn stage_placement(
        &mut self,
        doc: &ProjectDocument,
        expected: &ClipContext,
        value: PlacementValue,
    ) -> Result<(), String> {
        if self.unresolved() {
            return Err("Resolve the retained clip edit before moving a clip".into());
        }
        if expected.serial != self.serial
            || expected.selection != self.selection
            || expected.draft != self.draft
        {
            return Err(
                "The clip selection or draft changed during the arrangement gesture".into(),
            );
        }
        let draft = self
            .draft
            .as_ref()
            .ok_or("Select an audio clip before moving it")?;
        if draft.base_revision != doc.revision()
            || draft.selection.project_id != doc.id()
            || self.selection.as_ref() != Some(&draft.selection)
        {
            return Err(
                "The saved arrangement changed; select the current clip before moving it".into(),
            );
        }
        editable(doc, &draft.selection)?;
        if source_identity(doc, &draft.selection).as_ref() != Some(&draft.source) {
            return Err("The clip source changed; select it afresh before moving it".into());
        }
        value.validate_for(&draft.selection.address)?;
        let raw = clip(doc, &draft.selection).ok_or("The arrangement clip disappeared")?;
        let current = match &draft.fields {
            ClipDraftFields::LegacyAudio(fields) => match fields.start_tick.mode {
                FieldMode::Keep => raw.get("startTick").and_then(Value::as_u64).or(Some(0)),
                FieldMode::Remove => Some(0),
                FieldMode::Set => integer::<u64>(&fields.start_tick.text, false, "Clip start").ok(),
            }
            .map(PlacementValue::LegacyTick),
            ClipDraftFields::NativeAudio(fields) => match fields.position_mode {
                ValueMode::Keep => raw["position"]["frame"].as_i64(),
                ValueMode::Set => integer::<i64>(&fields.position_frame, true, "Placement").ok(),
            }
            .map(PlacementValue::NativeFrame),
        };
        // Even explicit Remove/Set spelling must survive a no-op gesture.
        if current == Some(value) {
            return Ok(());
        }
        let draft = self.draft.as_mut().expect("validated clip draft");
        match (&mut draft.fields, value) {
            (ClipDraftFields::LegacyAudio(fields), PlacementValue::LegacyTick(value)) => {
                fields.start_tick.mode = FieldMode::Set;
                fields.start_tick.text = value.to_string();
            }
            (ClipDraftFields::NativeAudio(fields), PlacementValue::NativeFrame(value)) => {
                fields.position_mode = ValueMode::Set;
                fields.position_frame = value.to_string();
            }
            _ => return Err("Arrangement placement and clip representation differ".into()),
        }
        self.changed();
        Ok(())
    }
    pub fn dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(ClipDraft::dirty)
    }
    pub fn unresolved(&self) -> bool {
        self.failure.is_some()
    }
    pub fn select(
        &mut self,
        doc: &ProjectDocument,
        selection: ClipSelection,
    ) -> Result<(), String> {
        selection
            .address
            .validate()
            .map_err(|error| error.to_string())?;
        let draft = ClipDraft::from_document(doc, selection.clone())?;
        self.selection = Some(selection);
        self.draft = Some(draft);
        self.changed();
        Ok(())
    }
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.draft = None;
        self.changed();
    }
    pub fn discard(&mut self, doc: &ProjectDocument) {
        if self.failure.is_some() {
            return;
        }
        self.draft = None;
        if let Some(selection) = self.selection.clone() {
            let _ = self.select(doc, selection);
        }
        self.changed();
    }
    pub fn document_installed(&mut self, doc: &ProjectDocument) {
        if self
            .selection
            .as_ref()
            .is_some_and(|s| s.project_id != doc.id())
        {
            self.selection = None;
        }
        self.draft = None;
        if let Some(selection) = self.selection.clone() {
            let _ = self.select(doc, selection);
        }
        if let Some(failure) = &mut self.failure
            && failure.suspended
            && failure.known_rejected
            && failure.intent.project_id.0 == doc.id()
        {
            failure.reviewed_revision = Some(doc.revision());
        }
        self.changed();
    }
    pub fn intent(
        &self,
        doc: &ProjectDocument,
        action: &ClipAction,
    ) -> Result<ProjectEditIntent, String> {
        if self.unresolved() {
            return Err("Resolve the retained clip edit before making another clip change".into());
        }
        let (selection, edit) = match action {
            ClipAction::Save => {
                let draft = self.draft.as_ref().ok_or("Select an audio clip first")?;
                if !draft.dirty() {
                    return Err("The clip has no unsaved change".into());
                }
                if draft.base_revision != doc.revision() {
                    return Err(
                        "The saved clip revision changed; inspect its latest values before saving"
                            .into(),
                    );
                }
                if source_identity(doc, &draft.selection).as_ref() != Some(&draft.source) {
                    return Err("The clip source changed; select it afresh before editing".into());
                }
                (
                    draft.selection.clone(),
                    EditOperation::UpdateClip {
                        patch: draft.patch()?,
                        timeline: draft.timeline(),
                    },
                )
            }
            ClipAction::Restore(selection, timeline) => (
                selection.clone(),
                EditOperation::RestoreClip {
                    timeline: *timeline,
                },
            ),
            _ => return Err("This clip choice does not submit an edit".into()),
        };
        editable(doc, &selection)?;
        if matches!(edit, EditOperation::RestoreClip { .. })
            && clip(doc, &selection).is_none_or(|clip| clip.get("editSource").is_none())
        {
            return Err("This clip has no original edit snapshot".into());
        }
        let intent = ProjectEditIntent {
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            project_id: ProjectId(doc.id().into()),
            expected_revision: doc.revision(),
            scope: EditScope::DocumentClip {
                address: selection.address,
            },
            edit,
            extensions: Default::default(),
        };
        intent.validate().map_err(|error| error.to_string())?;
        Ok(intent)
    }
    pub fn failed(&mut self, intent: ProjectEditIntent, message: String, known_rejected: bool) {
        self.failed_with_draft(intent, message, known_rejected, self.draft.clone());
    }
    pub fn failed_with_draft(
        &mut self,
        intent: ProjectEditIntent,
        message: String,
        known_rejected: bool,
        draft: Option<ClipDraft>,
    ) {
        if let Some(failure) = &mut self.failure {
            if failure.intent == intent {
                failure.message = message;
                failure.known_rejected = known_rejected;
                return;
            }
            self.notice=Some("A different clip result arrived while an earlier intent is retained. The earlier evidence remains available; inspect saved storage before continuing.".into());
            return;
        }
        self.failure = Some(ClipFailure {
            intent,
            draft,
            message,
            known_rejected,
            suspended: false,
            reviewed_revision: None,
        });
        self.changed();
    }
    pub fn not_admitted(&mut self, intent: ProjectEditIntent, message: String) {
        if let Some(failure) = &mut self.failure
            && failure.intent == intent
        {
            failure.message = format!(
                "This retry was not admitted: {message}. The earlier outcome classification is unchanged."
            );
        } else {
            self.failed(
                intent,
                format!("Not admitted to the session worker: {message}"),
                true,
            );
        }
    }
    pub fn suspend(&mut self) {
        if let Some(failure) = &mut self.failure {
            failure.suspended = true;
            self.draft = None;
            self.changed();
        }
    }
    pub fn new_choice(&mut self, doc: &ProjectDocument) -> Result<(), String> {
        let old = self.reviewed(doc)?.clone();
        let replacement = if let Some(attempted) = &old.draft {
            editable(doc, &attempted.selection)?;
            if source_identity(doc, &attempted.selection).as_ref() != Some(&attempted.source) {
                return Err("The saved original source changed. Retained clip values were kept; release this rejected choice and select the current source afresh.".into());
            }
            let mut fresh = ClipDraft::from_document(doc, attempted.selection.clone())?;
            fresh.reapply(attempted)?;
            Some(fresh)
        } else {
            None
        };
        self.failure = None;
        self.released = Some(old);
        if let Some(draft) = &replacement {
            self.selection = Some(draft.selection.clone());
        }
        self.draft = replacement;
        self.notice=Some("Rejected clip edit released. Review these local values; only a new explicit Save submits a new operation.".into());
        self.changed();
        Ok(())
    }
    fn reviewed(&self, doc: &ProjectDocument) -> Result<&ClipFailure, String> {
        let failure = self.failure.as_ref().ok_or("No retained clip edit")?;
        if !failure.known_rejected
            || !failure.suspended
            || failure.intent.project_id.0 != doc.id()
            || failure.reviewed_revision != Some(doc.revision())
        {
            return Err("Review the latest saved target session before releasing or replacing this rejected clip choice".into());
        }
        Ok(failure)
    }
    pub fn release_rejected(&mut self, doc: &ProjectDocument) -> Result<(), String> {
        self.reviewed(doc)?;
        self.released = self.failure.take();
        self.draft = None;
        self.changed();
        self.notice=Some("Rejected clip choice released; its attempted values remain below. Select a target and make a new explicit edit when ready.".into());
        Ok(())
    }
    pub fn succeeded(&mut self) {
        self.failure = None;
        self.notice = None;
        self.changed();
    }

    pub fn failure_ui(
        &self,
        ui: &mut egui::Ui,
        available: bool,
        current: Option<&ProjectDocument>,
    ) -> Option<ClipAction> {
        let mut action = None;
        if let Some(failure) = &self.failure {
            ui.group(|ui|{
                ui.heading(if failure.known_rejected {"Clip edit was not published"} else {"Clip edit outcome is uncertain"});
                ui.label(&failure.message);
                ui.small(format!("Retained clip operation {} · target {} · expected revision {}",display_key(&failure.intent.operation_id.0),display_key(&failure.intent.project_id.0),failure.intent.expected_revision));
                if let Some(draft)=&failure.draft {draft_summary(ui,draft);}
                ui.small("This card is window-local evidence, not a durable draft inbox. Closing does not cancel a possibly completed edit. Reconnect never retries it automatically.");
                let target_open=current.is_some_and(|doc|doc.id()==failure.intent.project_id.0);
                let reviewed=current.is_some_and(|doc|target_open&&failure.reviewed_revision==Some(doc.revision()));
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled(available&&target_open,egui::Button::new("Retry exact clip edit")).clicked() {action=Some(ClipAction::Retry);}
                    if ui.add_enabled(available&&!failure.suspended,egui::Button::new("Keep clip draft locally and inspect saved session")).clicked() {action=Some(ClipAction::Suspend);}
                    if failure.known_rejected {
                        if ui.add_enabled(available,egui::Button::new("Review latest saved clip session")).clicked() {action=Some(ClipAction::ReviewLatest);}
                        if ui.add_enabled(available&&reviewed,egui::Button::new("Make a new clip choice")).clicked() {action=Some(ClipAction::NewChoice);}
                        if ui.add_enabled(available&&reviewed,egui::Button::new("Release rejected clip choice")).clicked() {action=Some(ClipAction::ReleaseRejected);}
                    }
                });
                if failure.suspended {ui.small("Retained evidence is suspended for inspection/navigation. No fresh musical mutation is allowed until this intent is resolved.");}
            });
        }
        if let Some(released) = &self.released {
            ui.small(format!(
                "Earlier rejected clip choice retained: {} · no automatic replay",
                display_key(&released.intent.operation_id.0)
            ));
            if let Some(draft) = &released.draft {
                draft_summary(ui, draft);
            }
        }
        if let Some(notice) = &self.notice {
            ui.label(notice);
        }
        action
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        doc: &ProjectDocument,
        track_key: &str,
        available: bool,
        other_draft_dirty: bool,
        other_unresolved: bool,
    ) -> Option<ClipAction> {
        let mut action = None;
        let selected_track = track(doc, track_key)?;
        if selected_track["kind"] != "audio"
            && !(doc.schema_version() == 2 && selected_track["kind"] == "recording")
        {
            return None;
        }
        ui.heading("Audio clip editor");
        ui.small("Non-destructive saved-source views · source waveform below · no stretch or draft audition");
        if selected_track["kind"] == "audio" {
            if selected_track.get("audio").is_some_and(Value::is_object) {
                let selection = ClipSelection {
                    project_id: doc.id().into(),
                    address: DocumentClipAddress::LegacyAudio {
                        track_id: track_key.into(),
                    },
                };
                ui.scope_builder(
                    egui::UiBuilder::new().scope_id(egui::Id::unique((
                        "legacy-clip-selector",
                        doc.id(),
                        track_key,
                    ))),
                    |ui| {
                        if ui
                            .add_enabled(
                                available,
                                egui::Button::new("Select legacy audio clip")
                                    .selected(self.selection.as_ref() == Some(&selection)),
                            )
                            .clicked()
                        {
                            action = Some(ClipAction::Select(selection));
                        }
                    },
                );
            } else {
                ui.label("This audio track has no declared clip settings.");
            }
        } else if let Some(clips) = selected_track["recording"]["clips"].as_array() {
            ui.label(format!(
                "{} declared clip(s) · select by exact identity",
                clips.len()
            ));
            egui::ScrollArea::vertical().id_salt(("clip-picker",doc.id(),track_key)).max_height(160.0)
                .show_rows(ui,24.0,clips.len(),|ui,range|{
                    for index in range {
                        let raw=&clips[index];let Some(id)=raw["id"].as_str() else {continue;};
                        ui.scope_builder(egui::UiBuilder::new().scope_id(egui::Id::unique(("native-clip-selector",doc.id(),track_key,id))),|ui|{
                            if raw["range"]["unit"]=="audio_frames" {
                                let selection=ClipSelection {project_id:doc.id().into(),address:DocumentClipAddress::NativeAudio {track_id:track_key.into(),clip_id:id.into()}};
                                if ui.add_enabled(available,egui::Button::new(format!("Select audio clip {}",display_key(id))).selected(self.selection.as_ref()==Some(&selection))).clicked() {action=Some(ClipAction::Select(selection));}
                            } else {ui.label(format!("{} · raw events, read-only; ordinals are not audio duration",display_key(id)));}
                        });
                    }
                });
        }
        if other_unresolved {
            ui.label("Resolve the retained musical edit before changing clip settings.");
        }
        if other_draft_dirty {
            ui.label("Another draft family has unsaved edits. Save, discard or cancel that choice before editing this clip.");
            if ui
                .add_enabled(
                    available && !other_unresolved && !self.unresolved(),
                    egui::Button::new("Begin clip editing"),
                )
                .clicked()
            {
                action = Some(ClipAction::BeginClip);
            }
        }
        let unlocked = selected_track["locked"] == false;
        if !unlocked {
            ui.label("This track is locked; clip settings and originals are read-only.");
        }
        let matches = self
            .selection
            .as_ref()
            .is_some_and(|s| s.project_id == doc.id() && track_id(&s.address) == track_key);
        if !matches {
            ui.label("Select a declared audio clip to inspect its saved settings.");
            return action;
        }
        let selection = self.selection.as_ref().expect("matched selection").clone();
        ui.small(format!("Track key: {}", display_key(track_key)));
        if let DocumentClipAddress::NativeAudio { clip_id, .. } = &selection.address {
            ui.small(format!("Clip key: {}", display_key(clip_id)));
        }
        let Some(saved) = clip(doc, &selection) else {
            ui.label("The selected clip is absent from this saved revision. Undo may restore its exact identity; no neighbouring clip was selected.");
            return action;
        };
        source_details(ui, doc, &selection, saved);
        let editable =
            available && unlocked && !other_draft_dirty && !other_unresolved && !self.unresolved();
        let own_unresolved = self.unresolved();
        let Some(draft) = &mut self.draft else {
            return action;
        };
        let before = draft.clone();
        let native_id = match &selection.address {
            DocumentClipAddress::NativeAudio { clip_id, .. } => clip_id.as_str(),
            _ => "",
        };
        ui.scope_builder(egui::UiBuilder::new().scope_id(egui::Id::unique(("clip-settings",doc.id(),track_key,native_id))),|ui|{
            ui.small(format!("Draft based on saved revision {}",draft.base_revision));
            ui.add_enabled_ui(editable,|ui|{
                match &mut draft.fields {
                    ClipDraftFields::LegacyAudio(fields)=>{
                        for (label,key,field) in [("Clip start (ticks)","startTick",&mut fields.start_tick),("Source in (seconds)","trimStartSeconds",&mut fields.trim_start_seconds),("Source out (seconds)","trimEndSeconds",&mut fields.trim_end_seconds),("Fade in (seconds)","fadeInSeconds",&mut fields.fade_in_seconds),("Fade out (seconds)","fadeOutSeconds",&mut fields.fade_out_seconds)] {
                            optional_field_ui(ui,label,field);
                            if saved.get(key).and_then(Value::as_number).is_some_and(|n|n.as_str().len()>LEGACY_TEXT) {ui.small("Saved number exceeds the editable text limit. Keep preserves it exactly; Set requires a new explicit value and starts blank.");}
                        }
                    }
                    ClipDraftFields::NativeAudio(fields)=>{
                        value_mode_ui(ui,"Placement",&mut fields.position_mode);
                        ui.add_enabled_ui(fields.position_mode==ValueMode::Set,|ui|{text_ui(ui,"Placement (frames)",&mut fields.position_frame,NATIVE_TEXT);});
                        value_mode_ui(ui,"Source window",&mut fields.window_mode);
                        ui.add_enabled_ui(fields.window_mode==ValueMode::Set,|ui|{
                            text_ui(ui,"Source start (frames)",&mut fields.source_start_frame,NATIVE_TEXT);
                            text_ui(ui,"Source length (frames)",&mut fields.frame_count,NATIVE_TEXT);
                        });
                        field_mode_ui(ui,"Fade settings",&mut fields.fades_mode);
                        ui.add_enabled_ui(fields.fades_mode==FieldMode::Set,|ui|{
                            text_ui(ui,"Fade in (frames)",&mut fields.fade_in_frames,NATIVE_TEXT);
                            text_ui(ui,"Fade out (frames)",&mut fields.fade_out_frames,NATIVE_TEXT);
                            egui::ComboBox::new("clip-fade-curve", "Fade curve").selected_text(match fields.curve {FadeCurve::Linear=>"Linear",FadeCurve::EqualPower=>"Equal power"})
                                .show_ui(ui,|ui|{ui.selectable_value(&mut fields.curve,FadeCurve::Linear,"Linear");ui.selectable_value(&mut fields.curve,FadeCurve::EqualPower,"Equal power");});
                        });
                    }
                }
                ui.radio_value(&mut draft.extend_loop,false,"Keep current song extent");
                ui.radio_value(&mut draft.extend_loop,true,"Extend song to include clip");
                if draft.extend_loop {ui.checkbox(&mut draft.allow_upgrade,"Allow native format upgrade beyond 128 bars");}
            });
            let valid=draft.patch();
            if let Err(error)=&valid {ui.colored_label(ui.visuals().error_fg_color,error);}
            ui.small("Keep preserves saved presence and number spelling. Use default removes the field; Set zero writes an explicit value.");
            ui.small("Combined source-window, fade, loop and no-op checks run on Save. No source is clamped or shortened. The saved arrangement stays visible until acknowledgement.");
            ui.horizontal_wrapped(|ui|{
                if ui.add_enabled(editable&&draft.dirty()&&valid.is_ok(),egui::Button::new("Save clip")).clicked() {action=Some(ClipAction::Save);}
                if ui.add_enabled(available&&draft.dirty()&&!own_unresolved,egui::Button::new("Discard clip")).clicked() {action=Some(ClipAction::Discard);}
                if ui.add_enabled(editable&&saved.get("editSource").is_some(),egui::Button::new("Restore original clip")).clicked() {action=Some(ClipAction::Restore(selection.clone(),draft.timeline()));}
            });
        });
        if *draft != before {
            self.changed();
        }
        action
    }
}

fn field_label(mode: FieldMode) -> &'static str {
    match mode {
        FieldMode::Keep => "Keep saved",
        FieldMode::Set => "Set value",
        FieldMode::Remove => "Use default",
    }
}
fn field_mode_ui(ui: &mut egui::Ui, label: &str, mode: &mut FieldMode) {
    egui::ComboBox::new(("optional-clip-mode", label), format!("{label} mode"))
        .selected_text(field_label(*mode))
        .show_ui(ui, |ui| {
            ui.selectable_value(mode, FieldMode::Keep, "Keep saved");
            ui.selectable_value(mode, FieldMode::Set, "Set value");
            ui.selectable_value(mode, FieldMode::Remove, "Use default");
        });
}
fn value_mode_ui(ui: &mut egui::Ui, label: &str, mode: &mut ValueMode) {
    egui::ComboBox::new(("required-clip-mode", label), format!("{label} mode"))
        .selected_text(match mode {
            ValueMode::Keep => "Keep saved",
            ValueMode::Set => "Set value",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(mode, ValueMode::Keep, "Keep saved");
            ui.selectable_value(mode, ValueMode::Set, "Set value");
        });
}
fn text_ui(ui: &mut egui::Ui, label: &str, text: &mut String, limit: usize) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::TextEdit::singleline(text)
                .char_limit(limit)
                .desired_width(220.0),
        )
        .accessible_name(label);
    });
}
fn optional_field_ui(ui: &mut egui::Ui, label: &str, field: &mut ClipField) {
    field_mode_ui(ui, label, &mut field.mode);
    ui.add_enabled_ui(field.mode == FieldMode::Set, |ui| {
        text_ui(ui, label, &mut field.text, LEGACY_TEXT);
    });
}
fn display_key(raw: &str) -> String {
    let mut result = String::new();
    for ch in raw.chars().take(72) {
        result.extend(ch.escape_debug());
    }
    if raw.chars().nth(72).is_some() {
        result.push('…');
    }
    if result.is_empty() {
        "\"\"".into()
    } else {
        result
    }
}
fn scalar(value: Option<&Value>) -> String {
    match value {
        None => "absent (saved default)".into(),
        Some(Value::Null) => "explicit null".into(),
        Some(Value::Number(number)) => {
            let raw = number.as_str();
            if raw.len() > 80 {
                format!("{}… ({} characters retained)", &raw[..80], raw.len())
            } else {
                raw.into()
            }
        }
        Some(Value::String(value)) => display_key(value),
        Some(Value::Bool(value)) => value.to_string(),
        _ => "structured metadata retained".into(),
    }
}
fn native_settings(ui: &mut egui::Ui, raw: &Value, prefix: &str) {
    for (label, value) in [
        (
            "source start",
            raw.get("range").and_then(|v| v.get("start")),
        ),
        ("frame count", raw.get("range").and_then(|v| v.get("count"))),
        (
            "placement frame",
            raw.get("position").and_then(|v| v.get("frame")),
        ),
        (
            "sample rate",
            raw.get("position").and_then(|v| v.get("sampleRate")),
        ),
    ] {
        ui.small(format!("{prefix} {label}: {}", scalar(value)));
    }
    match raw.get("fades") {
        None => {
            ui.small(format!("{prefix} fades: absent (no fade)"));
        }
        Some(Value::Null) => {
            ui.small(format!("{prefix} fades: explicit null (no fade)"));
        }
        Some(fades) => {
            for field in ["inFrames", "outFrames", "curve"] {
                ui.small(format!(
                    "{prefix} fade {field}: {}",
                    scalar(fades.get(field))
                ));
            }
        }
    }
}
pub(super) fn source_details(
    ui: &mut egui::Ui,
    doc: &ProjectDocument,
    selection: &ClipSelection,
    raw: &Value,
) {
    if raw.get("editSource").is_some() {
        ui.label("Original clip settings retained");
    }
    ui.collapsing("Saved clip source and original settings",|ui|{
        match &selection.address {
            DocumentClipAddress::LegacyAudio {track_id}=>{
                if let Some(track)=track(doc,track_id) {for field in ["assetId","sourceAssetId"] {ui.small(format!("Saved {field}: {}",scalar(track.get(field))));}}
                for field in ["origin","mimeType","durationSeconds","byteLength","startTick","trimStartSeconds","trimEndSeconds","fadeInSeconds","fadeOutSeconds"] {ui.small(format!("Saved {field}: {}",scalar(raw.get(field))));}
                if let Some(timing)=raw.get("recordingTiming") {for field in ["method","status","calibration"] {ui.small(format!("Retained recording {field}: {}",scalar(timing.get(field))));}}
            }
            DocumentClipAddress::NativeAudio {..}=>{
                for field in ["takeId","manifestSha256","streamId"] {ui.small(format!("Saved {field}: {}",scalar(raw.get("source").and_then(|v|v.get(field)))));}
                native_settings(ui,raw,"Saved");
                ui.small("This is the saved clip window, not authenticated full descriptor/prefix bounds. Original clocks, gaps and packets stay in the sealed source.");
            }
        }
        if let Some(source)=raw.get("editSource") {
            ui.label("Permanent original settings");
            if let Some(original)=source.get("original") {match selection.address {
                DocumentClipAddress::LegacyAudio {..}=>for field in ["startTick","trimStartSeconds","trimEndSeconds","fadeInSeconds","fadeOutSeconds"] {ui.small(format!("Original {field}: {}",scalar(original.get(field))));},
                DocumentClipAddress::NativeAudio {..}=>native_settings(ui,original,"Original"),
            }}
            ui.small("Snapshot presence does not mean the current settings differ. Opaque snapshot extensions remain retained without being expanded or rewritten.");
        }
        ui.small("Original bytes, timing observations and unfamiliar metadata are retained. Clip settings do not establish source calibration, decoding support or physical recording accuracy.");
    });
}
fn draft_summary(ui: &mut egui::Ui, draft: &ClipDraft) {
    ui.small(format!(
        "Retained clip draft at revision {} · source {}",
        draft.base_revision,
        match draft.source {
            ClipSourceIdentity::LegacyAudio { .. } => "legacy asset",
            ClipSourceIdentity::NativeAudio { .. } => "sealed native take",
        }
    ));
    match &draft.fields {
        ClipDraftFields::LegacyAudio(d) => {
            for (label, field) in [
                ("start", &d.start_tick),
                ("source in", &d.trim_start_seconds),
                ("source out", &d.trim_end_seconds),
                ("fade in", &d.fade_in_seconds),
                ("fade out", &d.fade_out_seconds),
            ] {
                ui.small(format!(
                    "Retained {label}: {} · {}",
                    field_label(field.mode),
                    display_key(&field.text)
                ));
            }
        }
        ClipDraftFields::NativeAudio(d) => {
            ui.small(format!(
                "Retained placement {:?}: {} · source window {:?}: {} + {} frames",
                d.position_mode,
                display_key(&d.position_frame),
                d.window_mode,
                display_key(&d.source_start_frame),
                display_key(&d.frame_count)
            ));
            ui.small(format!(
                "Retained fades {}: {} / {} frames · {:?}",
                field_label(d.fades_mode),
                display_key(&d.fade_in_frames),
                display_key(&d.fade_out_frames),
                d.curve
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::contracts::TakeId;
    use omniusong_core::native::{TakeClip, TakeRange, TakeReference, TimelinePosition};
    use serde_json::json;

    fn legacy() -> (ProjectDocument, ClipSelection) {
        let mut value = ProjectDocument::new("Synthetic clip editor")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["id"] = "音源\0\n".into();
        value["tracks"][0]["kind"] = "audio".into();
        value["tracks"][0]["assetId"] = format!("{}.wav", "a".repeat(64)).into();
        value["tracks"][0]["sourceAssetId"] = format!("{}.wav", "b".repeat(64)).into();
        value["tracks"][0]["audio"] = json!({"origin":"import","mimeType":"audio/wav","durationSeconds":10,"byteLength":100,"trimStartSeconds":serde_json::from_str::<Number>("0.12345678901234567890123456789").unwrap()});
        let doc = ProjectDocument::from_value(value).unwrap();
        let selection = ClipSelection {
            project_id: doc.id().into(),
            address: DocumentClipAddress::LegacyAudio {
                track_id: "音源\0\n".into(),
            },
        };
        (doc, selection)
    }
    fn native(fades: Option<Value>) -> (ProjectDocument, ClipSelection) {
        let mut doc = ProjectDocument::new("Synthetic native clip editor")
            .unwrap()
            .upgrade_native()
            .unwrap();
        doc.append_take_clip(
            "recording",
            "Synthetic original",
            TakeClip {
                id: "audio-one".into(),
                source: TakeReference {
                    take_id: TakeId("synthetic-take".into()),
                    manifest_sha256: "a".repeat(64),
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 100,
                    count: 480,
                },
                position: TimelinePosition {
                    frame: -123,
                    sample_rate: 48_000,
                },
                fades: None,
                edit_source: None,
            },
        )
        .unwrap();
        let mut value = doc.value().clone();
        if let Some(fades) = fades {
            value["tracks"][2]["recording"]["clips"][0]["fades"] = fades;
        }
        let selection = ClipSelection {
            project_id: doc.id().into(),
            address: DocumentClipAddress::NativeAudio {
                track_id: "recording".into(),
                clip_id: "audio-one".into(),
            },
        };
        (ProjectDocument::from_value(value).unwrap(), selection)
    }
    fn legacy_fields(editor: &mut ClipEditor) -> &mut LegacyClipDraft {
        let ClipDraftFields::LegacyAudio(fields) = &mut editor.draft.as_mut().unwrap().fields
        else {
            panic!("legacy")
        };
        fields
    }
    fn change_start(editor: &mut ClipEditor, text: &str) {
        let field = &mut legacy_fields(editor).start_tick;
        field.mode = FieldMode::Set;
        field.text = text.into();
    }
    fn reviewed_failure(
        editor: &mut ClipEditor,
        doc: &ProjectDocument,
        known: bool,
    ) -> ProjectEditIntent {
        let intent = editor.intent(doc, &ClipAction::Save).unwrap();
        editor.failed(intent.clone(), "Synthetic failure".into(), known);
        editor.suspend();
        editor.document_installed(doc);
        intent
    }

    #[test]
    fn legacy_patch_keeps_exact_number_spelling_and_explicit_presence_modes() {
        let (doc, selection) = legacy();
        let mut editor = ClipEditor::default();
        editor.select(&doc, selection).unwrap();
        assert!(!editor.dirty());
        assert!(editor.intent(&doc, &ClipAction::Save).is_err());
        let fields = legacy_fields(&mut editor);
        assert_eq!(
            fields.trim_start_seconds.text,
            "0.12345678901234567890123456789"
        );
        fields.trim_start_seconds.mode = FieldMode::Set;
        fields.fade_in_seconds.mode = FieldMode::Set;
        fields.fade_in_seconds.text = "0.0000".into();
        fields.fade_out_seconds.mode = FieldMode::Remove;
        let patch = editor.draft.as_ref().unwrap().patch().unwrap();
        let ClipPatch::LegacyAudio(fields) = patch else {
            panic!("legacy patch")
        };
        assert_eq!(fields.start_tick, FieldChange::Keep);
        assert_eq!(fields.fade_out_seconds, FieldChange::Remove);
        let FieldChange::Set { value } = fields.trim_start_seconds else {
            panic!("number")
        };
        assert_eq!(value.as_str(), "0.12345678901234567890123456789");
        let FieldChange::Set { value } = fields.fade_in_seconds else {
            panic!("number")
        };
        assert_eq!(value.as_str(), "0.0000");
        let intent = editor.intent(&doc, &ClipAction::Save).unwrap();
        assert_eq!(intent.expected_revision, doc.revision());
        assert_eq!(
            intent.scope,
            EditScope::DocumentClip {
                address: editor.selection.clone().unwrap().address
            }
        );
    }

    #[test]
    fn invalid_intermediates_remain_dirty_and_text_limits_never_truncate_sources() {
        let (doc, selection) = legacy();
        let mut editor = ClipEditor::default();
        editor.select(&doc, selection.clone()).unwrap();
        for invalid in ["", "-", "0.", "1e", " NaN", " 0.5", "1e10000"] {
            let field = &mut legacy_fields(&mut editor).trim_end_seconds;
            field.mode = FieldMode::Set;
            field.text = invalid.into();
            assert!(editor.dirty());
            assert!(editor.draft.as_ref().unwrap().patch().is_err(), "{invalid}");
            assert!(editor.intent(&doc, &ClipAction::Save).is_err());
            assert_eq!(legacy_fields(&mut editor).trim_end_seconds.text, invalid);
        }
        let mut value = doc.value().clone();
        let lexeme = format!("0.{}1", "0".repeat(600));
        value["tracks"][0]["audio"]["trimStartSeconds"] = serde_json::from_str(&lexeme).unwrap();
        let large = ProjectDocument::from_value(value).unwrap();
        editor.select(&large, selection).unwrap();
        let field = &mut legacy_fields(&mut editor).trim_start_seconds;
        assert_eq!(field.mode, FieldMode::Keep);
        assert!(field.text.is_empty());
        field.mode = FieldMode::Set;
        assert!(editor.draft.as_ref().unwrap().patch().is_err());
        let field = &mut legacy_fields(&mut editor).trim_start_seconds;
        field.text = "0".repeat(257);
        assert!(editor.draft.as_ref().unwrap().patch().is_err());
        assert_eq!(
            clip(&large, editor.selection.as_ref().unwrap()).unwrap()["trimStartSeconds"]
                .as_number()
                .unwrap()
                .as_str(),
            lexeme
        );
    }

    #[test]
    fn native_frames_stay_integer_and_fade_keep_preserves_absent_null_and_zero() {
        for fades in [
            None,
            Some(Value::Null),
            Some(json!({"inFrames":0,"outFrames":0,"curve":"equal_power"})),
        ] {
            let (doc, selection) = native(fades);
            let mut editor = ClipEditor::default();
            editor.select(&doc, selection).unwrap();
            let ClipDraftFields::NativeAudio(fields) = &mut editor.draft.as_mut().unwrap().fields
            else {
                panic!("native")
            };
            assert_eq!(fields.position_frame, "-123");
            assert_eq!(fields.source_start_frame, "100");
            assert_eq!(fields.frame_count, "480");
            fields.position_mode = ValueMode::Set;
            fields.position_frame = i64::MIN.to_string();
            fields.window_mode = ValueMode::Set;
            fields.source_start_frame = "101".into();
            fields.frame_count = "479".into();
            let ClipPatch::NativeAudio(patch) = editor.draft.as_ref().unwrap().patch().unwrap()
            else {
                panic!("native")
            };
            assert_eq!(patch.position_frame, ValueChange::Set { value: i64::MIN });
            assert_eq!(patch.fades, FieldChange::Keep);
            assert_eq!(
                patch.source_window,
                ValueChange::Set {
                    value: AudioSourceWindow {
                        start_frame: 101,
                        frame_count: 479
                    }
                }
            );
            let ClipDraftFields::NativeAudio(fields) = &mut editor.draft.as_mut().unwrap().fields
            else {
                panic!("native")
            };
            fields.fades_mode = FieldMode::Remove;
            let ClipPatch::NativeAudio(patch) = editor.draft.as_ref().unwrap().patch().unwrap()
            else {
                panic!("native")
            };
            assert_eq!(patch.fades, FieldChange::Remove);
            let ClipDraftFields::NativeAudio(fields) = &mut editor.draft.as_mut().unwrap().fields
            else {
                panic!("native")
            };
            fields.fades_mode = FieldMode::Set;
            fields.fade_in_frames = "0".into();
            fields.fade_out_frames = "0".into();
            let ClipPatch::NativeAudio(patch) = editor.draft.as_ref().unwrap().patch().unwrap()
            else {
                panic!("native")
            };
            assert!(matches!(
                patch.fades,
                FieldChange::Set {
                    value: AudioFades {
                        in_frames: 0,
                        out_frames: 0,
                        ..
                    }
                }
            ));
            let ClipDraftFields::NativeAudio(fields) = &mut editor.draft.as_mut().unwrap().fields
            else {
                panic!("native")
            };
            fields.position_frame = "-".into();
            assert!(editor.draft.as_ref().unwrap().patch().is_err());
            let ClipDraftFields::NativeAudio(fields) = &mut editor.draft.as_mut().unwrap().fields
            else {
                panic!("native")
            };
            fields.position_frame = "0".repeat(33);
            assert!(editor.draft.as_ref().unwrap().patch().is_err());
        }
    }

    #[test]
    fn rejected_new_choice_checks_both_legacy_source_keys_before_reapplying() {
        for field in ["assetId", "sourceAssetId"] {
            let (doc, selection) = legacy();
            let mut editor = ClipEditor::default();
            editor.select(&doc, selection).unwrap();
            change_start(&mut editor, "123");
            let intent = reviewed_failure(&mut editor, &doc, true);
            let mut value = doc.value().clone();
            value["tracks"][0][field] = format!("{}.wav", "c".repeat(64)).into();
            value["revision"] = 1.into();
            let changed = ProjectDocument::from_value(value).unwrap();
            editor.document_installed(&changed);
            assert!(editor.new_choice(&changed).is_err());
            assert_eq!(editor.failure.as_ref().unwrap().intent, intent);
            editor.release_rejected(&changed).unwrap();
            assert!(!editor.unresolved());
            assert!(editor.released.is_some());
        }
    }

    #[test]
    fn rejected_native_choice_checks_take_manifest_stream_and_rate() {
        for (field, replacement) in [
            ("takeId", json!("other-take")),
            ("manifestSha256", json!("b".repeat(64))),
            ("streamId", json!("other-stream")),
            ("sampleRate", json!(96_000)),
        ] {
            let (doc, selection) = native(None);
            let mut editor = ClipEditor::default();
            editor.select(&doc, selection).unwrap();
            let ClipDraftFields::NativeAudio(fields) = &mut editor.draft.as_mut().unwrap().fields
            else {
                panic!("native")
            };
            fields.position_mode = ValueMode::Set;
            fields.position_frame = "17".into();
            let intent = reviewed_failure(&mut editor, &doc, true);
            let mut value = doc.value().clone();
            let raw = &mut value["tracks"][2]["recording"]["clips"][0];
            if field == "sampleRate" {
                raw["position"][field] = replacement;
            } else {
                raw["source"][field] = replacement;
            }
            value["revision"] = 1.into();
            let changed = ProjectDocument::from_value(value).unwrap();
            editor.document_installed(&changed);
            assert!(editor.new_choice(&changed).is_err());
            assert_eq!(editor.failure.as_ref().unwrap().intent, intent);
        }
    }

    #[test]
    fn new_choice_rebases_only_non_keep_fields_and_requires_an_explicit_new_save() {
        let (doc, selection) = legacy();
        let mut editor = ClipEditor::default();
        editor.select(&doc, selection).unwrap();
        change_start(&mut editor, "123");
        let old = reviewed_failure(&mut editor, &doc, true);
        let mut value = doc.value().clone();
        value["revision"] = 4.into();
        value["tracks"][0]["audio"]["trimStartSeconds"] = json!(0.5);
        let latest = ProjectDocument::from_value(value).unwrap();
        editor.document_installed(&latest);
        editor.new_choice(&latest).unwrap();
        assert!(!editor.unresolved());
        let fields = legacy_fields(&mut editor);
        assert_eq!(fields.start_tick.text, "123");
        assert_eq!(fields.start_tick.mode, FieldMode::Set);
        assert_eq!(fields.trim_start_seconds.mode, FieldMode::Keep);
        assert_eq!(fields.trim_start_seconds.text, "0.5");
        let new = editor.intent(&latest, &ClipAction::Save).unwrap();
        assert_ne!(new.operation_id, old.operation_id);
        assert_eq!(new.expected_revision, 4);
        assert_eq!(editor.released.as_ref().unwrap().intent, old);
    }

    #[test]
    fn unknown_outcome_keeps_exact_intent_and_clear_selection_keeps_the_card() {
        let (doc, selection) = legacy();
        let mut editor = ClipEditor::default();
        editor.select(&doc, selection).unwrap();
        change_start(&mut editor, "17");
        let old = reviewed_failure(&mut editor, &doc, false);
        let old_draft = editor.failure.as_ref().unwrap().draft.clone();
        editor.not_admitted(old.clone(), "Busy".into());
        assert!(!editor.failure.as_ref().unwrap().known_rejected);
        assert!(editor.new_choice(&doc).is_err());
        assert!(editor.release_rejected(&doc).is_err());
        let context = editor.context();
        editor.clear_selection();
        assert_ne!(editor.context(), context);
        assert!(editor.selection.is_none());
        assert!(editor.unresolved());
        assert_eq!(editor.failure.as_ref().unwrap().intent, old);
        assert_eq!(editor.failure.as_ref().unwrap().draft, old_draft);
        assert!(editor.intent(&doc, &ClipAction::Save).is_err());
    }

    #[test]
    fn source_reordering_missing_targets_locked_and_events_never_retarget() {
        let (doc, selection) = native(None);
        let mut editor = ClipEditor::default();
        editor.select(&doc, selection.clone()).unwrap();
        let mut value = doc.value().clone();
        let mut sibling = value["tracks"][2]["recording"]["clips"][0].clone();
        sibling["id"] = "other".into();
        value["tracks"][2]["recording"]["clips"]
            .as_array_mut()
            .unwrap()
            .insert(0, sibling);
        let reordered = ProjectDocument::from_value(value.clone()).unwrap();
        editor.document_installed(&reordered);
        assert_eq!(editor.selection, Some(selection.clone()));
        value["tracks"][2]["recording"]["clips"]
            .as_array_mut()
            .unwrap()
            .remove(1);
        let missing = ProjectDocument::from_value(value).unwrap();
        editor.document_installed(&missing);
        assert_eq!(editor.selection, Some(selection.clone()));
        assert!(editor.draft.is_none());
        let mut value = doc.value().clone();
        value["tracks"][2]["locked"] = true.into();
        let locked = ProjectDocument::from_value(value).unwrap();
        editor.select(&locked, selection.clone()).unwrap();
        let ClipDraftFields::NativeAudio(fields) = &mut editor.draft.as_mut().unwrap().fields
        else {
            panic!("native")
        };
        fields.position_mode = ValueMode::Set;
        fields.position_frame = "17".into();
        assert!(editor.intent(&locked, &ClipAction::Save).is_err());
        let mut value = doc.value().clone();
        value["tracks"][2]["recording"]["clips"][0]["range"] =
            json!({"unit":"events","start":100,"count":480});
        let events = ProjectDocument::from_value(value).unwrap();
        assert!(editor.select(&events, selection).is_err());
    }

    #[test]
    fn selection_context_tracks_raw_drafts_and_discard_is_local() {
        let (doc, selection) = legacy();
        let original = doc.clone();
        let mut editor = ClipEditor::default();
        editor.select(&doc, selection).unwrap();
        let baseline = editor.context();
        change_start(&mut editor, "123");
        assert_ne!(editor.context(), baseline);
        let draft = editor.draft.clone();
        let intent = editor.intent(&doc, &ClipAction::Save).unwrap();
        editor.failed_with_draft(intent.clone(), "Rejected".into(), true, draft.clone());
        editor.discard(&doc);
        assert_eq!(editor.failure.as_ref().unwrap().draft, draft);
        assert_eq!(editor.failure.as_ref().unwrap().intent, intent);
        assert_eq!(doc, original);
        editor.succeeded();
        editor.discard(&doc);
        assert!(!editor.dirty());
        assert_eq!(doc, original);
    }

    #[test]
    fn real_egui_picker_is_bounded_and_events_stay_read_only() {
        let (doc, selection) = native(None);
        let mut value = doc.value().clone();
        let template = value["tracks"][2]["recording"]["clips"][0].clone();
        let clips = value["tracks"][2]["recording"]["clips"]
            .as_array_mut()
            .unwrap();
        for index in 1..16_384 {
            let mut raw = template.clone();
            raw["id"] = format!("clip-{index}").into();
            if index == 1 {
                raw["range"] = json!({"unit":"events","start":100,"count":480});
            }
            clips.push(raw);
        }
        let doc = ProjectDocument::from_value(value).unwrap();
        let mut editor = ClipEditor::default();
        editor.select(&doc, selection).unwrap();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 1000.0),
                )),
                ..Default::default()
            },
            |ui| {
                editor.ui(ui, &doc, "recording", true, false, true);
            },
        );
        output.textures_delta.clear();
        let tree = output.platform_output.accesskit_update.unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        assert!(text.iter().any(|s| s.contains("raw events, read-only")));
        assert!(
            text.iter()
                .any(|s| s.contains("Resolve the retained musical edit"))
        );
        assert!(
            text.iter()
                .filter(|s| s.starts_with("Select audio clip "))
                .count()
                < 100
        );
        assert!(tree.nodes.len() < 2000);
        assert!(!editor.dirty());
    }
}
