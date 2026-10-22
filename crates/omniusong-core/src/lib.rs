//! Lossless musical documents with explicit schema-2 sealed-take references.
//!
//! Opaque fields stay in the document. Typed projections are views, never a
//! lossy replacement for a captured performance or a provider's metadata.

pub mod clip_edit;
pub mod clip_edit_types;
pub mod contracts;
pub mod native;
pub mod note_edit;
pub mod note_edit_types;
pub mod performance_projection;
pub use clip_edit_types::{
    AudioSourceWindow, ClipPatch, ClipTimelinePolicy, DocumentClipAddress, FieldChange,
    LegacyAudioClipPatch, NativeAudioClipPatch, ValueChange,
};
pub use note_edit_types::{DocumentNoteAddress, NoteTimelinePolicy, NoteValues};

use serde_json::{Value, json};
use std::{collections::BTreeSet, error::Error, fmt};

pub const PPQ: u64 = 960;
pub const MAX_PROJECT_BYTES: usize = 1_900_000;
pub use native::MAX_NATIVE_PROJECT_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentError(pub String);

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for DocumentError {}

type Result<T> = std::result::Result<T, DocumentError>;

/// Musical ticks are not audio samples or an operating-system clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tick(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleFrame(pub i64);

/// Unsigned offset within an immutable source, distinct from signed preroll time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceFrame(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Velocity16(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpressionValue32(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDocument {
    value: Value,
}

fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(DocumentError(message.into()))
    }
}

fn integer(value: &Value, min: u64, max: u64, label: &str) -> Result<u64> {
    value
        .as_u64()
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| DocumentError(format!("Invalid {label}")))
}

fn finite(value: &Value, min: f64, max: f64, label: &str) -> Result<()> {
    require(
        value
            .as_f64()
            .is_some_and(|n| n.is_finite() && (min..=max).contains(&n)),
        format!("Invalid {label}"),
    )
}

fn text(value: &Value, min: usize, max: usize, label: &str) -> Result<()> {
    require(
        value
            .as_str()
            .is_some_and(|s| (min..=max).contains(&s.chars().count())),
        format!("Invalid {label}"),
    )
}

pub fn valid_asset_id(id: &str) -> bool {
    id.split_once('.').is_some_and(|(digest, extension)| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && matches!(extension, "wav" | "webm" | "mp4" | "ogg" | "mp3" | "flac")
    })
}

fn validate_expression(note: &Value, duration: u64) -> Result<()> {
    if let Some(lanes) = note.get("expression").filter(|v| !v.is_null()) {
        let lanes = lanes
            .as_array()
            .ok_or_else(|| DocumentError("Invalid expression lanes".into()))?;
        for lane in lanes {
            require(
                matches!(lane["type"].as_str(), Some("pitch" | "pressure" | "timbre")),
                "Unknown expression lane",
            )?;
            let points = lane["points"]
                .as_array()
                .filter(|p| p.len() <= 10_000)
                .ok_or_else(|| DocumentError("Invalid expression points".into()))?;
            let mut previous = 0;
            for point in points {
                let tick = integer(&point["tick"], previous, duration, "expression tick")?;
                integer(&point["value32"], 0, u32::MAX.into(), "expression value")?;
                previous = tick;
            }
        }
    }
    Ok(())
}

fn validate_note(note: &Value, snapshot: bool) -> Result<()> {
    if !snapshot {
        text(&note["id"], 0, 99, "note identity")?;
    }
    integer(&note["pitch"], 0, 127, "note pitch")?;
    integer(&note["startTick"], 0, 1_000_000_000, "note start")?;
    let duration = integer(&note["durationTicks"], 1, 100_000_000, "note duration")?;
    integer(&note["velocity16"], 0, u16::MAX.into(), "note velocity")?;
    validate_expression(note, duration)?;
    if let Some(original) = note.get("editSource") {
        require(
            !snapshot && original["version"] == 1 && original.get("editSource").is_none(),
            "Invalid original note snapshot",
        )?;
        validate_note(original, true)?;
    }
    if let Some(input) = note.get("input") {
        validate_source(input)?;
        require(
            matches!(
                input["protocol"].as_str(),
                Some("local" | "midi1" | "midi2")
            ),
            "Invalid input protocol",
        )?;
        integer(&input["group"], 0, 15, "MIDI group")?;
        integer(&input["channel"], 0, 15, "MIDI channel")?;
        finite(&input["receivedTimeMs"], 0.0, 1e15, "input receipt time")?;
        if let Some(v) = input.get("releaseVelocity16") {
            integer(v, 0, u16::MAX.into(), "release velocity")?;
        }
        if let Some(v) = input.get("keyDurationTicks") {
            integer(v, 1, 100_000_000, "key duration")?;
        }
    }
    Ok(())
}

fn validate_source(source: &Value) -> Result<()> {
    text(&source["sourceId"], 1, 200, "input source")?;
    text(&source["sourceName"], 0, 160, "input source name")
}

fn clip_window(audio: &Value) -> Result<()> {
    let duration = audio["durationSeconds"]
        .as_f64()
        .ok_or_else(|| DocumentError("Invalid source duration".into()))?;
    let optional = |field: &str, default: f64| -> Result<f64> {
        match audio.get(field) {
            None => Ok(default),
            Some(v) => v
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| DocumentError(format!("Invalid {field}"))),
        }
    };
    let start = optional("trimStartSeconds", 0.0)?;
    let end = optional("trimEndSeconds", duration)?;
    require(
        start >= 0.0 && end <= duration && end > start && end - start + 1e-9 >= 0.001,
        "Source window must fit within the original audio",
    )?;
    let fade_in = optional("fadeInSeconds", 0.0)?;
    let fade_out = optional("fadeOutSeconds", 0.0)?;
    require(
        fade_in >= 0.0 && fade_out >= 0.0 && fade_in + fade_out <= end - start + 1e-9,
        "Fades must fit inside the source window",
    )?;
    if let Some(start) = audio.get("startTick") {
        integer(start, 0, 1_000_000_000, "audio start")?;
    }
    Ok(())
}

fn validate_audio(audio: &Value) -> Result<()> {
    require(
        matches!(
            audio["origin"].as_str(),
            Some("microphone" | "import" | "generated")
        ),
        "Invalid audio origin",
    )?;
    text(&audio["mimeType"], 0, 100, "audio MIME type")?;
    finite(&audio["durationSeconds"], 0.001, 86400.0, "source duration")?;
    integer(
        &audio["byteLength"],
        1,
        30 * 1024 * 1024,
        "audio byte length",
    )?;
    clip_window(audio)?;
    if let Some(source) = audio.get("editSource") {
        require(source["version"] == 1, "Unsupported original clip snapshot")?;
        let settings = source["original"]
            .as_object()
            .ok_or_else(|| DocumentError("Invalid original clip settings".into()))?;
        let keys = [
            "startTick",
            "trimStartSeconds",
            "trimEndSeconds",
            "fadeInSeconds",
            "fadeOutSeconds",
        ];
        require(
            settings.keys().all(|k| keys.contains(&k.as_str())),
            "Invalid original clip settings",
        )?;
        let mut original = audio.clone();
        let original = original
            .as_object_mut()
            .ok_or_else(|| DocumentError("Invalid audio clip".into()))?;
        for key in keys {
            original.remove(key);
        }
        original.remove("editSource");
        original.extend(settings.clone());
        clip_window(&Value::Object(original.clone()))?;
    }
    if let Some(timing) = audio.get("recordingTiming") {
        require(
            audio["origin"] == "microphone",
            "Recording timing needs a microphone source",
        )?;
        require(
            timing["version"] == 1
                && timing["method"] == "browser-recorder-start-call"
                && timing["calibration"] == "unmeasured"
                && timing["ppq"] == PPQ,
            "Invalid legacy recording timing estimate",
        )?;
        finite(
            &timing["captureAudioTimeSeconds"],
            0.0,
            1e12,
            "capture clock",
        )?;
        finite(
            &timing["songStartAudioTimeSeconds"],
            0.0,
            1e12,
            "song start clock",
        )?;
        finite(&timing["tempo"], 30.0, 300.0, "recording tempo")?;
        let loop_ticks = integer(
            &timing["loopTicks"],
            3840,
            128 * 3840,
            "recording loop length",
        )?;
        require(loop_ticks % 3840 == 0, "Invalid recording loop length")?;
        let seconds_per_tick = 60.0 / timing["tempo"].as_f64().unwrap() / PPQ as f64;
        let elapsed = timing["captureAudioTimeSeconds"].as_f64().unwrap()
            - timing["songStartAudioTimeSeconds"].as_f64().unwrap();
        let duration = audio["durationSeconds"].as_f64().unwrap();
        let source_in = (-elapsed).max(0.0);
        let start = (elapsed / seconds_per_tick).round().max(0.0);
        let status = if duration - source_in < 0.001 {
            "ended-before-song"
        } else if start + (duration - source_in) / seconds_per_tick > (128 * 3840) as f64 {
            "outside-timeline"
        } else {
            "estimated"
        };
        require(
            timing["status"] == status,
            "Invalid legacy recording placement status",
        )?;
    }
    Ok(())
}

impl ProjectDocument {
    pub fn new(name: &str) -> Result<Self> {
        Self::from_value(json!({
            "schemaVersion": 1, "id": uuid::Uuid::new_v4().to_string(), "revision": 0,
            "ppq": PPQ, "name": name, "tempo": 104, "bars": 4, "key": "C minor", "lyrics": "",
            "tracks": [
                {"id":uuid::Uuid::new_v4().to_string(),"name":"Your piano","kind":"notes","instrument":"keys","gain":0.65,"muted":false,"locked":false,"armed":false,"notes":[]},
                {"id":uuid::Uuid::new_v4().to_string(),"name":"Pocket drums","kind":"notes","instrument":"drums","gain":0.65,"muted":false,"locked":false,"armed":false,"notes":[]}
            ], "history": []
        }))
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        require(
            bytes.len() <= MAX_NATIVE_PROJECT_BYTES,
            "Project exceeds native metadata size limit",
        )?;
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|e| DocumentError(format!("Invalid project JSON: {e}")))?;
        if value["schemaVersion"] == 1 {
            require(
                bytes.len() <= MAX_PROJECT_BYTES,
                "Project exceeds schema-1 size limit",
            )?;
        }
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self> {
        let document = Self { value };
        document.validate()?;
        document.to_json()?;
        Ok(document)
    }

    pub fn to_json(&self) -> Result<Vec<u8>> {
        let bytes =
            serde_json::to_vec_pretty(&self.value).map_err(|e| DocumentError(e.to_string()))?;
        require(
            bytes.len() <= self.metadata_limit(),
            "Project exceeds its schema metadata size limit",
        )?;
        Ok(bytes)
    }

    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn schema_version(&self) -> u64 {
        self.value["schemaVersion"]
            .as_u64()
            .expect("validated schema version")
    }
    pub fn metadata_limit(&self) -> usize {
        if self.supports_native_takes() {
            MAX_NATIVE_PROJECT_BYTES
        } else {
            MAX_PROJECT_BYTES
        }
    }
    pub fn id(&self) -> &str {
        self.value["id"].as_str().expect("validated identity")
    }
    pub fn name(&self) -> &str {
        self.value["name"].as_str().expect("validated name")
    }
    pub fn revision(&self) -> u64 {
        self.value["revision"].as_u64().expect("validated revision")
    }
    pub fn tempo(&self) -> f64 {
        self.value["tempo"].as_f64().expect("validated tempo")
    }
    pub fn bars(&self) -> u64 {
        self.value["bars"].as_u64().expect("validated bars")
    }
    pub fn tracks(&self) -> &[Value] {
        self.value["tracks"].as_array().expect("validated tracks")
    }

    pub fn rename(&mut self, name: &str) -> Result<()> {
        text(&Value::String(name.into()), 1, 160, "project name")?;
        require(!name.trim().is_empty(), "Project needs a name")?;
        self.value["name"] = name.into();
        Ok(())
    }

    pub fn set_tempo(&mut self, tempo: f64) -> Result<()> {
        require(
            tempo.is_finite() && (30.0..=300.0).contains(&tempo),
            "Tempo must be 30–300 BPM",
        )?;
        self.value["tempo"] = json!(tempo);
        Ok(())
    }

    pub fn with_revision(&self, revision: u64) -> Result<Self> {
        let mut value = self.value.clone();
        value["revision"] = revision.into();
        Self::from_value(value)
    }

    /// Creates an independent native session; source musical fields remain exact.
    pub fn imported_copy(&self, source_sha256: &str) -> Result<Self> {
        let mut value = self.value.clone();
        let mut receipt = json!({"version":1,"sourceId":self.id(),"sourceRevision":self.revision(),"sourceSha256":source_sha256});
        if let Some(previous) = self.value.get("nativeImport") {
            receipt["previousImport"] = previous.clone();
        }
        value["nativeImport"] = receipt;
        value["id"] = uuid::Uuid::new_v4().to_string().into();
        value["revision"] = 0.into();
        Self::from_value(value)
    }

    pub fn referenced_assets(&self) -> Result<BTreeSet<String>> {
        fn collect_known(value: &Value, refs: &mut BTreeSet<String>) -> Result<()> {
            for key in ["assetId", "sourceAssetId"] {
                if let Some(value) = value.get(key).filter(|v| !v.is_null() && *v != "") {
                    let id = value
                        .as_str()
                        .filter(|id| valid_asset_id(id))
                        .ok_or_else(|| {
                            DocumentError(format!("Invalid audio reference in {key}"))
                        })?;
                    refs.insert(id.to_owned());
                }
            }
            Ok(())
        }
        let mut refs = BTreeSet::new();
        // Schema-owned reference paths only: opaque provider identifiers are not
        // local asset IDs just because a foreign key happens to share the name.
        for track in self.tracks() {
            if matches!(track["kind"].as_str(), Some("notes" | "audio")) {
                collect_known(track, &mut refs)?;
                collect_known(&track["provenance"]["input"], &mut refs)?;
            }
        }
        for entry in self.value["history"].as_array().into_iter().flatten() {
            collect_known(&entry["provenance"]["input"], &mut refs)?;
        }
        require(refs.len() <= 128, "Too many schema-1 audio assets")?;
        Ok(refs)
    }

    fn validate(&self) -> Result<()> {
        let p = &self.value;
        require(
            matches!(p["schemaVersion"].as_u64(), Some(1..=3)),
            "Unsupported project schema; source was not changed",
        )?;
        let native = self.supports_native_takes();
        text(&p["id"], 1, 80, "project identity")?;
        require(
            p["id"]
                .as_str()
                .is_some_and(|s| s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')),
            "Invalid project identity",
        )?;
        text(&p["name"], 1, 160, "project name")?;
        require(
            !p["name"].as_str().unwrap_or_default().trim().is_empty(),
            "Project needs a name",
        )?;
        integer(&p["revision"], 0, 1_000_000_000, "project revision")?;
        require(p["ppq"] == PPQ, "Unsupported project time resolution")?;
        finite(&p["tempo"], 30.0, 300.0, "tempo")?;
        integer(
            &p["bars"],
            1,
            if native { native::MAX_NATIVE_BARS } else { 128 },
            "project bars",
        )?;
        if let Some(transport) = p.get("transport") {
            require(
                transport["version"] == 1 && transport["metronome"].is_boolean(),
                "Invalid transport settings",
            )?;
            integer(&transport["countInBars"], 0, 2, "count-in bars")?;
            finite(&transport["clickLevel"], 0.0, 1.0, "click level")?;
        }
        text(&p["lyrics"], 0, 50_000, "lyrics")?;
        let tracks = p["tracks"]
            .as_array()
            .filter(|t| t.len() <= 64)
            .ok_or_else(|| DocumentError("Invalid tracks".into()))?;
        let mut track_ids = BTreeSet::new();
        for track in tracks {
            text(&track["id"], 0, 200, "track identity")?;
            require(
                track_ids.insert(track["id"].as_str().unwrap_or_default()),
                "Duplicate track identity",
            )?;
            text(&track["name"], 0, 160, "track name")?;
            let recording = native && track["kind"] == "recording";
            require(
                recording || matches!(track["kind"].as_str(), Some("notes" | "audio")),
                "Unknown track kind",
            )?;
            finite(&track["gain"], 0.0, 2.0, "track gain")?;
            require(
                track["muted"].is_boolean() && track["locked"].is_boolean(),
                "Invalid track switches",
            )?;
            if let Some(armed) = track.get("armed") {
                require(armed.is_boolean(), "Invalid recording arm")?;
            }
            if recording {
                // The versioned recording payload is checked below. Legacy-
                // looking extension fields on this new kind remain opaque.
                continue;
            }
            require(
                matches!(
                    track["instrument"].as_str(),
                    Some("keys" | "bass" | "drums" | "pad" | "sample" | "voice")
                ),
                "Unknown schema-1 instrument",
            )?;
            let notes = track["notes"]
                .as_array()
                .filter(|n| n.len() <= 20_000)
                .ok_or_else(|| DocumentError("Invalid track notes".into()))?;
            let mut note_ids = BTreeSet::new();
            for note in notes {
                validate_note(note, false)?;
                require(
                    note_ids.insert(note["id"].as_str().unwrap_or_default()),
                    "Duplicate note identity",
                )?;
            }
            if let Some(audio) = track.get("audio") {
                validate_audio(audio)?;
            }
        }
        if native {
            native::validate_recording_tracks(tracks)?;
        }
        performance_projection::validate_document_registry(self)?;
        require(
            p["history"].as_array().is_some_and(|h| h.len() <= 5000),
            "Invalid project history",
        )?;
        if let Some(capture) = p.get("inputCapture") {
            require(capture["version"] == 1, "Unsupported raw capture version")?;
            integer(
                &capture["droppedEvents"],
                0,
                1_000_000_000,
                "dropped input count",
            )?;
            let events = capture["events"]
                .as_array()
                .filter(|e| e.len() <= 4096)
                .ok_or_else(|| DocumentError("Invalid raw capture".into()))?;
            for event in events {
                validate_source(event)?;
                require(
                    event["trackId"]
                        .as_str()
                        .is_some_and(|s| track_ids.contains(s)),
                    "Invalid captured track",
                )?;
                integer(&event["tick"], 0, 1_000_000_000, "capture tick")?;
                finite(&event["receivedTimeMs"], 0.0, 1e15, "capture receipt time")?;
                if let Some(host) = event.get("hostTime") {
                    require(
                        host.as_str().is_some_and(|s| {
                            !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
                        }),
                        "Native timestamp must stay a decimal string",
                    )?;
                }
                if event["format"] == "ump" {
                    let words = event["words"]
                        .as_array()
                        .filter(|w| !w.is_empty() && w.len() <= 4)
                        .ok_or_else(|| DocumentError("Invalid UMP capture".into()))?;
                    for word in words {
                        integer(word, 0, u32::MAX.into(), "UMP word")?;
                    }
                    let counts = [1, 1, 1, 2, 2, 4, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4];
                    require(
                        words.len() == counts[(words[0].as_u64().unwrap() >> 28) as usize],
                        "Truncated UMP capture",
                    )?;
                } else {
                    require(event["format"] == "midi1", "Unknown raw input format")?;
                    let bytes = event["bytes"]
                        .as_array()
                        .filter(|b| !b.is_empty() && b.len() <= 1024)
                        .ok_or_else(|| DocumentError("Invalid MIDI 1 capture".into()))?;
                    for byte in bytes {
                        integer(byte, 0, 255, "MIDI byte")?;
                    }
                }
            }
        }
        self.referenced_assets()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_opaque_metadata_raw_resolution_and_large_clock_strings() {
        let mut value = ProjectDocument::new("Expressive source").unwrap().value;
        value["vendor:unknown"] =
            json!({"unfamiliar":{"model":"retained","counter":18_446_744_073_709_551_615u64}});
        let track_id = value["tracks"][0]["id"].clone();
        value["inputCapture"] = json!({"version":1,"droppedEvents":0,"events":[{"trackId":track_id,"tick":0,"sourceId":"controller","sourceName":"MIDI 2 keys","receivedTimeMs":1,"hostTime":"18446744073709551615","format":"ump","words":[1083194368u32,4294967295u32]}]});
        value["tracks"][0]["notes"] = json!([{"id":"voice-a","pitch":60,"startTick":9,"durationTicks":960,"velocity16":65535,"expression":[{"type":"pitch","points":[{"tick":0,"value32":2147483648u32},{"tick":960,"value32":4294967295u32}]}],"foreign:metadata":{"confidence":0.75}}]);
        let document = ProjectDocument::from_value(value.clone()).unwrap();
        let reopened = ProjectDocument::from_json(&document.to_json().unwrap()).unwrap();
        assert_eq!(reopened.value(), &value);
        let copied = reopened.imported_copy("receipt").unwrap();
        assert_ne!(copied.id(), reopened.id());
        assert_eq!(copied.tracks(), reopened.tracks());
        assert_eq!(copied.value()["vendor:unknown"], value["vendor:unknown"]);
        assert_eq!(copied.value()["inputCapture"], value["inputCapture"]);
    }

    #[test]
    fn rejects_invalid_expression_and_unsupported_versions_without_mutation() {
        let original = ProjectDocument::new("Source").unwrap();
        let mut invalid = original.value().clone();
        invalid["schemaVersion"] = 3.into();
        assert!(ProjectDocument::from_value(invalid).is_err());
        let mut invalid = original.value().clone();
        invalid["tracks"][0]["notes"] = json!([{"id":"a","pitch":60,"startTick":0,"durationTicks":10,"velocity16":500,"expression":[{"type":"pressure","points":[{"tick":11,"value32":4294967295u32}]}]}]);
        assert!(ProjectDocument::from_value(invalid).is_err());
        let mut edited = original.clone();
        assert!(edited.set_tempo(f64::NAN).is_err());
        assert!(edited.rename(" ").is_err());
        assert_eq!(edited, original);
    }

    #[test]
    fn discovers_provenance_assets_and_rejects_path_references() {
        let mut value = ProjectDocument::new("Assets").unwrap().value;
        let id = format!("{}.wav", "a".repeat(64));
        value["history"] = json!([{"provenance":{"input":{"sourceAssetId":id}}}]);
        let document = ProjectDocument::from_value(value.clone()).unwrap();
        assert_eq!(document.referenced_assets().unwrap(), BTreeSet::from([id]));
        value["history"][0]["provenance"]["input"]["sourceAssetId"] = "../recording.wav".into();
        assert!(ProjectDocument::from_value(value).is_err());
    }

    #[test]
    fn unknown_numbers_are_not_rounded_to_machine_float_or_integer() {
        let mut value = ProjectDocument::new("Exact external metadata")
            .unwrap()
            .value;
        value["vendor:numbers"] = serde_json::from_str(r#"{"counter":18446744073709551616000001,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
        let document = ProjectDocument::from_value(value).unwrap();
        let reopened = ProjectDocument::from_json(&document.to_json().unwrap()).unwrap();
        assert_eq!(
            reopened.value()["vendor:numbers"]["counter"].to_string(),
            "18446744073709551616000001"
        );
        assert_eq!(
            reopened.value()["vendor:numbers"]["decimal"].to_string(),
            "0.123456789012345678901234567890123456789"
        );
    }

    #[test]
    fn opaque_remote_ids_and_prior_import_metadata_survive_migration() {
        let mut value = ProjectDocument::new("Opaque provider data").unwrap().value;
        value["vendor:remote"] = json!({"assetId":"provider-remote-123","sourceAssetId":17,"looksLocal":format!("{}.wav","b".repeat(64))});
        value["nativeImport"] = json!({"foreign":"keep me","sourceId":"older-session"});
        let source = ProjectDocument::from_value(value).unwrap();
        assert!(source.referenced_assets().unwrap().is_empty());
        let imported = source.imported_copy("receipt").unwrap();
        assert_eq!(
            imported.value()["vendor:remote"],
            source.value()["vendor:remote"]
        );
        assert_eq!(
            imported.value()["nativeImport"]["previousImport"],
            source.value()["nativeImport"]
        );
    }

    #[test]
    fn clip_windows_original_snapshots_and_transport_are_validated() {
        let mut value = ProjectDocument::new("Clip").unwrap().value;
        value["tracks"][0]["audio"] = json!({"origin":"import","mimeType":"audio/wav","durationSeconds":2.0,"byteLength":100,"trimStartSeconds":0.5,"trimEndSeconds":1.5,"fadeInSeconds":0.2,"fadeOutSeconds":0.4,"editSource":{"version":1,"original":{}}});
        assert!(ProjectDocument::from_value(value.clone()).is_ok());
        let mut invalid = value.clone();
        invalid["tracks"][0]["audio"]["trimEndSeconds"] = 3.into();
        assert!(ProjectDocument::from_value(invalid).is_err());
        let mut invalid = value.clone();
        invalid["tracks"][0]["audio"]["fadeInSeconds"] = 1.into();
        assert!(ProjectDocument::from_value(invalid).is_err());
        let mut invalid = value.clone();
        invalid["tracks"][0]["audio"]["editSource"]["version"] = 9.into();
        assert!(ProjectDocument::from_value(invalid).is_err());
        value["transport"] = json!({"version":1,"metronome":true,"countInBars":3,"clickLevel":0.5});
        assert!(ProjectDocument::from_value(value).is_err());
    }

    #[test]
    fn legacy_recording_clock_estimates_are_preserved_and_validated() {
        let mut value = ProjectDocument::new("Legacy timing").unwrap().value;
        value["tracks"][0]["audio"] = json!({"origin":"microphone","mimeType":"audio/webm","durationSeconds":2.0,"byteLength":100,"recordingTiming":{"version":1,"method":"browser-recorder-start-call","calibration":"unmeasured","captureAudioTimeSeconds":8.0,"songStartAudioTimeSeconds":9.0,"tempo":120,"ppq":960,"loopTicks":15360,"status":"estimated"}});
        assert!(ProjectDocument::from_value(value.clone()).is_ok());
        value["tracks"][0]["audio"]["recordingTiming"]["status"] = "calibrated".into();
        assert!(ProjectDocument::from_value(value).is_err());
    }
}
