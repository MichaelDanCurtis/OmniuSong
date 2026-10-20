//! Owner integration cases for durable note admission and complete archives.
use omniusong_core::{
    DocumentNoteAddress, NoteTimelinePolicy, NoteValues, ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
};
use omniusong_store::{NativeStore, RestoreCopyIntent, StoreError};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/note-edit-owner")
                .join(uuid::Uuid::new_v4().to_string()),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Note edit fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
const KEEP: NoteTimelinePolicy = NoteTimelinePolicy::PreserveLoop;
fn changed() -> NoteValues {
    NoteValues {
        pitch: 65,
        start_tick: 3957,
        duration_ticks: 1201,
        velocity16: 0,
    }
}
fn source_fixture(store: &mut NativeStore) -> (ProjectDocument, DocumentNoteAddress) {
    let mut value = ProjectDocument::new("Durable exact notes")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["id"] = "🎹".repeat(200).into();
    value["tracks"][0]["notes"] = json!([{"id":"","pitch":60,"startTick":123,"durationTicks":987,"velocity16":49123,"expression":[{"type":"pitch","points":[{"tick":0,"value32":2147483648u32},{"tick":700,"value32":2271560481u32}]}],"input":{"sourceId":"synthetic","sourceName":"Synthetic owner fixture","protocol":"midi2","group":3,"channel":2,"receivedTimeMs":10.25,"keyDurationTicks":500,"releaseVelocity16":37155},"vendor":serde_json::from_str::<Value>(r#"{"uuid":"do-not-remap","huge":184467440737095516160001,"decimal":0.123456789012345678901234567890123456789}"#).unwrap()}]);
    value["vendor:opaque"] = value["tracks"][0]["notes"][0]["vendor"].clone();
    let address = DocumentNoteAddress {
        track_id: "🎹".repeat(200),
        note_id: String::new(),
    };
    (
        store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap(),
        address,
    )
}
fn edit(
    doc: &ProjectDocument,
    id: &str,
    address: &DocumentNoteAddress,
    edit: EditOperation,
) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(doc.id().into()),
        expected_revision: doc.revision(),
        scope: EditScope::DocumentNote {
            address: address.clone(),
        },
        edit,
        extensions: Default::default(),
    }
}
fn history(doc: &ProjectDocument, id: &str, edit: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(doc.id().into()),
        expected_revision: doc.revision(),
        scope: EditScope::Project,
        edit,
        extensions: Default::default(),
    }
}
#[derive(Debug, PartialEq)]
struct State {
    payload: String,
    revision: i64,
    cursor: i64,
    edits: Vec<(i64, String, String)>,
    operations: Vec<(String, String, String)>,
}
fn state(db: &Connection, id: &str) -> State {
    let (payload, revision, cursor) = db
        .query_row(
            "SELECT payload,revision,cursor FROM projects WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let mut query=db.prepare("SELECT sequence,before_json,after_json FROM edits WHERE project_id=?1 ORDER BY sequence").unwrap();
    let edits = query
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut query=db.prepare("SELECT operation_id,request_json,result_json FROM operations WHERE project_id=?1 ORDER BY operation_id").unwrap();
    let operations = query
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    State {
        payload,
        revision,
        cursor,
        edits,
        operations,
    }
}
#[test]
fn lost_ack_after_delete_replays_original_result_without_reapplying_or_rebasing() {
    let fixture = Fixture::new();
    let root = fixture.0.join("store");
    let mut store = NativeStore::open(&root).unwrap();
    let (original, address) = source_fixture(&mut store);
    let request = edit(
        &original,
        "update-original",
        &address,
        EditOperation::UpdateNote {
            values: changed(),
            timeline: KEEP,
        },
    );
    let saved = store.apply_edit(&request).unwrap();
    let deleted = store
        .apply_edit(&edit(
            &saved.document,
            "delete-later",
            &address,
            EditOperation::DeleteNote,
        ))
        .unwrap()
        .document;
    let replay = store.apply_edit(&request).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, saved.document);
    assert_eq!(store.load(original.id()).unwrap(), deleted);
    let db = Connection::open(root.join("studio.sqlite3")).unwrap();
    let before = state(&db, original.id());
    let mut conflict = request.clone();
    conflict.edit = EditOperation::UpdateNote {
        values: NoteValues {
            velocity16: 1,
            ..changed()
        },
        timeline: KEEP,
    };
    assert!(store.apply_edit(&conflict).is_err());
    assert!(matches!(
        store.apply_edit(&edit(
            &original,
            "stale-new-id",
            &address,
            EditOperation::DeleteNote
        )),
        Err(StoreError::Conflict)
    ));
    assert_eq!(state(&db, original.id()), before);
    drop(store);
    drop(db);
    let mut reopened = NativeStore::open_existing(&root).unwrap();
    assert_eq!(reopened.load(original.id()).unwrap(), deleted);
    assert_eq!(
        reopened.apply_edit(&request).unwrap().document,
        saved.document
    );
}
#[test]
fn core_rejections_are_known_and_final_receipt_abort_rolls_back_every_row() {
    let fixture = Fixture::new();
    let root = fixture.0.join("store");
    let mut store = NativeStore::open(&root).unwrap();
    let (original, address) = source_fixture(&mut store);
    let db = Connection::open(root.join("studio.sqlite3")).unwrap();
    let before = state(&db, original.id());
    let invalid = edit(
        &original,
        "shorten-expression",
        &address,
        EditOperation::UpdateNote {
            values: NoteValues {
                duration_ticks: 699,
                ..changed()
            },
            timeline: KEEP,
        },
    );
    assert!(matches!(
        store.apply_edit(&invalid),
        Err(StoreError::EditRejected(_))
    ));
    let no_op = edit(
        &original,
        "same-values",
        &address,
        EditOperation::UpdateNote {
            values: NoteValues {
                pitch: 60,
                start_tick: 123,
                duration_ticks: 987,
                velocity16: 49123,
            },
            timeline: KEEP,
        },
    );
    assert!(matches!(
        store.apply_edit(&no_op),
        Err(StoreError::EditRejected(_))
    ));
    assert_eq!(state(&db, original.id()), before);
    db.execute_batch("CREATE TRIGGER fail_note_receipt BEFORE INSERT ON operations WHEN NEW.operation_id='abort-note' BEGIN SELECT RAISE(ABORT,'Injected note receipt failure'); END;").unwrap();
    let request = edit(
        &original,
        "abort-note",
        &address,
        EditOperation::UpdateNote {
            values: changed(),
            timeline: KEEP,
        },
    );
    assert!(matches!(
        store.apply_edit(&request),
        Err(StoreError::Database(_))
    ));
    assert_eq!(state(&db, original.id()), before);
    db.execute_batch("DROP TRIGGER fail_note_receipt").unwrap();
    assert_eq!(store.apply_edit(&request).unwrap().document.revision(), 1);
}
#[test]
fn all_four_operations_survive_undo_redo_complete_archive_and_typed_copy() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    let mut store = NativeStore::open(&root).unwrap();
    let (original, address) = source_fixture(&mut store);
    let manual = DocumentNoteAddress {
        note_id: "manual-note".into(),
        ..address.clone()
    };
    let added = store
        .apply_edit(&edit(
            &original,
            "add-manual",
            &manual,
            EditOperation::AddNote {
                values: changed(),
                timeline: KEEP,
            },
        ))
        .unwrap()
        .document;
    let updated = store
        .apply_edit(&edit(
            &added,
            "update-human",
            &address,
            EditOperation::UpdateNote {
                values: changed(),
                timeline: KEEP,
            },
        ))
        .unwrap()
        .document;
    let restored = store
        .apply_edit(&edit(
            &updated,
            "restore-human",
            &address,
            EditOperation::RestoreNote { timeline: KEEP },
        ))
        .unwrap()
        .document;
    assert_eq!(
        restored.tracks()[0]["notes"][0]["input"],
        original.tracks()[0]["notes"][0]["input"]
    );
    assert_eq!(
        restored.tracks()[0]["notes"][0]["vendor"],
        original.tracks()[0]["notes"][0]["vendor"]
    );
    assert!(restored.tracks()[0]["notes"][0].get("editSource").is_some());
    let deleted = store
        .apply_edit(&edit(
            &restored,
            "delete-manual",
            &manual,
            EditOperation::DeleteNote,
        ))
        .unwrap()
        .document;
    let undone = store
        .apply_edit(&history(&deleted, "undo-delete", EditOperation::Undo))
        .unwrap()
        .document;
    assert_eq!(undone.tracks(), restored.tracks());
    let redone = store
        .apply_edit(&history(&undone, "redo-delete", EditOperation::Redo))
        .unwrap()
        .document;
    assert_eq!(redone.tracks(), deleted.tracks());
    let archive = fixture.0.join("complete.omniusong-native");
    let receipt = store.export_archive(original.id(), &archive).unwrap();
    assert_eq!(receipt.operation_rows, 6);
    assert_eq!(receipt.history_rows, 4);
    let identity_root = fixture.0.join("identity");
    let mut identity = NativeStore::open(&identity_root).unwrap();
    let identity_doc = identity.restore_archive(&archive).unwrap();
    assert_eq!(identity_doc, redone);
    drop(identity);
    assert_eq!(
        NativeStore::open_existing(&identity_root)
            .unwrap()
            .load(original.id())
            .unwrap(),
        redone
    );
    let copy_root = fixture.0.join("copy");
    let mut copy = NativeStore::open(&copy_root).unwrap();
    let copied = copy
        .restore_archive_as_copy(
            &archive,
            &RestoreCopyIntent {
                operation_id: OperationId("copy-notes".into()),
                expected_archive_sha256: receipt.sha256,
            },
        )
        .unwrap()
        .document;
    assert_ne!(copied.id(), original.id());
    assert_eq!(copied.tracks(), redone.tracks());
    assert_eq!(
        copied.value()["vendor:opaque"],
        original.value()["vendor:opaque"]
    );
    let db = Connection::open(copy_root.join("studio.sqlite3")).unwrap();
    let mut query=db.prepare("SELECT operation_id,request_json FROM operations WHERE project_id=?1 ORDER BY operation_id").unwrap();
    let rows = query
        .query_map([copied.id()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(rows.len(), 6);
    for (id, raw) in rows {
        assert!(
            ![
                "add-manual",
                "update-human",
                "restore-human",
                "delete-manual",
                "undo-delete",
                "redo-delete"
            ]
            .contains(&id.as_str())
        );
        let request: ProjectEditIntent = serde_json::from_str(&raw).unwrap();
        assert_eq!(request.project_id.0, copied.id());
        if let EditScope::DocumentNote {
            address: copied_address,
        } = request.scope
        {
            assert_eq!(copied_address.track_id, address.track_id);
            assert!(copied_address.note_id.is_empty() || copied_address.note_id == manual.note_id);
        }
    }
}
#[test]
fn explicit_extent_promotion_is_a_single_durable_reversible_note_action() {
    let fixture = Fixture::new();
    let root = fixture.0.join("store");
    let mut store = NativeStore::open(&root).unwrap();
    let (original, address) = source_fixture(&mut store);
    let long = NoteValues {
        start_tick: 1_000_000_000,
        duration_ticks: 100_000_000,
        ..changed()
    };
    let preserved = store
        .apply_edit(&edit(
            &original,
            "long-preserve",
            &address,
            EditOperation::UpdateNote {
                values: long,
                timeline: KEEP,
            },
        ))
        .unwrap()
        .document;
    assert_eq!(preserved.schema_version(), 1);
    assert_eq!(preserved.bars(), original.bars());
    let extended = store
        .apply_edit(&edit(
            &preserved,
            "only-extent",
            &address,
            EditOperation::UpdateNote {
                values: long,
                timeline: NoteTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: true,
                },
            },
        ))
        .unwrap()
        .document;
    assert_eq!(extended.schema_version(), 2);
    assert_eq!(extended.tracks(), preserved.tracks());
    let undone = store
        .apply_edit(&history(&extended, "undo-promotion", EditOperation::Undo))
        .unwrap()
        .document;
    assert_eq!(undone.schema_version(), 1);
    assert_eq!(undone.tracks(), preserved.tracks());
    assert_eq!(undone.bars(), preserved.bars());
    let redone = store
        .apply_edit(&history(&undone, "redo-promotion", EditOperation::Redo))
        .unwrap()
        .document;
    assert_eq!(redone.schema_version(), 2);
    assert_eq!(redone.bars(), extended.bars());
}
