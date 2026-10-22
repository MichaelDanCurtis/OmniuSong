//! Schema-3 ownership of an original procedural performance interpretation.
//!
//! The retained review and bindings describe the initial interpretation. Current
//! editable notes may change or disappear. These shape/ownership checks do not
//! authenticate a take, recompute a projection, or verify a review digest.

use crate::contracts::{NoteId, Validate};
use crate::native::TakeReference;
use crate::{DocumentError, NoteValues, ProjectDocument, Result, require};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const PROJECTION_SCHEMA_VERSION: u64 = 3;
pub const MAX_PERFORMANCE_PROJECTION_ENTRIES: usize = 64;
pub const MAX_PROJECTION_LANES: usize = 16;
pub const MAX_ORIGINAL_PROJECTION_NOTES: usize = 20_000;
pub const MAX_RETAINED_PROJECTION_REVIEW_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionInstrument {
    Keys,
    Bass,
    Drums,
    Pad,
}
impl ProjectionInstrument {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keys => "keys",
            Self::Bass => "bass",
            Self::Drums => "drums",
            Self::Pad => "pad",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionNoteBinding {
    pub note_id: NoteId,
    pub identity_sha256: String,
    pub values: NoteValues,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionLaneBinding {
    pub source: u8,
    pub track_id: String,
    pub instrument: ProjectionInstrument,
    pub original_notes: Vec<ProjectionNoteBinding>,
}

/// Exact retained UTF-8 bytes. The shared performance layer owns canonical
/// encoding, all interpretation semantics and digest/source authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedProjectionReview {
    pub version: u32,
    pub canonical_json: String,
    pub review_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PerformanceProjectionBinding {
    pub id: String,
    pub version: u32,
    pub source: TakeReference,
    pub review: RetainedProjectionReview,
    pub lanes: Vec<ProjectionLaneBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PerformanceProjectionRegistry {
    pub version: u32,
    pub entries: Vec<PerformanceProjectionBinding>,
}

fn native_identifier(value: &str, label: &str) -> Result<()> {
    require(
        !value.is_empty()
            && value.len() <= 96
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)),
        format!("Invalid performance projection {label}"),
    )
}
fn digest_shape(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn contract(error: crate::contracts::ContractError) -> DocumentError {
    DocumentError(error.to_string())
}

// Intentionally thin: parsing the full note arrays here would duplicate the
// shared interpreter and confer no source authority. All other embedded fields
// remain verbatim in canonical_json for that layer's strict validation.
#[derive(Deserialize)]
struct ReviewEnvelope {
    version: u32,
    review_sha256: String,
    request: ReviewRequest,
}
#[derive(Deserialize)]
struct ReviewRequest {
    source: ReviewSource,
}
#[derive(Deserialize)]
struct ReviewSource {
    take_id: String,
    manifest_sha256: String,
}

impl RetainedProjectionReview {
    fn validate_for(&self, source: &TakeReference) -> Result<()> {
        require(
            self.version == 1,
            "Unsupported retained performance projection review version",
        )?;
        require(
            !self.canonical_json.is_empty()
                && self.canonical_json.len() <= MAX_RETAINED_PROJECTION_REVIEW_BYTES,
            "Retained performance projection review exceeds its UTF-8 byte limit",
        )?;
        require(
            digest_shape(&self.review_sha256),
            "Invalid retained performance projection review digest",
        )?;
        let envelope: ReviewEnvelope =
            serde_json::from_str(&self.canonical_json).map_err(|error| {
                DocumentError(format!(
                    "Invalid retained performance projection review JSON: {error}"
                ))
            })?;
        require(
            envelope.version == 1 && envelope.review_sha256 == self.review_sha256,
            "Retained performance projection review version or embedded digest differs",
        )?;
        require(
            envelope.request.source.take_id == source.take_id.0
                && envelope.request.source.manifest_sha256 == source.manifest_sha256,
            "Retained performance projection review source differs from its owned take reference",
        )
    }
}

impl PerformanceProjectionBinding {
    /// Structural binding checks only; never source or review-hash authority.
    pub fn validate(&self) -> Result<()> {
        require(
            self.version == 1,
            "Unsupported performance projection binding version",
        )?;
        native_identifier(&self.id, "identity")?;
        self.source.validate()?;
        require(
            self.source.stream_id == "audio",
            "Performance projection ownership requires the original audio stream",
        )?;
        require(
            !self.lanes.is_empty() && self.lanes.len() <= MAX_PROJECTION_LANES,
            "Performance projection requires one through sixteen nonempty original lanes",
        )?;
        let mut lane_keys = BTreeSet::new();
        let mut tracks = BTreeSet::new();
        let mut notes = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut count = 0usize;
        for lane in &self.lanes {
            require(
                (1..=4).contains(&lane.source),
                "Invalid performance projection source handle",
            )?;
            native_identifier(&lane.track_id, "lane track identity")?;
            require(
                lane_keys.insert((lane.source, lane.instrument)),
                "Duplicate original source/instrument lane",
            )?;
            require(
                tracks.insert(lane.track_id.as_str()),
                "Duplicate reserved performance projection track identity",
            )?;
            require(
                !lane.original_notes.is_empty(),
                "An original performance projection lane cannot be empty",
            )?;
            count = count
                .checked_add(lane.original_notes.len())
                .ok_or_else(|| DocumentError("Original projection note count overflow".into()))?;
            require(
                count <= MAX_ORIGINAL_PROJECTION_NOTES,
                "Too many original performance projection note bindings",
            )?;
            for note in &lane.original_notes {
                note.note_id.validate().map_err(contract)?;
                note.values.validate().map_err(contract)?;
                require(
                    digest_shape(&note.identity_sha256),
                    "Invalid original performance voice identity digest",
                )?;
                require(
                    notes.insert(note.note_id.0.as_str()),
                    "Duplicate original performance projection note identity",
                )?;
                require(
                    identities.insert(note.identity_sha256.as_str()),
                    "Duplicate original voice identity within a performance projection",
                )?;
            }
        }
        self.review.validate_for(&self.source)
    }
}

impl PerformanceProjectionRegistry {
    /// Strict outer registry shape. Source authentication and immutable review
    /// recomputation remain a prerequisite in the shared performance layer.
    pub fn validate(&self) -> Result<()> {
        require(
            self.version == 1,
            "Unsupported performance projection registry version",
        )?;
        require(
            self.entries.len() <= MAX_PERFORMANCE_PROJECTION_ENTRIES,
            "Too many retained performance projections",
        )?;
        let mut ids = BTreeSet::new();
        let mut tracks = BTreeSet::new();
        let mut notes = BTreeSet::new();
        let mut count = 0usize;
        let mut review_bytes = 0usize;
        for entry in &self.entries {
            // Refuse aggregate oversize before parsing any later review. The
            // surrounding document's existing 8 MiB limit is stricter still.
            review_bytes = review_bytes
                .checked_add(entry.review.canonical_json.len())
                .ok_or_else(|| DocumentError("Retained projection byte count overflow".into()))?;
            require(
                review_bytes <= MAX_RETAINED_PROJECTION_REVIEW_BYTES,
                "Retained projection reviews exceed the document metadata byte limit",
            )?;
            require(
                !entry.lanes.is_empty() && entry.lanes.len() <= MAX_PROJECTION_LANES,
                "Invalid original performance projection lane count",
            )?;
            for lane in &entry.lanes {
                count = count
                    .checked_add(lane.original_notes.len())
                    .ok_or_else(|| {
                        DocumentError("Registry projection note count overflow".into())
                    })?;
                require(
                    count <= MAX_ORIGINAL_PROJECTION_NOTES,
                    "Too many original note bindings across performance projections",
                )?;
            }
            // Establish bounded lexical identities before inserting them into
            // cross-entry comparison sets.
            entry.validate()?;
            require(
                ids.insert(entry.id.as_str()),
                "Duplicate performance projection identity",
            )?;
            for lane in &entry.lanes {
                require(
                    tracks.insert(lane.track_id.as_str()),
                    "Reserved projection track identity is reused across the registry",
                )?;
                for note in &lane.original_notes {
                    require(
                        notes.insert(note.note_id.0.as_str()),
                        "Reserved projection note identity is reused across the registry",
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Bound the owned arrays/string slots before cloning untyped document JSON
/// into its strict DTO. Existing schema-1/2 values never reach this function.
fn registry_from_value(value: &Value) -> Result<PerformanceProjectionRegistry> {
    fn text_slot<'a>(value: &'a Value, key: &str, maximum: usize) -> Result<&'a str> {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| text.len() <= maximum)
            .ok_or_else(|| {
                DocumentError(format!("Invalid or oversized performance projection {key}"))
            })
    }
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .filter(|entries| entries.len() <= MAX_PERFORMANCE_PROJECTION_ENTRIES)
        .ok_or_else(|| {
            DocumentError("Missing or oversized performance projection registry entries".into())
        })?;
    let mut notes = 0usize;
    let mut review_bytes = 0usize;
    for entry in entries {
        text_slot(entry, "id", 96)?;
        let source = &entry["source"];
        text_slot(source, "takeId", 96)?;
        text_slot(source, "manifestSha256", 64)?;
        require(
            text_slot(source, "streamId", 5)? == "audio",
            "Performance projection ownership requires the original audio stream",
        )?;
        text_slot(&entry["review"], "reviewSha256", 64)?;
        let lanes = entry
            .get("lanes")
            .and_then(Value::as_array)
            .filter(|lanes| !lanes.is_empty() && lanes.len() <= MAX_PROJECTION_LANES)
            .ok_or_else(|| DocumentError("Invalid original performance projection lanes".into()))?;
        for lane in lanes {
            text_slot(lane, "trackId", 96)?;
            let original = lane
                .get("originalNotes")
                .and_then(Value::as_array)
                .filter(|notes| !notes.is_empty() && notes.len() <= MAX_ORIGINAL_PROJECTION_NOTES)
                .ok_or_else(|| {
                    DocumentError("Invalid original performance projection note bindings".into())
                })?;
            notes = notes
                .checked_add(original.len())
                .ok_or_else(|| DocumentError("Original projection note count overflow".into()))?;
            require(
                notes <= MAX_ORIGINAL_PROJECTION_NOTES,
                "Too many original note bindings across performance projections",
            )?;
            for note in original {
                text_slot(note, "noteId", 96)?;
                text_slot(note, "identitySha256", 64)?;
            }
        }
        let json = entry
            .get("review")
            .and_then(|review| review.get("canonicalJson"))
            .and_then(Value::as_str)
            .filter(|json| !json.is_empty() && json.len() <= MAX_RETAINED_PROJECTION_REVIEW_BYTES)
            .ok_or_else(|| {
                DocumentError("Invalid retained performance projection review byte length".into())
            })?;
        review_bytes = review_bytes
            .checked_add(json.len())
            .ok_or_else(|| DocumentError("Retained projection byte count overflow".into()))?;
        require(
            review_bytes <= MAX_RETAINED_PROJECTION_REVIEW_BYTES,
            "Retained projection reviews exceed the document metadata byte limit",
        )?;
    }
    let registry = PerformanceProjectionRegistry::deserialize(value).map_err(|error| {
        DocumentError(format!("Invalid performance projection registry: {error}"))
    })?;
    registry.validate()?;
    Ok(registry)
}

pub(crate) fn validate_document_registry(document: &ProjectDocument) -> Result<()> {
    let Some(registry) = document.performance_projections()? else {
        return Ok(());
    };
    let mut tracks = BTreeSet::new();
    let mut notes = BTreeMap::new();
    for entry in &registry.entries {
        for lane in &entry.lanes {
            tracks.insert(lane.track_id.as_str());
            for note in &lane.original_notes {
                notes.insert(note.note_id.0.as_str(), lane.track_id.as_str());
            }
        }
    }
    for track in document.tracks() {
        let track_id = track["id"]
            .as_str()
            .ok_or_else(|| DocumentError("Invalid current track identity".into()))?;
        if tracks.contains(track_id) {
            require(
                track["kind"] == "notes",
                "A retained projection lane identity belongs to a notes track",
            )?;
        }
        // Recording tracks keep legacy-looking extension data opaque, just as
        // the existing native format does. Notes/audio arrays are schema-owned.
        if matches!(track["kind"].as_str(), Some("notes" | "audio")) {
            let current = track["notes"]
                .as_array()
                .ok_or_else(|| DocumentError("Invalid current track notes".into()))?;
            for note in current {
                if let Some(owner) = note["id"].as_str().and_then(|id| notes.get(id)) {
                    require(
                        *owner == track_id,
                        "An original projected note identity cannot move to another lane",
                    )?;
                }
            }
        }
    }
    Ok(())
}

impl ProjectDocument {
    /// Schema-1/2 keys remain opaque even when they resemble this registry.
    pub fn performance_projections(&self) -> Result<Option<PerformanceProjectionRegistry>> {
        match self.schema_version() {
            1 | 2 => Ok(None),
            PROJECTION_SCHEMA_VERSION => {
                let value = self.value().get("performanceProjections").ok_or_else(|| {
                    DocumentError(
                        "Schema 3 requires an explicit performance projection registry".into(),
                    )
                })?;
                registry_from_value(value).map(Some)
            }
            _ => Err(DocumentError("Unsupported musical document schema".into())),
        }
    }

    pub fn projection_source_references(&self) -> Result<BTreeSet<TakeReference>> {
        Ok(self
            .performance_projections()?
            .into_iter()
            .flat_map(|registry| registry.entries)
            .map(|entry| entry.source)
            .collect())
    }

    /// Reservations survive deletion of the editable note or its whole lane.
    pub fn projection_note_id_reserved(&self, note_id: &str) -> Result<bool> {
        Ok(self.performance_projections()?.is_some_and(|registry| {
            registry.entries.iter().any(|entry| {
                entry.lanes.iter().any(|lane| {
                    lane.original_notes
                        .iter()
                        .any(|note| note.note_id.0 == note_id)
                })
            })
        }))
    }

    pub fn projection_track_id_reserved(&self, track_id: &str) -> Result<bool> {
        Ok(self.performance_projections()?.is_some_and(|registry| {
            registry
                .entries
                .iter()
                .any(|entry| entry.lanes.iter().any(|lane| lane.track_id == track_id))
        }))
    }

    /// Explicit format promotion only. No entry, note, take, ID, revision or
    /// history row is minted, and no opaque registry key is overwritten.
    pub fn upgrade_performance_projection_schema(&self) -> Result<Self> {
        require(
            matches!(self.schema_version(), 1 | 2),
            "Projection schema upgrade requires a schema-1 or schema-2 document",
        )?;
        require(
            self.value().get("performanceProjections").is_none(),
            "An existing opaque performanceProjections key prevents explicit schema promotion",
        )?;
        let mut value = self.value().clone();
        value["schemaVersion"] = PROJECTION_SCHEMA_VERSION.into();
        value["performanceProjections"] = serde_json::json!({"version":1,"entries":[]});
        Self::from_value(value)
    }
}

#[cfg(test)]
mod tests;
