//! Independent OSRE byte/SQL oracles on isolated synthetic stores. Catalog
//! lifecycle JSON below is deliberately opaque store metadata with recognizable
//! linkage, not a legal typed session history or native recording authority.
#![cfg(unix)]

#[path = "export_origin_tests.rs"]
mod export_origin_tests;
#[path = "export_reconciliation_tests.rs"]
mod export_reconciliation_tests;
#[path = "inert_reconciliation_oracle_tests.rs"]
mod inert_reconciliation_oracle_tests;

use super::*;
use crate::STORE_SCHEMA_VERSION;
use omniusong_core::{ProjectDocument, contracts::TakeId};
use omniusong_recording::{
    CaptureReader, ChunkHeader, ChunkKind, ClockDomainId, ClockEpochId, CopyActionOwner, HostClock,
    HostTimestamp, InspectionLimits, InspectionResource, Journal, PcmEncoding, PcmFormat,
    PcmLayout, RecordingError, RecoveryCopy, RecoveryPrefix, SealedJournal, SourceRange,
    StreamDescriptor, StreamKind, TakeDescriptor,
};
use rusqlite::{Connection, OpenFlags, params, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const EXPORT: &str = "11111111111111111111111111111111";
const ACTION: &str = "22222222222222222222222222222222";
const STAGE: &str = "33333333333333333333333333333333";
const MISSING: &str = "44444444444444444444444444444444";
const HEADER: &[u8; 8] = b"OSRE\x01\r\n\0";
const END: &[u8; 8] = b"OSREEND1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
fn cells(connection: &Connection, sql: &str) -> Vec<Vec<Cell>> {
    let mut statement = connection.prepare(sql).unwrap();
    let columns = statement.column_count();
    let mut rows = statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| {
                    Ok(match row.get_ref(column)? {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(n) => Cell::Integer(n),
                        ValueRef::Real(n) => Cell::Real(n.to_bits()),
                        ValueRef::Text(b) => Cell::Text(b.to_vec()),
                        ValueRef::Blob(b) => Cell::Blob(b.to_vec()),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.sort();
    rows
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Catalog {
    schema: Vec<Vec<Cell>>,
    tables: BTreeMap<String, Vec<Vec<Cell>>>,
    version: i64,
    application: i64,
}
fn catalog(connection: &Connection) -> Catalog {
    let names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    Catalog {
        schema: cells(
            connection,
            "SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema",
        ),
        tables: names
            .into_iter()
            .map(|name| {
                let rows = cells(
                    connection,
                    &format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")),
                );
                (name, rows)
            })
            .collect(),
        version: connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap(),
        application: connection
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .unwrap(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    Directory,
    File(Vec<u8>),
}
type Tree = BTreeMap<String, Item>;
fn tree(root: &Path) -> Tree {
    fn visit(root: &Path, path: &Path, output: &mut Tree) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let key = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace(std::path::MAIN_SEPARATOR, "/");
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(
                !metadata.file_type().is_symlink(),
                "oracle only follows its known ordinary fixture"
            );
            if metadata.is_dir() {
                output.insert(key, Item::Directory);
                visit(root, &path, output);
            } else {
                output.insert(key, Item::File(fs::read(path).unwrap()));
            }
        }
    }
    let mut result = Tree::new();
    if root.exists() {
        visit(root, root, &mut result);
    }
    result
}
fn retained_tree(root: &Path) -> Tree {
    tree(root)
        .into_iter()
        .filter(|(path, _)| {
            ["captures", "source-actions", "takes"]
                .iter()
                .any(|name| path == name || path.starts_with(&format!("{name}/")))
        })
        .collect()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn committed_wal_catalog_cells_survive_without_checkpointing_or_rewriting_live_sources() {
    let mut fixture = Fixture::new();
    fixture.store.connection.execute_batch("PRAGMA journal_mode=WAL; INSERT INTO assets(id,payload) VALUES('committed-in-wal',x'00ff1234');").unwrap();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let main = fs::read(fixture.store.root().join("studio.sqlite3")).unwrap();
    let receipt = fixture.export("wal-catalog.osre");
    let parsed = parse_independent(&receipt.path);
    let restored = fixture.root.join("wal-inert");
    fixture
        .store
        .restore_retained_evidence(&receipt.path, &restored, &budget())
        .unwrap();
    let index = parsed
        .manifest
        .entries
        .iter()
        .position(|e| e.path == "catalog.sqlite3")
        .unwrap();
    let copy = Connection::open_with_flags(
        restored.join("objects").join(format!("{index:04}.bin")),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(catalog(&copy), before);
    assert_eq!(
        fs::read(fixture.store.root().join("studio.sqlite3")).unwrap(),
        main
    );
    assert_live_unchanged(&fixture, &before, &files);
}
fn governor(limits: InspectionLimits) -> InspectionBudget {
    InspectionBudget::new(limits, Arc::new(AtomicBool::new(false))).unwrap()
}
fn budget() -> InspectionBudget {
    governor(InspectionLimits::default())
}

struct Fixture {
    root: PathBuf,
    store: NativeStore,
    original: PathBuf,
    working: PathBuf,
    owned: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/retained-evidence-archive-store-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut store = NativeStore::open(root.join("native")).unwrap();
        let project = store
            .create(&ProjectDocument::new("Original catalog remains live").unwrap())
            .unwrap();
        let mut value = project.value().clone();
        value["name"] = "Later metadata and original history".into();
        store
            .save(
                &ProjectDocument::from_value(value).unwrap(),
                project.revision(),
            )
            .unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TABLE opaque_evidence_cells(n, i, r, t, b); PRAGMA application_id=734921;",
            )
            .unwrap();
        store
            .connection
            .execute(
                "INSERT INTO opaque_evidence_cells VALUES(NULL,-17,1.25,?1,?2)",
                params!["text\0é", vec![0u8, 255, 128, 7]],
            )
            .unwrap();
        store.connection.execute_batch("INSERT INTO opaque_evidence_cells VALUES(NULL,9223372036854775807,-0.5,CAST(x'61ff0062' AS TEXT),x'00ff');").unwrap();
        store
            .connection
            .execute(
                "INSERT INTO assets(id,payload) VALUES(?1,?2)",
                params!["unreferenced-opaque", b"\0asset\xfftail".as_slice()],
            )
            .unwrap();
        let native = store.root().to_owned();
        let stage = native.join("captures").join(STAGE);
        fs::create_dir_all(stage.join("empty-observation")).unwrap();
        fs::create_dir_all(native.join("source-actions")).unwrap();
        let original = stage.join("take.sqlite3");
        let descriptor = TakeDescriptor {
            version: 1,
            take_id: TakeId("synthetic-osre-original".into()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                source_id: "synthetic".into(),
                source_name: "Synthetic raw evidence; no device".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("clock".into()),
                    epoch: ClockEpochId("epoch".into()),
                    ticks_per_second_numerator: 48_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 1,
                        encoding: PcmEncoding::F32Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("audio-epoch".into()),
                },
                initial_source_position: 0,
            }],
            provenance: BTreeMap::from([(
                "opaque-extension".into(),
                serde_json::json!({"text":"raw\u{0}evidence","future":[1,true,null]}),
            )]),
        };
        let mut journal = Journal::create(&original, &descriptor).unwrap();
        for sequence in 0..2 {
            let stamp = HostTimestamp {
                domain: ClockDomainId("clock".into()),
                epoch: ClockEpochId("epoch".into()),
                ticks: sequence * 2,
            };
            journal
                .append(
                    &ChunkHeader {
                        version: 1,
                        take_id: descriptor.take_id.clone(),
                        stream_id: "audio".into(),
                        segment_id: "segment".into(),
                        sequence,
                        range: SourceRange::AudioFrames {
                            start: sequence * 2,
                            count: 2,
                        },
                        interpretation: descriptor.streams[0].kind.clone(),
                        observed_start: stamp.clone(),
                        observed_end: stamp,
                        clock_anchors: vec![],
                        kind: ChunkKind::Content,
                    },
                    &[0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat(),
                )
                .unwrap();
        }
        drop(journal);
        let governor = budget();
        let reader = CaptureReader::open_bounded(&original, &governor).unwrap();
        let image_sha = reader.inspection().journal_sha256.clone();
        let image_bytes = reader.inspection().byte_length;
        let prefix = RecoveryPrefix {
            chunk_count: reader.inspection().report.verified_chunks,
            chunk_index_sha256: reader.inspection().report.chunk_index_sha256.clone(),
        };
        let owner = CopyActionOwner::create(&native.join("source-actions"), ACTION).unwrap();
        let recovered = RecoveryCopy::create(reader, native.join("source-actions"), ACTION)
            .unwrap()
            .finalize(&prefix, "opaque-store-test-recovery")
            .unwrap();
        drop(owner);
        let working = recovered.paths.working.clone();
        let sealed = SealedJournal::open(&working).unwrap();
        let object = store.ingest_take(&sealed).unwrap();
        drop(sealed);
        let owned = native
            .join("takes")
            .join(format!("{}.sqlite3", object.object_sha256));
        let source_path = serde_json::to_string(&original).unwrap();
        let intent = format!(
            " {{\"version\":1,\"journal_path\":{source_path},\"descriptor_sha256\":\"{}\",\"opaque\":\"\\u0061\"}} \n",
            recovered.inspection.descriptor_sha256
        );
        store
            .save_performance_intent(STAGE, None, intent.as_bytes())
            .unwrap();
        let missing_path =
            serde_json::to_string(&native.join("captures").join(MISSING).join("take.sqlite3"))
                .unwrap();
        let absent = format!(
            " {{\"version\":1,\"journal_path\":{missing_path},\"descriptor_sha256\":null,\"opaque\":true}} \n"
        );
        store
            .save_performance_intent(MISSING, None, absent.as_bytes())
            .unwrap();
        let review = format!(
            " {{\"version\":1,\"stage_id\":\"{STAGE}\",\"journal_path\":{source_path},\"opaque\":\"\\u0062\"}} \n"
        );
        let prepared = format!(
            " {{\"version\":1,\"id\":\"{ACTION}\",\"stage_id\":\"{STAGE}\",\"opaque\":\"prepared raw bytes\"}} \n"
        );
        store
            .save_performance_source_action(ACTION, None, review.as_bytes(), prepared.as_bytes())
            .unwrap();
        let started = format!(
            " {{\"version\":1,\"id\":\"{ACTION}\",\"phase\":{{\"state\":\"started\"}}}} \n"
        );
        store
            .save_performance_source_action(ACTION, Some(1), review.as_bytes(), started.as_bytes())
            .unwrap();
        let verified = serde_json::json!({"version":1,"id":ACTION,"phase":{"state":"verified","result":{
            "take_id":descriptor.take_id.0,"descriptor_sha256":recovered.inspection.descriptor_sha256,"original_rows_sha256":recovered.inspection.original_rows_sha256,
            "evidence_sha256":image_sha,"evidence_bytes":image_bytes,"working_sha256":recovered.inspection.journal_sha256,"working_bytes":recovered.inspection.byte_length,
            "manifest_sha256":recovered.manifest.sha256,"selected_chunk_count":prefix.chunk_count,"selected_chunk_index_sha256":prefix.chunk_index_sha256,"unknown_tail":true}}});
        let verified = format!(" {} \n", serde_json::to_string_pretty(&verified).unwrap());
        store
            .save_performance_source_action(ACTION, Some(2), review.as_bytes(), verified.as_bytes())
            .unwrap();
        let decision = format!(
            " {{\"version\":1,\"id\":\"{ACTION}\",\"decision\":{{\"state\":\"kept_locally\",\"opaque_provenance\":true}}}} \n"
        );
        store
            .save_performance_source_action(ACTION, Some(3), review.as_bytes(), decision.as_bytes())
            .unwrap();
        fs::write(
            stage.join("take.sqlite3-journal"),
            b"raw ambiguous rollback sidecar\0\xff",
        )
        .unwrap();
        fs::write(
            recovered.paths.evidence.with_extension("sqlite3-wal"),
            b"future raw WAL bytes",
        )
        .unwrap();
        fs::write(
            working.with_extension("sqlite3-shm"),
            b"raw shared memory bytes",
        )
        .unwrap();
        fs::write(stage.join("unknown.bin"), b"uninterpreted\0binary\xff").unwrap();
        fs::write(
            native.join("takes/.recovered-ingest-orphan.partial"),
            b"retained incomplete ingest\0",
        )
        .unwrap();
        fs::write(
            native.join("takes/orphan-future.sqlite3"),
            b"not SQLite; retain exactly",
        )
        .unwrap();
        fs::create_dir(native.join("takes/empty-orphan-directory")).unwrap();
        Self {
            root,
            store,
            original,
            working,
            owned,
        }
    }
    fn archive(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
    fn export(&mut self, name: &str) -> EvidenceArchiveReceipt {
        self.store
            .export_retained_evidence_full_root(self.archive(name), EXPORT, &budget())
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("OSRE fixture retained: {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

pub(super) fn ownerless_sqlite_probe(path: &Path, expect_busy: bool) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "evidence_archive::raw::tests::sqlite_writer_probe_subprocess",
            "--ignored",
            "--nocapture",
        ])
        .env("OMNIUSONG_RAW_SQLITE_PROBE_PATH", path)
        .env("OMNIUSONG_RAW_SQLITE_PROBE_OWNER", "no")
        .env(
            "OMNIUSONG_RAW_SQLITE_PROBE_BUSY",
            if expect_busy { "yes" } else { "no" },
        )
        .output()
        .unwrap();
    assert!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("RAW_SQLITE_PROBE_COMPLETED"),
        "ownerless SQLite probe: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn evidence_refusal_preserves_ownerless_ingest_sqlite_locks_across_store_handles() {
    let mut fixture = Fixture::new();
    let mut second = NativeStore::open_existing(fixture.store.root()).unwrap();
    let writer = crate::retained_file_coordination::writer(fixture.store.root()).unwrap();
    let stage = fixture
        .store
        .root()
        .join("takes/.omniusong-backup-active.partial");
    fs::create_dir(&stage).unwrap();
    let path = stage.join("journal.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE probe(n); INSERT INTO probe VALUES(9); BEGIN IMMEDIATE;").unwrap();
    ownerless_sqlite_probe(&path, true);
    let destination = fixture.archive("writer-refusal.osre");
    assert!(matches!(
        second.export_retained_evidence_full_root(&destination, EXPORT, &budget()),
        Err(StoreError::Recording(RecordingError::Owned))
    ));
    assert!(!destination.exists());
    ownerless_sqlite_probe(&path, true);
    // Coordination is per root: unrelated stores remain available.
    let mut unrelated = Fixture::new();
    unrelated.export("unrelated.osre");
    connection.execute_batch("ROLLBACK").unwrap();
    ownerless_sqlite_probe(&path, false);
    drop(connection);
    drop(writer);
    let receipt = fixture.export("writer-refusal.osre");
    assert_eq!(
        parse_independent(&destination).payload["takes/.omniusong-backup-active.partial/journal.sqlite3"],
        fs::read(&path).unwrap()
    );
    // Historical replay authenticates existing bytes without acquiring the
    // live tree, even while a new ingest transaction is active.
    let writer = crate::retained_file_coordination::writer(fixture.store.root()).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(
        second
            .export_retained_evidence_full_root(&destination, EXPORT, &budget())
            .unwrap(),
        receipt
    );
    ownerless_sqlite_probe(&path, true);
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
    drop(writer);
}

#[test]
fn evidence_export_excludes_ingestion_until_all_raw_descriptors_close() {
    let mut fixture = Fixture::new();
    let mut second = NativeStore::open_existing(fixture.store.root()).unwrap();
    let source = SealedJournal::open(&fixture.owned).unwrap();
    let destination = fixture.archive("exclusive.osre");
    let mut observed = false;
    fixture
        .store
        .export_evidence_using(&destination, EXPORT, &budget(), |point| {
            if point == EvidenceExportBoundary::AfterInventory {
                observed = true;
                assert!(matches!(
                    second.ingest_take(&source),
                    Err(StoreError::Recording(RecordingError::Owned))
                ));
            }
            Ok(())
        })
        .unwrap();
    assert!(observed);
    // A successful export releases its process gate and every raw owner.
    assert_eq!(
        second.ingest_take(&source).unwrap().take_id,
        source.descriptor().take_id
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled_budget =
        InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancel)).unwrap();
    let cancelled_path = fixture.archive("cancel-after-inventory.osre");
    let error = fixture
        .store
        .export_evidence_using(&cancelled_path, EXPORT, &cancelled_budget, |point| {
            if point == EvidenceExportBoundary::AfterInventory {
                cancel.store(true, Ordering::Release);
            }
            Ok(())
        })
        .unwrap_err();
    typed_stop(&error, None);
    assert!(!cancelled_path.exists());
    // Cancellation after raw acquisition releases both ownership layers.
    assert_eq!(
        second.ingest_take(&source).unwrap().take_id,
        source.descriptor().take_id
    );
    fixture.export("after-cancel.osre");
}

#[test]
fn nested_catalog_and_overlapping_ingest_are_refused_before_any_database_descriptor() {
    let mut fixture = Fixture::new();
    let nested = NativeStore::open(fixture.store.root().join("takes/nested-native")).unwrap();
    let path = nested.root().join("studio.sqlite3");
    let writer = crate::retained_file_coordination::writer(nested.root()).unwrap();
    nested.connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    ownerless_sqlite_probe(&path, true);
    let destination = fixture.archive("nested.osre");
    assert!(matches!(
        fixture
            .store
            .export_retained_evidence_full_root(&destination, EXPORT, &budget()),
        Err(StoreError::Recording(RecordingError::Owned))
    ));
    ownerless_sqlite_probe(&path, true);
    drop(writer);
    // Catalog saves also exist without a file-ingestion lease. Even an idle
    // nested NativeStore is unsupported as raw evidence; metadata must refuse.
    let error = fixture
        .store
        .export_retained_evidence_full_root(&destination, EXPORT, &budget())
        .unwrap_err();
    assert!(error.to_string().contains("Nested native catalog"));
    ownerless_sqlite_probe(&path, true);
    assert!(!destination.exists());
    nested.connection.execute_batch("ROLLBACK").unwrap();
    ownerless_sqlite_probe(&path, false);
    drop(nested);
    let upper = path.with_file_name("STUDIO.SQLITE3");
    fs::rename(&path, &upper).unwrap();
    let error = fixture
        .store
        .export_retained_evidence_full_root(&destination, EXPORT, &budget())
        .unwrap_err();
    assert!(error.to_string().contains("Nested native catalog"));
    assert!(!destination.exists());
}

#[test]
fn cross_root_archive_destinations_and_nested_open_cannot_bypass_evidence_lease() {
    let mut fixture = Fixture::new();
    let mut unrelated = Fixture::new();
    let project = unrelated
        .store
        .create(&ProjectDocument::new("Disjoint song").unwrap())
        .unwrap();
    let unrelated_output = unrelated.archive("disjoint.osar");
    let forbidden_output = fixture.store.root().join("takes/cross-root.osar");
    let new_root = fixture.store.root().join("new-nested-store");
    let output = fixture.archive("cross-root.osre");
    fixture
        .store
        .export_evidence_using(&output, EXPORT, &budget(), |point| {
            if point == EvidenceExportBoundary::AfterInventory {
                assert!(matches!(
                    unrelated
                        .store
                        .export_archive(project.id(), &forbidden_output),
                    Err(StoreError::Recording(RecordingError::Owned))
                ));
                assert!(matches!(
                    NativeStore::open(&new_root),
                    Err(StoreError::Recording(RecordingError::Owned))
                ));
                assert!(!forbidden_output.exists());
                assert!(!new_root.join("studio.sqlite3").exists());
                unrelated
                    .store
                    .export_archive(project.id(), &unrelated_output)
                    .unwrap();
            }
            Ok(())
        })
        .unwrap();
    assert!(unrelated_output.exists());
    // A fresh OSRE's private catalog is beside its destination, so that path
    // must exclude the other store's ownerless ingest writers as well.
    let source_writer = crate::retained_file_coordination::writer(unrelated.store.root()).unwrap();
    let other_output = unrelated.store.root().join("takes/other-source.osre");
    assert!(matches!(
        fixture.store.export_retained_evidence_full_root(
            &other_output,
            "55555555555555555555555555555555",
            &budget()
        ),
        Err(StoreError::Recording(RecordingError::Owned))
    ));
    assert!(!other_output.exists());
    drop(source_writer);
}

#[test]
fn wrong_native_database_selection_and_active_partial_inputs_preserve_sqlite_writer_locks() {
    let fixture = Fixture::new();
    let catalog = fixture.store.root().join("studio.sqlite3");
    fixture
        .store
        .connection
        .execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE")
        .unwrap();
    ownerless_sqlite_probe(&catalog, true);
    assert!(inspect_evidence_archive(&catalog, &budget()).is_err());
    assert!(
        fixture
            .store
            .restore_retained_evidence(
                &catalog,
                fixture.archive("wrong-catalog-restore"),
                &budget()
            )
            .is_err()
    );
    ownerless_sqlite_probe(&catalog, true);
    assert!(catalog.with_file_name("studio.sqlite3-shm").exists());
    for name in [
        "studio.sqlite3-shm",
        "studio.sqlite3-wal",
        "studio.sqlite3-journal",
        "STUDIO.SQLITE3-SHM",
    ] {
        let path = catalog.with_file_name(name);
        assert!(
            inspect_evidence_archive(&path, &budget())
                .unwrap_err()
                .to_string()
                .contains("reserved")
        );
    }
    ownerless_sqlite_probe(&catalog, true);
    fixture.store.connection.execute_batch("ROLLBACK").unwrap();
    let path = fixture.store.root().join("takes/.ingest-active.partial");
    let writer = crate::retained_file_coordination::writer(fixture.store.root()).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE probe(n); BEGIN IMMEDIATE")
        .unwrap();
    ownerless_sqlite_probe(&path, true);
    assert!(matches!(
        inspect_evidence_archive(&path, &budget()),
        Err(StoreError::Recording(RecordingError::Owned))
    ));
    assert!(matches!(
        fixture.store.restore_retained_evidence(
            &path,
            fixture.archive("wrong-partial-restore"),
            &budget()
        ),
        Err(StoreError::Recording(RecordingError::Owned))
    ));
    ownerless_sqlite_probe(&path, true);
    assert!(!fixture.archive("wrong-catalog-restore").exists());
    assert!(!fixture.archive("wrong-partial-restore").exists());
    connection.execute_batch("ROLLBACK").unwrap();
    ownerless_sqlite_probe(&path, false);
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut owner = path.as_os_str().to_os_string();
    owner.push(".owner.lock");
    fs::write(PathBuf::from(owner), b"existing misnamed recording owner").unwrap();
    drop(writer);
    assert!(
        inspect_evidence_archive(&path, &budget())
            .unwrap_err()
            .to_string()
            .contains("Owned recording name")
    );
    ownerless_sqlite_probe(&path, true);
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
}

struct Parsed {
    manifest: EvidenceManifest,
    manifest_bytes: Vec<u8>,
    payload: BTreeMap<String, Vec<u8>>,
}
fn parse_independent(path: &Path) -> Parsed {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..8], HEADER);
    let length = usize::try_from(u64::from_le_bytes(bytes[8..16].try_into().unwrap())).unwrap();
    assert!(length <= 2 * 1024 * 1024);
    let manifest_bytes = bytes[16..16 + length].to_vec();
    let manifest: EvidenceManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest_bytes, serde_json::to_vec(&manifest).unwrap());
    let mut offset = 16 + length;
    let mut payload = BTreeMap::new();
    for entry in &manifest.entries {
        if entry.kind == EvidenceEntryKind::File {
            let size = usize::try_from(entry.byte_length).unwrap();
            let body = bytes[offset..offset + size].to_vec();
            assert_eq!(entry.sha256.as_deref(), Some(sha(&body).as_str()));
            assert!(payload.insert(entry.path.clone(), body).is_none());
            offset += size;
        }
    }
    assert_eq!(bytes.len(), offset + 40);
    assert_eq!(
        &bytes[offset..offset + 32],
        Sha256::digest(&bytes[..offset]).as_slice()
    );
    assert_eq!(&bytes[offset + 32..], END);
    Parsed {
        manifest,
        manifest_bytes,
        payload,
    }
}
fn assert_live_unchanged(fixture: &Fixture, expected_catalog: &Catalog, expected_tree: &Tree) {
    assert_eq!(catalog(&fixture.store.connection), *expected_catalog);
    assert_eq!(retained_tree(fixture.store.root()), *expected_tree);
}

#[test]
fn full_root_archive_preserves_raw_namespaces_and_every_catalog_sql_cell_then_restores_inertly() {
    let mut fixture = Fixture::new();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let receipt = fixture.export("complete.osre");
    let parsed = parse_independent(&receipt.path);
    assert_eq!(parsed.manifest.version, 1);
    assert_eq!(parsed.manifest.export_id, EXPORT);
    assert_eq!(
        parsed.manifest.scope,
        "catalog-and-all-retained-captures-source-actions-takes"
    );
    assert_eq!(parsed.manifest.catalog_schema, STORE_SCHEMA_VERSION);
    assert_eq!(
        parsed.manifest.roots,
        ["captures", "source-actions", "takes"].map(|path| EvidenceRoot {
            path: path.into(),
            present: true
        })
    );
    let paths: Vec<_> = parsed
        .manifest
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    let archived: Tree = parsed
        .manifest
        .entries
        .iter()
        .filter(|e| e.path != "catalog.sqlite3")
        .map(|entry| {
            (
                entry.path.clone(),
                if entry.kind == EvidenceEntryKind::Directory {
                    Item::Directory
                } else {
                    Item::File(parsed.payload[&entry.path].clone())
                },
            )
        })
        .collect();
    assert_eq!(archived, files);
    assert!(
        parsed
            .manifest
            .absences
            .iter()
            .any(|absence| absence.path.contains(MISSING))
    );
    assert_eq!(
        receipt.file_count,
        parsed
            .manifest
            .entries
            .iter()
            .filter(|e| e.kind == EvidenceEntryKind::File)
            .count() as u64
    );
    assert_eq!(
        receipt.directory_count,
        parsed
            .manifest
            .entries
            .iter()
            .filter(|e| e.kind == EvidenceEntryKind::Directory)
            .count() as u64
    );
    assert_eq!(
        receipt.payload_bytes,
        parsed.payload.values().map(|b| b.len() as u64).sum::<u64>()
    );
    assert_eq!(
        receipt.byte_length,
        fs::metadata(&receipt.path).unwrap().len()
    );
    assert_eq!(receipt.sha256, sha(&fs::read(&receipt.path).unwrap()));
    assert_eq!(
        parsed.manifest.origin_root,
        fixture.store.root().to_str().unwrap()
    );
    let inspection = inspect_evidence_archive(&receipt.path, &budget()).unwrap();
    assert_eq!(inspection.receipt, receipt);
    assert_eq!(inspection.manifest, parsed.manifest);
    let destination = fixture.root.join("inert-restored");
    let restored = fixture
        .store
        .restore_retained_evidence(&receipt.path, &destination, &budget())
        .unwrap();
    assert_eq!(restored.directory, destination);
    assert_eq!(restored.archive, receipt);
    assert_eq!(
        fs::read(destination.join("manifest.json")).unwrap(),
        parsed.manifest_bytes
    );
    assert_eq!(
        serde_json::from_slice::<EvidenceRestoreReceipt>(
            &fs::read(destination.join("complete.json")).unwrap()
        )
        .unwrap(),
        restored
    );
    let mut expected_names = BTreeSet::from([
        "manifest.json".to_owned(),
        "complete.json".to_owned(),
        "objects".to_owned(),
    ]);
    for (index, entry) in parsed.manifest.entries.iter().enumerate() {
        if entry.kind == EvidenceEntryKind::File {
            let name = format!("objects/{index:04}.bin");
            assert_eq!(
                fs::read(destination.join(&name)).unwrap(),
                parsed.payload[&entry.path]
            );
            expected_names.insert(name.clone());
            if entry.path == "catalog.sqlite3" {
                let connection = Connection::open_with_flags(
                    destination.join(name),
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .unwrap();
                assert_eq!(
                    catalog(&connection),
                    before,
                    "backup preserves exact SQL cell types and payload bytes"
                );
            }
        }
    }
    assert_eq!(
        tree(&destination).into_keys().collect::<BTreeSet<_>>(),
        expected_names
    );
    assert!(!destination.join("studio.sqlite3").exists());
    assert!(
        !destination.join("captures").exists()
            && !destination.join("source-actions").exists()
            && !destination.join("takes").exists()
    );
    assert_live_unchanged(&fixture, &before, &files);
}

#[test]
fn malformed_source_and_sidecar_bytes_are_preserved_without_source_sqlite_open_or_repair() {
    let mut fixture = Fixture::new();
    fs::write(
        &fixture.original,
        b"future or damaged source bytes\0\xff; not SQLite",
    )
    .unwrap();
    fs::write(
        fixture.original.parent().unwrap().join("observer.lock"),
        b"opaque operational lock bytes\0",
    )
    .unwrap();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let receipt = fixture.export("raw-malformed.osre");
    let parsed = parse_independent(&receipt.path);
    assert_eq!(
        parsed.payload[&format!("captures/{STAGE}/take.sqlite3")],
        fs::read(&fixture.original).unwrap()
    );
    let destination = fixture.root.join("raw-malformed-restored");
    fixture
        .store
        .restore_retained_evidence(&receipt.path, &destination, &budget())
        .unwrap();
    for (index, entry) in parsed.manifest.entries.iter().enumerate() {
        if entry.kind == EvidenceEntryKind::File {
            assert_eq!(
                fs::read(destination.join(format!("objects/{index:04}.bin"))).unwrap(),
                parsed.payload[&entry.path]
            );
        }
    }
    assert_live_unchanged(&fixture, &before, &files);
}

#[test]
fn missing_namespaces_and_unexecuted_requests_are_explicit_absences_without_created_owners() {
    let fixture = Fixture::new();
    let mut store = NativeStore::open(fixture.root.join("empty-native")).unwrap();
    let path = store
        .root()
        .join("captures")
        .join(MISSING)
        .join("take.sqlite3");
    let intent = serde_json::json!({"version":1,"journal_path":path,"descriptor_sha256":null,"opaque":"absence only"});
    store
        .save_performance_intent(MISSING, None, &serde_json::to_vec(&intent).unwrap())
        .unwrap();
    let before = catalog(&store.connection);
    let destination = fixture.root.join("empty.osre");
    let receipt = store
        .export_retained_evidence_full_root(&destination, EXPORT, &budget())
        .unwrap();
    let parsed = parse_independent(&receipt.path);
    assert!(parsed.manifest.roots.iter().all(|root| !root.present));
    assert_eq!(parsed.manifest.entries.len(), 1);
    assert_eq!(parsed.manifest.entries[0].path, "catalog.sqlite3");
    assert_eq!(parsed.manifest.absences.len(), 1);
    assert_eq!(
        parsed.manifest.absences[0].path,
        format!("captures/{MISSING}/take.sqlite3")
    );
    assert!(retained_tree(store.root()).is_empty());
    assert_eq!(catalog(&store.connection), before);
}

#[test]
fn complete_same_request_replays_after_all_sources_disappear_but_wrong_export_or_origin_refuses() {
    let mut fixture = Fixture::new();
    let first = fixture.export("historical.osre");
    let archive_bytes = fs::read(&first.path).unwrap();
    for name in ["captures", "source-actions", "takes"] {
        fs::remove_dir_all(fixture.store.root().join(name)).unwrap();
    }
    let before = catalog(&fixture.store.connection);
    let replay = fixture
        .store
        .export_retained_evidence_full_root(&first.path, EXPORT, &budget())
        .unwrap();
    assert_eq!(replay, first);
    assert!(
        fixture
            .store
            .export_retained_evidence_full_root(&first.path, ACTION, &budget())
            .is_err()
    );
    let mut foreign = NativeStore::open(fixture.root.join("other-native")).unwrap();
    assert!(
        foreign
            .export_retained_evidence_full_root(&first.path, EXPORT, &budget())
            .is_err(),
        "same export ID cannot claim another canonical root"
    );
    assert_eq!(fs::read(&first.path).unwrap(), archive_bytes);
    assert_eq!(catalog(&fixture.store.connection), before);
    assert!(retained_tree(fixture.store.root()).is_empty());
}

#[test]
fn partial_existing_export_and_existing_restore_targets_are_retained_without_overwrite() {
    let mut fixture = Fixture::new();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let partial = fixture.archive("partial.osre");
    fs::write(&partial, HEADER).unwrap();
    assert!(
        fixture
            .store
            .export_retained_evidence_full_root(&partial, EXPORT, &budget())
            .is_err()
    );
    assert_eq!(fs::read(&partial).unwrap(), HEADER);
    let complete = fixture.export("complete.osre");
    for file in [false, true] {
        let destination = fixture.root.join(if file {
            "existing-file"
        } else {
            "existing-directory"
        });
        if file {
            fs::write(&destination, b"retain existing file").unwrap();
        } else {
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("sentinel"), b"retain existing directory").unwrap();
        }
        let expected = if file {
            fs::read(&destination).unwrap()
        } else {
            fs::read(destination.join("sentinel")).unwrap()
        };
        assert!(
            fixture
                .store
                .restore_retained_evidence(&complete.path, &destination, &budget())
                .is_err()
        );
        assert_eq!(
            if file {
                fs::read(destination).unwrap()
            } else {
                fs::read(destination.join("sentinel")).unwrap()
            },
            expected
        );
    }
    let inside = fixture.store.root().join("not-an-adopted-root");
    assert!(
        fixture
            .store
            .restore_retained_evidence(&complete.path, &inside, &budget())
            .is_err()
    );
    assert!(!inside.exists());
    let inside_export = fixture.store.root().join("must-not-export-inside.osre");
    assert!(
        fixture
            .store
            .export_retained_evidence_full_root(&inside_export, EXPORT, &budget())
            .is_err()
    );
    assert!(!inside_export.exists());
    assert_live_unchanged(&fixture, &before, &files);
}

fn owner_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".owner.lock");
    PathBuf::from(name)
}
#[test]
fn every_active_capture_working_action_or_archive_owner_refuses_without_changing_bytes() {
    let mut fixture = Fixture::new();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    for (index, path) in [
        owner_path(&fixture.original),
        owner_path(&fixture.working),
        fixture
            .store
            .root()
            .join("source-actions")
            .join(format!("{ACTION}.execution.lock")),
    ]
    .into_iter()
    .enumerate()
    {
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        owner.try_lock().unwrap();
        let destination = fixture.archive(&format!("busy-{index}.osre"));
        assert!(
            fixture
                .store
                .export_retained_evidence_full_root(&destination, EXPORT, &budget())
                .is_err()
        );
        drop(owner);
        assert_live_unchanged(&fixture, &before, &files);
    }
    let complete = fixture.export("busy-archive.osre");
    let archive = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&complete.path)
        .unwrap();
    archive.try_lock().unwrap();
    assert!(inspect_evidence_archive(&complete.path, &budget()).is_err());
    assert!(
        fixture
            .store
            .export_retained_evidence_full_root(&complete.path, EXPORT, &budget())
            .is_err()
    );
    let restored = fixture.root.join("busy-restore");
    assert!(
        fixture
            .store
            .restore_retained_evidence(&complete.path, &restored, &budget())
            .is_err()
    );
    assert!(!restored.exists());
    drop(archive);
    assert_live_unchanged(&fixture, &before, &files);
}

#[test]
fn missing_required_capture_working_image_registered_object_or_owner_refuses_complete_export() {
    for case in 0..5 {
        let mut fixture = Fixture::new();
        let removed = match case {
            0 => fixture.original.clone(),
            1 => fixture.working.clone(),
            2 => fixture
                .store
                .root()
                .join("source-actions")
                .join(ACTION)
                .join("evidence/take.sqlite3"),
            3 => fixture.owned.clone(),
            _ => owner_path(&fixture.original),
        };
        fs::remove_file(removed).unwrap();
        let before = catalog(&fixture.store.connection);
        let files = retained_tree(fixture.store.root());
        let destination = fixture.archive(&format!("missing-{case}.osre"));
        assert!(
            fixture
                .store
                .export_retained_evidence_full_root(&destination, EXPORT, &budget())
                .is_err(),
            "missing case {case}"
        );
        if destination.exists() {
            assert!(!fs::read(destination).unwrap().ends_with(END));
        }
        assert_live_unchanged(&fixture, &before, &files);
    }
}

#[test]
fn unsupported_linkage_refuses_instead_of_silently_omitting_opaque_lifecycle_rows() {
    let mut fixture = Fixture::new();
    fixture
        .store
        .save_performance_intent(
            "55555555555555555555555555555555",
            None,
            b" {\"version\":999,\"opaque_future\":true} \n",
        )
        .unwrap();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    assert!(
        fixture
            .store
            .export_retained_evidence_full_root(
                fixture.archive("unsupported-link.osre"),
                EXPORT,
                &budget()
            )
            .is_err()
    );
    assert_live_unchanged(&fixture, &before, &files);
}

fn bundle(
    manifest: &EvidenceManifest,
    payload: &BTreeMap<String, Vec<u8>>,
    raw_manifest: Option<Vec<u8>>,
) -> Vec<u8> {
    let json = raw_manifest.unwrap_or_else(|| serde_json::to_vec(manifest).unwrap());
    let mut bytes = HEADER.to_vec();
    bytes.extend_from_slice(&(json.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&json);
    for entry in &manifest.entries {
        if entry.kind == EvidenceEntryKind::File
            && let Some(body) = payload.get(&entry.path)
        {
            bytes.extend_from_slice(body);
        }
    }
    let checksum = Sha256::digest(&bytes);
    bytes.extend_from_slice(&checksum);
    bytes.extend_from_slice(END);
    bytes
}
fn parser_fixture(fixture: &mut Fixture) -> (EvidenceManifest, BTreeMap<String, Vec<u8>>) {
    let receipt = fixture.export("parser-linked-source.osre");
    let Parsed {
        mut manifest,
        mut payload,
        ..
    } = parse_independent(&receipt.path);
    let body = b"raw payload\0\xff".to_vec();
    manifest.entries.push(EvidenceEntry {
        path: "captures/note.bin".into(),
        kind: EvidenceEntryKind::File,
        byte_length: body.len() as u64,
        sha256: Some(sha(&body)),
    });
    payload.insert("captures/note.bin".into(), body);
    manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
    (manifest, payload)
}
fn assert_authenticated_container(bytes: &[u8]) {
    assert_eq!(&bytes[..8], HEADER);
    assert_eq!(&bytes[bytes.len() - 8..], END);
    let stop = bytes.len() - 40;
    assert_eq!(
        &bytes[stop..stop + 32],
        Sha256::digest(&bytes[..stop]).as_slice()
    );
}
fn refuses_inspect_and_restore(fixture: &Fixture, name: &str, bytes: &[u8]) {
    let path = fixture.archive(&format!("{name}.osre"));
    fs::write(&path, bytes).unwrap();
    assert!(
        inspect_evidence_archive(&path, &budget()).is_err(),
        "inspection accepted {name}"
    );
    let destination = fixture.root.join(format!("restored-{name}"));
    assert!(
        fixture
            .store
            .restore_retained_evidence(&path, &destination, &budget())
            .is_err(),
        "restore accepted {name}"
    );
    assert!(
        !destination.join("complete.json").exists(),
        "failed restore marked {name} complete"
    );
    assert_eq!(
        fs::read(path).unwrap(),
        bytes,
        "archive {name} was changed on refusal"
    );
}

#[test]
fn corrupt_truncated_trailing_and_rehashed_wrong_payload_archives_never_complete_restore() {
    let mut fixture = Fixture::new();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let (manifest, payload) = parser_fixture(&mut fixture);
    let valid = bundle(&manifest, &payload, None);
    let good = fixture.archive("handcrafted-valid.osre");
    fs::write(&good, &valid).unwrap();
    inspect_evidence_archive(&good, &budget()).unwrap();
    for (index, length) in [0, 8, 15, 16, valid.len() - 41, valid.len() - 1]
        .into_iter()
        .enumerate()
    {
        refuses_inspect_and_restore(&fixture, &format!("truncated-{index}"), &valid[..length]);
    }
    let mut corrupt = valid.clone();
    corrupt[0] ^= 1;
    refuses_inspect_and_restore(&fixture, "bad-magic", &corrupt);
    let mut corrupt = valid.clone();
    let footer = corrupt.len() - 40;
    corrupt[footer] ^= 1;
    refuses_inspect_and_restore(&fixture, "bad-footer-hash", &corrupt);
    let mut corrupt = valid.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    refuses_inspect_and_restore(&fixture, "bad-footer-tag", &corrupt);
    let mut corrupt = valid.clone();
    corrupt.push(0);
    refuses_inspect_and_restore(&fixture, "surplus-tail", &corrupt);
    let mut changed = payload.clone();
    changed.get_mut("captures/note.bin").unwrap()[0] ^= 1;
    let rehashed = bundle(&manifest, &changed, None);
    assert_authenticated_container(&rehashed);
    refuses_inspect_and_restore(&fixture, "payload-hash-mismatch", &rehashed);
    let mut bad_length = HEADER.to_vec();
    bad_length.extend_from_slice(&u64::MAX.to_le_bytes());
    refuses_inspect_and_restore(&fixture, "manifest-u64-overflow", &bad_length);
    assert_live_unchanged(&fixture, &before, &files);
}

#[test]
fn canonical_rehashed_metadata_forgeries_reach_strict_path_kind_parent_root_and_version_checks() {
    let mut fixture = Fixture::new();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let (base, payload) = parser_fixture(&mut fixture);
    for case in 0..28 {
        let mut manifest = base.clone();
        let mut raw = None;
        let file = manifest
            .entries
            .iter()
            .position(|e| e.path == "captures/note.bin")
            .unwrap();
        match case {
            0 => manifest.version = 2,
            1 => manifest.export_id = "0".repeat(32),
            2 => manifest.scope = "selected-song".into(),
            3 => manifest.catalog_schema = 7,
            4 => manifest.entries[file].path = "captures/../escape.bin".into(),
            5 => manifest.entries[file].path = "/absolute.bin".into(),
            6 => manifest.entries[file].path = "captures\\alias.bin".into(),
            7 => manifest.entries[file].path = "captures//empty.bin".into(),
            8 => manifest.entries[file].path = "captures/./dot.bin".into(),
            9 => manifest.entries[file].path = "captures/control\0name".into(),
            10 => manifest.entries[file].path = "captures/colon:name".into(),
            11 => {
                manifest.entries[file].path = format!("captures/{}", "x/".repeat(12)) + "deep.bin"
            }
            12 => manifest.entries[file].path = format!("captures/{}", "x".repeat(4096)),
            13 => manifest.entries.push(manifest.entries[file].clone()),
            14 => {
                let mut alias = manifest.entries[file].clone();
                alias.path = "captures/NOTE.bin".into();
                manifest.entries.push(alias);
                manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
            }
            15 => manifest.entries[file].path = "captures/absent-parent/child.bin".into(),
            16 => {
                manifest.entries[file].path = "captures/tree".into();
                manifest.entries.push(EvidenceEntry {
                    path: "captures/tree/child.bin".into(),
                    kind: EvidenceEntryKind::File,
                    byte_length: 0,
                    sha256: Some(sha(&[])),
                });
                manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
            }
            17 => manifest.roots[0].present = false,
            18 => manifest.roots.swap(0, 1),
            19 => manifest.entries.retain(|e| e.path != "catalog.sqlite3"),
            20 => {
                manifest.entries[0].sha256 = Some(sha(&[]));
                manifest.entries[0].byte_length = 1;
            }
            21 => manifest.entries[file].sha256 = None,
            22 => manifest.entries[file].byte_length = u64::MAX,
            23 => manifest.absences.push(EvidenceAbsence {
                path: "captures/note.bin".into(),
                reason: "contradicts present bytes".into(),
            }),
            24 => {
                let absence = EvidenceAbsence {
                    path: "takes/missing.bin".into(),
                    reason: "opaque absent original".into(),
                };
                manifest.absences = vec![absence.clone(), absence];
            }
            25 => {
                let canonical = String::from_utf8(serde_json::to_vec(&manifest).unwrap()).unwrap();
                raw = Some(
                    canonical
                        .replacen("\"kind\":\"file\"", "\"kind\":\"future_file\"", 1)
                        .into_bytes(),
                );
            }
            26 => raw = Some(serde_json::to_vec_pretty(&manifest).unwrap()),
            _ => manifest.origin_root = "relative-origin".into(),
        }
        let bytes = bundle(&manifest, &payload, raw);
        assert_authenticated_container(&bytes);
        refuses_inspect_and_restore(&fixture, &format!("metadata-{case}"), &bytes);
    }
    let mut over = base.clone();
    for index in 0..2048 {
        over.entries.push(EvidenceEntry {
            path: format!("captures/dir-{index:04}"),
            kind: EvidenceEntryKind::Directory,
            byte_length: 0,
            sha256: None,
        });
    }
    over.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let bytes = bundle(&over, &payload, None);
    assert_authenticated_container(&bytes);
    refuses_inspect_and_restore(&fixture, "too-many-entries", &bytes);
    let mut unknown = String::from_utf8(serde_json::to_vec(&base).unwrap()).unwrap();
    unknown.pop();
    unknown.push_str(",\"future_field\":true}");
    let bytes = bundle(&base, &payload, Some(unknown.into_bytes()));
    assert_authenticated_container(&bytes);
    refuses_inspect_and_restore(&fixture, "unknown-manifest-field", &bytes);
    assert_live_unchanged(&fixture, &before, &files);
}

#[test]
fn symlink_hardlink_non_utf8_colon_and_overdepth_inventory_are_refused_without_skipping_entries() {
    for case in 0..5 {
        let mut fixture = Fixture::new();
        let before = catalog(&fixture.store.connection);
        let files = retained_tree(fixture.store.root());
        let stage = fixture.original.parent().unwrap();
        let bad = match case {
            0 => {
                let path = stage.join("redirected.bin");
                std::os::unix::fs::symlink(&fixture.owned, &path).unwrap();
                path
            }
            1 => {
                let path = stage.join("alias.bin");
                fs::hard_link(&fixture.owned, &path).unwrap();
                assert_eq!(fs::metadata(&fixture.owned).unwrap().nlink(), 2);
                path
            }
            2 => {
                let path = stage.join(std::ffi::OsString::from_vec(vec![b'n', 0xff, b'x']));
                if let Err(error) = fs::write(&path, b"unnameable raw entry") {
                    // This host filesystem rejects invalid UTF-8 before any entry
                    // exists. That is filesystem evidence, not qualification
                    // of the raw inventory branch on another Unix filesystem.
                    assert!(
                        cfg!(target_os = "macos") && error.raw_os_error() == Some(92),
                        "unexpected fixture creation failure: {error}"
                    );
                    assert!(fs::symlink_metadata(&path).is_err());
                    assert_live_unchanged(&fixture, &before, &files);
                    continue;
                }
                path
            }
            3 => {
                let path = stage.join("colon:entry");
                fs::write(&path, b"must not disappear").unwrap();
                path
            }
            _ => {
                let path = stage.join((0..13).map(|_| "nested").collect::<Vec<_>>().join("/"));
                fs::create_dir_all(&path).unwrap();
                path
            }
        };
        let destination = fixture.archive(&format!("invalid-inventory-{case}.osre"));
        assert!(
            fixture
                .store
                .export_retained_evidence_full_root(&destination, EXPORT, &budget())
                .is_err(),
            "invalid member case {case}"
        );
        assert!(
            fs::symlink_metadata(&bad).is_ok(),
            "refusal deleted retained member"
        );
        if case == 4 {
            fs::remove_dir_all(stage.join("nested")).unwrap();
        } else {
            fs::remove_file(&bad).unwrap();
        }
        assert_live_unchanged(&fixture, &before, &files);
    }
}

#[test]
fn input_and_destination_ancestor_aliases_refuse_instead_of_redirecting_archive_or_restore() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("ordinary.osre");
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let alias = fixture.root.join("archive-alias.osre");
    std::os::unix::fs::symlink(&receipt.path, &alias).unwrap();
    assert!(inspect_evidence_archive(&alias, &budget()).is_err());
    let parent = fixture.root.join("redirected-parent");
    let actual = fixture.root.join("ordinary-parent");
    fs::create_dir(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &parent).unwrap();
    assert!(
        fixture
            .store
            .export_retained_evidence_full_root(parent.join("redirected.osre"), EXPORT, &budget())
            .is_err()
    );
    assert!(
        fixture
            .store
            .restore_retained_evidence(&receipt.path, parent.join("redirected-restore"), &budget())
            .is_err()
    );
    assert!(fs::read_dir(actual).unwrap().next().is_none());
    assert_live_unchanged(&fixture, &before, &files);
}

fn typed_stop(error: &StoreError, resource: Option<InspectionResource>) {
    match (error, resource) {
        (StoreError::Recording(RecordingError::InspectionCancelled), None) => (),
        (
            StoreError::Recording(RecordingError::InspectionLimit {
                resource: actual, ..
            }),
            Some(expected),
        ) => assert_eq!(*actual, expected),
        _ => panic!("expected typed sticky stop, got {error:?}"),
    }
}
#[test]
fn cancellation_and_custom_file_work_limits_remain_typed_sticky_without_completing_restore() {
    let mut fixture = Fixture::new();
    let complete = fixture.export("limits-source.osre");
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    for operation in 0..3 {
        let cancelled = Arc::new(AtomicBool::new(true));
        let governor =
            InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
        let target = fixture.root.join(format!("cancelled-{operation}"));
        let error = match operation {
            0 => fixture
                .store
                .export_retained_evidence_full_root(&target, EXPORT, &governor)
                .unwrap_err(),
            1 => inspect_evidence_archive(&complete.path, &governor).unwrap_err(),
            _ => fixture
                .store
                .restore_retained_evidence(&complete.path, &target, &governor)
                .unwrap_err(),
        };
        typed_stop(&error, None);
        cancelled.store(false, Ordering::Release);
        assert!(matches!(
            governor.checkpoint(),
            Err(RecordingError::InspectionCancelled)
        ));
        assert!(!target.join("complete.json").exists());
    }
    for file_limit in [false, true] {
        let limits = if file_limit {
            InspectionLimits {
                max_file_bytes: 1,
                ..InspectionLimits::default()
            }
        } else {
            InspectionLimits {
                max_work_bytes: 8,
                ..InspectionLimits::default()
            }
        };
        let resource = if file_limit {
            InspectionResource::FileBytes
        } else {
            InspectionResource::WorkBytes
        };
        let meter = governor(limits);
        let error = inspect_evidence_archive(&complete.path, &meter).unwrap_err();
        typed_stop(&error, Some(resource));
        assert!(matches!(
            meter.checkpoint(),
            Err(RecordingError::InspectionLimit { .. })
        ));
        let meter = governor(limits);
        let target = fixture.root.join(if file_limit {
            "file-limited-restore"
        } else {
            "work-limited-restore"
        });
        typed_stop(
            &fixture
                .store
                .restore_retained_evidence(&complete.path, &target, &meter)
                .unwrap_err(),
            Some(resource),
        );
        assert!(!target.join("complete.json").exists());
    }
    assert_live_unchanged(&fixture, &before, &files);
}
#[test]
fn metadata_heavy_catalog_refuses_small_work_budget_before_catalog_snapshot_staging() {
    let mut fixture = Fixture::new();
    let path = fixture
        .store
        .root()
        .join("captures")
        .join("66666666666666666666666666666666")
        .join("take.sqlite3");
    for revision in 0..16 {
        let value = serde_json::json!({"version":1,"journal_path":path,"descriptor_sha256":null,"opaque_revision":revision,"padding":"x".repeat(15_000)});
        fixture
            .store
            .save_performance_intent(
                "66666666666666666666666666666666",
                (revision > 0).then_some(revision),
                &serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
    }
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let names = fs::read_dir(&fixture.root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect::<BTreeSet<_>>();
    let governor = governor(InspectionLimits {
        max_work_bytes: 8 * 1024 * 1024,
        ..InspectionLimits::default()
    });
    let mut inventoried = false;
    let error = fixture
        .store
        .export_evidence_using(
            &fixture.archive("metadata-limited.osre"),
            EXPORT,
            &governor,
            |point| {
                if point == EvidenceExportBoundary::AfterInventory {
                    inventoried = true;
                }
                Ok(())
            },
        )
        .unwrap_err();
    assert!(
        inventoried,
        "the test must reach linkage metadata after ordinary raw inventory"
    );
    typed_stop(&error, Some(InspectionResource::WorkBytes));
    assert_eq!(
        fs::read_dir(&fixture.root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<BTreeSet<_>>(),
        names,
        "metadata reservation must fail before a catalog stage or destination is created"
    );
    assert_live_unchanged(&fixture, &before, &files);
}

#[test]
fn catalog_commit_file_set_change_or_ancestor_replacement_before_final_proof_withholds_footer() {
    for case in 0..4 {
        let mut fixture = Fixture::new();
        let before = catalog(&fixture.store.connection);
        let files = retained_tree(fixture.store.root());
        let destination = fixture.archive(&format!("race-{case}.osre"));
        let catalog_path = fixture.store.root().join("studio.sqlite3");
        let captures = fixture.store.root().join("captures");
        let parked = fixture.root.join("parked-original-captures");
        let parked_catalog = fixture.root.join("parked-studio.sqlite3");
        let late = fixture.original.parent().unwrap().join("late-member.bin");
        let mut changed = false;
        let result=fixture.store.export_evidence_using(&destination,EXPORT,&budget(),|point| {
            if point==EvidenceExportBoundary::BeforeFinalProof {
                match case {
                    0=>{let other=Connection::open(&catalog_path).unwrap();other.execute("INSERT INTO assets(id,payload) VALUES('concurrent-evidence-test',x'001122')",[]).unwrap();},
                    1=>fs::write(&late,b"late retained member").unwrap(),
                    2=>{fs::rename(&captures,&parked).unwrap();fs::create_dir(&captures).unwrap();},
                    _=>{let bytes=fs::read(&catalog_path).unwrap();fs::rename(&catalog_path,&parked_catalog).unwrap();fs::write(&catalog_path,bytes).unwrap();},
                }changed=true;
            }Ok(())
        });
        assert!(
            changed && result.is_err(),
            "race {case} must refuse after deterministic mutation"
        );
        assert!(
            !fs::read(&destination).unwrap().ends_with(END),
            "race {case} wrote completion footer"
        );
        if case == 0 {
            let mut expected = before.clone();
            let connection = Connection::open(&catalog_path).unwrap();
            expected
                .tables
                .insert("assets".into(), cells(&connection, "SELECT * FROM assets"));
            assert_eq!(catalog(&fixture.store.connection), expected);
            assert_eq!(retained_tree(fixture.store.root()), files);
        } else {
            if case == 1 {
                assert_eq!(fs::read(&late).unwrap(), b"late retained member");
                fs::remove_file(&late).unwrap();
            } else if case == 2 {
                assert!(fs::read_dir(&captures).unwrap().next().is_none());
                fs::remove_dir(&captures).unwrap();
                fs::rename(&parked, &captures).unwrap();
            } else {
                assert!(parked_catalog.exists());
                fs::remove_file(&catalog_path).unwrap();
                fs::rename(&parked_catalog, &catalog_path).unwrap();
            }
            assert_live_unchanged(&fixture, &before, &files);
        }
    }
}

#[test]
fn substituted_restore_directory_changed_object_or_surplus_member_never_receives_completion_marker()
{
    let mut fixture = Fixture::new();
    let receipt = fixture.export("output-proof.osre");
    let parsed = parse_independent(&receipt.path);
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    let index = parsed
        .manifest
        .entries
        .iter()
        .position(|e| e.kind == EvidenceEntryKind::File)
        .unwrap();
    for case in 0..4 {
        let destination = fixture.root.join(format!("substituted-restore-{case}"));
        let parked = fixture.root.join(format!("parked-restore-{case}"));
        let mut changed = false;
        let result = fixture.store.restore_evidence_using(
            &receipt.path,
            &destination,
            &budget(),
            |point, directory| {
                if point == EvidenceRestoreBoundary::BeforeCompletion {
                    let object = directory.join("objects").join(format!("{index:04}.bin"));
                    match case {
                        0 => {
                            fs::rename(directory, &parked).unwrap();
                            fs::create_dir(directory).unwrap();
                        }
                        1 => {
                            let mut bytes = fs::read(&object).unwrap();
                            if bytes.is_empty() {
                                bytes.push(1);
                            } else {
                                bytes[0] ^= 1;
                            }
                            fs::write(&object, bytes).unwrap();
                        }
                        2 => fs::write(directory.join("surplus.bin"), b"unlisted member").unwrap(),
                        _ => {
                            fs::rename(&object, &parked).unwrap();
                            std::os::unix::fs::symlink(&parked, &object).unwrap();
                        }
                    }
                    changed = true;
                }
                Ok(())
            },
        );
        assert!(
            changed && result.is_err(),
            "restore output mutation case {case} must refuse"
        );
        assert!(!destination.join("complete.json").exists());
        if case == 0 {
            assert!(!parked.join("complete.json").exists());
        }
        assert_live_unchanged(&fixture, &before, &files);
    }
}

fn catalog_closed_refusal(fixture: &mut Fixture, name: &str, bytes: &[u8]) -> [StoreError; 3] {
    let before = catalog(&fixture.store.connection);
    let native_bytes = tree(fixture.store.root());
    let path = fixture.archive(&format!("{name}.osre"));
    fs::write(&path, bytes).unwrap();
    assert_authenticated_container(bytes);
    // Check canonical JSON, every included File hash, lengths and both container
    // checksums independently. These cases must reach catalog linkage validation.
    parse_independent(&path);
    let inspected = inspect_evidence_archive(&path, &budget());
    assert!(
        inspected.is_err(),
        "inspection accepted catalog closure forgery {name}"
    );
    let replayed = fixture
        .store
        .export_retained_evidence_full_root(&path, EXPORT, &budget());
    assert!(
        replayed.is_err(),
        "historical replay accepted catalog closure forgery {name}"
    );
    let destination = fixture.root.join(format!("closure-restore-{name}"));
    let restored = fixture
        .store
        .restore_retained_evidence(&path, &destination, &budget());
    assert!(
        restored.is_err(),
        "restore accepted catalog closure forgery {name}"
    );
    assert!(!destination.join("complete.json").exists());
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(catalog(&fixture.store.connection), before);
    assert_eq!(
        tree(fixture.store.root()),
        native_bytes,
        "live files changed for {name}"
    );
    [
        inspected.unwrap_err(),
        replayed.unwrap_err(),
        restored.unwrap_err(),
    ]
}

#[test]
fn unchanged_catalog_rehashed_required_file_omissions_and_optional_absence_lies_refuse_every_read_path()
 {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("catalog-closure-source.osre");
    let parsed = parse_independent(&receipt.path);
    let catalog_bytes = parsed.payload["catalog.sqlite3"].clone();
    inspect_evidence_archive(&receipt.path, &budget()).unwrap();
    let required = [
        fixture
            .owned
            .strip_prefix(fixture.store.root())
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned(),
        format!("source-actions/{ACTION}/evidence/take.sqlite3"),
        fixture
            .working
            .strip_prefix(fixture.store.root())
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned(),
        fixture
            .original
            .strip_prefix(fixture.store.root())
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned(),
    ];
    for (index, path) in required.iter().enumerate() {
        for declare_absent in [false, true] {
            let mut manifest = parsed.manifest.clone();
            let mut payload = parsed.payload.clone();
            assert!(payload.remove(path).is_some());
            manifest.entries.retain(|entry| &entry.path != path);
            if declare_absent {
                manifest.absences.push(EvidenceAbsence {
                    path: path.clone(),
                    reason: "Started does not prove artifact creation or completion".into(),
                });
                manifest.absences.sort_by(|a, b| a.path.cmp(&b.path));
            }
            assert_eq!(payload["catalog.sqlite3"], catalog_bytes);
            let bytes = bundle(&manifest, &payload, None);
            catalog_closed_refusal(
                &mut fixture,
                &format!("required-{index}-{declare_absent}"),
                &bytes,
            );
        }
    }
    let optional_path = format!("captures/{MISSING}/take.sqlite3");
    let optional = parsed
        .manifest
        .absences
        .iter()
        .position(|entry| entry.path == optional_path)
        .unwrap();
    for case in 0..4 {
        let mut manifest = parsed.manifest.clone();
        match case {
            0 => {
                manifest.absences.remove(optional);
            }
            1 => manifest.absences[optional].reason = "Reviewed original is required".into(),
            2 => {
                manifest.absences[optional].path =
                    format!("captures/{}/take.sqlite3", "5".repeat(32))
            }
            _ => manifest.absences.push(EvidenceAbsence {
                path: format!("captures/{}/take.sqlite3", "5".repeat(32)),
                reason: "Unobserved recording request does not prove an original exists".into(),
            }),
        }
        manifest.absences.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(parsed.payload["catalog.sqlite3"], catalog_bytes);
        let bytes = bundle(&manifest, &parsed.payload, None);
        catalog_closed_refusal(&mut fixture, &format!("optional-absence-{case}"), &bytes);
    }
    // OSRE has checksums, not a signature or an external inventory commitment.
    // This tests closure against the unchanged catalog, not a coherent rewrite
    // of catalog plus inventory, nor detection of an omitted unreferenced file.
}

#[test]
fn catalog_schema_identity_link_table_class_and_vm_budget_are_checked_in_memory_on_every_read_path()
{
    let mut fixture = Fixture::new();
    let receipt = fixture.export("catalog-shape-source.osre");
    let parsed = parse_independent(&receipt.path);
    let before = catalog(&fixture.store.connection);
    let native_bytes = tree(fixture.store.root());
    for version in [0, 5, 7] {
        let mut manifest = parsed.manifest.clone();
        manifest.catalog_schema = version;
        let bytes = bundle(&manifest, &parsed.payload, None);
        catalog_closed_refusal(
            &mut fixture,
            &format!("declared-catalog-schema-{version}"),
            &bytes,
        );
    }
    for actual in [5u32, 7] {
        for declared in [6, actual] {
            let mut manifest = parsed.manifest.clone();
            manifest.catalog_schema = declared;
            let mut payload = parsed.payload.clone();
            let catalog = payload.get_mut("catalog.sqlite3").unwrap();
            assert_eq!(&catalog[..16], b"SQLite format 3\0");
            // SQLite's user_version is a big-endian u32 at header offset 60.
            catalog[60..64].copy_from_slice(&actual.to_be_bytes());
            let entry = manifest
                .entries
                .iter_mut()
                .find(|entry| entry.path == "catalog.sqlite3")
                .unwrap();
            entry.sha256 = Some(sha(catalog));
            let bytes = bundle(&manifest, &payload, None);
            catalog_closed_refusal(
                &mut fixture,
                &format!("actual-{actual}-declared-{declared}"),
                &bytes,
            );
        }
    }
    for table in [
        "recording_intents",
        "performance_source_reviews",
        "performance_source_actions",
        "native_takes",
    ] {
        let mut memory = Connection::open_in_memory().unwrap();
        let original = &parsed.payload["catalog.sqlite3"];
        memory
            .deserialize_read_exact(
                rusqlite::MAIN_DB,
                original.as_slice(),
                original.len(),
                false,
            )
            .unwrap();
        memory.execute_batch(&format!(
            "ALTER TABLE {table} RENAME TO opaque_original_{table}; CREATE VIEW {table} AS SELECT * FROM opaque_original_{table};"
        )).unwrap();
        let altered = memory.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
        assert_eq!(
            memory
                .query_row(
                    "SELECT type FROM sqlite_schema WHERE name=?1",
                    [table],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "view"
        );
        let mut manifest = parsed.manifest.clone();
        let mut payload = parsed.payload.clone();
        let entry = manifest
            .entries
            .iter_mut()
            .find(|entry| entry.path == "catalog.sqlite3")
            .unwrap();
        entry.byte_length = altered.len() as u64;
        entry.sha256 = Some(sha(&altered));
        payload.insert("catalog.sqlite3".into(), altered);
        let bytes = bundle(&manifest, &payload, None);
        catalog_closed_refusal(&mut fixture, &format!("link-table-as-view-{table}"), &bytes);
    }
    for operation in 0..3 {
        let meter = governor(InspectionLimits {
            max_vm_steps: 0,
            ..InspectionLimits::default()
        });
        let destination = fixture.root.join(format!("vm-refused-restore-{operation}"));
        let error = match operation {
            0 => inspect_evidence_archive(&receipt.path, &meter).unwrap_err(),
            1 => fixture
                .store
                .export_retained_evidence_full_root(&receipt.path, EXPORT, &meter)
                .unwrap_err(),
            _ => fixture
                .store
                .restore_retained_evidence(&receipt.path, &destination, &meter)
                .unwrap_err(),
        };
        typed_stop(&error, Some(InspectionResource::VmSteps));
        typed_stop(
            &StoreError::Recording(meter.checkpoint().unwrap_err()),
            Some(InspectionResource::VmSteps),
        );
        assert!(!destination.join("complete.json").exists());
        assert_eq!(catalog(&fixture.store.connection), before);
        assert_eq!(tree(fixture.store.root()), native_bytes);
    }
    assert_eq!(
        fs::read(&receipt.path).unwrap(),
        bundle(&parsed.manifest, &parsed.payload, None)
    );
}

#[test]
fn generated_known_catalog_columns_refuse_by_layout_before_payload_projection_on_every_read_path() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("generated-column-source.osre");
    let parsed = parse_independent(&receipt.path);
    for table in [
        "recording_intents",
        "performance_source_reviews",
        "performance_source_actions",
        "native_takes",
    ] {
        let generated = if table == "native_takes" {
            "object_sha256"
        } else {
            "payload"
        };
        for storage in ["VIRTUAL", "STORED"] {
            let mut memory = Connection::open_in_memory().unwrap();
            let original = &parsed.payload["catalog.sqlite3"];
            memory
                .deserialize_read_exact(
                    rusqlite::MAIN_DB,
                    original.as_slice(),
                    original.len(),
                    false,
                )
                .unwrap();
            let columns: Vec<(String, String)> = memory
                .prepare(&format!("PRAGMA table_xinfo({table})"))
                .unwrap()
                .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let definitions = columns
                .iter()
                .map(|(name, kind)| {
                    if name == generated {
                        // Both expressions start from at most 32 zero bytes. This proves
                        // the layout guard; it does not exercise a large C allocation.
                        let expression = if table == "native_takes" {
                            "lower(hex(zeroblob(32)))"
                        } else {
                            "zeroblob(8)"
                        };
                        format!("\"{name}\" {kind} GENERATED ALWAYS AS ({expression}) {storage}")
                    } else {
                        format!("\"{name}\" {kind}")
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            let ordinary = columns
                .iter()
                .filter(|(name, _)| name != generated)
                .map(|(name, _)| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(",");
            memory.execute_batch(&format!(
                "ALTER TABLE {table} RENAME TO opaque_before_generated_{table};
                 CREATE TABLE {table}({definitions});
                 INSERT INTO {table}({ordinary}) SELECT {ordinary} FROM opaque_before_generated_{table};"
            )).unwrap();
            let hidden: i64 = memory
                .query_row(
                    "SELECT hidden FROM pragma_table_xinfo(?1) WHERE name=?2",
                    params![table, generated],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(hidden, if storage == "VIRTUAL" { 2 } else { 3 });
            let altered = memory.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
            let mut manifest = parsed.manifest.clone();
            let mut payload = parsed.payload.clone();
            let catalog = manifest
                .entries
                .iter_mut()
                .find(|entry| entry.path == "catalog.sqlite3")
                .unwrap();
            catalog.byte_length = altered.len() as u64;
            catalog.sha256 = Some(sha(&altered));
            payload.insert("catalog.sqlite3".into(), altered);
            let bytes = bundle(&manifest, &payload, None);
            let errors = catalog_closed_refusal(
                &mut fixture,
                &format!("generated-{table}-{storage}"),
                &bytes,
            );
            for error in errors {
                assert!(
                    matches!(&error, StoreError::Invalid(message) if message == "Archived catalog linkage column layout is generated, hidden or unsupported"),
                    "generated {table}.{generated} {storage} was not rejected by the layout guard: {error:?}"
                );
            }
        }
    }
}
