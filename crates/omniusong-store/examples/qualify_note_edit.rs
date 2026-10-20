//! Fresh, retained synthetic note-edit/OSAR evidence. Never opens devices or a live catalog.
use omniusong_core::{
    DocumentNoteAddress, NoteTimelinePolicy, NoteValues, ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
};
use omniusong_store::{NativeStore, RestoreCopyIntent};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

type Outcome<T> = Result<T, Box<dyn Error>>;
const TRACK: &str = "鍵盤\0synthetic-source";
const NOTE: &str = "\0expressive-original";
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_hash(path: &Path) -> Outcome<String> {
    let mut input = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn write_new(path: &Path, bytes: &[u8]) -> Outcome<()> {
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    Ok(())
}
fn fixture() -> Outcome<ProjectDocument> {
    let mut value = ProjectDocument::new("Synthetic source · no device or AI")?
        .value()
        .clone();
    value["tracks"][0]["id"] = json!(TRACK);
    value["tracks"][0]["notes"] = json!([{"id":NOTE,"pitch":60,"startTick":37,"durationTicks":960,"velocity16":65001,"expression":[{"type":"pitch","points":[{"tick":0,"value32":2147483648u32},{"tick":960,"value32":4294967295u32}],"vendor:lane":{"confidence":"opaque"}},{"type":"pressure","points":[{"tick":0,"value32":0},{"tick":480,"value32":4294967295u32}]}],"input":{"sourceId":"synthetic-midi2-source","sourceName":"Software-generated fixture; not a performance","protocol":"midi2","group":7,"channel":3,"receivedTimeMs":123.5,"releaseVelocity16":60001,"keyDurationTicks":960,"vendor:clock":{"hostTicks":"18446744073709551615","calibrated":false}},"vendor:model":{"role":"fixture; no inference","sourceIdentity":"not-remapped"}}]);
    value["tracks"][0]["vendor:opaque"] = serde_json::from_str(
        "{\"integer\":1844674407370955161601,\"decimal\":1.2300,\"negative\":-999999999999999999999999}",
    )?;
    value["inputCapture"] = json!({"version":1,"droppedEvents":0,"events":[{"trackId":TRACK,"tick":37,"sourceId":"synthetic-midi2-source","sourceName":"Software-generated fixture; not a performance","receivedTimeMs":123.5,"hostTime":"18446744073709551615","format":"ump","words":[1083194368u32,4294967295u32],"vendor:raw":"preserve entire packet"}]});
    value["vendor:qualification"] = json!({"synthetic":true,"capturedFromDevice":false,"humanPerformance":false,"inference":false});
    Ok(ProjectDocument::from_value(value)?)
}
fn address(note_id: &str) -> DocumentNoteAddress {
    DocumentNoteAddress {
        track_id: TRACK.into(),
        note_id: note_id.into(),
    }
}
fn intent(
    document: &ProjectDocument,
    id: &str,
    note_id: &str,
    edit: EditOperation,
) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        scope: if matches!(edit, EditOperation::Undo | EditOperation::Redo) {
            EditScope::Project
        } else {
            EditScope::DocumentNote {
                address: address(note_id),
            }
        },
        edit,
        extensions: Default::default(),
    }
}
fn apply(
    store: &mut NativeStore,
    document: &ProjectDocument,
    id: &str,
    note_id: &str,
    edit: EditOperation,
) -> Outcome<ProjectDocument> {
    let receipt = store.apply_edit(&intent(document, id, note_id, edit))?;
    assert!(!receipt.replayed);
    Ok(receipt.document)
}
fn keep_source(original: &ProjectDocument, current: &ProjectDocument) {
    assert_eq!(
        current.value()["inputCapture"],
        original.value()["inputCapture"]
    );
    assert_eq!(
        current.tracks()[0]["vendor:opaque"],
        original.tracks()[0]["vendor:opaque"]
    );
    assert_eq!(current.tracks()[1], original.tracks()[1]);
    assert_eq!(
        current.value()["vendor:qualification"],
        original.value()["vendor:qualification"]
    );
    assert_eq!(current.value()["history"], original.value()["history"]);
}
fn root_only(before: &ProjectDocument, after: &ProjectDocument) {
    assert_ne!(before.id(), after.id());
    let mut expected = before.value().clone();
    expected["id"] = json!(after.id());
    assert_eq!(&expected, after.value());
}
fn retain(root: &Path, name: &str, document: &ProjectDocument) -> Outcome<Value> {
    let bytes = document.to_json()?;
    let path = root.join(name);
    write_new(&path, &bytes)?;
    Ok(
        json!({"path":path,"projectId":document.id(),"revision":document.revision(),"schemaVersion":document.schema_version(),"bars":document.bars(),"byteLength":bytes.len(),"sha256":hash(&bytes)}),
    )
}

fn main() -> Outcome<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Usage: qualify_note_edit FRESH_DIRECTORY FRESH_RECEIPT_JSON".into());
    }
    let root = PathBuf::from(&args[0]);
    let receipt_path = PathBuf::from(&args[1]);
    match fs::symlink_metadata(&receipt_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => return Err("Receipt destination already exists; nothing overwritten".into()),
    }
    fs::create_dir(&root)?;
    let native = root.join("store");
    let mut store = NativeStore::open(&native)?;
    let original = store.create(&fixture()?)?;
    let original_json = retain(&root, "original-project.json", &original)?;
    let original_hash = original_json["sha256"].as_str().unwrap().to_owned();
    let source_note = original.tracks()[0]["notes"][0].clone();
    let source_archive = store.export_archive(
        original.id(),
        &root.join("original-source.omniusong-native"),
    )?;
    let copied = store.restore_archive_as_copy(
        &source_archive.path,
        &RestoreCopyIntent {
            operation_id: OperationId("note-qualifier-source-copy".into()),
            expected_archive_sha256: source_archive.sha256.clone(),
        },
    )?;
    root_only(&original, &copied.document);
    let target = copied.document;
    let initial_json = retain(&root, "copied-initial.json", &target)?;
    let values = NoteValues {
        pitch: 67,
        start_tick: 123,
        duration_ticks: 1920,
        velocity16: 0,
    };
    let first_intent = intent(
        &target,
        "note-qualifier-update-first",
        NOTE,
        EditOperation::UpdateNote {
            values,
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    );
    let first = store.apply_edit(&first_intent)?.document;
    assert_eq!(first.revision(), 1);
    keep_source(&original, &first);
    assert_eq!(first.tracks()[0]["notes"][0]["input"], source_note["input"]);
    assert_eq!(
        first.tracks()[0]["notes"][0]["vendor:model"],
        source_note["vendor:model"]
    );
    let snapshot = first.tracks()[0]["notes"][0]["editSource"].clone();
    assert_eq!(snapshot["pitch"], source_note["pitch"]);
    assert_eq!(snapshot["expression"], source_note["expression"]);
    let first_json = retain(&root, "edited-first.json", &first)?;
    let second = apply(
        &mut store,
        &first,
        "note-qualifier-update-second",
        NOTE,
        EditOperation::UpdateNote {
            values: NoteValues {
                pitch: 72,
                velocity16: u16::MAX,
                ..values
            },
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    )?;
    assert_eq!(second.tracks()[0]["notes"][0]["editSource"], snapshot);
    let second_json = retain(&root, "edited-second.json", &second)?;
    let deleted = apply(
        &mut store,
        &second,
        "note-qualifier-delete",
        NOTE,
        EditOperation::DeleteNote,
    )?;
    assert!(deleted.tracks()[0]["notes"].as_array().unwrap().is_empty());
    keep_source(&original, &deleted);
    let deleted_json = retain(&root, "deleted-current.json", &deleted)?;
    let undone = apply(
        &mut store,
        &deleted,
        "note-qualifier-undo-delete",
        NOTE,
        EditOperation::Undo,
    )?;
    assert_eq!(undone.tracks(), second.tracks());
    let redone_delete = apply(
        &mut store,
        &undone,
        "note-qualifier-redo-delete",
        NOTE,
        EditOperation::Redo,
    )?;
    assert_eq!(redone_delete.tracks(), deleted.tracks());
    let restored_delete = apply(
        &mut store,
        &redone_delete,
        "note-qualifier-undo-delete-again",
        NOTE,
        EditOperation::Undo,
    )?;
    let restored_note = apply(
        &mut store,
        &restored_delete,
        "note-qualifier-restore-original",
        NOTE,
        EditOperation::RestoreNote {
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    )?;
    let mut expected_note = source_note;
    expected_note["editSource"] = snapshot.clone();
    assert_eq!(restored_note.tracks()[0]["notes"][0], expected_note);
    let restored_note_json = retain(&root, "restored-original-values.json", &restored_note)?;
    let authored_id = uuid::Uuid::new_v4().to_string();
    let authored = NoteValues {
        pitch: 71,
        start_tick: 12000,
        duration_ticks: 960,
        velocity16: 40000,
    };
    let added = apply(
        &mut store,
        &restored_note,
        "note-qualifier-add-authored",
        &authored_id,
        EditOperation::AddNote {
            values: authored,
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    )?;
    assert!(added.tracks()[0]["notes"][1].get("input").is_none());
    assert_eq!(added.tracks()[0]["notes"][1]["expression"], json!([]));
    let long = NoteValues {
        start_tick: 128 * 3840,
        ..authored
    };
    let refused = intent(
        &added,
        "note-qualifier-refuse-implicit-promotion",
        &authored_id,
        EditOperation::UpdateNote {
            values: long,
            timeline: NoteTimelinePolicy::ExtendLoop {
                allow_native_schema_upgrade: false,
            },
        },
    );
    assert!(store.apply_edit(&refused).is_err());
    assert_eq!(store.load(target.id())?, added);
    let extended = apply(
        &mut store,
        &added,
        "note-qualifier-explicit-promotion",
        &authored_id,
        EditOperation::UpdateNote {
            values: long,
            timeline: NoteTimelinePolicy::ExtendLoop {
                allow_native_schema_upgrade: true,
            },
        },
    )?;
    assert_eq!(extended.schema_version(), 2);
    assert_eq!(extended.bars(), 129);
    assert_eq!(
        extended.tracks()[0]["notes"][1]["editSource"]["expression"],
        json!([])
    );
    let undo_extent = apply(
        &mut store,
        &extended,
        "note-qualifier-undo-promotion",
        NOTE,
        EditOperation::Undo,
    )?;
    assert_eq!(undo_extent.schema_version(), 1);
    assert_eq!(undo_extent.bars(), 4);
    assert_eq!(undo_extent.tracks(), added.tracks());
    let final_document = apply(
        &mut store,
        &undo_extent,
        "note-qualifier-redo-promotion",
        NOTE,
        EditOperation::Redo,
    )?;
    assert_eq!(final_document.tracks(), extended.tracks());
    assert_eq!(final_document.schema_version(), 2);
    assert_eq!(final_document.bars(), 129);
    keep_source(&original, &final_document);
    let final_json = retain(&root, "final-project.json", &final_document)?;
    assert_eq!(store.load(original.id())?, original);
    drop(store);
    let reopened = NativeStore::open_existing(&native)?;
    assert_eq!(reopened.load(target.id())?, final_document);
    assert_eq!(reopened.load(original.id())?, original);
    let final_archive =
        reopened.export_archive(target.id(), &root.join("edited-complete.omniusong-native"))?;
    let archive_sha = file_hash(&final_archive.path)?;
    assert_eq!(archive_sha, final_archive.sha256);
    let mut restored = NativeStore::open(root.join("restored-store"))?;
    let inspection = restored.inspect_archive(&final_archive.path)?;
    assert_eq!(inspection.sha256, archive_sha);
    let restored_document = restored.restore_archive(&final_archive.path)?;
    assert_eq!(restored_document, final_document);
    let replay = restored.apply_edit(&first_intent)?;
    assert!(replay.replayed);
    assert_eq!(replay.document, first);
    assert_eq!(restored.load(target.id())?, final_document);
    let second_copy = restored.restore_archive_as_copy(
        &final_archive.path,
        &RestoreCopyIntent {
            operation_id: OperationId("note-qualifier-final-copy".into()),
            expected_archive_sha256: archive_sha.clone(),
        },
    )?;
    assert!(!second_copy.replayed);
    root_only(&final_document, &second_copy.document);
    assert_eq!(restored.load(target.id())?, final_document);
    keep_source(&original, &second_copy.document);
    let copied_json = retain(&root, "restored-copy-project.json", &second_copy.document)?;
    let copied_archive = restored.export_archive(
        second_copy.document.id(),
        &root.join("copied-complete.omniusong-native"),
    )?;
    let mut second_restore = NativeStore::open(root.join("copy-reopened-store"))?;
    assert_eq!(
        second_restore.restore_archive(&copied_archive.path)?,
        second_copy.document
    );
    assert_eq!(
        file_hash(&root.join("original-project.json"))?,
        original_hash
    );
    assert_eq!(file_hash(&source_archive.path)?, source_archive.sha256);
    assert_eq!(file_hash(&final_archive.path)?, archive_sha);
    let receipt = json!({"format":"omniusong.native-note-edit-qualification","version":1,"syntheticOnly":true,"physicalInputOpened":false,"audioDeviceOpened":false,"guiOperated":false,"modelInference":false,"fixtureRoot":root,"nativeStore":native,"originalProjectId":original.id(),"editedProjectId":target.id(),"restoredCopyProjectId":second_copy.document.id(),"authoredNoteId":authored_id,"exactDocumentTrackKey":TRACK,"exactDocumentNoteKey":NOTE,"projects":{"original":original_json,"initialCopy":initial_json,"firstEdit":first_json,"secondEdit":second_json,"deleted":deleted_json,"restoredOriginalValues":restored_note_json,"final":final_json,"restoredCopy":copied_json},"sourceArchive":{"path":source_archive.path,"sha256":source_archive.sha256,"byteLength":source_archive.byte_length},"editedArchive":{"path":final_archive.path,"sha256":archive_sha,"byteLength":final_archive.byte_length,"historyRows":final_archive.history_rows,"operationRows":final_archive.operation_rows},"copiedArchive":{"path":copied_archive.path,"sha256":copied_archive.sha256,"byteLength":copied_archive.byte_length},"retainedInputCaptureSha256":hash(&serde_json::to_vec(&original.value()["inputCapture"])?),"immutableEditSourceSha256":hash(&serde_json::to_vec(&snapshot)?),"opaqueMetadataSha256":hash(&serde_json::to_vec(&original.tracks()[0]["vendor:opaque"])?),"checks":{"exactFourFieldEditsAndVelocities":true,"relativeExpressionAndInputIdentityUnchanged":true,"rawUmpAndHostClockExact":true,"unknownNumbersExact":true,"immutableFirstSnapshotRetainedOnRestore":true,"deleteUndoRedoExact":true,"noInlineHistoryFabrication":true,"noInventedInputForAuthoredNote":true,"explicitSchemaPromotionUndoRedo":true,"refusedImplicitPromotionDidNotPublish":true,"originalProjectUnchanged":true,"reopenExact":true,"identityRestoreExact":true,"archivedOldOperationReplayReturnsOriginalSnapshot":true,"replayDoesNotOverwriteLatest":true,"copyChangesOnlyRootProjectIdentity":true,"copyOfCopyArchiveReopenExact":true,"sourceFilesUnchanged":true},"limits":["Synthetic library/storage/archive fixture only. Not a GUI note editor or dirty/navigation/focus qualification.","Raw UMP is fixture data, not physical controller delivery. No microphone, audible playback, MIDI expression delivery, inference, latency, power-loss or Windows acceptance.","The CLI retains fresh fixture files and receipts. It never reads or edits the live native catalog."]});
    write_new(&receipt_path, &serde_json::to_vec_pretty(&receipt)?)?;
    println!(
        "Qualified synthetic note edits and complete archives: {}",
        receipt_path.display()
    );
    Ok(())
}
