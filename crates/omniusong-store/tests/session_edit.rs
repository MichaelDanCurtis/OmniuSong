//! A name/tempo draft is one durable operation and one reversible user action.
use omniusong_core::{
    ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
};
use omniusong_store::NativeStore;
use rusqlite::Connection;
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Session edit fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn intent(document: &ProjectDocument, id: &str, edit: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        scope: EditScope::Project,
        edit,
        extensions: Default::default(),
    }
}

#[test]
fn paired_session_save_is_atomic_reversible_and_idempotent_through_later_edits() {
    let fixture = Fixture(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/session-edit")
            .join(uuid::Uuid::new_v4().to_string()),
    );
    let mut store = NativeStore::open(&fixture.0).unwrap();
    let mut value = ProjectDocument::new("Original name")
        .unwrap()
        .value()
        .clone();
    value["vendor"] = serde_json::from_str(r#"{"decimal":0.123456789012345678901234567890123456789,"integer":18446744073709551616001}"#).unwrap();
    let original = store
        .create(&ProjectDocument::from_value(value).unwrap())
        .unwrap();
    let save = intent(
        &original,
        "paired-save",
        EditOperation::UpdateSession {
            name: "Changed name".into(),
            tempo: 117.25,
        },
    );
    let receipt = store.apply_edit(&save).unwrap();
    assert_eq!(receipt.document.revision(), 1);
    assert_eq!(receipt.document.name(), "Changed name");
    assert_eq!(receipt.document.tempo(), 117.25);
    let later = store
        .apply_edit(&intent(
            &receipt.document,
            "later",
            EditOperation::SetTempo { bpm: 130.0 },
        ))
        .unwrap()
        .document;
    let retry = store.apply_edit(&save).unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.document, receipt.document);
    assert_eq!(store.load(original.id()).unwrap(), later);
    let first_undo = store.undo(original.id(), later.revision()).unwrap();
    assert_eq!(first_undo.name(), "Changed name");
    assert_eq!(first_undo.tempo(), 117.25);
    let second_undo = store.undo(original.id(), first_undo.revision()).unwrap();
    assert_eq!(second_undo.name(), original.name());
    assert_eq!(second_undo.tempo(), original.tempo());
    let redone = store.redo(original.id(), second_undo.revision()).unwrap();
    assert_eq!(redone.name(), "Changed name");
    assert_eq!(redone.tempo(), 117.25);
    assert_eq!(redone.value()["vendor"], original.value()["vendor"]);
    let invalid = intent(
        &redone,
        "invalid",
        EditOperation::UpdateSession {
            name: "Must not change".into(),
            tempo: 301.0,
        },
    );
    assert!(store.apply_edit(&invalid).is_err());
    let db = Connection::open(fixture.0.join("studio.sqlite3")).unwrap();
    let before: (i64, i64) = db
        .query_row(
            "SELECT (SELECT count(*) FROM operations),(SELECT count(*) FROM edits)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    db.execute_batch("CREATE TRIGGER final_pair_failure BEFORE INSERT ON operations WHEN NEW.operation_id='failed-pair' BEGIN SELECT RAISE(ABORT,'Injected final receipt failure'); END;").unwrap();
    let failed = intent(
        &redone,
        "failed-pair",
        EditOperation::UpdateSession {
            name: "Must roll back".into(),
            tempo: 99.0,
        },
    );
    assert!(store.apply_edit(&failed).is_err());
    let after: (i64, i64) = db
        .query_row(
            "SELECT (SELECT count(*) FROM operations),(SELECT count(*) FROM edits)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(store.load(original.id()).unwrap(), redone);
    drop(store);
    assert_eq!(
        NativeStore::open(&fixture.0)
            .unwrap()
            .load(original.id())
            .unwrap(),
        redone
    );
}
