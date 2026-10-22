//! Bounded publication intentions and catalog-commit receipts.
//!
//! These DTOs contain neither a document nor a caller-supplied note/review blob.
//! A valid receipt records a catalog commitment; it grants no playable-document
//! or source-authentication authority.
use crate::{Result, StoreError};
use omniusong_core::{
    NoteTimelinePolicy,
    contracts::{OperationId, ProjectId, Validate},
    native::TakeReference,
    performance_projection::ProjectionLaneChoice,
};
use omniusong_performance::projection::PerformanceProjectionRequest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Write};

pub const PROJECTION_OPERATION_PREFIX: &str = "os-perf-proj:";
const OPERATION_DOMAIN: &[u8] = b"OmniuSong.performance-projection-operation.v1\0";
const MAX_PUBLICATION_BYTES: usize = 65_536;
const MAX_RECEIPT_BYTES: usize = 8_192;

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(StoreError::Invalid(message.into()))
    }
}
fn native_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}
fn lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn previous_revision(previous: u64) -> Result<u64> {
    previous
        .checked_add(1)
        .filter(|committed| *committed <= i64::MAX as u64)
        .ok_or_else(|| {
            StoreError::Invalid(
                "Projection publication revision exceeds SQLite's signed revision range".into(),
            )
        })
}

/// One stable operation identity for this project and projection. Framed exact
/// bytes prevent ambiguous concatenation; no source or document is inspected.
pub fn projection_operation_id(project_id: &ProjectId, projection_id: &str) -> Result<OperationId> {
    project_id
        .validate()
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
    require(
        native_identifier(projection_id),
        "Invalid performance projection identity",
    )?;
    let mut digest = Sha256::new();
    digest.update(OPERATION_DOMAIN);
    for field in [project_id.0.as_bytes(), projection_id.as_bytes()] {
        digest.update((field.len() as u64).to_le_bytes());
        digest.update(field);
    }
    Ok(OperationId(format!(
        "{PROJECTION_OPERATION_PREFIX}{:x}",
        digest.finalize()
    )))
}

/// Generic mutation/archive entry points cannot manufacture this namespace's
/// commitment or replay semantics. Its dedicated publisher owns admission.
pub(crate) fn reject_reserved_operation(id: &str) -> Result<()> {
    require(
        !id.starts_with(PROJECTION_OPERATION_PREFIX),
        "Performance projection operation requires its dedicated authenticated publication path",
    )
}

struct BoundedJson {
    bytes: Vec<u8>,
    maximum: usize,
}
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "Projection DTO exceeds its encoded byte limit",
            ));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| std::io::Error::other("Cannot reserve bounded projection DTO bytes"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encoded_json(value: &impl Serialize, maximum: usize) -> Result<String> {
    let mut writer = BoundedJson {
        bytes: Vec::new(),
        maximum,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
    String::from_utf8(writer.bytes).map_err(|error| StoreError::Invalid(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionPublication {
    pub version: u32,
    pub operation_id: OperationId,
    pub project_id: ProjectId,
    pub expected_revision: u64,
    pub projection_id: String,
    pub request: PerformanceProjectionRequest,
    pub expected_review_sha256: String,
    pub lanes: Vec<ProjectionLaneChoice>,
    pub timeline: NoteTimelinePolicy,
    pub allow_projection_schema_upgrade: bool,
}
impl ProjectionPublication {
    fn validate_shape(&self) -> Result<()> {
        require(
            self.version == 1,
            "Unsupported performance projection publication version",
        )?;
        let operation = projection_operation_id(&self.project_id, &self.projection_id)?;
        require(
            self.operation_id == operation,
            "Projection publication operation identity differs from its project and projection",
        )?;
        previous_revision(self.expected_revision)?;
        self.request.validate()?;
        require(
            lowercase_sha256(&self.expected_review_sha256),
            "Projection publication requires an exact lowercase review SHA-256",
        )?;
        require(
            !self.lanes.is_empty() && self.lanes.len() <= 16,
            "Projection publication requires one through sixteen nonempty lanes",
        )?;
        let mut destinations = BTreeSet::new();
        let mut previous = None;
        for lane in &self.lanes {
            require(
                (1..=4).contains(&lane.source),
                "Invalid projection publication source handle",
            )?;
            require(
                native_identifier(&lane.track_id),
                "Invalid native projection publication track identity",
            )?;
            require(
                destinations.insert(lane.track_id.as_str()),
                "Projection publication destination track identities must be unique",
            )?;
            // Check the UTF-8 byte length before scanning potentially large text.
            require(
                lane.track_name.len() <= 640,
                "Projection publication track name exceeds its UTF-8 byte limit",
            )?;
            require(
                lane.track_name.chars().count() <= 160
                    && !lane.track_name.trim().is_empty()
                    && !lane.track_name.chars().any(char::is_control),
                "Projection publication track names must be nonblank, control-free and at most 160 characters",
            )?;
            let key = (lane.source, lane.instrument);
            require(
                previous.is_none_or(|old| old < key),
                "Projection publication lanes must be sorted unique source/instrument groups",
            )?;
            previous = Some(key);
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_shape()?;
        encoded_json(self, MAX_PUBLICATION_BYTES).map(|_| ())
    }
    /// Exact compact typed encoding of the complete saved intention.
    pub fn canonical_json(&self) -> Result<String> {
        self.validate_shape()?;
        encoded_json(self, MAX_PUBLICATION_BYTES)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionCommitReceipt {
    pub version: u32,
    pub operation_id: OperationId,
    pub project_id: ProjectId,
    pub projection_id: String,
    pub previous_revision: u64,
    pub committed_revision: u64,
    pub request_sha256: String,
    pub result_sha256: String,
    pub review_sha256: String,
    pub source: TakeReference,
    pub lane_count: u32,
    pub note_count: u32,
    pub replayed: bool,
}
impl ProjectionCommitReceipt {
    pub fn validate(&self) -> Result<()> {
        require(
            self.version == 1,
            "Unsupported performance projection commitment version",
        )?;
        let operation = projection_operation_id(&self.project_id, &self.projection_id)?;
        require(
            self.operation_id == operation,
            "Projection commitment operation identity differs from its project and projection",
        )?;
        require(
            previous_revision(self.previous_revision)? == self.committed_revision,
            "Projection commitment revisions are not adjacent",
        )?;
        require(
            [
                &self.request_sha256,
                &self.result_sha256,
                &self.review_sha256,
            ]
            .into_iter()
            .all(|value| lowercase_sha256(value)),
            "Projection commitment requires exact lowercase SHA-256 identities",
        )?;
        self.source.validate()?;
        require(
            self.source.stream_id == "audio",
            "Projection commitment source must name the original audio stream",
        )?;
        require(
            (1..=16).contains(&self.lane_count)
                && (1..=20_000).contains(&self.note_count)
                && self.note_count >= self.lane_count,
            "Projection commitment lane or note counts are out of range",
        )?;
        encoded_json(self, MAX_RECEIPT_BYTES).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::{contracts::TakeId, performance_projection::ProjectionInstrument};
    use omniusong_performance::projection::{
        PerformanceFrameRegion, PerformanceProjectionDuration, PerformanceProjectionPolicy,
        PerformanceProjectionRounding, PerformanceProjectionSourcePin,
    };
    use omniusong_recording::RecoveryPrefix;
    use serde_json::{Value, json};

    fn publication() -> ProjectionPublication {
        let project_id = ProjectId("project-one".into());
        let projection_id = "projection-one".to_owned();
        ProjectionPublication {
            version: 1,
            operation_id: projection_operation_id(&project_id, &projection_id).unwrap(),
            project_id,
            expected_revision: 7,
            projection_id,
            request: PerformanceProjectionRequest {
                version: 1,
                source: PerformanceProjectionSourcePin {
                    take_id: "synthetic-not-an-authentication-token".into(),
                    manifest_sha256: "a".repeat(64),
                    descriptor_sha256: "b".repeat(64),
                    original_rows_sha256: "c".repeat(64),
                    journal_sha256_at_review: "d".repeat(64),
                    checkpoint_sha256: "e".repeat(64),
                    selection: RecoveryPrefix {
                        chunk_count: 1,
                        chunk_index_sha256: "f".repeat(64),
                    },
                },
                region: PerformanceFrameRegion { start: 0, end: 80 },
                sources: vec![1],
                tempo_bits: 120.0f64.to_bits(),
                destination_start_tick: 0,
                rounding: PerformanceProjectionRounding::NearestTiesUp,
                duration: PerformanceProjectionDuration::ReleaseGate,
                collapsed: PerformanceProjectionPolicy::Reject,
                incomplete: PerformanceProjectionPolicy::Reject,
                region_crossing: PerformanceProjectionPolicy::Reject,
            },
            expected_review_sha256: "1".repeat(64),
            lanes: vec![ProjectionLaneChoice {
                source: 1,
                instrument: ProjectionInstrument::Keys,
                track_id: "new-lane".into(),
                track_name: "Exact original interpretation".into(),
            }],
            timeline: NoteTimelinePolicy::PreserveLoop,
            allow_projection_schema_upgrade: true,
        }
    }
    fn receipt() -> ProjectionCommitReceipt {
        let request = publication();
        ProjectionCommitReceipt {
            version: 1,
            operation_id: request.operation_id,
            project_id: request.project_id,
            projection_id: request.projection_id,
            previous_revision: 7,
            committed_revision: 8,
            request_sha256: "2".repeat(64),
            result_sha256: "3".repeat(64),
            review_sha256: request.expected_review_sha256,
            source: TakeReference {
                take_id: TakeId(request.request.source.take_id),
                manifest_sha256: request.request.source.manifest_sha256,
                stream_id: "audio".into(),
            },
            lane_count: 1,
            note_count: 1,
            replayed: false,
        }
    }

    #[test]
    fn operation_identity_has_independent_length_framed_domain_oracle() {
        let mut bytes = b"OmniuSong.performance-projection-operation.v1\0".to_vec();
        bytes.extend_from_slice(&3u64.to_le_bytes());
        bytes.extend_from_slice(b"abc");
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(b"de");
        let expected = format!("os-perf-proj:{:x}", Sha256::digest(&bytes));
        assert_eq!(
            projection_operation_id(&ProjectId("abc".into()), "de")
                .unwrap()
                .0,
            expected
        );
        assert_ne!(
            projection_operation_id(&ProjectId("ab".into()), "cde")
                .unwrap()
                .0,
            expected
        );
        assert_ne!(
            projection_operation_id(&ProjectId("de".into()), "abc")
                .unwrap()
                .0,
            expected
        );
        assert_eq!(
            projection_operation_id(&ProjectId("abc".into()), "de")
                .unwrap()
                .0,
            expected
        );
        assert!(
            projection_operation_id(&ProjectId("project@legal".into()), "projection-_.:").is_ok()
        );
        for project in ["", "with space", "a\n", "😀"] {
            assert!(projection_operation_id(&ProjectId(project.into()), "valid").is_err());
        }
        for projection in ["", "projection@not-native", "with space", "a\0", "😀"] {
            assert!(projection_operation_id(&ProjectId("valid".into()), projection).is_err());
        }
        assert!(projection_operation_id(&ProjectId("p".repeat(97)), "valid").is_err());
        assert!(projection_operation_id(&ProjectId("valid".into()), &"p".repeat(97)).is_err());
    }

    #[test]
    fn publication_roundtrip_is_strict_snake_case_and_contains_only_an_intention() {
        let request = publication();
        request.validate().unwrap();
        let canonical = request.canonical_json().unwrap();
        assert_eq!(canonical, serde_json::to_string(&request).unwrap());
        assert!(canonical.len() <= MAX_PUBLICATION_BYTES);
        assert_eq!(
            serde_json::from_str::<ProjectionPublication>(&canonical).unwrap(),
            request
        );
        let value: Value = serde_json::from_str(&canonical).unwrap();
        assert!(value.get("operation_id").is_some() && value.get("operationId").is_none());
        assert!(value["lanes"][0].get("trackId").is_some());
        for field in [
            "notes",
            "review",
            "canonical_json",
            "path",
            "root",
            "document",
            "ui_context",
        ] {
            let mut extra = value.clone();
            extra[field] = json!({"caller":"claim"});
            assert!(serde_json::from_value::<ProjectionPublication>(extra).is_err());
        }
        let mut extra = value.clone();
        extra["request"]["review"] = true.into();
        assert!(serde_json::from_value::<ProjectionPublication>(extra).is_err());
        let mut missing = value;
        missing
            .as_object_mut()
            .unwrap()
            .remove("allow_projection_schema_upgrade");
        assert!(serde_json::from_value::<ProjectionPublication>(missing).is_err());
    }

    #[test]
    fn publication_rejects_versions_unbound_ids_hashes_and_revision_overflow() {
        let original = publication();
        let mut bad = original.clone();
        bad.version = 2;
        assert!(bad.validate().is_err());
        let mut bad = original.clone();
        bad.operation_id.0.push('0');
        assert!(bad.validate().is_err());
        let mut bad = original.clone();
        bad.project_id.0 = "different".into();
        assert!(bad.validate().is_err());
        let mut bad = original.clone();
        bad.projection_id = "different".into();
        assert!(bad.validate().is_err());
        for revision in [i64::MAX as u64, u64::MAX] {
            let mut bad = original.clone();
            bad.expected_revision = revision;
            assert!(bad.validate().is_err());
        }
        let mut edge = original.clone();
        edge.expected_revision = i64::MAX as u64 - 1;
        edge.validate().unwrap();
        for hash in [
            "A".repeat(64),
            "1".repeat(63),
            "g".repeat(64),
            String::new(),
        ] {
            let mut bad = original.clone();
            bad.expected_review_sha256 = hash;
            assert!(bad.validate().is_err());
        }
        let mut bad = original.clone();
        bad.request.version = 2;
        assert!(bad.validate().is_err());
        let mut bad = original;
        bad.request.sources = vec![2, 1];
        assert!(bad.validate().is_err());
    }

    #[test]
    fn lane_layout_and_names_keep_native_identity_and_byte_first_bounds() {
        let original = publication();
        let mut maximum = original.clone();
        maximum.lanes.clear();
        for source in 1..=4 {
            for (index, instrument) in [
                ProjectionInstrument::Keys,
                ProjectionInstrument::Bass,
                ProjectionInstrument::Drums,
                ProjectionInstrument::Pad,
            ]
            .into_iter()
            .enumerate()
            {
                maximum.lanes.push(ProjectionLaneChoice {
                    source,
                    instrument,
                    track_id: format!("lane-{source}-{index}"),
                    track_name: "😀".repeat(160),
                });
            }
        }
        maximum.validate().unwrap();
        assert!(maximum.canonical_json().unwrap().len() <= MAX_PUBLICATION_BYTES);
        let mut empty = original.clone();
        empty.lanes.clear();
        assert!(empty.validate().is_err());
        let mut excessive = maximum.clone();
        excessive.lanes.push(original.lanes[0].clone());
        assert!(excessive.validate().is_err());
        let mut unsorted = maximum.clone();
        unsorted.lanes.swap(0, 1);
        assert!(unsorted.validate().is_err());
        let mut repeated = maximum.clone();
        repeated.lanes[1] = repeated.lanes[0].clone();
        assert!(repeated.validate().is_err());
        let mut destination = maximum;
        destination.lanes[1].track_id = destination.lanes[0].track_id.clone();
        assert!(destination.validate().is_err());
        for id in ["", "native@not-allowed", "a\n", "é"] {
            let mut bad = original.clone();
            bad.lanes[0].track_id = id.into();
            assert!(bad.validate().is_err());
        }
        for name in [
            " ".to_owned(),
            "bad\nname".into(),
            "bad\0name".into(),
            "a".repeat(161),
            "😀".repeat(161),
            "a".repeat(1_000_000),
        ] {
            let mut bad = original.clone();
            bad.lanes[0].track_name = name;
            assert!(bad.canonical_json().is_err());
        }
        for source in [0, 5, u8::MAX] {
            let mut bad = original.clone();
            bad.lanes[0].source = source;
            assert!(bad.validate().is_err());
        }
    }

    #[test]
    fn commit_receipt_is_bounded_strict_catalog_history_not_source_authority() {
        let original = receipt();
        original.validate().unwrap();
        // The take/hash values are invented: shape acceptance opens no original
        // and must never be advertised as validating their source or a song.
        let bytes = serde_json::to_vec(&original).unwrap();
        assert!(bytes.len() <= MAX_RECEIPT_BYTES);
        assert_eq!(
            serde_json::from_slice::<ProjectionCommitReceipt>(&bytes).unwrap(),
            original
        );
        let mut replayed = original.clone();
        replayed.replayed = true;
        replayed.validate().unwrap();
        let mut bad = original.clone();
        bad.version = 2;
        assert!(bad.validate().is_err());
        let mut bad = original.clone();
        bad.operation_id.0 = "different".into();
        assert!(bad.validate().is_err());
        let mut bad = original.clone();
        bad.projection_id = "different".into();
        assert!(bad.validate().is_err());
        let mut bad = original.clone();
        bad.committed_revision += 1;
        assert!(bad.validate().is_err());
        let mut bad = original.clone();
        bad.previous_revision = i64::MAX as u64;
        bad.committed_revision = i64::MAX as u64 + 1;
        assert!(bad.validate().is_err());
        let mut edge = original.clone();
        edge.previous_revision = i64::MAX as u64 - 1;
        edge.committed_revision = i64::MAX as u64;
        edge.validate().unwrap();
        for counts in [(0, 1), (17, 17), (1, 0), (1, 20_001), (2, 1)] {
            let mut bad = original.clone();
            bad.lane_count = counts.0;
            bad.note_count = counts.1;
            assert!(bad.validate().is_err());
        }
        let mut maximum = original.clone();
        maximum.lane_count = 16;
        maximum.note_count = 20_000;
        maximum.validate().unwrap();
        for field in 0..3 {
            let mut bad = original.clone();
            match field {
                0 => bad.request_sha256 = "A".repeat(64),
                1 => bad.result_sha256 = "g".repeat(64),
                _ => bad.review_sha256 = "0".repeat(63),
            }
            assert!(bad.validate().is_err());
        }
        let mut bad = original.clone();
        bad.source.stream_id = "control".into();
        assert!(bad.validate().is_err());
        let mut bad = original.clone();
        bad.source.manifest_sha256 = "f".repeat(63);
        assert!(bad.validate().is_err());
        let value = serde_json::to_value(&original).unwrap();
        for field in [
            "document",
            "notes",
            "source_authenticated",
            "canonical_json",
            "path",
        ] {
            let mut extra = value.clone();
            extra[field] = true.into();
            assert!(serde_json::from_value::<ProjectionCommitReceipt>(extra).is_err());
        }
    }

    #[test]
    fn bounded_serialization_stops_at_encoded_bytes_including_json_escapes() {
        assert_eq!(encoded_json(&"a", 3).unwrap(), "\"a\"");
        assert!(encoded_json(&"a", 2).is_err());
        assert!(encoded_json(&"\n", 3).is_err());
        assert_eq!(encoded_json(&"\n", 4).unwrap(), "\"\\n\"");
        assert!(encoded_json(&"x".repeat(MAX_PUBLICATION_BYTES), MAX_PUBLICATION_BYTES).is_err());
        assert!(encoded_json(&"x".repeat(MAX_RECEIPT_BYTES), MAX_RECEIPT_BYTES).is_err());
    }

    #[test]
    fn generic_paths_refuse_the_entire_reserved_prefix_without_claiming_authentication() {
        assert!(reject_reserved_operation(&publication().operation_id.0).is_err());
        assert!(reject_reserved_operation(PROJECTION_OPERATION_PREFIX).is_err());
        assert!(reject_reserved_operation("os-perf-proj:malformed").is_err());
        assert!(reject_reserved_operation("ordinary-operation").is_ok());
        assert!(
            reject_reserved_operation("Os-perf-proj:case-is-not-the-reserved-namespace").is_ok()
        );
    }
}
