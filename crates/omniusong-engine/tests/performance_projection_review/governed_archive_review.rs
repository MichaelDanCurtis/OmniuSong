//! Real device-free writer originals through the three bounded OSAR routes.
//! No native input/output, inference, one-auth counter or Windows claim.
use super::*;
use omniusong_core::{
    NoteTimelinePolicy, ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
    performance_projection::{ProjectionInstrument, ProjectionLaneChoice},
};
use omniusong_store::{
    ArchiveCompletion, ArchiveFailure, ArchivePhase, ArchiveReceipt, NativeStore,
    ProjectionPublication, RestoreCopyIntent, StoreError, projection_operation_id,
};
use rusqlite::{Connection, params, types::ValueRef};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

struct ArchiveFixture {
    original: Fixture,
    root: PathBuf,
    store: NativeStore,
    project: ProjectDocument,
    intent: ProjectionPublication,
}
impl ArchiveFixture {
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
            .join("../../.runtime/governed-projection-archive-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let mut store = NativeStore::open(root.join("source")).unwrap();
        let source = SealedJournal::open(&original.path).unwrap();
        store.ingest_take(&source).unwrap();
        drop(source);
        // Hash-addressed synthetic legacy bytes, with no WAV decoding claim.
        let asset = b"ordinary retained synthetic asset";
        let asset_id = format!("{}.wav", sha(asset));
        db(&store)
            .execute(
                "INSERT INTO assets VALUES(?1,?2)",
                params![asset_id, asset.as_slice()],
            )
            .unwrap();
        let mut value = ProjectDocument::new("Bounded human original")
            .unwrap()
            .value()
            .clone();
        value["com.example.retained"] =
            json!({"project_id":"opaque-not-remapped","unicode":"π", "array":[3,2,1]});
        value["tracks"][0]["assetId"] = asset_id.into();
        let project = store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap();
        let mut rename = edit(&project, "ordinary-with-opaque-fields", "Before projection");
        rename
            .extensions
            .insert("projection_id".into(), json!("opaque ordinary extension"));
        rename.extensions.insert(
            "opaque".into(),
            json!({"source":{"take_id":"not-a-reference"}}),
        );
        let project = store.apply_edit(&rename).unwrap().document;
        let reference = TakeReference {
            take_id: original.config.take_id.clone(),
            manifest_sha256: original.request().source.manifest_sha256,
            stream_id: "audio".into(),
        };
        let (governor, _) = budget(InspectionLimits::default());
        let reader = store.open_take_with_budget(&reference, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let mut request = original.request();
        request.source = held.source_pin().clone();
        request.sources = vec![1];
        request.tempo_bits = project.tempo().to_bits();
        let review = held.project(&request).unwrap();
        assert_eq!(review.notes.len(), 1);
        assert_eq!(review.notes[0].values.velocity16, 0);
        let project_id = ProjectId(project.id().into());
        let intent = ProjectionPublication {
            version: 1,
            operation_id: projection_operation_id(&project_id, "projection-main").unwrap(),
            project_id,
            expected_revision: project.revision(),
            projection_id: "projection-main".into(),
            request,
            expected_review_sha256: review.review_sha256,
            lanes: vec![ProjectionLaneChoice {
                source: 1,
                instrument: ProjectionInstrument::Keys,
                track_id: "retained-note-lane".into(),
                track_name: "Original zero-velocity note".into(),
            }],
            timeline: NoteTimelinePolicy::PreserveLoop,
            allow_projection_schema_upgrade: true,
        };
        drop(held);
        drop(reader);
        let (governor, _) = budget(InspectionLimits::default());
        store
            .publish_performance_projection(&intent, &governor)
            .unwrap();
        let project = store.load(project.id()).unwrap();
        Self {
            original,
            root,
            store,
            project,
            intent,
        }
    }
    fn export(&self, name: &str) -> (PathBuf, ArchiveReceipt) {
        export(&self.store, self.project.id(), &self.root.join(name))
    }
    fn add_ordinary_audio(&mut self) {
        use omniusong_engine::{
            capture::CaptureJournal,
            realtime::{AudioPacket, MAX_CHANNELS, PACKET_FRAMES},
        };
        let path = self.root.join("separate-audio.sqlite3");
        let descriptor = serde_json::from_value(json!({
            "version":1,"take_id":uuid::Uuid::new_v4().to_string(),"streams":[{
                "stream_id":"audio","segment_id":"ordinary-segment","source_id":"synthetic-packets",
                "source_name":"Device-free ordinary source","source_connection_generation":1,
                "host_clock":{"domain":"ordinary-clock","epoch":"ordinary-epoch","ticks_per_second_numerator":1000000000,"ticks_per_second_denominator":1},
                "kind":{"kind":"audio","format":{"sample_rate":8000,"channels":1,"encoding":"f32_le","layout":"interleaved"},"audio_epoch":"ordinary-epoch"},"initial_source_position":0
            }],"provenance":{"synthetic":true}
        })).unwrap();
        let mut writer = CaptureJournal::create(&path, descriptor).unwrap();
        let mut start = 0;
        for index in 0..8 {
            let frames = if index == 7 {
                208
            } else {
                PACKET_FRAMES as u16
            };
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
        drop(writer);
        CaptureReader::open(&path)
            .unwrap()
            .recover("governed-review.audio-recovered")
            .unwrap();
        let source = SealedJournal::open(&path).unwrap();
        assert_eq!(source.descriptor().streams.len(), 1);
        let object = self.store.ingest_take(&source).unwrap();
        drop(source);
        let mut current = self.project.clone();
        current
            .append_take_clip(
                "ordinary-originals",
                "Separate original audio",
                TakeClip {
                    id: "ordinary-audio-clip".into(),
                    source: TakeReference {
                        take_id: object.take_id,
                        manifest_sha256: object.manifest_sha256,
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames {
                        start: 0,
                        count: 2000,
                    },
                    position: TimelinePosition {
                        frame: 0,
                        sample_rate: 8000,
                    },
                    fades: None,
                    edit_source: None,
                },
            )
            .unwrap();
        self.project = self.store.save(&current, self.project.revision()).unwrap();
    }
}
impl Drop for ArchiveFixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Governed archive fixture retained at {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn db(store: &NativeStore) -> Connection {
    Connection::open(store.root().join("studio.sqlite3")).unwrap()
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
fn copy_intent(receipt: &ArchiveReceipt, id: &str) -> RestoreCopyIntent {
    RestoreCopyIntent {
        operation_id: OperationId(id.into()),
        expected_archive_sha256: receipt.sha256.clone(),
    }
}
fn export(store: &NativeStore, project: &str, path: &Path) -> (PathBuf, ArchiveReceipt) {
    let (budget, _) = budget(InspectionLimits::default());
    let outcome = store
        .export_archive_bounded_outcome(project, path, &budget)
        .unwrap();
    assert_eq!(outcome.completion, ArchiveCompletion::ExportDurable);
    assert_eq!(sha(&fs::read(path).unwrap()), outcome.value.sha256);
    (path.into(), outcome.value)
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
    schema: Vec<Vec<Cell>>,
    tables: BTreeMap<String, Vec<Vec<Cell>>>,
}
fn catalog(store: &NativeStore) -> Catalog {
    let db = db(store);
    let mut names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    names.push("sqlite_schema".into());
    let mut tables = BTreeMap::new();
    for name in names {
        let mut statement = db
            .prepare(&format!(
                "SELECT * FROM \"{}\" NOT INDEXED",
                name.replace('"', "\"\"")
            ))
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
        tables.insert(name, values);
    }
    let schema = tables.remove("sqlite_schema").unwrap();
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
#[derive(Debug, PartialEq, Eq)]
struct Original {
    manifest: String,
    object_sha: String,
    byte_length: i64,
    manifest_json: String,
    bytes: Vec<u8>,
}
fn originals(store: &NativeStore) -> BTreeMap<String, Original> {
    let db = db(store);
    let mut statement = db.prepare("SELECT take_id,manifest_sha256,object_sha256,byte_length,manifest_json FROM native_takes ORDER BY take_id").unwrap();
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.into_iter()
        .map(|(id, manifest, object_sha, byte_length, manifest_json)| {
            let bytes = fs::read(
                store
                    .root()
                    .join("takes")
                    .join(format!("{object_sha}.sqlite3")),
            )
            .unwrap();
            assert_eq!(bytes.len() as i64, byte_length);
            assert_eq!(sha(&bytes), object_sha);
            (
                id,
                Original {
                    manifest,
                    object_sha,
                    byte_length,
                    manifest_json,
                    bytes,
                },
            )
        })
        .collect()
}
fn assert_copy_document(source: &ProjectDocument, copied: &ProjectDocument) {
    assert_ne!(source.id(), copied.id());
    let mut normalized = copied.value().clone();
    normalized["id"] = source.id().into();
    assert_eq!(ProjectDocument::from_value(normalized).unwrap(), *source);
    assert_eq!(
        source.performance_projections().unwrap(),
        copied.performance_projections().unwrap()
    );
}
fn assert_live_mappings(store: &NativeStore, project: &str, source_intent: &ProjectionPublication) {
    let db = db(store);
    assert!(
        db.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none()
    );
    let operation =
        projection_operation_id(&ProjectId(project.into()), &source_intent.projection_id).unwrap();
    let request: String = db
        .query_row(
            "SELECT request_json FROM operations WHERE operation_id=?1 AND project_id=?2",
            params![operation.0, project],
            |r| r.get(0),
        )
        .unwrap();
    let mut expected = source_intent.clone();
    expected.project_id = ProjectId(project.into());
    expected.operation_id = operation;
    assert_eq!(request, expected.canonical_json().unwrap());
    let broken: i64 = db.query_row("SELECT count(*) FROM archive_copies a JOIN archive_copy_proof_operations m ON m.proof_sha256=a.proof_sha256 LEFT JOIN operations o ON o.operation_id=m.destination_operation_id AND o.project_id=a.project_id WHERE a.project_id=?1 AND (o.operation_id IS NULL OR o.request_sha256!=m.destination_request_sha256 OR o.result_sha256!=m.destination_result_sha256)", [project], |r| r.get(0)).unwrap();
    assert_eq!(broken, 0);
    let copied_ordinary: String = db.query_row("SELECT request_json FROM operations WHERE project_id=?1 AND operation_id!=?2 AND json_extract(request_json,'$.projection_id')='opaque ordinary extension'", params![project, expected.operation_id.0], |r| r.get(0)).unwrap();
    let ordinary: ProjectEditIntent = serde_json::from_str(&copied_ordinary).unwrap();
    assert_eq!(
        ordinary.extensions["projection_id"],
        json!("opaque ordinary extension")
    );
    assert_eq!(
        ordinary.extensions["opaque"],
        json!({"source":{"take_id":"not-a-reference"}})
    );
    let (budget, _) = budget(InspectionLimits::default());
    store
        .inspect_retained_projection_closure(project, &budget)
        .unwrap();
}

#[test]
fn actual_v1_v2_projection_audio_and_opaque_fields_survive_bounded_identity_and_two_copy_generations()
 {
    for v2 in [false, true] {
        let mut f = ArchiveFixture::new(v2);
        f.add_ordinary_audio();
        let before = catalog(&f.store);
        let source_originals = originals(&f.store);
        let staged = f.original.bytes();
        assert_eq!(source_originals.len(), 2);
        let (path, receipt) = f.export("generation-zero.osar");
        let mut identity = NativeStore::open(f.root.join("identity")).unwrap();
        let (budget, _) = budget(InspectionLimits::default());
        let restored = identity
            .restore_archive_bounded_outcome(&path, &budget)
            .unwrap();
        assert_eq!(restored.completion, ArchiveCompletion::CatalogCommitted);
        assert_eq!(restored.value, f.project);
        assert_eq!(identity.load(f.project.id()).unwrap(), f.project);
        assert_eq!(originals(&identity), source_originals);
        let mut first = NativeStore::open(f.root.join("copy-one")).unwrap();
        let (budget, _) = self::budget(InspectionLimits::default());
        let copied = first
            .restore_archive_as_copy_bounded_outcome(
                &path,
                &copy_intent(&receipt, "bounded-copy-one"),
                &budget,
            )
            .unwrap();
        assert_eq!(copied.completion, ArchiveCompletion::CatalogCommitted);
        assert!(!copied.value.replayed);
        assert_copy_document(&f.project, &copied.value.document);
        assert_eq!(originals(&first), source_originals);
        assert_live_mappings(&first, copied.value.document.id(), &f.intent);
        let (next_path, next_receipt) = export(
            &first,
            copied.value.document.id(),
            &f.root.join("generation-one.osar"),
        );
        let mut second = NativeStore::open(f.root.join("copy-two")).unwrap();
        let (budget, _) = self::budget(InspectionLimits::default());
        let leaf = second
            .restore_archive_as_copy_bounded_outcome(
                &next_path,
                &copy_intent(&next_receipt, "bounded-copy-two"),
                &budget,
            )
            .unwrap();
        assert_copy_document(&f.project, &leaf.value.document);
        assert_eq!(originals(&second), source_originals);
        assert_live_mappings(&second, leaf.value.document.id(), &f.intent);
        export(
            &second,
            leaf.value.document.id(),
            &f.root.join("generation-two.osar"),
        );
        // Undo the later audio save and projection, then prune their history.
        // The separate audio now survives only in real copy-proof results;
        // the projection still has its required deterministic operation row.
        let undo_audio = second
            .undo(leaf.value.document.id(), leaf.value.document.revision())
            .unwrap();
        let undo_projection = second.undo(undo_audio.id(), undo_audio.revision()).unwrap();
        let branch = second
            .apply_edit(&edit(
                &undo_projection,
                "ancestor-only-human-branch",
                "Ancestor-only originals retained",
            ))
            .unwrap()
            .document;
        assert!(branch.take_clips().unwrap().is_empty());
        assert!(
            branch
                .performance_projections()
                .unwrap()
                .is_none_or(|registry| registry.entries.is_empty())
        );
        let (ancestor_path, _) = export(&second, branch.id(), &f.root.join("ancestor-only.osar"));
        let mut ancestor_identity = NativeStore::open(f.root.join("ancestor-identity")).unwrap();
        let (budget, _) = self::budget(InspectionLimits::default());
        assert_eq!(
            ancestor_identity
                .restore_archive_bounded_outcome(&ancestor_path, &budget)
                .unwrap()
                .value,
            branch
        );
        assert_eq!(originals(&ancestor_identity), source_originals);
        let (budget, _) = self::budget(InspectionLimits::default());
        let closure = ancestor_identity
            .inspect_retained_projection_closure(branch.id(), &budget)
            .unwrap();
        assert_eq!(
            (closure.lineage_proof_rows, closure.distinct_bindings),
            (2, 1)
        );
        assert_eq!(catalog(&f.store), before);
        assert_eq!(originals(&f.store), source_originals);
        assert_eq!(f.original.bytes(), staged);
    }
}

fn primary(error: &StoreError) -> &StoreError {
    match error {
        StoreError::Archive(error) => primary(error.operation_error()),
        StoreError::Catalog(error) => primary(error.operation_error()),
        StoreError::ArchiveSql(error) => primary(error.operation_error()),
        other => other,
    }
}
fn precommit_refusal(error: &ArchiveFailure) {
    assert!(!error.publication_uncertain(), "{error:?}");
    assert!(!error.catalog_connection_unusable(), "{error:?}");
    assert!(!matches!(
        error.context().phase,
        ArchivePhase::CatalogCommitted | ArchivePhase::ExportDurable
    ));
}
fn remove_triggers(db: &Connection, table: &str) {
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1")
        .unwrap()
        .query_map([table], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for name in names {
        db.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))
            .unwrap();
    }
}
fn branch_after_undo(f: &mut ArchiveFixture) -> ProjectDocument {
    let published = f.project.clone();
    let undone = f.store.undo(published.id(), published.revision()).unwrap();
    assert!(
        undone
            .performance_projections()
            .unwrap()
            .is_none_or(|registry| registry.entries.is_empty())
    );
    f.project = f
        .store
        .apply_edit(&edit(
            &undone,
            "prune-projection-before-image",
            "New human branch",
        ))
        .unwrap()
        .document;
    assert!(
        f.project
            .performance_projections()
            .unwrap()
            .is_none_or(|registry| registry.entries.is_empty())
    );
    published
}

#[test]
fn bounded_archives_keep_undo_hidden_and_pruned_publication_results_with_real_copy_ancestry() {
    let mut f = ArchiveFixture::new(true);
    let original_registry = f.project.performance_projections().unwrap().unwrap();
    let undone = f.store.undo(f.project.id(), f.project.revision()).unwrap();
    f.project = undone;
    let (hidden_path, hidden_receipt) = f.export("undo-hidden.osar");
    let mut hidden = NativeStore::open(f.root.join("hidden-identity")).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    assert_eq!(
        hidden
            .restore_archive_bounded_outcome(&hidden_path, &governor)
            .unwrap()
            .value,
        f.project
    );
    let (governor, _) = budget(InspectionLimits::default());
    let retained = hidden
        .inspect_retained_projection_closure(f.project.id(), &governor)
        .unwrap();
    assert_eq!(retained.distinct_bindings, 1);
    f.project = f
        .store
        .apply_edit(&edit(
            &f.project,
            "pruned-after-undo",
            "Pruned human branch",
        ))
        .unwrap()
        .document;
    let (path, receipt) = f.export("pruned-operation-only.osar");
    let mut first = NativeStore::open(f.root.join("pruned-copy-one")).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    let copied = first
        .restore_archive_as_copy_bounded_outcome(
            &path,
            &copy_intent(&receipt, "pruned-copy-one"),
            &governor,
        )
        .unwrap()
        .value
        .document;
    assert_copy_document(&f.project, &copied);
    assert_live_mappings(&first, copied.id(), &f.intent);
    let request_id =
        projection_operation_id(&ProjectId(copied.id().into()), &f.intent.projection_id).unwrap();
    let raw: String = db(&first)
        .query_row(
            "SELECT result_json FROM operations WHERE operation_id=?1",
            [&request_id.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        ProjectDocument::from_json(raw.as_bytes())
            .unwrap()
            .performance_projections()
            .unwrap()
            .unwrap(),
        original_registry
    );
    let (next, next_receipt) = export(&first, copied.id(), &f.root.join("pruned-copy-one.osar"));
    let mut leaf = NativeStore::open(f.root.join("pruned-copy-two")).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    let copied_twice = leaf
        .restore_archive_as_copy_bounded_outcome(
            &next,
            &copy_intent(&next_receipt, "pruned-copy-two"),
            &governor,
        )
        .unwrap()
        .value
        .document;
    assert_copy_document(&f.project, &copied_twice);
    assert_live_mappings(&leaf, copied_twice.id(), &f.intent);
    let proofs: i64 = db(&leaf)
        .query_row(
            "SELECT count(*) FROM archive_project_lineage WHERE project_id=?1",
            [copied_twice.id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(proofs, 2);
    let (governor, _) = budget(InspectionLimits::default());
    let closure = leaf
        .inspect_retained_projection_closure(copied_twice.id(), &governor)
        .unwrap();
    assert_eq!(
        (closure.distinct_bindings, closure.lineage_proof_rows),
        (1, 2)
    );
    export(
        &leaf,
        copied_twice.id(),
        &f.root.join("pruned-leaf-reexport.osar"),
    );
    assert_eq!(originals(&leaf), originals(&f.store));
    assert_eq!(sha(&fs::read(&hidden_path).unwrap()), hidden_receipt.sha256);
}

fn false_pitch(document: &ProjectDocument) -> ProjectDocument {
    let mut registry = document.performance_projections().unwrap().unwrap();
    let binding = &mut registry.entries[0];
    let mut review: PerformanceProjectionReview =
        serde_json::from_str(&binding.review.canonical_json).unwrap();
    review.notes[0].values.pitch = 61;
    review.review_sha256.clear();
    let mut digest = Sha256::new();
    digest.update(b"OmniuSong.performance-projection-review.v1\0");
    digest.update(serde_json::to_vec(&review).unwrap());
    review.review_sha256 = format!("{:x}", digest.finalize());
    binding.review.canonical_json = serde_json::to_string(&review).unwrap();
    binding.review.review_sha256 = review.review_sha256;
    binding.lanes[0].original_notes[0].values.pitch = 61;
    let track = binding.lanes[0].track_id.clone();
    let (governor, _) = budget(InspectionLimits::default());
    check_retained_binding_encoding_bounded(binding, &governor).unwrap();
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

#[test]
fn coherently_rehashed_false_music_in_an_operation_only_snapshot_refuses_actual_bounded_export() {
    let mut f = ArchiveFixture::new(true);
    let published = branch_after_undo(&mut f);
    let forged = false_pitch(&published);
    let registry = forged.performance_projections().unwrap().unwrap();
    let binding = &registry.entries[0];
    let (control, _) = budget(InspectionLimits::default());
    let reader = f
        .store
        .open_take_with_budget(&binding.source, &control)
        .unwrap();
    let held = AuthenticatedPerformance::open(&reader).unwrap();
    assert!(
        matches!(held.validate_binding(binding), Err(RecordingError::Invalid(message))
        if message == "Retained performance projection differs from the held original derivation")
    );
    drop(held);
    drop(reader);
    let mut intent = f.intent.clone();
    intent.expected_review_sha256 = binding.review.review_sha256.clone();
    let request = intent.canonical_json().unwrap();
    let result = String::from_utf8(forged.to_json().unwrap()).unwrap();
    let db = db(&f.store);
    remove_triggers(&db, "operations");
    db.execute("UPDATE operations SET request_json=?1,request_sha256=?2,result_json=?3,result_sha256=?4 WHERE operation_id=?5",
        params![request, sha(request.as_bytes()), result, sha(result.as_bytes()), intent.operation_id.0]).unwrap();
    let before = catalog(&f.store);
    let source = originals(&f.store);
    let staged = f.original.bytes();
    let output = f.root.join("false-music-never-published.osar");
    let (governor, _) = budget(InspectionLimits::default());
    let failure = f
        .store
        .export_archive_bounded_outcome(f.project.id(), &output, &governor)
        .unwrap_err();
    precommit_refusal(&failure);
    assert!(
        matches!(primary(failure.operation_error()), StoreError::Recording(RecordingError::Invalid(message))
        if message == "Retained performance projection differs from the held original derivation"),
        "{failure:?}"
    );
    assert!(!output.exists());
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), source);
    assert_eq!(f.original.bytes(), staged);
}

#[test]
fn pruned_initial_publication_still_requires_its_original_muted_lane_on_bounded_export() {
    let mut f = ArchiveFixture::new(false);
    let published = branch_after_undo(&mut f);
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
        .find(|track| track["id"] == lane)
        .unwrap()["muted"] = false.into();
    let changed = ProjectDocument::from_value(value).unwrap();
    let raw = String::from_utf8(changed.to_json().unwrap()).unwrap();
    let db = db(&f.store);
    remove_triggers(&db, "operations");
    db.execute(
        "UPDATE operations SET result_json=?1,result_sha256=?2 WHERE operation_id=?3",
        params![raw, sha(raw.as_bytes()), f.intent.operation_id.0],
    )
    .unwrap();
    let before = catalog(&f.store);
    let source = originals(&f.store);
    let staged = f.original.bytes();
    let output = f.root.join("unmuted-initial-never-published.osar");
    let (governor, _) = budget(InspectionLimits::default());
    let failure = f
        .store
        .export_archive_bounded_outcome(f.project.id(), &output, &governor)
        .unwrap_err();
    precommit_refusal(&failure);
    assert!(
        matches!(primary(failure.operation_error()), StoreError::Invalid(message)
        if message == "Recorded publication initial note lane differs"),
        "{failure:?}"
    );
    assert!(!output.exists());
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), source);
    assert_eq!(f.original.bytes(), staged);
}

#[test]
fn both_bounded_restores_rollback_registration_replacement_and_unrelated_asset_trigger_changes() {
    let f = ArchiveFixture::new(true);
    let (path, receipt) = f.export("trigger-source.osar");
    let input = fs::read(&path).unwrap();
    let source_catalog = catalog(&f.store);
    let source_objects = originals(&f.store);
    let staged = f.original.bytes();
    for copy in [false, true] {
        for registration in [false, true] {
            let name = format!("target-copy-{copy}-registration-{registration}");
            let mut target = NativeStore::open(f.root.join(name)).unwrap();
            let target_db = db(&target);
            let baseline = b"unrelated destination asset";
            let id = format!("{}.wav", sha(baseline));
            target_db
                .execute(
                    "INSERT INTO assets VALUES(?1,?2)",
                    params![id, baseline.as_slice()],
                )
                .unwrap();
            let trigger = if registration {
                format!(
                    "CREATE TRIGGER alter_destination AFTER INSERT ON projects BEGIN INSERT OR REPLACE INTO native_takes SELECT take_id,manifest_sha256,'{}',byte_length,manifest_json FROM native_takes; END;",
                    "f".repeat(64)
                )
            } else {
                "CREATE TRIGGER alter_destination AFTER INSERT ON projects BEGIN UPDATE assets SET payload=zeroblob(length(payload)); END;".into()
            };
            target_db.execute_batch(&trigger).unwrap();
            let before = catalog(&target);
            let (governor, _) = budget(InspectionLimits::default());
            let result = if copy {
                target
                    .restore_archive_as_copy_bounded_outcome(
                        &path,
                        &copy_intent(&receipt, "trigger-copy"),
                        &governor,
                    )
                    .map(|_| ())
            } else {
                target
                    .restore_archive_bounded_outcome(&path, &governor)
                    .map(|_| ())
            };
            let failure = result.unwrap_err();
            precommit_refusal(&failure);
            assert!(
                matches!(
                    primary(failure.operation_error()),
                    StoreError::Invalid(_) | StoreError::Recording(RecordingError::Invalid(_))
                ),
                "{failure:?}"
            );
            assert_eq!(catalog(&target), before);
            assert!(originals(&target).is_empty());
            target_db
                .execute_batch("DROP TRIGGER alter_destination")
                .unwrap();
            let (fresh, _) = budget(InspectionLimits::default());
            let completion = if copy {
                target
                    .restore_archive_as_copy_bounded_outcome(
                        &path,
                        &copy_intent(&receipt, "trigger-copy"),
                        &fresh,
                    )
                    .unwrap()
                    .completion
            } else {
                target
                    .restore_archive_bounded_outcome(&path, &fresh)
                    .unwrap()
                    .completion
            };
            assert_eq!(completion, ArchiveCompletion::CatalogCommitted);
            assert_eq!(originals(&target), source_objects);
        }
    }
    assert_eq!(fs::read(&path).unwrap(), input);
    assert_eq!(catalog(&f.store), source_catalog);
    assert_eq!(originals(&f.store), source_objects);
    assert_eq!(f.original.bytes(), staged);
}

#[test]
fn an_existing_wrong_owned_object_is_never_overwritten_or_adopted_by_bounded_restore() {
    let f = ArchiveFixture::new(true);
    let (path, _) = f.export("wrong-owned-source.osar");
    let source = originals(&f.store);
    let object = source.values().next().unwrap();
    let input = fs::read(&path).unwrap();
    let mut target = NativeStore::open(f.root.join("wrong-owned-target")).unwrap();
    fs::create_dir_all(target.root().join("takes")).unwrap();
    let wrong = target
        .root()
        .join("takes")
        .join(format!("{}.sqlite3", object.object_sha));
    let wrong_bytes = vec![0x5a; object.byte_length as usize];
    fs::write(&wrong, &wrong_bytes).unwrap();
    let before = catalog(&target);
    let (governor, _) = budget(InspectionLimits::default());
    let failure = target
        .restore_archive_bounded_outcome(&path, &governor)
        .unwrap_err();
    precommit_refusal(&failure);
    assert_eq!(failure.catalog_sqlite_error_code(), None);
    assert_eq!(catalog(&target), before);
    assert_eq!(fs::read(&wrong).unwrap(), wrong_bytes);
    assert_eq!(fs::read(&path).unwrap(), input);
    fs::remove_file(&wrong).unwrap();
    let (fresh, _) = budget(InspectionLimits::default());
    assert_eq!(
        target
            .restore_archive_bounded_outcome(&path, &fresh)
            .unwrap()
            .value,
        f.project
    );
    assert_eq!(originals(&target), source);
}

#[test]
fn exact_bounded_copy_replay_needs_no_input_but_missing_owned_source_stays_a_recorded_refusal() {
    let f = ArchiveFixture::new(true);
    let (path, receipt) = f.export("replay-source.osar");
    let intent = copy_intent(&receipt, "bounded-recorded-copy");
    let mut target = NativeStore::open(f.root.join("replay-target")).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    let original = target
        .restore_archive_as_copy_bounded_outcome(&path, &intent, &governor)
        .unwrap()
        .value
        .document;
    let changed = target
        .apply_edit(&edit(&original, "later-human-edit", "Later human edit"))
        .unwrap()
        .document;
    fs::rename(&path, f.root.join("parked-replay-input.osar")).unwrap();
    assert!(!path.exists());
    let before = catalog(&target);
    let native = originals(&target);
    let staged = f.original.bytes();
    let (fresh, _) = budget(InspectionLimits::default());
    let replay = target
        .restore_archive_as_copy_bounded_outcome(&path, &intent, &fresh)
        .unwrap();
    assert_eq!(replay.completion, ArchiveCompletion::RecordedReplay);
    assert!(replay.value.replayed);
    assert_eq!(replay.value.document, original);
    assert_eq!(target.load(changed.id()).unwrap(), changed);
    assert_eq!(catalog(&target), before);
    assert_eq!(originals(&target), native);
    let object = native.values().next().unwrap();
    let owned = target
        .root()
        .join("takes")
        .join(format!("{}.sqlite3", object.object_sha));
    let parked = f.root.join("parked-owned-original.sqlite3");
    fs::rename(&owned, &parked).unwrap();
    let (fresh, _) = budget(InspectionLimits::default());
    let failure = target
        .restore_archive_as_copy_bounded_outcome(&path, &intent, &fresh)
        .unwrap_err();
    assert_eq!(failure.context().phase, ArchivePhase::RecordedReplay);
    assert!(failure.context().staging.is_none());
    assert_eq!(
        failure.context().operation_id.as_deref(),
        Some(intent.operation_id.0.as_str())
    );
    assert_eq!(failure.catalog_sqlite_error_code(), None);
    assert!(!failure.catalog_connection_unusable());
    assert_eq!(catalog(&target), before);
    assert!(!owned.exists() && !path.exists());
    assert_eq!(fs::read(&parked).unwrap(), object.bytes);
    assert!(target.load(changed.id()).is_err());
    assert_eq!(catalog(&target), before);
    fs::rename(&parked, &owned).unwrap();
    let (fresh, _) = budget(InspectionLimits::default());
    assert!(
        target
            .restore_archive_as_copy_bounded_outcome(&path, &intent, &fresh)
            .unwrap()
            .value
            .replayed
    );
    assert_eq!(catalog(&target), before);
    assert_eq!(originals(&target), native);
    assert_eq!(f.original.bytes(), staged);
}

#[test]
fn strict_osar_v1_without_lineage_tables_accepts_genuine_performance_profiles_in_both_restores() {
    for v2 in [false, true] {
        let f = ArchiveFixture::new(v2);
        let (path, _) = f.export("strict-osar-v1.osar");
        {
            let archive = Connection::open(&path).unwrap();
            archive.execute_batch("DROP TABLE archive_copies; DROP TABLE archive_project_lineage; DROP TABLE archive_copy_proof_operations; DROP TABLE archive_copy_proofs; PRAGMA user_version=1;").unwrap();
        }
        let input = fs::read(&path).unwrap();
        let expected = sha(&input);
        let source = originals(&f.store);
        let mut identity = NativeStore::open(f.root.join("strict-v1-identity")).unwrap();
        let (governor, _) = budget(InspectionLimits::default());
        assert_eq!(
            identity
                .restore_archive_bounded_outcome(&path, &governor)
                .unwrap()
                .value,
            f.project
        );
        assert_eq!(originals(&identity), source);
        let mut copied = NativeStore::open(f.root.join("strict-v1-copy")).unwrap();
        let (governor, _) = budget(InspectionLimits::default());
        let receipt = copied
            .restore_archive_as_copy_bounded_outcome(
                &path,
                &RestoreCopyIntent {
                    operation_id: OperationId("strict-v1-copy".into()),
                    expected_archive_sha256: expected,
                },
                &governor,
            )
            .unwrap();
        assert_copy_document(&f.project, &receipt.value.document);
        assert_eq!(originals(&copied), source);
        assert_live_mappings(&copied, receipt.value.document.id(), &f.intent);
        assert_eq!(fs::read(&path).unwrap(), input);
    }
}

#[test]
fn all_three_actual_routes_keep_typed_stops_and_reuse_the_same_store_with_fresh_governors() {
    let f = ArchiveFixture::new(true);
    let (path, receipt) = f.export("limit-source.osar");
    let input = fs::read(&path).unwrap();
    let source = originals(&f.store);
    let staged = f.original.bytes();
    for route in 0..3 {
        let mut target = NativeStore::open(f.root.join(format!("limit-target-{route}"))).unwrap();
        let before = if route == 0 {
            catalog(&f.store)
        } else {
            catalog(&target)
        };
        let mut stopped = Vec::new();
        for expected in [
            None,
            Some(InspectionResource::WorkBytes),
            Some(InspectionResource::VmSteps),
        ] {
            let limits = match expected {
                Some(InspectionResource::WorkBytes) => InspectionLimits {
                    max_work_bytes: 1,
                    ..Default::default()
                },
                Some(InspectionResource::VmSteps) => InspectionLimits {
                    max_vm_steps: 1,
                    ..Default::default()
                },
                _ => InspectionLimits::default(),
            };
            let (governor, cancel) = budget(limits);
            if expected.is_none() {
                cancel.store(true, Ordering::Release);
            }
            let result = match route {
                0 => f
                    .store
                    .export_archive_bounded_outcome(
                        f.project.id(),
                        &f.root.join("limited-never-published.osar"),
                        &governor,
                    )
                    .map(|_| ()),
                1 => target
                    .restore_archive_bounded_outcome(&path, &governor)
                    .map(|_| ()),
                _ => target
                    .restore_archive_as_copy_bounded_outcome(
                        &path,
                        &copy_intent(&receipt, "limited-copy"),
                        &governor,
                    )
                    .map(|_| ()),
            };
            let failure = result.unwrap_err();
            precommit_refusal(&failure);
            match (expected, failure.governor_stop()) {
                (None, Some(RecordingError::InspectionCancelled)) => {}
                (Some(wanted), Some(RecordingError::InspectionLimit { resource, .. })) => {
                    assert_eq!(*resource, wanted)
                }
                _ => panic!("Missing typed independent governor stop: {failure:?}"),
            }
            assert_eq!(
                if route == 0 {
                    catalog(&f.store)
                } else {
                    catalog(&target)
                },
                before
            );
            assert_eq!(originals(&f.store), source);
            assert_eq!(f.original.bytes(), staged);
            assert_eq!(fs::read(&path).unwrap(), input);
            let usage = governor.usage();
            stopped.push((governor, usage));
        }
        let (fresh, _) = budget(InspectionLimits::default());
        let completion = match route {
            0 => {
                f.store
                    .export_archive_bounded_outcome(
                        f.project.id(),
                        &f.root.join("fresh-after-stops.osar"),
                        &fresh,
                    )
                    .unwrap()
                    .completion
            }
            1 => {
                target
                    .restore_archive_bounded_outcome(&path, &fresh)
                    .unwrap()
                    .completion
            }
            _ => {
                target
                    .restore_archive_as_copy_bounded_outcome(
                        &path,
                        &copy_intent(&receipt, "limited-copy"),
                        &fresh,
                    )
                    .unwrap()
                    .completion
            }
        };
        assert_eq!(
            completion,
            if route == 0 {
                ArchiveCompletion::ExportDurable
            } else {
                ArchiveCompletion::CatalogCommitted
            }
        );
        for (old, usage) in stopped {
            assert_eq!(old.usage(), usage);
        }
        if route != 0 {
            assert_eq!(originals(&target), source);
        }
    }
}

#[test]
fn export_work_credit_is_cumulative_across_calls_without_raising_compiled_ceilings() {
    let f = ArchiveFixture::new(false);
    let before = catalog(&f.store);
    let source = originals(&f.store);
    let (control, _) = budget(InspectionLimits::default());
    f.store
        .export_archive_bounded_outcome(f.project.id(), &f.root.join("A.osar"), &control)
        .unwrap();
    let ceiling = control.usage().work_bytes.checked_add(131072).unwrap();
    assert!(ceiling < InspectionLimits::default().max_work_bytes);
    let (shared, _) = budget(InspectionLimits {
        max_work_bytes: ceiling,
        ..Default::default()
    });
    f.store
        .export_archive_bounded_outcome(f.project.id(), &f.root.join("B.osar"), &shared)
        .unwrap();
    let failure = f
        .store
        .export_archive_bounded_outcome(f.project.id(), &f.root.join("C.osar"), &shared)
        .unwrap_err();
    precommit_refusal(&failure);
    assert!(matches!(
        failure.governor_stop(),
        Some(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        })
    ));
    assert!(!f.root.join("C.osar").exists());
    let stopped = shared.usage();
    let (fresh, _) = budget(InspectionLimits::default());
    f.store
        .export_archive_bounded_outcome(f.project.id(), &f.root.join("D.osar"), &fresh)
        .unwrap();
    assert_eq!(shared.usage(), stopped);
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), source);
    // Usage here proves cumulative refusal/reuse, not a count of source opens.
}

#[test]
fn existing_legacy_archive_routes_still_refuse_current_and_hidden_projection_authority() {
    let mut f = ArchiveFixture::new(true);
    let (path, receipt) = f.export("bounded-only.osar");
    let before = catalog(&f.store);
    let source = originals(&f.store);
    let input = fs::read(&path).unwrap();
    let error = f
        .store
        .export_archive(f.project.id(), &f.root.join("legacy-never-published.osar"))
        .unwrap_err();
    assert!(error.to_string().to_lowercase().contains("projection"));
    assert_eq!(catalog(&f.store), before);
    let mut identity = NativeStore::open(f.root.join("legacy-identity")).unwrap();
    let identity_before = catalog(&identity);
    assert!(
        identity
            .restore_archive(&path)
            .unwrap_err()
            .to_string()
            .to_lowercase()
            .contains("projection")
    );
    assert_eq!(catalog(&identity), identity_before);
    let mut copied = NativeStore::open(f.root.join("legacy-copy")).unwrap();
    let copy_before = catalog(&copied);
    assert!(
        copied
            .restore_archive_as_copy(&path, &copy_intent(&receipt, "legacy-refused-copy"))
            .unwrap_err()
            .to_string()
            .to_lowercase()
            .contains("projection")
    );
    assert_eq!(catalog(&copied), copy_before);
    branch_after_undo(&mut f);
    let hidden_before = catalog(&f.store);
    assert!(
        f.store
            .export_archive(
                f.project.id(),
                &f.root.join("legacy-hidden-never-published.osar")
            )
            .unwrap_err()
            .to_string()
            .to_lowercase()
            .contains("projection")
    );
    assert_eq!(catalog(&f.store), hidden_before);
    assert_eq!(originals(&f.store), source);
    assert_eq!(fs::read(&path).unwrap(), input);
}
