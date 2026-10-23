//! Injected channel/token composition only. These tests do not execute an
//! archive, corrupt a catalog or qualify a native device/workflow.
use super::*;
use crate::project_archives::{from_export, from_failure};
use omniusong_store::{
    ArchiveCompletion, ArchiveFailure, ArchiveOutcome, ArchivePhase, StoreError,
};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn request() -> ProjectArchiveRequest {
    ProjectArchiveRequest {
        version: 1,
        origin_root: "/synthetic/native".into(),
        operation: ProjectArchiveOperation::Export {
            project_id: ProjectId("11111111-1111-4111-8111-111111111111".into()),
            destination: "/synthetic/complete.osar".into(),
        },
    }
}
fn command(request: ProjectArchiveRequest) -> Command {
    Command::RunProjectArchive {
        request: Box::new(request),
    }
}
fn exported(request: &ProjectArchiveRequest) -> ProjectArchiveReply {
    let ProjectArchiveOperation::Export {
        project_id,
        destination,
    } = &request.operation
    else {
        panic!("export fixture")
    };
    from_export(
        &request.fingerprint().unwrap(),
        ArchiveOutcome::new(
            ArchiveReceipt {
                path: destination.clone(),
                project_id: project_id.0.clone(),
                sha256: "a".repeat(64),
                byte_length: 4096,
                history_rows: 1,
                operation_rows: 1,
                asset_count: 0,
                take_count: 0,
            },
            ArchiveCompletion::ExportDurable,
        ),
    )
}
fn response(id: RequestId, reply: ProjectArchiveReply) -> Completion {
    Completion {
        id,
        kind: CommandKind::RunProjectArchive,
        result: Ok(Reply::ProjectArchive(Box::new(reply))),
    }
}
fn poll(service: &mut SessionService) -> Completion {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = service.try_poll().unwrap() {
            return value;
        }
        assert!(Instant::now() < until, "held completion timed out");
        std::thread::yield_now();
    }
}

#[test]
fn foreign_archive_reference_keeps_credit_until_exact_reference_arrives() {
    let (mut service, peer) = SessionService::test_pair();
    let request = request();
    let id = service.try_submit(command(request.clone())).unwrap();
    let _ = peer.requests.recv().unwrap();
    let mut foreign = exported(&request);
    foreign.request_sha256 = "f".repeat(64);
    peer.responses.send(response(id, foreign)).unwrap();
    let error = service.try_poll().unwrap_err();
    assert_eq!(error.kind, FailureKind::Protocol);
    assert!(error.outcome_unknown);
    assert_eq!(service.pending().unwrap().id, id);
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Busy
    );
    peer.responses
        .send(response(id, exported(&request)))
        .unwrap();
    assert!(matches!(
        service.try_poll().unwrap().unwrap().result,
        Ok(Reply::ProjectArchive(_))
    ));
    assert!(!service.is_busy());
    assert!(service.retry_command().is_none());
}

#[test]
fn matching_archive_reference_bad_body_consumes_credit_as_uncertain_protocol() {
    let (mut service, peer) = SessionService::test_pair();
    let request = request();
    let id = service.try_submit(command(request.clone())).unwrap();
    let _ = peer.requests.recv().unwrap();
    let mut bad = exported(&request);
    if let ProjectArchiveOutcome::Exported { receipt, .. } = &mut bad.outcome {
        receipt.project_id = ProjectId("other-project".into());
    }
    peer.responses.send(response(id, bad)).unwrap();
    let error = service.try_poll().unwrap().unwrap().result.unwrap_err();
    assert_eq!(error.kind, FailureKind::Protocol);
    assert!(error.outcome_unknown);
    assert!(!service.is_busy());
    assert!(!service.is_disconnected());
    assert!(service.retry_command().is_none());
    assert!(service.try_submit(Command::Refresh).is_ok());
}

#[test]
fn matching_invalid_body_cannot_erase_an_independent_catalog_retirement() {
    let (mut service, peer) = SessionService::test_pair();
    let request = request();
    let id = service.try_submit(command(request.clone())).unwrap();
    let _ = peer.requests.recv().unwrap();
    let mut bad = exported(&request);
    // Deliberate constructor control: success plus retirement is incoherent,
    // but a known fatal core must survive rejecting the matching body.
    bad.catalog_retirement = Some(ProjectArchiveCatalogRetirement {
        sqlite_code: Some(11),
        connection_unusable: false,
        detail: ProjectArchiveMessage {
            text: "oversized diagnostic".repeat(1000),
            truncated: false,
        },
    });
    peer.responses.send(response(id, bad)).unwrap();
    let error = service.try_poll().unwrap().unwrap().result.unwrap_err();
    assert_eq!(error.kind, FailureKind::Protocol);
    assert!(error.outcome_unknown);
    assert!(service.is_disconnected());
    assert!(!service.is_busy());
    let retired = service.try_poll().unwrap_err();
    assert_eq!(retired.kind, FailureKind::StorageUnavailable);
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Disconnected
    );
}

#[test]
fn malformed_matching_reply_cannot_omit_its_explicit_live_fatal_retirement() {
    use omniusong_store::{ArchiveSqlFailure, ArchiveSqlOrigin};
    let (mut service, peer) = SessionService::test_pair();
    let request = request();
    let id = service.try_submit(command(request.clone())).unwrap();
    let _ = peer.requests.recv().unwrap();
    let error = StoreError::ArchiveSql(Box::new(ArchiveSqlFailure::new(
        ArchiveSqlOrigin::LiveCatalog,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(11),
            Some("explicit live fatal constructor control".into()),
        ),
    )));
    let mut reply = from_failure(
        &request.fingerprint().unwrap(),
        ArchiveFailure::new(request.preflight_context(), error),
        true,
    );
    assert!(reply.catalog_retirement.is_some());
    reply.catalog_retirement = None;
    assert!(reply.validate_for(&request).is_err());
    peer.responses.send(response(id, reply)).unwrap();
    assert_eq!(
        service
            .try_poll()
            .unwrap()
            .unwrap()
            .result
            .unwrap_err()
            .kind,
        FailureKind::Protocol
    );
    assert!(service.is_disconnected());
    assert_eq!(
        service.try_poll().unwrap_err().kind,
        FailureKind::StorageUnavailable
    );
}

#[test]
fn lost_nonretryable_archive_retains_uncertainty_on_every_disconnected_poll() {
    let (mut service, peer) = SessionService::test_pair();
    let _ = service.try_submit(command(request())).unwrap();
    let _ = peer.requests.recv().unwrap();
    drop(peer);
    for _ in 0..3 {
        let error = service.try_poll().unwrap_err();
        assert_eq!(error.kind, FailureKind::Disconnected);
        assert!(error.outcome_unknown);
        assert!(service.retry_command().is_none());
    }
}

#[test]
fn a_recorded_copy_refusal_is_delivered_without_generic_mutation_uncertainty_or_retry() {
    let (mut service, peer) = SessionService::test_pair();
    let request = ProjectArchiveRequest {
        version: 1,
        origin_root: "/synthetic/native".into(),
        operation: ProjectArchiveOperation::RestoreCopy {
            archive_path: "/synthetic/absent.osar".into(),
            intent: RestoreCopyIntent {
                operation_id: OperationId("recorded-copy".into()),
                expected_archive_sha256: "a".repeat(64),
            },
        },
    };
    let id = service.try_submit(command(request.clone())).unwrap();
    let _ = peer.requests.recv().unwrap();
    let mut context = request.preflight_context();
    context.phase = ArchivePhase::RecordedReplay;
    let receipt = from_failure(
        &request.fingerprint().unwrap(),
        ArchiveFailure::new(
            context,
            StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "injected missing owned original",
            )),
        ),
        true,
    );
    receipt.validate_for(&request).unwrap();
    peer.responses.send(response(id, receipt)).unwrap();
    let Reply::ProjectArchive(result) = service.try_poll().unwrap().unwrap().result.unwrap() else {
        panic!("typed result")
    };
    let ProjectArchiveOutcome::Failed { context, .. } = result.outcome else {
        panic!("recorded refusal")
    };
    assert_eq!(context.phase, ProjectArchivePhase::RecordedReplay);
    assert!(!service.is_busy());
    assert!(!service.is_disconnected());
    assert!(service.retry_command().is_none());
}

type Tokens = [Arc<AtomicBool>; 4];
struct Held {
    entered: mpsc::Sender<Tokens>,
    release: mpsc::Receiver<()>,
    done: mpsc::Sender<()>,
}
impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.done.send(());
    }
}
impl Backend for Held {
    fn handle(&mut self, _: &Command) -> Result<Reply, ServiceError> {
        panic!("requires token dispatch")
    }
    fn handle_with_project_archive_cancellation(
        &mut self,
        command: &Command,
        inspection: &Arc<AtomicBool>,
        source: &Arc<AtomicBool>,
        evidence: &Arc<AtomicBool>,
        archive: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.entered
            .send([
                Arc::clone(inspection),
                Arc::clone(source),
                Arc::clone(evidence),
                Arc::clone(archive),
            ])
            .unwrap();
        self.release.recv_timeout(Duration::from_secs(5)).unwrap();
        let Command::RunProjectArchive { request } = command else {
            panic!("archive command")
        };
        // Inject a completed result after cancellation; cancellation cannot
        // replace independently reported completion at the service boundary.
        Ok(Reply::ProjectArchive(Box::new(exported(request))))
    }
}
fn held() -> (
    SessionService,
    mpsc::Receiver<Tokens>,
    mpsc::Sender<()>,
    mpsc::Receiver<()>,
) {
    let (entered, tokens) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    (
        SessionService::spawn_with_backend(Held {
            entered,
            release: wait,
            done,
        })
        .unwrap(),
        tokens,
        release,
        finished,
    )
}

#[test]
fn exact_archive_cancel_has_its_own_token_and_preserves_credit_and_late_completion() {
    let (mut service, tokens, release, finished) = held();
    let id = service.try_submit(command(request())).unwrap();
    let first = tokens.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!service.cancel_project_archive(RequestId {
        sequence: id.sequence + 1,
        ..id
    }));
    assert!(!service.cancel_source_inspection(id));
    assert!(!service.cancel_source_action(id));
    assert!(!service.cancel_evidence_job(id));
    assert!(service.cancel_project_archive(id));
    assert!(first[3].load(Ordering::Acquire));
    assert!(
        first[..3]
            .iter()
            .all(|token| !token.load(Ordering::Acquire))
    );
    assert!(service.try_poll().unwrap().is_none());
    assert!(service.is_busy());
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Busy
    );
    release.send(()).unwrap();
    let Reply::ProjectArchive(result) = poll(&mut service).result.unwrap() else {
        panic!("late completion")
    };
    assert!(matches!(
        result.outcome,
        ProjectArchiveOutcome::Exported { .. }
    ));
    let next = service.try_submit(command(request())).unwrap();
    let second = tokens.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!second[3].load(Ordering::Acquire));
    assert!(!service.cancel_project_archive(id));
    assert!(service.cancel_project_archive(next));
    release.send(()).unwrap();
    let _ = poll(&mut service);
    drop(service);
    finished.recv_timeout(Duration::from_secs(5)).unwrap();
}

#[test]
fn dropping_a_client_does_not_cancel_an_accepted_project_archive() {
    let (mut service, tokens, release, finished) = held();
    let _ = service.try_submit(command(request())).unwrap();
    let active = tokens.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(service);
    assert!(active[0].load(Ordering::Acquire));
    assert!(
        active[1..]
            .iter()
            .all(|token| !token.load(Ordering::Acquire))
    );
    release.send(()).unwrap();
    finished.recv_timeout(Duration::from_secs(5)).unwrap();
}

#[test]
fn initial_governor_stop_is_pre_effect_and_no_store_is_a_typed_preflight_refusal() {
    let request = request();
    let mut backend = NativeBackend {
        paths: Paths {
            native: request.origin_root.clone(),
            legacy: "/synthetic/legacy".into(),
        },
        store: None,
        legacy: LegacyScan::default(),
        legacy_notice: None,
        require_existing: true,
    };
    let stopped = backend
        .run_project_archive(&request, Arc::new(AtomicBool::new(true)))
        .unwrap_err();
    assert_eq!(stopped.kind, FailureKind::InspectionCancelled);
    assert!(!stopped.outcome_unknown);
    assert!(backend.store.is_none());
    let Reply::ProjectArchive(refusal) = backend
        .run_project_archive(&request, Arc::new(AtomicBool::new(false)))
        .unwrap()
    else {
        panic!("typed preflight")
    };
    refusal.validate_for(&request).unwrap();
    assert!(!refusal.invoked);
    assert!(backend.store.is_none());
    assert!(matches!(
        refusal.outcome,
        ProjectArchiveOutcome::Failed {
            context: ProjectArchiveContext {
                phase: ProjectArchivePhase::NotStarted,
                ..
            },
            ..
        }
    ));
}
