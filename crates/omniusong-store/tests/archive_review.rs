//! Independent complete-closure archive and atomic restore regressions.
use omniusong_core::{
    ProjectDocument,
    contracts::{
        EditOperation, EditScope, Extensions, OperationId, ProjectEditIntent, ProjectId, TrackId,
    },
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, TakePublication};
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/archive-review")
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
    fn archive(&self) -> PathBuf {
        self.0.join("complete.omniusong")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Archive review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn edit(project: &ProjectDocument, id: &str) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        scope: EditScope::Project,
        edit: EditOperation::Rename {
            name: "Later branch".into(),
        },
        extensions: Extensions::new(),
    }
}
fn row_count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}
fn assert_no_published_rows(fixture: &Fixture, target: &str) {
    let db = fixture.db(target);
    for table in [
        "projects",
        "edits",
        "operations",
        "imports",
        "assets",
        "native_takes",
    ] {
        assert_eq!(row_count(&db, table), 0, "published partial {table}");
    }
}

fn recovered_source(fixture: &Fixture) -> SealedJournal {
    let take=TakeDescriptor{version:JOURNAL_VERSION,take_id:TakeId("archive-original".into()),streams:vec![StreamDescriptor{
        stream_id:"audio".into(),segment_id:"segment".into(),source_id:"source".into(),source_name:"Synthetic archive original".into(),source_connection_generation:1,
        host_clock:HostClock{domain:ClockDomainId("host".into()),epoch:ClockEpochId("boot".into()),ticks_per_second_numerator:24_000_000,ticks_per_second_denominator:1},
        kind:StreamKind::Audio{format:PcmFormat{sample_rate:48_000,channels:2,encoding:PcmEncoding::S16Le,layout:PcmLayout::Interleaved},audio_epoch:ClockEpochId("audio".into())},initial_source_position:0}],
        provenance:BTreeMap::from([("vendor".into(),serde_json::from_str(r#"{"integer":123456789012345678901234567890123456789,"decimal":0.12345678901234567890123456789}"#).unwrap())])};
    let path = fixture.0.join("source.sqlite3");
    let mut journal = Journal::create(&path, &take).unwrap();
    for sequence in 0..2 {
        let stamp = HostTimestamp {
            domain: ClockDomainId("host".into()),
            epoch: ClockEpochId("boot".into()),
            ticks: u64::MAX,
        };
        journal
            .append(
                &ChunkHeader {
                    version: CHUNK_VERSION,
                    take_id: take.take_id.clone(),
                    stream_id: "audio".into(),
                    segment_id: "segment".into(),
                    sequence,
                    range: SourceRange::AudioFrames {
                        start: sequence,
                        count: 1,
                    },
                    interpretation: take.streams[0].kind.clone(),
                    observed_start: stamp.clone(),
                    observed_end: stamp,
                    clock_anchors: vec![],
                    kind: ChunkKind::Content,
                },
                &[1, 2, 3, 4],
            )
            .unwrap();
    }
    drop(journal);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'09080706' WHERE sequence=1;").unwrap();
    drop(db);
    let mut journal = Journal::open(&path).unwrap();
    journal.seal_recovered("recover").unwrap();
    drop(journal);
    SealedJournal::open(path).unwrap()
}
fn native_branch(fixture: &Fixture) -> (ProjectDocument, TakePublication, SealedManifest) {
    let source = recovered_source(fixture);
    let manifest = source.manifest().clone();
    let mut store = fixture.store("source-store");
    let original = store
        .create(&ProjectDocument::new("Original").unwrap())
        .unwrap();
    store.ingest_take(&source).unwrap();
    let intent = TakePublication {
        operation_id: OperationId("retained-take-operation".into()),
        project_id: ProjectId(original.id().into()),
        expected_revision: 0,
        track_id: TrackId("recording".into()),
        track_name: "Original recording".into(),
        clip: TakeClip {
            edit_source: None,
            id: "retained-clip".into(),
            source: TakeReference {
                take_id: source.descriptor().take_id.clone(),
                manifest_sha256: manifest.sha256.clone(),
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames { start: 0, count: 1 },
            position: TimelinePosition {
                frame: -24_000,
                sample_rate: 48_000,
            },
            fades: None,
        },
    };
    store.publish_take(&intent).unwrap();
    let undone = store.undo(original.id(), 1).unwrap();
    let branch = store
        .apply_edit(&edit(&undone, "retained-branch-operation"))
        .unwrap()
        .document;
    assert!(branch.referenced_takes().unwrap().is_empty());
    store
        .export_archive(original.id(), &fixture.archive())
        .unwrap();
    (branch, intent, manifest)
}

#[test]
fn opaque_kind_version_and_intent_edit_extensions_round_trip_without_wrapper_reinterpretation() {
    let fixture = Fixture::new();
    let mut store = fixture.store("source-store");
    let original = store
        .create(&ProjectDocument::new("Opaque request").unwrap())
        .unwrap();
    let mut intent = edit(&original, "opaque-operation");
    intent
        .extensions
        .insert("kind".into(), json!("take_publication"));
    intent.extensions.insert("version".into(), json!(73));
    intent.extensions.insert(
        "intent".into(),
        serde_json::from_str(
            r#"{"integer":1234567890123456789012345678901234567890,"takeId":"foreign-opaque"}"#,
        )
        .unwrap(),
    );
    let committed = store.apply_edit(&intent).unwrap().document;
    let receipt = store
        .export_archive(original.id(), &fixture.archive())
        .unwrap();
    assert_eq!(receipt.operation_rows, 1);
    assert_eq!(
        receipt.sha256,
        digest(&fs::read(fixture.archive()).unwrap())
    );
    let mut restored = fixture.store("restored");
    assert_eq!(
        restored.restore_archive(&fixture.archive()).unwrap(),
        committed
    );
    let replay = restored.apply_edit(&intent).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, committed);
    let request: String = fixture
        .db("restored")
        .query_row("SELECT request_json FROM operations", [], |row| row.get(0))
        .unwrap();
    assert!(request.contains("1234567890123456789012345678901234567890"));
    assert!(request.contains("take_publication"));
}

#[test]
fn source_import_bytes_assets_and_both_history_sides_survive_archive_restore() {
    let fixture = Fixture::new();
    let bytes = b"immutable synthetic audio bytes";
    let asset = format!("{}.wav", digest(bytes));
    let legacy = fixture.0.join("legacy");
    let assets = fixture.0.join("legacy-assets");
    fs::create_dir_all(&assets).unwrap();
    fs::write(assets.join(&asset), bytes).unwrap();
    let mut value = ProjectDocument::new("Imported original")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["assetId"] = json!(asset);
    value["vendor"] = json!({"takeId":"opaque"});
    let mut raw = b" \n".to_vec();
    raw.extend(serde_json::to_vec_pretty(&value).unwrap());
    raw.extend_from_slice(b"\n \t");
    fs::write(&legacy, &raw).unwrap();
    let mut store = fixture.store("source-store");
    let imported = store.import_legacy(&legacy, &assets).unwrap();
    let mut no_asset = imported.value().clone();
    no_asset["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("assetId");
    let current = store
        .save(&ProjectDocument::from_value(no_asset).unwrap(), 0)
        .unwrap();
    assert!(current.referenced_assets().unwrap().is_empty());
    let receipt = store
        .export_archive(imported.id(), &fixture.archive())
        .unwrap();
    assert_eq!(receipt.asset_count, 1);
    assert_eq!(receipt.history_rows, 1);
    let mut restored = fixture.store("restored");
    assert_eq!(
        restored.restore_archive(&fixture.archive()).unwrap(),
        current
    );
    assert_eq!(
        restored.imported_original(imported.id()).unwrap(),
        Some(raw.clone())
    );
    assert_eq!(restored.asset(&asset).unwrap(), bytes);
    let undo = restored.undo(imported.id(), 1).unwrap();
    assert!(undo.referenced_assets().unwrap().contains(&asset));
    let redo = restored.redo(imported.id(), undo.revision()).unwrap();
    assert!(redo.referenced_assets().unwrap().is_empty());
    assert_eq!(fs::read(&legacy).unwrap(), raw);
    assert_eq!(fs::read(assets.join(asset)).unwrap(), bytes);
}

#[test]
fn operation_only_native_root_retains_complete_recovered_object_and_historical_receipt() {
    let fixture = Fixture::new();
    let (branch, intent, manifest) = native_branch(&fixture);
    let archive = Connection::open(fixture.archive()).unwrap();
    assert_eq!(row_count(&archive, "native_takes"), 1);
    assert_eq!(row_count(&archive, "operations"), 2);
    assert_eq!(row_count(&archive, "edits"), 1);
    drop(archive);
    let mut restored = fixture.store("restored");
    assert_eq!(
        restored.restore_archive(&fixture.archive()).unwrap(),
        branch
    );
    let replay = restored.publish_take(&intent).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document.revision(), 1);
    assert_eq!(restored.load(branch.id()).unwrap(), branch);
    let object = fs::read_dir(fixture.0.join("restored/takes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "sqlite3"))
        .unwrap();
    let reader = SealedJournal::open(&object).unwrap();
    assert_eq!(reader.manifest(), &manifest);
    assert!(reader.read_chunk("audio", 1).is_err());
    let db = Connection::open(&object).unwrap();
    assert_eq!(row_count(&db, "chunks"), 2);
    assert_eq!(row_count(&db, "interruptions"), 1);
    assert_eq!(row_count(&db, "recovery_index"), 1);
    let bad: Vec<u8> = db
        .query_row("SELECT payload FROM chunks WHERE sequence=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(bad, [9, 8, 7, 6]);
    let undo = restored.undo(branch.id(), branch.revision()).unwrap();
    let redo = restored.redo(branch.id(), undo.revision()).unwrap();
    assert_eq!(redo.name(), branch.name());
    assert_eq!(
        restored.publish_take(&intent).unwrap().document.revision(),
        1
    );
    assert_eq!(restored.load(branch.id()).unwrap(), redo);
}

#[test]
fn corrupted_missing_reordered_oversized_or_truncated_archive_chunks_publish_no_project() {
    let fixture = Fixture::new();
    native_branch(&fixture);
    for mode in [
        "payload",
        "missing",
        "sequence",
        "oversize",
        "traversal",
        "negative",
        "version",
        "extra-schema",
        "truncated",
    ] {
        let path = fixture.0.join(format!("{mode}.omniusong"));
        fs::copy(fixture.archive(), &path).unwrap();
        if mode == "truncated" {
            let len = fs::metadata(&path).unwrap().len();
            fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(len / 2)
                .unwrap();
        } else {
            let db = Connection::open(&path).unwrap();
            match mode {
                "payload" => {
                    db.execute("UPDATE take_chunks SET payload=zeroblob(byte_length)", [])
                        .unwrap();
                }
                "missing" => {
                    db.execute("DELETE FROM take_chunks WHERE sequence=0", [])
                        .unwrap();
                }
                "sequence" => {
                    db.execute("UPDATE take_chunks SET sequence=sequence+1", [])
                        .unwrap();
                }
                "oversize" => {
                    db.execute(
                        "UPDATE take_chunks SET payload=zeroblob(1048577),byte_length=1048577",
                        [],
                    )
                    .unwrap();
                }
                "traversal" => {
                    db.execute("UPDATE native_takes SET object_sha256='../escape'", [])
                        .unwrap();
                }
                "negative" => {
                    db.execute("UPDATE archive_meta SET take_bytes=-1", [])
                        .unwrap();
                }
                "version" => db.pragma_update(None, "user_version", 99).unwrap(),
                _ => db
                    .execute_batch("CREATE VIEW unexpected_view AS SELECT * FROM projects;")
                    .unwrap(),
            }
        }
        let name = format!("reject-{mode}");
        let mut restored = fixture.store(&name);
        assert!(restored.restore_archive(&path).is_err(), "accepted {mode}");
        assert_no_published_rows(&fixture, &name);
        assert!(!fixture.0.join("escape").exists());
    }
}

#[test]
fn final_store_insert_failure_rolls_back_all_visible_restore_rows_and_retry_succeeds() {
    let fixture = Fixture::new();
    let (branch, _, _) = native_branch(&fixture);
    let mut restored = fixture.store("restored");
    fixture.db("restored").execute_batch("CREATE TRIGGER isolated_restore_fault BEFORE INSERT ON operations BEGIN SELECT RAISE(ABORT,'isolated archive receipt fault'); END;").unwrap();
    assert!(restored.restore_archive(&fixture.archive()).is_err());
    assert_no_published_rows(&fixture, "restored");
    fixture
        .db("restored")
        .execute_batch("DROP TRIGGER isolated_restore_fault;")
        .unwrap();
    assert_eq!(
        restored.restore_archive(&fixture.archive()).unwrap(),
        branch
    );
}

#[test]
fn restore_allows_unrelated_projects_but_rejects_project_and_global_operation_collisions() {
    let fixture = Fixture::new();
    let mut source = fixture.store("source-store");
    let original = source
        .create(&ProjectDocument::new("Archive project").unwrap())
        .unwrap();
    let intent = edit(&original, "archive-global-id");
    let current = source.apply_edit(&intent).unwrap().document;
    source
        .export_archive(original.id(), &fixture.archive())
        .unwrap();
    let mut target = fixture.store("target");
    let unrelated = target
        .create(&ProjectDocument::new("Unrelated").unwrap())
        .unwrap();
    assert_eq!(target.restore_archive(&fixture.archive()).unwrap(), current);
    assert_eq!(target.load(unrelated.id()).unwrap(), unrelated);
    assert!(target.restore_archive(&fixture.archive()).is_err());
    assert_eq!(row_count(&fixture.db("target"), "projects"), 2);
    let mut collision = fixture.store("collision");
    let other = collision
        .create(&ProjectDocument::new("Operation owner").unwrap())
        .unwrap();
    let existing = collision
        .apply_edit(&edit(&other, "archive-global-id"))
        .unwrap()
        .document;
    assert!(collision.restore_archive(&fixture.archive()).is_err());
    assert_eq!(collision.load(other.id()).unwrap(), existing);
    assert_eq!(row_count(&fixture.db("collision"), "projects"), 1);
    assert_eq!(row_count(&fixture.db("collision"), "operations"), 1);
}

#[test]
fn export_existing_name_and_symlink_conflicts_leave_existing_bytes_and_source_unchanged() {
    let fixture = Fixture::new();
    let mut store = fixture.store("source-store");
    let project = store
        .create(&ProjectDocument::new("Conflict").unwrap())
        .unwrap();
    fs::write(fixture.archive(), b"existing archive").unwrap();
    assert!(
        store
            .export_archive(project.id(), &fixture.archive())
            .is_err()
    );
    assert_eq!(fs::read(fixture.archive()).unwrap(), b"existing archive");
    assert_eq!(store.load(project.id()).unwrap(), project);
    #[cfg(unix)]
    {
        let link = fixture.0.join("dangling.omniusong");
        let absent = fixture.0.join("absent-target");
        std::os::unix::fs::symlink(&absent, &link).unwrap();
        assert!(store.export_archive(project.id(), &link).is_err());
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!absent.exists());
    }
}

#[test]
fn oversized_metadata_rows_are_rejected_before_any_restore_publication() {
    let fixture = Fixture::new();
    let mut source = fixture.store("source-store");
    let original = source
        .create(&ProjectDocument::new("Metadata bound").unwrap())
        .unwrap();
    source
        .export_archive(original.id(), &fixture.archive())
        .unwrap();
    let db = Connection::open(fixture.archive()).unwrap();
    db.execute(
        "UPDATE projects SET payload=CAST(zeroblob(?1) AS TEXT)",
        [i64::try_from(omniusong_core::native::MAX_NATIVE_PROJECT_BYTES + 1).unwrap()],
    )
    .unwrap();
    drop(db);
    let mut restored = fixture.store("restored");
    assert!(restored.restore_archive(&fixture.archive()).is_err());
    assert_no_published_rows(&fixture, "restored");
}
