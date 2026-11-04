//! Actual SQLite/owned-file tests for the dedicated store bridge. The ledger
//! rows here are deliberately OPAQUE store fixtures, not legal typed session
//! Prepared/Started/Verified histories, six-stream admission or native authority.
#![cfg(unix)]

use super::*;
use crate::{NativeStore, TakeObject, checksum};
use omniusong_core::{
    ProjectDocument,
    contracts::{
        EditOperation, EditScope, Extensions, OperationId, ProjectEditIntent, ProjectId, TrackId,
    },
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use rusqlite::{Connection, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    },
};

const ACTION: &str = "33333333333333333333333333333333";
const SECOND: &str = "55555555555555555555555555555555";
const REVIEW: &[u8] = b" {\"opaque_store_review\":true,\"unknown\":[1,2,3]}\n";
const PENDING: &[u8] = b"{\"opaque_store_phase\":\"pending\"}";
const COMMITTED: &[u8] = b"{\"opaque_store_phase\":\"committed\",\"revision\":1}";
const RETIRED: &[u8] = b"{\"opaque_store_phase\":\"retired\"}";

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    actions: PathBuf,
    store: NativeStore,
    publication: TakePublication,
    image_sha: String,
    image_bytes: u64,
}
impl Fixture {
    fn new(bad_tail: bool) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/source-attachment-store-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir(root.join("actions")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let source = root.join("source/take.sqlite3");
        let descriptor = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "synthetic".into(),
                source_id: "synthetic-pcm".into(),
                source_name: "Synthetic store test; no device".into(),
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
                "uninterpreted-original".into(),
                serde_json::json!({"text":"retain\u{0}unknown metadata","number":42}),
            )]),
        };
        let mut journal = Journal::create(&source, &descriptor).unwrap();
        for sequence in 0..3 {
            let stamp = HostTimestamp {
                domain: ClockDomainId("clock".into()),
                epoch: ClockEpochId("epoch".into()),
                ticks: sequence * 2,
            };
            journal
                .append(
                    &ChunkHeader {
                        version: CHUNK_VERSION,
                        take_id: descriptor.take_id.clone(),
                        stream_id: "audio".into(),
                        segment_id: "synthetic".into(),
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
        if bad_tail {
            mutate_chunk(
                &source,
                "UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=3",
            );
        }
        let original = fs::read(&source).unwrap();
        let actions = root.join("actions");
        let owner = CopyActionOwner::create(&actions, ACTION).unwrap();
        let budget = budget();
        let reader = CaptureReader::open_bounded(&source, &budget).unwrap();
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.recording-index.v1\0");
        hash.update(reader.inspection().descriptor_sha256.as_bytes());
        for ordinal in 1u64..=2 {
            hash.update(ordinal.to_le_bytes());
            hash.update(reader.read_ordinal(ordinal).unwrap().sha256.as_bytes());
        }
        let selection = RecoveryPrefix {
            chunk_count: 2,
            chunk_index_sha256: format!("{:x}", hash.finalize()),
        };
        let recovered = RecoveryCopy::create(reader, &actions, ACTION)
            .unwrap()
            .finalize(&selection, "synthetic-store-recovery")
            .unwrap();
        drop(owner);
        let mut store = NativeStore::open(root.join("store")).unwrap();
        let project = store
            .create(&ProjectDocument::new("Target before attachment").unwrap())
            .unwrap();
        store
            .save_performance_source_action(ACTION, None, REVIEW, PENDING)
            .unwrap();
        let publication = TakePublication {
            operation_id: OperationId(format!(
                "{SOURCE_ATTACHMENT_OPERATION_PREFIX}{}",
                "a".repeat(64)
            )),
            project_id: ProjectId(project.id().into()),
            expected_revision: 0,
            track_id: TrackId("recovered-track".into()),
            track_name: "Recovered performance".into(),
            clip: TakeClip {
                id: "recovered-clip".into(),
                source: TakeReference {
                    take_id: descriptor.take_id,
                    manifest_sha256: recovered.manifest.sha256,
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames { start: 0, count: 4 },
                position: TimelinePosition {
                    frame: 96,
                    sample_rate: 48_000,
                },
                fades: None,
                edit_source: None,
            },
        };
        Self {
            root,
            source,
            actions,
            store,
            publication,
            image_sha: checksum(&original),
            image_bytes: original.len() as u64,
        }
    }
    fn owner(&self) -> CopyActionOwner {
        CopyActionOwner::open_existing(&self.actions, ACTION).unwrap()
    }
    fn view<'a>(
        &self,
        owner: &'a CopyActionOwner,
        budget: &InspectionBudget,
    ) -> RecoveredCopyView<'a> {
        RecoveredCopyView::open(owner, &self.image_sha, self.image_bytes, budget).unwrap()
    }
    fn ingest(&mut self) -> TakeObject {
        let owner = self.owner();
        let governor = budget();
        let view = self.view(&owner, &governor);
        self.store.ingest_recovered_copy(&view).unwrap()
    }
    fn history(&self) -> PerformanceSourceActionHistory {
        self.store.list_performance_source_actions().unwrap()
    }
    fn publish(
        &mut self,
        history: &PerformanceSourceActionHistory,
        governor: &InspectionBudget,
    ) -> crate::Result<(crate::EditReceipt, StoredPerformanceSourceAction)> {
        publish(&mut self.store, &self.publication, history, governor)
    }
    fn originals(&self) -> BTreeMap<PathBuf, FileIdentity> {
        let mut result = files(&self.root.join("source"));
        result.extend(
            files(&self.actions)
                .into_iter()
                .map(|(path, value)| (Path::new("actions").join(path), value)),
        );
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Source attachment store fixture retained at {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn budget() -> InspectionBudget {
    InspectionBudget::new(
        InspectionLimits::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
fn publish(
    store: &mut NativeStore,
    publication: &TakePublication,
    history: &PerformanceSourceActionHistory,
    governor: &InspectionBudget,
) -> crate::Result<(crate::EditReceipt, StoredPerformanceSourceAction)> {
    store.publish_source_attachment(SourceAttachmentCommit {
        publication,
        id: ACTION,
        expected_revision: 1,
        review: REVIEW,
        committed_payload: COMMITTED,
        expected_history: history,
        budget: governor,
    })
}
fn retire(
    store: &mut NativeStore,
    publication: &TakePublication,
    history: &PerformanceSourceActionHistory,
) -> crate::Result<StoredPerformanceSourceAction> {
    store.save_performance_source_action_if_history_matches_and_unpublished(
        ACTION,
        Some(1),
        REVIEW,
        RETIRED,
        history,
        publication,
    )
}
fn rename(project: &ProjectDocument, operation: &str) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(operation.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        scope: EditScope::Project,
        edit: EditOperation::Rename {
            name: "Later musician edit".into(),
        },
        extensions: Extensions::new(),
    }
}
fn mutate_chunk(path: &Path, sql: &str) {
    let connection = Connection::open(path).unwrap();
    let triggers: Vec<(String, String)> = connection.prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND tbl_name='chunks' ORDER BY name").unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    for (name, _) in &triggers {
        connection
            .execute_batch(&format!("DROP TRIGGER \"{name}\";"))
            .unwrap();
    }
    connection.execute_batch(sql).unwrap();
    for (_, sql) in triggers {
        connection.execute_batch(&sql).unwrap();
    }
}
#[derive(Debug, PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    inode: u64,
    mode: u32,
    bytes: Vec<u8>,
}
fn files(root: &Path) -> BTreeMap<PathBuf, FileIdentity> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, FileIdentity>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(root, &path, result);
            } else {
                assert!(!metadata.file_type().is_symlink());
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    FileIdentity {
                        dev: metadata.dev(),
                        inode: metadata.ino(),
                        mode: metadata.mode(),
                        bytes: fs::read(path).unwrap(),
                    },
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
#[derive(Debug, PartialEq, Eq)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
fn rows(connection: &Connection, sql: &str) -> Vec<Vec<Cell>> {
    let mut statement = connection.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| {
                    Ok(match row.get_ref(column)? {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(n) => Cell::Integer(n),
                        ValueRef::Real(n) => Cell::Real(n.to_bits()),
                        ValueRef::Text(bytes) => Cell::Text(bytes.to_vec()),
                        ValueRef::Blob(bytes) => Cell::Blob(bytes.to_vec()),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn snapshot(connection: &Connection) -> BTreeMap<String, Vec<Vec<Cell>>> {
    let names: Vec<String> = connection.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([], |row| row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let values = rows(
                connection,
                &format!(
                    "SELECT * FROM \"{}\" ORDER BY rowid",
                    name.replace('"', "\"\"")
                ),
            );
            (name, values)
        })
        .collect()
}

#[test]
fn source_attachment_commits_project_history_operation_and_opaque_source_row_together() {
    let mut f = Fixture::new(true);
    let object = f.ingest();
    let before = f.store.load(&f.publication.project_id.0).unwrap();
    let originals = f.originals();
    let history = f.history();
    let governor = budget();
    let (receipt, source_row) = f.publish(&history, &governor).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.previous_revision, 0);
    assert_eq!(receipt.document.revision(), 1);
    assert_eq!(receipt.operation_id, f.publication.operation_id.0);
    assert_eq!(
        receipt.document.take_clips().unwrap(),
        vec![f.publication.clip.clone()]
    );
    assert_eq!(source_row.revision, 2);
    assert_eq!(source_row.payload, COMMITTED);
    assert_eq!(source_row.payload_sha256, checksum(COMMITTED));
    assert_eq!(source_row.review_blob_sha256, checksum(REVIEW));
    assert_eq!(f.store.load(before.id()).unwrap(), receipt.document);
    let (old, next): (String, String) = f
        .store
        .connection
        .query_row(
            "SELECT before_json,after_json FROM edits WHERE project_id=?1 AND sequence=1",
            [before.id()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(ProjectDocument::from_json(old.as_bytes()).unwrap(), before);
    assert_eq!(
        ProjectDocument::from_json(next.as_bytes()).unwrap(),
        receipt.document
    );
    let proof = f
        .store
        .take_publication_receipt(&f.publication)
        .unwrap()
        .unwrap();
    assert!(proof.replayed);
    assert_eq!(proof.document, receipt.document);
    assert_eq!(proof.operation_id, receipt.operation_id);
    assert_eq!(f.history().revisions.last().unwrap(), &source_row);
    assert_eq!(f.history().reviews[0].payload, REVIEW);
    assert_eq!(f.originals(), originals);
    assert_eq!(f.store.registered_take(&object.take_id).unwrap(), object);
    let exact = snapshot(&f.store.connection);
    let current_history = f.history();
    assert!(f.publish(&current_history, &budget()).is_err());
    assert_eq!(snapshot(&f.store.connection), exact);
}

#[test]
fn source_attachment_stale_history_target_or_action_revision_commits_nothing() {
    for kind in 0..3 {
        let mut f = Fixture::new(false);
        f.ingest();
        let history = f.history();
        if kind == 0 {
            f.store
                .save_performance_source_action(SECOND, None, REVIEW, PENDING)
                .unwrap();
        }
        if kind == 1 {
            let current = f.store.load(&f.publication.project_id.0).unwrap();
            f.store.apply_edit(&rename(&current, "later-edit")).unwrap();
        }
        let before = snapshot(&f.store.connection);
        let originals = f.originals();
        let governor = budget();
        let result = if kind == 2 {
            f.store.publish_source_attachment(SourceAttachmentCommit {
                publication: &f.publication,
                id: ACTION,
                expected_revision: 9,
                review: REVIEW,
                committed_payload: COMMITTED,
                expected_history: &history,
                budget: &governor,
            })
        } else {
            f.publish(&history, &governor)
        };
        assert!(
            matches!(result, Err(StoreError::Conflict)),
            "case {kind}: {result:?}"
        );
        assert_eq!(snapshot(&f.store.connection), before);
        assert_eq!(f.originals(), originals);
        assert!(f.store.connection.is_autocommit());
    }
}

#[test]
fn source_attachment_sql_errors_at_every_mutation_boundary_roll_back_all_rows() {
    for trigger in [
        "CREATE TRIGGER fail_boundary BEFORE INSERT ON edits BEGIN SELECT RAISE(ABORT,'synthetic edit failure'); END;",
        "CREATE TRIGGER fail_boundary BEFORE UPDATE ON projects BEGIN SELECT RAISE(ABORT,'synthetic project failure'); END;",
        "CREATE TRIGGER fail_boundary BEFORE INSERT ON operations BEGIN SELECT RAISE(ABORT,'synthetic operation failure'); END;",
        "CREATE TRIGGER fail_boundary BEFORE INSERT ON performance_source_actions WHEN NEW.revision=2 BEGIN SELECT RAISE(ABORT,'synthetic source outcome failure'); END;",
    ] {
        let mut f = Fixture::new(true);
        f.ingest();
        f.store.connection.execute_batch(trigger).unwrap();
        let history = f.history();
        let before = snapshot(&f.store.connection);
        let originals = f.originals();
        assert!(f.publish(&history, &budget()).is_err(), "{trigger}");
        assert_eq!(snapshot(&f.store.connection), before, "{trigger}");
        assert_eq!(f.originals(), originals);
        assert!(f.store.connection.is_autocommit());
    }
}

#[test]
fn source_outcome_trigger_cannot_silently_rewrite_project_history_or_operation_postimages() {
    for corruption in [
        "UPDATE projects SET payload=(SELECT before_json FROM edits WHERE sequence=1),revision=0;",
        "UPDATE projects SET cursor=0;",
        "UPDATE edits SET before_json=after_json;",
        "UPDATE edits SET after_json=before_json;",
        "DELETE FROM edits;",
        "UPDATE operations SET result_json='{}';",
        "DELETE FROM operations;",
    ] {
        let mut f = Fixture::new(false);
        f.ingest();
        f.store.connection.execute_batch(&format!("CREATE TRIGGER corrupt_after_source AFTER INSERT ON performance_source_actions WHEN NEW.revision=2 BEGIN {corruption} END;")).unwrap();
        let history = f.history();
        let before = snapshot(&f.store.connection);
        let originals = f.originals();
        assert!(
            f.publish(&history, &budget()).is_err(),
            "accepted corruption: {corruption}"
        );
        assert_eq!(snapshot(&f.store.connection), before, "{corruption}");
        assert_eq!(f.originals(), originals);
        assert!(f.store.connection.is_autocommit());
    }
}

#[test]
fn source_outcome_trigger_cannot_rewrite_its_own_retained_bytes() {
    let mut f = Fixture::new(true);
    f.ingest();
    f.store.connection.execute_batch("DROP TRIGGER immutable_performance_source_action_update;
        CREATE TRIGGER corrupt_source AFTER INSERT ON performance_source_actions WHEN NEW.revision=2 BEGIN UPDATE performance_source_actions SET payload=X'7b7d' WHERE id=NEW.id AND revision=1; END;").unwrap();
    let history = f.history();
    let before = snapshot(&f.store.connection);
    let originals = f.originals();
    assert!(f.publish(&history, &budget()).is_err());
    assert_eq!(snapshot(&f.store.connection), before);
    assert_eq!(f.originals(), originals);
}

#[test]
fn retirement_fences_stale_publisher_and_confirmed_publication_cannot_be_retired() {
    for publication_first in [false, true] {
        let mut f = Fixture::new(false);
        f.ingest();
        let stale = f.history();
        let originals = f.originals();
        let mut other = NativeStore::open(f.root.join("store")).unwrap();
        if publication_first {
            f.publish(&stale, &budget()).unwrap();
            let current = f.history();
            let before = snapshot(&f.store.connection);
            assert!(retire(&mut other, &f.publication, &current).is_err());
            assert_eq!(snapshot(&f.store.connection), before);
        } else {
            let retired = retire(&mut other, &f.publication, &stale).unwrap();
            assert_eq!(retired.revision, 2);
            assert_eq!(retired.payload, RETIRED);
            let before = snapshot(&f.store.connection);
            assert!(matches!(
                f.publish(&stale, &budget()),
                Err(StoreError::Conflict)
            ));
            assert_eq!(snapshot(&f.store.connection), before);
            assert!(
                f.store
                    .take_publication_receipt(&f.publication)
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(f.originals(), originals);
    }
}

#[test]
fn concurrent_retirement_and_publication_have_exactly_one_atomic_winner() {
    let mut f = Fixture::new(false);
    f.ingest();
    let history = f.history();
    let originals = f.originals();
    let mut publisher = NativeStore::open(f.root.join("store")).unwrap();
    let mut retirer = NativeStore::open(f.root.join("store")).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let gate = Arc::clone(&barrier);
    let publication = f.publication.clone();
    let expected = history.clone();
    let publication_thread = std::thread::spawn(move || {
        gate.wait();
        publish(&mut publisher, &publication, &expected, &budget())
            .map(|_| ())
            .map_err(|error| matches!(error, StoreError::Conflict))
    });
    let publication = f.publication.clone();
    let retirement_thread = std::thread::spawn(move || {
        barrier.wait();
        retire(&mut retirer, &publication, &history)
            .map(|_| ())
            .map_err(|error| matches!(error, StoreError::Conflict))
    });
    let published = publication_thread.join().unwrap();
    let retired = retirement_thread.join().unwrap();
    assert!(
        matches!(
            (&published, &retired),
            (Ok(()), Err(true)) | (Err(true), Ok(()))
        ),
        "publication={published:?}, retirement={retired:?}"
    );
    let history = f.history();
    assert_eq!(history.revisions.len(), 2);
    assert_eq!(history.reviews.len(), 1);
    let proof = f.store.take_publication_receipt(&f.publication).unwrap();
    assert_eq!(proof.is_some(), published.is_ok());
    assert_eq!(
        f.store
            .load(&f.publication.project_id.0)
            .unwrap()
            .revision(),
        u64::from(published.is_ok())
    );
    assert_eq!(
        history.revisions[1].payload,
        if published.is_ok() {
            COMMITTED
        } else {
            RETIRED
        }
    );
    assert_eq!(f.originals(), originals);
}

#[test]
fn reserved_source_operations_cannot_bypass_bridge_through_generic_entrypoints() {
    let mut f = Fixture::new(false);
    f.ingest();
    let originals = f.originals();
    let before = snapshot(&f.store.connection);
    assert!(
        f.store
            .attach_capture(f.root.join("missing-input.sqlite3"), &f.publication)
            .is_err()
    );
    assert!(f.store.publish_take(&f.publication).is_err());
    let document = f.store.load(&f.publication.project_id.0).unwrap();
    assert!(
        f.store
            .apply_edit(&rename(&document, &f.publication.operation_id.0))
            .is_err()
    );
    assert_eq!(snapshot(&f.store.connection), before);
    assert_eq!(f.originals(), originals);
}

#[test]
fn reserved_inbound_identity_restore_refuses_all_catalog_rows_and_preserves_archive() {
    let mut f = Fixture::new(true);
    f.ingest();
    let history = f.history();
    f.publish(&history, &budget()).unwrap();
    let archive = f.root.join("with-source-operation.sqlite3");
    f.store
        .export_archive(&f.publication.project_id.0, &archive)
        .unwrap();
    let bytes = fs::read(&archive).unwrap();
    let originals = f.originals();
    let mut destination = NativeStore::open(f.root.join("restore-destination")).unwrap();
    let before = snapshot(&destination.connection);
    let error = destination
        .restore_archive(&archive)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Source attachment operation requires"),
        "{error}"
    );
    assert_eq!(snapshot(&destination.connection), before);
    assert_eq!(fs::read(&archive).unwrap(), bytes);
    assert_eq!(f.originals(), originals);
    // A refusal may retain staged/published owned artifacts; it must not claim
    // rollback of filesystem work or turn this into evidence-archive acceptance.
}

#[test]
fn historical_receipt_is_metadata_only_and_does_not_restore_old_document() {
    let mut f = Fixture::new(false);
    let object = f.ingest();
    let history = f.history();
    let (committed, _) = f.publish(&history, &budget()).unwrap();
    let latest = f
        .store
        .apply_edit(&rename(&committed.document, "after-attachment"))
        .unwrap()
        .document;
    fs::remove_dir_all(&f.actions).unwrap();
    fs::remove_file(&f.source).unwrap();
    fs::remove_file(
        f.root
            .join("store/takes")
            .join(format!("{}.sqlite3", object.object_sha256)),
    )
    .unwrap();
    let current_history = f.history();
    let before = snapshot(&f.store.connection);
    let receipt = f
        .store
        .take_publication_receipt_if_source_history_matches(&f.publication, &current_history)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.document, committed.document);
    assert!(receipt.replayed);
    assert!(matches!(
        f.store
            .take_publication_receipt_if_source_history_matches(&f.publication, &history),
        Err(StoreError::Conflict)
    ));
    let stored: String = f
        .store
        .connection
        .query_row(
            "SELECT payload FROM projects WHERE id=?1",
            [latest.id()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        ProjectDocument::from_json(stored.as_bytes()).unwrap(),
        latest
    );
    assert_eq!(snapshot(&f.store.connection), before);
}

#[test]
fn fenced_metadata_absence_requires_the_same_complete_retained_history() {
    let mut f = Fixture::new(false);
    let history = f.history();
    let before = snapshot(&f.store.connection);
    assert!(
        f.store
            .take_publication_receipt_if_source_history_matches(&f.publication, &history)
            .unwrap()
            .is_none()
    );
    assert_eq!(snapshot(&f.store.connection), before);
    f.store
        .save_performance_source_action(SECOND, None, REVIEW, PENDING)
        .unwrap();
    let changed = snapshot(&f.store.connection);
    assert!(matches!(
        f.store
            .take_publication_receipt_if_source_history_matches(&f.publication, &history),
        Err(StoreError::Conflict)
    ));
    assert_eq!(snapshot(&f.store.connection), changed);
}

#[test]
fn bounded_ingestion_uses_held_working_owner_and_retains_exact_raw_original_bytes() {
    let mut f = Fixture::new(true);
    let owner = f.owner();
    let governor = budget();
    let view = f.view(&owner, &governor);
    assert!(matches!(
        CaptureReader::open_bounded(&owner.paths().working, &budget()),
        Err(RecordingError::Owned)
    ));
    let originals = f.originals();
    let raw = fs::read(&owner.paths().working).unwrap();
    let usage = governor.usage().work_bytes;
    let object = f.store.ingest_recovered_copy(&view).unwrap();
    assert_eq!(object.object_sha256, checksum(&raw));
    assert_eq!(object.byte_length, raw.len() as u64);
    assert_eq!(
        object.manifest,
        *view
            .working_reader()
            .inspection()
            .manifest
            .as_ref()
            .unwrap()
    );
    assert_eq!(
        fs::read(
            f.root
                .join("store/takes")
                .join(format!("{}.sqlite3", object.object_sha256))
        )
        .unwrap(),
        raw
    );
    assert!(governor.usage().work_bytes > usage);
    assert_eq!(f.originals(), originals);
    let owned_before = files(&f.root.join("store/takes"));
    let catalog_before = snapshot(&f.store.connection);
    assert_eq!(f.store.ingest_recovered_copy(&view).unwrap(), object);
    assert_eq!(files(&f.root.join("store/takes")), owned_before);
    assert_eq!(snapshot(&f.store.connection), catalog_before);
    assert_eq!(f.originals(), originals);
    assert!(matches!(
        CaptureReader::open_bounded(&owner.paths().working, &budget()),
        Err(RecordingError::Owned)
    ));
    let owned = f
        .store
        .open_take_with_budget(&f.publication.clip.source, &governor)
        .unwrap();
    assert_eq!(
        owned.inspect_snapshot().unwrap().original_rows_sha256,
        view.working_reader().inspection().original_rows_sha256
    );
}

#[test]
fn dedup_rejects_same_manifest_with_different_corrupt_or_unselected_original_rows() {
    for extra_row in [false, true] {
        let mut f = Fixture::new(true);
        let registered = f.ingest();
        let first_owner = f.owner();
        let first_budget = budget();
        let first_view = f.view(&first_owner, &first_budget);
        let original_digest = first_view
            .working_reader()
            .inspection()
            .original_rows_sha256
            .clone();
        drop(first_view);
        let other_owner = CopyActionOwner::create(&f.actions, SECOND).unwrap();
        for parent in [
            other_owner.paths().evidence.parent().unwrap(),
            other_owner.paths().working.parent().unwrap(),
        ] {
            fs::create_dir_all(parent).unwrap();
        }
        fs::copy(&first_owner.paths().evidence, &other_owner.paths().evidence).unwrap();
        fs::copy(&first_owner.paths().working, &other_owner.paths().working).unwrap();
        let mut owner_path = other_owner.paths().working.as_os_str().to_owned();
        owner_path.push(".owner.lock");
        fs::write(PathBuf::from(owner_path), []).unwrap();
        let change = if extra_row {
            "INSERT INTO chunks(ordinal,stream_id,sequence,header,payload,sha256) SELECT 4,stream_id,3,header,payload,sha256 FROM chunks WHERE ordinal=3"
        } else {
            "UPDATE chunks SET payload=X'0102030405060708' WHERE ordinal=3"
        };
        mutate_chunk(&other_owner.paths().evidence, change);
        mutate_chunk(&other_owner.paths().working, change);
        let image = fs::read(&other_owner.paths().evidence).unwrap();
        let governor = budget();
        let view = RecoveredCopyView::open(
            &other_owner,
            &checksum(&image),
            image.len() as u64,
            &governor,
        )
        .unwrap();
        assert_eq!(
            view.working_reader()
                .inspection()
                .manifest
                .as_ref()
                .unwrap(),
            &registered.manifest
        );
        assert_ne!(
            view.working_reader().inspection().original_rows_sha256,
            original_digest
        );
        let before = snapshot(&f.store.connection);
        let originals = f.originals();
        let owned = files(&f.root.join("store/takes"));
        assert!(f.store.ingest_recovered_copy(&view).is_err());
        assert_eq!(snapshot(&f.store.connection), before);
        assert_eq!(f.originals(), originals);
        assert_eq!(files(&f.root.join("store/takes")), owned);
    }
}

#[test]
fn damaged_registered_object_never_falls_back_to_available_working_copy() {
    let mut f = Fixture::new(false);
    let object = f.ingest();
    let path = f
        .root
        .join("store/takes")
        .join(format!("{}.sqlite3", object.object_sha256));
    let mut raw = fs::read(&path).unwrap();
    raw.push(0x5a);
    fs::write(&path, &raw).unwrap();
    let owner = f.owner();
    let governor = budget();
    let view = f.view(&owner, &governor);
    let before = snapshot(&f.store.connection);
    let originals = f.originals();
    let owned = files(&f.root.join("store/takes"));
    assert!(f.store.ingest_recovered_copy(&view).is_err());
    assert!(
        f.store
            .open_take_with_budget(&f.publication.clip.source, &governor)
            .is_err()
    );
    assert_eq!(snapshot(&f.store.connection), before);
    assert_eq!(f.originals(), originals);
    assert_eq!(files(&f.root.join("store/takes")), owned);
}

#[test]
fn bounded_ingestion_cancellation_and_publication_limit_preserve_sql_and_originals() {
    let mut f = Fixture::new(false);
    let owner = f.owner();
    let cancelled = Arc::new(AtomicBool::new(false));
    let governor =
        InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
    let view = f.view(&owner, &governor);
    let before = snapshot(&f.store.connection);
    let originals = f.originals();
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        f.store.ingest_recovered_copy(&view),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    assert_eq!(snapshot(&f.store.connection), before);
    assert_eq!(f.originals(), originals);
    drop(view);
    drop(owner);
    f.ingest();
    let history = f.history();
    let before = snapshot(&f.store.connection);
    let originals = f.originals();
    let tiny = InspectionBudget::new(
        InspectionLimits {
            max_work_bytes: 1,
            ..Default::default()
        },
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(matches!(
        f.publish(&history, &tiny),
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        }))
    ));
    assert_eq!(snapshot(&f.store.connection), before);
    assert_eq!(f.originals(), originals);
}

fn attached_archive(f: &mut Fixture, name: &str) -> (PathBuf, String, ProjectDocument) {
    f.ingest();
    let history = f.history();
    let (committed, _) = f.publish(&history, &budget()).unwrap();
    let path = f.root.join(name);
    let archive = f
        .store
        .export_archive(committed.document.id(), &path)
        .unwrap();
    (path, archive.sha256, committed.document)
}

fn namespace(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, path: &Path, output: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            output.push(path.strip_prefix(root).unwrap().to_owned());
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                visit(root, &path, output);
            }
        }
    }
    let mut output = Vec::new();
    visit(root, root, &mut output);
    output.sort();
    output
}

#[test]
fn new_reserved_archive_copy_operation_refuses_before_staging_or_catalog_work() {
    let mut f = Fixture::new(true);
    let (archive, sha, _) = attached_archive(&mut f, "reserved-new-copy.sqlite3");
    let archive_bytes = fs::read(&archive).unwrap();
    let originals = f.originals();
    let root = f.root.join("reserved-copy-destination");
    let mut destination = NativeStore::open(&root).unwrap();
    assert_eq!(destination.inspect_archive(&archive).unwrap().sha256, sha);
    let before = snapshot(&destination.connection);
    let before_files = files(&root);
    let before_names = namespace(&root);
    let request = crate::RestoreCopyIntent {
        operation_id: OperationId(format!(
            "{SOURCE_ATTACHMENT_OPERATION_PREFIX}{}",
            "b".repeat(64)
        )),
        expected_archive_sha256: sha,
    };
    let error = destination
        .restore_archive_as_copy(&archive, &request)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Source attachment operation requires"),
        "{error}"
    );
    assert_eq!(snapshot(&destination.connection), before);
    assert_eq!(namespace(&root), before_names);
    assert_eq!(files(&root), before_files);
    assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
    assert_eq!(f.originals(), originals);
}

#[test]
fn source_attached_archive_restores_as_copy_with_normal_remapped_ids_and_exact_replay() {
    let mut f = Fixture::new(true);
    let (archive, sha, source) = attached_archive(&mut f, "source-attached-copy.sqlite3");
    let archive_bytes = fs::read(&archive).unwrap();
    let originals = f.originals();
    let mut destination = NativeStore::open(f.root.join("normal-copy-destination")).unwrap();
    let request = crate::RestoreCopyIntent {
        operation_id: OperationId("normal-source-attachment-copy".into()),
        expected_archive_sha256: sha.clone(),
    };
    let receipt = destination
        .restore_archive_as_copy(&archive, &request)
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.source_project_id, source.id());
    assert_eq!(receipt.source_archive_sha256, sha);
    assert_ne!(receipt.document.id(), source.id());
    let mut expected = source.value().clone();
    expected["id"] = receipt.document.id().into();
    assert_eq!(receipt.document.value(), &expected);
    assert_eq!(
        receipt.document.take_clips().unwrap(),
        source.take_clips().unwrap()
    );
    let (operation, json): (String, String) = destination
        .connection
        .query_row(
            "SELECT operation_id,request_json FROM operations WHERE project_id=?1",
            [receipt.document.id()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(!operation.starts_with(SOURCE_ATTACHMENT_OPERATION_PREFIX));
    assert_ne!(operation, f.publication.operation_id.0);
    let envelope: serde_json::Value = serde_json::from_str(&json).unwrap();
    let mapped: TakePublication = serde_json::from_value(envelope["intent"].clone()).unwrap();
    let mut expected_publication = f.publication.clone();
    expected_publication.operation_id.0 = operation;
    expected_publication.project_id.0 = receipt.document.id().into();
    assert_eq!(mapped, expected_publication);
    assert_eq!(
        destination
            .take_publication_receipt(&mapped)
            .unwrap()
            .unwrap()
            .document,
        receipt.document
    );
    let proof_source: String = destination
        .connection
        .query_row(
            "SELECT source_operation_id FROM archive_copy_proof_operations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(proof_source, f.publication.operation_id.0);
    let source_object = f
        .store
        .registered_take(&f.publication.clip.source.take_id)
        .unwrap();
    assert_eq!(
        destination.registered_take(&source_object.take_id).unwrap(),
        source_object
    );
    let owned_name = format!("{}.sqlite3", source_object.object_sha256);
    assert_eq!(
        fs::read(
            f.root
                .join("normal-copy-destination/takes")
                .join(&owned_name)
        )
        .unwrap(),
        fs::read(f.root.join("store/takes").join(owned_name)).unwrap()
    );
    assert!(
        destination
            .list_performance_source_actions()
            .unwrap()
            .revisions
            .is_empty()
    );
    let latest = destination
        .apply_edit(&rename(&receipt.document, "after-normal-copy"))
        .unwrap()
        .document;
    let before = snapshot(&destination.connection);
    let replay = destination
        .restore_archive_as_copy(&f.root.join("missing-acquisition.sqlite3"), &request)
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, receipt.document);
    assert_eq!(destination.load(latest.id()).unwrap(), latest);
    assert_eq!(snapshot(&destination.connection), before);
    assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
    assert_eq!(f.originals(), originals);
}

#[test]
fn valid_checksummed_adversarial_reserved_copy_is_preserved_outbound_but_not_admitted_inbound() {
    // Construct a valid-checksummed adversarial fixture from a real zero-map
    // copy. This is not evidence that a reserved copy was historically admitted
    // by the production API, nor a bypass exposed to application callers.
    let f = Fixture::new(false);
    let initial = f.root.join("plain-project.sqlite3");
    let initial_receipt = f
        .store
        .export_archive(&f.publication.project_id.0, &initial)
        .unwrap();
    let mut historical = NativeStore::open(f.root.join("adversarial-copy-fixture")).unwrap();
    let ordinary_id = "ordinary-copy-before-synthetic-rewrite";
    let ordinary_request = crate::RestoreCopyIntent {
        operation_id: OperationId(ordinary_id.into()),
        expected_archive_sha256: initial_receipt.sha256.clone(),
    };
    let copied = historical
        .restore_archive_as_copy(&initial, &ordinary_request)
        .unwrap();
    let (old_sha, old_json): (String, String) = historical.connection.query_row(
        "SELECT p.proof_sha256,p.receipt_json FROM archive_copy_proofs p JOIN archive_copies c USING(proof_sha256)", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    let proof: serde_json::Value = serde_json::from_str(&old_json).unwrap();
    assert_eq!(proof["mapping_count"], 0);
    assert_eq!(proof["ancestors"], serde_json::json!([]));
    assert!(
        rows(
            &historical.connection,
            "SELECT * FROM archive_copy_proof_operations"
        )
        .is_empty()
    );
    let reserved_id = format!("{SOURCE_ATTACHMENT_OPERATION_PREFIX}{}", "c".repeat(64));
    let old_token = format!(
        "\"operation_id\":{}",
        serde_json::to_string(ordinary_id).unwrap()
    );
    let new_token = format!(
        "\"operation_id\":{}",
        serde_json::to_string(&reserved_id).unwrap()
    );
    assert_eq!(old_json.matches(&old_token).count(), 1);
    // Retain the canonical struct field order and every other proof byte.
    let new_json = old_json.replacen(&old_token, &new_token, 1);
    let new_sha = checksum(new_json.as_bytes());
    let triggers: Vec<(String, String)> = historical.connection.prepare(
        "SELECT name,sql FROM sqlite_schema WHERE name IN ('immutable_archive_copy_proofs_update','immutable_archive_project_lineage_update','immutable_archive_copies_update') ORDER BY name")
        .unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    assert_eq!(triggers.len(), 3);
    let transaction = historical.connection.transaction().unwrap();
    transaction
        .execute_batch("PRAGMA defer_foreign_keys=ON;")
        .unwrap();
    for (name, _) in &triggers {
        transaction
            .execute_batch(&format!("DROP TRIGGER \"{name}\";"))
            .unwrap();
    }
    transaction
        .execute(
            "UPDATE archive_copy_proofs SET proof_sha256=?1,receipt_json=?2 WHERE proof_sha256=?3",
            rusqlite::params![new_sha, new_json, old_sha],
        )
        .unwrap();
    transaction
        .execute(
            "UPDATE archive_project_lineage SET proof_sha256=?1 WHERE proof_sha256=?2",
            rusqlite::params![new_sha, old_sha],
        )
        .unwrap();
    transaction
        .execute(
            "UPDATE archive_copies SET operation_id=?1,proof_sha256=?2 WHERE proof_sha256=?3",
            rusqlite::params![reserved_id, new_sha, old_sha],
        )
        .unwrap();
    for (_, sql) in triggers {
        transaction.execute_batch(&sql).unwrap();
    }
    transaction.commit().unwrap();
    assert!(rows(&historical.connection, "PRAGMA foreign_key_check").is_empty());
    let originals = f.originals();
    let historical_before = snapshot(&historical.connection);
    let reserved_request = crate::RestoreCopyIntent {
        operation_id: OperationId(reserved_id.clone()),
        expected_archive_sha256: initial_receipt.sha256,
    };
    let replay = historical
        .restore_archive_as_copy(&f.root.join("missing-original-archive"), &reserved_request)
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, copied.document);
    assert_eq!(replay.operation_id.0, reserved_id);
    assert_eq!(snapshot(&historical.connection), historical_before);

    let outbound = f.root.join("retained-reserved-copy.sqlite3");
    let exported = historical
        .export_archive(copied.document.id(), &outbound)
        .unwrap();
    assert_eq!(snapshot(&historical.connection), historical_before);
    let bytes = fs::read(&outbound).unwrap();
    assert_eq!(checksum(&bytes), exported.sha256);
    let archive =
        Connection::open_with_flags(&outbound, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let actual: (String, String, String) = archive.query_row("SELECT c.operation_id,p.proof_sha256,p.receipt_json FROM archive_copies c JOIN archive_copy_proofs p USING(proof_sha256)", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(actual, (reserved_id.clone(), new_sha, new_json));
    drop(archive);
    let mut destination = NativeStore::open(f.root.join("reserved-lineage-destination")).unwrap();
    let before = snapshot(&destination.connection);
    let error = destination
        .restore_archive(&outbound)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Source attachment operation requires"),
        "{error}"
    );
    assert_eq!(snapshot(&destination.connection), before);
    assert_eq!(fs::read(&outbound).unwrap(), bytes);
    // A new normal-ID copy retains the reserved ID only as immutable ancestry;
    // it cannot promote that historical ID into the live operation namespace.
    let normal_request = crate::RestoreCopyIntent {
        operation_id: OperationId("safe-copy-of-adversarial-lineage".into()),
        expected_archive_sha256: exported.sha256,
    };
    let safe = destination
        .restore_archive_as_copy(&outbound, &normal_request)
        .unwrap();
    assert_ne!(safe.document.id(), copied.document.id());
    let live: String = destination
        .connection
        .query_row("SELECT operation_id FROM archive_copies", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(live, normal_request.operation_id.0);
    let proof_count: i64 = destination
        .connection
        .query_row("SELECT count(*) FROM archive_copy_proofs", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(proof_count, 2);
    assert_eq!(fs::read(&outbound).unwrap(), bytes);
    assert_eq!(f.originals(), originals);
}
