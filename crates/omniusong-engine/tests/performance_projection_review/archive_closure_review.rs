//! Independent real-original tests for the read-only retained-closure diagnostic.
//! These do not qualify OSAR export, restore/copy, native audio or inference.
use super::*;
use omniusong_core::{
    DocumentNoteAddress, NoteTimelinePolicy, ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
    performance_projection::{ProjectionInstrument, ProjectionLaneChoice},
};
use omniusong_store::{
    NativeStore, ProjectionPublication, RestoreCopyIntent, StoreError, projection_operation_id,
};
use rusqlite::{Connection, params, types::ValueRef};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

struct ClosureFixture {
    original: Fixture,
    root: PathBuf,
    store: NativeStore,
    project: ProjectDocument,
    owned: PathBuf,
}
impl ClosureFixture {
    fn new(v2: bool) -> Self {
        let original = Fixture::record(v2, |tap, writer| {
            tap.admit(&[on(1, 1, 1, 0)]);
            tap.render(writer, 80);
            tap.admit(&[off(1, 1, 2, 65535)]);
            tap.render(writer, 2000);
            if v2 {
                tap.finish(writer, 64);
            }
        });
        original.recover();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-projection-closure-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let mut store = NativeStore::open(root.join("store")).unwrap();
        let project = store
            .create(&ProjectDocument::new("Closure destination").unwrap())
            .unwrap();
        let source = SealedJournal::open(&original.path).unwrap();
        let object = store.ingest_take(&source).unwrap();
        drop(source);
        let owned = store
            .root()
            .join("takes")
            .join(format!("{}.sqlite3", object.object_sha256));
        Self {
            original,
            root,
            store,
            project,
            owned,
        }
    }
    fn db(&self) -> Connection {
        Connection::open(self.store.root().join("studio.sqlite3")).unwrap()
    }
    fn intent(&self, current: &ProjectDocument, id: &str, lane: &str) -> ProjectionPublication {
        let staged = self.original.request();
        let reference = omniusong_core::native::TakeReference {
            take_id: self.original.config.take_id.clone(),
            manifest_sha256: staged.source.manifest_sha256,
            stream_id: "audio".into(),
        };
        let (governor, _) = budget(InspectionLimits::default());
        let source = self
            .store
            .open_take_with_budget(&reference, &governor)
            .unwrap();
        let held = AuthenticatedPerformance::open(&source).unwrap();
        let mut request = self.original.request();
        request.source = held.source_pin().clone();
        request.sources = vec![1];
        request.tempo_bits = current.tempo().to_bits();
        let review = held.project(&request).unwrap();
        assert_eq!(review.notes.len(), 1);
        let project_id = ProjectId(current.id().into());
        ProjectionPublication {
            version: 1,
            operation_id: projection_operation_id(&project_id, id).unwrap(),
            project_id,
            expected_revision: current.revision(),
            projection_id: id.into(),
            request,
            expected_review_sha256: review.review_sha256,
            lanes: vec![lane_choice(lane)],
            timeline: NoteTimelinePolicy::PreserveLoop,
            allow_projection_schema_upgrade: true,
        }
    }
    fn publish(&mut self) -> (ProjectionPublication, ProjectDocument) {
        let intent = self.intent(&self.project, "projection-one", "original-note-lane");
        let (governor, _) = budget(InspectionLimits::default());
        self.store
            .publish_performance_projection(&intent, &governor)
            .unwrap();
        let current = self.store.load(self.project.id()).unwrap();
        (intent, current)
    }
}
impl Drop for ClosureFixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Closure review retained at {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn lane_choice(id: &str) -> ProjectionLaneChoice {
    ProjectionLaneChoice {
        source: 1,
        instrument: ProjectionInstrument::Keys,
        track_id: id.into(),
        track_name: "Original zero-velocity key".into(),
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn edit(document: &ProjectDocument, id: &str, name: &str) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        scope: EditScope::Project,
        edit: EditOperation::Rename { name: name.into() },
        extensions: Default::default(),
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
#[derive(Debug, PartialEq, Eq)]
struct Catalog {
    application: i64,
    version: i64,
    schema: Vec<(String, String, String, Option<String>)>,
    tables: Vec<(String, Vec<Vec<Cell>>)>,
}
fn quote(id: &str) -> String {
    format!("\"{}\"", id.replace('"', "\"\""))
}
fn catalog(db: &Connection) -> Catalog {
    let schema = db
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let tables = names
        .into_iter()
        .map(|name| {
            let mut statement = db
                .prepare(&format!("SELECT * FROM {}", quote(&name)))
                .unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            let mut values = Vec::new();
            while let Some(row) = rows.next().unwrap() {
                values.push(
                    (0..columns)
                        .map(|i| match row.get_ref(i).unwrap() {
                            ValueRef::Null => Cell::Null,
                            ValueRef::Integer(v) => Cell::Integer(v),
                            ValueRef::Real(v) => Cell::Real(v.to_bits()),
                            ValueRef::Text(v) => Cell::Text(v.to_vec()),
                            ValueRef::Blob(v) => Cell::Blob(v.to_vec()),
                        })
                        .collect::<Vec<_>>(),
                );
            }
            values.sort();
            (name, values)
        })
        .collect();
    Catalog {
        application: db
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .unwrap(),
        version: db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap(),
        schema,
        tables,
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            assert!(
                !kind.is_symlink(),
                "Synthetic fixture unexpectedly contains a symlink"
            );
            if kind.is_dir() {
                walk(base, &path, output);
            } else {
                output.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
}
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    catalog: Catalog,
    files: BTreeMap<PathBuf, Vec<u8>>,
    staged: BTreeMap<PathBuf, Vec<u8>>,
}
impl Snapshot {
    fn capture(f: &ClosureFixture) -> Self {
        Self {
            catalog: catalog(&f.db()),
            files: files(&f.root),
            staged: f.original.bytes(),
        }
    }
    fn unchanged(&self, f: &ClosureFixture) {
        assert_eq!(Snapshot::capture(f), *self);
    }
}
fn semantic_refusal<T: std::fmt::Debug>(result: Result<T, StoreError>) {
    assert!(
        matches!(
            &result,
            Err(StoreError::Invalid(_)) | Err(StoreError::Recording(RecordingError::Invalid(_)))
        ),
        "Expected a semantic refusal, not an unrelated SQL/I/O failure: {result:?}"
    );
}
fn remove_immutable_triggers(db: &Connection, table: &str) {
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1")
        .unwrap()
        .query_map([table], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for name in names {
        db.execute_batch(&format!("DROP TRIGGER {}", quote(&name)))
            .unwrap();
    }
}
fn replace_operation_result(db: &Connection, id: &str, result: &ProjectDocument) {
    let raw = String::from_utf8(result.to_json().unwrap()).unwrap();
    db.execute(
        "UPDATE operations SET result_json=?1,result_sha256=?2 WHERE operation_id=?3",
        params![raw, hash(raw.as_bytes()), id],
    )
    .unwrap();
}
fn false_pitch(document: &ProjectDocument) -> ProjectDocument {
    let mut registry = document.performance_projections().unwrap().unwrap();
    let binding = &mut registry.entries[0];
    let mut review: PerformanceProjectionReview =
        serde_json::from_str(&binding.review.canonical_json).unwrap();
    assert_eq!(review.notes[0].values.pitch, 60);
    review.notes[0].values.pitch = 61;
    review.review_sha256.clear();
    let mut digest = Sha256::new();
    digest.update(b"OmniuSong.performance-projection-review.v1\0");
    digest.update(serde_json::to_vec(&review).unwrap());
    review.review_sha256 = format!("{:x}", digest.finalize());
    binding.review.canonical_json = serde_json::to_string(&review).unwrap();
    binding.review.review_sha256 = review.review_sha256;
    binding.lanes[0].original_notes[0].values.pitch = 61;
    let (encoding, _) = budget(InspectionLimits::default());
    check_retained_binding_encoding_bounded(binding, &encoding).unwrap();
    let track = binding.lanes[0].track_id.clone();
    let mut value = document.value().clone();
    value["performanceProjections"] = serde_json::to_value(registry).unwrap();
    value["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["id"] == track)
        .unwrap()["notes"][0]["pitch"] = 61.into();
    ProjectDocument::from_value(value).unwrap()
}

// This is internally consistent synthetic lineage metadata, not a claim that
// an archive was produced or a copy operation was executed. The zero-row map
// uses the independent empty framed-domain digest.
fn synthetic_proof(document: &ProjectDocument) -> (String, String) {
    #[derive(serde::Serialize)]
    struct ProofRecord<'a> {
        version: u32,
        operation_id: &'a str,
        source_archive_sha256: String,
        source_archive_byte_length: u64,
        source_project_id: &'a str,
        destination_project_id: &'a str,
        mapping_count: u64,
        mapping_sha256: String,
        ancestors: Vec<String>,
        result: serde_json::Value,
        result_sha256: String,
    }
    let proof = ProofRecord {
        version: 1,
        operation_id: "synthetic-copy-admission",
        source_archive_sha256: "f".repeat(64),
        source_archive_byte_length: 123,
        source_project_id: "historical-source-project",
        destination_project_id: document.id(),
        mapping_count: 0,
        mapping_sha256: hash(b"OmniuSong.copy-mapping.v1\0"),
        ancestors: vec![],
        result: document.value().clone(),
        result_sha256: hash(&document.to_json().unwrap()),
    };
    let raw = serde_json::to_string(&proof).unwrap();
    (hash(raw.as_bytes()), raw)
}

fn attach_synthetic_proof(f: &ClosureFixture, document: &ProjectDocument) {
    let (sha, raw) = synthetic_proof(document);
    let db = f.db();
    db.execute(
        "INSERT INTO archive_copy_proofs(proof_sha256,receipt_json) VALUES(?1,?2)",
        params![sha, raw],
    )
    .unwrap();
    db.execute(
        "INSERT INTO archive_project_lineage(project_id,proof_sha256) VALUES(?1,?2)",
        params![f.project.id(), sha],
    )
    .unwrap();
    db.execute("INSERT INTO archive_copies(operation_id,project_id,proof_sha256) VALUES('synthetic-copy-admission',?1,?2)",params![f.project.id(),sha]).unwrap();
}

fn ingest_audio_only(f: &mut ClosureFixture) -> omniusong_core::native::TakeReference {
    use omniusong_engine::{
        capture::CaptureJournal,
        realtime::{AudioPacket, MAX_CHANNELS, PACKET_FRAMES},
    };
    let path = f.root.join("separate-audio-only.sqlite3");
    let descriptor:omniusong_recording::TakeDescriptor=serde_json::from_value(json!({
        "version":1,"take_id":uuid::Uuid::new_v4().to_string(),"streams":[{
            "stream_id":"audio","segment_id":"ordinary-audio-segment","source_id":"synthetic-packets",
            "source_name":"Device-free closure audio","source_connection_generation":1,
            "host_clock":{"domain":"synthetic-audio-clock","epoch":"synthetic-audio-epoch","ticks_per_second_numerator":1000000000,"ticks_per_second_denominator":1},
            "kind":{"kind":"audio","format":{"sample_rate":8000,"channels":1,"encoding":"f32_le","layout":"interleaved"},"audio_epoch":"synthetic-audio-epoch"},
            "initial_source_position":0
        }],"provenance":{"synthetic":true}
    })).unwrap();
    let mut writer = CaptureJournal::create(&path, descriptor).unwrap();
    let mut start = 0;
    for i in 0..8 {
        let frames = if i == 7 { 208 } else { PACKET_FRAMES as u16 };
        writer
            .append_packet(&AudioPacket {
                start_frame: start,
                frames,
                channels: 1,
                clock_start_ns: start * 125000,
                clock_end_ns: (start + u64::from(frames)) * 125000,
                samples: [0.125; PACKET_FRAMES * MAX_CHANNELS],
            })
            .unwrap();
        start += u64::from(frames);
    }
    assert_eq!(start, 2000);
    // At 8 kHz the writer flushed its exact 2000-frame quarter-second chunk.
    // Dropping it leaves an interrupted original; no normal seal is requested.
    drop(writer);
    let (manifest, replayed) = CaptureReader::open(&path)
        .unwrap()
        .recover("closure-review.audio-recovered")
        .unwrap();
    assert!(!replayed && manifest.body.interrupted.is_some());
    let reader = SealedJournal::open(&path).unwrap();
    assert_eq!(reader.descriptor().streams.len(), 1);
    assert!(
        !reader
            .descriptor()
            .provenance
            .contains_key("omniusong.performance.v1")
    );
    assert!(
        !reader
            .descriptor()
            .provenance
            .contains_key("omniusong.performance.v2")
    );
    let object = f.store.ingest_take(&reader).unwrap();
    omniusong_core::native::TakeReference {
        take_id: object.take_id,
        manifest_sha256: object.manifest_sha256,
        stream_id: "audio".into(),
    }
}

#[test]
fn real_v1_v2_edited_deleted_and_undo_hidden_originals_have_complete_read_only_closure() {
    for v2 in [false, true] {
        let mut f = ClosureFixture::new(v2);
        let (_, published) = f.publish();
        let registry = published.performance_projections().unwrap().unwrap();
        let binding = &registry.entries[0];
        let address = DocumentNoteAddress {
            track_id: binding.lanes[0].track_id.clone(),
            note_id: binding.lanes[0].original_notes[0].note_id.0.clone(),
        };
        let mut values = binding.lanes[0].original_notes[0].values;
        values.pitch = 71;
        values.velocity16 = 65535;
        values.start_tick += 17;
        let mut changed = published
            .update_note(&address, values, NoteTimelinePolicy::PreserveLoop)
            .unwrap();
        changed.rename("Later human edits").unwrap();
        changed.set_tempo(141.25).unwrap();
        let changed = f.store.save(&changed, published.revision()).unwrap();
        let deleted = changed.delete_note(&address).unwrap();
        let mut value = deleted.value().clone();
        value["tracks"]
            .as_array_mut()
            .unwrap()
            .retain(|t| t["id"] != address.track_id);
        let missing_lane = f
            .store
            .save(
                &ProjectDocument::from_value(value).unwrap(),
                changed.revision(),
            )
            .unwrap();
        f.store
            .undo(missing_lane.id(), missing_lane.revision())
            .unwrap();
        let before = Snapshot::capture(&f);
        let (governor, _) = budget(InspectionLimits::default());
        let result = f
            .store
            .inspect_retained_projection_closure(f.project.id(), &governor)
            .unwrap();
        assert_eq!(result.version, 1);
        assert_eq!(result.project_id, f.project.id());
        assert_eq!(
            (
                result.snapshots,
                result.history_rows,
                result.operation_rows,
                result.lineage_proof_rows,
                result.distinct_bindings
            ),
            (8, 3, 1, 0, 1)
        );
        governor.checkpoint().unwrap();
        before.unchanged(&f);
    }
}

#[test]
fn two_distinct_projections_share_one_registered_original_without_mutating_any_snapshot() {
    let mut f = ClosureFixture::new(true);
    let (_, first) = f.publish();
    let second = f.intent(&first, "projection-two", "second-note-lane");
    let (publication_budget, _) = budget(InspectionLimits::default());
    f.store
        .publish_performance_projection(&second, &publication_budget)
        .unwrap();
    let before = Snapshot::capture(&f);
    let (governor, token) = budget(InspectionLimits::default());
    let result = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &governor)
        .unwrap();
    assert_eq!(
        (
            result.snapshots,
            result.history_rows,
            result.operation_rows,
            result.lineage_proof_rows,
            result.distinct_bindings
        ),
        (7, 2, 2, 0, 2)
    );
    let first_usage = governor.usage();
    let repeated = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &governor)
        .unwrap();
    assert_eq!(repeated.distinct_bindings, 2);
    assert!(governor.usage().work_bytes > first_usage.work_bytes);
    assert!(governor.usage().vm_steps > first_usage.vm_steps);
    token.store(true, Ordering::Release);
    let stopped_usage = governor.usage();
    assert!(matches!(
        f.store
            .inspect_retained_projection_closure(f.project.id(), &governor),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    assert_eq!(governor.usage(), stopped_usage);
    before.unchanged(&f);
    // file_bytes is a maximum, not an authentication invocation counter.
}

#[test]
fn a_recording_clip_and_projection_can_share_the_same_real_performance_original() {
    let mut f = ClosureFixture::new(true);
    let (_, mut published) = f.publish();
    let source = published
        .performance_projections()
        .unwrap()
        .unwrap()
        .entries[0]
        .source
        .clone();
    let revision = published.revision();
    published
        .append_take_clip(
            "same-original-audio",
            "Original rendered audio",
            omniusong_core::native::TakeClip {
                id: "same-original-audio-clip".into(),
                source,
                range: omniusong_core::native::TakeRange::AudioFrames {
                    start: 0,
                    count: 64,
                },
                position: omniusong_core::native::TimelinePosition {
                    frame: 0,
                    sample_rate: 8000,
                },
                fades: None,
                edit_source: None,
            },
        )
        .unwrap();
    f.store.save(&published, revision).unwrap();
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    let result = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &governor)
        .unwrap();
    assert_eq!(
        (
            result.snapshots,
            result.history_rows,
            result.operation_rows,
            result.lineage_proof_rows,
            result.distinct_bindings
        ),
        (6, 2, 1, 0, 1)
    );
    before.unchanged(&f);
}

#[test]
fn a_separate_audio_only_original_keeps_ordinary_role_and_hidden_clip_geometry_is_checked() {
    let mut f = ClosureFixture::new(true);
    let (_, mut published) = f.publish();
    let audio = ingest_audio_only(&mut f);
    assert_ne!(audio.take_id, f.original.config.take_id);
    let revision = published.revision();
    published
        .append_take_clip(
            "ordinary-audio-track",
            "Separate ordinary PCM",
            omniusong_core::native::TakeClip {
                id: "ordinary-audio-clip".into(),
                source: audio,
                range: omniusong_core::native::TakeRange::AudioFrames {
                    start: 0,
                    count: 64,
                },
                position: omniusong_core::native::TimelinePosition {
                    frame: 0,
                    sample_rate: 8000,
                },
                fades: None,
                edit_source: None,
            },
        )
        .unwrap();
    let with_audio = f.store.save(&published, revision).unwrap();
    let before_control = Snapshot::capture(&f);
    let (control, _) = budget(InspectionLimits::default());
    let accepted = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &control)
        .unwrap();
    assert_eq!(
        (
            accepted.snapshots,
            accepted.history_rows,
            accepted.operation_rows,
            accepted.lineage_proof_rows,
            accepted.distinct_bindings
        ),
        (6, 2, 1, 0, 1)
    );
    before_control.unchanged(&f);

    f.store
        .undo(with_audio.id(), with_audio.revision())
        .unwrap();
    let mut invalid = with_audio.value().clone();
    invalid["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["id"] == "ordinary-audio-track")
        .unwrap()["recording"]["clips"][0]["range"]["count"] = 2001.into();
    let invalid = ProjectDocument::from_value(invalid).unwrap();
    let db = f.db();
    remove_immutable_triggers(&db, "edits");
    db.execute(
        "UPDATE edits SET after_json=?1 WHERE project_id=?2 AND sequence=2",
        params![
            String::from_utf8(invalid.to_json().unwrap()).unwrap(),
            f.project.id()
        ],
    )
    .unwrap();
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    semantic_refusal(
        f.store
            .inspect_retained_projection_closure(f.project.id(), &governor),
    );
    before.unchanged(&f);
}

#[test]
fn two_supported_ordinary_copy_generations_remain_valid_ancestry_after_leaf_projection() {
    let mut f = ClosureFixture::new(true);
    let ordinary = f
        .store
        .apply_edit(&edit(
            &f.project,
            "ordinary-before-copy",
            "Ordinary source edit",
        ))
        .unwrap()
        .document;
    assert!(ordinary.performance_projections().unwrap().is_none());
    let first_archive = f.root.join("ordinary-generation-zero.omniusong");
    let first_export = f
        .store
        .export_archive(ordinary.id(), &first_archive)
        .unwrap();
    let mut first = NativeStore::open(f.root.join("copy-one")).unwrap();
    let first_copy = first
        .restore_archive_as_copy(
            &first_archive,
            &RestoreCopyIntent {
                operation_id: OperationId("ordinary-copy-one".into()),
                expected_archive_sha256: first_export.sha256,
            },
        )
        .unwrap()
        .document;
    let first_changed = first
        .apply_edit(&edit(
            &first_copy,
            "ordinary-between-copies",
            "Ordinary first-generation edit",
        ))
        .unwrap()
        .document;
    assert!(first_changed.performance_projections().unwrap().is_none());
    let second_archive = f.root.join("ordinary-generation-one.omniusong");
    let second_export = first
        .export_archive(first_changed.id(), &second_archive)
        .unwrap();
    let mut leaf = NativeStore::open(f.root.join("copy-two")).unwrap();
    let leaf_project = leaf
        .restore_archive_as_copy(
            &second_archive,
            &RestoreCopyIntent {
                operation_id: OperationId("ordinary-copy-two".into()),
                expected_archive_sha256: second_export.sha256,
            },
        )
        .unwrap()
        .document;
    assert!(leaf_project.performance_projections().unwrap().is_none());
    // Both public archive/copy generations are ordinary and precede the first
    // projection. This does not exercise nonempty projection archive admission.
    let source = SealedJournal::open(&f.original.path).unwrap();
    let object = leaf.ingest_take(&source).unwrap();
    drop(source);
    let owned = leaf
        .root()
        .join("takes")
        .join(format!("{}.sqlite3", object.object_sha256));
    f.store = leaf;
    f.project = leaf_project;
    f.owned = owned;
    f.publish();
    let before_control = Snapshot::capture(&f);
    let (control, _) = budget(InspectionLimits::default());
    let accepted = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &control)
        .unwrap();
    assert_eq!(
        (
            accepted.snapshots,
            accepted.history_rows,
            accepted.operation_rows,
            accepted.lineage_proof_rows,
            accepted.distinct_bindings
        ),
        (12, 3, 3, 2, 1)
    );
    before_control.unchanged(&f);
    let db = f.db();
    let parent:String=db.query_row("SELECT proof_sha256 FROM archive_project_lineage WHERE project_id=?1 AND proof_sha256!=(SELECT proof_sha256 FROM archive_copies WHERE project_id=?1)",[f.project.id()],|r|r.get(0)).unwrap();
    let rows: i64 = db
        .query_row(
            "SELECT count(*) FROM archive_copy_proof_operations WHERE proof_sha256=?1",
            [&parent],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        rows, 1,
        "The first actual copy retained one ordinary operation mapping"
    );
    remove_immutable_triggers(&db, "archive_copy_proof_operations");
    assert_eq!(
        db.execute(
            "DELETE FROM archive_copy_proof_operations WHERE proof_sha256=?1",
            [&parent]
        )
        .unwrap(),
        1
    );
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    semantic_refusal(
        f.store
            .inspect_retained_projection_closure(f.project.id(), &governor),
    );
    before.unchanged(&f);
}

#[test]
fn coherently_rehashed_false_pitch_in_operation_only_or_undo_hidden_history_refuses() {
    for operation_only in [true, false] {
        let mut f = ClosureFixture::new(true);
        let (intent, published) = f.publish();
        let undone = f.store.undo(published.id(), published.revision()).unwrap();
        if operation_only {
            f.store
                .apply_edit(&edit(&undone, "pruned-redo-branch", "Replacement branch"))
                .unwrap();
            // The typed operation remains, but the new branch removes its old
            // before/after edge. A history-result mismatch cannot satisfy this
            // source-rederivation oracle.
            let before_control = Snapshot::capture(&f);
            let (control, _) = budget(InspectionLimits::default());
            let accepted = f
                .store
                .inspect_retained_projection_closure(f.project.id(), &control)
                .unwrap();
            assert_eq!((accepted.history_rows, accepted.operation_rows), (1, 2));
            before_control.unchanged(&f);
        }
        let forged = false_pitch(&published);
        let db = f.db();
        if operation_only {
            remove_immutable_triggers(&db, "operations");
            let mut false_intent = intent.clone();
            false_intent.expected_review_sha256 =
                forged.performance_projections().unwrap().unwrap().entries[0]
                    .review
                    .review_sha256
                    .clone();
            let request = false_intent.canonical_json().unwrap();
            db.execute(
                "UPDATE operations SET request_json=?1,request_sha256=?2 WHERE operation_id=?3",
                params![request, hash(request.as_bytes()), intent.operation_id.0],
            )
            .unwrap();
            replace_operation_result(&db, &intent.operation_id.0, &forged);
            // The compact consistency receipt accepts these attacker-authored
            // cells. It still cannot authenticate their false musical claim.
            assert!(
                f.store
                    .projection_publication_receipt(&false_intent)
                    .unwrap()
                    .is_some()
            );
        } else {
            remove_immutable_triggers(&db, "edits");
            db.execute(
                "UPDATE edits SET after_json=?1 WHERE project_id=?2 AND sequence=1",
                params![
                    String::from_utf8(forged.to_json().unwrap()).unwrap(),
                    published.id()
                ],
            )
            .unwrap();
        }
        let before = Snapshot::capture(&f);
        let (governor, _) = budget(InspectionLimits::default());
        semantic_refusal(
            f.store
                .inspect_retained_projection_closure(f.project.id(), &governor),
        );
        before.unchanged(&f);
    }
}

#[test]
fn source_valid_different_binding_cannot_reuse_an_id_in_a_pruned_operation_result() {
    let mut f = ClosureFixture::new(true);
    let (intent, published) = f.publish();
    let abandoned = f
        .store
        .apply_edit(&edit(&published, "abandoned-result", "Abandoned branch"))
        .unwrap()
        .document;
    let undone = f.store.undo(abandoned.id(), abandoned.revision()).unwrap();
    f.store
        .apply_edit(&edit(&undone, "live-branch", "Current branch"))
        .unwrap();
    let reference = published
        .performance_projections()
        .unwrap()
        .unwrap()
        .entries[0]
        .source
        .clone();
    let different = {
        let (proof_budget, _) = budget(InspectionLimits::default());
        let reader = f
            .store
            .open_take_with_budget(&reference, &proof_budget)
            .unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let binding = held
            .build_binding(
                &intent.projection_id,
                &intent.request,
                &intent.expected_review_sha256,
                &[lane_choice("different-but-source-valid-lane")],
            )
            .unwrap();
        held.validate_binding(&binding).unwrap();
        binding
    };
    let mut registry = abandoned.performance_projections().unwrap().unwrap();
    assert_ne!(different, registry.entries[0]);
    let old_track = registry.entries[0].lanes[0].track_id.clone();
    registry.entries[0] = different;
    let mut value = abandoned.value().clone();
    value["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["id"] == old_track)
        .unwrap()["id"] = "different-but-source-valid-lane".into();
    value["performanceProjections"] = serde_json::to_value(registry).unwrap();
    let different_result = ProjectDocument::from_value(value).unwrap();
    let db = f.db();
    remove_immutable_triggers(&db, "operations");
    replace_operation_result(&db, "abandoned-result", &different_result);
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    semantic_refusal(
        f.store
            .inspect_retained_projection_closure(f.project.id(), &governor),
    );
    before.unchanged(&f);
}

#[test]
fn initial_publication_result_keeps_initial_muted_lanes_after_its_before_image_is_pruned() {
    let mut f = ClosureFixture::new(true);
    let (intent, published) = f.publish();
    let undone = f.store.undo(published.id(), published.revision()).unwrap();
    f.store
        .apply_edit(&edit(
            &undone,
            "pruned-muted-before-image",
            "Replacement branch",
        ))
        .unwrap();
    let before_control = Snapshot::capture(&f);
    let (control, _) = budget(InspectionLimits::default());
    let accepted = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &control)
        .unwrap();
    assert_eq!((accepted.history_rows, accepted.operation_rows), (1, 2));
    before_control.unchanged(&f);
    let mut value = published.value().clone();
    let lane = published
        .performance_projections()
        .unwrap()
        .unwrap()
        .entries[0]
        .lanes[0]
        .track_id
        .clone();
    value["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["id"] == lane)
        .unwrap()["muted"] = false.into();
    let unmuted = ProjectDocument::from_value(value).unwrap();
    let db = f.db();
    remove_immutable_triggers(&db, "operations");
    replace_operation_result(&db, &intent.operation_id.0, &unmuted);
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    assert!(
        matches!(f.store.inspect_retained_projection_closure(f.project.id(), &governor),
        Err(StoreError::Invalid(message)) if message == "Recorded publication initial note lane differs")
    );
    before.unchanged(&f);
}

#[test]
fn coherently_rehashed_false_music_in_an_associated_proof_only_refuses_after_valid_control() {
    let mut f = ClosureFixture::new(true);
    let (_, published) = f.publish();
    let (proof_sha, proof_json) = synthetic_proof(&published);
    let db = f.db();
    db.execute(
        "INSERT INTO archive_copy_proofs(proof_sha256,receipt_json) VALUES(?1,?2)",
        params![proof_sha, proof_json],
    )
    .unwrap();
    db.execute(
        "INSERT INTO archive_project_lineage(project_id,proof_sha256) VALUES(?1,?2)",
        params![published.id(), proof_sha],
    )
    .unwrap();
    db.execute("INSERT INTO archive_copies(operation_id,project_id,proof_sha256) VALUES('synthetic-copy-admission',?1,?2)",params![published.id(),proof_sha]).unwrap();
    let before_control = Snapshot::capture(&f);
    let (control, _) = budget(InspectionLimits::default());
    let accepted = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &control)
        .unwrap();
    assert_eq!(
        (
            accepted.version,
            accepted.snapshots,
            accepted.history_rows,
            accepted.operation_rows,
            accepted.lineage_proof_rows,
            accepted.distinct_bindings
        ),
        (1, 5, 1, 1, 1, 1)
    );
    before_control.unchanged(&f);

    let false_result = false_pitch(&published);
    let (false_sha, false_json) = synthetic_proof(&false_result);
    assert_ne!(false_sha, proof_sha);
    for table in [
        "archive_copy_proofs",
        "archive_project_lineage",
        "archive_copies",
    ] {
        remove_immutable_triggers(&db, table);
    }
    db.execute(
        "INSERT INTO archive_copy_proofs(proof_sha256,receipt_json) VALUES(?1,?2)",
        params![false_sha, false_json],
    )
    .unwrap();
    db.execute(
        "UPDATE archive_project_lineage SET proof_sha256=?1 WHERE proof_sha256=?2",
        params![false_sha, proof_sha],
    )
    .unwrap();
    db.execute(
        "UPDATE archive_copies SET proof_sha256=?1 WHERE proof_sha256=?2",
        params![false_sha, proof_sha],
    )
    .unwrap();
    db.execute(
        "DELETE FROM archive_copy_proofs WHERE proof_sha256=?1",
        [&proof_sha],
    )
    .unwrap();
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    semantic_refusal(
        f.store
            .inspect_retained_projection_closure(f.project.id(), &governor),
    );
    before.unchanged(&f);
}

#[test]
fn genuinely_source_valid_proof_only_new_binding_requires_a_live_typed_publication() {
    let mut f = ClosureFixture::new(true);
    let (intent, published) = f.publish();
    let current = f
        .store
        .apply_edit(&edit(
            &published,
            "advance-for-proof",
            "Current authored revision",
        ))
        .unwrap()
        .document;
    let reference = published
        .performance_projections()
        .unwrap()
        .unwrap()
        .entries[0]
        .source
        .clone();
    let choices = [lane_choice("proof-only-new-lane")];
    let binding = {
        let (governor, _) = budget(InspectionLimits::default());
        let source = f
            .store
            .open_take_with_budget(&reference, &governor)
            .unwrap();
        let held = AuthenticatedPerformance::open(&source).unwrap();
        let binding = held
            .build_binding(
                "proof-only-projection",
                &intent.request,
                &intent.expected_review_sha256,
                &choices,
            )
            .unwrap();
        held.validate_binding(&binding).unwrap();
        binding
    };
    let proof_result = current
        .append_performance_projection(&binding, &choices, NoteTimelinePolicy::PreserveLoop, false)
        .unwrap();
    assert_eq!(proof_result.revision(), current.revision());
    assert_eq!(
        proof_result
            .performance_projections()
            .unwrap()
            .unwrap()
            .entries
            .len(),
        2
    );
    let absent = projection_operation_id(&ProjectId(current.id().into()), &binding.id).unwrap();
    let exists: bool = f
        .db()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)",
            [&absent.0],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!exists);
    attach_synthetic_proof(&f, &proof_result);
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    let result = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &governor);
    assert!(
        matches!(&result,Err(StoreError::Invalid(message)) if message=="Retained binding has no live deterministic publication row"),
        "The valid proof-only binding must fail specifically at live publication linkage: {result:?}"
    );
    before.unchanged(&f);
}

#[test]
fn every_retained_binding_requires_its_live_deterministic_typed_publication_row() {
    for delete_row in [true, false] {
        let mut f = ClosureFixture::new(true);
        let (intent, published) = f.publish();
        let db = f.db();
        remove_immutable_triggers(&db, "operations");
        if delete_row {
            db.execute(
                "DELETE FROM operations WHERE operation_id=?1",
                [&intent.operation_id.0],
            )
            .unwrap();
        } else {
            // A canonical ordinary Rename with a legacy reserved-prefix ID
            // stays ordinary. It cannot stand in for the projection receipt.
            let ordinary = ProjectEditIntent {
                operation_id: intent.operation_id.clone(),
                project_id: intent.project_id.clone(),
                expected_revision: intent.expected_revision,
                scope: EditScope::Project,
                edit: EditOperation::Rename {
                    name: published.name().into(),
                },
                extensions: Default::default(),
            };
            let request = serde_json::to_string(&ordinary).unwrap();
            db.execute(
                "UPDATE operations SET request_json=?1,request_sha256=?2 WHERE operation_id=?3",
                params![request, hash(request.as_bytes()), intent.operation_id.0],
            )
            .unwrap();
        }
        let before = Snapshot::capture(&f);
        let (governor, _) = budget(InspectionLimits::default());
        semantic_refusal(
            f.store
                .inspect_retained_projection_closure(f.project.id(), &governor),
        );
        before.unchanged(&f);
    }
}

#[test]
fn missing_or_replaced_owned_original_refuses_without_repairing_or_adopting_files() {
    for missing in [true, false] {
        let mut f = ClosureFixture::new(true);
        f.publish();
        let parked = f.root.join("parked-owned.sqlite3");
        fs::rename(&f.owned, &parked).unwrap();
        let replacement = if missing {
            None
        } else {
            Some(ClosureFixture::new(false))
        };
        if let Some(other) = &replacement {
            fs::copy(&other.owned, &f.owned).unwrap();
        }
        let before = Snapshot::capture(&f);
        let (governor, _) = budget(InspectionLimits::default());
        let result = f
            .store
            .inspect_retained_projection_closure(f.project.id(), &governor);
        if missing {
            assert!(
                matches!(&result,Err(StoreError::Io(error)) if error.kind()==std::io::ErrorKind::NotFound),
                "Missing owned original must remain unavailable: {result:?}"
            );
        } else {
            semantic_refusal(result);
        }
        before.unchanged(&f);
    }
}

#[test]
fn sticky_cancellation_and_metadata_work_or_sql_vm_limits_leave_every_cell_and_file_exact() {
    let mut f = ClosureFixture::new(true);
    f.publish();
    let before = Snapshot::capture(&f);
    let (cancelled, token) = budget(InspectionLimits::default());
    token.store(true, Ordering::Release);
    assert!(matches!(
        f.store
            .inspect_retained_projection_closure(f.project.id(), &cancelled),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    assert_eq!(cancelled.usage(), Default::default());
    let mut stopped = Vec::new();
    for (limits, expected) in [
        (
            InspectionLimits {
                max_work_bytes: 1,
                ..InspectionLimits::default()
            },
            InspectionResource::WorkBytes,
        ),
        (
            InspectionLimits {
                max_vm_steps: 1,
                ..InspectionLimits::default()
            },
            InspectionResource::VmSteps,
        ),
    ] {
        let (governor, _) = budget(limits);
        let result = f
            .store
            .inspect_retained_projection_closure(f.project.id(), &governor);
        assert!(
            matches!(&result,Err(StoreError::Recording(RecordingError::InspectionLimit{resource,..})) if *resource==expected),
            "Expected exact bounded diagnostic stop: {result:?}"
        );
        assert!(
            matches!(governor.checkpoint(),Err(RecordingError::InspectionLimit{resource,..}) if resource==expected)
        );
        before.unchanged(&f);
        let usage = governor.usage();
        stopped.push((governor, usage));
    }
    let cancelled_usage = cancelled.usage();
    let (fresh, _) = budget(InspectionLimits::default());
    let accepted = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &fresh)
        .unwrap();
    assert_eq!(
        (
            accepted.snapshots,
            accepted.history_rows,
            accepted.operation_rows,
            accepted.distinct_bindings
        ),
        (4, 1, 1, 1)
    );
    assert!(fresh.usage().work_bytes > 0 && fresh.usage().vm_steps > 0);
    fresh.checkpoint().unwrap();
    assert_eq!(cancelled.usage(), cancelled_usage);
    for (old, usage) in stopped {
        assert_eq!(old.usage(), usage);
    }
    before.unchanged(&f);
}

#[test]
fn legacy_opaque_projection_lookalikes_never_become_native_source_ownership() {
    let mut f = ClosureFixture::new(true);
    let mut raw = ProjectDocument::new("Legacy opaque envelope")
        .unwrap()
        .value()
        .clone();
    raw["performanceProjections"] = json!({"version":1,"entries":[{"id":"looks-native","source":{"takeId":"missing-original","manifestSha256":"f".repeat(64),"streamId":"audio"},"review":{"canonicalJson":"not a kernel review","reviewSha256":"a".repeat(64)}}]});
    let legacy = ProjectDocument::from_value(raw).unwrap();
    assert_eq!(legacy.schema_version(), 1);
    assert!(legacy.referenced_takes().unwrap().is_empty());
    let legacy = f.store.create(&legacy).unwrap();
    let updated = f
        .store
        .apply_edit(&edit(
            &legacy,
            "legacy-name",
            "Opaque receipt remains ordinary",
        ))
        .unwrap()
        .document;
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    let result = f
        .store
        .inspect_retained_projection_closure(updated.id(), &governor)
        .unwrap();
    assert_eq!(
        (
            result.snapshots,
            result.history_rows,
            result.operation_rows,
            result.lineage_proof_rows,
            result.distinct_bindings
        ),
        (4, 1, 1, 0, 0)
    );
    assert_eq!(governor.usage().file_bytes, 0);
    before.unchanged(&f);
}

#[test]
fn forward_receipt_lookup_bounds_a_foreign_namespace_before_materializing_its_identity() {
    let mut f = ClosureFixture::new(true);
    let (intent, published) = f.publish();
    let db = f.db();
    remove_immutable_triggers(&db, "operations");
    // The selected operation scan excludes this foreign namespace. The
    // deterministic history lookup must still preflight its own raw row.
    // Keep actual SQL foreign keys valid; only the unselected row's metadata
    // identity is maliciously oversized.
    let foreign = "f".repeat(1024);
    db.execute(
        "INSERT INTO projects(id,payload,revision,cursor) VALUES(?1,?2,0,0)",
        params![
            foreign,
            String::from_utf8(published.to_json().unwrap()).unwrap()
        ],
    )
    .unwrap();
    db.execute(
        "UPDATE operations SET project_id=?1 WHERE operation_id=?2",
        params![foreign, intent.operation_id.0],
    )
    .unwrap();
    let violations: i64 = db
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
    let before = Snapshot::capture(&f);
    let (governor, _) = budget(InspectionLimits::default());
    for result in [
        f.store
            .inspect_retained_projection_closure(f.project.id(), &governor)
            .map(|_| ()),
        f.store.projection_publication_receipt(&intent).map(|_| ()),
    ] {
        assert!(
            matches!(result, Err(StoreError::Invalid(message)) if message == "Recorded publication exceeds metadata bounds")
        );
    }
    before.unchanged(&f);
}
