//! Independent restore-copy lineage and stable session-operation regressions.
use omniusong_core::{
    ProjectDocument,
    contracts::{
        EditOperation, EditScope, Extensions, OperationId, ProjectEditIntent, ProjectId, TrackId,
    },
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, RestoreCopyIntent, TakePublication};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/archive-copy-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn store(&self, name: &str) -> NativeStore {
        NativeStore::open(self.0.join(name)).unwrap()
    }
    fn db(&self, name: &str) -> Connection {
        Connection::open(self.0.join(name).join("studio.sqlite3")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Archive-copy review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn intent(project: &ProjectDocument, id: &str, edit: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        scope: EditScope::Project,
        edit,
        extensions: Extensions::new(),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn copy_intent(sha: &str, id: &str) -> RestoreCopyIntent {
    RestoreCopyIntent {
        operation_id: OperationId(id.into()),
        expected_archive_sha256: sha.into(),
    }
}
fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
fn rewrite_document(json: &str, id: &str) -> Value {
    let mut value: Value = serde_json::from_str(json).unwrap();
    value["id"] = id.into();
    value
}
fn history_take_references(db: &Connection) -> usize {
    let mut statement = db
        .prepare("SELECT before_json FROM edits UNION ALL SELECT after_json FROM edits")
        .unwrap();
    statement
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|row| {
            ProjectDocument::from_json(row.unwrap().as_bytes())
                .unwrap()
                .referenced_takes()
                .unwrap()
                .len()
        })
        .sum()
}
fn remove_immutability(db: &Connection, table: &str) {
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name=?1")
        .unwrap()
        .query_map([table], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    for name in names {
        db.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))
            .unwrap();
    }
}
fn source_archive(f: &Fixture) -> (ProjectDocument, PathBuf, String, Vec<u8>) {
    let mut store = f.store("source");
    let mut value = ProjectDocument::new("Imported source")
        .unwrap()
        .value()
        .clone();
    let source_id = value["id"].clone();
    value["vendor"] = json!({"project_id":source_id,"operation_id":"source-first","id":source_id,"nativeImport":{"sourceId":source_id}});
    value["precision"] = serde_json::from_str(r#"{"integer":1234567890123456789012345678901234567890,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
    value["tracks"][0]["notes"] = json!([{"id":"source-first","pitch":60,"startTick":0,"durationTicks":960,"velocity16":65535,"vendor":{"project_id":source_id}}]);
    let bytes = b"immutable copy fixture asset";
    let asset = format!("{}.wav", digest(bytes));
    let assets = f.0.join("legacy-assets");
    fs::create_dir_all(&assets).unwrap();
    fs::write(assets.join(&asset), bytes).unwrap();
    value["tracks"][0]["assetId"] = asset.into();
    let mut original = b" \n".to_vec();
    original.extend(serde_json::to_vec_pretty(&value).unwrap());
    original.extend_from_slice(b"\n\t ");
    let path = f.0.join("legacy.json");
    fs::write(&path, &original).unwrap();
    let imported = store.import_legacy(&path, &assets).unwrap();
    let mut first = intent(
        &imported,
        "source-first",
        EditOperation::UpdateSession {
            name: "First pair".into(),
            tempo: 117.25,
        },
    );
    first
        .extensions
        .insert("kind".into(), json!("take_publication"));
    first.extensions.insert("version".into(), json!(1));
    first.extensions.insert(
        "intent".into(),
        json!({"operation_id":"source-first","project_id":imported.id()}),
    );
    let first = store.apply_edit(&first).unwrap().document;
    let second = store
        .apply_edit(&intent(
            &first,
            "source-second",
            EditOperation::Rename {
                name: "Abandoned redo".into(),
            },
        ))
        .unwrap()
        .document;
    let undone = store
        .apply_edit(&intent(&second, "source-undo", EditOperation::Undo))
        .unwrap()
        .document;
    let current = store
        .apply_edit(&intent(
            &undone,
            "source-branch",
            EditOperation::Rename {
                name: "Retained branch".into(),
            },
        ))
        .unwrap()
        .document;
    let archive = f.0.join("source.omniusong");
    let receipt = store.export_archive(current.id(), &archive).unwrap();
    (current, archive, receipt.sha256, original)
}

fn assert_maps_and_snapshots(
    source: &Connection,
    destination: &Connection,
    source_id: &str,
    destination_id: &str,
) {
    let mut history = source.prepare("SELECT sequence,before_json,after_json FROM edits WHERE project_id=?1 ORDER BY sequence").unwrap();
    for row in history
        .query_map([source_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
    {
        let (sequence, before, after) = row.unwrap();
        let (new_before, new_after): (String, String) = destination
            .query_row(
                "SELECT before_json,after_json FROM edits WHERE project_id=?1 AND sequence=?2",
                params![destination_id, sequence],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&new_before).unwrap(),
            rewrite_document(&before, destination_id)
        );
        assert_eq!(
            serde_json::from_str::<Value>(&new_after).unwrap(),
            rewrite_document(&after, destination_id)
        );
    }
    let (proof_sha,proof_json):(String,String)=destination.query_row("SELECT p.proof_sha256,p.receipt_json FROM archive_copies c JOIN archive_copy_proofs p USING(proof_sha256) WHERE c.project_id=?1",[destination_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(digest(proof_json.as_bytes()), proof_sha);
    let proof: Value = serde_json::from_str(&proof_json).unwrap();
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.copy-mapping.v1\0");
    let mut mappings=destination.prepare("SELECT source_operation_id,destination_operation_id,source_request_sha256,source_result_sha256,destination_request_sha256,destination_result_sha256 FROM archive_copy_proof_operations WHERE proof_sha256=?1 ORDER BY source_operation_id").unwrap();
    let mut mapping_count = 0;
    for row in mappings
        .query_map([&proof_sha], |r| {
            Ok([
                r.get::<_, String>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ])
        })
        .unwrap()
    {
        let fields = row.unwrap();
        mapping_count += 1;
        for field in &fields {
            hash.update((field.len() as u64).to_le_bytes());
            hash.update(field.as_bytes());
        }
        assert_ne!(fields[0], fields[1]);
        let (request,result,request_hash,result_hash):(String,String,String,String)=source.query_row("SELECT request_json,result_json,request_sha256,result_sha256 FROM operations WHERE operation_id=?1",[&fields[0]],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!((&fields[2], &fields[3]), (&request_hash, &result_hash));
        let (new_request,new_result,new_request_hash,new_result_hash):(String,String,String,String)=destination.query_row("SELECT request_json,result_json,request_sha256,result_sha256 FROM operations WHERE operation_id=?1 AND project_id=?2",params![fields[1],destination_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(
            (&fields[4], &fields[5]),
            (&new_request_hash, &new_result_hash)
        );
        assert_eq!(digest(new_request.as_bytes()), new_request_hash);
        assert_eq!(digest(new_result.as_bytes()), new_result_hash);
        let mut expected: Value = serde_json::from_str(&request).unwrap();
        let typed = if expected.get("operation_id").is_some() {
            &mut expected
        } else {
            &mut expected["intent"]
        };
        typed["operation_id"] = fields[1].clone().into();
        typed["project_id"] = destination_id.into();
        assert_eq!(
            serde_json::from_str::<Value>(&new_request).unwrap(),
            expected
        );
        assert_eq!(
            serde_json::from_str::<Value>(&new_result).unwrap(),
            rewrite_document(&result, destination_id)
        );
    }
    assert_eq!(mapping_count, count(source, "operations"));
    assert_eq!(proof["mapping_count"], mapping_count);
    assert_eq!(proof["mapping_sha256"], format!("{:x}", hash.finalize()));
}

#[test]
fn session_name_and_tempo_are_one_atomic_edit_with_stable_retry_and_durable_history() {
    let fixture = Fixture::new();
    let mut store = fixture.store("paired");
    let mut value = ProjectDocument::new("Before").unwrap().value().clone();
    value["opaque"]=serde_json::from_str(r#"{"projectId":"UUID-looking-retained","number":1234567890123456789012345678901234567890}"#).unwrap();
    let original = store
        .create(&ProjectDocument::from_value(value).unwrap())
        .unwrap();
    let first = intent(
        &original,
        "paired-session",
        EditOperation::UpdateSession {
            name: "Together".into(),
            tempo: 117.25,
        },
    );
    let committed = store.apply_edit(&first).unwrap().document;
    assert_eq!(
        (committed.name(), committed.tempo(), committed.revision()),
        ("Together", 117.25, 1)
    );
    assert_eq!(committed.value()["opaque"], original.value()["opaque"]);
    for (id, name, tempo) in [
        ("bad-tempo", "Would partially rename", f64::NAN),
        ("bad-name", "   ", 121.5),
    ] {
        let bad = intent(
            &committed,
            id,
            EditOperation::UpdateSession {
                name: name.into(),
                tempo,
            },
        );
        assert!(store.apply_edit(&bad).is_err());
        assert_eq!(store.load(original.id()).unwrap(), committed);
    }
    let later = store
        .apply_edit(&intent(
            &committed,
            "paired-later",
            EditOperation::UpdateSession {
                name: "Later".into(),
                tempo: 130.5,
            },
        ))
        .unwrap()
        .document;
    let replay = store.apply_edit(&first).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, committed);
    assert_eq!(store.load(original.id()).unwrap(), later);
    let db = fixture.db("paired");
    let edits: i64 = db
        .query_row("SELECT count(*) FROM edits", [], |row| row.get(0))
        .unwrap();
    let operations: i64 = db
        .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    assert_eq!((edits, operations), (2, 2));
    drop(db);
    drop(store);
    let mut store = fixture.store("paired");
    let undo = store.undo(original.id(), 2).unwrap();
    assert_eq!((undo.name(), undo.tempo()), ("Together", 117.25));
    let redo = store.redo(original.id(), undo.revision()).unwrap();
    assert_eq!((redo.name(), redo.tempo()), ("Later", 130.5));
    assert_eq!(store.apply_edit(&first).unwrap().document, committed);
    assert_eq!(store.load(original.id()).unwrap(), redo);
}

#[test]
fn copy_rewrites_only_typed_ids_across_history_requests_results_and_retains_exact_import() {
    let f = Fixture::new();
    let (source, path, sha, raw) = source_archive(&f);
    let archive_before = fs::read(&path).unwrap();
    let source_db_before = fs::read(f.0.join("source/studio.sqlite3")).unwrap();
    let mut target = f.store("target");
    let inspected = target.inspect_archive(&path).unwrap();
    assert_eq!(
        (inspected.sha256, inspected.project_id),
        (sha.clone(), source.id().into())
    );
    let copied = target
        .restore_archive_as_copy(&path, &copy_intent(&sha, "typed-copy"))
        .unwrap();
    assert!(!copied.replayed);
    assert_ne!(copied.document.id(), source.id());
    let source_json = String::from_utf8(source.to_json().unwrap()).unwrap();
    assert_eq!(
        copied.document.value(),
        &rewrite_document(&source_json, copied.document.id())
    );
    assert_eq!(
        target.imported_original(copied.document.id()).unwrap(),
        Some(raw.clone())
    );
    assert_maps_and_snapshots(
        &f.db("source"),
        &f.db("target"),
        source.id(),
        copied.document.id(),
    );
    for asset in source.referenced_assets().unwrap() {
        assert_eq!(
            target.asset(&asset).unwrap(),
            fs::read(f.0.join("legacy-assets").join(asset)).unwrap()
        );
    }
    assert_eq!(fs::read(&path).unwrap(), archive_before);
    assert_eq!(fs::read(f.0.join("legacy.json")).unwrap(), raw);
    assert_eq!(
        fs::read(f.0.join("source/studio.sqlite3")).unwrap(),
        source_db_before
    );
    drop(target);
    assert_eq!(
        f.store("target").load(copied.document.id()).unwrap(),
        copied.document
    );
}

#[test]
fn independent_copy_intents_allocate_distinct_maps_and_exact_retry_ignores_missing_input() {
    let f = Fixture::new();
    let (source, path, sha, _) = source_archive(&f);
    let mut target = f.store("target");
    let request = copy_intent(&sha, "copy-stable");
    let first = target.restore_archive_as_copy(&path, &request).unwrap();
    let second = target
        .restore_archive_as_copy(&path, &copy_intent(&sha, "copy-fresh"))
        .unwrap();
    assert_ne!(first.document.id(), second.document.id());
    let db = f.db("target");
    let mut operations = db
        .prepare("SELECT operation_id FROM operations WHERE project_id=?1")
        .unwrap();
    let a: BTreeSet<String> = operations
        .query_map([first.document.id()], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    let b: BTreeSet<String> = operations
        .query_map([second.document.id()], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!((a.len(), b.len()), (4, 4));
    assert!(a.is_disjoint(&b));
    assert!(!a.contains("source-first") && !b.contains("source-first"));
    assert_maps_and_snapshots(&f.db("source"), &db, source.id(), second.document.id());
    let later = target
        .apply_edit(&intent(
            &first.document,
            "after-copy",
            EditOperation::UpdateSession {
                name: "Later copy state".into(),
                tempo: 132.5,
            },
        ))
        .unwrap()
        .document;
    fs::remove_file(&path).unwrap();
    drop(target);
    let mut target = f.store("target");
    let repeated = target.restore_archive_as_copy(&path, &request).unwrap();
    assert!(repeated.replayed);
    assert_eq!(repeated.document, first.document);
    assert_eq!(target.load(later.id()).unwrap(), later);
    assert!(
        target
            .restore_archive_as_copy(&path, &copy_intent(&"f".repeat(64), "copy-stable"))
            .is_err()
    );
    assert_eq!(target.list().unwrap().len(), 2);
}

#[test]
fn inherited_lineage_is_immutable_historical_namespace_and_survives_identity_restore() {
    let f = Fixture::new();
    let (_, path, sha, _) = source_archive(&f);
    let mut first_store = f.store("first-copy");
    let first = first_store
        .restore_archive_as_copy(&path, &copy_intent(&sha, "ancestor-copy-id"))
        .unwrap();
    let inherited: (String, String) = f
        .db("first-copy")
        .query_row(
            "SELECT proof_sha256,receipt_json FROM archive_copy_proofs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let first_archive = f.0.join("first.omniusong");
    let first_export = first_store
        .export_archive(first.document.id(), &first_archive)
        .unwrap();
    let mut second_store = f.store("second-copy");
    let unrelated = second_store
        .create(&ProjectDocument::new("Unrelated local").unwrap())
        .unwrap();
    let unrelated = second_store
        .apply_edit(&intent(
            &unrelated,
            "ancestor-copy-id",
            EditOperation::Rename {
                name: "Legitimate local ID collision".into(),
            },
        ))
        .unwrap()
        .document;
    let request = copy_intent(&first_export.sha256, "descendant-copy-id");
    let second = second_store
        .restore_archive_as_copy(&first_archive, &request)
        .unwrap();
    assert_eq!(second_store.load(unrelated.id()).unwrap(), unrelated);
    let db = f.db("second-copy");
    let exact: String = db
        .query_row(
            "SELECT receipt_json FROM archive_copy_proofs WHERE proof_sha256=?1",
            [&inherited.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exact, inherited.1);
    assert_eq!(count(&db, "archive_copy_proofs"), 2);
    assert_eq!(count(&db, "archive_copies"), 1);
    let live: String = db
        .query_row("SELECT operation_id FROM archive_copies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(live, "descendant-copy-id");
    let latest:String=db.query_row("SELECT receipt_json FROM archive_copy_proofs WHERE proof_sha256=(SELECT proof_sha256 FROM archive_copies)",[],|r|r.get(0)).unwrap();
    let latest: Value = serde_json::from_str(&latest).unwrap();
    assert_eq!(latest["ancestors"], json!([inherited.0]));
    let second_archive = f.0.join("second.omniusong");
    second_store
        .export_archive(second.document.id(), &second_archive)
        .unwrap();
    let mut restored = f.store("identity-restored");
    assert_eq!(
        restored.restore_archive(&second_archive).unwrap(),
        second.document
    );
    assert_eq!(count(&f.db("identity-restored"), "archive_copy_proofs"), 2);
    assert_eq!(count(&f.db("identity-restored"), "archive_copies"), 1);
    fs::remove_file(first_archive).unwrap();
    fs::remove_file(second_archive).unwrap();
    assert!(
        restored
            .restore_archive_as_copy(Path::new("absent"), &request)
            .unwrap()
            .replayed
    );
}

#[test]
fn cross_kind_global_ids_and_changed_archive_bytes_reject_without_partial_copy() {
    let f = Fixture::new();
    let (_, path, sha, _) = source_archive(&f);
    let mut target = f.store("target");
    let original = target
        .create(&ProjectDocument::new("Existing").unwrap())
        .unwrap();
    let existing = target
        .apply_edit(&intent(
            &original,
            "existing-edit-id",
            EditOperation::Rename {
                name: "Existing edit".into(),
            },
        ))
        .unwrap()
        .document;
    assert!(
        target
            .restore_archive_as_copy(Path::new("missing"), &copy_intent(&sha, "existing-edit-id"))
            .is_err()
    );
    let copied = target
        .restore_archive_as_copy(&path, &copy_intent(&sha, "existing-copy-id"))
        .unwrap();
    assert!(
        target
            .apply_edit(&intent(
                &existing,
                "existing-copy-id",
                EditOperation::Rename {
                    name: "Must reject".into()
                }
            ))
            .is_err()
    );
    assert_eq!(target.load(existing.id()).unwrap(), existing);
    assert_eq!(target.load(copied.document.id()).unwrap(), copied.document);
    let altered = f.0.join("altered.omniusong");
    fs::copy(&path, &altered).unwrap();
    let db = Connection::open(&altered).unwrap();
    db.execute_batch("PRAGMA user_version=99;").unwrap();
    drop(db);
    assert!(
        target
            .restore_archive_as_copy(&altered, &copy_intent(&sha, "changed-byte-copy"))
            .is_err()
    );
    assert_eq!(target.list().unwrap().len(), 2);
    assert_eq!(count(&f.db("target"), "archive_copies"), 1);
}

#[test]
fn corrupt_mapping_proof_and_live_operation_are_rejected_on_source_free_retry() {
    let f = Fixture::new();
    let (_, path, sha, _) = source_archive(&f);
    for (name, table, sql) in [
        (
            "bad-map",
            "archive_copy_proof_operations",
            "UPDATE archive_copy_proof_operations SET destination_result_sha256=printf('%064d',0) WHERE source_operation_id='source-first'",
        ),
        (
            "bad-proof",
            "archive_copy_proofs",
            "UPDATE archive_copy_proofs SET receipt_json=replace(receipt_json,'Imported source','Forged imported')",
        ),
        (
            "bad-live",
            "operations",
            "UPDATE operations SET result_sha256=printf('%064d',0) WHERE operation_id=(SELECT destination_operation_id FROM archive_copy_proof_operations ORDER BY source_operation_id LIMIT 1)",
        ),
    ] {
        let mut target = f.store(name);
        let request = copy_intent(&sha, "corrupt-copy");
        let copied = target.restore_archive_as_copy(&path, &request).unwrap();
        let db = f.db(name);
        remove_immutability(&db, table);
        // Proof text uses the retained source name in history/result metadata.
        if name == "bad-proof" {
            db.execute(
                "UPDATE archive_copy_proofs SET receipt_json=receipt_json||' '",
                [],
            )
            .unwrap();
        } else {
            assert_eq!(db.execute(sql, []).unwrap(), 1);
        }
        drop(db);
        drop(target);
        let mut target = f.store(name);
        assert!(
            target
                .restore_archive_as_copy(Path::new("missing-source"), &request)
                .is_err(),
            "{name}"
        );
        assert_eq!(count(&f.db(name), "projects"), 1);
        assert_eq!(count(&f.db(name), "archive_copies"), 1);
        assert_eq!(target.load(copied.document.id()).unwrap(), copied.document);
    }
}

#[test]
fn missing_ancestor_or_final_sql_abort_never_publishes_project_or_lineage() {
    let f = Fixture::new();
    let (_, path, sha, _) = source_archive(&f);
    let mut intermediate = f.store("intermediate");
    let first = intermediate
        .restore_archive_as_copy(&path, &copy_intent(&sha, "parent-copy"))
        .unwrap();
    let one = f.0.join("one.omniusong");
    let one_sha = intermediate
        .export_archive(first.document.id(), &one)
        .unwrap()
        .sha256;
    let second = intermediate
        .restore_archive_as_copy(&one, &copy_intent(&one_sha, "child-copy"))
        .unwrap();
    let corrupt = f.0.join("dangling.omniusong");
    intermediate
        .export_archive(second.document.id(), &corrupt)
        .unwrap();
    let db = Connection::open(&corrupt).unwrap();
    db.execute("DELETE FROM archive_project_lineage WHERE proof_sha256=(SELECT proof_sha256 FROM archive_copy_proofs WHERE json_array_length(json_extract(receipt_json,'$.ancestors'))=0)",[]).unwrap();
    drop(db);
    let mut target = f.store("target");
    let inspected = target.inspect_archive(&corrupt).unwrap();
    assert!(
        target
            .restore_archive_as_copy(&corrupt, &copy_intent(&inspected.sha256, "dangling-copy"))
            .is_err()
    );
    assert!(target.list().unwrap().is_empty());
    let db = f.db("target");
    db.execute_batch("CREATE TRIGGER review_final_abort BEFORE INSERT ON archive_copies BEGIN SELECT RAISE(ABORT,'independent final copy fault'); END;").unwrap();
    let failed = target
        .restore_archive_as_copy_outcome(&path, &copy_intent(&sha, "retry-final-fault"))
        .unwrap_err();
    assert_eq!(
        failed.context().action,
        omniusong_store::ArchiveAction::Copy
    );
    assert_eq!(
        failed.context().phase,
        omniusong_store::ArchivePhase::ObjectsPlaced
    );
    assert_eq!(
        failed.context().operation_id.as_deref(),
        Some("retry-final-fault")
    );
    assert_eq!(
        failed.context().expected_archive_sha256.as_deref(),
        Some(sha.as_str())
    );
    assert_eq!(
        failed.context().archive_sha256.as_deref(),
        Some(sha.as_str())
    );
    assert!(!failed.publication_uncertain());
    assert_eq!(
        failed.catalog_sqlite_error_code().unwrap() & 0xff,
        rusqlite::ffi::SQLITE_CONSTRAINT
    );
    assert!(failed.context().staging.as_ref().unwrap().is_dir());
    for table in [
        "projects",
        "edits",
        "operations",
        "imports",
        "assets",
        "native_takes",
        "archive_copy_proofs",
        "archive_copy_proof_operations",
        "archive_project_lineage",
        "archive_copies",
    ] {
        assert_eq!(count(&db, table), 0, "{table}");
    }
    db.execute_batch("DROP TRIGGER review_final_abort").unwrap();
    drop(db);
    assert!(
        !target
            .restore_archive_as_copy(&path, &copy_intent(&sha, "retry-final-fault"))
            .unwrap()
            .replayed
    );
}

#[test]
fn concurrent_identical_copy_admission_commits_one_project_and_one_complete_map() {
    let f = Fixture::new();
    let (_, path, sha, _) = source_archive(&f);
    drop(f.store("target"));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut children = Vec::new();
    for _ in 0..2 {
        let root = f.0.join("target");
        let path = path.clone();
        let request = copy_intent(&sha, "concurrent-independent-copy");
        let barrier = barrier.clone();
        children.push(std::thread::spawn(move || {
            let mut store = NativeStore::open(root).unwrap();
            barrier.wait();
            store.restore_archive_as_copy(&path, &request).unwrap()
        }));
    }
    let a = children.remove(0).join().unwrap();
    let b = children.remove(0).join().unwrap();
    assert_eq!(a.document, b.document);
    assert_ne!(a.replayed, b.replayed);
    let db = f.db("target");
    assert_eq!(count(&db, "projects"), 1);
    assert_eq!(count(&db, "archive_copies"), 1);
    assert_eq!(count(&db, "archive_copy_proofs"), 1);
    assert_eq!(count(&db, "operations"), 4);
    assert_eq!(count(&db, "archive_copy_proof_operations"), 4);
}

fn synthetic_original(
    f: &Fixture,
    source: &ProjectDocument,
) -> (TakeDescriptor, SealedJournal, PathBuf) {
    let take = TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId("source-first".into()),
        streams: vec![StreamDescriptor {
            stream_id: "source-first".into(),
            segment_id: "source-segment".into(),
            source_id: "physical-original".into(),
            source_name: "Synthetic original".into(),
            source_connection_generation: 7,
            host_clock: HostClock {
                domain: ClockDomainId("source-clock".into()),
                epoch: ClockEpochId("source-boot".into()),
                ticks_per_second_numerator: 24_000_000,
                ticks_per_second_denominator: 1,
            },
            kind: StreamKind::Audio {
                format: PcmFormat {
                    sample_rate: 48_000,
                    channels: 2,
                    encoding: PcmEncoding::S16Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: ClockEpochId("original-audio-epoch".into()),
            },
            initial_source_position: 100,
        }],
        provenance: BTreeMap::from([(
            "vendor".into(),
            json!({"project_id":source.id(),"operation_id":"source-first"}),
        )]),
    };
    let journal_path = f.0.join("original.sqlite3");
    let mut journal = Journal::create(&journal_path, &take).unwrap();
    let clock = HostTimestamp {
        domain: ClockDomainId("source-clock".into()),
        epoch: ClockEpochId("source-boot".into()),
        ticks: u64::MAX,
    };
    journal
        .append(
            &ChunkHeader {
                version: CHUNK_VERSION,
                take_id: take.take_id.clone(),
                stream_id: "source-first".into(),
                segment_id: "source-segment".into(),
                sequence: 0,
                range: SourceRange::AudioFrames {
                    start: 100,
                    count: 1,
                },
                interpretation: take.streams[0].kind.clone(),
                observed_start: clock.clone(),
                observed_end: clock,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            },
            &[1, 2, 3, 4],
        )
        .unwrap();
    journal.seal("seal-original").unwrap();
    drop(journal);
    let sealed = SealedJournal::open(&journal_path).unwrap();
    (take, sealed, journal_path)
}

#[test]
fn operation_only_take_original_survives_branch_truncation_and_wrapped_request_mapping() {
    let f = Fixture::new();
    let (source, _, _, _) = source_archive(&f);
    let (take, sealed, journal_path) = synthetic_original(&f, &source);
    let original_bytes = fs::read(&journal_path).unwrap();
    let mut store = f.store("source");
    let object = store.ingest_take(&sealed).unwrap();
    let publication = TakePublication {
        operation_id: OperationId("source-native".into()),
        project_id: ProjectId(source.id().into()),
        expected_revision: source.revision(),
        track_id: TrackId("source-first".into()),
        track_name: "Original recording".into(),
        clip: TakeClip {
            edit_source: None,
            id: "source-first".into(),
            source: TakeReference {
                take_id: take.take_id.clone(),
                manifest_sha256: sealed.manifest().sha256.clone(),
                stream_id: "source-first".into(),
            },
            range: TakeRange::AudioFrames {
                start: 100,
                count: 1,
            },
            position: TimelinePosition {
                frame: -96_000,
                sample_rate: 48_000,
            },
            fades: None,
        },
    };
    let published = store.publish_take(&publication).unwrap().document;
    let undone = store
        .apply_edit(&intent(&published, "native-undo", EditOperation::Undo))
        .unwrap()
        .document;
    let branch = store
        .apply_edit(&intent(
            &undone,
            "native-branch",
            EditOperation::Rename {
                name: "No current take reference".into(),
            },
        ))
        .unwrap()
        .document;
    assert!(branch.referenced_takes().unwrap().is_empty());
    let db = f.db("source");
    assert_eq!(history_take_references(&db), 0);
    let archive = f.0.join("native.omniusong");
    let receipt = store.export_archive(branch.id(), &archive).unwrap();
    assert_eq!(receipt.take_count, 1);
    let source_owned =
        f.0.join("source/takes")
            .join(format!("{}.sqlite3", object.object_sha256));
    let owned_bytes = fs::read(&source_owned).unwrap();
    let mut target = f.store("native-target");
    let request = copy_intent(&receipt.sha256, "op-only-copy");
    let copied = target.restore_archive_as_copy(&archive, &request).unwrap();
    assert!(copied.document.referenced_takes().unwrap().is_empty());
    assert_maps_and_snapshots(
        &db,
        &f.db("native-target"),
        source.id(),
        copied.document.id(),
    );
    let registered = target.registered_take(&take.take_id).unwrap();
    assert_eq!(registered, object);
    let target_owned =
        f.0.join("native-target/takes")
            .join(format!("{}.sqlite3", object.object_sha256));
    assert_eq!(fs::read(&target_owned).unwrap(), owned_bytes);
    let mapped:String=f.db("native-target").query_row("SELECT request_json FROM operations WHERE operation_id=(SELECT destination_operation_id FROM archive_copy_proof_operations WHERE source_operation_id='source-native')",[],|r|r.get(0)).unwrap();
    let mapped: Value = serde_json::from_str(&mapped).unwrap();
    let mapped: TakePublication = serde_json::from_value(mapped["intent"].clone()).unwrap();
    assert_eq!(mapped.clip, publication.clip);
    assert_eq!(mapped.track_id, publication.track_id);
    assert_eq!(mapped.project_id.0, copied.document.id());
    assert_ne!(mapped.operation_id, publication.operation_id);
    assert!(target.publish_take(&mapped).unwrap().replayed);
    let second = target
        .restore_archive_as_copy(
            &archive,
            &copy_intent(&receipt.sha256, "second-op-only-copy"),
        )
        .unwrap();
    assert_ne!(second.document.id(), copied.document.id());
    assert_eq!(count(&f.db("native-target"), "native_takes"), 1);
    drop(sealed);
    fs::remove_file(&archive).unwrap();
    fs::remove_file(&journal_path).unwrap();
    assert!(
        target
            .restore_archive_as_copy(Path::new("missing"), &request)
            .unwrap()
            .replayed
    );
    fs::remove_file(&target_owned).unwrap();
    assert!(
        target
            .restore_archive_as_copy(Path::new("missing"), &request)
            .is_err()
    );
    assert_eq!(fs::read(&source_owned).unwrap(), owned_bytes);
    assert!(!original_bytes.is_empty());
}

#[test]
fn ancestor_proof_result_alone_keeps_original_in_export_copy_restore_and_retry_closure() {
    let f = Fixture::new();
    let mut source = f.store("proof-source");
    let base = source
        .create(&ProjectDocument::new("Proof-only original").unwrap())
        .unwrap();
    let (take, sealed, _) = synthetic_original(&f, &base);
    let object = source.ingest_take(&sealed).unwrap();
    let mut native = base.upgrade_native().unwrap();
    native
        .append_take_clip(
            "recording",
            "Original",
            TakeClip {
                edit_source: None,
                id: "proof-only-clip".into(),
                source: TakeReference {
                    take_id: take.take_id.clone(),
                    manifest_sha256: sealed.manifest().sha256.clone(),
                    stream_id: "source-first".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 100,
                    count: 1,
                },
                position: TimelinePosition {
                    frame: -48_000,
                    sample_rate: 48_000,
                },
                fades: None,
            },
        )
        .unwrap();
    let native = source.save(&native, 0).unwrap();
    let archive = f.0.join("proof-original.omniusong");
    let exported = source.export_archive(native.id(), &archive).unwrap();
    let mut copied = f.store("proof-copy");
    let first = copied
        .restore_archive_as_copy(&archive, &copy_intent(&exported.sha256, "proof-first-copy"))
        .unwrap();
    let mut branch = copied
        .undo(first.document.id(), first.document.revision())
        .unwrap();
    branch.rename("Current state has no original").unwrap();
    let branch = copied.save(&branch, branch.revision()).unwrap();
    assert!(branch.referenced_takes().unwrap().is_empty());
    let db = f.db("proof-copy");
    assert_eq!(count(&db, "operations"), 0);
    assert_eq!(history_take_references(&db), 0);
    let proof_json: String = db
        .query_row("SELECT receipt_json FROM archive_copy_proofs", [], |r| {
            r.get(0)
        })
        .unwrap();
    let proof: Value = serde_json::from_str(&proof_json).unwrap();
    assert_eq!(
        ProjectDocument::from_value(proof["result"].clone())
            .unwrap()
            .referenced_takes()
            .unwrap()
            .len(),
        1
    );
    let proof_archive = f.0.join("proof-only.omniusong");
    let receipt = copied.export_archive(branch.id(), &proof_archive).unwrap();
    assert_eq!(receipt.take_count, 1);
    let expected = fs::read(
        f.0.join("proof-copy/takes")
            .join(format!("{}.sqlite3", object.object_sha256)),
    )
    .unwrap();
    let mut restored = f.store("proof-restored");
    assert_eq!(restored.restore_archive(&proof_archive).unwrap(), branch);
    assert_eq!(restored.registered_take(&take.take_id).unwrap(), object);
    let mut descendant = f.store("proof-descendant");
    let request = copy_intent(&receipt.sha256, "proof-descendant-copy");
    let next = descendant
        .restore_archive_as_copy(&proof_archive, &request)
        .unwrap();
    assert!(next.document.referenced_takes().unwrap().is_empty());
    assert_eq!(count(&f.db("proof-descendant"), "archive_copy_proofs"), 2);
    assert_eq!(count(&f.db("proof-descendant"), "operations"), 0);
    let owned =
        f.0.join("proof-descendant/takes")
            .join(format!("{}.sqlite3", object.object_sha256));
    assert_eq!(fs::read(&owned).unwrap(), expected);
    fs::remove_file(proof_archive).unwrap();
    assert!(
        descendant
            .restore_archive_as_copy(Path::new("gone"), &request)
            .unwrap()
            .replayed
    );
    fs::remove_file(owned).unwrap();
    assert!(
        descendant
            .restore_archive_as_copy(Path::new("gone"), &request)
            .is_err()
    );
    assert!(
        descendant
            .export_archive(next.document.id(), &f.0.join("must-not-export.omniusong"))
            .is_err()
    );
}
