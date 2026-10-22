//! Unix-qualified synthetic library copies only. No native devices, catalog,
//! durable executor, Complete receipt, or normal performance-seal authority.
#![cfg(unix)]

use omniusong_core::contracts::{ClockEpochId, EndpointId, TakeId};
use omniusong_engine::{
    LiveBatch, LiveClock, LiveEvent, LiveEventKind, LiveId, LivePatch, LiveScope,
    LiveSourceBinding, LiveSourceKind,
    live::recording_live_pair,
    performance_copy::recover_performance_copy,
    performance_records::{PerformanceConfig, PerformanceSource},
    performance_tap::{PerformancePacket, PerformancePacketKind, TapInfo},
    performance_writer::{
        PerformanceInspection, PerformanceWriter, inspect_performance, inspect_performance_bounded,
    },
};
use omniusong_recording::{
    CaptureReader, CopyRecoveryBoundary, InspectionBudget, InspectionLimits, InspectionResource,
    Journal, RecordingError, SourcePosition,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const ACTION: &str = "11111111111111111111111111111111";
const OPERATION: &str = "synthetic-copy-recovery";
const STREAMS: [&str; 6] = ["audio", "gui-1", "gui-2", "gui-3", "gui-4", "control"];

#[derive(Debug, Clone, Copy)]
enum Mode {
    Clean,
    Partial,
    Fault,
    Finish,
    Empty,
}
struct Fixture {
    root: PathBuf,
    source: PathBuf,
    parent: PathBuf,
}
impl Fixture {
    fn new(v2: bool, mode: Mode) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-copy-engine-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir(root.join("source-actions")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let source = root.join("source/take.sqlite3");
        let parent = root.join("source-actions");
        let config = config();
        let mut writer = if v2 {
            PerformanceWriter::create_finishable(&source, config.clone())
        } else {
            PerformanceWriter::create(&source, config.clone())
        }
        .unwrap();
        let (mut admission, mut renderer, mut owner) =
            recording_live_pair(config.info.scope, 8000, 1).unwrap();
        if !matches!(mode, Mode::Empty) {
            let mut batch = LiveBatch::empty(
                config.info.scope,
                1,
                admission.lease_credit().map(|credit| credit.token),
            );
            for source in 1..=4 {
                batch.try_push(on(source)).unwrap();
            }
            admission.admit(batch).unwrap();
            renderer.render_into(&mut [0.; 64]);
            let first = drain(&mut owner);
            push(&mut writer, &first);
            writer.flush().unwrap();
            match mode {
                Mode::Partial | Mode::Fault => {
                    let progress = admission.progress();
                    let mut batch = LiveBatch::empty(
                        progress.scope,
                        progress.next_sequence().unwrap(),
                        admission.lease_credit().map(|credit| credit.token),
                    );
                    batch
                        .try_push(LiveEvent {
                            source: 2,
                            connection_generation: 1,
                            observed_ticks: 200,
                            event: if matches!(mode, Mode::Fault) {
                                LiveEventKind::NoteOff {
                                    voice: 999,
                                    release_velocity16: 0,
                                }
                            } else {
                                LiveEventKind::Sustain { down: true }
                            },
                        })
                        .unwrap();
                    admission.admit(batch).unwrap();
                    renderer.render_into(&mut [0.; 512]);
                    let packets = drain(&mut owner);
                    if matches!(mode, Mode::Partial) {
                        let first_pcm = packets
                            .iter()
                            .position(|packet| matches!(packet.kind, PerformancePacketKind::Pcm(_)))
                            .unwrap();
                        push(&mut writer, &packets[..=first_pcm]);
                    } else {
                        push(&mut writer, &packets);
                        writer.flush().unwrap();
                    }
                }
                Mode::Finish => {
                    admission
                        .begin_finish(config.info.scope, LiveId([43; 16]))
                        .unwrap();
                    renderer.render_into(&mut [0.; 64]);
                    push(&mut writer, &drain(&mut owner));
                    writer.flush().unwrap();
                }
                _ => {}
            }
        }
        drop((renderer, owner, admission));
        writer.close_interrupted().unwrap();
        Self {
            root,
            source,
            parent,
        }
    }
    fn bounded(&self, budget: &InspectionBudget) -> CaptureReader {
        CaptureReader::open_bounded(&self.source, budget).unwrap()
    }
    fn source_tree(&self) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        tree(self.source.parent().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Performance copy fixture retained: {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn config() -> PerformanceConfig {
    PerformanceConfig {
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        info: TapInfo {
            scope: LiveScope {
                worker_generation: LiveId([31; 16]),
                session_id: LiveId([32; 16]),
                panic_generation: 1,
            },
            sample_rate: 8000,
            channels: 1,
        },
        gui_clock: LiveClock {
            domain: "copy-test.gui".into(),
            epoch: "copy-test.gui-epoch".into(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 1,
        },
        output_epoch: ClockEpochId("copy-test.output".into()),
        sources: std::array::from_fn(|index| PerformanceSource {
            binding: LiveSourceBinding {
                kind: [
                    LiveSourceKind::ComputerKeys,
                    LiveSourceKind::PointerKeys,
                    LiveSourceKind::Pads,
                    LiveSourceKind::AccessiblePulse,
                ][index],
                handle: index as u8 + 1,
                connection_generation: 1,
            },
            endpoint: EndpointId(format!("copy-test.source-{index}")),
        }),
    }
}
fn on(source: u8) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: 100 + u64::from(source),
        event: LiveEventKind::NoteOn {
            note: 1,
            voice: 1,
            key: 60,
            velocity16: u16::MAX,
            patch: LivePatch::Keys,
        },
    }
}
fn drain(
    owner: &mut omniusong_engine::performance_tap::PerformanceTapOwner,
) -> Vec<PerformancePacket> {
    let mut packets = Vec::new();
    while let Some(packet) = owner.try_pop().unwrap() {
        packets.push(packet);
    }
    packets
}
fn push(writer: &mut PerformanceWriter, packets: &[PerformancePacket]) {
    for packet in packets {
        writer.push_packet(*packet).unwrap();
    }
}
fn governor(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
        cancelled,
    )
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if path.is_dir() {
                out.insert(relative, None);
                visit(root, &path, out);
            } else {
                out.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
type OriginalRow = (i64, String, i64, Vec<u8>, Vec<u8>, String);
fn originals(path: &Path) -> Vec<OriginalRow> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT ordinal,stream_id,sequence,header,payload,sha256 FROM chunks ORDER BY ordinal",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn operation(action: &str, operation: &str, expected: &PerformanceInspection) -> String {
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.performance-copy-recovery.v1\0");
    hash.update(serde_json::to_vec(&(action, operation, expected)).unwrap());
    format!("performance-copy:{:x}", hash.finalize())
}

#[test]
fn v1_v2_fault_finish_and_partial_tail_copy_all_originals_but_finalize_only_the_six_stream_checkpoint()
 {
    for (v2, mode) in [
        (false, Mode::Partial),
        (true, Mode::Partial),
        (false, Mode::Fault),
        (true, Mode::Fault),
        (true, Mode::Finish),
    ] {
        let fixture = Fixture::new(v2, mode);
        let source = fs::read(&fixture.source).unwrap();
        let source_tree = fixture.source_tree();
        let rows = originals(&fixture.source);
        assert_eq!(
            rows.iter()
                .map(|row| row.1.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(STREAMS)
        );
        let (budget, _) = governor(InspectionLimits::default());
        let reader = fixture.bounded(&budget);
        let opened = budget.usage();
        let expected = inspect_performance_bounded(&reader).unwrap();
        let reviewed = budget.usage();
        assert!(expected.unknown_tail);
        assert_eq!(
            expected.capture_fault.is_some(),
            matches!(mode, Mode::Fault)
        );
        assert_eq!(expected.finish_cut.is_some(), matches!(mode, Mode::Finish));
        if matches!(mode, Mode::Partial) {
            assert!(
                expected.received_generic_chunks > expected.selection.as_ref().unwrap().chunk_count
            );
            assert!(expected.problem.is_some());
            assert_eq!(expected.checkpoint.as_ref().unwrap().audio_end, 64);
        }
        let receipt =
            recover_performance_copy(reader, &expected, &fixture.parent, ACTION, OPERATION)
                .unwrap();
        assert_eq!(
            receipt.manifest.body.operation_id,
            operation(ACTION, OPERATION, &expected)
        );
        assert!(
            receipt.manifest.body.interrupted.is_some(),
            "Finish evidence must not become a normal seal"
        );
        assert!(receipt.inspection.report.unknown_tail);
        assert_eq!(
            receipt.manifest.body.chunk_count,
            expected.selection.as_ref().unwrap().chunk_count
        );
        assert_eq!(
            receipt.manifest.body.chunk_index_sha256,
            expected.selection.as_ref().unwrap().chunk_index_sha256
        );
        assert_eq!(
            receipt.manifest.body.streams,
            expected.checkpoint.as_ref().unwrap().prefixes
        );
        assert_eq!(receipt.manifest.body.streams.len(), 6);
        assert_eq!(
            receipt.inspection.original_rows_sha256,
            expected.original_rows_sha256
        );
        assert_eq!(
            receipt.inspection.manifest.as_ref(),
            Some(&receipt.manifest)
        );
        assert_eq!(fs::read(&fixture.source).unwrap(), source);
        assert_eq!(fs::read(&receipt.paths.evidence).unwrap(), source);
        assert_eq!(fixture.source_tree(), source_tree);
        assert_eq!(originals(&receipt.paths.working), rows);
        assert_eq!(originals(&receipt.paths.evidence), rows);
        let working = CaptureReader::open(&receipt.paths.working).unwrap();
        let actual = inspect_performance(&working).unwrap();
        assert_eq!(actual.selection, expected.selection);
        assert_eq!(actual.checkpoint, expected.checkpoint);
        assert_eq!(actual.checkpoint_sha256, expected.checkpoint_sha256);
        assert_eq!(actual.descriptor_sha256, expected.descriptor_sha256);
        assert_eq!(actual.original_rows_sha256, expected.original_rows_sha256);
        assert_eq!(actual.capture_fault, expected.capture_fault);
        assert_eq!(actual.capture_fault_pending, expected.capture_fault_pending);
        assert_eq!(actual.finish_cut, expected.finish_cut);
        assert!(actual.unknown_tail);
        assert_eq!(
            actual.received_generic_chunks,
            receipt.manifest.body.chunk_count
        );
        assert_eq!(receipt.usage, budget.usage());
        assert!(
            receipt.usage.work_bytes > reviewed.work_bytes
                && reviewed.work_bytes > opened.work_bytes
        );
        assert_eq!(
            receipt.usage.groups - reviewed.groups,
            2 * expected.checkpoint.as_ref().unwrap().group_sequence
        );
        assert!(
            receipt.usage.row_visits > reviewed.row_visits
                && receipt.usage.vm_steps > reviewed.vm_steps
        );
        eprintln!(
            "performance copy v2={v2} {mode:?}: source={} opened={opened:?} reviewed={reviewed:?} final={:?}",
            source.len(),
            receipt.usage
        );
    }
}

#[test]
fn cancellation_and_altered_expected_refuse_before_creating_any_artifact() {
    let fixture = Fixture::new(true, Mode::Clean);
    let source = fixture.source_tree();
    let expected = {
        let (budget, _) = governor(InspectionLimits::default());
        let reader = fixture.bounded(&budget);
        inspect_performance_bounded(&reader).unwrap()
    };
    for field in 0..8 {
        let mut altered = expected.clone();
        match field {
            0 => altered.descriptor_sha256 = "e".repeat(64),
            1 => altered.original_rows_sha256 = "e".repeat(64),
            2 => altered.journal_sha256_at_review = "e".repeat(64),
            3 => altered.selection.as_mut().unwrap().chunk_count += 1,
            4 => altered.checkpoint.as_mut().unwrap().audio_end += 1,
            5 => altered.capture_fault_pending = !altered.capture_fault_pending,
            6 => altered.unknown_tail = false,
            _ => altered.problem = Some("unobserved semantic diagnosis".into()),
        }
        let (budget, _) = governor(InspectionLimits::default());
        let error = recover_performance_copy(
            fixture.bounded(&budget),
            &altered,
            &fixture.parent,
            ACTION,
            OPERATION,
        )
        .unwrap_err();
        assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
        assert!(matches!(error.cause, RecordingError::Invalid(_)));
        assert!(tree(&fixture.parent).is_empty());
    }
    let (budget, cancelled) = governor(InspectionLimits::default());
    let reader = fixture.bounded(&budget);
    cancelled.store(true, Ordering::Release);
    let error = recover_performance_copy(reader, &expected, &fixture.parent, ACTION, OPERATION)
        .unwrap_err();
    assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
    assert!(matches!(error.cause, RecordingError::InspectionCancelled));
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert!(tree(&fixture.parent).is_empty());
    assert_eq!(fixture.source_tree(), source);
}

#[test]
fn working_semantic_limit_retains_both_images_and_typed_sticky_stop_without_finalization() {
    let fixture = Fixture::new(false, Mode::Clean);
    let source = fixture.source_tree();
    let (budget, _) = governor(InspectionLimits {
        max_groups: 2,
        ..InspectionLimits::default()
    });
    let reader = fixture.bounded(&budget);
    let expected = inspect_performance_bounded(&reader).unwrap();
    assert_eq!(budget.usage().groups, 1);
    let error = recover_performance_copy(reader, &expected, &fixture.parent, ACTION, OPERATION)
        .unwrap_err();
    assert_eq!(error.boundary, CopyRecoveryBoundary::WorkingVerified);
    assert!(matches!(
        error.cause,
        RecordingError::InspectionLimit {
            resource: InspectionResource::Groups,
            ..
        }
    ));
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::Groups,
            ..
        })
    ));
    assert_eq!(
        fs::read(&error.paths.evidence).unwrap(),
        fs::read(&fixture.source).unwrap()
    );
    assert_eq!(
        fs::read(&error.paths.working).unwrap(),
        fs::read(&fixture.source).unwrap()
    );
    let working = CaptureReader::open(&error.paths.working).unwrap();
    assert!(working.inspection().manifest.is_none());
    assert_eq!(inspect_performance(&working).unwrap(), expected);
    assert_eq!(fixture.source_tree(), source);
}

#[test]
fn unbounded_empty_checkpoint_invalid_identity_and_cumulative_work_limit_never_create_action() {
    let fixture = Fixture::new(true, Mode::Clean);
    let source = fixture.source_tree();
    let (probe, _) = governor(InspectionLimits::default());
    let reader = fixture.bounded(&probe);
    let opened = probe.usage();
    let expected = inspect_performance_bounded(&reader).unwrap();
    drop(reader);
    let error = recover_performance_copy(
        CaptureReader::open(&fixture.source).unwrap(),
        &expected,
        &fixture.parent,
        ACTION,
        OPERATION,
    )
    .unwrap_err();
    assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
    assert!(matches!(error.cause, RecordingError::Invalid(_)));
    for (action, operation) in [
        ("0".repeat(32), OPERATION.to_owned()),
        ("../escape".into(), OPERATION.into()),
        (ACTION.into(), "../invalid-operation".into()),
        (ACTION.into(), "x".repeat(97)),
    ] {
        let (budget, _) = governor(InspectionLimits::default());
        let error = recover_performance_copy(
            fixture.bounded(&budget),
            &expected,
            &fixture.parent,
            &action,
            &operation,
        )
        .unwrap_err();
        assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
        assert!(matches!(error.cause, RecordingError::Invalid(_)));
    }
    let (budget, _) = governor(InspectionLimits {
        max_work_bytes: opened.work_bytes,
        ..InspectionLimits::default()
    });
    let reader = fixture.bounded(&budget);
    let error = recover_performance_copy(reader, &expected, &fixture.parent, ACTION, OPERATION)
        .unwrap_err();
    assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
    assert!(matches!(
        error.cause,
        RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        }
    ));
    assert!(tree(&fixture.parent).is_empty());
    assert_eq!(fixture.source_tree(), source);
    let empty = Fixture::new(false, Mode::Empty);
    let (budget, _) = governor(InspectionLimits::default());
    let reader = empty.bounded(&budget);
    let expected = inspect_performance_bounded(&reader).unwrap();
    assert!(expected.selection.is_none());
    let error =
        recover_performance_copy(reader, &expected, &empty.parent, ACTION, OPERATION).unwrap_err();
    assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
    assert!(tree(&empty.parent).is_empty());
}

#[test]
fn existing_action_is_never_replayed_overwritten_or_cleaned_even_with_a_new_operation() {
    let fixture = Fixture::new(true, Mode::Finish);
    let source = fixture.source_tree();
    let (budget, _) = governor(InspectionLimits::default());
    let reader = fixture.bounded(&budget);
    let expected = inspect_performance_bounded(&reader).unwrap();
    let receipt =
        recover_performance_copy(reader, &expected, &fixture.parent, ACTION, OPERATION).unwrap();
    assert_eq!(
        receipt.manifest.body.operation_id,
        operation(ACTION, OPERATION, &expected)
    );
    let artifacts = tree(&fixture.parent);
    for operation in [OPERATION, "another-operation-must-not-replace"] {
        let (budget, _) = governor(InspectionLimits::default());
        let error = recover_performance_copy(
            fixture.bounded(&budget),
            &expected,
            &fixture.parent,
            ACTION,
            operation,
        )
        .unwrap_err();
        assert_eq!(error.boundary, CopyRecoveryBoundary::ExistingArtifacts);
        assert_eq!(tree(&fixture.parent), artifacts);
    }
    assert_eq!(fixture.source_tree(), source);
}

#[test]
fn unsupported_schema_extension_is_copied_as_evidence_but_refuses_working_mutation() {
    let fixture = Fixture::new(false, Mode::Partial);
    let original_rows = originals(&fixture.source);
    let connection = Connection::open(&fixture.source).unwrap();
    connection
        .execute_batch("ALTER TABLE chunks ADD COLUMN retained_extension BLOB DEFAULT x'000102ff';")
        .unwrap();
    drop(connection);
    assert_eq!(originals(&fixture.source), original_rows);
    let source = fixture.source_tree();
    let bytes = fs::read(&fixture.source).unwrap();
    let (budget, _) = governor(InspectionLimits::default());
    let reader = fixture.bounded(&budget);
    let expected = inspect_performance_bounded(&reader).unwrap();
    let error = recover_performance_copy(reader, &expected, &fixture.parent, ACTION, OPERATION)
        .unwrap_err();
    assert_eq!(error.boundary, CopyRecoveryBoundary::WorkingVerified);
    assert!(matches!(error.cause, RecordingError::Invalid(_)));
    assert_eq!(fs::read(&error.paths.evidence).unwrap(), bytes);
    assert_eq!(fs::read(&error.paths.working).unwrap(), bytes);
    for path in [&fixture.source, &error.paths.evidence, &error.paths.working] {
        assert_eq!(originals(path), original_rows);
        let connection =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let extension: (String, Vec<u8>) = connection.query_row(
            "SELECT typeof(retained_extension),retained_extension FROM chunks ORDER BY ordinal LIMIT 1",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(extension, ("blob".into(), vec![0, 1, 2, 255]));
    }
    assert!(
        CaptureReader::open(&error.paths.working)
            .unwrap()
            .inspection()
            .manifest
            .is_none()
    );
    assert_eq!(fixture.source_tree(), source);
}

#[test]
fn an_already_normal_generic_journal_refuses_without_manufacturing_performance_seal_authority() {
    let fixture = Fixture::new(false, Mode::Clean);
    let (mut descriptor, audio, expected) = {
        let (budget, _) = governor(InspectionLimits::default());
        let reader = fixture.bounded(&budget);
        let expected = inspect_performance_bounded(&reader).unwrap();
        let audio = (1..=reader.inspection().report.verified_chunks)
            .map(|ordinal| reader.read_ordinal(ordinal).unwrap())
            .find(|chunk| chunk.header.stream_id == "audio")
            .unwrap();
        (reader.inspection().descriptor.clone(), audio, expected)
    };
    // A separate generic fixture can be normally sealed without a native
    // performance permit. No performance Complete/finalize path is called.
    descriptor.provenance.clear();
    fs::create_dir(fixture.root.join("normal-generic")).unwrap();
    let path = fixture.root.join("normal-generic/take.sqlite3");
    let mut journal = Journal::create(&path, &descriptor).unwrap();
    journal.append(&audio.header, &audio.payload).unwrap();
    let normal = journal.seal("synthetic-generic-normal").unwrap();
    assert!(normal.body.interrupted.is_none());
    drop(journal);
    let before = tree(path.parent().unwrap());
    let (budget, _) = governor(InspectionLimits::default());
    let error = recover_performance_copy(
        CaptureReader::open_bounded(&path, &budget).unwrap(),
        &expected,
        &fixture.parent,
        ACTION,
        OPERATION,
    )
    .unwrap_err();
    assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
    assert!(
        matches!(error.cause,RecordingError::Invalid(ref message) if message.contains("unsealed"))
    );
    assert_eq!(tree(path.parent().unwrap()), before);
    assert!(tree(&fixture.parent).is_empty());
    assert_eq!(
        normal.body.streams[0].source_end,
        SourcePosition::AudioFrames(64)
    );
}

#[test]
fn existing_six_stream_results_reconcile_the_same_manifest_without_the_original_path() {
    use omniusong_engine::performance_copy::reconcile_performance_copy;
    use omniusong_recording::{CopyActionOwner, RecoveredCopyView};
    for (v2, mode) in [
        (false, Mode::Partial),
        (true, Mode::Partial),
        (false, Mode::Fault),
        (true, Mode::Fault),
        (true, Mode::Finish),
    ] {
        let fixture = Fixture::new(v2, mode);
        let source = fs::read(&fixture.source).unwrap();
        let raw_rows = originals(&fixture.source);
        let (budget, _) = governor(InspectionLimits::default());
        let reader = fixture.bounded(&budget);
        let expected = inspect_performance_bounded(&reader).unwrap();
        let owner = CopyActionOwner::create(&fixture.parent, ACTION).unwrap();
        let created =
            recover_performance_copy(reader, &expected, &fixture.parent, ACTION, OPERATION)
                .unwrap();
        let created_usage = budget.usage();
        {
            let view = RecoveredCopyView::open(
                &owner,
                &expected.journal_sha256_at_review,
                source.len() as u64,
                &budget,
            )
            .unwrap();
            let checked = reconcile_performance_copy(&view, &expected, ACTION, OPERATION).unwrap();
            assert_eq!(checked.manifest, created.manifest);
            assert_eq!(checked.inspection, created.inspection);
            assert!(checked.usage.work_bytes > created_usage.work_bytes);
            assert!(checked.usage.groups > created_usage.groups);
        }
        drop(owner);
        // Removing only this synthetic fixture's original proves that this is
        // an artifact replay, rather than convenient source-based recreation.
        fs::remove_dir_all(fixture.source.parent().unwrap()).unwrap();
        let before = tree(&fixture.root);
        let (budget, _) = governor(InspectionLimits::default());
        let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
        let view = RecoveredCopyView::open(
            &owner,
            &expected.journal_sha256_at_review,
            source.len() as u64,
            &budget,
        )
        .unwrap();
        let checked = reconcile_performance_copy(&view, &expected, ACTION, OPERATION).unwrap();
        assert_eq!(checked.manifest, created.manifest);
        assert_eq!(checked.inspection, created.inspection);
        assert_eq!(checked.usage, budget.usage());
        assert_eq!(originals(&checked.paths.working), raw_rows);
        assert_eq!(fs::read(&checked.paths.evidence).unwrap(), source);
        assert_eq!(tree(&fixture.root), before);
        assert!(
            !checked
                .paths
                .evidence
                .with_file_name("take.sqlite3.owner.lock")
                .exists()
        );
    }
}

#[test]
fn reconciliation_requires_original_semantics_exact_operation_and_one_sticky_governor() {
    use omniusong_engine::performance_copy::reconcile_performance_copy;
    use omniusong_recording::{CopyActionOwner, RecoveredCopyView};
    let fixture = Fixture::new(true, Mode::Partial);
    let (budget, _) = governor(InspectionLimits::default());
    let reader = fixture.bounded(&budget);
    let expected = inspect_performance_bounded(&reader).unwrap();
    let owner = CopyActionOwner::create(&fixture.parent, ACTION).unwrap();
    let receipt =
        recover_performance_copy(reader, &expected, &fixture.parent, ACTION, OPERATION).unwrap();
    drop(owner);
    let before = tree(&fixture.root);
    let (budget, cancelled) = governor(InspectionLimits::default());
    let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
    let view = RecoveredCopyView::open(
        &owner,
        &expected.journal_sha256_at_review,
        fs::metadata(&receipt.paths.evidence).unwrap().len(),
        &budget,
    )
    .unwrap();
    for change in 0..4 {
        let mut expected = expected.clone();
        let mut action = ACTION.to_owned();
        let mut operation = OPERATION.to_owned();
        match change {
            0 => expected.capture_fault_pending = !expected.capture_fault_pending,
            1 => expected.checkpoint_sha256 = Some("f".repeat(64)),
            2 => action = "2".repeat(32),
            3 => operation = "different-synthetic-request".into(),
            _ => unreachable!(),
        }
        assert!(matches!(
            reconcile_performance_copy(&view, &expected, &action, &operation),
            Err(RecordingError::Invalid(_))
        ));
        assert_eq!(tree(&fixture.root), before);
    }
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        reconcile_performance_copy(&view, &expected, ACTION, OPERATION),
        Err(RecordingError::InspectionCancelled)
    ));
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        view.receipt(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_eq!(tree(&fixture.root), before);
    drop(view);
    drop(owner);
    let (budget, _) = governor(InspectionLimits {
        max_groups: 0,
        ..Default::default()
    });
    let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
    let view = RecoveredCopyView::open(
        &owner,
        &expected.journal_sha256_at_review,
        fs::metadata(&receipt.paths.evidence).unwrap().len(),
        &budget,
    )
    .unwrap();
    assert!(matches!(
        reconcile_performance_copy(&view, &expected, ACTION, OPERATION),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::Groups,
            ..
        })
    ));
    assert_eq!(tree(&fixture.root), before);
}
