//! Independent operation-ledger and additive-migration regressions.
use omniusong_core::{
    ProjectDocument,
    contracts::{
        BlobRef, CandidateId, EditOperation, EditScope, Extensions, OperationId, ProjectEditIntent,
        ProjectId,
    },
};
use omniusong_store::{NativeStore, StoreError};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, sync::Arc};

const ABC_ASSET: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.wav";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/operation-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn open(&self) -> NativeStore {
        NativeStore::open(&self.0).unwrap()
    }
    fn db(&self) -> Connection {
        Connection::open(self.0.join("studio.sqlite3")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Operation review fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn intent(project: &ProjectDocument, operation: &str, edit: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(operation.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        scope: EditScope::Project,
        edit,
        extensions: Extensions::new(),
    }
}

#[derive(Debug, PartialEq)]
struct State {
    project: (String, i64, i64),
    edits: Vec<(i64, String, String)>,
    operation_count: i64,
}
fn state(db: &Connection, id: &str) -> State {
    let project = db
        .query_row(
            "SELECT payload,revision,cursor FROM projects WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let mut query = db.prepare("SELECT sequence,before_json,after_json FROM edits WHERE project_id=?1 ORDER BY sequence").unwrap();
    let edits = query
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let operation_count = db
        .query_row("SELECT count(*) FROM operations", [], |r| r.get(0))
        .unwrap();
    State {
        project,
        edits,
        operation_count,
    }
}

#[test]
fn retry_returns_original_receipt_after_later_edits_without_reverting_current_state() {
    let root = Fixture::new();
    let mut store = root.open();
    let source = store
        .create(&ProjectDocument::new("Original").unwrap())
        .unwrap();
    let mut rename = intent(
        &source,
        "rename-1",
        EditOperation::Rename {
            name: "First edit".into(),
        },
    );
    rename.extensions.insert(
        "vendor:precise".into(),
        serde_json::from_str(
            r#"{"counter":18446744073709551617,"decimal":0.12345678901234567890123456789}"#,
        )
        .unwrap(),
    );
    let first = store.apply_edit(&rename).unwrap();
    assert!(!first.replayed);
    assert_eq!(first.previous_revision, 0);
    assert_eq!(first.operation_id, "rename-1");
    let tempo = intent(
        &first.document,
        "tempo-2",
        EditOperation::SetTempo { bpm: 137.25 },
    );
    let latest = store.apply_edit(&tempo).unwrap();
    drop(store);
    let before = state(&root.db(), source.id());
    let mut store = root.open();
    let repeated = store.apply_edit(&rename).unwrap();
    assert!(repeated.replayed);
    assert_eq!(repeated.document, first.document);
    assert_eq!(repeated.previous_revision, first.previous_revision);
    assert_eq!(store.load(source.id()).unwrap(), latest.document);
    let mut conflicting = rename.clone();
    conflicting.edit = EditOperation::Rename {
        name: "Conflicting retry".into(),
    };
    assert!(matches!(
        store.apply_edit(&conflicting),
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(state(&root.db(), source.id()), before);
    let request: String = root
        .db()
        .query_row(
            "SELECT request_json FROM operations WHERE operation_id='rename-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let encoded: serde_json::Value = serde_json::from_str(&request).unwrap();
    assert_eq!(
        encoded["vendor:precise"]["counter"].to_string(),
        "18446744073709551617"
    );
    assert_eq!(
        encoded["vendor:precise"]["decimal"].to_string(),
        "0.12345678901234567890123456789"
    );
    drop(store);
    assert_eq!(root.open().load(source.id()).unwrap(), latest.document);
}

#[test]
fn undo_and_redo_retries_remain_receipts_after_redo_branch_is_truncated() {
    let root = Fixture::new();
    let mut store = root.open();
    let p0 = store
        .create(&ProjectDocument::new("Source").unwrap())
        .unwrap();
    let p1 = store
        .apply_edit(&intent(
            &p0,
            "edit-a",
            EditOperation::Rename {
                name: "Branch A".into(),
            },
        ))
        .unwrap()
        .document;
    let undo = intent(&p1, "undo-a", EditOperation::Undo);
    let p2 = store.apply_edit(&undo).unwrap().document;
    let redo = intent(&p2, "redo-a", EditOperation::Redo);
    let p3 = store.apply_edit(&redo).unwrap().document;
    let p4 = store
        .apply_edit(&intent(&p3, "undo-again", EditOperation::Undo))
        .unwrap()
        .document;
    let p5 = store
        .apply_edit(&intent(
            &p4,
            "edit-b",
            EditOperation::Rename {
                name: "New branch B".into(),
            },
        ))
        .unwrap()
        .document;
    drop(store);
    let before = state(&root.db(), p0.id());
    let mut store = root.open();
    let old_redo = store.apply_edit(&redo).unwrap();
    assert!(old_redo.replayed);
    assert_eq!(old_redo.document, p3);
    let old_undo = store.apply_edit(&undo).unwrap();
    assert!(old_undo.replayed);
    assert_eq!(old_undo.document, p2);
    assert_eq!(store.load(p0.id()).unwrap(), p5);
    assert!(store.redo(p0.id(), p5.revision()).is_err());
    assert_eq!(state(&root.db(), p0.id()), before);
    drop(store);
    assert_eq!(root.open().load(p0.id()).unwrap(), p5);
}

#[test]
fn unsupported_invalid_and_stale_intents_publish_no_receipt_or_edit() {
    let root = Fixture::new();
    let mut store = root.open();
    let original = store
        .create(&ProjectDocument::new("Unchanged").unwrap())
        .unwrap();
    let before = state(&root.db(), original.id());
    let mut requests = vec![
        intent(
            &original,
            "accept",
            EditOperation::AcceptCandidate {
                candidate_id: CandidateId("candidate-1".into()),
            },
        ),
        intent(
            &original,
            "prepared",
            EditOperation::ApplyPreparedEdit {
                edit: BlobRef {
                    sha256: "a".repeat(64),
                    byte_len: 3,
                    media_type: "application/json".into(),
                    extensions: Extensions::new(),
                },
            },
        ),
        intent(
            &original,
            "invalid-tempo",
            EditOperation::SetTempo { bpm: f64::NAN },
        ),
    ];
    let mut stale = intent(
        &original,
        "stale",
        EditOperation::Rename {
            name: "Stale".into(),
        },
    );
    stale.expected_revision = 7;
    requests.push(stale);
    for request in requests {
        assert!(store.apply_edit(&request).is_err());
        assert_eq!(state(&root.db(), original.id()), before);
        assert_eq!(store.load(original.id()).unwrap(), original);
    }
}

#[test]
fn receipt_insert_failure_rolls_back_project_history_and_operation() {
    let root = Fixture::new();
    let mut store = root.open();
    let original = store
        .create(&ProjectDocument::new("Committed source").unwrap())
        .unwrap();
    drop(store);
    let db = root.db();
    let before = state(&db, original.id());
    db.execute_batch("CREATE TRIGGER fail_operation_receipt BEFORE INSERT ON operations BEGIN SELECT RAISE(ABORT, 'review receipt failure'); END;").unwrap();
    drop(db);
    let request = intent(
        &original,
        "atomic-rename",
        EditOperation::Rename {
            name: "Must roll back".into(),
        },
    );
    let mut store = root.open();
    assert!(matches!(
        store.apply_edit(&request),
        Err(StoreError::Database(_))
    ));
    drop(store);
    assert_eq!(state(&root.db(), original.id()), before);
    assert_eq!(root.open().load(original.id()).unwrap(), original);
    root.db()
        .execute_batch("DROP TRIGGER fail_operation_receipt;")
        .unwrap();
    let receipt = root.open().apply_edit(&request).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.document.revision(), 1);
}

#[test]
fn modified_request_or_result_receipt_is_rejected_without_rewriting_project() {
    for column in ["request_json", "result_json"] {
        let root = Fixture::new();
        let mut store = root.open();
        let original = store
            .create(&ProjectDocument::new("Receipt source").unwrap())
            .unwrap();
        let request = intent(
            &original,
            "protected",
            EditOperation::Rename {
                name: "Committed".into(),
            },
        );
        let receipt = store.apply_edit(&request).unwrap();
        drop(store);
        let db = root.db();
        // Still-valid JSON with different exact bytes must fail the retained hash.
        db.execute(
            &format!(
                "UPDATE operations SET {column}={column} || ' ' WHERE operation_id='protected'"
            ),
            [],
        )
        .unwrap();
        let damaged: String = db
            .query_row(
                &format!("SELECT {column} FROM operations WHERE operation_id='protected'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        let before = state(&db, original.id());
        drop(db);
        let mut store = root.open();
        assert!(matches!(
            store.apply_edit(&request),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(store.load(original.id()).unwrap(), receipt.document);
        drop(store);
        let db = root.db();
        assert_eq!(state(&db, original.id()), before);
        assert_eq!(
            db.query_row(
                &format!("SELECT {column} FROM operations WHERE operation_id='protected'"),
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            damaged
        );
    }
}

#[test]
fn schema_one_upgrade_preserves_assets_imports_and_history_before_new_operations() {
    let root = Fixture::new();
    let mut value = ProjectDocument::new("Legacy schema-one source")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["sourceAssetId"] = ABC_ASSET.into();
    value["vendor:unknown"] = json!({"keep":"exact", "clock":"18446744073709551615"});
    let source = ProjectDocument::from_value(value).unwrap();
    let raw = source.to_json().unwrap();
    let imported = source
        .imported_copy(&format!("{:x}", Sha256::digest(&raw)))
        .unwrap();
    let mut current = imported.with_revision(1).unwrap();
    current
        .rename("Already edited legacy native session")
        .unwrap();
    let before_json = String::from_utf8(imported.to_json().unwrap()).unwrap();
    let current_json = String::from_utf8(current.to_json().unwrap()).unwrap();
    let db = root.db();
    db.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY NOT NULL,payload TEXT NOT NULL,revision INTEGER NOT NULL,cursor INTEGER NOT NULL);
        CREATE TABLE edits(project_id TEXT NOT NULL REFERENCES projects(id),sequence INTEGER NOT NULL,before_json TEXT NOT NULL,after_json TEXT NOT NULL,PRIMARY KEY(project_id,sequence));
        CREATE TABLE assets(id TEXT PRIMARY KEY NOT NULL,payload BLOB NOT NULL);
        CREATE TABLE imports(project_id TEXT PRIMARY KEY NOT NULL REFERENCES projects(id),source_sha256 TEXT NOT NULL,original_json BLOB NOT NULL);
        PRAGMA user_version=1;").unwrap();
    db.execute(
        "INSERT INTO projects VALUES(?1,?2,1,1)",
        params![current.id(), current_json],
    )
    .unwrap();
    db.execute(
        "INSERT INTO edits VALUES(?1,1,?2,?3)",
        params![current.id(), before_json, current_json],
    )
    .unwrap();
    db.execute(
        "INSERT INTO assets VALUES(?1,?2)",
        params![ABC_ASSET, b"abc".as_slice()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO imports VALUES(?1,?2,?3)",
        params![current.id(), format!("{:x}", Sha256::digest(&raw)), raw],
    )
    .unwrap();
    drop(db);
    let mut store = root.open();
    let db = root.db();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        4
    );
    let stored_json: String = db
        .query_row(
            "SELECT payload FROM projects WHERE id=?1",
            [current.id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored_json, current_json);
    assert_eq!(
        db.query_row("SELECT count(*) FROM operations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT before_json FROM edits WHERE project_id=?1",
            [current.id()],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        before_json
    );
    assert_eq!(store.load(current.id()).unwrap(), current);
    assert_eq!(store.asset(ABC_ASSET).unwrap(), b"abc");
    assert_eq!(store.imported_original(current.id()).unwrap().unwrap(), raw);
    let undone = store.undo(current.id(), 1).unwrap();
    assert_eq!(undone.name(), imported.name());
    let redone = store.redo(current.id(), 2).unwrap();
    assert_eq!(redone.name(), current.name());
    assert_eq!(redone.tracks(), source.tracks());
    drop(store);
    assert_eq!(root.open().load(current.id()).unwrap(), redone);
}

#[test]
fn future_store_schema_is_rejected_without_changing_database_bytes() {
    let root = Fixture::new();
    let mut store = root.open();
    let project = store
        .create(&ProjectDocument::new("Future fixture").unwrap())
        .unwrap();
    drop(store);
    let db = root.db();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    let before = fs::read(root.0.join("studio.sqlite3")).unwrap();
    assert!(matches!(
        NativeStore::open(&root.0),
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(fs::read(root.0.join("studio.sqlite3")).unwrap(), before);
    assert_eq!(
        root.db()
            .query_row(
                "SELECT count(*) FROM projects WHERE id=?1",
                [project.id()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn simultaneous_first_open_migrates_once_and_admits_independent_operations() {
    let root = Fixture::new();
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|index| {
            let directory = root.0.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let mut store = NativeStore::open(directory).unwrap();
                let project = store
                    .create(&ProjectDocument::new("Concurrent original").unwrap())
                    .unwrap();
                let request = intent(
                    &project,
                    &format!("concurrent-{index}"),
                    EditOperation::Rename {
                        name: format!("Writer {index}"),
                    },
                );
                store.apply_edit(&request).unwrap().document.id().to_owned()
            })
        })
        .collect();
    let ids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let store = root.open();
    assert_eq!(store.list().unwrap().len(), 4);
    for id in ids {
        assert_eq!(store.load(&id).unwrap().revision(), 1);
    }
    let db = root.db();
    assert_eq!(
        db.query_row("SELECT count(*) FROM operations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        4
    );
}

#[test]
fn concurrent_identical_operation_retries_publish_exactly_one_edit() {
    let root = Fixture::new();
    let mut store = root.open();
    let source = store
        .create(&ProjectDocument::new("One human action").unwrap())
        .unwrap();
    drop(store);
    let request = intent(
        &source,
        "same-operation",
        EditOperation::Rename {
            name: "Exactly once".into(),
        },
    );
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let root = root.0.clone();
            let barrier = barrier.clone();
            let request = request.clone();
            std::thread::spawn(move || {
                let mut store = NativeStore::open(root).unwrap();
                barrier.wait();
                store.apply_edit(&request).unwrap()
            })
        })
        .collect();
    let receipts: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(receipts.iter().filter(|r| r.replayed).count(), 1);
    assert_eq!(receipts[0].document, receipts[1].document);
    assert_eq!(root.open().load(source.id()).unwrap().revision(), 1);
    let snapshot = state(&root.db(), source.id());
    assert_eq!(snapshot.edits.len(), 1);
    assert_eq!(snapshot.operation_count, 1);
}

#[test]
fn just_over_cap_database_rows_are_rejected_without_publishing_edits() {
    use omniusong_core::{
        MAX_PROJECT_BYTES, contracts::WORKER_MAX_BYTES, native::MAX_NATIVE_PROJECT_BYTES,
    };
    // SQLite creates bounded fault rows directly; Rust never builds the large blob.
    for target in ["project", "history", "request", "result", "import", "asset"] {
        let root = Fixture::new();
        let mut store = root.open();
        let source = store
            .create(&ProjectDocument::new("Bounded rows").unwrap())
            .unwrap();
        let request = intent(
            &source,
            "bounded-operation",
            EditOperation::Rename {
                name: "Committed".into(),
            },
        );
        let committed = store.apply_edit(&request).unwrap().document;
        let db = root.db();
        match target {
            "project" => {
                db.execute(
                    "UPDATE projects SET payload=CAST(zeroblob(?2) AS TEXT) WHERE id=?1",
                    params![
                        source.id(),
                        i64::try_from(MAX_NATIVE_PROJECT_BYTES + 1).unwrap()
                    ],
                )
                .unwrap();
                assert!(matches!(
                    store.load(source.id()),
                    Err(StoreError::Invalid(_))
                ));
                let listed = store.list().unwrap();
                assert_eq!(listed.len(), 1);
                assert!(listed[0].problem.is_some());
            }
            "history" => {
                db.execute(
                    "UPDATE edits SET before_json=CAST(zeroblob(?2) AS TEXT) WHERE project_id=?1",
                    params![
                        source.id(),
                        i64::try_from(MAX_NATIVE_PROJECT_BYTES + 1).unwrap()
                    ],
                )
                .unwrap();
                assert!(store.undo(source.id(), committed.revision()).is_err());
                assert_eq!(store.load(source.id()).unwrap(), committed);
            }
            "request" | "result" => {
                let (column, cap) = if target == "request" {
                    ("request_json", WORKER_MAX_BYTES)
                } else {
                    ("result_json", MAX_NATIVE_PROJECT_BYTES)
                };
                db.execute(
                    &format!("UPDATE operations SET {column}=CAST(zeroblob(?1) AS TEXT)"),
                    [i64::try_from(cap + 1).unwrap()],
                )
                .unwrap();
                assert!(store.apply_edit(&request).is_err());
                assert_eq!(store.load(source.id()).unwrap(), committed);
            }
            "import" => {
                db.execute("INSERT INTO imports(project_id,source_sha256,original_json) VALUES(?1,?2,zeroblob(?3))",params![source.id(),"0".repeat(64),i64::try_from(MAX_PROJECT_BYTES+1).unwrap()]).unwrap();
                assert!(store.imported_original(source.id()).is_err());
            }
            _ => {
                db.execute(
                    "INSERT INTO assets(id,payload) VALUES(?1,zeroblob(?2))",
                    params![ABC_ASSET, 30 * 1024 * 1024 + 1],
                )
                .unwrap();
                assert!(matches!(
                    store.asset(ABC_ASSET),
                    Err(StoreError::Invalid(_))
                ));
            }
        }
        let (revision,cursor,operations):(i64,i64,i64) = db.query_row("SELECT revision,cursor,(SELECT count(*) FROM operations) FROM projects WHERE id=?1",[source.id()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(
            (revision, cursor, operations),
            (1, 1, 1),
            "failed guard published state for {target}"
        );
    }
}
