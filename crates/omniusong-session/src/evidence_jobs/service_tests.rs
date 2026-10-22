//! Capacity, cancellation, reply identity and retirement composition only.
//! Injected backends are not artifact execution or native-device acceptance.
use super::runner::current_catalog_retirement;
use super::*;
use crate::{Backend, Command, Completion, Reply, RequestId, SessionService, SubmitFailure};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
const JOB: &str = "11111111111111111111111111111111";
fn request() -> EvidenceJobRequest {
    EvidenceJobRequest {
        version: 1,
        job_id: JOB.into(),
        origin_root: "/synthetic/native".into(),
        operation: EvidenceOperation::Export {
            scope: omniusong_store::EVIDENCE_SCOPE.into(),
            export_id: JOB.into(),
            destination: "/synthetic/recorded.osre".into(),
        },
    }
}
fn run() -> Command {
    Command::RunEvidenceJob {
        request: Box::new(request()),
    }
}
fn reconcile() -> Command {
    Command::ReconcileEvidenceJob {
        job_id: JOB.into(),
        expected_request_sha256: request().fingerprint().unwrap(),
        expected_history_sha256: "a".repeat(64),
    }
}
fn job_reply(fatal: bool) -> EvidenceJobReply {
    let problem = EvidenceProblem {
        kind: EvidenceProblemKind::Sqlite,
        origin: if fatal {
            EvidenceResourceOrigin::LiveCatalog
        } else {
            EvidenceResourceOrigin::Evidence
        },
        detail: EvidenceMessage::new("original failure remains distinct from cancellation"),
        sqlite_code: Some(11),
        budget_stop: Some(EvidenceBudgetStop::Cancelled),
    };
    EvidenceJobReply {
        job_id: JOB.into(),
        request_sha256: request().fingerprint().unwrap(),
        history_sha256: None,
        attempted: true,
        outcome: Some(EvidenceOutcome::Stopped {
            problem: problem.clone(),
        }),
        metadata_problem: Some(EvidenceMessage::new("injected terminal metadata refusal")),
        catalog_retirement: current_catalog_retirement(
            problem.origin,
            problem.sqlite_code,
            &problem.detail,
        ),
    }
}
type CancellationTokens = (Arc<AtomicBool>, Arc<AtomicBool>, Arc<AtomicBool>);
type HeldService = (
    SessionService,
    mpsc::Receiver<CancellationTokens>,
    mpsc::Sender<()>,
    mpsc::Receiver<()>,
);
struct Held {
    entered: mpsc::Sender<CancellationTokens>,
    release: mpsc::Receiver<()>,
    done: mpsc::Sender<()>,
    fatal: bool,
}
impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.done.send(());
    }
}
impl Backend for Held {
    fn handle(&mut self, _: &Command) -> Result<Reply, ServiceError> {
        panic!("evidence must use its separate compatible entry point")
    }
    fn handle_with_evidence_cancellation(
        &mut self,
        command: &Command,
        inspection: &Arc<AtomicBool>,
        source: &Arc<AtomicBool>,
        evidence: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.entered
            .send((
                Arc::clone(inspection),
                Arc::clone(source),
                Arc::clone(evidence),
            ))
            .unwrap();
        self.release.recv_timeout(Duration::from_secs(3)).unwrap();
        if matches!(command, Command::InspectRetainedEvidence { .. }) {
            Err(ServiceError {
                kind: FailureKind::InspectionCancelled,
                message: "injected stopped read".into(),
                outcome_unknown: false,
            })
        } else {
            Ok(Reply::EvidenceJobOutcome(Box::new(job_reply(self.fatal))))
        }
    }
}
fn held(fatal: bool) -> HeldService {
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let service = SessionService::spawn_with_backend(Held {
        entered: entered_tx,
        release: release_rx,
        done: done_tx,
        fatal,
    })
    .unwrap();
    (service, entered, release, done)
}
fn completion(service: &mut SessionService) -> Completion {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(completion) = service.try_poll().unwrap() {
            return completion;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn exact_evidence_cancellation_holds_credit_and_is_independent_of_other_tokens() {
    let (mut service, entered, release, done) = held(false);
    let id = service.try_submit(run()).unwrap();
    let (inspection, source, evidence) = entered.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        !Arc::ptr_eq(&inspection, &source)
            && !Arc::ptr_eq(&inspection, &evidence)
            && !Arc::ptr_eq(&source, &evidence)
    );
    assert!(!service.cancel_evidence_job(RequestId {
        generation: id.generation,
        sequence: id.sequence + 1
    }));
    assert!(!service.cancel_source_inspection(id) && !service.cancel_source_action(id));
    assert!(service.cancel_evidence_job(id));
    assert!(
        evidence.load(Ordering::Acquire)
            && !inspection.load(Ordering::Acquire)
            && !source.load(Ordering::Acquire)
    );
    assert!(service.is_busy());
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Busy
    );
    release.send(()).unwrap();
    let result = completion(&mut service);
    assert_eq!(result.id, id);
    assert!(!service.is_busy() && !service.is_disconnected());
    let Reply::EvidenceJobOutcome(reply) = result.result.unwrap() else {
        panic!("reply")
    };
    assert!(reply.catalog_retirement.is_none() && reply.metadata_problem.is_some());
    drop(service);
    done.recv_timeout(Duration::from_secs(3)).unwrap();
}
#[test]
fn cancellation_primary_and_journal_failure_do_not_hide_nested_catalog_retirement() {
    let (mut service, entered, release, done) = held(true);
    let id = service.try_submit(run()).unwrap();
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(service.cancel_evidence_job(id));
    release.send(()).unwrap();
    let result = completion(&mut service);
    let Reply::EvidenceJobOutcome(reply) = result.result.unwrap() else {
        panic!("reply")
    };
    assert!(matches!(
        reply.outcome,
        Some(EvidenceOutcome::Stopped {
            problem: EvidenceProblem {
                kind: EvidenceProblemKind::Sqlite,
                budget_stop: Some(EvidenceBudgetStop::Cancelled),
                ..
            }
        })
    ));
    assert!(reply.metadata_problem.is_some());
    assert_eq!(
        reply.catalog_retirement.as_ref().unwrap().kind,
        FailureKind::StorageUnavailable
    );
    assert!(service.is_disconnected() && !service.is_busy());
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Disconnected
    );
    assert_eq!(
        service.try_poll().unwrap_err().kind,
        FailureKind::StorageUnavailable
    );
    done.recv_timeout(Duration::from_secs(3)).unwrap();
}
#[test]
fn drop_cancels_only_evidence_inspection_and_never_an_accepted_job_or_reconciliation() {
    for command in [
        run(),
        Command::InspectRetainedEvidence {
            path: "/synthetic/review.osre".into(),
        },
        reconcile(),
    ] {
        let inspect = matches!(command, Command::InspectRetainedEvidence { .. });
        let (mut service, entered, release, done) = held(false);
        service.try_submit(command).unwrap();
        let (inspection, source, evidence) = entered.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(service);
        assert!(inspection.load(Ordering::Acquire));
        assert!(!source.load(Ordering::Acquire));
        assert_eq!(evidence.load(Ordering::Acquire), inspect);
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(3)).unwrap();
    }
}
#[test]
fn lost_job_or_reconciliation_reply_retains_uncertainty_without_generic_retry() {
    for command in [run(), reconcile()] {
        assert!(!command.retry_safe());
        let (mut service, peer) = SessionService::test_pair();
        service.try_submit(command).unwrap();
        peer.requests.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(peer);
        let error = service.try_poll().unwrap_err();
        assert_eq!(error.kind, FailureKind::Disconnected);
        assert!(error.outcome_unknown);
        assert!(service.retry_command().is_none());
    }
}
#[test]
fn stale_or_payload_mismatched_job_reply_cannot_release_the_current_credit() {
    let (mut service, peer) = SessionService::test_pair();
    let id = service.try_submit(run()).unwrap();
    let accepted = peer.requests.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut bad = job_reply(false);
    bad.job_id = "2".repeat(32);
    peer.responses
        .send(Completion {
            id,
            kind: accepted.command.kind(),
            result: Ok(Reply::EvidenceJobOutcome(Box::new(bad))),
        })
        .unwrap();
    let error = service.try_poll().unwrap_err();
    assert_eq!(error.kind, FailureKind::Protocol);
    assert!(error.outcome_unknown && service.is_busy());
    let bad_id = RequestId {
        generation: id.generation + 1,
        sequence: id.sequence,
    };
    peer.responses
        .send(Completion {
            id: bad_id,
            kind: accepted.command.kind(),
            result: Ok(Reply::EvidenceJobOutcome(Box::new(job_reply(false)))),
        })
        .unwrap();
    assert_eq!(service.try_poll().unwrap_err().kind, FailureKind::Protocol);
    assert!(service.is_busy());
    peer.responses
        .send(Completion {
            id,
            kind: accepted.command.kind(),
            result: Ok(Reply::EvidenceJobOutcome(Box::new(job_reply(false)))),
        })
        .unwrap();
    assert!(service.try_poll().unwrap().is_some());
    assert!(!service.is_busy());
    assert!(!service.cancel_evidence_job(id));
}

#[test]
fn a_real_sized_result_larger_than_a_journal_record_still_releases_service_credit() {
    fn long_path(character: char, n: usize) -> PathBuf {
        let mut text = String::from("/synthetic/");
        let mut left = n;
        while left > 0 {
            let size = left.min(250);
            text.extend(std::iter::repeat_n(character, size));
            left -= size;
            if left > 0 {
                text.push('/');
            }
        }
        PathBuf::from(text)
    }
    let mut found = None;
    for n in 3900..4070 {
        let archive_path = long_path('"', n);
        let directory = long_path('\\', n);
        let request = EvidenceJobRequest {
            version: 1,
            job_id: JOB.into(),
            origin_root: "/synthetic/native".into(),
            operation: EvidenceOperation::Restore {
                archive_path: archive_path.clone(),
                directory: directory.clone(),
                expected_archive_sha256: "a".repeat(64),
            },
        };
        let outcome = EvidenceOutcome::Restored {
            receipt: EvidenceRestoreReceipt {
                directory,
                archive: EvidenceArchiveReceipt {
                    path: archive_path,
                    export_id: JOB.into(),
                    sha256: "a".repeat(64),
                    byte_length: 1_000_000,
                    file_count: 1,
                    directory_count: 0,
                    payload_bytes: 999_000,
                },
            },
        };
        if request.validate().is_ok()
            && check_bounded(&outcome, MAX_EVIDENCE_JOB_RECORD_BYTES).is_err()
        {
            found = Some((request, outcome));
            break;
        }
    }
    let (request, outcome) = found.expect("valid request can yield a larger receipt");
    assert!(outcome.validate_for(&request).is_ok());
    let (mut service, peer) = SessionService::test_pair();
    let id = service
        .try_submit(Command::RunEvidenceJob {
            request: Box::new(request.clone()),
        })
        .unwrap();
    let accepted = peer.requests.recv_timeout(Duration::from_secs(3)).unwrap();
    peer.responses
        .send(Completion {
            id,
            kind: accepted.command.kind(),
            result: Ok(Reply::EvidenceJobOutcome(Box::new(EvidenceJobReply {
                job_id: JOB.into(),
                request_sha256: request.fingerprint().unwrap(),
                history_sha256: None,
                attempted: true,
                outcome: Some(outcome),
                metadata_problem: Some(EvidenceMessage::new(
                    "result does not fit the durable envelope; artifacts retained",
                )),
                catalog_retirement: None,
            }))),
        })
        .unwrap();
    let completion = service.try_poll().unwrap().unwrap();
    assert!(matches!(
        completion.result,
        Ok(Reply::EvidenceJobOutcome(_))
    ));
    assert!(!service.is_busy() && !service.is_disconnected());
}
