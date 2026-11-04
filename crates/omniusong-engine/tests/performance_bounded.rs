//! Synthetic software-rendered sources only. No native device or catalog access.
use omniusong_core::contracts::{ClockEpochId, EndpointId, TakeId};
use omniusong_engine::{
    live::recording_live_pair,
    performance_records::{PerformanceConfig, PerformanceSource},
    performance_tap::TapInfo,
    performance_writer::{
        PerformanceWriter, inspect_performance, inspect_performance_bounded,
        inspect_sealed_performance, inspect_sealed_performance_bounded, recover_performance,
    },
    *,
};
use omniusong_recording::{
    CaptureReader, InspectionBudget, InspectionLimits, InspectionResource, RecordingError,
    SealedJournal,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Fixture {
    root: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new(finishable: bool) -> Self {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-bounded-tests");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        let path = root.join("take.sqlite3");
        let scope = LiveScope {
            worker_generation: LiveId([51; 16]),
            session_id: LiveId([52; 16]),
            panic_generation: 1,
        };
        let config = PerformanceConfig {
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            info: TapInfo {
                scope,
                sample_rate: 8000,
                channels: 1,
            },
            gui_clock: LiveClock {
                domain: "bounded-test-gui".into(),
                epoch: "bounded-test-gui-epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            output_epoch: ClockEpochId("bounded-test-output".into()),
            sources: std::array::from_fn(|i| PerformanceSource {
                binding: LiveSourceBinding {
                    kind: [
                        LiveSourceKind::ComputerKeys,
                        LiveSourceKind::PointerKeys,
                        LiveSourceKind::Pads,
                        LiveSourceKind::AccessiblePulse,
                    ][i],
                    handle: i as u8 + 1,
                    connection_generation: 1,
                },
                endpoint: EndpointId(format!("bounded-test-source-{i}")),
            }),
        };
        let mut writer = if finishable {
            PerformanceWriter::create_finishable(&path, config)
        } else {
            PerformanceWriter::create(&path, config)
        }
        .unwrap();
        let (admission, mut renderer, mut tap) = recording_live_pair(scope, 8000, 1).unwrap();
        for _ in 0..2 {
            renderer.render_into(&mut [0.0; 16]);
            while let Some(packet) = tap.try_pop().unwrap() {
                writer.push_packet(packet).unwrap();
            }
            writer.flush().unwrap();
        }
        drop(renderer);
        drop(tap);
        drop(admission);
        writer.close_interrupted().unwrap();
        Self { root, path }
    }
    fn hash(&self) -> String {
        let mut file = fs::File::open(&self.path).unwrap();
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let count = file.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        format!("{:x}", hash.finalize())
    }
    fn files(&self) -> Vec<PathBuf> {
        let mut files = fs::read_dir(&self.root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        files.sort();
        files
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn budget(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, cancelled.clone()).unwrap(),
        cancelled,
    )
}

#[test]
fn bounded_v1_and_v2_capture_reviews_match_legacy_identity_and_count_repeat_semantic_passes() {
    for v2 in [false, true] {
        let f = Fixture::new(v2);
        let before = f.hash();
        let files = f.files();
        let legacy = {
            let reader = CaptureReader::open(&f.path).unwrap();
            inspect_performance(&reader).unwrap()
        };
        let (budget, _) = budget(InspectionLimits::default());
        let reader = CaptureReader::open_bounded(&f.path, &budget).unwrap();
        let opened = budget.usage();
        let first = inspect_performance_bounded(&reader).unwrap();
        let once = budget.usage();
        assert_eq!(first, legacy);
        assert!(first.problem.is_none());
        assert_eq!(first.checkpoint.as_ref().unwrap().group_sequence, 2);
        assert_eq!(once.groups - opened.groups, 2);
        assert!(once.record_visits > opened.record_visits);
        assert!(once.work_bytes > opened.work_bytes);
        let second = inspect_performance_bounded(&reader).unwrap();
        let twice = budget.usage();
        assert_eq!(second, first);
        assert_eq!(twice.groups - once.groups, 2);
        assert!(twice.record_visits > once.record_visits);
        assert!(twice.work_bytes > once.work_bytes);
        assert_eq!(reader.inspection_budget().unwrap().usage(), twice);
        drop(reader);
        assert_eq!(f.hash(), before);
        assert_eq!(f.files(), files);
    }
}

#[test]
fn a_group_limit_after_a_valid_checkpoint_is_an_outer_sticky_refusal_not_a_prefix() {
    let f = Fixture::new(false);
    let before = f.hash();
    let (budget, _) = budget(InspectionLimits {
        max_groups: 1,
        ..Default::default()
    });
    let reader = CaptureReader::open_bounded(&f.path, &budget).unwrap();
    let first = inspect_performance_bounded(&reader).unwrap_err();
    assert!(matches!(
        first,
        RecordingError::InspectionLimit {
            resource: InspectionResource::Groups,
            limit: 1,
            attempted: 2
        }
    ));
    assert_eq!(budget.usage().groups, 1);
    assert!(matches!(
        inspect_performance_bounded(&reader),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::Groups,
            ..
        })
    ));
    drop(reader);
    assert_eq!(f.hash(), before);
}

#[test]
fn compatibility_wrappers_cannot_drop_a_bounded_readers_semantic_budget_or_return_partial_success()
{
    for sealed in [false, true] {
        for cancel in [false, true] {
            let fixture = Fixture::new(true);
            if sealed {
                let reader = CaptureReader::open(&fixture.path).unwrap();
                let reviewed = inspect_performance(&reader).unwrap();
                recover_performance(reader, &reviewed, "bounded-compatibility-seal").unwrap();
            }
            let before = fixture.hash();
            let (budget, cancelled) = budget(InspectionLimits {
                max_groups: 1,
                ..Default::default()
            });
            let (first, repeated) = if sealed {
                let reader = SealedJournal::open_bounded(&fixture.path, &budget).unwrap();
                cancelled.store(cancel, Ordering::Release);
                let first = inspect_sealed_performance(&reader);
                cancelled.store(false, Ordering::Release);
                (first, inspect_sealed_performance(&reader))
            } else {
                let reader = CaptureReader::open_bounded(&fixture.path, &budget).unwrap();
                cancelled.store(cancel, Ordering::Release);
                let first = inspect_performance(&reader);
                cancelled.store(false, Ordering::Release);
                (first, inspect_performance(&reader))
            };
            assert!(
                first.is_err() && repeated.is_err(),
                "sealed={sealed}, cancel={cancel}"
            );
            if cancel {
                assert!(matches!(
                    budget.checkpoint(),
                    Err(RecordingError::InspectionCancelled)
                ));
                assert_eq!(budget.usage().groups, 0);
            } else {
                assert!(matches!(
                    budget.checkpoint(),
                    Err(RecordingError::InspectionLimit {
                        resource: InspectionResource::Groups,
                        limit: 1,
                        attempted: 2
                    })
                ));
                assert_eq!(budget.usage().groups, 1);
            }
            assert_eq!(fixture.hash(), before);
        }
    }
}

#[test]
fn cancellation_after_bounded_open_is_typed_sticky_and_cannot_become_corrupt_tail_success() {
    let f = Fixture::new(false);
    let before = f.hash();
    let (budget, cancelled) = budget(InspectionLimits::default());
    let reader = CaptureReader::open_bounded(&f.path, &budget).unwrap();
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        inspect_performance_bounded(&reader),
        Err(RecordingError::InspectionCancelled)
    ));
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        inspect_performance_bounded(&reader),
        Err(RecordingError::InspectionCancelled)
    ));
    drop(reader);
    assert_eq!(f.hash(), before);
}

#[test]
fn bounded_semantic_entrypoints_refuse_unbounded_readers_without_inventing_new_governors() {
    let f = Fixture::new(false);
    let reader = CaptureReader::open(&f.path).unwrap();
    let review = inspect_performance(&reader).unwrap();
    assert!(matches!(
        inspect_performance_bounded(&reader),
        Err(RecordingError::Invalid(_))
    ));
    recover_performance(reader, &review, "bounded-test-seal").unwrap();
    let reader = SealedJournal::open(&f.path).unwrap();
    assert!(matches!(
        inspect_sealed_performance_bounded(&reader),
        Err(RecordingError::Invalid(_))
    ));
}

#[test]
fn sealed_bounded_reader_shares_open_snapshot_reread_and_semantic_work_without_source_changes() {
    let f = Fixture::new(true);
    let reader = CaptureReader::open(&f.path).unwrap();
    let reviewed = inspect_performance(&reader).unwrap();
    recover_performance(reader, &reviewed, "bounded-sealed-test").unwrap();
    let before = f.hash();
    let files = f.files();
    let legacy = {
        let sealed = SealedJournal::open(&f.path).unwrap();
        inspect_sealed_performance(&sealed).unwrap()
    };
    let (budget, _) = budget(InspectionLimits::default());
    let reader = SealedJournal::open_bounded(&f.path, &budget).unwrap();
    let opened = budget.usage();
    let actual = inspect_sealed_performance_bounded(&reader).unwrap();
    let usage = budget.usage();
    assert_eq!(actual, legacy);
    assert!(
        actual.finish_cut.is_none(),
        "this is recovered software evidence, not a native normal Finish"
    );
    assert!(usage.chunk_visits > opened.chunk_visits);
    assert!(usage.work_bytes > opened.work_bytes);
    assert_eq!(usage.groups - opened.groups, 2);
    drop(reader);
    assert_eq!(f.hash(), before);
    assert_eq!(f.files(), files);
}

#[test]
fn performance_inspection_owned_review_fields_are_strict_while_legacy_serialized_shape_is_unchanged()
 {
    let f = Fixture::new(false);
    let reader = CaptureReader::open(&f.path).unwrap();
    let review = inspect_performance(&reader).unwrap();
    let mut value = serde_json::to_value(&review).unwrap();
    assert!(value.get("finish_cut").is_none());
    value["future_owned_field"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<omniusong_engine::performance_writer::PerformanceInspection>(
            value
        )
        .is_err()
    );
}
