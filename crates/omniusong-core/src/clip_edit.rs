//! Pure non-destructive clip mutations. Source objects and clock observations
//! are immutable; the store owns revision/history and operation admission.
use crate::contracts::Validate;
use crate::native::{self, TakeClip, TakeRange};
use crate::{
    ClipPatch, ClipTimelinePolicy, DocumentClipAddress, DocumentError, FieldChange,
    ProjectDocument, Result, ValueChange, require,
};
use serde::Serialize;
use serde_json::{Map, Value, json};

const LEGACY_FIELDS: &[&str] = &[
    "startTick",
    "trimStartSeconds",
    "trimEndSeconds",
    "fadeInSeconds",
    "fadeOutSeconds",
];
const NATIVE_FIELDS: &[&str] = &["range", "position", "fades"];

#[derive(Clone, Copy)]
enum Target {
    Legacy(usize),
    Native(usize, usize),
}
impl Target {
    fn clip(self, value: &Value) -> &Value {
        match self {
            Self::Legacy(track) => &value["tracks"][track]["audio"],
            Self::Native(track, clip) => &value["tracks"][track]["recording"]["clips"][clip],
        }
    }
    fn clip_mut(self, value: &mut Value) -> &mut Value {
        match self {
            Self::Legacy(track) => &mut value["tracks"][track]["audio"],
            Self::Native(track, clip) => &mut value["tracks"][track]["recording"]["clips"][clip],
        }
    }
    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Legacy(_) => LEGACY_FIELDS,
            Self::Native(_, _) => NATIVE_FIELDS,
        }
    }
}

fn contract(error: crate::contracts::ContractError) -> DocumentError {
    DocumentError(error.to_string())
}
fn target(
    document: &ProjectDocument,
    address: &DocumentClipAddress,
    editing: bool,
) -> Result<Target> {
    address.validate().map_err(contract)?;
    let (track_id, native) = match address {
        DocumentClipAddress::LegacyAudio { track_id } => (track_id, false),
        DocumentClipAddress::NativeAudio { track_id, .. } => (track_id, true),
    };
    let index = document
        .tracks()
        .iter()
        .position(|track| track["id"].as_str() == Some(track_id.as_str()))
        .ok_or_else(|| DocumentError("Clip track was not found".into()))?;
    let track = &document.tracks()[index];
    if editing {
        require(
            track["locked"] == false,
            "Unlock this track before editing its clip",
        )?;
    }
    require(
        track["kind"] == if native { "recording" } else { "audio" },
        "Clip address does not select its declared track kind",
    )?;
    if let DocumentClipAddress::NativeAudio { clip_id, .. } = address {
        let clips = track["recording"]["clips"]
            .as_array()
            .expect("validated recording payload");
        let clip = clips
            .iter()
            .position(|clip| clip["id"].as_str() == Some(clip_id.as_str()))
            .ok_or_else(|| DocumentError("Native audio clip was not found".into()))?;
        require(
            clips[clip]["range"]["unit"] == "audio_frames",
            "Raw event clips cannot be edited as audio",
        )?;
        Ok(Target::Native(index, clip))
    } else {
        require(
            track["audio"].is_object(),
            "The audio track has no declared clip",
        )?;
        Ok(Target::Legacy(index))
    }
}
fn settings(clip: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|key| clip.get(*key).map(|value| ((*key).into(), value.clone())))
        .collect()
}
fn field<T: Serialize>(
    object: &mut Map<String, Value>,
    key: &str,
    change: &FieldChange<T>,
) -> Result<()> {
    match change {
        FieldChange::Keep => {}
        FieldChange::Remove => {
            object.remove(key);
        }
        FieldChange::Set { value } => {
            object.insert(
                key.into(),
                serde_json::to_value(value).map_err(|error| DocumentError(error.to_string()))?,
            );
        }
    }
    Ok(())
}
fn validate_target(value: &Value, selected: Target) -> Result<()> {
    match selected {
        Target::Legacy(_) => crate::validate_audio(selected.clip(value)),
        Target::Native(track, _) => native::recording_payload(&value["tracks"][track]).map(|_| ()),
    }
}
fn required_bars(value: &Value, selected: Target, tempo: f64) -> Result<u64> {
    let clip = selected.clip(value);
    match selected {
        Target::Legacy(_) => {
            // Evaluation order is a public compatibility rule. This estimate
            // never replaces the exact stored JSON numbers or source settings.
            let number =
                |key: &str, default: f64| clip.get(key).and_then(Value::as_f64).unwrap_or(default);
            let start = number("startTick", 0.0);
            let duration = number("trimEndSeconds", number("durationSeconds", 0.0))
                - number("trimStartSeconds", 0.0);
            let beats = duration * tempo;
            let ticks = beats * 960.0;
            let delta = ticks / 60.0;
            let end = start + delta;
            let bars = (end / 3840.0).ceil();
            require(
                [start, duration, beats, ticks, delta, end, bars]
                    .iter()
                    .all(|v| v.is_finite() && *v >= 0.0)
                    && bars <= native::MAX_NATIVE_BARS as f64,
                "Legacy clip extent exceeds the native timeline",
            )?;
            Ok(bars as u64)
        }
        Target::Native(_, _) => {
            let clip: TakeClip = serde_json::from_value(clip.clone())
                .map_err(|error| DocumentError(error.to_string()))?;
            clip.validate()?;
            let TakeRange::AudioFrames { count, .. } = clip.range else {
                return Err(DocumentError(
                    "Raw event clips have no audio-frame extent".into(),
                ));
            };
            let end = clip
                .position
                .frame
                .checked_add(count as i64)
                .ok_or_else(|| DocumentError("Native clip signed extent overflow".into()))?
                .max(0) as u128;
            // The validated tempo is 30..300, hence binary exponent 4..8.
            let bits = tempo.to_bits();
            let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023;
            require((4..=8).contains(&exponent), "Invalid tempo binary exponent")?;
            let mantissa = ((1u64 << 52) | (bits & ((1u64 << 52) - 1))) as u128;
            let denominator = 1u128
                .checked_shl((52 - exponent) as u32)
                .and_then(|d| d.checked_mul(u128::from(clip.position.sample_rate)))
                .and_then(|d| d.checked_mul(240))
                .ok_or_else(|| DocumentError("Native clip extent denominator overflow".into()))?;
            let numerator = end
                .checked_mul(mantissa)
                .ok_or_else(|| DocumentError("Native clip extent numerator overflow".into()))?;
            let bars = (numerator / denominator)
                .checked_add(u128::from(numerator % denominator != 0))
                .ok_or_else(|| DocumentError("Native clip extent ceiling overflow".into()))?;
            require(
                bars <= u128::from(native::MAX_NATIVE_BARS),
                "Native clip extent exceeds the native timeline",
            )?;
            Ok(bars as u64)
        }
    }
}
fn finish(
    original: &ProjectDocument,
    mut value: Value,
    selected: Target,
    policy: ClipTimelinePolicy,
) -> Result<ProjectDocument> {
    validate_target(&value, selected)?;
    if let ClipTimelinePolicy::ExtendLoop {
        allow_native_schema_upgrade,
    } = policy
    {
        let next = original
            .bars()
            .max(required_bars(&value, selected, original.tempo())?);
        if next > 128 && original.schema_version() == 1 {
            require(
                allow_native_schema_upgrade,
                "Extending beyond 128 bars requires explicit native schema upgrade permission",
            )?;
            value["schemaVersion"] = native::NATIVE_SCHEMA_VERSION.into();
        }
        value["bars"] = next.into();
    }
    require(
        value != *original.value(),
        "Clip edit would not change the document",
    )?;
    ProjectDocument::from_value(value)
}

impl ProjectDocument {
    /// Apply only explicitly requested settings. First original snapshots retain
    /// exact presence and raw values, including omitted/default/null distinctions.
    pub fn update_clip(
        &self,
        address: &DocumentClipAddress,
        patch: &ClipPatch,
        policy: ClipTimelinePolicy,
    ) -> Result<Self> {
        patch.validate().map_err(contract)?;
        require(
            patch.matches_address(address),
            "Clip address and patch representations differ",
        )?;
        let selected = target(self, address, true)?;
        let before = settings(selected.clip(self.value()), selected.fields());
        let mut value = self.value().clone();
        let clip = selected.clip_mut(&mut value);
        match patch {
            ClipPatch::LegacyAudio(patch) => {
                let audio = clip.as_object_mut().expect("validated legacy audio");
                field(audio, "startTick", &patch.start_tick)?;
                field(audio, "trimStartSeconds", &patch.trim_start_seconds)?;
                field(audio, "trimEndSeconds", &patch.trim_end_seconds)?;
                field(audio, "fadeInSeconds", &patch.fade_in_seconds)?;
                field(audio, "fadeOutSeconds", &patch.fade_out_seconds)?;
            }
            ClipPatch::NativeAudio(patch) => {
                if let ValueChange::Set { value } = patch.position_frame {
                    clip["position"]["frame"] = value.into();
                }
                if let ValueChange::Set { value } = &patch.source_window {
                    *clip.get_mut("range").expect("validated native range") = json!({"unit":"audio_frames","start":value.start_frame,"count":value.frame_count});
                }
                field(
                    clip.as_object_mut().expect("validated native clip"),
                    "fades",
                    &patch.fades,
                )?;
            }
        }
        if settings(clip, selected.fields()) != before {
            if clip.get("editSource").is_none() {
                clip["editSource"] = json!({"version":1,"original":before});
            }
            if let Target::Native(track, _) = selected {
                value["tracks"][track]["recording"]["version"] =
                    native::EDITED_RECORDING_PAYLOAD_VERSION.into();
            }
        }
        finish(self, value, selected, policy)
    }

    /// Restore exact original setting presence/values while retaining the whole
    /// immutable snapshot and recording payload version.
    pub fn restore_clip(
        &self,
        address: &DocumentClipAddress,
        policy: ClipTimelinePolicy,
    ) -> Result<Self> {
        let selected = target(self, address, true)?;
        let source = selected
            .clip(self.value())
            .get("editSource")
            .ok_or_else(|| DocumentError("This clip has no original edit snapshot".into()))?;
        let originals = source["original"]
            .as_object()
            .expect("validated original clip settings");
        let mut value = self.value().clone();
        let clip = selected
            .clip_mut(&mut value)
            .as_object_mut()
            .expect("validated clip");
        for key in selected.fields() {
            clip.remove(*key);
        }
        clip.extend(originals.clone());
        finish(self, value, selected, policy)
    }

    /// Positive arrangement extent only: exact native frames/binary tempo, or
    /// the pinned declared-seconds legacy estimate. This does not decode audio.
    pub fn clip_required_bars(&self, address: &DocumentClipAddress) -> Result<u64> {
        required_bars(self.value(), target(self, address, false)?, self.tempo())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::TakeId;
    use crate::native::{AudioFades, FadeCurve, TakeReference, TimelinePosition};
    use crate::{AudioSourceWindow, LegacyAudioClipPatch, NativeAudioClipPatch};

    fn legacy() -> (ProjectDocument, DocumentClipAddress) {
        let mut value = ProjectDocument::new("Synthetic exact clip")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["id"] = "録音\0\n".into();
        value["tracks"][0]["kind"] = "audio".into();
        value["tracks"][0]["audio"] = json!({"origin":"import","mimeType":"audio/wav","durationSeconds":10,"byteLength":100,
            "vendor":serde_json::from_str::<Value>(r#"{"integer":1844674407370955161600001,"decimal":0.12345678901234567890123456789}"#).unwrap()});
        value["tracks"][0]["recording"] = json!({"version":"foreign","clips":"opaque"});
        (
            ProjectDocument::from_value(value).unwrap(),
            DocumentClipAddress::LegacyAudio {
                track_id: "録音\0\n".into(),
            },
        )
    }
    fn legacy_patch() -> LegacyAudioClipPatch {
        LegacyAudioClipPatch {
            start_tick: FieldChange::Keep,
            trim_start_seconds: FieldChange::Keep,
            trim_end_seconds: FieldChange::Keep,
            fade_in_seconds: FieldChange::Keep,
            fade_out_seconds: FieldChange::Keep,
        }
    }
    fn native_patch() -> NativeAudioClipPatch {
        NativeAudioClipPatch {
            position_frame: ValueChange::Keep,
            source_window: ValueChange::Keep,
            fades: FieldChange::Keep,
        }
    }
    fn clip(id: &str) -> TakeClip {
        TakeClip {
            id: id.into(),
            source: TakeReference {
                take_id: TakeId("synthetic-take".into()),
                manifest_sha256: "a".repeat(64),
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames {
                start: 100,
                count: 96_000,
            },
            position: TimelinePosition {
                frame: 0,
                sample_rate: 48_000,
            },
            fades: None,
            edit_source: None,
        }
    }
    fn native_project(fades: Option<Value>) -> (ProjectDocument, DocumentClipAddress) {
        let mut project = ProjectDocument::new("Synthetic frame clip")
            .unwrap()
            .upgrade_native()
            .unwrap();
        project
            .append_take_clip("recording", "Original", clip("selected"))
            .unwrap();
        project
            .append_take_clip("recording", "Unchanged sibling", clip("sibling"))
            .unwrap();
        let mut value = project.value().clone();
        if let Some(fades) = fades {
            value["tracks"][2]["recording"]["clips"][0]["fades"] = fades;
        }
        value["tracks"][2]["recording"]["clips"][1]["fades"] = Value::Null;
        value["tracks"][2]["audio"] = json!({"editSource":"opaque on this kind"});
        (
            ProjectDocument::from_value(value).unwrap(),
            DocumentClipAddress::NativeAudio {
                track_id: "recording".into(),
                clip_id: "selected".into(),
            },
        )
    }
    fn number(value: &str) -> serde_json::Number {
        serde_json::from_str(value).unwrap()
    }

    #[test]
    fn legacy_settings_use_exact_presence_and_numbers_and_keep_first_source_forever() {
        let (source, address) = legacy();
        let mut patch = legacy_patch();
        patch.start_tick = FieldChange::Set { value: 0 }; // Explicit default is an edit.
        patch.trim_start_seconds = FieldChange::Set {
            value: number("0.12345678901234567890123456789"),
        };
        patch.fade_in_seconds = FieldChange::Set {
            value: number("0.000"),
        };
        let first = source
            .update_clip(
                &address,
                &ClipPatch::LegacyAudio(patch),
                ClipTimelinePolicy::PreserveLoop,
            )
            .unwrap();
        assert_eq!(
            first.value()["tracks"][0]["audio"]["editSource"],
            json!({"version":1,"original":{}})
        );
        assert_eq!(
            first.value()["tracks"][0]["audio"]["trimStartSeconds"].to_string(),
            "0.12345678901234567890123456789"
        );
        assert_eq!(
            first.value()["tracks"][0]["audio"]["fadeInSeconds"].to_string(),
            "0.000"
        );
        let mut value = first.value().clone();
        value["tracks"][0]["audio"]["editSource"]["vendor"] =
            source.value()["tracks"][0]["audio"]["vendor"].clone();
        let first = ProjectDocument::from_value(value).unwrap();
        let mut patch = legacy_patch();
        patch.start_tick = FieldChange::Set { value: 12345 };
        let second = first
            .update_clip(
                &address,
                &ClipPatch::LegacyAudio(patch),
                ClipTimelinePolicy::PreserveLoop,
            )
            .unwrap();
        assert_eq!(
            first.value()["tracks"][0]["audio"]["editSource"],
            second.value()["tracks"][0]["audio"]["editSource"]
        );
        let restored = second
            .restore_clip(&address, ClipTimelinePolicy::PreserveLoop)
            .unwrap();
        let mut expected = source.value().clone();
        expected["tracks"][0]["audio"]["editSource"] =
            first.value()["tracks"][0]["audio"]["editSource"].clone();
        assert_eq!(restored.value(), &expected);
        assert!(
            restored
                .restore_clip(&address, ClipTimelinePolicy::PreserveLoop)
                .is_err()
        );
        assert!(
            source.value()["tracks"][0]["audio"]
                .get("editSource")
                .is_none()
        );
    }

    #[test]
    fn remove_and_keep_are_distinct_and_noops_do_not_create_snapshots() {
        let (source, address) = legacy();
        assert!(
            source
                .update_clip(
                    &address,
                    &ClipPatch::LegacyAudio(legacy_patch()),
                    ClipTimelinePolicy::PreserveLoop
                )
                .is_err()
        );
        let mut value = source.value().clone();
        value["tracks"][0]["audio"]["fadeOutSeconds"] = serde_json::from_str("0.0000").unwrap();
        let source = ProjectDocument::from_value(value).unwrap();
        let mut patch = legacy_patch();
        patch.fade_out_seconds = FieldChange::Remove;
        let removed = source
            .update_clip(
                &address,
                &ClipPatch::LegacyAudio(patch),
                ClipTimelinePolicy::PreserveLoop,
            )
            .unwrap();
        assert!(
            removed.value()["tracks"][0]["audio"]
                .get("fadeOutSeconds")
                .is_none()
        );
        assert_eq!(
            removed.value()["tracks"][0]["audio"]["editSource"]["original"]["fadeOutSeconds"]
                .to_string(),
            "0.0000"
        );
        let restored = removed
            .restore_clip(&address, ClipTimelinePolicy::PreserveLoop)
            .unwrap();
        assert_eq!(
            restored.value()["tracks"][0]["audio"]["fadeOutSeconds"].to_string(),
            "0.0000"
        );
    }

    #[test]
    fn native_edits_preserve_absent_null_and_object_fades_and_all_siblings() {
        for fades in [
            None,
            Some(Value::Null),
            Some(json!({"inFrames":12,"outFrames":19,"curve":"equal_power"})),
        ] {
            let (source, address) = native_project(fades);
            let mut patch = native_patch();
            patch.position_frame = ValueChange::Set { value: -123 };
            patch.source_window = ValueChange::Set {
                value: AudioSourceWindow {
                    start_frame: 101,
                    frame_count: 95_999,
                },
            };
            let first = source
                .update_clip(
                    &address,
                    &ClipPatch::NativeAudio(patch),
                    ClipTimelinePolicy::PreserveLoop,
                )
                .unwrap();
            assert_eq!(first.value()["tracks"][2]["recording"]["version"], 2);
            assert_eq!(source.value()["tracks"][2]["recording"]["version"], 1);
            let before = &source.value()["tracks"][2]["recording"]["clips"][0];
            let after = &first.value()["tracks"][2]["recording"]["clips"][0];
            assert_eq!(
                before.get("fades"),
                after["editSource"]["original"].get("fades")
            );
            assert_eq!(
                source.value()["tracks"][2]["recording"]["clips"][1],
                first.value()["tracks"][2]["recording"]["clips"][1]
            );
            let projection = first.take_clips().unwrap().remove(0);
            let original = projection.original_view().unwrap().unwrap();
            assert_eq!(original.source, projection.source);
            assert_eq!(original.position.frame, 0);
            assert_eq!(
                original.range,
                TakeRange::AudioFrames {
                    start: 100,
                    count: 96_000
                }
            );
            assert!(original.edit_source.is_none());
            let mut patch = native_patch();
            patch.fades = FieldChange::Set {
                value: AudioFades {
                    in_frames: 11,
                    out_frames: 17,
                    curve: FadeCurve::Linear,
                },
            };
            let again = first
                .update_clip(
                    &address,
                    &ClipPatch::NativeAudio(patch),
                    ClipTimelinePolicy::PreserveLoop,
                )
                .unwrap();
            assert_eq!(
                after["editSource"],
                again.value()["tracks"][2]["recording"]["clips"][0]["editSource"]
            );
            let restored = again
                .restore_clip(&address, ClipTimelinePolicy::PreserveLoop)
                .unwrap();
            let mut expected = source.value().clone();
            expected["tracks"][2]["recording"]["version"] = 2.into();
            expected["tracks"][2]["recording"]["clips"][0]["editSource"] =
                after["editSource"].clone();
            assert_eq!(restored.value(), &expected);
        }
    }

    #[test]
    fn native_extent_uses_exact_binary_tempo_and_signed_frame_math() {
        let (source, address) = native_project(None);
        let mut value = source.value().clone();
        value["tempo"] = json!(120.0);
        let exact = ProjectDocument::from_value(value.clone()).unwrap();
        assert_eq!(exact.clip_required_bars(&address).unwrap(), 1);
        value["tempo"] = json!(f64::from_bits(120.0f64.to_bits() + 1));
        assert_eq!(
            ProjectDocument::from_value(value.clone())
                .unwrap()
                .clip_required_bars(&address)
                .unwrap(),
            2
        );
        value["tracks"][2]["recording"]["clips"][0]["position"]["frame"] = (-96_000).into();
        let preroll = ProjectDocument::from_value(value).unwrap();
        assert_eq!(preroll.clip_required_bars(&address).unwrap(), 0);
        let mut patch = native_patch();
        patch.position_frame = ValueChange::Set { value: i64::MAX };
        assert!(
            preroll
                .update_clip(
                    &address,
                    &ClipPatch::NativeAudio(patch),
                    ClipTimelinePolicy::PreserveLoop
                )
                .is_err()
        );
    }

    #[test]
    fn explicit_extension_promotes_only_when_needed_and_extent_only_does_not_snapshot() {
        let (source, address) = legacy();
        let mut value = source.value().clone();
        value["tracks"][0]["audio"]["startTick"] = (128 * 3840).into();
        value["tracks"][0]["audio"]["trimEndSeconds"] = json!(0.001);
        let source = ProjectDocument::from_value(value).unwrap();
        assert_eq!(source.clip_required_bars(&address).unwrap(), 129);
        assert!(
            source
                .update_clip(
                    &address,
                    &ClipPatch::LegacyAudio(legacy_patch()),
                    ClipTimelinePolicy::ExtendLoop {
                        allow_native_schema_upgrade: false
                    }
                )
                .is_err()
        );
        let extended = source
            .update_clip(
                &address,
                &ClipPatch::LegacyAudio(legacy_patch()),
                ClipTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: true,
                },
            )
            .unwrap();
        assert_eq!((extended.schema_version(), extended.bars()), (2, 129));
        assert!(
            extended.value()["tracks"][0]["audio"]
                .get("editSource")
                .is_none()
        );
        assert_eq!(source.schema_version(), 1);
        let (source, address) = native_project(None);
        let mut value = source.value().clone();
        value["bars"] = 1.into();
        value["tracks"][2]["recording"]["clips"][0]["position"]["frame"] = 480_000.into();
        let source = ProjectDocument::from_value(value).unwrap();
        let extended = source
            .update_clip(
                &address,
                &ClipPatch::NativeAudio(native_patch()),
                ClipTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: false,
                },
            )
            .unwrap();
        assert!(extended.bars() > 1);
        assert_eq!(extended.value()["tracks"][2]["recording"]["version"], 1);
        assert!(extended.take_clips().unwrap()[0].edit_source.is_none());
    }

    #[test]
    fn all_rejections_leave_originals_unchanged_and_opaque_kinds_uninterpreted() {
        let (source, address) = legacy();
        let original = source.clone();
        let mut patch = legacy_patch();
        patch.trim_end_seconds = FieldChange::Set { value: number("0") };
        assert!(
            source
                .update_clip(
                    &address,
                    &ClipPatch::LegacyAudio(patch),
                    ClipTimelinePolicy::PreserveLoop
                )
                .is_err()
        );
        let mut patch = legacy_patch();
        patch.fade_in_seconds = FieldChange::Set { value: number("9") };
        patch.fade_out_seconds = FieldChange::Set { value: number("2") };
        assert!(
            source
                .update_clip(
                    &address,
                    &ClipPatch::LegacyAudio(patch),
                    ClipTimelinePolicy::PreserveLoop
                )
                .is_err()
        );
        assert!(
            source
                .update_clip(
                    &address,
                    &ClipPatch::NativeAudio(native_patch()),
                    ClipTimelinePolicy::PreserveLoop
                )
                .is_err()
        );
        assert!(
            source
                .restore_clip(&address, ClipTimelinePolicy::PreserveLoop)
                .is_err()
        );
        let mut locked = source.value().clone();
        locked["tracks"][0]["locked"] = true.into();
        assert!(
            ProjectDocument::from_value(locked)
                .unwrap()
                .restore_clip(&address, ClipTimelinePolicy::PreserveLoop)
                .is_err()
        );
        assert_eq!(source, original);
        let (source, address) = native_project(None);
        let mut value = source.value().clone();
        value["tracks"][2]["recording"]["clips"][0]["range"] =
            json!({"unit":"events","start":100,"count":2});
        let events = ProjectDocument::from_value(value).unwrap();
        assert!(
            events
                .update_clip(
                    &address,
                    &ClipPatch::NativeAudio(native_patch()),
                    ClipTimelinePolicy::PreserveLoop
                )
                .is_err()
        );
        assert!(events.clip_required_bars(&address).is_err());
    }
}
