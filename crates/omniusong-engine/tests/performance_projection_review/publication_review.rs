//! Actual synthetic renderer/writer originals through the public typed store
//! publisher. Compact recorded commitment is distinct from active source proof.
#[path = "catalog_review.rs"]
mod catalog_review;
use super::*;
use omniusong_core::{
    DocumentNoteAddress, NoteTimelinePolicy, ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
    performance_projection::{ProjectionInstrument, ProjectionLaneChoice},
};
use omniusong_store::{
    NativeReader, NativeStore, ProjectionPublication, StoreError, projection_operation_id,
};
use rusqlite::{Connection, params, types::ValueRef};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

struct Harness {
    original: Fixture,
    root: PathBuf,
}
struct Prepared {
    store: NativeStore,
    project: ProjectDocument,
    intent: ProjectionPublication,
    owned: PathBuf,
}
impl Harness {
    fn new() -> Self {
        let original = Fixture::record(true, |tap, writer| {
            tap.admit(&[
                on(1, 1, 1, 0),
                event(
                    2,
                    1,
                    LiveEventKind::NoteOn {
                        note: 1,
                        voice: 1,
                        key: 48,
                        velocity16: 65535,
                        patch: LivePatch::Bass,
                    },
                ),
                event(
                    3,
                    1,
                    LiveEventKind::NoteOn {
                        note: 1,
                        voice: 1,
                        key: 36,
                        velocity16: 32768,
                        patch: LivePatch::Drums,
                    },
                ),
                event(
                    4,
                    1,
                    LiveEventKind::Pulse {
                        note: 1,
                        voice: 1,
                        key: 67,
                        velocity16: 41234,
                        release_velocity16: 54321,
                        patch: LivePatch::Pad,
                        duration_frames: 80,
                    },
                ),
            ]);
            tap.render(writer, 80);
            tap.admit(&[off(1, 1, 2, 0), off(2, 1, 2, 65535)]);
            tap.render(writer, 2000);
            tap.finish(writer, 64);
        });
        original.recover();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-projection-publication-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self { original, root }
    }
    fn db(&self) -> Connection {
        Connection::open(self.root.join("store/studio.sqlite3")).unwrap()
    }
    fn prepare(&self) -> Prepared {
        let mut store = NativeStore::open(self.root.join("store")).unwrap();
        let mut project = ProjectDocument::new("Unchanged human-authored destination").unwrap();
        project.set_tempo(120.0).unwrap();
        let project = store.create(&project).unwrap();
        let source = SealedJournal::open(&self.original.path).unwrap();
        let object = store.ingest_take(&source).unwrap();
        drop(source);
        let owned = store
            .root()
            .join("takes")
            .join(format!("{}.sqlite3", object.object_sha256));
        assert!(owned.is_file());
        let intent = self.intent(&store, &project, "projection-one", "first", 1000);
        assert_eq!(
            intent.request.source.journal_sha256_at_review, object.object_sha256,
            "Review the exact owned backup bytes, not the staged input filename"
        );
        Prepared {
            store,
            project,
            intent,
            owned,
        }
    }
    fn intent(
        &self,
        store: &NativeStore,
        project: &ProjectDocument,
        id: &str,
        prefix: &str,
        tick: u64,
    ) -> ProjectionPublication {
        let reference = omniusong_core::native::TakeReference {
            take_id: self.original.config.take_id.clone(),
            manifest_sha256: self.original.request().source.manifest_sha256,
            stream_id: "audio".into(),
        };
        let (governor, _) = budget(InspectionLimits::default());
        let reader = store.open_take_with_budget(&reference, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let mut request = self.original.request();
        request.source = held.source_pin().clone();
        request.tempo_bits = project.tempo().to_bits();
        request.destination_start_tick = tick;
        let review = held.project(&request).unwrap();
        assert_eq!(review.notes.len(), 4);
        let project_id = ProjectId(project.id().into());
        ProjectionPublication {
            version: 1,
            operation_id: projection_operation_id(&project_id, id).unwrap(),
            project_id,
            expected_revision: project.revision(),
            projection_id: id.into(),
            request,
            expected_review_sha256: review.review_sha256,
            lanes: choices(prefix),
            timeline: NoteTimelinePolicy::PreserveLoop,
            allow_projection_schema_upgrade: true,
        }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Publication review retained at {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn choices(prefix: &str) -> Vec<ProjectionLaneChoice> {
    [
        (1, ProjectionInstrument::Keys, "keys"),
        (2, ProjectionInstrument::Bass, "bass"),
        (3, ProjectionInstrument::Drums, "drums"),
        (4, ProjectionInstrument::Pad, "pad"),
    ]
    .into_iter()
    .map(|(source, instrument, name)| ProjectionLaneChoice {
        source,
        instrument,
        track_id: format!("{prefix}-{name}"),
        track_name: format!("Original {prefix} {name}"),
    })
    .collect()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn edit(project: &ProjectDocument, id: &str, action: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        scope: EditScope::Project,
        edit: action,
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
fn quoted(id: &str) -> String {
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
                .prepare(&format!("SELECT * FROM {}", quoted(&name)))
                .unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            let mut values = Vec::new();
            while let Some(row) = rows.next().unwrap() {
                let cells = (0..columns)
                    .map(|i| match row.get_ref(i).unwrap() {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(v) => Cell::Integer(v),
                        ValueRef::Real(v) => Cell::Real(v.to_bits()),
                        ValueRef::Text(v) => Cell::Text(v.to_vec()),
                        ValueRef::Blob(v) => Cell::Blob(v.to_vec()),
                    })
                    .collect();
                values.push(cells);
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
fn counts(db: &Connection) -> (i64, i64, i64) {
    db.query_row("SELECT (SELECT count(*) FROM projects),(SELECT count(*) FROM edits),(SELECT count(*) FROM operations)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap()
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
        db.execute_batch(&format!("DROP TRIGGER {}", quoted(&name)))
            .unwrap();
    }
}
fn assert_source_bytes(
    harness: &Harness,
    staged: &BTreeMap<PathBuf, Vec<u8>>,
    owned: &Path,
    bytes: &[u8],
) {
    assert_eq!(&harness.original.bytes(), staged);
    assert_eq!(fs::read(owned).unwrap(), bytes);
}

#[test]
fn actual_publication_commits_every_lane_one_revision_one_history_row_and_one_exact_receipt() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    assert!(
        p.store
            .projection_publication_receipt(&p.intent)
            .unwrap()
            .is_none()
    );
    assert_eq!(counts(&harness.db()), (1, 0, 0));
    let (governor, _) = budget(InspectionLimits::default());
    let receipt = p
        .store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    receipt.validate().unwrap();
    assert!(!receipt.replayed);
    assert_eq!(
        (
            receipt.previous_revision,
            receipt.committed_revision,
            receipt.lane_count,
            receipt.note_count
        ),
        (0, 1, 4, 4)
    );
    let current = p.store.load(p.project.id()).unwrap();
    assert_eq!((current.schema_version(), current.revision()), (3, 1));
    assert_eq!(
        &current.tracks()[..p.project.tracks().len()],
        p.project.tracks()
    );
    assert_eq!(current.tracks().len(), p.project.tracks().len() + 4);
    let registry = current.performance_projections().unwrap().unwrap();
    assert_eq!(registry.entries.len(), 1);
    let binding = &registry.entries[0];
    assert_eq!(binding.id, p.intent.projection_id);
    assert_eq!(
        binding.review.review_sha256,
        p.intent.expected_review_sha256
    );
    for lane in &binding.lanes {
        let track = current
            .tracks()
            .iter()
            .find(|track| track["id"] == lane.track_id)
            .unwrap();
        assert_eq!(track["muted"], true);
        assert_eq!(track["instrument"], lane.instrument.as_str());
        assert_eq!(track["notes"].as_array().unwrap().len(), 1);
        assert_eq!(
            track["notes"][0]["velocity16"],
            lane.original_notes[0].values.velocity16
        );
    }
    assert_eq!(binding.lanes[0].original_notes[0].values.velocity16, 0);
    assert_eq!(binding.lanes[1].original_notes[0].values.velocity16, 65535);
    let db = harness.db();
    let (before, after): (String, String) = db
        .query_row(
            "SELECT before_json,after_json FROM edits WHERE project_id=?1 AND sequence=1",
            [current.id()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(before.as_bytes(), p.project.to_json().unwrap());
    assert_eq!(after.as_bytes(), current.to_json().unwrap());
    let (request,result,request_sha,result_sha):(String,String,String,String) = db.query_row("SELECT request_json,result_json,request_sha256,result_sha256 FROM operations WHERE operation_id=?1",[&p.intent.operation_id.0],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(request, p.intent.canonical_json().unwrap());
    assert_eq!(result, after);
    assert_eq!(hash(request.as_bytes()), request_sha);
    assert_eq!(hash(result.as_bytes()), result_sha);
    assert_eq!(receipt.request_sha256, request_sha);
    assert_eq!(receipt.result_sha256, result_sha);
    assert_eq!(counts(&db), (1, 1, 1));
    let before_replay = catalog(&db);
    let mut recorded = receipt.clone();
    recorded.replayed = true;
    assert_eq!(
        p.store.projection_publication_receipt(&p.intent).unwrap(),
        Some(recorded.clone())
    );
    let (fresh, _) = budget(InspectionLimits::default());
    assert_eq!(
        p.store
            .publish_performance_projection(&p.intent, &fresh)
            .unwrap(),
        recorded
    );
    assert_eq!(catalog(&db), before_replay);
    assert_source_bytes(&harness, &staged, &p.owned, &original);
}

#[test]
fn public_musical_edits_delete_and_missing_lane_preserve_the_original_registry_and_source() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    let committed = p
        .store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let published = p.store.load(p.project.id()).unwrap();
    let registry = published.performance_projections().unwrap().unwrap();
    let lane = &registry.entries[0].lanes[0];
    let address = DocumentNoteAddress {
        track_id: lane.track_id.clone(),
        note_id: lane.original_notes[0].note_id.0.clone(),
    };
    let mut values = lane.original_notes[0].values;
    values.pitch = 71;
    values.start_tick += 17;
    values.duration_ticks = 31;
    values.velocity16 = 12345;
    let mut edited = published
        .update_note(&address, values, NoteTimelinePolicy::PreserveLoop)
        .unwrap();
    edited.rename("Later authored name").unwrap();
    edited.set_tempo(141.25).unwrap();
    let mut value = edited.value().clone();
    value["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|track| track["id"] == address.track_id)
        .unwrap()["instrument"] = "pad".into();
    let edited = p
        .store
        .save(
            &ProjectDocument::from_value(value).unwrap(),
            published.revision(),
        )
        .unwrap();
    assert_eq!(edited.performance_projections().unwrap().unwrap(), registry);
    let deleted = edited.delete_note(&address).unwrap();
    let mut value = deleted.value().clone();
    let missing = &registry.entries[0].lanes[1].track_id;
    value["tracks"]
        .as_array_mut()
        .unwrap()
        .retain(|track| track["id"] != *missing);
    let saved = p
        .store
        .save(
            &ProjectDocument::from_value(value).unwrap(),
            edited.revision(),
        )
        .unwrap();
    assert_eq!(saved.performance_projections().unwrap().unwrap(), registry);
    assert_eq!(p.store.load(saved.id()).unwrap(), saved);
    assert_eq!(
        saved.referenced_takes().unwrap(),
        [registry.entries[0].source.clone()].into()
    );
    let recorded = p
        .store
        .projection_publication_receipt(&p.intent)
        .unwrap()
        .unwrap();
    assert_eq!(recorded.committed_revision, committed.committed_revision);
    assert_eq!(recorded.result_sha256, committed.result_sha256);
    assert_source_bytes(&harness, &staged, &p.owned, &original);
}

#[test]
fn undo_removes_exact_append_and_redo_restores_it_as_one_history_operation() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let published = p.store.load(p.project.id()).unwrap();
    let undone = p.store.undo(p.project.id(), published.revision()).unwrap();
    let mut expected = p.project.value().clone();
    expected["revision"] = 2.into();
    assert_eq!(undone.value(), &expected);
    let redone = p.store.redo(p.project.id(), undone.revision()).unwrap();
    let mut expected = published.value().clone();
    expected["revision"] = 3.into();
    assert_eq!(redone.value(), &expected);
    assert_eq!(counts(&harness.db()), (1, 1, 1));
    assert!(
        p.store
            .projection_publication_receipt(&p.intent)
            .unwrap()
            .unwrap()
            .replayed
    );
    assert_source_bytes(&harness, &staged, &p.owned, &original);
}

#[test]
fn wrong_revision_tempo_layout_pin_review_or_upgrade_permission_never_writes_a_partial_publication()
{
    let harness = Harness::new();
    let mut p = harness.prepare();
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    let before = catalog(&harness.db());
    let mut wrong = Vec::new();
    let mut revision = p.intent.clone();
    revision.expected_revision += 1;
    wrong.push(revision);
    let mut tempo = p.intent.clone();
    tempo.request.tempo_bits = 121.0f64.to_bits();
    wrong.push(tempo);
    let mut layout = p.intent.clone();
    layout.lanes.pop();
    wrong.push(layout);
    let mut pin = p.intent.clone();
    pin.request.source.original_rows_sha256 = "0".repeat(64);
    wrong.push(pin);
    let mut review = p.intent.clone();
    review.expected_review_sha256 = "0".repeat(64);
    wrong.push(review);
    let mut permission = p.intent.clone();
    permission.allow_projection_schema_upgrade = false;
    wrong.push(permission);
    let mut collision = p.intent.clone();
    collision.lanes[0].track_id = p.project.tracks()[0]["id"].as_str().unwrap().into();
    wrong.push(collision);
    for intent in wrong {
        let (governor, _) = budget(InspectionLimits::default());
        assert!(
            p.store
                .publish_performance_projection(&intent, &governor)
                .is_err()
        );
        assert_eq!(catalog(&harness.db()), before);
        assert_source_bytes(&harness, &staged, &p.owned, &original);
    }
    assert!(
        p.store
            .projection_publication_receipt(&p.intent)
            .unwrap()
            .is_none()
    );
}

#[test]
fn generic_save_cannot_insert_drop_rewrite_reorder_or_downgrade_projection_ownership() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let one = p.store.load(p.project.id()).unwrap();
    let second = harness.intent(&p.store, &one, "projection-two", "second", 3000);
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&second, &governor)
        .unwrap();
    let current = p.store.load(p.project.id()).unwrap();
    let third = harness.intent(&p.store, &current, "projection-three", "third", 5000);
    let third_binding = {
        let reference = current.performance_projections().unwrap().unwrap().entries[0]
            .source
            .clone();
        let (governor, _) = budget(InspectionLimits::default());
        let reader = p
            .store
            .open_take_with_budget(&reference, &governor)
            .unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        held.build_binding(
            &third.projection_id,
            &third.request,
            &third.expected_review_sha256,
            &third.lanes,
        )
        .unwrap()
    };
    let mut attacks = Vec::new();
    attacks.push(
        current
            .append_performance_projection(
                &third_binding,
                &third.lanes,
                NoteTimelinePolicy::PreserveLoop,
                false,
            )
            .unwrap(),
    );
    let mut dropped = current.value().clone();
    dropped["performanceProjections"]["entries"] = json!([]);
    attacks.push(ProjectDocument::from_value(dropped).unwrap());
    let mut partial = current.value().clone();
    partial["performanceProjections"]["entries"]
        .as_array_mut()
        .unwrap()
        .pop();
    attacks.push(ProjectDocument::from_value(partial).unwrap());
    let mut renamed = current.value().clone();
    renamed["performanceProjections"]["entries"][0]["id"] = "caller-renamed-original".into();
    attacks.push(ProjectDocument::from_value(renamed).unwrap());
    let mut reordered = current.value().clone();
    reordered["performanceProjections"]["entries"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    attacks.push(ProjectDocument::from_value(reordered).unwrap());
    for keep_opaque in [true, false] {
        let mut downgraded = current.value().clone();
        downgraded["schemaVersion"] = 2.into();
        if !keep_opaque {
            downgraded
                .as_object_mut()
                .unwrap()
                .remove("performanceProjections");
        }
        attacks.push(ProjectDocument::from_value(downgraded).unwrap());
    }
    let before = catalog(&harness.db());
    for candidate in attacks {
        assert!(
            p.store.save(&candidate, current.revision()).is_err(),
            "Generic Save granted new ownership"
        );
        assert_eq!(catalog(&harness.db()), before);
        assert_source_bytes(&harness, &staged, &p.owned, &original);
    }
    let copied = current.imported_copy(&"0".repeat(64)).unwrap();
    assert!(
        p.store.create(&copied).is_err(),
        "Generic Create admitted an already populated registry"
    );
    assert_eq!(catalog(&harness.db()), before);
    assert_eq!(p.store.load(current.id()).unwrap(), current);
}

#[test]
fn old_commit_survives_a_new_branch_and_replays_as_compact_fact_with_all_original_paths_absent() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let (governor, _) = budget(InspectionLimits::default());
    let committed = p
        .store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let published = p.store.load(p.project.id()).unwrap();
    let later = p
        .store
        .apply_edit(&edit(
            &published,
            "abandoned-name",
            EditOperation::Rename {
                name: "Abandoned later name".into(),
            },
        ))
        .unwrap()
        .document;
    let undone = p.store.undo(later.id(), later.revision()).unwrap();
    let branch = p
        .store
        .apply_edit(&edit(
            &undone,
            "new-branch",
            EditOperation::Rename {
                name: "Current authored branch".into(),
            },
        ))
        .unwrap()
        .document;
    assert_eq!(branch.revision(), 4);
    assert_eq!(
        branch
            .performance_projections()
            .unwrap()
            .unwrap()
            .entries
            .len(),
        1
    );
    let owned_bytes = fs::read(&p.owned).unwrap();
    let staged_bytes = fs::read(&harness.original.path).unwrap();
    let parked_owned = harness.root.join("owned-parked.sqlite3");
    let parked_staged = harness.root.join("staged-parked.sqlite3");
    fs::rename(&p.owned, &parked_owned).unwrap();
    fs::rename(&harness.original.path, &parked_staged).unwrap();
    let before = catalog(&harness.db());
    assert!(
        p.store.load(branch.id()).is_err(),
        "Active load must still require the retained owned original"
    );
    let mut recorded = committed.clone();
    recorded.replayed = true;
    assert_eq!(
        p.store.projection_publication_receipt(&p.intent).unwrap(),
        Some(recorded.clone())
    );
    let (cancelled, token) = budget(InspectionLimits::default());
    token.store(true, Ordering::Release);
    assert_eq!(
        p.store
            .publish_performance_projection(&p.intent, &cancelled)
            .unwrap(),
        recorded
    );
    assert!(matches!(
        cancelled.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_eq!(
        catalog(&harness.db()),
        before,
        "Replay must not reinstall revision1 or alter the newer branch"
    );
    let current: String = harness
        .db()
        .query_row(
            "SELECT payload FROM projects WHERE id=?1",
            [branch.id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(current.as_bytes(), branch.to_json().unwrap());
    assert_eq!(fs::read(parked_owned).unwrap(), owned_bytes);
    assert_eq!(fs::read(parked_staged).unwrap(), staged_bytes);
    assert!(!p.owned.exists() && !harness.original.path.exists());
}

#[test]
fn reused_derived_operation_identity_cannot_change_any_part_of_the_completed_request() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let before = catalog(&harness.db());
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    for field in 0..4 {
        let mut conflict = p.intent.clone();
        match field {
            0 => conflict.request.destination_start_tick += 1,
            1 => conflict.expected_revision += 1,
            2 => conflict.lanes[0].track_name = "Another initial name".into(),
            _ => conflict.expected_review_sha256 = "0".repeat(64),
        }
        assert_eq!(conflict.operation_id, p.intent.operation_id);
        assert!(p.store.projection_publication_receipt(&conflict).is_err());
        let (governor, _) = budget(InspectionLimits::default());
        assert!(
            p.store
                .publish_performance_projection(&conflict, &governor)
                .is_err()
        );
        assert_eq!(catalog(&harness.db()), before);
        assert_source_bytes(&harness, &staged, &p.owned, &original);
    }
}

#[test]
fn fresh_cancellation_work_and_file_limits_stay_typed_without_partial_project_writes() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let before = catalog(&harness.db());
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    let (governor, cancellation) = budget(InspectionLimits::default());
    cancellation.store(true, Ordering::Release);
    assert!(matches!(
        p.store.publish_performance_projection(&p.intent, &governor),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    assert!(matches!(
        governor.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_eq!(catalog(&harness.db()), before);
    for (limits, resource) in [
        (
            InspectionLimits {
                max_work_bytes: 1,
                ..Default::default()
            },
            InspectionResource::WorkBytes,
        ),
        (
            InspectionLimits {
                max_file_bytes: original.len() as u64 - 1,
                ..Default::default()
            },
            InspectionResource::FileBytes,
        ),
    ] {
        let (governor, _) = budget(limits);
        let result = p.store.publish_performance_projection(&p.intent, &governor);
        assert!(
            matches!(&result,Err(StoreError::Recording(RecordingError::InspectionLimit{resource:actual,..})) if *actual==resource),
            "Typed stop lost: {result:?}"
        );
        assert!(
            matches!(governor.checkpoint(),Err(RecordingError::InspectionLimit{resource:actual,..}) if actual==resource)
        );
        assert_eq!(catalog(&harness.db()), before);
        assert_source_bytes(&harness, &staged, &p.owned, &original);
    }
}

#[test]
fn a_last_receipt_sql_abort_or_postimage_trigger_mutation_rolls_back_the_complete_candidate() {
    for abort in [true, false] {
        let harness = Harness::new();
        let mut p = harness.prepare();
        let db = harness.db();
        let trigger = if abort {
            "CREATE TRIGGER refuse_projection_receipt BEFORE INSERT ON operations WHEN NEW.operation_id LIKE 'os-perf-proj:%' BEGIN SELECT RAISE(ABORT,'injected final receipt failure'); END;"
        } else {
            "CREATE TRIGGER change_projection_postimage AFTER INSERT ON operations WHEN NEW.operation_id LIKE 'os-perf-proj:%' BEGIN UPDATE projects SET payload=json_set(payload,'$.name','injected postimage substitution') WHERE id=NEW.project_id; END;"
        };
        db.execute_batch(trigger).unwrap();
        let before = catalog(&db);
        let staged = harness.original.bytes();
        let original = fs::read(&p.owned).unwrap();
        let (governor, _) = budget(InspectionLimits::default());
        assert!(
            p.store
                .publish_performance_projection(&p.intent, &governor)
                .is_err()
        );
        assert_eq!(
            catalog(&db),
            before,
            "A refusal after all lanes were assembled must leave no partial history, payload or receipt"
        );
        assert!(
            p.store
                .projection_publication_receipt(&p.intent)
                .unwrap()
                .is_none()
        );
        assert_eq!(p.store.load(p.project.id()).unwrap(), p.project);
        assert_source_bytes(&harness, &staged, &p.owned, &original);
    }
}

#[test]
fn coherent_rehashed_catalog_history_and_receipt_forgery_cannot_authenticate_false_music() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let actual = p.store.load(p.project.id()).unwrap();
    let mut registry = actual.performance_projections().unwrap().unwrap();
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
    binding.review.review_sha256 = review.review_sha256.clone();
    binding.lanes[0].original_notes[0].values.pitch = 61;
    let track_id = binding.lanes[0].track_id.clone();
    let (encoding_budget, _) = budget(InspectionLimits::default());
    check_retained_binding_encoding_bounded(binding, &encoding_budget).unwrap();
    let mut value = actual.value().clone();
    value["performanceProjections"] = serde_json::to_value(&registry).unwrap();
    value["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|track| track["id"] == track_id)
        .unwrap()["notes"][0]["pitch"] = 61.into();
    let forged = ProjectDocument::from_value(value).unwrap();
    let mut false_intent = p.intent.clone();
    false_intent.expected_review_sha256 = review.review_sha256;
    let request = false_intent.canonical_json().unwrap();
    let result = String::from_utf8(forged.to_json().unwrap()).unwrap();
    let db = harness.db();
    remove_triggers(&db, "edits");
    remove_triggers(&db, "operations");
    db.execute(
        "UPDATE projects SET payload=?1 WHERE id=?2",
        params![result, actual.id()],
    )
    .unwrap();
    db.execute(
        "UPDATE edits SET after_json=?1 WHERE project_id=?2 AND sequence=1",
        params![result, actual.id()],
    )
    .unwrap();
    db.execute("UPDATE operations SET request_json=?1,request_sha256=?2,result_json=?3,result_sha256=?4 WHERE operation_id=?5",params![request,hash(request.as_bytes()),result,hash(result.as_bytes()),false_intent.operation_id.0]).unwrap();
    let before = catalog(&db);
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    // This reports only internally coherent cells in an attacker-written
    // catalog. It must not become a playable document or a source-proof token.
    let recorded = p
        .store
        .projection_publication_receipt(&false_intent)
        .unwrap()
        .unwrap();
    assert_eq!(recorded.result_sha256, hash(result.as_bytes()));
    assert!(p.store.load(actual.id()).is_err());
    let (governor, _) = budget(InspectionLimits::default());
    assert!(p.store.load_with_budget(actual.id(), &governor).is_err());
    let mut changed = forged.clone();
    changed.rename("Do not save source-false music").unwrap();
    assert!(p.store.save(&changed, forged.revision()).is_err());
    assert!(p.store.undo(actual.id(), forged.revision()).is_err());
    assert!(
        p.store
            .apply_edit(&edit(
                &forged,
                "reject-forged-edit",
                EditOperation::Rename {
                    name: "Also refused".into()
                }
            ))
            .is_err()
    );
    assert_eq!(catalog(&db), before);
    assert_source_bytes(&harness, &staged, &p.owned, &original);
}

#[test]
fn authentic_projection_stays_closed_to_osar_and_metadata_reader_even_after_undo_hides_it() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let current = p.store.load(p.project.id()).unwrap();
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    let before = catalog(&harness.db());
    let reader = NativeReader::open(p.store.root()).unwrap();
    assert!(
        reader.load_document(current.id()).is_err(),
        "The metadata-only reader must not grant source-validated schema3 authority"
    );
    drop(reader);
    let destination = harness
        .root
        .join("authentic-but-not-osar-authorized.omniusong");
    assert!(p.store.export_archive(current.id(), &destination).is_err());
    assert!(!destination.exists());
    assert_eq!(catalog(&harness.db()), before);
    let undone = p.store.undo(current.id(), current.revision()).unwrap();
    assert!(undone.performance_projections().unwrap().is_none());
    let before = catalog(&harness.db());
    let hidden = harness.root.join("hidden-authentic-history.omniusong");
    assert!(p.store.export_archive(undone.id(), &hidden).is_err());
    assert!(!hidden.exists());
    assert_eq!(catalog(&harness.db()), before);
    assert_source_bytes(&harness, &staged, &p.owned, &original);
}

#[test]
fn only_registered_owned_originals_are_admitted_and_repeated_bindings_share_the_governor() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let mut unregistered = NativeStore::open(harness.root.join("unregistered")).unwrap();
    unregistered.create(&p.project).unwrap();
    let unregistered_db = Connection::open(unregistered.root().join("studio.sqlite3")).unwrap();
    let before = catalog(&unregistered_db);
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    assert!(
        unregistered
            .publish_performance_projection(&p.intent, &governor)
            .is_err(),
        "The staged original or another store's owned filename is not registration authority"
    );
    assert_eq!(catalog(&unregistered_db), before);
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let one = p.store.load(p.project.id()).unwrap();
    let (one_budget, _) = budget(InspectionLimits::default());
    p.store.load_with_budget(one.id(), &one_budget).unwrap();
    let one_usage = one_budget.usage();
    let second = harness.intent(&p.store, &one, "projection-two", "second", 3000);
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&second, &governor)
        .unwrap();
    let before = catalog(&harness.db());
    let (two_budget, _) = budget(InspectionLimits::default());
    let two = p.store.load_with_budget(one.id(), &two_budget).unwrap();
    let two_usage = two_budget.usage();
    assert_eq!(
        two.performance_projections()
            .unwrap()
            .unwrap()
            .entries
            .len(),
        2
    );
    assert!(
        two_usage.work_bytes > one_usage.work_bytes
            && two_usage.record_visits > one_usage.record_visits,
        "The second binding's rederivation cannot reset or evade cumulative work/record accounting"
    );
    // FileBytes is a maximum-size metric, not an inspection-call count. This
    // equality makes no claim about how often the original was authenticated.
    assert_eq!(two_usage.file_bytes, one_usage.file_bytes);
    let (limited, _) = budget(InspectionLimits {
        max_work_bytes: one_usage.work_bytes,
        ..Default::default()
    });
    assert!(matches!(
        p.store.load_with_budget(two.id(), &limited),
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        }))
    ));
    assert!(matches!(
        limited.checkpoint(),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        })
    ));
    assert_eq!(catalog(&harness.db()), before);
    assert_source_bytes(&harness, &staged, &p.owned, &original);
}

#[test]
fn replacement_of_a_cached_registration_inside_the_final_operation_rolls_back_every_active_path() {
    for mode in ["publication", "edit", "undo"] {
        let harness = Harness::new();
        let mut p = harness.prepare();
        let current = if mode == "publication" {
            p.project.clone()
        } else {
            let (governor, _) = budget(InspectionLimits::default());
            p.store
                .publish_performance_projection(&p.intent, &governor)
                .unwrap();
            p.store.load(p.project.id()).unwrap()
        };
        let db = harness.db();
        db.execute_batch("PRAGMA recursive_triggers=OFF").unwrap();
        let replacement = format!(
            "INSERT OR REPLACE INTO native_takes(take_id,manifest_sha256,object_sha256,byte_length,manifest_json) SELECT take_id,manifest_sha256,'{}',byte_length,manifest_json FROM native_takes",
            "f".repeat(64)
        );
        // Prove the attack itself bypasses the retained UPDATE/DELETE guards
        // with recursion off, instead of accepting any unrelated SQL failure.
        let before_probe = catalog(&db);
        let probe = db.unchecked_transaction().unwrap();
        probe.execute_batch(&replacement).unwrap();
        let changed: String = probe
            .query_row("SELECT object_sha256 FROM native_takes", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(changed, "f".repeat(64));
        probe.rollback().unwrap();
        assert_eq!(catalog(&db), before_probe);
        db.execute_batch(&format!(
            "CREATE TRIGGER replace_registration_after_operation AFTER INSERT ON operations BEGIN {replacement}; END;"
        )).unwrap();
        let before = catalog(&db);
        let staged = harness.original.bytes();
        let original = fs::read(&p.owned).unwrap();
        let result = if mode == "publication" {
            let (governor, _) = budget(InspectionLimits::default());
            p.store
                .publish_performance_projection(&p.intent, &governor)
                .map(|_| ())
        } else {
            let action = if mode == "undo" {
                EditOperation::Undo
            } else {
                EditOperation::Rename {
                    name: "Do not acknowledge a replaced registration".into(),
                }
            };
            p.store
                .apply_edit(&edit(&current, "registration-fence-operation", action))
                .map(|_| ())
        };
        assert!(
            matches!(&result, Err(StoreError::Invalid(message)) if message.contains("Registered source changed")),
            "The final cached-registration comparison must catch {mode}, not a source reopen or immutable SQL trigger: {result:?}"
        );
        assert_eq!(
            catalog(&db),
            before,
            "The registration, complete project, history and receipt all roll back for {mode}"
        );
        assert_eq!(p.store.load(current.id()).unwrap(), current);
        assert_source_bytes(&harness, &staged, &p.owned, &original);
    }
}

#[test]
fn fresh_generic_paths_cannot_claim_projection_namespace_but_exact_legacy_receipts_still_replay() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    let staged = harness.original.bytes();
    let original = fs::read(&p.owned).unwrap();
    let before = catalog(&harness.db());
    let mut reserved_edit = edit(
        &p.project,
        "temporary",
        EditOperation::Rename {
            name: "Unadmitted namespace claim".into(),
        },
    );
    reserved_edit.operation_id = p.intent.operation_id.clone();
    let rejected = p.store.apply_edit(&reserved_edit);
    assert!(
        matches!(&rejected, Err(StoreError::Invalid(message)) if message.to_lowercase().contains("projection"))
    );
    let take = omniusong_store::TakePublication {
        operation_id: p.intent.operation_id.clone(),
        project_id: ProjectId(p.project.id().into()),
        expected_revision: 0,
        track_id: omniusong_core::contracts::TrackId("no-ingestion".into()),
        track_name: "Must refuse before capture access".into(),
        clip: omniusong_core::native::TakeClip {
            id: "no-clip".into(),
            source: omniusong_core::native::TakeReference {
                take_id: TakeId("unregistered-do-not-ingest".into()),
                manifest_sha256: "f".repeat(64),
                stream_id: "audio".into(),
            },
            range: omniusong_core::native::TakeRange::AudioFrames { start: 0, count: 4 },
            position: omniusong_core::native::TimelinePosition {
                frame: 0,
                sample_rate: 8000,
            },
            fades: None,
            edit_source: None,
        },
    };
    let rejected = p
        .store
        .attach_capture(harness.root.join("absent-capture.sqlite3"), &take);
    assert!(
        matches!(&rejected, Err(StoreError::Invalid(message)) if message.to_lowercase().contains("projection")),
        "Refusal must precede opening or ingesting a nonexistent capture: {rejected:?}"
    );
    let copy = omniusong_store::RestoreCopyIntent {
        operation_id: p.intent.operation_id.clone(),
        expected_archive_sha256: "e".repeat(64),
    };
    let rejected = p
        .store
        .restore_archive_as_copy_outcome(&harness.root.join("absent-archive.omniusong"), &copy)
        .unwrap_err();
    assert!(
        matches!(rejected.operation_error(), StoreError::Invalid(message) if message.to_lowercase().contains("projection")),
        "Refusal must precede archive acquisition: {rejected:?}"
    );
    assert_eq!(
        rejected.context().phase,
        omniusong_store::ArchivePhase::NotStarted
    );
    assert!(rejected.context().staging.is_none());
    assert!(rejected.context().archive_sha256.is_none());
    assert_eq!(
        rejected.context().expected_archive_sha256.as_deref(),
        Some(copy.expected_archive_sha256.as_str())
    );
    assert_eq!(catalog(&harness.db()), before);
    assert_source_bytes(&harness, &staged, &p.owned, &original);

    // Simulate an ordinary edit receipt written before this operation prefix
    // became reserved. Its exact request/result remain historical facts.
    let ordinary = edit(
        &p.project,
        "ordinary-before-reservation",
        EditOperation::Rename {
            name: "Historical ordinary completion".into(),
        },
    );
    let completed = p.store.apply_edit(&ordinary).unwrap();
    let mut historical = ordinary.clone();
    historical.operation_id = p.intent.operation_id.clone();
    let request = serde_json::to_string(&historical).unwrap();
    let db = harness.db();
    remove_triggers(&db, "operations");
    db.execute("UPDATE operations SET operation_id=?1,request_json=?2,request_sha256=?3 WHERE operation_id=?4",
        params![historical.operation_id.0,request,hash(request.as_bytes()),ordinary.operation_id.0]).unwrap();
    let before_replay = catalog(&db);
    let replayed = p.store.apply_edit(&historical).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.document, completed.document);
    assert_eq!(p.store.load(p.project.id()).unwrap(), completed.document);
    let (governor, _) = budget(InspectionLimits::default());
    assert!(
        p.store
            .publish_performance_projection(&p.intent, &governor)
            .is_err(),
        "A legacy ordinary receipt cannot be reinterpreted as the new typed publication"
    );
    assert_eq!(catalog(&db), before_replay);
    assert_source_bytes(&harness, &staged, &p.owned, &original);
}

#[test]
fn a_final_asset_corruption_trigger_rolls_back_and_active_load_prepays_asset_materialization() {
    let harness = Harness::new();
    let mut p = harness.prepare();
    // These are hash-addressed synthetic bytes, not a decoded WAV or a claim
    // about audio quality. The existing notes-track assetId is a typed reference.
    let asset = vec![37u8; 1024 * 1024];
    let asset_id = format!("{}.wav", hash(&asset));
    let db = harness.db();
    db.execute(
        "INSERT INTO assets(id,payload) VALUES(?1,?2)",
        params![asset_id, asset],
    )
    .unwrap();
    let mut value = p.project.value().clone();
    value["tracks"][0]["assetId"] = asset_id.clone().into();
    p.project = p
        .store
        .save(
            &ProjectDocument::from_value(value).unwrap(),
            p.project.revision(),
        )
        .unwrap();
    p.intent = harness.intent(&p.store, &p.project, "projection-one", "first", 1000);
    db.execute_batch("CREATE TRIGGER corrupt_asset_after_projection_receipt AFTER INSERT ON operations BEGIN UPDATE assets SET payload=zeroblob(length(payload)); END;").unwrap();
    let before = catalog(&db);
    let staged = harness.original.bytes();
    let owned = fs::read(&p.owned).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    let result = p.store.publish_performance_projection(&p.intent, &governor);
    assert!(
        matches!(&result,Err(StoreError::Invalid(message)) if message.to_lowercase().contains("asset") || message.to_lowercase().contains("audio")),
        "Final asset-content validation must catch the same-length substitution: {result:?}"
    );
    assert_eq!(catalog(&db), before);
    let retained: Vec<u8> = db
        .query_row(
            "SELECT payload FROM assets WHERE id=?1",
            [&asset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, asset);
    assert!(
        p.store
            .projection_publication_receipt(&p.intent)
            .unwrap()
            .is_none()
    );
    assert_eq!(p.store.load(p.project.id()).unwrap(), p.project);
    assert_source_bytes(&harness, &staged, &p.owned, &owned);

    db.execute_batch("DROP TRIGGER corrupt_asset_after_projection_receipt")
        .unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    p.store
        .publish_performance_projection(&p.intent, &governor)
        .unwrap();
    let current = p.store.load(p.project.id()).unwrap();
    let before_load = catalog(&db);
    // Even the asset alone needs four times this byte count in prepaid work;
    // this smaller caller limit cannot authorize a materialize/hash attempt.
    let (limited, _) = budget(InspectionLimits {
        max_work_bytes: asset.len() as u64,
        ..Default::default()
    });
    assert!(matches!(
        p.store.load_with_budget(current.id(), &limited),
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        }))
    ));
    assert!(matches!(
        limited.checkpoint(),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        })
    ));
    assert_eq!(catalog(&db), before_load);
    let retained: Vec<u8> = db
        .query_row(
            "SELECT payload FROM assets WHERE id=?1",
            [&asset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, asset);
    assert_source_bytes(&harness, &staged, &p.owned, &owned);
}
