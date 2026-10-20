//! Independent retained-capture inspection/recovery checks. Synthetic journals only.
use omniusong_recording::*;
use rusqlite::Connection;
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/capture-inspection-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn db(&self) -> Connection {
        Connection::open(self.path()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Capture inspection review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn descriptor() -> TakeDescriptor {
    TakeDescriptor { version:JOURNAL_VERSION,take_id:TakeId(uuid::Uuid::new_v4().to_string()),streams:vec![StreamDescriptor {
        stream_id:"audio".into(),segment_id:"segment".into(),source_id:"synthetic-review-input".into(),source_name:"Device-free retained capture".into(),source_connection_generation:1,
        host_clock:HostClock { domain:ClockDomainId("host".into()),epoch:ClockEpochId("epoch".into()),ticks_per_second_numerator:1_000_000_000,ticks_per_second_denominator:1 },
        kind:StreamKind::Audio { format:PcmFormat { sample_rate:48_000,channels:1,encoding:PcmEncoding::S16Le,layout:PcmLayout::Interleaved },audio_epoch:ClockEpochId("audio-epoch".into()) },initial_source_position:100,
    }],provenance:BTreeMap::from([("vendor".into(),serde_json::from_str(r#"{"takeId":"opaque-original","number":123456789012345678901234567890,"decimal":0.12345678901234567890123456789}"#).unwrap())]) }
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
                vec![7; count as usize * 2]
            },
        )
        .unwrap();
}
fn row_counts(f: &Fixture) -> (String, i64, i64, i64) {
    let c = f.db();
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
#[test]
fn inspection_retains_exact_recording_bytes_state_and_exclusive_ownership() {
    let f = Fixture::new();
    let take = descriptor();
    let mut writer = Journal::create(f.path(), &take).unwrap();
    append(&mut writer, &take, 0, 100, 2, false);
    let live_bytes = fs::read(f.path()).unwrap();
    assert!(matches!(
        CaptureReader::open(f.path()),
        Err(RecordingError::Owned)
    ));
    assert_eq!(writer.state().unwrap(), TakeState::Recording);
    assert_eq!(fs::read(f.path()).unwrap(), live_bytes);
    drop(writer);
    let before = fs::read(f.path()).unwrap();
    let rows = row_counts(&f);
    let reader = CaptureReader::open(f.path()).unwrap();
    let inspected = reader.inspection();
    assert_eq!(inspected.descriptor, take);
    assert_eq!(inspected.report.state, TakeState::Recording);
    assert!(inspected.report.unknown_tail);
    assert_eq!(inspected.report.streams[0].source_end.value(), 102);
    assert_eq!(inspected.report.streams[0].recorded_units, 2);
    assert!(inspected.manifest.is_none());
    assert!(matches!(
        Journal::open(f.path()),
        Err(RecordingError::Owned)
    ));
    assert_eq!(fs::read(f.path()).unwrap(), before);
    drop(reader);
    assert_eq!(row_counts(&f), rows);
    assert_eq!(fs::read(f.path()).unwrap(), before);
}
#[test]
fn inspection_distinguishes_interrupted_and_sealed_without_additional_mutation() {
    let f = Fixture::new();
    let take = descriptor();
    let mut writer = Journal::create(f.path(), &take).unwrap();
    append(&mut writer, &take, 0, 100, 2, false);
    append(&mut writer, &take, 1, 102, 1, true);
    drop(writer);
    let interrupted = Journal::open(f.path()).unwrap();
    drop(interrupted);
    let before = fs::read(f.path()).unwrap();
    let rows = row_counts(&f);
    let reader = CaptureReader::open(f.path()).unwrap();
    assert_eq!(reader.inspection().report.state, TakeState::Interrupted);
    assert!(reader.inspection().report.unknown_tail);
    assert_eq!(reader.inspection().report.streams[0].gap_units, 1);
    drop(reader);
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert_eq!(row_counts(&f), rows);
    let mut recovered = Journal::open(f.path()).unwrap();
    let sealed = recovered
        .seal_recovered("existing-explicit-recovery")
        .unwrap();
    drop(recovered);
    let before = fs::read(f.path()).unwrap();
    let rows = row_counts(&f);
    let reader = CaptureReader::open(f.path()).unwrap();
    assert_eq!(reader.inspection().report.state, TakeState::Sealed);
    assert!(reader.inspection().report.unknown_tail);
    assert_eq!(reader.inspection().manifest.as_ref().unwrap(), &sealed);
    assert_eq!(
        reader.inspection().report.streams[0].source_end.value(),
        103
    );
    drop(reader);
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert_eq!(row_counts(&f), rows);
}
#[test]
fn corrupt_unselected_tail_is_bound_by_full_file_fingerprint_and_retained_on_recovery() {
    let f = Fixture::new();
    let take = descriptor();
    let mut writer = Journal::create(f.path(), &take).unwrap();
    append(&mut writer, &take, 0, 100, 2, false);
    append(&mut writer, &take, 1, 102, 2, false);
    drop(writer);
    f.db().execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'01020304' WHERE sequence=1;").unwrap();
    let before = fs::read(f.path()).unwrap();
    let rows = row_counts(&f);
    let first = CaptureReader::open(f.path()).unwrap();
    let preview = first.inspection().clone();
    assert_eq!(preview.report.verified_chunks, 1);
    assert_eq!(preview.report.streams[0].source_end.value(), 102);
    assert!(!preview.report.problems.is_empty());
    drop(first);
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert_eq!(row_counts(&f), rows);
    f.db()
        .execute("UPDATE chunks SET payload=x'04030201' WHERE sequence=1", [])
        .unwrap();
    let reader = CaptureReader::open(f.path()).unwrap();
    assert_eq!(
        reader.inspection().report.chunk_index_sha256,
        preview.report.chunk_index_sha256
    );
    assert_eq!(
        reader.inspection().report.verified_chunks,
        preview.report.verified_chunks
    );
    assert_ne!(
        reader.inspection().journal_sha256,
        preview.journal_sha256,
        "A changed bad tail must invalidate the review despite an identical verified prefix"
    );
    let (manifest, replayed) = reader.recover("inspection-review-recovery").unwrap();
    assert!(!replayed);
    assert_eq!(manifest.body.streams[0].source_end.value(), 102);
    assert_eq!(manifest.body.chunk_count, 1);
    assert!(
        manifest
            .body
            .interrupted
            .as_ref()
            .unwrap()
            .report
            .unknown_tail
    );
    let c = f.db();
    let payload: Vec<u8> = c
        .query_row("SELECT payload FROM chunks WHERE sequence=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(payload, [4, 3, 2, 1]);
    assert_eq!(row_counts(&f).1, 2);
    let reader = CaptureReader::open(f.path()).unwrap();
    let (same, replayed) = reader.recover("inspection-review-recovery").unwrap();
    assert!(replayed);
    assert_eq!(same, manifest);
    assert!(
        CaptureReader::open(f.path())
            .unwrap()
            .recover("different-recovery-operation")
            .is_err()
    );
}
#[cfg(unix)]
#[test]
fn redirected_or_aliased_journals_never_become_inspection_owners() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let take = descriptor();
    let writer = Journal::create(f.path(), &take).unwrap();
    drop(writer);
    let before = fs::read(f.path()).unwrap();
    let alias = f.0.join("alias.sqlite3");
    fs::hard_link(f.path(), &alias).unwrap();
    assert!(CaptureReader::open(f.path()).is_err());
    assert!(CaptureReader::open(&alias).is_err());
    fs::remove_file(alias).unwrap();
    let alias = f.0.join("symlink.sqlite3");
    symlink(f.path(), &alias).unwrap();
    assert!(CaptureReader::open(&alias).is_err());
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert_eq!(row_counts(&f), ("recording".into(), 0, 0, 0));
}
