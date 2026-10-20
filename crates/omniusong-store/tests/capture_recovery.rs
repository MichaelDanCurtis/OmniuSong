//! Owner tests: local stage errors, exact review admission and independent commits.
use omniusong_recording::*;
use omniusong_store::{CaptureStageState, NativeStore, StoreError};
use rusqlite::Connection;
use std::{collections::BTreeMap, fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> (Self, NativeStore) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/capture-recovery-owner")
            .join(uuid::Uuid::new_v4().to_string());
        let store = NativeStore::open(&path).unwrap();
        (Self(path), store)
    }
    fn stage(&self) -> (String, PathBuf) {
        let id = uuid::Uuid::new_v4().to_string();
        let path = self.0.join("captures").join(&id).join("take.sqlite3");
        let take = descriptor();
        let mut j = Journal::create(&path, &take).unwrap();
        for sequence in 0..2 {
            let stamp = HostTimestamp {
                domain: ClockDomainId("host".into()),
                epoch: ClockEpochId("epoch".into()),
                ticks: sequence * 1000,
            };
            j.append(
                &ChunkHeader {
                    version: CHUNK_VERSION,
                    take_id: take.take_id.clone(),
                    stream_id: "audio".into(),
                    segment_id: "segment".into(),
                    sequence,
                    range: SourceRange::AudioFrames {
                        start: 100 + sequence * 2,
                        count: 2,
                    },
                    interpretation: take.streams[0].kind.clone(),
                    observed_start: stamp.clone(),
                    observed_end: stamp,
                    clock_anchors: vec![],
                    kind: ChunkKind::Content,
                },
                &[1; 8],
            )
            .unwrap();
        }
        drop(j);
        (id, path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn descriptor() -> TakeDescriptor {
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "segment".into(),
            source_id: "synthetic".into(),
            source_name: "No device".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("host".into()),
                epoch: ClockEpochId("epoch".into()),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            kind: StreamKind::Audio {
                format: PcmFormat {
                    sample_rate: 48000,
                    channels: 1,
                    encoding: PcmEncoding::F32Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: ClockEpochId("audio".into()),
            },
            initial_source_position: 100,
        }],
        provenance: BTreeMap::new(),
    }
}
fn counts(path: &std::path::Path) -> (String, i64, i64) {
    let c = Connection::open(path).unwrap();
    (
        c.query_row("SELECT state FROM take_meta", [], |r| r.get(0))
            .unwrap(),
        c.query_row("SELECT count(*) FROM interruptions", [], |r| r.get(0))
            .unwrap(),
        c.query_row("SELECT count(*) FROM sealed_manifests", [], |r| r.get(0))
            .unwrap(),
    )
}
#[test]
fn scan_inspect_recover_and_retry_never_publish_main_catalog_rows() {
    let (f, store) = Fixture::new();
    let (id, path) = f.stage();
    let main = f.0.join("studio.sqlite3");
    let catalog = fs::read(&main).unwrap();
    let stage = fs::read(&path).unwrap();
    let scan = store.scan_captures();
    assert_eq!(scan.stages.len(), 1);
    assert_eq!(scan.stages[0].state, CaptureStageState::Uninspected);
    assert_eq!(fs::read(&path).unwrap(), stage);
    let review = store.inspect_capture(&id).unwrap();
    assert_eq!(fs::read(&path).unwrap(), stage);
    let receipt = store.recover_capture(&review, "owner-recovery").unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.audio[0].start_frame, 100);
    assert_eq!(receipt.audio[0].frames, 4);
    assert!(receipt.unknown_tail);
    let replay = store.recover_capture(&review, "owner-recovery").unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.manifest_sha256, receipt.manifest_sha256);
    let before = fs::read(&path).unwrap();
    assert!(
        store
            .recover_capture(&review, "different-operation")
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read(main).unwrap(), catalog);
    assert!(store.list().unwrap().is_empty());
}
#[test]
fn stage_sqlite_errors_do_not_masquerade_as_catalog_corruption() {
    let (f, mut store) = Fixture::new();
    let (id, path) = f.stage();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("DROP TABLE chunks;").unwrap();
    drop(c);
    let error = store.inspect_capture(&id).unwrap_err();
    assert!(matches!(
        error,
        StoreError::Recording(RecordingError::Database(_))
    ));
    let project = omniusong_core::ProjectDocument::new("catalog still usable").unwrap();
    store.create(&project).unwrap();
    assert_eq!(store.load(project.id()).unwrap(), project);
}
#[test]
fn changed_physical_header_requires_new_review_even_if_all_original_rows_match() {
    let (f, store) = Fixture::new();
    let (id, path) = f.stage();
    let old = store.inspect_capture(&id).unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("PRAGMA application_id=12345;").unwrap();
    drop(c);
    let current = store.inspect_capture(&id).unwrap();
    assert_eq!(current.original_rows_sha256, old.original_rows_sha256);
    assert_eq!(current.chunk_index_sha256, old.chunk_index_sha256);
    assert_ne!(current.journal_sha256, old.journal_sha256);
    let before = fs::read(&path).unwrap();
    assert!(store.recover_capture(&old, "old-review").is_err());
    assert_eq!(counts(&path), ("recording".into(), 0, 0));
    assert_eq!(fs::read(&path).unwrap(), before);
    store.recover_capture(&current, "fresh-review").unwrap();
}
#[test]
fn review_token_cannot_be_kept_while_changing_any_reviewed_source_field() {
    let (f, store) = Fixture::new();
    let (id, path) = f.stage();
    let review = store.inspect_capture(&id).unwrap();
    let before = fs::read(&path).unwrap();
    for field in 0..4 {
        let mut forged = review.clone();
        match field {
            0 => forged.byte_length += 1,
            1 => {
                forged.audio[0].frames += 1;
                forged.audio[0].recorded_frames += 1;
            }
            2 => forged.original_rows_sha256 = "0".repeat(64),
            _ => forged.take_id = "different-take".into(),
        };
        assert!(store.recover_capture(&forged, "forged-review").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    assert_eq!(counts(&path), ("recording".into(), 0, 0));
}
#[test]
fn recovered_manifest_does_not_hide_later_changes_in_retained_bad_rows() {
    let (f, store) = Fixture::new();
    let (id, path) = f.stage();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(8) WHERE sequence=1;").unwrap();
    drop(c);
    let review = store.inspect_capture(&id).unwrap();
    assert_eq!(review.verified_chunks, 1);
    let receipt = store
        .recover_capture(&review, "recovery-tail-proof")
        .unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute(
        "UPDATE chunks SET payload=?1 WHERE sequence=1",
        [vec![2u8; 8]],
    )
    .unwrap();
    drop(c);
    let current = store.inspect_capture(&id).unwrap();
    assert_eq!(current.manifest_sha256, Some(receipt.manifest_sha256));
    assert_eq!(current.chunk_index_sha256, review.chunk_index_sha256);
    assert_ne!(current.original_rows_sha256, review.original_rows_sha256);
    let before = fs::read(&path).unwrap();
    assert!(
        store
            .recover_capture(&review, "recovery-tail-proof")
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn opaque_extra_original_column_is_retained_and_authenticated_on_replay() {
    let (f, store) = Fixture::new();
    let (id, path) = f.stage();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("ALTER TABLE chunks ADD COLUMN \"vendor:opaque\" BLOB; DROP TRIGGER immutable_chunk_update;").unwrap();
    c.execute("UPDATE chunks SET \"vendor:opaque\"=?1", [vec![7u8; 65537]])
        .unwrap();
    drop(c);
    let review = store.inspect_capture(&id).unwrap();
    store
        .recover_capture(&review, "opaque-source-review")
        .unwrap();
    assert!(
        store
            .recover_capture(&review, "opaque-source-review")
            .unwrap()
            .replayed
    );
    let c = Connection::open(&path).unwrap();
    c.execute(
        "UPDATE chunks SET \"vendor:opaque\"=?1 WHERE sequence=1",
        [vec![8u8; 65537]],
    )
    .unwrap();
    drop(c);
    let after = store.inspect_capture(&id).unwrap();
    assert!(after.manifest_sha256.is_some());
    assert_ne!(after.original_rows_sha256, review.original_rows_sha256);
    assert!(
        store
            .recover_capture(&review, "opaque-source-review")
            .is_err()
    );
}
#[test]
fn recovery_state_trigger_cannot_change_originals_before_interruption_commit() {
    let (f, store) = Fixture::new();
    let (id, path) = f.stage();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("DROP TRIGGER immutable_chunk_update; CREATE TRIGGER source_changing_interruption AFTER UPDATE OF state ON take_meta WHEN NEW.state='interrupted' BEGIN UPDATE chunks SET payload=zeroblob(8) WHERE sequence=1; END;").unwrap();
    drop(c);
    let review = store.inspect_capture(&id).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(
        store
            .recover_capture(&review, "trigger-source-review")
            .is_err()
    );
    assert_eq!(counts(&path), ("recording".into(), 0, 0));
    assert_eq!(fs::read(path).unwrap(), before);
}
