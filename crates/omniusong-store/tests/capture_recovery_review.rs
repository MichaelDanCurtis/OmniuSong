//! Independent retained-stage recovery/publication/archive closure regressions.
//! All audio is synthetic; no device, legacy data or application store is accessed.
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, TakePublication};
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

struct Fixture {
    root: PathBuf,
    store_root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/capture-recovery-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let store_root = root.join("store");
        drop(NativeStore::open(&store_root).unwrap());
        Self { root, store_root }
    }
    fn store(&self) -> NativeStore {
        NativeStore::open_existing(&self.store_root).unwrap()
    }
    fn stage(&self) -> (String, PathBuf, TakeDescriptor, Journal) {
        let id = uuid::Uuid::new_v4().to_string();
        let path = self
            .store_root
            .join("captures")
            .join(&id)
            .join("take.sqlite3");
        let take = descriptor();
        let journal = Journal::create(&path, &take).unwrap();
        (id, path, take, journal)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Capture recovery review retained at {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn descriptor() -> TakeDescriptor {
    TakeDescriptor { version:JOURNAL_VERSION,take_id:TakeId(uuid::Uuid::new_v4().to_string()),streams:vec![StreamDescriptor { stream_id:"audio".into(),segment_id:"segment".into(),source_id:"review-original".into(),source_name:"Synthetic retained source".into(),source_connection_generation:9,host_clock:HostClock { domain:ClockDomainId("host".into()),epoch:ClockEpochId("boot".into()),ticks_per_second_numerator:1_000_000_000,ticks_per_second_denominator:1 },kind:StreamKind::Audio { format:PcmFormat { sample_rate:48_000,channels:1,encoding:PcmEncoding::S16Le,layout:PcmLayout::Interleaved },audio_epoch:ClockEpochId("audio".into()) },initial_source_position:100 }],provenance:BTreeMap::from([("vendor".into(),serde_json::from_str(r#"{"integer":123456789012345678901234567890,"takeId":"opaque","decimal":0.12345678901234567890123456789}"#).unwrap())]) }
}
fn append(
    journal: &mut Journal,
    take: &TakeDescriptor,
    sequence: u64,
    start: u64,
    count: u64,
    gap: bool,
) {
    let stamp = HostTimestamp {
        domain: take.streams[0].host_clock.domain.clone(),
        epoch: take.streams[0].host_clock.epoch.clone(),
        ticks: sequence * 1_000_000,
    };
    journal
        .append(
            &ChunkHeader {
                version: CHUNK_VERSION,
                take_id: take.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                sequence,
                range: SourceRange::AudioFrames { start, count },
                interpretation: take.streams[0].kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: if gap {
                    ChunkKind::Gap {
                        reason: GapReason::TransferOverflow,
                    }
                } else {
                    ChunkKind::Content
                },
            },
            &if gap {
                vec![]
            } else {
                vec![5; count as usize * 2]
            },
        )
        .unwrap();
}
fn row_counts(path: &Path) -> (String, i64, i64, i64) {
    let c = Connection::open(path).unwrap();
    (
        c.query_row("SELECT state FROM take_meta", [], |r| r.get(0))
            .unwrap(),
        c.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
            .unwrap(),
        c.query_row("SELECT count(*) FROM interruptions", [], |r| r.get(0))
            .unwrap(),
        c.query_row("SELECT count(*) FROM sealed_manifests", [], |r| r.get(0))
            .unwrap(),
    )
}
fn publication(document: &ProjectDocument, source: &SealedJournal) -> TakePublication {
    TakePublication {
        operation_id: OperationId("attach-reviewed-original".into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        track_id: TrackId("recovered-track".into()),
        track_name: "Recovered verified prefix".into(),
        clip: TakeClip {
            edit_source: None,
            id: "recovered-clip".into(),
            source: TakeReference {
                take_id: source.descriptor().take_id.clone(),
                manifest_sha256: source.manifest().sha256.clone(),
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames {
                start: 100,
                count: 2,
            },
            position: TimelinePosition {
                frame: 0,
                sample_rate: 48_000,
            },
            fades: None,
        },
    }
}

#[test]
fn scan_is_bounded_nonrecursive_and_never_creates_or_opens_stages() {
    use omniusong_store::CaptureStageState;
    let f = Fixture::new();
    let store = f.store();
    let captures = f.store_root.join("captures");
    assert!(!captures.exists());
    let empty = store.scan_captures();
    assert!(empty.stages.is_empty());
    assert!(!captures.exists());
    let (id, path, take, mut writer) = f.stage();
    append(&mut writer, &take, 0, 100, 2, false);
    let bytes = fs::read(&path).unwrap();
    let scan = store.scan_captures();
    assert!(!scan.truncated);
    let stage = scan.stages.iter().find(|s| s.stage_id == id).unwrap();
    assert_eq!(stage.state, CaptureStageState::Uninspected);
    assert!(matches!(
        store.inspect_capture(&id),
        Err(omniusong_store::StoreError::Recording(
            RecordingError::Owned
        ))
    ));
    assert_eq!(writer.state().unwrap(), TakeState::Recording);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    drop(writer);
    for index in 0..130 {
        fs::create_dir(captures.join(format!("invalid-{index}"))).unwrap();
    }
    let scan = store.scan_captures();
    assert!(scan.truncated);
    assert!(scan.stages.len() <= 128);
    assert!(
        scan.stages
            .iter()
            .all(|s| s.stage_id == id || s.state == CaptureStageState::Invalid)
    );
    assert_eq!(row_counts(&path), ("recording".into(), 1, 0, 0));
}

#[test]
fn inspection_source_interval_is_not_recorded_frame_count_or_known_completion() {
    let f = Fixture::new();
    let store = f.store();
    let (id, path, take, mut writer) = f.stage();
    append(&mut writer, &take, 0, 100, 2, false);
    append(&mut writer, &take, 1, 102, 1, true);
    drop(writer);
    let bytes = fs::read(&path).unwrap();
    let preview = store.inspect_capture(&id).unwrap();
    assert!(preview.unknown_tail);
    assert!(preview.manifest_sha256.is_none());
    assert_eq!(preview.verified_chunks, 2);
    let a = &preview.audio[0];
    assert_eq!(
        (a.start_frame, a.frames, a.recorded_frames, a.gap_frames),
        (100, 3, 2, 1)
    );
    assert_eq!((a.sample_rate, a.channels), (48_000, 1));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let recovered = store
        .recover_capture(&preview, "recover-selected-prefix")
        .unwrap();
    assert!(recovered.unknown_tail);
    assert!(!recovered.replayed);
    assert_eq!(recovered.audio[0].frames, 3);
    let source = SealedJournal::open(&recovered.journal_path).unwrap();
    let manifest = source.manifest();
    assert_eq!(manifest.body.streams[0].source_end.value(), 103);
    assert_eq!(manifest.body.streams[0].recorded_units, 2);
    assert_eq!(manifest.body.streams[0].gap_units, 1);
    assert!(
        manifest
            .body
            .interrupted
            .as_ref()
            .unwrap()
            .report
            .unknown_tail
    );
}

#[test]
fn stale_review_changed_request_and_other_operation_cannot_recover_or_replay() {
    let f = Fixture::new();
    let store = f.store();
    let (id, path, take, mut writer) = f.stage();
    append(&mut writer, &take, 0, 100, 2, false);
    append(&mut writer, &take, 1, 102, 2, false);
    drop(writer);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'01020304' WHERE sequence=1;").unwrap();
    drop(db);
    let stale = store.inspect_capture(&id).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE chunks SET payload=x'04030201' WHERE sequence=1", [])
        .unwrap();
    let changed_bytes = fs::read(&path).unwrap();
    assert!(store.recover_capture(&stale, "reviewed-once").is_err());
    assert_eq!(fs::read(&path).unwrap(), changed_bytes);
    assert_eq!(row_counts(&path), ("recording".into(), 2, 0, 0));
    let reviewed = store.inspect_capture(&id).unwrap();
    assert_ne!(stale.journal_sha256, reviewed.journal_sha256);
    assert_ne!(stale.original_rows_sha256, reviewed.original_rows_sha256);
    assert_eq!(stale.chunk_index_sha256, reviewed.chunk_index_sha256);
    let first = store.recover_capture(&reviewed, "reviewed-once").unwrap();
    assert!(!first.replayed);
    let sealed_bytes = fs::read(&path).unwrap();
    let replay = store.recover_capture(&reviewed, "reviewed-once").unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.manifest_sha256, first.manifest_sha256);
    assert_eq!(fs::read(&path).unwrap(), sealed_bytes);
    assert!(
        store
            .recover_capture(&reviewed, "different-operation")
            .is_err()
    );
    let mut changed = reviewed.clone();
    changed.unknown_tail = false;
    assert!(store.recover_capture(&changed, "reviewed-once").is_err());
    let now = store.inspect_capture(&id).unwrap();
    assert!(
        store.recover_capture(&now, "reviewed-once").is_err(),
        "A newly reviewed sealed document is a different recovery request"
    );
    assert_eq!(fs::read(&path).unwrap(), sealed_bytes);
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE chunks SET payload=x'09090909' WHERE sequence=1", [])
        .unwrap();
    assert!(
        store.recover_capture(&reviewed, "reviewed-once").is_err(),
        "Matching selected manifest/op must not authenticate a changed unselected original tail"
    );
    let altered = store.inspect_capture(&id).unwrap();
    let mut forged = reviewed.clone();
    forged.original_rows_sha256 = altered.original_rows_sha256;
    assert!(
        store.recover_capture(&forged, "reviewed-once").is_err(),
        "Keeping the old inspection token cannot authorize a substituted original-row proof"
    );
    let mut forged = reviewed.clone();
    forged.audio[0].recorded_frames += 1;
    assert!(
        store.recover_capture(&forged, "reviewed-once").is_err(),
        "The inspection token must bind audio summaries as well as source hashes"
    );
}

#[cfg(unix)]
#[test]
fn stage_and_root_redirects_hardlinks_and_noncanonical_ids_stay_nonactionable() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let store = f.store();
    let (id, path, take, mut writer) = f.stage();
    append(&mut writer, &take, 0, 100, 2, false);
    drop(writer);
    let before = fs::read(&path).unwrap();
    for bad in [
        "../outside",
        "../../take.sqlite3",
        "not-a-uuid",
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
    ] {
        assert!(store.inspect_capture(bad).is_err());
    }
    let alias = f.root.join("alias.sqlite3");
    fs::hard_link(&path, &alias).unwrap();
    assert!(store.inspect_capture(&id).is_err());
    assert!(
        store
            .scan_captures()
            .stages
            .iter()
            .find(|s| s.stage_id == id)
            .unwrap()
            .problem
            .is_some()
    );
    fs::remove_file(alias).unwrap();
    let redirected_id = uuid::Uuid::new_v4().to_string();
    let redirected = f.store_root.join("captures").join(&redirected_id);
    symlink(path.parent().unwrap(), &redirected).unwrap();
    assert!(store.inspect_capture(&redirected_id).is_err());
    let file_id = uuid::Uuid::new_v4().to_string();
    let file_dir = f.store_root.join("captures").join(&file_id);
    fs::create_dir(&file_dir).unwrap();
    symlink(&path, file_dir.join("take.sqlite3")).unwrap();
    assert!(store.inspect_capture(&file_id).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let f2 = Fixture::new();
    let store2 = f2.store();
    symlink(
        f.store_root.join("captures"),
        f2.store_root.join("captures"),
    )
    .unwrap();
    let rejected = store2.scan_captures();
    assert!(rejected.stages.is_empty());
    assert!(!rejected.problems.is_empty());
    assert!(store2.inspect_capture(&id).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn empty_or_gap_only_audio_is_not_recovered_as_a_playable_original() {
    for gap_only in [false, true] {
        let f = Fixture::new();
        let store = f.store();
        let (id, path, take, mut writer) = f.stage();
        if gap_only {
            append(&mut writer, &take, 0, 100, 2, true);
        }
        drop(writer);
        let preview = store.inspect_capture(&id).unwrap();
        assert_eq!(preview.audio[0].recorded_frames, 0);
        let before = fs::read(&path).unwrap();
        assert!(
            store
                .recover_capture(&preview, "no-recorded-audio")
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn damaged_tail_recovery_attachment_history_and_archive_preserve_complete_original_evidence() {
    let f = Fixture::new();
    let mut store = f.store();
    let (id, path, take, mut writer) = f.stage();
    append(&mut writer, &take, 0, 100, 2, false);
    append(&mut writer, &take, 1, 102, 2, false);
    drop(writer);
    Connection::open(&path).unwrap().execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'01020304' WHERE sequence=1;").unwrap();
    let preview = store.inspect_capture(&id).unwrap();
    assert_eq!(preview.audio[0].frames, 2);
    assert!(!preview.problems.is_empty());
    let recovered = store
        .recover_capture(&preview, "recover-original-for-project")
        .unwrap();
    assert!(recovered.unknown_tail);
    let source = SealedJournal::open(&path).unwrap();
    let source_manifest = source.manifest().clone();
    let source_bytes = fs::read(&path).unwrap();
    assert_eq!(source_manifest.body.descriptor, take);
    assert_eq!(source_manifest.body.chunk_count, 1);
    assert!(source.read_chunk("audio", 1).is_err());
    let original = store
        .create(&ProjectDocument::new("Recovery publication").unwrap())
        .unwrap();
    let intent = publication(&original, &source);
    let attached = store.attach_capture(&path, &intent).unwrap();
    assert!(!attached.replayed);
    assert_eq!(
        attached.document.take_clips().unwrap()[0].range,
        TakeRange::AudioFrames {
            start: 100,
            count: 2
        }
    );
    let undone = store
        .undo(original.id(), attached.document.revision())
        .unwrap();
    assert!(undone.take_clips().unwrap().is_empty());
    let redone = store.redo(original.id(), undone.revision()).unwrap();
    assert_eq!(redone.take_clips().unwrap().len(), 1);
    let undone = store.undo(original.id(), redone.revision()).unwrap();
    let archive = f.root.join("recovered-history.omniusong-native");
    store.export_archive(original.id(), &archive).unwrap();
    let mut restored_store = NativeStore::open(f.root.join("restored")).unwrap();
    let restored = restored_store.restore_archive(&archive).unwrap();
    assert_eq!(restored.to_json().unwrap(), undone.to_json().unwrap());
    let restored_take = restored_store.open_take(&intent.clip.source).unwrap();
    assert_eq!(restored_take.manifest(), &source_manifest);
    let physical = Connection::open(restored_take.path()).unwrap();
    let chunks: i64 = physical
        .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    let bad: Vec<u8> = physical
        .query_row("SELECT payload FROM chunks WHERE sequence=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let interruptions: i64 = physical
        .query_row("SELECT count(*) FROM interruptions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(chunks, 2);
    assert_eq!(bad, [1, 2, 3, 4]);
    assert_eq!(interruptions, 1);
    drop(physical);
    drop(restored_take);
    let replay = restored_store
        .attach_capture(f.root.join("missing-stage.sqlite3"), &intent)
        .unwrap();
    assert!(replay.replayed);
    let redo = restored_store
        .redo(restored.id(), restored.revision())
        .unwrap();
    assert_eq!(redo.take_clips().unwrap().len(), 1);
    assert_eq!(fs::read(&path).unwrap(), source_bytes);
    assert!(path.exists(), "Export never deletes the retained stage");
}
