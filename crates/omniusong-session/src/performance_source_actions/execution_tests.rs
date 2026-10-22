//! Isolated synthetic durable execution regressions. No native inputs.
#![cfg(unix)]

use super::*;
use crate::{Backend, Command, LegacyScan, Paths, PerformanceSourceIntentReference};
use omniusong_core::contracts::{ClockEpochId, EndpointId, OperationId, TakeId};
use omniusong_engine::{
    LiveBatch, LiveClock, LiveEvent, LiveEventKind, LiveId, LivePatch, LiveScope,
    LiveSourceBinding, LiveSourceKind,
    live::recording_live_pair,
    performance_records::{PerformanceConfig, PerformanceSource},
    performance_tap::{PerformancePacket, PerformancePacketKind, TapInfo},
    performance_writer::PerformanceWriter,
};
use omniusong_store::NativeStore;
use sha2::{Digest, Sha256};
use std::{io::Write, sync::atomic::Ordering};

const STAGE: &str = "02020202020202020202020202020202";
const ACTION: &str = "11111111111111111111111111111111";
const OTHER: &str = "22222222222222222222222222222222";

#[derive(Clone, Copy)]
pub(super) enum Mode {
    Clean,
    Tail,
    Fault,
    Finish,
}
pub(super) struct Fixture {
    pub(super) root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-source-execution-session-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self {
            root: fs::canonicalize(root).unwrap(),
        }
    }
    pub(super) fn paths(&self) -> Paths {
        Paths {
            native: self.root.join("native"),
            legacy: self.root.join("legacy"),
        }
    }
    pub(super) fn backend(&self) -> NativeBackend {
        let mut backend = NativeBackend {
            paths: self.paths(),
            store: None,
            legacy: LegacyScan::default(),
            legacy_notice: None,
            require_existing: false,
        };
        backend.handle(&Command::Initialize).unwrap();
        backend
    }
    pub(super) fn source_path(&self) -> PathBuf {
        self.paths()
            .native
            .join("captures")
            .join(STAGE)
            .join("take.sqlite3")
    }
    pub(super) fn action_parent(&self) -> PathBuf {
        self.paths().native.join("source-actions")
    }
    fn source(&self, mode: Mode) {
        let path = self.source_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let config = PerformanceConfig {
            take_id: TakeId(format!("performance-{STAGE}")),
            info: TapInfo {
                scope: LiveScope {
                    worker_generation: LiveId([1; 16]),
                    session_id: LiveId([2; 16]),
                    panic_generation: 1,
                },
                sample_rate: 8000,
                channels: 1,
            },
            gui_clock: LiveClock {
                domain: "execution-test.gui".into(),
                epoch: "execution-test.epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            output_epoch: ClockEpochId("execution-test.output".into()),
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
                endpoint: EndpointId(format!("execution-test.source-{index}")),
            }),
        };
        let mut writer = PerformanceWriter::create_finishable(&path, config.clone()).unwrap();
        let (mut admission, mut renderer, mut owner) =
            recording_live_pair(config.info.scope, 8000, 1).unwrap();
        let mut batch = LiveBatch::empty(
            config.info.scope,
            1,
            admission.lease_credit().map(|c| c.token),
        );
        for source in 1..=4 {
            batch
                .try_push(LiveEvent {
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
                })
                .unwrap();
        }
        admission.admit(batch).unwrap();
        renderer.render_into(&mut [0.; 64]);
        push(&mut writer, &drain(&mut owner));
        writer.flush().unwrap();
        match mode {
            Mode::Tail | Mode::Fault => {
                let progress = admission.progress();
                let mut batch = LiveBatch::empty(
                    progress.scope,
                    progress.next_sequence().unwrap(),
                    admission.lease_credit().map(|c| c.token),
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
                if matches!(mode, Mode::Tail) {
                    let pcm = packets
                        .iter()
                        .position(|p| matches!(p.kind, PerformancePacketKind::Pcm(_)))
                        .unwrap();
                    push(&mut writer, &packets[..=pcm]);
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
            Mode::Clean => (),
        }
        drop((renderer, owner, admission));
        writer.close_interrupted().unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Execution fixture retained: {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn drain(
    owner: &mut omniusong_engine::performance_tap::PerformanceTapOwner,
) -> Vec<PerformancePacket> {
    let mut result = vec![];
    while let Some(packet) = owner.try_pop().unwrap() {
        result.push(packet);
    }
    result
}
fn push(writer: &mut PerformanceWriter, packets: &[PerformancePacket]) {
    for p in packets {
        writer.push_packet(*p).unwrap();
    }
}
fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn token() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
pub(super) fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                out.insert(relative, None);
                visit(root, &path, out);
            } else {
                out.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    if root.exists() {
        visit(root, root, &mut out);
    }
    out
}
pub(super) fn projects(
    backend: &mut NativeBackend,
) -> Vec<(omniusong_store::ProjectSummary, Vec<u8>)> {
    backend
        .store()
        .unwrap()
        .list()
        .unwrap()
        .into_iter()
        .map(|s| {
            let bytes = backend
                .store()
                .unwrap()
                .load(&s.id)
                .unwrap()
                .to_json()
                .unwrap();
            (s, bytes)
        })
        .collect()
}
pub(super) fn history(backend: &mut NativeBackend) -> PerformanceSourceActionHistory {
    backend
        .store()
        .unwrap()
        .list_performance_source_actions()
        .unwrap()
}
pub(super) fn setup(
    mode: Mode,
) -> (
    Fixture,
    NativeBackend,
    PerformanceSourceReview,
    Vec<u8>,
    PreparedPerformanceSourceActionReceipt,
) {
    let fixture = Fixture::new();
    let mut backend = fixture.backend();
    backend
        .handle(&Command::Create {
            name: "Unchanged project".into(),
        })
        .unwrap();
    fixture.source(mode);
    let Reply::PerformanceSourceInspected(review) = backend
        .handle(&Command::InspectPerformanceSource {
            stage_id: STAGE.into(),
        })
        .unwrap()
    else {
        panic!("source review")
    };
    let raw = serde_json::to_vec_pretty(&review).unwrap();
    let action = super::super::PreparedPerformanceSourceAction::prepare(
        ACTION.into(),
        &raw,
        OperationId("execution-test.operation".into()),
        None,
    )
    .unwrap();
    let Reply::PreparedPerformanceSourceAction(prepared) = backend
        .save_prepared_source_action(None, &action, &raw)
        .unwrap()
    else {
        panic!("Prepared")
    };
    assert_eq!(prepared.action.review_blob_sha256, checksum(&raw));
    assert_eq!(
        prepared.payload_sha256,
        checksum(&serde_json::to_vec(&action).unwrap())
    );
    (fixture, backend, *review, raw, *prepared)
}
pub(super) fn execute(
    backend: &mut NativeBackend,
    prepared: &PreparedPerformanceSourceActionReceipt,
) -> Result<PerformanceSourceExecutionReceipt, ServiceError> {
    let Reply::PerformanceSourceExecution(receipt) = backend.execute_source_action(
        &prepared.action.id,
        prepared.revision,
        &prepared.payload_sha256,
        token(),
    )?
    else {
        panic!("execution")
    };
    Ok(*receipt)
}
fn reconcile(
    backend: &mut NativeBackend,
    prepared: &PreparedPerformanceSourceActionReceipt,
) -> Result<PerformanceSourceExecutionReceipt, ServiceError> {
    let Reply::PerformanceSourceExecution(receipt) = backend.reconcile_source_action(
        &prepared.action.id,
        prepared.revision,
        &prepared.payload_sha256,
        token(),
    )?
    else {
        panic!("reconcile")
    };
    Ok(*receipt)
}
fn request(prepared: &PreparedPerformanceSourceActionReceipt) -> Request<'_> {
    Request {
        id: &prepared.action.id,
        revision: prepared.revision,
        payload_sha256: &prepared.payload_sha256,
    }
}
fn gap(
    backend: &mut NativeBackend,
    prepared: &PreparedPerformanceSourceActionReceipt,
    boundary: Boundary,
) -> ServiceError {
    backend
        .run_source_action(request(prepared), token(), true, |point, _| {
            if point == boundary {
                Err(ServiceError::backend(
                    "Simulated lost acknowledgement",
                    true,
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err()
}
fn append_started(
    backend: &mut NativeBackend,
    prepared: &PreparedPerformanceSourceActionReceipt,
    raw: &[u8],
) -> StoredPerformanceSourceAction {
    let started = PerformanceSourceExecutionRevision {
        version: 1,
        id: prepared.action.id.clone(),
        prepared_payload_sha256: prepared.payload_sha256.clone(),
        phase: PerformanceSourceExecutionPhase::Started,
    };
    backend
        .store()
        .unwrap()
        .save_performance_source_action(
            &prepared.action.id,
            Some(1),
            raw,
            &serde_json::to_vec(&started).unwrap(),
        )
        .unwrap()
}

#[test]
fn actual_writer_prepared_started_verified_retains_all_source_bytes_fault_cut_and_tail() {
    for mode in [Mode::Clean, Mode::Tail, Mode::Fault, Mode::Finish] {
        let (fixture, mut backend, review, raw, prepared) = setup(mode);
        let originals = tree(fixture.source_path().parent().unwrap());
        let project = projects(&mut backend);
        let receipt = execute(&mut backend, &prepared).unwrap();
        assert_eq!(receipt.state.revision, 3);
        assert_eq!(receipt.state.prepared, prepared);
        assert!(!receipt.replayed && receipt.original_reopened);
        assert!(receipt.observed.unknown_tail);
        assert_eq!(
            receipt.observed.evidence_sha256,
            checksum(&fs::read(fixture.source_path()).unwrap())
        );
        assert_eq!(
            receipt.observed.selected_chunk_count,
            review.performance.selection.as_ref().unwrap().chunk_count
        );
        assert!(matches!(
            receipt.state.execution.as_ref().unwrap().phase,
            PerformanceSourceExecutionPhase::Verified { .. }
        ));
        assert_eq!(originals, tree(fixture.source_path().parent().unwrap()));
        let evidence = fixture
            .action_parent()
            .join(ACTION)
            .join("evidence/take.sqlite3");
        assert_eq!(
            fs::read(&evidence).unwrap(),
            fs::read(fixture.source_path()).unwrap()
        );
        assert!(!evidence.with_extension("sqlite3.owner.lock").exists());
        assert_eq!(project, projects(&mut backend));
        let rows = history(&mut backend);
        assert_eq!(rows.reviews[0].payload, raw);
        assert_eq!(
            rows.revisions
                .iter()
                .map(|r| r.revision)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        for row in &rows.revisions {
            assert_eq!(row.payload_sha256, checksum(&row.payload));
        }
        let reader = CaptureReader::open(
            fixture
                .action_parent()
                .join(ACTION)
                .join("recovery/take.sqlite3"),
        )
        .unwrap();
        assert!(
            reader
                .inspection()
                .manifest
                .as_ref()
                .unwrap()
                .body
                .interrupted
                .is_some()
        );
        let streams: BTreeSet<_> = reader
            .inspection()
            .descriptor
            .streams
            .iter()
            .map(|s| s.stream_id.as_str())
            .collect();
        assert_eq!(
            streams,
            ["audio", "gui-1", "gui-2", "gui-3", "gui-4", "control"]
                .into_iter()
                .collect()
        );
        drop(reader);
        if matches!(mode, Mode::Fault) {
            assert!(review.performance.capture_fault.is_some());
        }
        if matches!(mode, Mode::Tail) {
            assert!(
                review.performance.received_generic_chunks > receipt.observed.selected_chunk_count
            );
        }
        if matches!(mode, Mode::Finish) {
            assert!(review.performance.finish_cut.is_some());
        }
        assert!(receipt.usage.work_bytes > review.usage.work_bytes);
        validate_usage(receipt.usage).unwrap();
    }
}

#[test]
fn verified_reopen_and_every_historical_retry_authenticate_artifacts_without_original() {
    let (fixture, mut backend, _, _, prepared) = setup(Mode::Tail);
    let first = execute(&mut backend, &prepared).unwrap();
    let before = history(&mut backend);
    fs::rename(
        fixture.source_path().parent().unwrap(),
        fixture.root.join("removed-original"),
    )
    .unwrap();
    let artifacts = tree(&fixture.action_parent());
    drop(backend);
    let mut backend = fixture.backend();
    for row in &before.revisions {
        for write in [false, true] {
            let Reply::PerformanceSourceExecution(receipt) = if write {
                backend.execute_source_action(ACTION, row.revision, &row.payload_sha256, token())
            } else {
                backend.reconcile_source_action(ACTION, row.revision, &row.payload_sha256, token())
            }
            .unwrap() else {
                panic!("existing proof")
            };
            assert_eq!(receipt.state, first.state);
            assert_eq!(receipt.observed, first.observed);
            assert!(receipt.replayed && !receipt.original_reopened);
        }
    }
    assert_eq!(history(&mut backend), before);
    assert_eq!(tree(&fixture.action_parent()), artifacts);
    assert!(!fixture.source_path().exists());
}

#[test]
fn started_acknowledgement_gap_never_creates_or_restarts_an_action() {
    let (fixture, mut backend, _, _, prepared) = setup(Mode::Clean);
    let original = tree(fixture.source_path().parent().unwrap());
    assert!(gap(&mut backend, &prepared, Boundary::Started).outcome_unknown);
    let before = history(&mut backend);
    assert_eq!(before.revisions.len(), 2);
    assert!(!fixture.action_parent().exists());
    for write in [false, true] {
        let error = if write {
            execute(&mut backend, &prepared)
        } else {
            reconcile(&mut backend, &prepared)
        }
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::SourceUnavailable);
        assert!(error.outcome_unknown);
        assert!(!fixture.action_parent().exists());
    }
    assert_eq!(before, history(&mut backend));
    assert_eq!(original, tree(fixture.source_path().parent().unwrap()));
}

#[test]
fn recovered_commit_gap_reconcile_is_read_only_and_explicit_execute_publishes_verified() {
    let (fixture, mut backend, _, _, prepared) = setup(Mode::Fault);
    assert!(gap(&mut backend, &prepared, Boundary::Recovered).outcome_unknown);
    let before = history(&mut backend);
    assert_eq!(before.revisions.len(), 2);
    let artifacts = tree(&fixture.action_parent());
    fs::rename(
        fixture.source_path().parent().unwrap(),
        fixture.root.join("removed-original"),
    )
    .unwrap();
    let observed = reconcile(&mut backend, &prepared).unwrap();
    assert_eq!(observed.state.revision, 2);
    assert!(matches!(
        observed.state.execution.unwrap().phase,
        PerformanceSourceExecutionPhase::Started
    ));
    assert!(observed.replayed && !observed.original_reopened);
    assert_eq!(before, history(&mut backend));
    let verified = execute(&mut backend, &prepared).unwrap();
    assert_eq!(verified.state.revision, 3);
    assert_eq!(verified.observed, observed.observed);
    assert!(verified.replayed && !verified.original_reopened);
    assert_eq!(artifacts, tree(&fixture.action_parent()));
}

#[test]
fn post_artifact_history_conflict_retains_uncertainty_and_same_result_can_be_reconciled() {
    let (fixture, mut backend, _, raw, prepared) = setup(Mode::Clean);
    let original = tree(fixture.source_path().parent().unwrap());
    let other = super::super::PreparedPerformanceSourceAction::prepare(
        OTHER.into(),
        &raw,
        OperationId("execution-test.other".into()),
        None,
    )
    .unwrap();
    let error = backend
        .run_source_action(request(&prepared), token(), true, |boundary, _| {
            if boundary == Boundary::Recovered {
                let mut concurrent = NativeStore::open_existing(&fixture.paths().native).unwrap();
                concurrent
                    .save_performance_source_action(
                        OTHER,
                        None,
                        &raw,
                        &serde_json::to_vec(&other).unwrap(),
                    )
                    .unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::RevisionConflict);
    assert!(
        error.outcome_unknown,
        "known ledger no-commit does not roll back recovered files"
    );
    let rows = history(&mut backend);
    assert_eq!(rows.revisions.iter().filter(|r| r.id == ACTION).count(), 2);
    let artifacts = tree(&fixture.action_parent());
    assert_eq!(execute(&mut backend, &prepared).unwrap().state.revision, 3);
    assert_eq!(artifacts, tree(&fixture.action_parent()));
    assert_eq!(original, tree(fixture.source_path().parent().unwrap()));
}

#[test]
fn cancellation_and_stale_source_fail_before_started_but_cancellation_after_started_is_uncertain() {
    for case in 0..4 {
        let (fixture, mut backend, _, _, prepared) = setup(Mode::Clean);
        let before = history(&mut backend);
        let project = projects(&mut backend);
        let bytes = fs::read(fixture.source_path()).unwrap();
        let cancelled = token();
        let held = if case == 1 {
            Some(CaptureReader::open(fixture.source_path()).unwrap())
        } else {
            None
        };
        if case == 0 {
            cancelled.store(true, Ordering::Release);
        }
        if case == 2 {
            fs::rename(fixture.source_path(), fixture.root.join("missing.sqlite3")).unwrap();
        }
        if case == 3 {
            fs::OpenOptions::new()
                .append(true)
                .open(fixture.source_path())
                .unwrap()
                .write_all(b"physical stale tail")
                .unwrap();
        }
        let error = backend
            .run_source_action(request(&prepared), cancelled, true, |_, _| Ok(()))
            .unwrap_err();
        assert_eq!(
            error.kind,
            match case {
                0 => FailureKind::InspectionCancelled,
                1 => FailureKind::SourceBusy,
                _ => FailureKind::SourceUnavailable,
            }
        );
        assert!(!error.outcome_unknown);
        assert!(!fixture.action_parent().exists());
        assert_eq!(before, history(&mut backend));
        assert_eq!(project, projects(&mut backend));
        drop(held);
        if case == 3 {
            fs::write(fixture.source_path(), bytes).unwrap();
        }
    }
    let (fixture, mut backend, _, _, prepared) = setup(Mode::Clean);
    let cancelled = token();
    let error = backend
        .run_source_action(
            request(&prepared),
            Arc::clone(&cancelled),
            true,
            |boundary, _| {
                if boundary == Boundary::Started {
                    cancelled.store(true, Ordering::Release);
                }
                Ok(())
            },
        )
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::InspectionCancelled);
    assert!(error.outcome_unknown);
    assert_eq!(history(&mut backend).revisions.len(), 2);
    assert!(!fixture.action_parent().exists());
}

#[test]
fn partial_missing_changed_or_busy_existing_artifacts_are_preserved_and_never_fall_back_to_source()
{
    for case in 0..5 {
        let (fixture, mut backend, _, raw, prepared) = setup(Mode::Clean);
        if case < 2 {
            append_started(&mut backend, &prepared, &raw);
            fs::create_dir(fixture.action_parent()).unwrap();
            let owner = CopyActionOwner::create(&fixture.action_parent(), ACTION).unwrap();
            if case == 1 {
                fs::create_dir_all(owner.paths().action.join("evidence")).unwrap();
                fs::write(&owner.paths().evidence, b"partial evidence retained").unwrap();
            }
        } else {
            gap(&mut backend, &prepared, Boundary::Recovered);
        }
        let action = fixture.action_parent().join(ACTION);
        if case == 2 {
            fs::OpenOptions::new()
                .append(true)
                .open(action.join("evidence/take.sqlite3"))
                .unwrap()
                .write_all(b"changed image")
                .unwrap();
        }
        if case == 3 {
            fs::write(
                action.join("recovery/take.sqlite3-wal"),
                b"sidecar retained",
            )
            .unwrap();
        }
        let owner = if case == 4 {
            Some(CopyActionOwner::open_existing(&fixture.action_parent(), ACTION).unwrap())
        } else {
            None
        };
        let artifacts = tree(&fixture.action_parent());
        let original = tree(fixture.source_path().parent().unwrap());
        let before = history(&mut backend);
        for write in [false, true] {
            let error = if write {
                execute(&mut backend, &prepared)
            } else {
                reconcile(&mut backend, &prepared)
            }
            .unwrap_err();
            assert_eq!(
                error.kind,
                if case == 4 {
                    FailureKind::SourceBusy
                } else {
                    FailureKind::SourceUnavailable
                }
            );
            assert!(error.outcome_unknown);
        }
        assert_eq!(tree(&fixture.action_parent()), artifacts);
        assert_eq!(tree(fixture.source_path().parent().unwrap()), original);
        assert_eq!(history(&mut backend), before);
        drop(owner);
        backend.handle(&Command::Refresh).unwrap();
    }
}

#[test]
fn wrong_historical_request_and_forged_origins_refuse_without_artifact_namespace() {
    let (fixture, mut backend, _, raw, prepared) = setup(Mode::Clean);
    let before = history(&mut backend);
    for (id, revision, hash) in [
        (OTHER, 1, prepared.payload_sha256.clone()),
        (ACTION, 2, prepared.payload_sha256.clone()),
        (ACTION, 1, "a".repeat(64)),
        (ACTION, 4, prepared.payload_sha256.clone()),
    ] {
        let error = backend
            .execute_source_action(id, revision, &hash, token())
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::Rejected);
        assert!(!error.outcome_unknown);
    }
    assert!(reconcile(&mut backend, &prepared).is_err());
    assert_eq!(before, history(&mut backend));
    assert!(!fixture.action_parent().exists());
    let forged = super::super::PreparedPerformanceSourceAction::prepare(
        OTHER.into(),
        &raw,
        OperationId("execution-test.forged-origin".into()),
        Some(PerformanceSourceIntentReference {
            id: LiveId([77; 16]),
            revision: 1,
            payload_sha256: "a".repeat(64),
        }),
    )
    .unwrap();
    let row = backend
        .store()
        .unwrap()
        .save_performance_source_action(OTHER, None, &raw, &serde_json::to_vec(&forged).unwrap())
        .unwrap();
    let error = backend
        .execute_source_action(OTHER, 1, &row.payload_sha256, token())
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::Rejected);
    assert!(!error.outcome_unknown);
    assert!(!fixture.action_parent().exists());
}

#[test]
fn typed_history_rejects_noncanonical_unsupported_and_cross_field_verified_payloads() {
    for case in 0..15 {
        let (fixture, mut backend, review, raw, prepared) = setup(Mode::Clean);
        let started = append_started(&mut backend, &prepared, &raw);
        let selection = review.performance.selection.as_ref().unwrap();
        let result = RecoveredPerformanceSourceResult {
            take_id: review.take_id.clone(),
            descriptor_sha256: review.performance.descriptor_sha256.clone(),
            original_rows_sha256: review.performance.original_rows_sha256.clone(),
            evidence_sha256: review.performance.journal_sha256_at_review.clone(),
            evidence_bytes: review.byte_length,
            working_sha256: "a".repeat(64),
            working_bytes: 4096,
            manifest_sha256: "b".repeat(64),
            selected_chunk_count: selection.chunk_count,
            selected_chunk_index_sha256: selection.chunk_index_sha256.clone(),
            unknown_tail: true,
        };
        let mut verified = PerformanceSourceExecutionRevision {
            version: 1,
            id: ACTION.into(),
            prepared_payload_sha256: prepared.payload_sha256.clone(),
            phase: PerformanceSourceExecutionPhase::Verified {
                result: Box::new(result),
                usage: InspectionUsage::default(),
            },
        };
        let PerformanceSourceExecutionPhase::Verified { result, usage } = &mut verified.phase
        else {
            unreachable!()
        };
        match case {
            0 => verified.version = 2,
            1 => verified.prepared_payload_sha256 = started.payload_sha256,
            2 => verified.id = OTHER.into(),
            3 => result.take_id = "different-take".into(),
            4 => result.descriptor_sha256 = "c".repeat(64),
            5 => result.original_rows_sha256 = "c".repeat(64),
            6 => result.evidence_sha256 = "c".repeat(64),
            7 => result.evidence_bytes += 1,
            8 => result.working_bytes = 536_870_913,
            9 => result.manifest_sha256 = "A".repeat(64),
            10 => result.selected_chunk_count += 1,
            11 => result.selected_chunk_index_sha256 = "c".repeat(64),
            12 => result.unknown_tail = false,
            13 => usage.vm_steps = 32_000_001,
            _ => (),
        }
        let payload = if case == 14 {
            serde_json::to_vec_pretty(&verified).unwrap()
        } else {
            serde_json::to_vec(&verified).unwrap()
        };
        backend
            .store()
            .unwrap()
            .save_performance_source_action(ACTION, Some(2), &raw, &payload)
            .unwrap();
        assert!(
            backend.list_source_action_states().is_err(),
            "forged case {case}"
        );
        assert!(
            execute(&mut backend, &prepared).is_err(),
            "forged case {case}"
        );
        assert!(!fixture.action_parent().exists());
    }
}

#[test]
fn verified_acknowledgement_loss_and_later_missing_artifacts_preserve_recorded_success() {
    let (fixture, mut backend, _, _, prepared) = setup(Mode::Finish);
    // A caller losing this success does not roll back the already committed
    // Verified row. Its historical request authenticates that same result.
    let lost = execute(&mut backend, &prepared).unwrap();
    let before = history(&mut backend);
    drop(backend);
    let mut backend = fixture.backend();
    assert_eq!(execute(&mut backend, &prepared).unwrap().state, lost.state);
    fs::rename(
        fixture.action_parent().join(ACTION),
        fixture.root.join("missing-artifacts"),
    )
    .unwrap();
    let artifacts = tree(&fixture.action_parent());
    let error = reconcile(&mut backend, &prepared).unwrap_err();
    assert_eq!(error.kind, FailureKind::SourceUnavailable);
    assert!(error.outcome_unknown);
    assert_eq!(history(&mut backend), before);
    assert_eq!(tree(&fixture.action_parent()), artifacts);
    let Reply::PerformanceSourceActionStates(states) = backend.list_source_action_states().unwrap()
    else {
        panic!("states")
    };
    assert_eq!(states, [lost.state]);
}

#[test]
fn shared_parent_creation_race_accepts_only_revalidated_ordinary_directory() {
    for redirected in [false, true] {
        let fixture = Fixture::new();
        let _backend = fixture.backend();
        let root = fixture.paths().native;
        let result = parent_directory_using(&root, true, |parent| {
            if redirected {
                let target = fixture.root.join("redirected");
                fs::create_dir(&target).unwrap();
                std::os::unix::fs::symlink(target, parent).unwrap();
            } else {
                fs::create_dir(parent).unwrap();
            }
            Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
        });
        if redirected {
            assert_eq!(result.unwrap_err().kind, FailureKind::SourceUnavailable);
        } else {
            assert_eq!(result.unwrap(), fixture.action_parent());
        }
        assert!(tree(&fixture.action_parent()).is_empty());
    }
}

#[test]
fn reserved_recovery_operation_cannot_be_newly_claimed_but_historical_prepared_replay_is_exact() {
    let (fixture, mut backend, _, raw, _) = setup(Mode::Clean);
    let old = super::super::PreparedPerformanceSourceAction::prepare(
        OTHER.into(),
        &raw,
        OperationId(format!("os-src-attach:{}", "a".repeat(64))),
        None,
    )
    .unwrap();
    let before = history(&mut backend);
    let original = tree(fixture.source_path().parent().unwrap());
    assert!(
        backend
            .save_prepared_source_action(None, &old, &raw)
            .is_err()
    );
    assert_eq!(history(&mut backend), before);
    // A literal immutable row admitted before this namespace existed. This is
    // metadata replay compatibility, not a new permission to publish anything.
    let saved = backend
        .store()
        .unwrap()
        .save_performance_source_action(OTHER, None, &raw, &serde_json::to_vec(&old).unwrap())
        .unwrap();
    let all = history(&mut backend);
    let Reply::PreparedPerformanceSourceAction(replayed) = backend
        .save_prepared_source_action(None, &old, &raw)
        .unwrap()
    else {
        panic!("historical Prepared replay")
    };
    assert_eq!(replayed.action, old);
    assert_eq!(replayed.payload_sha256, saved.payload_sha256);
    assert_eq!(history(&mut backend), all);
    assert_eq!(tree(fixture.source_path().parent().unwrap()), original);
}
