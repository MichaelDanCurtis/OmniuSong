//! One bounded operator recovery for the retained October 20 development incident.
//! This does not repair an original, infer lost operations or implement automatic recovery.
use omniusong_core::ProjectDocument;
use omniusong_store::NativeStore;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs,
    io::{Read, Write},
    path::Path,
};

type Outcome<T> = Result<T, Box<dyn Error>>;
const RAW_SHA: &str = "3cec762a9455ed3b53525529e0ddf8e8d631e4a92931f4c445bb9cdc00b234a4";
const ARCHIVE_SHA: &str = "00c6ab268772bddd03c198694799c02d8754c4909762f79c41c31b51903e627e";
const SNAPSHOT_SHA: &str = "04aceba28b170eda514355b77e92d12a9cf19072e6a90e00377af50a4717fcc3";
const LOST_PROJECT: &str = "670fcc05-94ad-47ea-bb67-41d98bf3e44d";
const NOTICE: &str = "omniusongOperatorRecovery";
const MISSING_OPERATIONS: [&str; 3] = [
    "33436211-fdbc-4076-9c31-c841d7739c89",
    "f6d04291-049e-4192-99d6-97e8edb18db8",
    "2d94be93-13f8-4962-b581-8c06dfd2871f",
];
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_small(path: &Path) -> Outcome<Vec<u8>> {
    let size = fs::metadata(path)?.len();
    if size > 2 * 1024 * 1024 {
        return Err("Incident input exceeds the 2 MiB operator bound".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Incident input grew beyond its bound".into());
    }
    Ok(bytes)
}
fn readonly(path: &Path) -> Outcome<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}
fn text_rows(connection: &Connection, sql: &str, columns: usize) -> Outcome<Vec<Vec<String>>> {
    let mut statement = connection.prepare(sql)?;
    Ok(statement
        .query_map([LOST_PROJECT], |row| {
            (0..columns)
                .map(|column| row.get(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
fn count(connection: &Connection, table: &str) -> Outcome<i64> {
    Ok(connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
}
fn main() -> Outcome<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 5 {
        return Err("Usage: recover_disconnected_preview RAW_FORENSIC_DB SALVAGED_STORE COMPLETED_ARCHIVE NEW_RECOVERY_ROOT NEW_RECEIPT_JSON".into());
    }
    let raw_path = Path::new(&args[0]);
    let salvaged_root = Path::new(&args[1]);
    let archive_path = Path::new(&args[2]);
    let root = Path::new(&args[3]);
    let receipt_path = Path::new(&args[4]);
    let raw = read_small(raw_path)?;
    let archive_bytes = read_small(archive_path)?;
    if hash(&raw) != RAW_SHA || hash(&archive_bytes) != ARCHIVE_SHA {
        return Err("Inputs do not match this preserved incident; nothing was published".into());
    }
    let salvaged_before = hash(&read_small(&salvaged_root.join("studio.sqlite3"))?);
    let db = readonly(&salvaged_root.join("studio.sqlite3"))?;
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    assert_eq!(integrity, "ok");
    assert_eq!(count(&db, "projects")?, 4);
    assert_eq!(count(&db, "edits")?, 7);
    assert_eq!(count(&db, "operations")?, 2);
    assert_eq!(count(&db, "native_takes")?, 1);
    let source_json: String = db.query_row(
        "SELECT payload FROM projects WHERE id=?1",
        [LOST_PROJECT],
        |r| r.get(0),
    )?;
    assert_eq!(hash(source_json.as_bytes()), SNAPSHOT_SHA);
    let source = ProjectDocument::from_json(source_json.as_bytes())?;
    assert_eq!(source.revision(), 8);
    assert_eq!(source.name(), "Native archive restored copy");
    assert_eq!(source.tempo(), 119.25);
    assert!(source.referenced_assets()?.is_empty());
    let history = text_rows(
        &db,
        "SELECT CAST(sequence AS TEXT),before_json,after_json FROM edits WHERE project_id=?1 ORDER BY sequence",
        3,
    )?;
    let operations = text_rows(
        &db,
        "SELECT operation_id,request_json,request_sha256,CAST(previous_revision AS TEXT),CAST(committed_revision AS TEXT),result_json,result_sha256 FROM operations WHERE project_id=?1 ORDER BY operation_id",
        7,
    )?;
    let proof = text_rows(
        &db,
        "SELECT proof_sha256,receipt_json FROM archive_copy_proofs WHERE proof_sha256 IN (SELECT proof_sha256 FROM archive_project_lineage WHERE project_id=?1) ORDER BY proof_sha256",
        2,
    )?;
    let mapping = text_rows(
        &db,
        "SELECT proof_sha256,source_operation_id,destination_operation_id,source_request_sha256,source_result_sha256,destination_request_sha256,destination_result_sha256 FROM archive_copy_proof_operations WHERE proof_sha256 IN (SELECT proof_sha256 FROM archive_project_lineage WHERE project_id=?1) ORDER BY source_operation_id",
        7,
    )?;
    let admission = text_rows(
        &db,
        "SELECT operation_id,project_id,proof_sha256 FROM archive_copies WHERE project_id=?1",
        3,
    )?;
    assert_eq!(
        (
            history.len(),
            operations.len(),
            proof.len(),
            mapping.len(),
            admission.len()
        ),
        (3, 1, 1, 1, 1)
    );
    drop(db);
    fs::create_dir(root)?; // Never overwrite or reuse an earlier candidate.
    let mut original_backup = NativeStore::open(root.join("validated-backup-store"))?;
    let backup_doc = original_backup.restore_archive(archive_path)?;
    assert_eq!(backup_doc.id(), "eb9b8562-4e5d-47c6-adae-e9194fcab668");
    drop(original_backup);
    let salvage = NativeStore::open(salvaged_root)?;
    // Missing history must cause the ordinary archive validator to reject this project.
    let failure = salvage.export_archive(
        source.id(),
        &root.join("must-not-publish-incomplete.omniusong-native"),
    );
    assert!(failure.is_err());
    assert!(
        !root
            .join("must-not-publish-incomplete.omniusong-native")
            .exists()
    );
    let incomplete_error = failure.unwrap_err().to_string();
    let mut target = NativeStore::open(root.join("recovered-store"))?;
    let mut healthy = vec![];
    for summary in salvage.list()? {
        if summary.id == LOST_PROJECT {
            continue;
        }
        let document = salvage.load(&summary.id)?;
        let exported = salvage.export_archive(
            &summary.id,
            &root.join(format!("complete-{}.omniusong-native", summary.id)),
        )?;
        let restored = target.restore_archive(&exported.path)?;
        assert_eq!(restored, document);
        assert_eq!(
            target.imported_original(document.id())?,
            salvage.imported_original(document.id())?
        );
        healthy.push(json!({"projectId":document.id(),"revision":document.revision(),"documentSha256":hash(&document.to_json()?),"archiveSha256":exported.sha256,"historyRows":exported.history_rows,"operationRows":exported.operation_rows,"exactDocumentAndCompleteRetainedClosure":true}));
    }
    assert_eq!(healthy.len(), 3);
    let references = source.referenced_takes()?;
    for reference in &references {
        target.ingest_take(&salvage.open_take(reference)?)?;
    }
    let mut fork = source.imported_copy(SNAPSHOT_SHA)?.value().clone();
    let previous = fork.get(NOTICE).cloned();
    let archive_hex: String = archive_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    fork[NOTICE] = json!({
        "format":"omniusong.operator-recovery-evidence","version":1,
        "mode":"fresh_snapshot_from_partial_salvage","automaticRecovery":false,
        "historicalLedgerComplete":false,"reconstructedOperationCount":0,
        "rawDatabaseSha256":RAW_SHA,"rawPhysicalPages":41,"rawAdvertisedPages":45,
        "sourceProjectId":source.id(),"sourceRevision":source.revision(),
        "sourceDocumentJson":source_json,"sourceDocumentSha256":SNAPSHOT_SHA,
        "missingHistorySequences":[4],"missingOperationIds":MISSING_OPERATIONS,
        "survivingHistoryRows":history,"survivingOperationRows":operations,
        "survivingCopyProofRows":proof,"survivingCopyMappingRows":mapping,"survivingCopyAdmissionRows":admission,
        "completedArchive":{"formatVersion":2,"sha256":ARCHIVE_SHA,"byteLength":archive_bytes.len(),"bytesHex":archive_hex},
        "toolSourceSha256":hash(include_bytes!("recover_disconnected_preview.rs")),
        "previousValue":previous,
        "notice":"Recovered current musical content in a new session. One old history row and three old edit receipts are unavailable. Surviving history and copy proof are evidence only; no old copy/operation identity is admitted."
    });
    fork["name"] = "Recovered · Native archive restored copy".into();
    let fork = ProjectDocument::from_value(fork)?;
    for (key, value) in source.value().as_object().unwrap() {
        if !["id", "revision", "name", "nativeImport", NOTICE].contains(&key.as_str()) {
            assert_eq!(
                &fork.value()[key],
                value,
                "Recovery changed musical/opaque field {key}"
            );
        }
    }
    let fork = target.create(&fork)?;
    assert_eq!(fork.revision(), 0);
    assert_ne!(fork.id(), source.id());
    let fresh_export =
        target.export_archive(fork.id(), &root.join("recovered-snapshot.omniusong-native"))?;
    let mut roundtrip = NativeStore::open(root.join("roundtrip-store"))?;
    let restored = roundtrip.restore_archive(&fresh_export.path)?;
    assert_eq!(restored, fork);
    assert_eq!(
        restored.value()[NOTICE]["sourceDocumentJson"]
            .as_str()
            .unwrap(),
        source_json
    );
    for reference in &references {
        assert_eq!(
            target.open_take(reference)?.manifest(),
            roundtrip.open_take(reference)?.manifest()
        );
    }
    drop(roundtrip);
    let target_root = root.join("recovered-store");
    drop(target);
    let reopened = NativeStore::open(&target_root)?;
    assert_eq!(reopened.list()?.len(), 4);
    assert_eq!(reopened.load(fork.id())?, fork);
    drop(reopened);
    let db = readonly(&target_root.join("studio.sqlite3"))?;
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))?,
        "ok"
    );
    for (table, expected) in [
        ("projects", 4),
        ("edits", 4),
        ("operations", 1),
        ("imports", 1),
        ("native_takes", 1),
        ("archive_copy_proofs", 0),
        ("archive_copy_proof_operations", 0),
        ("archive_project_lineage", 0),
        ("archive_copies", 0),
    ] {
        assert_eq!(
            count(&db, table)?,
            expected,
            "Unexpected recovered {table} count"
        );
    }
    for operation in MISSING_OPERATIONS {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM operations WHERE operation_id=?1",
                params![operation],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
    }
    drop(db);
    drop(salvage);
    assert_eq!(hash(&read_small(raw_path)?), RAW_SHA);
    assert_eq!(hash(&read_small(archive_path)?), ARCHIVE_SHA);
    assert_eq!(
        hash(&read_small(&salvaged_root.join("studio.sqlite3"))?),
        salvaged_before
    );
    let receipt = json!({
        "format":"omniusong.disconnect-recovery-qualification","version":1,
        "incidentDate":"2026-10-20","scope":"Bounded operator recovery of this development incident; not automatic production repair or a general recovery format.",
        "recoveryRoot":root,"recoveredStore":target_root,
        "rawDatabase":{"path":raw_path,"sha256":RAW_SHA,"physicalPages":41,"advertisedPages":45,"unchanged":true},
        "salvagedDatabase":{"path":salvaged_root.join("studio.sqlite3"),"sha256":salvaged_before,"unchanged":true},
        "completedArchive":{"path":archive_path,"sha256":ARCHIVE_SHA,"unchanged":true},
        "healthyProjects":healthy,"snapshotSourceProjectId":source.id(),"snapshotSourceRevision":8,
        "snapshotSourceDocumentSha256":SNAPSHOT_SHA,"newRecoveryProjectId":fork.id(),"newRecoveryRevision":0,
        "newRecoveryDocumentSha256":hash(&fork.to_json()?),"newRecoveryName":fork.name(),
        "newRecoveryMetadataBytes":fork.to_json()?.len(),"sourceMusicAndUnknownFieldsRetained":true,
        "embeddedSourceJsonExact":true,"embeddedCompletedArchiveExact":true,
        "missingHistorySequences":[4],"missingOperationIds":MISSING_OPERATIONS,
        "reconstructedOperations":0,"incompleteOriginalArchiveRejected":incomplete_error,
        "freshForkArchive":{"path":fresh_export.path,"sha256":fresh_export.sha256,"takeCount":fresh_export.take_count},
        "archiveRestoreAndReopenExact":true,"oldCopyAdmissionNotPublished":true,
        "originalDefaultStoreNotReplaced":true,
        "limits":["Three historical requests/results and one history row are missing; this does not restore them.","The recovered snapshot is a fresh project with explicit operator provenance, not continuation of the lost historical ledger.","A passing SQLite integrity check alone does not establish complete semantic recovery; retained rows and archives were separately checked.","No actual microphone/audio playback, MIDI protocol, inference, latency, installer or Windows qualification is claimed."]
    });
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(receipt_path)?;
    output.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    println!(
        "Recovered three complete projects and one explicit current-snapshot fork. Receipt {}",
        receipt_path.display()
    );
    Ok(())
}
