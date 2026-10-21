//! Independent bounded-reader tests. Synthetic workspace-local journals only.
//! These checks confer no performance recovery or native lifetime authority.
use omniusong_recording::*;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Fixture(PathBuf);
impl Fixture {
    fn new(chunks: u64, sealed: bool) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/bounded-inspection-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let fixture = Self(root);
        let take = descriptor();
        let mut writer = Journal::create(fixture.path(), &take).unwrap();
        for sequence in 0..chunks {
            let stamp = HostTimestamp {
                domain: take.streams[0].host_clock.domain.clone(),
                epoch: take.streams[0].host_clock.epoch.clone(),
                ticks: sequence * 2,
            };
            writer
                .append(
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
                    &[0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat(),
                )
                .unwrap();
        }
        if sealed {
            writer.seal("independent-normal-fixture").unwrap();
        }
        drop(writer);
        fixture
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn sidecar(&self, suffix: &str) -> PathBuf {
        let mut path = self.path().into_os_string();
        path.push(suffix);
        PathBuf::from(path)
    }
    fn sql(&self) -> Connection {
        Connection::open(self.path()).unwrap()
    }
    fn files(&self) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }
    fn owner_is_free(&self) {
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.sidecar(".owner.lock"))
            .unwrap();
        owner
            .try_lock()
            .expect("Inspection leaked its exclusive owner");
    }
    fn corrupt_second_chunk(&self) {
        self.sql()
            .execute_batch(
                "DROP TRIGGER immutable_chunk_update;
                 UPDATE chunks SET payload=zeroblob(8) WHERE sequence=1;",
            )
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Bounded inspection fixture retained at {}",
                self.0.display()
            );
        } else {
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
            source_id: "synthetic-review".into(),
            source_name: "Device-free bounded inspection".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("synthetic-frames".into()),
                epoch: ClockEpochId("synthetic-clock".into()),
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
                audio_epoch: ClockEpochId("synthetic-audio".into()),
            },
            initial_source_position: 100,
        }],
        provenance: BTreeMap::from([(
            "opaque".into(),
            serde_json::from_str(
                r#"{"huge":123456789012345678901234567890,"decimal":0.12345678901234567890123456789,"takeId":"uninterpreted"}"#,
            )
            .unwrap(),
        )]),
    }
}
fn budget(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
        cancelled,
    )
}
fn error<T>(result: omniusong_recording::Result<T>) -> RecordingError {
    match result {
        Ok(_) => panic!("Inspection unexpectedly succeeded"),
        Err(error) => error,
    }
}
fn assert_limit(error: RecordingError, resource: InspectionResource) {
    match error {
        RecordingError::InspectionLimit {
            resource: actual,
            limit,
            attempted,
        } => {
            assert_eq!(actual, resource);
            assert!(attempted > limit);
        }
        other => panic!("Expected typed inspection limit, got {other:?}"),
    }
}

#[test]
fn bounded_readers_match_legacy_identity_in_recording_normal_and_recovered_modes() {
    for mode in 0..3 {
        let f = Fixture::new(3, mode == 1);
        if mode == 2 {
            f.corrupt_second_chunk();
            CaptureReader::open(f.path())
                .unwrap()
                .recover("independent-recovered-fixture")
                .unwrap();
        }
        let expected = CaptureReader::open(f.path()).unwrap().inspection().clone();
        let before = f.files();
        let (governor, _) = budget(InspectionLimits::default());
        let reader = CaptureReader::open_bounded(f.path(), &governor).unwrap();
        assert_eq!(reader.inspection(), &expected);
        assert!(reader.inspection_budget().is_some());
        assert_eq!(reader.read_ordinal(1).unwrap().payload.len(), 8);
        assert!(governor.usage().work_bytes > 0);
        assert!(governor.usage().chunk_visits > 0);
        drop(reader);
        if mode != 0 {
            let (sealed_budget, _) = budget(InspectionLimits::default());
            let sealed = SealedJournal::open_bounded(f.path(), &sealed_budget).unwrap();
            assert!(sealed.inspection_budget().is_some());
            assert_eq!(sealed.inspect_snapshot().unwrap(), expected);
            assert_eq!(sealed.read_ordinal(1).unwrap().payload.len(), 8);
        }
        assert_eq!(f.files(), before);
        f.owner_is_free();
    }
}

#[test]
fn physical_file_limit_precedes_corrupt_sqlite_parsing_and_releases_owner() {
    let f = Fixture::new(2, false);
    let mut bytes = fs::read(f.path()).unwrap();
    bytes[..16].fill(0);
    fs::write(f.path(), bytes).unwrap();
    let before = f.files();
    let limits = InspectionLimits {
        max_file_bytes: 0,
        ..InspectionLimits::default()
    };
    let (governor, _) = budget(limits);
    assert_limit(
        error(CaptureReader::open_bounded(f.path(), &governor)),
        InspectionResource::FileBytes,
    );
    assert_eq!(f.files(), before);
    f.owner_is_free();
}

#[test]
fn all_three_sidecars_are_refused_by_both_forms_without_repair_or_side_effects() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let f = Fixture::new(2, true);
        fs::write(f.sidecar(suffix), b"retained ambiguous sidecar").unwrap();
        let before = f.files();
        let (governor, _) = budget(InspectionLimits::default());
        assert!(CaptureReader::open_bounded(f.path(), &governor).is_err());
        let (sealed_budget, _) = budget(InspectionLimits::default());
        assert!(SealedJournal::open_bounded(f.path(), &sealed_budget).is_err());
        assert_eq!(f.files(), before);
        f.owner_is_free();
    }
}

#[test]
fn cancellation_is_sticky_before_open_and_never_becomes_a_prefix_report() {
    let f = Fixture::new(2, false);
    let before = f.files();
    let (governor, cancelled) = budget(InspectionLimits::default());
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        error(CaptureReader::open_bounded(f.path(), &governor)),
        RecordingError::InspectionCancelled
    ));
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        governor.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    let (fresh, _) = budget(InspectionLimits::default());
    drop(CaptureReader::open_bounded(f.path(), &fresh).unwrap());
    assert_eq!(f.files(), before);
    f.owner_is_free();
}

#[test]
fn cancellation_after_a_verified_prefix_propagates_from_existing_reader_accessors() {
    let f = Fixture::new(3, true);
    let before = f.files();
    let (governor, cancelled) = budget(InspectionLimits::default());
    let reader = CaptureReader::open_bounded(f.path(), &governor).unwrap();
    assert_eq!(reader.inspection().report.verified_chunks, 3);
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        reader.read_ordinal(1),
        Err(RecordingError::InspectionCancelled)
    ));
    drop(reader);
    let (sealed_budget, cancelled) = budget(InspectionLimits::default());
    let sealed = SealedJournal::open_bounded(f.path(), &sealed_budget).unwrap();
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        sealed.inspect_snapshot(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert!(matches!(
        sealed.read_ordinal(1),
        Err(RecordingError::InspectionCancelled)
    ));
    drop(sealed);
    assert_eq!(f.files(), before);
    f.owner_is_free();
}

#[test]
fn all_physical_rows_are_preflighted_even_when_generic_prefix_stops_at_first_chunk() {
    let f = Fixture::new(40, false);
    f.corrupt_second_chunk();
    let legacy = CaptureReader::open(f.path()).unwrap();
    assert_eq!(legacy.inspection().report.verified_chunks, 1);
    drop(legacy);
    let before = f.files();
    let limits = InspectionLimits {
        max_physical_rows: 32,
        ..InspectionLimits::default()
    };
    let (governor, _) = budget(limits);
    assert_limit(
        error(CaptureReader::open_bounded(f.path(), &governor)),
        InspectionResource::PhysicalRows,
    );
    assert_eq!(f.files(), before);
    f.owner_is_free();
}

#[test]
fn repeated_reads_use_the_original_chunk_allowance_and_return_outer_limit() {
    let f = Fixture::new(3, false);
    let (probe, _) = budget(InspectionLimits::default());
    drop(CaptureReader::open_bounded(f.path(), &probe).unwrap());
    let open_visits = probe.usage().chunk_visits;
    assert!(open_visits >= 3);
    let limits = InspectionLimits {
        max_chunk_visits: open_visits,
        ..InspectionLimits::default()
    };
    let (governor, _) = budget(limits);
    let reader = CaptureReader::open_bounded(f.path(), &governor).unwrap();
    assert_eq!(reader.inspection().report.verified_chunks, 3);
    assert_limit(
        error(reader.read_ordinal(1)),
        InspectionResource::ChunkVisits,
    );
    assert_limit(
        error(governor.checkpoint()),
        InspectionResource::ChunkVisits,
    );
    drop(reader);
    f.owner_is_free();
}

#[test]
fn reusing_governor_cannot_reset_repeated_physical_and_original_work() {
    let f = Fixture::new(3, false);
    let before = f.files();
    let (probe, _) = budget(InspectionLimits::default());
    drop(CaptureReader::open_bounded(f.path(), &probe).unwrap());
    let once = probe.usage().work_bytes;
    assert!(once >= fs::metadata(f.path()).unwrap().len());
    let limits = InspectionLimits {
        max_work_bytes: once * 2 - 1,
        ..InspectionLimits::default()
    };
    let (governor, _) = budget(limits);
    drop(CaptureReader::open_bounded(f.path(), &governor).unwrap());
    assert_limit(
        error(CaptureReader::open_bounded(f.path(), &governor)),
        InspectionResource::WorkBytes,
    );
    assert_eq!(f.files(), before);
    f.owner_is_free();
}

#[test]
fn vm_and_scalar_row_allowances_refuse_as_typed_errors_without_mutation() {
    let f = Fixture::new(2, false);
    let before = f.files();
    for vm in [true, false] {
        let mut limits = InspectionLimits::default();
        let resource = if vm {
            limits.max_vm_steps = 0;
            InspectionResource::VmSteps
        } else {
            limits.max_row_visits = 0;
            InspectionResource::RowVisits
        };
        let (governor, _) = budget(limits);
        assert_limit(
            error(CaptureReader::open_bounded(f.path(), &governor)),
            resource,
        );
        assert_eq!(f.files(), before);
        f.owner_is_free();
    }
}

#[test]
fn view_and_generated_column_schema_are_rejected_before_payload_interpretation() {
    for generated in [false, true] {
        let f = Fixture::new(2, false);
        if generated {
            f.sql().execute_batch("ALTER TABLE chunks ADD COLUMN derived INTEGER GENERATED ALWAYS AS (length(payload)) VIRTUAL;").unwrap();
        } else {
            f.sql().execute_batch("DROP TABLE stream_prefix; CREATE VIEW stream_prefix AS SELECT 'audio' AS stream_id, 2 AS next_sequence;").unwrap();
        }
        let before = f.files();
        let (governor, _) = budget(InspectionLimits::default());
        assert!(matches!(
            error(CaptureReader::open_bounded(f.path(), &governor)),
            RecordingError::Invalid(_)
        ));
        assert_eq!(f.files(), before);
        f.owner_is_free();
    }
}

#[test]
fn opaque_ordinary_columns_and_damaged_tail_remain_bound_without_silent_omission() {
    let f = Fixture::new(3, false);
    f.corrupt_second_chunk();
    f.sql().execute_batch("ALTER TABLE chunks ADD COLUMN vendor_bytes BLOB; UPDATE chunks SET vendor_bytes=x'00ff017f' WHERE sequence=2;").unwrap();
    let expected = CaptureReader::open(f.path()).unwrap().inspection().clone();
    assert_eq!(expected.report.verified_chunks, 1);
    let before = f.files();
    let (governor, _) = budget(InspectionLimits::default());
    let actual = CaptureReader::open_bounded(f.path(), &governor).unwrap();
    assert_eq!(actual.inspection(), &expected);
    drop(actual);
    assert_eq!(f.files(), before);
    f.sql()
        .execute(
            "UPDATE chunks SET vendor_bytes=x'00ff017e' WHERE sequence=2",
            [],
        )
        .unwrap();
    let (changed_budget, _) = budget(InspectionLimits::default());
    let changed = CaptureReader::open_bounded(f.path(), &changed_budget).unwrap();
    assert_eq!(
        changed.inspection().report.chunk_index_sha256,
        expected.report.chunk_index_sha256
    );
    assert_ne!(
        changed.inspection().original_rows_sha256,
        expected.original_rows_sha256
    );
    assert_ne!(changed.inspection().journal_sha256, expected.journal_sha256);
}

#[test]
fn missing_owner_is_not_created_and_live_owner_remains_exclusive() {
    let f = Fixture::new(2, false);
    let reader_owner = OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.sidecar(".owner.lock"))
        .unwrap();
    reader_owner.try_lock().unwrap();
    let before = f.files();
    let (governor, _) = budget(InspectionLimits::default());
    assert!(matches!(
        error(CaptureReader::open_bounded(f.path(), &governor)),
        RecordingError::Owned
    ));
    assert_eq!(f.files(), before);
    drop(reader_owner);
    fs::remove_file(f.sidecar(".owner.lock")).unwrap();
    let before = f.files();
    let (governor, _) = budget(InspectionLimits::default());
    assert!(CaptureReader::open_bounded(f.path(), &governor).is_err());
    assert_eq!(f.files(), before);
    assert!(!f.sidecar(".owner.lock").exists());
}
