//! Explicit schema-2 references to sealed original takes.
//!
//! These types validate musical document structure, not object availability or
//! manifest authenticity. A store resolver must verify those before publication.

use crate::contracts::{TakeId, Validate};
use crate::{DocumentError, ProjectDocument, Result, require};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const NATIVE_SCHEMA_VERSION: u64 = 2;
pub const RECORDING_PAYLOAD_VERSION: u32 = 1;
pub const EDITED_RECORDING_PAYLOAD_VERSION: u32 = 2;
pub const MAX_NATIVE_PROJECT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_NATIVE_BARS: u64 = u32::MAX as u64;
pub const MAX_TAKE_CLIPS: usize = 16_384;

fn identifier(value: &str, label: &str) -> Result<()> {
    require(
        !value.is_empty()
            && value.len() <= 96
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)),
        format!("Invalid native {label}"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TakeReference {
    pub take_id: TakeId,
    pub manifest_sha256: String,
    pub stream_id: String,
}
impl TakeReference {
    pub fn validate(&self) -> Result<()> {
        self.take_id
            .validate()
            .map_err(|error| DocumentError(error.to_string()))?;
        identifier(&self.stream_id, "stream identity")?;
        require(
            self.manifest_sha256.len() == 64
                && self
                    .manifest_sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid sealed take manifest digest",
        )
    }
}

/// A half-open interval in the immutable source's own units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "snake_case", deny_unknown_fields)]
pub enum TakeRange {
    AudioFrames { start: u64, count: u64 },
    Events { start: u64, count: u64 },
}
impl TakeRange {
    pub fn start(&self) -> u64 {
        match self {
            Self::AudioFrames { start, .. } | Self::Events { start, .. } => *start,
        }
    }
    pub fn count(&self) -> u64 {
        match self {
            Self::AudioFrames { count, .. } | Self::Events { count, .. } => *count,
        }
    }
    pub fn end(&self) -> Result<u64> {
        require(
            self.count() > 0,
            "A native clip source range must be nonempty",
        )?;
        self.start()
            .checked_add(self.count())
            .filter(|end| *end <= i64::MAX as u64)
            .ok_or_else(|| DocumentError("Native clip source range overflow".into()))
    }
}

/// Arrangement placement, never a source clock observation or calibration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelinePosition {
    pub frame: i64,
    pub sample_rate: u32,
}
impl TimelinePosition {
    pub fn validate(&self) -> Result<()> {
        require(
            (8_000..=768_000).contains(&self.sample_rate),
            "Unsupported native arrangement sample rate",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FadeCurve {
    Linear,
    EqualPower,
}

/// Non-destructive gain envelopes in source audio frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioFades {
    pub in_frames: u64,
    pub out_frames: u64,
    pub curve: FadeCurve,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TakeClip {
    pub id: String,
    pub source: TakeReference,
    pub range: TakeRange,
    pub position: TimelinePosition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fades: Option<AudioFades>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_source: Option<Value>,
}
impl TakeClip {
    pub fn validate(&self) -> Result<()> {
        self.original_view().map(|_| ())
    }

    fn validate_current(&self) -> Result<()> {
        identifier(&self.id, "clip identity")?;
        self.source.validate()?;
        self.range.end()?;
        self.position.validate()?;
        match self.range {
            TakeRange::AudioFrames { count, .. } => {
                // Initial publication requires source and arrangement rates to
                // match. A future resampling policy must be explicit.
                require(
                    self.position.frame.checked_add(count as i64).is_some(),
                    "Native audio clip exceeds the signed arrangement timeline",
                )?;
                if let Some(fades) = &self.fades {
                    require(
                        fades
                            .in_frames
                            .checked_add(fades.out_frames)
                            .is_some_and(|sum| sum <= count),
                        "Native audio fades must fit inside the source window",
                    )?;
                }
            }
            TakeRange::Events { .. } => {
                require(
                    self.fades.is_none(),
                    "Raw event clips cannot carry audio-frame fades",
                )?;
            }
        }
        Ok(())
    }

    /// A semantic view of the original settings; never a replacement serializer
    /// for the raw document. Opaque snapshot extensions remain in edit_source.
    pub fn original_view(&self) -> Result<Option<TakeClip>> {
        self.validate_current()?;
        let Some(source) = &self.edit_source else {
            return Ok(None);
        };
        require(
            source.is_object() && source["version"] == 1,
            "Unsupported native original clip snapshot",
        )?;
        require(
            source.get("editSource").is_none(),
            "Native original clip snapshots cannot contain a nested snapshot",
        )?;
        let settings = source["original"]
            .as_object()
            .ok_or_else(|| DocumentError("Invalid native original clip settings".into()))?;
        require(
            settings.contains_key("range")
                && settings.contains_key("position")
                && settings
                    .keys()
                    .all(|key| matches!(key.as_str(), "range" | "position" | "fades")),
            "Invalid native original clip setting keys",
        )?;
        let range: TakeRange = serde_json::from_value(settings["range"].clone())
            .map_err(|error| DocumentError(format!("Invalid native original range: {error}")))?;
        let position: TimelinePosition = serde_json::from_value(settings["position"].clone())
            .map_err(|error| DocumentError(format!("Invalid native original position: {error}")))?;
        let fades: Option<AudioFades> = match settings.get("fades") {
            None | Some(Value::Null) => None,
            Some(value) => Some(serde_json::from_value(value.clone()).map_err(|error| {
                DocumentError(format!("Invalid native original fades: {error}"))
            })?),
        };
        require(
            matches!(self.range, TakeRange::AudioFrames { .. })
                && matches!(range, TakeRange::AudioFrames { .. }),
            "Only audio-frame clips can have original clip snapshots",
        )?;
        require(
            position.sample_rate == self.position.sample_rate,
            "Original clip sample rate differs from the immutable arrangement rate",
        )?;
        let original = TakeClip {
            id: self.id.clone(),
            source: self.source.clone(),
            range,
            position,
            fades,
            edit_source: None,
        };
        original.validate_current()?;
        Ok(Some(original))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingPayload {
    pub version: u32,
    pub clips: Vec<TakeClip>,
}
impl RecordingPayload {
    pub fn validate(&self) -> Result<()> {
        require(
            matches!(
                self.version,
                RECORDING_PAYLOAD_VERSION | EDITED_RECORDING_PAYLOAD_VERSION
            ),
            "Unsupported native recording payload version",
        )?;
        require(
            self.clips.len() <= MAX_TAKE_CLIPS,
            "Too many native take clips",
        )?;
        let mut identities = BTreeSet::new();
        for clip in &self.clips {
            require(
                self.version != RECORDING_PAYLOAD_VERSION || clip.edit_source.is_none(),
                "Recording payload version 1 cannot carry original clip snapshots",
            )?;
            clip.validate()?;
            require(
                identities.insert(&clip.id),
                "Duplicate native clip identity",
            )?;
        }
        Ok(())
    }
}

pub(crate) fn recording_payload(track: &Value) -> Result<RecordingPayload> {
    let identity = track["id"]
        .as_str()
        .ok_or_else(|| DocumentError("Invalid native track identity".into()))?;
    identifier(identity, "track identity")?;
    let payload: RecordingPayload = serde_json::from_value(track["recording"].clone())
        .map_err(|error| DocumentError(format!("Invalid native recording payload: {error}")))?;
    payload.validate()?;
    // Option<Value> intentionally projects JSON null as None. Inspect the raw
    // slot too, so explicit null cannot bypass version or snapshot validation.
    for clip in track["recording"]["clips"]
        .as_array()
        .expect("validated recording clips")
    {
        if let Some(source) = clip.get("editSource") {
            require(
                payload.version == EDITED_RECORDING_PAYLOAD_VERSION && source.is_object(),
                "Original clip snapshots require recording payload version 2 and a valid object",
            )?;
        }
    }
    Ok(payload)
}

pub(crate) fn validate_recording_tracks(tracks: &[Value]) -> Result<()> {
    let mut identities = BTreeSet::new();
    for track in tracks.iter().filter(|track| track["kind"] == "recording") {
        for clip in recording_payload(track)?.clips {
            require(
                identities.insert(clip.id),
                "Duplicate native clip identity across recording tracks",
            )?;
            require(
                identities.len() <= MAX_TAKE_CLIPS,
                "Too many native take clips in the project",
            )?;
        }
    }
    Ok(())
}

impl ProjectDocument {
    /// Explicit, value-preserving upgrade. Publication owns revision/history.
    pub fn upgrade_native(&self) -> Result<Self> {
        require(
            self.schema_version() == 1,
            "Native upgrade requires a schema-1 document",
        )?;
        let mut value = self.value.clone();
        value["schemaVersion"] = NATIVE_SCHEMA_VERSION.into();
        Self::from_value(value)
    }

    /// Append only to a native recording track, or create one if its ID is new.
    /// Failure leaves every document field unchanged.
    pub fn append_take_clip(
        &mut self,
        track_id: &str,
        track_name: &str,
        clip: TakeClip,
    ) -> Result<()> {
        require(
            self.schema_version() == NATIVE_SCHEMA_VERSION,
            "Appending a sealed take requires explicit schema-2 upgrade",
        )?;
        identifier(track_id, "track identity")?;
        require(
            clip.edit_source.is_none(),
            "New recording publication cannot supply an original edit snapshot",
        )?;
        clip.validate()?;
        require(
            !track_name.trim().is_empty() && track_name.chars().count() <= 160,
            "Invalid native recording track name",
        )?;
        let mut value = self.value.clone();
        let tracks = value["tracks"].as_array_mut().expect("validated tracks");
        if let Some(track) = tracks.iter_mut().find(|track| track["id"] == track_id) {
            require(
                track["kind"] == "recording",
                "Cannot reinterpret an existing track as a native recording",
            )?;
            require(track["locked"] == false, "Recording track is locked")?;
            let clips = track["recording"]["clips"]
                .as_array_mut()
                .expect("validated recording payload");
            clips.push(
                serde_json::to_value(clip).map_err(|error| DocumentError(error.to_string()))?,
            );
        } else {
            tracks.push(json!({
                "id": track_id,
                "name": track_name,
                "kind": "recording",
                "gain": 1.0,
                "muted": false,
                "locked": false,
                "armed": false,
                "recording": RecordingPayload {
                    version: RECORDING_PAYLOAD_VERSION,
                    clips: vec![clip],
                },
            }));
        }
        // The constructor checks the publishable metadata size before mutation.
        let candidate = Self::from_value(value)?;
        *self = candidate;
        Ok(())
    }

    /// Only schema-owned native paths are projected; legacy metadata stays opaque.
    pub fn take_clips(&self) -> Result<Vec<TakeClip>> {
        if self.schema_version() != NATIVE_SCHEMA_VERSION {
            return Ok(Vec::new());
        }
        let mut clips = Vec::new();
        for track in self
            .tracks()
            .iter()
            .filter(|track| track["kind"] == "recording")
        {
            clips.extend(recording_payload(track)?.clips);
        }
        Ok(clips)
    }

    pub fn referenced_takes(&self) -> Result<BTreeSet<TakeReference>> {
        Ok(self
            .take_clips()?
            .into_iter()
            .map(|clip| clip.source)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_PROJECT_BYTES;

    fn audio_clip(id: &str) -> TakeClip {
        TakeClip {
            id: id.into(),
            source: TakeReference {
                take_id: TakeId("take-original-1".into()),
                manifest_sha256: "a".repeat(64),
                stream_id: "microphone-1".into(),
            },
            range: TakeRange::AudioFrames {
                start: 12_000,
                count: 48_000 * 60 * 31,
            },
            position: TimelinePosition {
                frame: -24_000,
                sample_rate: 48_000,
            },
            fades: Some(AudioFades {
                in_frames: 4_800,
                out_frames: 9_600,
                curve: FadeCurve::EqualPower,
            }),
            edit_source: None,
        }
    }

    fn raw_clip(id: &str) -> TakeClip {
        TakeClip {
            id: id.into(),
            source: TakeReference {
                take_id: TakeId("take-midi-original".into()),
                manifest_sha256: "b".repeat(64),
                stream_id: "controller-ump".into(),
            },
            range: TakeRange::Events {
                start: 73,
                count: 90_000,
            },
            position: TimelinePosition {
                frame: i64::MIN,
                sample_rate: 96_000,
            },
            fades: None,
            edit_source: None,
        }
    }

    fn native_project() -> ProjectDocument {
        ProjectDocument::new("Native original")
            .unwrap()
            .upgrade_native()
            .unwrap()
    }

    #[test]
    fn explicit_upgrade_preserves_opaque_collisions_precision_and_original_snapshots() {
        let mut value = ProjectDocument::new("Original source")
            .unwrap()
            .value()
            .clone();
        value["recording"] = json!({"takeId":"foreign-root","version":"not our schema"});
        value["takeId"] = json!({"opaque":"root"});
        value["tracks"][0]["recording"] = json!({"clips":"foreign opaque data"});
        value["tracks"][0]["takeId"] = json!({"unexpected":"allowed metadata"});
        value["tracks"][0]["vendor:exact"] = serde_json::from_str(
            r#"{"integer":184467440737095516160000001,"decimal":0.123456789012345678901234567890123456789}"#,
        ).unwrap();
        value["tracks"][0]["notes"] = json!([{
            "id":"original-note","pitch":60,"startTick":123,"durationTicks":960,
            "velocity16":49152,"provenance":{"source":"human","takeId":"foreign"},
            "editSource":{"version":1,"pitch":60,"startTick":120,
                "durationTicks":960,"velocity16":49152,"vendor:original":"retained"}
        }]);
        value["tracks"][1]["kind"] = "audio".into();
        value["tracks"][1]["recording"] = json!({"version":99,"unknown":true});
        value["tracks"][1]["audio"] = json!({
            "origin":"microphone","mimeType":"audio/webm","durationSeconds":2.0,
            "byteLength":100,"trimStartSeconds":0.2,"trimEndSeconds":1.8,
            "editSource":{"version":1,"original":{}},
            "provenance":{"capture":"unaltered"}
        });
        value["nativeImport"] = json!({"prior":"import receipt","version":"opaque"});
        value["history"] = json!([{"takeId":"foreign-history","recording":{"foreign":true}}]);
        let source = ProjectDocument::from_value(value.clone()).unwrap();
        assert_eq!(source.schema_version(), 1);
        assert!(source.referenced_takes().unwrap().is_empty());
        let upgraded = source.upgrade_native().unwrap();
        let mut expected = value.clone();
        expected["schemaVersion"] = 2.into();
        assert_eq!(upgraded.value(), &expected);
        assert_eq!(source.value(), &value);
        assert!(upgraded.referenced_takes().unwrap().is_empty());
        assert!(upgraded.upgrade_native().is_err());
        let encoded = String::from_utf8(upgraded.to_json().unwrap()).unwrap();
        assert!(encoded.contains("184467440737095516160000001"));
        assert!(encoded.contains("0.123456789012345678901234567890123456789"));
        assert_eq!(
            ProjectDocument::from_json(encoded.as_bytes())
                .unwrap()
                .value(),
            &expected
        );
    }

    #[test]
    fn long_audio_and_raw_event_references_roundtrip_without_embedded_capture() {
        let mut project = native_project();
        let audio = audio_clip("clip-audio");
        let events = raw_clip("clip-events");
        project
            .append_take_clip("recording-audio", "Original audio", audio.clone())
            .unwrap();
        project
            .append_take_clip("recording-events", "Original gestures", events.clone())
            .unwrap();
        assert_eq!(
            project.take_clips().unwrap(),
            vec![audio.clone(), events.clone()]
        );
        assert_eq!(
            project.referenced_takes().unwrap(),
            BTreeSet::from([audio.source, events.source])
        );
        let native_tracks = &project.tracks()[2..];
        for track in native_tracks {
            assert!(track.get("audio").is_none());
            assert!(track.get("notes").is_none());
            assert!(track.get("inputCapture").is_none());
        }
        assert_eq!(project.bars(), 4); // Appending never silently edits the arrangement extent.
        let mut value = project.value().clone();
        value["bars"] = MAX_NATIVE_BARS.into();
        let long_timeline = ProjectDocument::from_value(value).unwrap();
        assert_eq!(long_timeline.bars(), MAX_NATIVE_BARS);
        assert_eq!(
            ProjectDocument::from_json(&long_timeline.to_json().unwrap()).unwrap(),
            long_timeline
        );
        assert!(long_timeline.to_json().unwrap().len() < 10_000);
    }

    #[test]
    fn schema_payload_version_unit_and_required_recording_shape_are_checked() {
        let mut project = native_project();
        project
            .append_take_clip("recording-1", "Take", audio_clip("clip-1"))
            .unwrap();
        for (path, replacement) in [
            ("/schemaVersion", json!(1)),
            ("/schemaVersion", json!(3)),
            ("/tracks/2/recording/version", json!(3)),
            ("/tracks/2/recording", Value::Null),
            ("/tracks/2/recording/clips/0/range/unit", json!("seconds")),
            ("/tracks/2/recording/clips/0/range/start", json!(-1)),
            (
                "/tracks/2/recording/clips/0/position/frame",
                json!(u64::MAX),
            ),
            ("/tracks/2/recording/clips/0/position/sampleRate", json!(0)),
            ("/tracks/2/id", json!("track with spaces")),
            ("/tracks/2/recording/clips/0/id", json!("clip/unsafe")),
        ] {
            let mut invalid = project.value().clone();
            *invalid.pointer_mut(path).unwrap() = replacement;
            assert!(ProjectDocument::from_value(invalid).is_err(), "{path}");
        }
        let mut unknown_semantics = project.value().clone();
        unknown_semantics["tracks"][2]["recording"]["clips"][0]["automaticResampling"] =
            true.into();
        assert!(ProjectDocument::from_value(unknown_semantics).is_err());
        let mut unknown_payload = project.value().clone();
        unknown_payload["tracks"][2]["recording"]["futureSemantic"] = true.into();
        assert!(ProjectDocument::from_value(unknown_payload).is_err());
    }

    #[test]
    fn hashes_range_overflow_signed_audio_extent_and_fades_reject_invalid_claims() {
        for hash in [
            "a".repeat(63),
            "A".repeat(64),
            "../manifest".into(),
            "g".repeat(64),
        ] {
            let mut clip = audio_clip("clip-1");
            clip.source.manifest_sha256 = hash;
            assert!(clip.validate().is_err());
        }
        for range in [
            TakeRange::AudioFrames { start: 0, count: 0 },
            TakeRange::Events {
                start: u64::MAX,
                count: 1,
            },
            TakeRange::AudioFrames {
                start: i64::MAX as u64,
                count: 1,
            },
            TakeRange::Events {
                start: 0,
                count: u64::MAX,
            },
        ] {
            assert!(range.end().is_err());
        }
        let mut clip = audio_clip("clip-1");
        clip.position.frame = i64::MAX;
        assert!(clip.validate().is_err());
        let mut clip = audio_clip("clip-1");
        clip.fades.as_mut().unwrap().in_frames = clip.range.count();
        assert!(clip.validate().is_err());
        clip.fades.as_mut().unwrap().in_frames = u64::MAX;
        assert!(clip.validate().is_err());
        let mut events = raw_clip("events");
        events.fades = Some(AudioFades {
            in_frames: 0,
            out_frames: 0,
            curve: FadeCurve::Linear,
        });
        assert!(events.validate().is_err());
    }

    #[test]
    fn failed_append_is_atomic_and_clip_identity_is_global() {
        let source = ProjectDocument::new("Still schema one").unwrap();
        let mut rejected = source.clone();
        assert!(
            rejected
                .append_take_clip("recording", "Take", audio_clip("clip-1"))
                .is_err()
        );
        assert_eq!(rejected, source);

        let mut project = source.upgrade_native().unwrap();
        project
            .append_take_clip("recording", "Original name", audio_clip("clip-1"))
            .unwrap();
        let before = project.clone();
        for (track_id, clip) in [
            ("recording", audio_clip("clip-1")),
            ("different-recording", audio_clip("clip-1")),
            (
                before.tracks()[0]["id"].as_str().unwrap(),
                audio_clip("clip-2"),
            ),
        ] {
            assert!(
                project
                    .append_take_clip(track_id, "Rejected", clip)
                    .is_err()
            );
            assert_eq!(project, before);
        }
        let mut value = project.value().clone();
        value["tracks"][2]["locked"] = true.into();
        let mut locked = ProjectDocument::from_value(value).unwrap();
        let before = locked.clone();
        assert!(
            locked
                .append_take_clip("recording", "Take", audio_clip("clip-2"))
                .is_err()
        );
        assert_eq!(locked, before);

        project
            .append_take_clip("recording", "Must not rename", audio_clip("clip-2"))
            .unwrap();
        assert_eq!(project.tracks()[2]["name"], "Original name");
        assert_eq!(project.referenced_takes().unwrap().len(), 1);
        let mut duplicate = project.value().clone();
        let mut copied = duplicate["tracks"][2].clone();
        copied["id"] = "new-recording".into();
        duplicate["tracks"].as_array_mut().unwrap().push(copied);
        assert!(ProjectDocument::from_value(duplicate).is_err());
    }

    #[test]
    fn extraction_uses_only_declared_paths_and_preserves_native_track_extensions() {
        let mut project = native_project();
        project
            .append_take_clip("recording", "Take", audio_clip("clip-1"))
            .unwrap();
        let mut value = project.value().clone();
        value["tracks"][2]["assetId"] = json!("provider-not-local");
        value["tracks"][2]["sourceAssetId"] = json!({"foreign":"metadata"});
        value["tracks"][2]["audio"] = json!({"foreign":"not a legacy audio payload"});
        value["tracks"][2]["notes"] = json!({"foreign":"not inferred editable notes"});
        value["tracks"][2]["provenance"] = json!({"input":{"assetId":"foreign"}});
        value["tracks"][0]["recording"] = json!({
            "version":1,"clips":[raw_clip("opaque-not-projected")]
        });
        value["recording"] = json!({"source":raw_clip("opaque-root")});
        let mut project = ProjectDocument::from_value(value).unwrap();
        assert!(project.referenced_assets().unwrap().is_empty());
        assert_eq!(project.referenced_takes().unwrap().len(), 1);
        let original_extensions = project.tracks()[2].clone();
        project
            .append_take_clip("recording", "Kept", audio_clip("clip-2"))
            .unwrap();
        let mut expected = original_extensions;
        expected["recording"]["clips"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(audio_clip("clip-2")).unwrap());
        assert_eq!(project.tracks()[2], expected);
    }

    #[test]
    fn native_metadata_cap_and_long_bars_do_not_relax_legacy_document_limits() {
        let mut value = native_project().value().clone();
        value["vendor:large"] = Value::String("x".repeat(MAX_PROJECT_BYTES + 1));
        let native = ProjectDocument::from_value(value.clone()).unwrap();
        assert_eq!(native.metadata_limit(), MAX_NATIVE_PROJECT_BYTES);
        assert!(native.to_json().unwrap().len() > MAX_PROJECT_BYTES);
        assert_eq!(
            ProjectDocument::from_json(&native.to_json().unwrap()).unwrap(),
            native
        );
        value["schemaVersion"] = 1.into();
        assert!(ProjectDocument::from_value(value).is_err());
        assert!(ProjectDocument::from_json(&vec![b' '; MAX_NATIVE_PROJECT_BYTES + 1]).is_err());

        let source = ProjectDocument::new("Legacy cap").unwrap();
        let mut padded = source.to_json().unwrap();
        padded.resize(MAX_PROJECT_BYTES + 1, b' ');
        assert!(ProjectDocument::from_json(&padded).is_err());
        let mut long_legacy = source.value().clone();
        long_legacy["bars"] = 129.into();
        assert!(ProjectDocument::from_value(long_legacy).is_err());
        let mut too_long_native = native_project().value().clone();
        too_long_native["bars"] = (MAX_NATIVE_BARS + 1).into();
        assert!(ProjectDocument::from_value(too_long_native).is_err());
        let mut excessive = native_project().value().clone();
        excessive["vendor:large"] = Value::String("x".repeat(MAX_NATIVE_PROJECT_BYTES));
        assert!(ProjectDocument::from_value(excessive).is_err());
    }

    #[test]
    fn original_snapshots_are_versioned_audio_only_and_keep_opaque_extensions() {
        let mut project = native_project();
        project
            .append_take_clip("recording", "Take", audio_clip("clip-1"))
            .unwrap();
        let path = "/tracks/2/recording/clips/0";
        let current = project.value().pointer(path).unwrap();
        let snapshot = json!({"version":1,"original":{"range":current["range"],"position":current["position"],"fades":null},
            "vendor":serde_json::from_str::<Value>(r#"{"integer":1844674407370955161600001,"decimal":0.12345678901234567890123456789}"#).unwrap()});
        for version in [1, 2] {
            let mut invalid = project.value().clone();
            invalid["tracks"][2]["recording"]["version"] = version.into();
            invalid.pointer_mut(path).unwrap()["editSource"] = Value::Null;
            assert!(ProjectDocument::from_value(invalid).is_err());
        }
        let mut invalid = project.value().clone();
        invalid.pointer_mut(path).unwrap()["editSource"] = snapshot.clone();
        assert!(ProjectDocument::from_value(invalid).is_err());

        let mut value = project.value().clone();
        value["tracks"][2]["recording"]["version"] = EDITED_RECORDING_PAYLOAD_VERSION.into();
        value.pointer_mut(path).unwrap()["editSource"] = snapshot.clone();
        let valid = ProjectDocument::from_value(value.clone()).unwrap();
        let typed = &valid.take_clips().unwrap()[0];
        assert_eq!(typed.edit_source.as_ref(), Some(&snapshot));
        let original = typed.original_view().unwrap().unwrap();
        assert!(original.fades.is_none());
        assert!(original.edit_source.is_none());
        assert_eq!(original.range, typed.range);
        for bad in [
            json!({"version":2,"original":snapshot["original"]}),
            json!({"version":1,"original":snapshot["original"],"editSource":{"version":1}}),
            json!({"version":1,"original":{"range":current["range"]}}),
            json!({"version":1,"original":{"range":current["range"],"position":current["position"],"editSource":{}}}),
            json!({"version":1,"original":{"range":{"unit":"events","start":0,"count":1},"position":current["position"]}}),
            json!({"version":1,"original":{"range":current["range"],"position":{"frame":0,"sampleRate":96000}}}),
            json!({"version":1,"original":{"range":current["range"],"position":current["position"],"fades":{"inFrames":u64::MAX,"outFrames":1,"curve":"linear"}}}),
        ] {
            let mut invalid = value.clone();
            invalid.pointer_mut(path).unwrap()["editSource"] = bad;
            assert!(ProjectDocument::from_value(invalid).is_err());
        }
        let before = project.clone();
        let mut supplied = audio_clip("new-clip");
        supplied.edit_source = Some(snapshot);
        assert!(
            project
                .append_take_clip("recording", "Take", supplied)
                .is_err()
        );
        assert_eq!(project, before);
        let mut typed_null = audio_clip("typed-null");
        typed_null.edit_source = Some(Value::Null);
        assert!(typed_null.validate().is_err());
    }
}
