//! Canonical archive operation syntax and typed identity remapping only.
//! Parsing grants no source proof, publication, postimage or collision authority.
use crate::{ProjectionPublication, Result, StoreError, TakePublication, projection_operation_id};
use omniusong_core::contracts::{
    OperationId, ProjectEditIntent, ProjectId, Validate, WORKER_MAX_BYTES,
};
use serde::Deserialize;
use serde_json::Value;
use std::io::Write;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ArchiveOperation {
    Projection(ProjectionPublication),
    Edit(ProjectEditIntent),
    Take(TakePublication),
}

fn invalid(message: impl ToString) -> StoreError {
    StoreError::Invalid(message.to_string())
}
fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(invalid(message)) }
}

// An edit's opaque extensions are preserved, but neither their first encoding
// nor a longer destination identity may allocate beyond the wire ceiling.
struct BoundedRequest(Vec<u8>);
impl Write for BoundedRequest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > WORKER_MAX_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other(
                "Archive operation exceeds its byte limit",
            ));
        }
        self.0
            .try_reserve(bytes.len())
            .map_err(|_| std::io::Error::other("Cannot reserve bounded archive operation"))?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode_edit(intent: &ProjectEditIntent) -> Result<String> {
    intent.validate().map_err(invalid)?;
    let mut output = BoundedRequest(Vec::new());
    serde_json::to_writer(&mut output, intent).map_err(invalid)?;
    String::from_utf8(output.0).map_err(invalid)
}

impl ArchiveOperation {
    pub(crate) fn parse(request: &str) -> Result<Self> {
        require(
            request.len() <= WORKER_MAX_BYTES,
            "Archive operation exceeds its byte limit",
        )?;
        let value: Value = serde_json::from_str(request).map_err(invalid)?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("Archive operation must be an object"))?;
        // Ordinary extensions are flattened: projection-looking keys can be
        // opaque user data. An exact canonical edit remains an edit, including
        // one whose old operation identity uses a now-reserved prefix.
        let operation = if object.contains_key("scope") && object.contains_key("edit") {
            Self::Edit(serde_json::from_str(request).map_err(invalid)?)
        } else if object.contains_key("projection_id") {
            Self::Projection(serde_json::from_str(request).map_err(invalid)?)
        } else if object.contains_key("operation_id") {
            Self::Edit(serde_json::from_str(request).map_err(invalid)?)
        } else {
            // Reject an unknown envelope before decoding its intent. Preserve
            // the existing wrapper diagnostic even when a future operation's
            // payload is not a valid take publication.
            require(
                object.len() == 3
                    && object.get("kind").and_then(Value::as_str) == Some("take_publication")
                    && object.get("version").and_then(Value::as_u64) == Some(1)
                    && object.contains_key("intent"),
                "Unknown archived operation wrapper",
            )?;
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct TakeRequest {
                kind: String,
                version: u32,
                intent: TakePublication,
            }
            let wrapper: TakeRequest = serde_json::from_str(request).map_err(invalid)?;
            require(
                wrapper.kind == "take_publication" && wrapper.version == 1,
                "Unknown archived operation wrapper",
            )?;
            Self::Take(wrapper.intent)
        };
        require(
            operation.canonical_json()? == request,
            "Noncanonical archived operation request",
        )?;
        Ok(operation)
    }

    fn canonical_json(&self) -> Result<String> {
        match self {
            Self::Projection(intent) => intent.canonical_json(),
            Self::Edit(intent) => encode_edit(intent),
            Self::Take(intent) => crate::takes::publication_request(intent),
        }
    }

    pub(crate) fn validate_identity(
        &self,
        operation: &str,
        project: &str,
        previous: u64,
    ) -> Result<()> {
        // Revalidate even a directly constructed enum before comparing a row.
        self.canonical_json()?;
        let (id, project_id, revision) = match self {
            Self::Projection(intent) => (
                &intent.operation_id.0,
                &intent.project_id.0,
                intent.expected_revision,
            ),
            Self::Edit(intent) => (
                &intent.operation_id.0,
                &intent.project_id.0,
                intent.expected_revision,
            ),
            Self::Take(intent) => (
                &intent.operation_id.0,
                &intent.project_id.0,
                intent.expected_revision,
            ),
        };
        require(
            id == operation && project_id == project && revision == previous,
            "Archived operation identity/revision mismatch",
        )
    }

    /// Return `(destination_operation_id, canonical_request_json)`. This does
    /// not allocate or reserve identities in any catalog. A projection's ID is
    /// derived from its destination project and unchanged projection identity;
    /// `ordinary_operation` is ignored for that variant.
    pub(crate) fn remap(
        &self,
        project: &str,
        ordinary_operation: &str,
    ) -> Result<(String, String)> {
        require(project.len() <= 96, "Invalid destination project identity")?;
        ProjectId(project.into()).validate().map_err(invalid)?;
        if !matches!(self, Self::Projection(_)) {
            require(
                ordinary_operation.len() <= 96,
                "Invalid destination operation identity",
            )?;
            OperationId(ordinary_operation.into())
                .validate()
                .map_err(invalid)?;
        }
        self.canonical_json()?;
        let mut mapped = self.clone();
        let operation = match &mut mapped {
            Self::Projection(intent) => {
                intent.project_id = ProjectId(project.into());
                intent.operation_id =
                    projection_operation_id(&intent.project_id, &intent.projection_id)?;
                intent.operation_id.0.clone()
            }
            Self::Edit(intent) => {
                intent.project_id = ProjectId(project.into());
                intent.operation_id = OperationId(ordinary_operation.into());
                intent.operation_id.0.clone()
            }
            Self::Take(intent) => {
                intent.project_id = ProjectId(project.into());
                intent.operation_id = OperationId(ordinary_operation.into());
                intent.operation_id.0.clone()
            }
        };
        Ok((operation, mapped.canonical_json()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::{
        NoteTimelinePolicy,
        contracts::{EditOperation, EditScope, TakeId, TrackId},
        native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
        performance_projection::{ProjectionInstrument, ProjectionLaneChoice},
    };
    use omniusong_performance::projection::{
        PerformanceFrameRegion, PerformanceProjectionDuration, PerformanceProjectionPolicy,
        PerformanceProjectionRequest, PerformanceProjectionRounding,
        PerformanceProjectionSourcePin,
    };
    use omniusong_recording::RecoveryPrefix;
    use serde_json::json;

    fn projection() -> ProjectionPublication {
        let project_id = ProjectId("source-project".into());
        let projection_id = "projection-kept-id".to_owned();
        ProjectionPublication {
            version: 1,
            operation_id: projection_operation_id(&project_id, &projection_id).unwrap(),
            project_id,
            expected_revision: 7,
            projection_id,
            request: PerformanceProjectionRequest {
                version: 1,
                source: PerformanceProjectionSourcePin {
                    take_id: "retained-original".into(),
                    manifest_sha256: "a".repeat(64),
                    descriptor_sha256: "b".repeat(64),
                    original_rows_sha256: "c".repeat(64),
                    journal_sha256_at_review: "d".repeat(64),
                    checkpoint_sha256: "e".repeat(64),
                    selection: RecoveryPrefix {
                        chunk_count: 2,
                        chunk_index_sha256: "f".repeat(64),
                    },
                },
                region: PerformanceFrameRegion {
                    start: 80,
                    end: 160,
                },
                sources: vec![1, 4],
                tempo_bits: 123.75f64.to_bits(),
                destination_start_tick: 321,
                rounding: PerformanceProjectionRounding::NearestTiesUp,
                duration: PerformanceProjectionDuration::ReleaseGate,
                collapsed: PerformanceProjectionPolicy::OmitWithDisclosure,
                incomplete: PerformanceProjectionPolicy::Reject,
                region_crossing: PerformanceProjectionPolicy::Reject,
            },
            expected_review_sha256: "1".repeat(64),
            lanes: vec![
                ProjectionLaneChoice {
                    source: 1,
                    instrument: ProjectionInstrument::Keys,
                    track_id: "unchanged-note-lane".into(),
                    track_name: "Original keys".into(),
                },
                ProjectionLaneChoice {
                    source: 4,
                    instrument: ProjectionInstrument::Pad,
                    track_id: "unchanged-pad-lane".into(),
                    track_name: "Original pad".into(),
                },
            ],
            timeline: NoteTimelinePolicy::PreserveLoop,
            allow_projection_schema_upgrade: true,
        }
    }
    fn edit() -> ProjectEditIntent {
        ProjectEditIntent {
            operation_id: OperationId("ordinary-edit".into()),
            project_id: ProjectId("source-project".into()),
            expected_revision: 7,
            scope: EditScope::Project,
            edit: EditOperation::Rename {
                name: "Preserved human name".into(),
            },
            extensions: Default::default(),
        }
    }
    fn take() -> TakePublication {
        TakePublication {
            operation_id: OperationId("ordinary-take".into()),
            project_id: ProjectId("source-project".into()),
            expected_revision: 7,
            track_id: TrackId("same-recording-track".into()),
            track_name: "Preserved original".into(),
            clip: TakeClip {
                id: "same-clip-id".into(),
                source: TakeReference {
                    take_id: TakeId("retained-original".into()),
                    manifest_sha256: "a".repeat(64),
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 20,
                    count: 64,
                },
                position: TimelinePosition {
                    frame: 123,
                    sample_rate: 8_000,
                },
                fades: None,
                edit_source: None,
            },
        }
    }
    fn variants() -> Vec<ArchiveOperation> {
        vec![
            ArchiveOperation::Projection(projection()),
            ArchiveOperation::Edit(edit()),
            ArchiveOperation::Take(take()),
        ]
    }

    #[test]
    fn exact_typed_requests_roundtrip_and_match_all_three_recorded_identity_fields() {
        for operation in variants() {
            let canonical = operation.canonical_json().unwrap();
            assert_eq!(ArchiveOperation::parse(&canonical).unwrap(), operation);
            let actual_id = match &operation {
                ArchiveOperation::Projection(intent) => &intent.operation_id.0,
                ArchiveOperation::Edit(intent) => &intent.operation_id.0,
                ArchiveOperation::Take(intent) => &intent.operation_id.0,
            };
            operation
                .validate_identity(actual_id, "source-project", 7)
                .unwrap();
            assert!(
                operation
                    .validate_identity("other", "source-project", 7)
                    .is_err()
            );
            assert!(operation.validate_identity(actual_id, "other", 7).is_err());
            assert!(
                operation
                    .validate_identity(actual_id, "source-project", 8)
                    .is_err()
            );
        }
    }

    #[test]
    fn canonical_bytes_are_required_for_every_variant() {
        for operation in variants() {
            let canonical = operation.canonical_json().unwrap();
            let value: Value = serde_json::from_str(&canonical).unwrap();
            let pretty = serde_json::to_string_pretty(&value).unwrap();
            assert!(ArchiveOperation::parse(&pretty).is_err());
            assert!(ArchiveOperation::parse(&format!(" {canonical}")).is_err());
            assert!(ArchiveOperation::parse(&format!("{canonical}\n")).is_err());
        }
        for malformed in ["null", "[]", "1", "{", "{}"] {
            assert!(ArchiveOperation::parse(malformed).is_err());
        }
        let canonical = projection().canonical_json().unwrap();
        let duplicate = canonical.replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
        assert!(ArchiveOperation::parse(&duplicate).is_err());
    }

    #[test]
    fn malformed_projection_shapes_never_gain_projection_authority() {
        let original = projection();
        let mut wrong_id = original.clone();
        wrong_id.operation_id = OperationId("valid-but-not-derived".into());
        assert!(ArchiveOperation::parse(&serde_json::to_string(&wrong_id).unwrap()).is_err());
        let mut future = original.clone();
        future.version = 2;
        assert!(ArchiveOperation::parse(&serde_json::to_string(&future).unwrap()).is_err());
        let mut unknown = serde_json::to_value(&original).unwrap();
        unknown["source_path"] = "/not-an-authority".into();
        assert!(ArchiveOperation::parse(&serde_json::to_string(&unknown).unwrap()).is_err());
        let mut nested_unknown = serde_json::to_value(&original).unwrap();
        nested_unknown["request"]["source"]["note_ids"] = json!(["pn-do-not-rewrite"]);
        assert!(ArchiveOperation::parse(&serde_json::to_string(&nested_unknown).unwrap()).is_err());
        let mut missing = serde_json::to_value(&original).unwrap();
        missing.as_object_mut().unwrap().remove("projection_id");
        assert!(ArchiveOperation::parse(&serde_json::to_string(&missing).unwrap()).is_err());
        let mut ordinary = edit();
        ordinary
            .extensions
            .insert("projection_id".into(), Value::Null);
        assert_eq!(
            ArchiveOperation::parse(&serde_json::to_string(&ordinary).unwrap()).unwrap(),
            ArchiveOperation::Edit(ordinary)
        );
    }

    #[test]
    fn projection_remap_changes_only_project_and_derived_operation_identity() {
        let original = projection();
        let parsed = ArchiveOperation::parse(&original.canonical_json().unwrap()).unwrap();
        let (id, request) = parsed
            .remap("destination-project", "ignored invalid / ID")
            .unwrap();
        let expected_id = projection_operation_id(
            &ProjectId("destination-project".into()),
            &original.projection_id,
        )
        .unwrap();
        assert_eq!(id, expected_id.0);
        assert_ne!(id, original.operation_id.0);
        let mut expected = original.clone();
        expected.project_id = ProjectId("destination-project".into());
        expected.operation_id = expected_id;
        assert_eq!(request, expected.canonical_json().unwrap());
        let ArchiveOperation::Projection(mapped) = ArchiveOperation::parse(&request).unwrap()
        else {
            panic!("Projection remapping changed the operation protocol")
        };
        assert_eq!(mapped.request, original.request);
        assert_eq!(mapped.lanes, original.lanes);
        assert_eq!(
            mapped.expected_review_sha256,
            original.expected_review_sha256
        );
        assert_eq!(mapped.projection_id, original.projection_id);
        assert_eq!(mapped.expected_revision, original.expected_revision);
        assert_eq!(
            parsed.remap("destination-project", "another").unwrap(),
            (id, request)
        );
        assert!(parsed.remap("invalid destination /", "ignored").is_err());
        // Determinism is not a reservation or collision check in a destination.
    }

    #[test]
    fn ordinary_reserved_prefix_receipts_and_flat_opaque_projection_fields_remain_ordinary() {
        for prefix in ["os-perf-proj:", "os-src-attach:"] {
            let mut original = edit();
            original.operation_id = OperationId(format!("{prefix}legacy-receipt"));
            original.extensions.insert(
                "vendor".into(),
                json!({"project_id":"source-project","operation_id":original.operation_id.0,
                    "projection_id":"opaque-projection","noteId":"pn-original-note",
                    "trackId":"original-track","source":{"take_id":"opaque-original"}}),
            );
            original
                .extensions
                .insert("projection_id".into(), json!("opaque-projection"));
            original.extensions.insert(
                "request".into(),
                json!({"source":{"take_id":"opaque-original"}}),
            );
            original.extensions.insert(
                "lanes".into(),
                json!([{"trackId":"opaque-lane","noteId":"opaque-note"}]),
            );
            let canonical = serde_json::to_string(&original).unwrap();
            let parsed = ArchiveOperation::parse(&canonical).unwrap();
            assert!(matches!(&parsed, ArchiveOperation::Edit(_)));
            parsed
                .validate_identity(&original.operation_id.0, "source-project", 7)
                .unwrap();
            let (id, request) = parsed
                .remap("destination-project", "new-ordinary-operation")
                .unwrap();
            assert_eq!(id, "new-ordinary-operation");
            let ArchiveOperation::Edit(mapped) = ArchiveOperation::parse(&request).unwrap() else {
                panic!("Ordinary receipt was reclassified by an operation prefix")
            };
            let mut expected = original.clone();
            expected.project_id = ProjectId("destination-project".into());
            expected.operation_id = OperationId("new-ordinary-operation".into());
            assert_eq!(mapped, expected);
            assert_eq!(mapped.extensions, original.extensions);
            assert!(
                parsed
                    .remap("destination-project", "bad / operation")
                    .is_err()
            );
        }
    }

    #[test]
    fn take_wrapper_is_exact_and_remap_preserves_every_nested_clip_field() {
        let original = take();
        let request = crate::takes::publication_request(&original).unwrap();
        let parsed = ArchiveOperation::parse(&request).unwrap();
        let (id, mapped) = parsed
            .remap("destination-project", "mapped-take-operation")
            .unwrap();
        assert_eq!(id, "mapped-take-operation");
        let mut expected = original.clone();
        expected.project_id = ProjectId("destination-project".into());
        expected.operation_id = OperationId(id);
        assert_eq!(
            mapped,
            crate::takes::publication_request(&expected).unwrap()
        );
        assert_eq!(
            ArchiveOperation::parse(&mapped).unwrap(),
            ArchiveOperation::Take(expected)
        );
        let wrapper: Value = serde_json::from_str(&request).unwrap();
        for (key, value) in [
            ("version", json!(2)),
            ("kind", json!("unknown")),
            ("extra", json!(true)),
        ] {
            let mut changed = wrapper.clone();
            changed[key] = value;
            assert!(matches!(
                ArchiveOperation::parse(&serde_json::to_string(&changed).unwrap()),
                Err(StoreError::Invalid(message)) if message == "Unknown archived operation wrapper"
            ));
        }
        let mut nested = wrapper;
        nested["intent"]["unknown"] = true.into();
        assert!(ArchiveOperation::parse(&serde_json::to_string(&nested).unwrap()).is_err());
    }

    #[test]
    fn actual_catalog_and_pending_collisions_refuse_derived_projection_ids_without_writes() {
        use rusqlite::Connection;
        fn catalog() -> Connection {
            let c = Connection::open_in_memory().unwrap();
            c.execute_batch("CREATE TABLE operations(operation_id TEXT PRIMARY KEY); CREATE TABLE archive_copies(operation_id TEXT PRIMARY KEY); CREATE TEMP TABLE pending_copy_map(destination_operation_id TEXT UNIQUE);").unwrap();
            c
        }
        fn rows(c: &Connection) -> (i64, i64, i64) {
            c.query_row("SELECT (SELECT count(*) FROM operations),(SELECT count(*) FROM archive_copies),(SELECT count(*) FROM pending_copy_map)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap()
        }
        let source = catalog();
        let destination = catalog();
        let intent = projection();
        let raw = intent.canonical_json().unwrap();
        let expected = projection_operation_id(
            &ProjectId("destination-project".into()),
            &intent.projection_id,
        )
        .unwrap()
        .0;
        let mapped = || {
            crate::archive_copy::mapped_copy_request(
                &destination,
                &source,
                &raw,
                "destination-project",
                "restore-request",
            )
        };
        let (id, request) = mapped().unwrap();
        assert_eq!(id, expected);
        ArchiveOperation::parse(&request)
            .unwrap()
            .validate_identity(&expected, "destination-project", intent.expected_revision)
            .unwrap();
        assert_eq!((rows(&source), rows(&destination)), ((0, 0, 0), (0, 0, 0)));
        for table in ["operations", "archive_copies", "pending_copy_map"] {
            let column = if table == "pending_copy_map" {
                "destination_operation_id"
            } else {
                "operation_id"
            };
            destination
                .execute(
                    &format!("INSERT INTO {table}({column}) VALUES(?1)"),
                    [&expected],
                )
                .unwrap();
            let before = rows(&destination);
            assert!(
                matches!(mapped(), Err(StoreError::Invalid(message)) if message == "Copied operation identity collides with retained or pending authority")
            );
            assert_eq!(rows(&destination), before);
            assert_eq!(rows(&source), (0, 0, 0));
            destination
                .execute(&format!("DELETE FROM {table}"), [])
                .unwrap();
        }
        source
            .execute("INSERT INTO operations VALUES(?1)", [&expected])
            .unwrap();
        assert!(mapped().is_err());
        assert_eq!((rows(&source), rows(&destination)), ((1, 0, 0), (0, 0, 0)));
        source.execute("DELETE FROM operations", []).unwrap();
        source
            .execute_batch("CREATE TABLE archive_copy_proofs(proof_sha256 TEXT PRIMARY KEY)")
            .unwrap();
        source
            .execute("INSERT INTO archive_copies VALUES(?1)", [&expected])
            .unwrap();
        assert!(mapped().is_err());
        assert_eq!((rows(&source), rows(&destination)), ((0, 1, 0), (0, 0, 0)));
        source.execute("DELETE FROM archive_copies", []).unwrap();
        assert!(
            crate::archive_copy::mapped_copy_request(
                &destination,
                &source,
                &raw,
                "destination-project",
                &expected
            )
            .is_err()
        );
        assert_eq!((rows(&source), rows(&destination)), ((0, 0, 0), (0, 0, 0)));
        // A version-1 source has no lineage/copy tables; collision checking
        // must retain that path instead of querying absent format-2 tables.
        source
            .execute_batch("DROP TABLE archive_copy_proofs; DROP TABLE archive_copies")
            .unwrap();
        assert_eq!(mapped().unwrap().0, expected);
        assert_eq!(rows(&destination), (0, 0, 0));
    }

    #[test]
    fn input_and_remapped_output_both_obey_the_original_wire_ceiling() {
        assert!(ArchiveOperation::parse(&" ".repeat(WORKER_MAX_BYTES + 1)).is_err());
        let mut large = edit();
        for i in 0..7 {
            large
                .extensions
                .insert(format!("opaque-{i}"), Value::String("x".repeat(8192)));
        }
        large
            .extensions
            .insert("tail".into(), Value::String(String::new()));
        let base = serde_json::to_string(&large).unwrap().len();
        let tail = WORKER_MAX_BYTES - 1 - base;
        assert!(tail <= 8192);
        large
            .extensions
            .insert("tail".into(), Value::String("x".repeat(tail)));
        let raw = serde_json::to_string(&large).unwrap();
        assert_eq!(raw.len(), WORKER_MAX_BYTES - 1);
        let parsed = ArchiveOperation::parse(&raw).unwrap();
        let project = "p".repeat(96);
        let operation = "o".repeat(96);
        ProjectId(project.clone()).validate().unwrap();
        OperationId(operation.clone()).validate().unwrap();
        assert!(
            matches!(parsed.remap(&project, &operation), Err(StoreError::Invalid(message))
            if message.contains("Archive operation exceeds its byte limit"))
        );
        let (id, unchanged) = parsed.remap("source-project", "ordinary-edit").unwrap();
        assert_eq!(id, "ordinary-edit");
        assert_eq!(unchanged, raw);
    }
}
