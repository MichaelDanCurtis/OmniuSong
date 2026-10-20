//! Synthetic OSAR-v1 -> copied session -> OSAR-v2 -> copied session qualification.
//! Uses an existing archive read-only; creates a fresh destination every run.
use omniusong_core::{
    ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
};
use omniusong_recording::{SealedJournal, TakeId};
use omniusong_store::{NativeStore, RestoreCopyIntent};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, error::Error, fs, io::Read, path::Path};

type Outcome<T> = Result<T, Box<dyn Error>>;

fn file_hash(path: &Path) -> Outcome<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn readonly(path: &Path) -> Outcome<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}
fn strings(db: &Connection, query: &str, id: &str) -> Outcome<Vec<String>> {
    Ok(db
        .prepare(query)?
        .query_map([id], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}
fn root_only(before: &ProjectDocument, after: &ProjectDocument) {
    assert_ne!(before.id(), after.id());
    let mut expected = before.value().clone();
    expected["id"] = json!(after.id());
    assert_eq!(&expected, after.value());
}
fn edit(document: &ProjectDocument, operation: &str, kind: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(operation.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        scope: EditScope::Project,
        edit: kind,
        extensions: Default::default(),
    }
}
fn payload_hash(snapshot: &SealedJournal) -> Outcome<(u64, String)> {
    let mut hash = Sha256::new();
    let mut length = 0;
    snapshot.visit_chunks(|_, chunk| {
        hash.update(&chunk.payload);
        length += chunk.payload.len() as u64;
        Ok(())
    })?;
    Ok((length, format!("{:x}", hash.finalize())))
}

fn main() -> Outcome<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("Usage: qualify_archive_copy V1_ARCHIVE NEW_FIXTURE_ROOT RECEIPT_JSON".into());
    }
    let archive = Path::new(&args[0]);
    let root = Path::new(&args[1]);
    let receipt_path = Path::new(&args[2]);
    fs::create_dir(root)?;
    let original_sha = file_hash(archive)?;
    let source = readonly(archive)?;
    let version: u32 = source.pragma_query_value(None, "user_version", |r| r.get(0))?;
    assert_eq!(
        version, 1,
        "Qualification requires an authentic earlier-format archive"
    );
    let (take_id, manifest_sha, object_sha): (String, String, String) = source.query_row(
        "SELECT take_id,manifest_sha256,object_sha256 FROM native_takes",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let source_operations: BTreeSet<String> = strings(
        &source,
        "SELECT operation_id FROM operations WHERE project_id=?1 ORDER BY operation_id",
        &source.query_row("SELECT id FROM projects", [], |r| r.get::<_, String>(0))?,
    )?
    .into_iter()
    .collect();
    drop(source);
    let first_root = root.join("first-store");
    let mut first = NativeStore::open(&first_root)?;
    let inspection = first.inspect_archive(archive)?;
    assert_eq!(inspection.sha256, original_sha);
    let original = first.restore_archive(archive)?;
    let intent = RestoreCopyIntent {
        operation_id: OperationId("qualification-copy-first".into()),
        expected_archive_sha256: original_sha.clone(),
    };
    let copy = first.restore_archive_as_copy(archive, &intent)?;
    assert!(!copy.replayed);
    root_only(&original, &copy.document);
    let initial = copy.document;
    let changed = first
        .apply_edit(&edit(
            &initial,
            "qualification-save-pair",
            EditOperation::UpdateSession {
                name: "Copied recording history".into(),
                tempo: 117.25,
            },
        ))?
        .document;
    let undone = first
        .apply_edit(&edit(&changed, "qualification-undo", EditOperation::Undo))?
        .document;
    assert_eq!(undone.name(), initial.name());
    assert_eq!(undone.tempo(), initial.tempo());
    let redone = first
        .apply_edit(&edit(&undone, "qualification-redo", EditOperation::Redo))?
        .document;
    assert_eq!(redone.name(), changed.name());
    assert_eq!(redone.tempo(), changed.tempo());
    let repeated = first.restore_archive_as_copy(&root.join("unavailable-source"), &intent)?;
    assert!(repeated.replayed);
    assert_eq!(repeated.document, initial);
    assert_eq!(first.load(initial.id())?, redone);
    assert_eq!(first.load(original.id())?, original);
    let object = first.registered_take(&TakeId(take_id.clone()))?;
    assert_eq!(object.object_sha256, object_sha);
    assert_eq!(object.manifest.sha256, manifest_sha);
    let source_ref = omniusong_core::native::TakeReference {
        take_id: TakeId(take_id.clone()),
        manifest_sha256: manifest_sha.clone(),
        stream_id: "audio".into(),
    };
    let first_payload = payload_hash(&first.open_take(&source_ref)?)?;
    let v2 = first.export_archive(initial.id(), &root.join("copied.omniusong-native"))?;
    assert_eq!(v2.take_count, 1);
    assert!(
        redone.referenced_takes()?.is_empty(),
        "Originals must be retained from historical receipts"
    );
    let db = readonly(&first_root.join("studio.sqlite3"))?;
    let first_ids: BTreeSet<String> = strings(
        &db,
        "SELECT operation_id FROM operations WHERE project_id=?1 ORDER BY operation_id",
        initial.id(),
    )?
    .into_iter()
    .collect();
    assert!(first_ids.is_disjoint(&source_operations));
    let first_proofs = strings(
        &db,
        "SELECT receipt_json FROM archive_copy_proofs WHERE proof_sha256 IN (SELECT proof_sha256 FROM archive_project_lineage WHERE project_id=?1) ORDER BY receipt_json",
        initial.id(),
    )?;
    assert_eq!(first_proofs.len(), 1);
    drop(db);
    drop(first);

    let second_root = root.join("second-store");
    let mut second = NativeStore::open(&second_root)?;
    let v2_db = readonly(&v2.path)?;
    let v2_version: u32 = v2_db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    assert_eq!(v2_version, 2);
    drop(v2_db);
    let restored = second.restore_archive(&v2.path)?;
    assert_eq!(restored, redone);
    let old_retry = second.restore_archive_as_copy(&root.join("missing-v1"), &intent)?;
    assert!(old_retry.replayed);
    assert_eq!(old_retry.document, initial);
    assert_eq!(second.load(initial.id())?, redone);
    let copied_again = second.restore_archive_as_copy(
        &v2.path,
        &RestoreCopyIntent {
            operation_id: OperationId("qualification-copy-second".into()),
            expected_archive_sha256: v2.sha256.clone(),
        },
    )?;
    root_only(&redone, &copied_again.document);
    assert_eq!(second.load(initial.id())?, redone);
    let second_payload = payload_hash(&second.open_take(&source_ref)?)?;
    assert_eq!(first_payload, second_payload);
    assert_eq!(
        second
            .registered_take(&TakeId(take_id.clone()))?
            .object_sha256,
        object_sha
    );
    let db = readonly(&second_root.join("studio.sqlite3"))?;
    let second_ids: BTreeSet<String> = strings(
        &db,
        "SELECT operation_id FROM operations WHERE project_id=?1 ORDER BY operation_id",
        copied_again.document.id(),
    )?
    .into_iter()
    .collect();
    assert!(second_ids.is_disjoint(&source_operations));
    assert!(second_ids.is_disjoint(&first_ids));
    let second_proofs = strings(
        &db,
        "SELECT receipt_json FROM archive_copy_proofs WHERE proof_sha256 IN (SELECT proof_sha256 FROM archive_project_lineage WHERE project_id=?1) ORDER BY receipt_json",
        copied_again.document.id(),
    )?;
    assert_eq!(second_proofs.len(), 2);
    assert!(
        first_proofs
            .iter()
            .all(|proof| second_proofs.contains(proof))
    );
    let store_version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    assert_eq!(store_version, 4);
    drop(db);
    drop(second);
    assert_eq!(file_hash(archive)?, original_sha);
    let receipt = json!({
        "format":"omniusong.native-archive-copy-qualification", "version":1,
        "evidenceDate":"2026-10-20", "fixtureRoot":root,
        "synthetic":true, "capturedFromDevice":false,
        "input":{"path":archive,"sha256":original_sha,"archiveVersion":version},
        "output":{"path":v2.path,"sha256":v2.sha256,"byteLength":v2.byte_length,"archiveVersion":v2_version,"takeCount":v2.take_count},
        "storeVersion":store_version,"originalProjectId":original.id(),"copiedProjectId":initial.id(),
        "secondCopyProjectId":copied_again.document.id(),
        "takeId":take_id,"objectSha256":object_sha,"manifestSha256":manifest_sha,
        "payloadBytes":first_payload.0,"payloadSha256":first_payload.1,
        "originalFileUnchanged":true,"originalSessionUnchanged":true,
        "typedRootOnlyDocumentRewrite":true,"freshLiveOperationIdentities":true,
        "firstLineageProofCount":first_proofs.len(),"secondLineageProofCount":second_proofs.len(),
        "ancestorProofBytesUnchanged":true,"atomicNameTempoUndoRedo":true,
        "replayWithoutSourceReturnsOriginalSnapshotAndPreservesLaterEdits":true,
        "historicalOperationOnlyOriginalsRetained":true,
        "limits":["Synthetic storage/archive fixture; no actual recording, latency, audible playback, listening, model inference or Windows qualification.","Native UI archive qualification has a separate observed-binary receipt."]
    });
    fs::write(receipt_path, serde_json::to_vec_pretty(&receipt)?)?;
    println!(
        "Qualified OSARv1/v2 copies, complete lineage and exact historical originals: {}",
        receipt_path.display()
    );
    Ok(())
}
