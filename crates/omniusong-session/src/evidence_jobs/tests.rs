//! Actual isolated OSRE jobs and retained metadata. No native devices or data.
#![cfg(unix)]
use super::journal::EvidenceJournal;
use super::runner::{EvidenceRunBoundary, current_catalog_retirement};
use super::*;
use crate::{LegacyScan, NativeBackend, Paths, Reply};
use omniusong_core::ProjectDocument;
use omniusong_recording::{InspectionLimits, RecordingError};
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
const JOB: &str = "11111111111111111111111111111111";
const OTHER: &str = "22222222222222222222222222222222";
fn token() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
fn budget() -> InspectionBudget {
    InspectionBudget::new(InspectionLimits::default(), token()).unwrap()
}
struct Fixture {
    root: PathBuf,
    backend: NativeBackend,
    project_id: String,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/evidence-job-runner-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut store = omniusong_store::NativeStore::open(root.join("native")).unwrap();
        let document = store
            .create(&ProjectDocument::new("Preserved synthetic song").unwrap())
            .unwrap();
        fs::create_dir_all(store.root().join("captures/opaque/empty")).unwrap();
        fs::write(
            store.root().join("captures/opaque/original.raw"),
            b"all retained synthetic bytes\0\xff",
        )
        .unwrap();
        let paths = Paths {
            native: store.root().to_owned(),
            legacy: root.join("legacy"),
        };
        Self {
            root,
            backend: NativeBackend {
                paths,
                store: Some(store),
                legacy: LegacyScan::default(),
                legacy_notice: None,
                require_existing: true,
            },
            project_id: document.id().into(),
        }
    }
    fn request(&self, id: &str, name: &str) -> EvidenceJobRequest {
        EvidenceJobRequest {
            version: 1,
            job_id: id.into(),
            origin_root: self.backend.paths.native.to_str().unwrap().into(),
            operation: EvidenceOperation::Export {
                scope: omniusong_store::EVIDENCE_SCOPE.into(),
                export_id: id.into(),
                destination: self.root.join(name),
            },
        }
    }
    fn view(&mut self, id: &str) -> EvidenceJobView {
        let Reply::EvidenceJob(view) = self.backend.read_evidence_job(id, token()).unwrap() else {
            panic!("job view")
        };
        *view
    }
    fn run(&mut self, request: &EvidenceJobRequest) -> EvidenceJobReply {
        let Reply::EvidenceJobOutcome(reply) =
            self.backend.run_evidence_job(request, token()).unwrap()
        else {
            panic!("job reply")
        };
        *reply
    }
    fn document(&mut self) -> serde_json::Value {
        self.backend
            .store()
            .unwrap()
            .load(&self.project_id)
            .unwrap()
            .value()
            .clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Evidence job fixture retained: {}", self.root.display())
        } else {
            self.backend.store.take();
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn files(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result)
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(path, path, &mut result);
    result
}
fn reply(result: Result<Reply, ServiceError>) -> EvidenceJobReply {
    let Reply::EvidenceJobOutcome(reply) = result.unwrap() else {
        panic!("outcome")
    };
    *reply
}

#[test]
fn exact_export_job_and_reviewed_restore_reconcile_after_restart_with_originals_absent() {
    let mut f = Fixture::new();
    let document = f.document();
    let raw = files(&f.backend.paths.native.join("captures"));
    let export = f.request(JOB, "complete.osre");
    let first = f.run(&export);
    assert!(
        first.attempted && first.metadata_problem.is_none() && first.catalog_retirement.is_none()
    );
    let Some(EvidenceOutcome::Exported { receipt }) = first.outcome else {
        panic!("actual export")
    };
    assert_eq!(
        receipt.sha256,
        format!("{:x}", Sha256::digest(fs::read(&receipt.path).unwrap()))
    );
    let view = f.view(JOB);
    assert!(view.started && view.terminal.as_ref().unwrap().completed());
    let restore = EvidenceJobRequest {
        version: 1,
        job_id: OTHER.into(),
        origin_root: export.origin_root.clone(),
        operation: EvidenceOperation::Restore {
            archive_path: receipt.path.clone(),
            directory: f.root.join("inert"),
            expected_archive_sha256: receipt.sha256.clone(),
        },
    };
    let restored = f.run(&restore);
    assert!(restored.outcome.as_ref().unwrap().completed() && restored.metadata_problem.is_none());
    assert_eq!(f.document(), document);
    assert_eq!(files(&f.backend.paths.native.join("captures")), raw);
    let restore_before = files(&f.root.join("inert"));
    let restore_view = f.view(OTHER);
    f.backend.store.take();
    fs::rename(&f.backend.paths.native, f.root.join("retained-native-away")).unwrap();
    assert!(!f.backend.paths.native.exists());
    // Export reconciliation and historical replay require neither a source store
    // nor an Initialize command, and cannot create its missing root.
    let export_reconciled = reply(f.backend.reconcile_evidence_job(
        JOB,
        view.request_sha256.as_ref().unwrap(),
        &view.history_sha256,
        token(),
    ));
    assert!(export_reconciled.outcome.as_ref().unwrap().completed());
    assert!(export_reconciled.catalog_retirement.is_none());
    let existing = f.run(&export);
    assert!(!existing.attempted && existing.catalog_retirement.is_none());
    fs::rename(&receipt.path, f.root.join("original-archive-away.osre")).unwrap();
    let restore_reconciled = reply(f.backend.reconcile_evidence_job(
        OTHER,
        restore_view.request_sha256.as_ref().unwrap(),
        &restore_view.history_sha256,
        token(),
    ));
    assert!(
        restore_reconciled.outcome.as_ref().unwrap().completed()
            && restore_reconciled.metadata_problem.is_none()
    );
    assert_eq!(files(&f.root.join("inert")), restore_before);
    assert!(!receipt.path.exists() && !f.backend.paths.native.exists());
}

#[test]
fn request_only_and_started_jobs_never_execute_on_repeat_or_metadata_restart() {
    for boundary in [
        EvidenceRunBoundary::RequestPersisted,
        EvidenceRunBoundary::StartedPersisted,
    ] {
        let mut f = Fixture::new();
        let request = f.request(JOB, "never-executed.osre");
        let mut reached = false;
        let error = f
            .backend
            .run_evidence_job_using(&request, &budget(), |at| {
                if at == boundary {
                    reached = true;
                    Err(ServiceError::backend(
                        "synthetic lost acknowledgement",
                        true,
                    ))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(reached && error.outcome_unknown);
        let view = f.view(JOB);
        assert_eq!(
            view.started,
            boundary == EvidenceRunBoundary::StartedPersisted
        );
        assert!(view.terminal.is_none());
        let before = files(&view.directory);
        f.backend.store.take();
        fs::rename(&f.backend.paths.native, f.root.join("native-away")).unwrap();
        assert!(!f.run(&request).attempted);
        assert_eq!(files(&view.directory), before);
        let Reply::EvidenceJobs(list) = f.backend.list_evidence_jobs(token()).unwrap() else {
            panic!("list")
        };
        assert_eq!(list.jobs.len(), 1);
        assert!(!request.destination().exists() && !f.backend.paths.native.exists());
        let reconciled = reply(f.backend.reconcile_evidence_job(
            JOB,
            view.request_sha256.as_ref().unwrap(),
            &view.history_sha256,
            token(),
        ));
        assert!(matches!(
            reconciled.outcome,
            Some(EvidenceOutcome::Stopped { .. })
        ));
        assert!(!request.destination().exists());
    }
}

#[test]
fn terminal_metadata_failure_retains_real_artifact_and_reconciles_without_reexecution() {
    let mut f = Fixture::new();
    let request = f.request(JOB, "completed-before-terminal.osre");
    let mut reached = false;
    let result = reply(f.backend.run_evidence_job_using(&request, &budget(), |at| {
        if at == EvidenceRunBoundary::BeforeTerminal {
            reached = true;
            Err(ServiceError::backend(
                "synthetic terminal-write refusal",
                false,
            ))
        } else {
            Ok(())
        }
    }));
    assert!(
        reached
            && result.attempted
            && result.outcome.as_ref().unwrap().completed()
            && result.metadata_problem.is_some()
            && result.history_sha256.is_none()
    );
    let archive = fs::read(request.destination()).unwrap();
    let view = f.view(JOB);
    assert!(view.started && view.terminal.is_none());
    let second = f.run(&request);
    assert!(!second.attempted && second.outcome.is_none());
    assert_eq!(fs::read(request.destination()).unwrap(), archive);
    let reconciled = reply(f.backend.reconcile_evidence_job(
        JOB,
        view.request_sha256.as_ref().unwrap(),
        &view.history_sha256,
        token(),
    ));
    assert!(reconciled.outcome.as_ref().unwrap().completed());
    assert_eq!(fs::read(request.destination()).unwrap(), archive);
    let after = f.view(JOB);
    assert!(after.terminal.is_none());
    assert_eq!(after.observations.len(), 1);
}

#[test]
fn cancellation_after_actual_export_return_preserves_its_successful_terminal() {
    let mut f = Fixture::new();
    let before = f.document();
    let request = f.request(JOB, "completed-before-cancellation.osre");
    let cancelled = token();
    let meter = InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
    let mut reached = false;
    let result = reply(f.backend.run_evidence_job_using(&request, &meter, |at| {
        if at == EvidenceRunBoundary::BeforeTerminal {
            reached = true;
            cancelled.store(true, Ordering::Release);
        }
        Ok(())
    }));
    assert!(reached && result.attempted && result.metadata_problem.is_none());
    assert!(result.outcome.as_ref().unwrap().completed());
    assert!(matches!(
        meter.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    let inspection =
        omniusong_store::inspect_evidence_archive(request.destination(), &budget()).unwrap();
    let EvidenceOutcome::Exported { receipt } = result.outcome.as_ref().unwrap() else {
        panic!("actual export success must survive late cancellation")
    };
    assert_eq!(receipt, &inspection.receipt);
    let view = f.view(JOB);
    assert_eq!(view.terminal, result.outcome);
    assert_eq!(Some(view.history_sha256), result.history_sha256);
    let archive = fs::read(request.destination()).unwrap();
    let replay = f.run(&request);
    assert!(!replay.attempted && replay.metadata_problem.is_none());
    assert_eq!(replay.outcome, result.outcome);
    assert_eq!(fs::read(request.destination()).unwrap(), archive);
    assert_eq!(f.document(), before);
}

#[test]
fn sticky_cancellation_can_persist_one_terminal_and_one_reconciliation_observation() {
    let mut f = Fixture::new();
    let request = f.request(JOB, "cancel-before-artifact.osre");
    let cancelled = token();
    let meter = InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
    let result = reply(f.backend.run_evidence_job_using(&request, &meter, |at| {
        if at == EvidenceRunBoundary::StartedPersisted {
            cancelled.store(true, Ordering::Release);
        }
        Ok(())
    }));
    assert!(!result.attempted && result.metadata_problem.is_none());
    assert!(matches!(
        result.outcome,
        Some(EvidenceOutcome::Stopped {
            problem: EvidenceProblem {
                budget_stop: Some(EvidenceBudgetStop::Cancelled),
                ..
            }
        })
    ));
    assert!(matches!(
        meter.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert!(!request.destination().exists());
    let view = f.view(JOB);
    assert!(matches!(
        view.terminal,
        Some(EvidenceOutcome::Stopped { .. })
    ));
    let journal = EvidenceJournal::open(&f.backend.paths.native, false, &budget())
        .unwrap()
        .unwrap();
    let entry = journal.read(JOB, &budget()).unwrap();
    let observation_cancelled = token();
    let stopped_budget = InspectionBudget::new(
        InspectionLimits::default(),
        Arc::clone(&observation_cancelled),
    )
    .unwrap();
    let permit = journal.reserve_finalization(&stopped_budget).unwrap();
    observation_cancelled.store(true, Ordering::Release);
    assert!(stopped_budget.checkpoint().is_err());
    let outcome = EvidenceOutcome::Stopped {
        problem: EvidenceProblem {
            kind: EvidenceProblemKind::Cancelled,
            origin: EvidenceResourceOrigin::Evidence,
            detail: EvidenceMessage::new("cancelled"),
            sqlite_code: None,
            budget_stop: Some(EvidenceBudgetStop::Cancelled),
        },
    };
    let observed = journal.observe(entry, &outcome, permit).unwrap();
    assert_eq!(observed.view.occupied_observations, 1);
}

#[test]
fn exact_request_history_and_destination_fences_preserve_existing_jobs() {
    let mut f = Fixture::new();
    let request = f.request(JOB, "fenced.osre");
    f.run(&request);
    let view = f.view(JOB);
    let before = files(&view.directory);
    let mut changed = request.clone();
    if let EvidenceOperation::Export { destination, .. } = &mut changed.operation {
        *destination = f.root.join("changed.osre")
    };
    assert!(f.backend.run_evidence_job(&changed, token()).is_err());
    assert_eq!(files(&view.directory), before);
    assert!(!changed.destination().exists());
    for (request_sha, history_sha) in [
        ("a".repeat(64), view.history_sha256.clone()),
        (view.request_sha256.clone().unwrap(), "b".repeat(64)),
    ] {
        let error = f
            .backend
            .reconcile_evidence_job(JOB, &request_sha, &history_sha, token())
            .unwrap_err();
        assert!(error.outcome_unknown);
        assert_eq!(files(&view.directory), before);
    }
    let mut other = f.request(OTHER, "unused.osre");
    if let EvidenceOperation::Export { destination, .. } = &mut other.operation {
        *destination = request.destination().to_owned()
    };
    assert!(f.backend.run_evidence_job(&other, token()).is_err());
    assert_eq!(files(&view.directory), before);
}

#[test]
fn overflow_receipts_strict_operations_and_later_partial_observations_refuse_false_claims() {
    let f = Fixture::new();
    let request = f.request(JOB, "overflow.osre");
    for (files, dirs) in [(u64::MAX, 1), (1, u64::MAX), (u64::MAX, u64::MAX)] {
        let outcome = EvidenceOutcome::Exported {
            receipt: EvidenceArchiveReceipt {
                path: request.destination().to_owned(),
                export_id: JOB.into(),
                sha256: "a".repeat(64),
                byte_length: 4096,
                file_count: files,
                directory_count: dirs,
                payload_bytes: 1024,
            },
        };
        assert!(outcome.validate_for(&request).is_err());
    }
    let mut operation = serde_json::to_value(&request.operation).unwrap();
    operation["unrecognized"] = true.into();
    assert!(serde_json::from_value::<EvidenceOperation>(operation).is_err());
    assert!(
        serde_json::from_value::<EvidenceBudgetStop>(serde_json::json!({"cancelled":{"extra":1}}))
            .is_err()
    );
    let exported = EvidenceOutcome::Exported {
        receipt: EvidenceArchiveReceipt {
            path: request.destination().to_owned(),
            export_id: JOB.into(),
            sha256: "a".repeat(64),
            byte_length: 4096,
            file_count: 1,
            directory_count: 0,
            payload_bytes: 1024,
        },
    };
    let view = EvidenceJobView {
        job_id: JOB.into(),
        directory: f.root.join("metadata"),
        request: Some(request.clone()),
        request_sha256: Some(request.fingerprint().unwrap()),
        history_sha256: "b".repeat(64),
        started: true,
        terminal: Some(exported.clone()),
        observations: vec![EvidenceObservation {
            sequence: 1,
            previous_history_sha256: "c".repeat(64),
            outcome: exported,
        }],
        occupied_observations: 2,
        metadata_problems: vec![],
    };
    assert_eq!(
        view.summary().latest_observation,
        Some(EvidenceObservationStatus::Unresolved)
    );
    assert!(view.latest_recorded_outcome().is_none());
    assert_eq!(view.summary().phase, EvidenceJobPhase::HistoricalComplete);
}

#[test]
fn only_current_live_or_mixed_fatal_codes_can_request_worker_retirement() {
    for origin in [
        EvidenceResourceOrigin::Evidence,
        EvidenceResourceOrigin::LiveCatalog,
        EvidenceResourceOrigin::BackupTransfer,
    ] {
        for code in [5, 10, 11, 26, 10 | (8 << 8)] {
            let result = current_catalog_retirement(
                origin,
                Some(code),
                &EvidenceMessage::new("message never determines origin"),
            );
            assert_eq!(
                result.is_some(),
                origin != EvidenceResourceOrigin::Evidence && crate::fatal_sqlite_primary(code)
            );
        }
    }
    assert!(
        current_catalog_retirement(
            EvidenceResourceOrigin::LiveCatalog,
            None,
            &EvidenceMessage::new("SQLITE_CORRUPT")
        )
        .is_none()
    );
}

#[test]
fn archive_diagnostic_mapper_honors_explicit_sql_origin_inside_nested_failure() {
    use omniusong_store::{
        ArchiveAction, ArchiveContext, ArchiveFailure, ArchiveSqlFailure, ArchiveSqlOrigin,
        StoreError,
    };
    for origin in [
        ArchiveSqlOrigin::Input,
        ArchiveSqlOrigin::PrivateOutput,
        ArchiveSqlOrigin::LiveCatalog,
    ] {
        let error = StoreError::Archive(Box::new(ArchiveFailure::new(
            ArchiveContext::new(ArchiveAction::Inspect, Path::new("isolated archive")),
            StoreError::ArchiveSql(Box::new(ArchiveSqlFailure::new(
                origin,
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(26),
                    Some("explicit diagnostic constructor".into()),
                ),
            ))),
        )));
        let problem =
            super::runner::problem_from_store(&error, EvidenceResourceOrigin::BackupTransfer, None);
        assert_eq!(problem.kind, EvidenceProblemKind::Sqlite);
        assert_eq!(problem.sqlite_code, Some(26));
        let expected = if origin == ArchiveSqlOrigin::LiveCatalog {
            EvidenceResourceOrigin::LiveCatalog
        } else {
            EvidenceResourceOrigin::Evidence
        };
        assert_eq!(problem.origin, expected);
        assert_eq!(
            current_catalog_retirement(problem.origin, problem.sqlite_code, &problem.detail)
                .is_some(),
            origin == ArchiveSqlOrigin::LiveCatalog
        );
        assert!(problem.detail.message.contains("destination="));
    }
}

fn corrupt_archived_catalog(path: &Path, destination: &Path) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"OSRE\x01\r\n\0");
    let length = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let mut manifest: omniusong_store::EvidenceManifest =
        serde_json::from_slice(&bytes[16..16 + length]).unwrap();
    let mut offset = 16 + length;
    let mut bodies = Vec::new();
    for entry in &mut manifest.entries {
        if entry.kind != omniusong_store::EvidenceEntryKind::File {
            continue;
        }
        let mut body = bytes[offset..offset + entry.byte_length as usize].to_vec();
        offset += body.len();
        if entry.path == "catalog.sqlite3" {
            body[100] = 255;
            entry.sha256 = Some(format!("{:x}", Sha256::digest(&body)));
        }
        bodies.push(body);
    }
    let json = serde_json::to_vec(&manifest).unwrap();
    let mut output = b"OSRE\x01\r\n\0".to_vec();
    output.extend_from_slice(&(json.len() as u64).to_le_bytes());
    output.extend_from_slice(&json);
    for body in bodies {
        output.extend_from_slice(&body)
    }
    let digest = Sha256::digest(&output);
    output.extend_from_slice(&digest);
    output.extend_from_slice(b"OSREEND1");
    fs::write(destination, output).unwrap();
}
#[test]
fn actual_session_worker_runs_jobs_and_keeps_archived_sqlite_corruption_local() {
    use crate::{Command, Completion, SessionService};
    use std::time::{Duration, Instant};
    fn completed(service: &mut SessionService, command: Command) -> Completion {
        let id = service.try_submit(command).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(completion) = service.try_poll().unwrap() {
                assert_eq!(completion.id, id);
                return completion;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let mut f = Fixture::new();
    let document = f.document();
    let mut paths = f.backend.paths.clone();
    let canonical_origin = paths.native.clone();
    paths.native = canonical_origin.join("..").join("native");
    f.backend.store.take();
    let mut service = SessionService::reconnect(paths).unwrap();
    let Reply::Library(library) = completed(&mut service, Command::Initialize).result.unwrap()
    else {
        panic!("initialized library")
    };
    assert_eq!(
        library.native_root.as_deref(),
        Some(canonical_origin.as_path())
    );
    let request = f.request(JOB, "actual-worker.osre");
    let result = reply(
        completed(
            &mut service,
            Command::RunEvidenceJob {
                request: Box::new(request.clone()),
            },
        )
        .result,
    );
    assert!(
        result.attempted
            && result.metadata_problem.is_none()
            && result.catalog_retirement.is_none()
    );
    let Some(EvidenceOutcome::Exported { receipt }) = result.outcome else {
        panic!("real exported artifact")
    };
    let Reply::EvidenceJob(view) = completed(
        &mut service,
        Command::ReadEvidenceJob { job_id: JOB.into() },
    )
    .result
    .unwrap() else {
        panic!("job metadata")
    };
    let inspected = completed(
        &mut service,
        Command::InspectRetainedEvidence {
            path: receipt.path.clone(),
        },
    )
    .result
    .unwrap();
    assert!(
        matches!(inspected,Reply::RetainedEvidenceInspected(inspection)if inspection.receipt.sha256==receipt.sha256)
    );
    let corrupt = f.root.join("rehashed-invalid-sqlite.osre");
    corrupt_archived_catalog(&receipt.path, &corrupt);
    let error = completed(
        &mut service,
        Command::InspectRetainedEvidence {
            path: corrupt.clone(),
        },
    )
    .result
    .unwrap_err();
    assert_eq!(error.kind, FailureKind::SourceUnavailable);
    assert!(!error.outcome_unknown && !service.is_disconnected());
    assert!(matches!(
        completed(&mut service, Command::Refresh).result,
        Ok(Reply::Library(_))
    ));
    let reconciled = reply(
        completed(
            &mut service,
            Command::ReconcileEvidenceJob {
                job_id: JOB.into(),
                expected_request_sha256: view.request_sha256.clone().unwrap(),
                expected_history_sha256: view.history_sha256.clone(),
            },
        )
        .result,
    );
    assert!(
        reconciled.attempted
            && reconciled.outcome.as_ref().unwrap().completed()
            && reconciled.catalog_retirement.is_none()
    );
    let Reply::Document { view, .. } = completed(
        &mut service,
        Command::Open {
            project_id: omniusong_core::contracts::ProjectId(f.project_id.clone()),
        },
    )
    .result
    .unwrap() else {
        panic!("healthy original document")
    };
    assert_eq!(view.document.value(), &document);
    assert!(!service.is_disconnected());
    drop(service);
}
