//! Independent note receipts, rollback, retained history and archive-copy closure.
use omniusong_core::{
    DocumentNoteAddress, NoteTimelinePolicy, NoteValues, ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
};
use omniusong_store::{NativeStore, RestoreCopyIntent, StoreError};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const KEEP: NoteTimelinePolicy = NoteTimelinePolicy::PreserveLoop;
const TRACK: &str = " 🎹\0legacy";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/note-edit-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(path)
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
            eprintln!("Note edit review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn document() -> ProjectDocument {
    let mut value = ProjectDocument::new("Imported note source")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["id"] = TRACK.into();
    value["tracks"][0]["notes"] = json!([{"id":"","pitch":61,"startTick":17,"durationTicks":961,"velocity16":32769,"expression":[{"type":"pressure","points":[{"tick":961,"value32":4294967295u32}],"foreign":"lane"}],"input":{"sourceId":"synthetic","sourceName":"Synthetic keys","protocol":"midi2","group":3,"channel":4,"receivedTimeMs":12.25,"releaseVelocity16":60001,"keyDurationTicks":321},"foreign":{"id":"opaque-id","takeId":"not-a-reference"}}]);
    value["inputCapture"] = json!({"version":1,"droppedEvents":0,"events":[{"trackId":TRACK,"tick":17,"sourceId":"synthetic","sourceName":"Synthetic keys","receivedTimeMs":12.25,"hostTime":"18446744073709551615","format":"ump","words":[1083194368u32,4294967295u32]}]});
    value["vendor"] = serde_json::from_str(r#"{"integer":18446744073709551616001,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
    ProjectDocument::from_value(value).unwrap()
}
fn values() -> NoteValues {
    NoteValues {
        pitch: 127,
        start_tick: 19,
        duration_ticks: 963,
        velocity16: 0,
    }
}
fn intent(doc: &ProjectDocument, id: &str, note: &str, edit: EditOperation) -> ProjectEditIntent {
    let scope = if matches!(
        edit,
        EditOperation::Undo | EditOperation::Redo | EditOperation::Rename { .. }
    ) {
        EditScope::Project
    } else {
        EditScope::DocumentNote {
            address: DocumentNoteAddress {
                track_id: TRACK.into(),
                note_id: note.into(),
            },
        }
    };
    let mut result = ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(doc.id().into()),
        expected_revision: doc.revision(),
        scope,
        edit,
        extensions: Default::default(),
    };
    result.extensions.insert(
        "vendor".into(),
        json!({"project_id":doc.id(),"operation_id":id,"precision":doc.value()["vendor"]}),
    );
    result
}
fn state(db: &Connection, id: &str) -> (String, i64, i64, i64, i64) {
    db.query_row("SELECT payload,revision,cursor,(SELECT count(*) FROM edits),(SELECT count(*) FROM operations) FROM projects WHERE id=?1", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap()
}

#[test]
fn exact_retry_after_later_edit_returns_original_receipt_without_reverting_current() {
    let f = Fixture::new();
    let mut store = f.store("source");
    let original = store.create(&document()).unwrap();
    let edit = intent(
        &original,
        "note-update",
        "",
        EditOperation::UpdateNote {
            values: values(),
            timeline: KEEP,
        },
    );
    let first = store.apply_edit(&edit).unwrap();
    assert!(!first.replayed);
    assert_eq!(first.document.revision(), original.revision() + 1);
    let deleted = store
        .apply_edit(&intent(
            &first.document,
            "note-delete",
            "",
            EditOperation::DeleteNote,
        ))
        .unwrap()
        .document;
    let replay = store.apply_edit(&edit).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, first.document);
    assert_eq!(store.load(original.id()).unwrap(), deleted);
    let mut different = edit.clone();
    different.edit = EditOperation::UpdateNote {
        values: NoteValues {
            velocity16: 1,
            ..values()
        },
        timeline: KEEP,
    };
    assert!(store.apply_edit(&different).is_err());
    let mut stale = edit.clone();
    stale.operation_id = OperationId("fresh-but-stale".into());
    assert!(store.apply_edit(&stale).is_err());
    let undone = store
        .apply_edit(&intent(&deleted, "undo-delete", "", EditOperation::Undo))
        .unwrap()
        .document;
    assert_eq!(undone.tracks(), first.document.tracks());
    let redone = store
        .apply_edit(&intent(&undone, "redo-delete", "", EditOperation::Redo))
        .unwrap()
        .document;
    assert_eq!(redone.tracks(), deleted.tracks());
    drop(store);
    let mut reopened = f.store("source");
    assert_eq!(reopened.load(original.id()).unwrap(), redone);
    assert!(reopened.apply_edit(&edit).unwrap().replayed);
    assert_eq!(reopened.load(original.id()).unwrap(), redone);
}

#[test]
fn final_receipt_failure_rolls_back_note_history_cursor_and_revision_then_exact_retry_works() {
    let f = Fixture::new();
    let mut store = f.store("source");
    let original = store.create(&document()).unwrap();
    let db = f.db("source");
    let before = state(&db, original.id());
    db.execute_batch("CREATE TRIGGER note_final_failure BEFORE INSERT ON operations WHEN NEW.operation_id='failed-note' BEGIN SELECT RAISE(ABORT,'Injected note receipt failure'); END;").unwrap();
    let request = intent(
        &original,
        "failed-note",
        "",
        EditOperation::UpdateNote {
            values: values(),
            timeline: KEEP,
        },
    );
    assert!(matches!(
        store.apply_edit(&request),
        Err(StoreError::Database(_))
    ));
    assert_eq!(state(&db, original.id()), before);
    assert_eq!(store.load(original.id()).unwrap(), original);
    db.execute_batch("DROP TRIGGER note_final_failure").unwrap();
    let committed = store.apply_edit(&request).unwrap();
    assert!(!committed.replayed);
    assert_eq!(committed.document.revision(), 1);
    assert!(store.apply_edit(&request).unwrap().replayed);
    assert_eq!(state(&db, original.id()).3, 1);
    assert_eq!(state(&db, original.id()).4, 1);
}

#[test]
fn invalid_scope_locked_noop_and_wrong_kind_never_create_durable_admission() {
    let f = Fixture::new();
    let mut store = f.store("source");
    for mode in ["noop", "locked", "audio", "scope"] {
        let mut value = document().value().clone();
        value["id"] = uuid::Uuid::new_v4().to_string().into();
        if mode == "locked" {
            value["tracks"][0]["locked"] = true.into();
        }
        if mode == "audio" {
            value["tracks"][0]["kind"] = "audio".into();
        }
        let original = store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap();
        let db = f.db("source");
        let before = state(&db, original.id());
        let mut request = intent(
            &original,
            mode,
            "",
            EditOperation::UpdateNote {
                values: if mode == "noop" {
                    NoteValues {
                        pitch: 61,
                        start_tick: 17,
                        duration_ticks: 961,
                        velocity16: 32769,
                    }
                } else {
                    values()
                },
                timeline: KEEP,
            },
        );
        if mode == "scope" {
            request.scope = EditScope::Project;
        }
        let failure = store.apply_edit(&request).unwrap_err();
        if mode == "scope" {
            assert!(matches!(failure, StoreError::Invalid(_)));
        } else {
            assert!(matches!(failure, StoreError::EditRejected(_)));
        }
        assert_eq!(state(&db, original.id()), before);
        assert_eq!(store.load(original.id()).unwrap(), original);
    }
}

#[test]
fn archived_notes_keep_truncated_operation_results_exact_import_and_copy_identity_mapping() {
    let f = Fixture::new();
    let original_source = document();
    let mut original_bytes = b" \n\t".to_vec();
    original_bytes.extend(original_source.to_json().unwrap());
    original_bytes.extend_from_slice(b"\n ");
    let import_path = f.0.join("source.json");
    fs::write(&import_path, &original_bytes).unwrap();
    let assets = f.0.join("assets");
    fs::create_dir(&assets).unwrap();
    let mut source = f.store("source");
    let imported = source.import_legacy(&import_path, &assets).unwrap();
    let update = intent(
        &imported,
        "edited",
        "",
        EditOperation::UpdateNote {
            values: values(),
            timeline: KEEP,
        },
    );
    let edited = source.apply_edit(&update).unwrap().document;
    let added = source
        .apply_edit(&intent(
            &edited,
            "added",
            "new-note",
            EditOperation::AddNote {
                values: values(),
                timeline: KEEP,
            },
        ))
        .unwrap()
        .document;
    let delete = intent(&added, "deleted", "", EditOperation::DeleteNote);
    let deleted = source.apply_edit(&delete).unwrap().document;
    let undone = source
        .apply_edit(&intent(&deleted, "undo-delete", "", EditOperation::Undo))
        .unwrap()
        .document;
    let restore = intent(
        &undone,
        "restored",
        "",
        EditOperation::RestoreNote { timeline: KEEP },
    );
    let restored = source.apply_edit(&restore).unwrap().document;
    let undone_again = source
        .apply_edit(&intent(&restored, "undo-restore", "", EditOperation::Undo))
        .unwrap()
        .document;
    let current = source
        .apply_edit(&intent(
            &undone_again,
            "branch",
            "",
            EditOperation::Rename {
                name: "New branch".into(),
            },
        ))
        .unwrap()
        .document;
    let archive = f.0.join("notes.omniusong-native");
    source.export_archive(imported.id(), &archive).unwrap();
    let archive_before = fs::read(&archive).unwrap();
    let mut identity = f.store("identity");
    assert_eq!(identity.restore_archive(&archive).unwrap(), current);
    assert_eq!(
        identity.imported_original(current.id()).unwrap().unwrap(),
        original_bytes
    );
    assert_eq!(identity.apply_edit(&delete).unwrap().document, deleted);
    assert_eq!(identity.apply_edit(&restore).unwrap().document, restored);
    assert_eq!(identity.load(current.id()).unwrap(), current);
    let mut copied_store = f.store("copy");
    let inspection = copied_store.inspect_archive(&archive).unwrap();
    let copy = copied_store
        .restore_archive_as_copy(
            &archive,
            &RestoreCopyIntent {
                operation_id: OperationId("copy-notes".into()),
                expected_archive_sha256: inspection.sha256,
            },
        )
        .unwrap()
        .document;
    assert_ne!(copy.id(), current.id());
    let mut expected = current.value().clone();
    expected["id"] = copy.id().into();
    assert_eq!(copy.value(), &expected);
    assert_eq!(
        copied_store.imported_original(copy.id()).unwrap().unwrap(),
        original_bytes
    );
    let source_db = f.db("source");
    let copied_db = f.db("copy");
    let rows: Vec<(String,String)> = copied_db.prepare("SELECT source_operation_id,destination_operation_id FROM archive_copy_proof_operations ORDER BY source_operation_id").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(rows.len(), 7);
    for (old_id, new_id) in rows {
        assert_ne!(old_id, new_id);
        let (old_request, old_result): (String, String) = source_db
            .query_row(
                "SELECT request_json,result_json FROM operations WHERE operation_id=?1",
                [&old_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let (new_request, new_result): (String, String) = copied_db
            .query_row(
                "SELECT request_json,result_json FROM operations WHERE operation_id=?1",
                [&new_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut expected_request: Value = serde_json::from_str(&old_request).unwrap();
        expected_request["operation_id"] = new_id.clone().into();
        expected_request["project_id"] = copy.id().into();
        assert_eq!(
            serde_json::from_str::<Value>(&new_request).unwrap(),
            expected_request
        );
        assert!(new_request.contains("18446744073709551616001"));
        assert!(new_request.contains("0.123456789012345678901234567890123456789"));
        let mut expected_result: Value = serde_json::from_str(&old_result).unwrap();
        expected_result["id"] = copy.id().into();
        assert_eq!(
            serde_json::from_str::<Value>(&new_result).unwrap(),
            expected_result
        );
        let request: ProjectEditIntent = serde_json::from_str(&new_request).unwrap();
        let replay = copied_store.apply_edit(&request).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.document.value(), &expected_result);
    }
    let retained: i64 = copied_db
        .query_row(
            "SELECT count(*) FROM operations WHERE project_id=?1",
            params![copy.id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(retained, 7);
    assert_eq!(copied_store.load(copy.id()).unwrap(), copy);
    assert_eq!(source.load(current.id()).unwrap(), current);
    assert_eq!(fs::read(&import_path).unwrap(), original_bytes);
    assert_eq!(fs::read(&archive).unwrap(), archive_before);
}

#[test]
fn archive_rejects_rehashed_note_receipts_that_contradict_the_typed_operation() {
    let f = Fixture::new();
    let mut source = f.store("source");
    let mut value = document().value().clone();
    value["bars"] = 1.into();
    let initial = source
        .create(&ProjectDocument::from_value(value).unwrap())
        .unwrap();
    let outside = NoteValues {
        start_tick: 5000,
        ..values()
    };
    let edited = source
        .apply_edit(&intent(
            &initial,
            "updated",
            "",
            EditOperation::UpdateNote {
                values: outside,
                timeline: KEEP,
            },
        ))
        .unwrap()
        .document;
    let added = source
        .apply_edit(&intent(
            &edited,
            "added",
            "new-note",
            EditOperation::AddNote {
                values: outside,
                timeline: KEEP,
            },
        ))
        .unwrap()
        .document;
    let restored = source
        .apply_edit(&intent(
            &added,
            "restored",
            "",
            EditOperation::RestoreNote { timeline: KEEP },
        ))
        .unwrap()
        .document;
    let deleted = source
        .apply_edit(&intent(
            &restored,
            "deleted",
            "new-note",
            EditOperation::DeleteNote,
        ))
        .unwrap()
        .document;
    let good_path = f.0.join("good.omniusong-native");
    source.export_archive(initial.id(), &good_path).unwrap();
    let good = fs::read(&good_path).unwrap();
    for fault in [
        "update-values",
        "add-values",
        "add-provenance",
        "delete-present",
        "restore-values",
        "restore-expression",
        "locked-target",
        "extend-coverage",
    ] {
        let archive_path = f.0.join(format!("{fault}.omniusong-native"));
        fs::write(&archive_path, &good).unwrap();
        let operation = match fault {
            "add-values" | "add-provenance" => "added",
            "delete-present" => "deleted",
            "restore-values" | "restore-expression" => "restored",
            _ => "updated",
        };
        let archive = Connection::open(&archive_path).unwrap();
        let request_fault = matches!(fault, "update-values" | "add-values" | "extend-coverage");
        let (json_column, hash_column) = if request_fault {
            ("request_json", "request_sha256")
        } else {
            ("result_json", "result_sha256")
        };
        let json: String = archive
            .query_row(
                &format!("SELECT {json_column} FROM operations WHERE operation_id=?1"),
                [operation],
                |r| r.get(0),
            )
            .unwrap();
        let rewritten = if request_fault {
            let mut request: ProjectEditIntent = serde_json::from_str(&json).unwrap();
            match &mut request.edit {
                EditOperation::UpdateNote { values, timeline }
                | EditOperation::AddNote { values, timeline } => {
                    if fault == "extend-coverage" {
                        *timeline = NoteTimelinePolicy::ExtendLoop {
                            allow_native_schema_upgrade: true,
                        };
                    } else {
                        values.pitch = 0;
                    }
                }
                _ => unreachable!(),
            }
            serde_json::to_string(&request).unwrap()
        } else {
            let mut result = ProjectDocument::from_json(json.as_bytes())
                .unwrap()
                .value()
                .clone();
            match fault {
                "delete-present" => result["tracks"][0]["notes"].as_array_mut().unwrap().push(json!({"id":"new-note","pitch":127,"startTick":5000,"durationTicks":963,"velocity16":0,"expression":[]})),
                "restore-values" => result["tracks"][0]["notes"][0]["pitch"] = 45.into(),
                "restore-expression" => result["tracks"][0]["notes"][0]["expression"] = Value::Null,
                "add-provenance" => result["tracks"][0]["notes"][1]["model"] = json!({"fabricated":"not in authored request"}),
                "locked-target" => result["tracks"][0]["locked"] = true.into(),
                _ => unreachable!(),
            }
            String::from_utf8(
                ProjectDocument::from_value(result)
                    .unwrap()
                    .to_json()
                    .unwrap(),
            )
            .unwrap()
        };
        let checksum = format!("{:x}", Sha256::digest(rewritten.as_bytes()));
        archive
            .execute(
                &format!(
                    "UPDATE operations SET {json_column}=?1,{hash_column}=?2 WHERE operation_id=?3"
                ),
                params![rewritten, checksum, operation],
            )
            .unwrap();
        drop(archive);
        let cause = match fault {
            "update-values" => "Archived update-note values differ",
            "add-values" | "add-provenance" => {
                "Archived add-note result is not the exact authored note"
            }
            "delete-present" => "Archived delete-note result still contains",
            "restore-values" | "restore-expression" => {
                "Archived restore-note fields/expression differ"
            }
            "locked-target" => "Archived note operation requires an unlocked note track",
            "extend-coverage" => "Archived extended loop does not cover",
            _ => unreachable!(),
        };
        let mut identity = f.store(&format!("identity-{fault}"));
        let failure = identity
            .restore_archive(&archive_path)
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains(cause),
            "Identity restore rejected {fault} for the wrong reason: {failure}"
        );
        assert!(identity.list().unwrap().is_empty());
        let mut copied = f.store(&format!("copy-{fault}"));
        let request = RestoreCopyIntent {
            operation_id: OperationId(format!("copy-{fault}")),
            expected_archive_sha256: format!(
                "{:x}",
                Sha256::digest(fs::read(&archive_path).unwrap())
            ),
        };
        let failure = copied
            .restore_archive_as_copy(&archive_path, &request)
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains(cause),
            "Copy restore rejected {fault} for the wrong reason: {failure}"
        );
        assert!(copied.list().unwrap().is_empty());
    }
    assert_eq!(source.load(initial.id()).unwrap(), deleted);
    assert_eq!(fs::read(&good_path).unwrap(), good);
}
