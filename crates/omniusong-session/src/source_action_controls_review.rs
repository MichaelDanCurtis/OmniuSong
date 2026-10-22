//! Worker-credit and cancellation isolation, without device or filesystem I/O.
use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

type Tokens = (CommandKind, Arc<AtomicBool>, Arc<AtomicBool>);
struct Controlled {
    started: mpsc::Sender<Tokens>,
    release: mpsc::Receiver<()>,
    retired: mpsc::Sender<()>,
}
impl Backend for Controlled {
    fn handle(&mut self, _command: &Command) -> Result<Reply, ServiceError> {
        panic!("control dispatch was bypassed")
    }
    fn handle_with_cancellations(
        &mut self,
        command: &Command,
        inspection: &Arc<AtomicBool>,
        action: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.started
            .send((command.kind(), Arc::clone(inspection), Arc::clone(action)))
            .unwrap();
        self.release.recv_timeout(Duration::from_secs(5)).unwrap();
        Err(ServiceError {
            kind: FailureKind::InspectionCancelled,
            message: "Synthetic control completion".into(),
            outcome_unknown: command.mutates(),
        })
    }
}
impl Drop for Controlled {
    fn drop(&mut self) {
        let _ = self.retired.send(());
    }
}
fn controlled() -> (
    SessionService,
    mpsc::Receiver<Tokens>,
    mpsc::Sender<()>,
    mpsc::Receiver<()>,
) {
    let (started_tx, started) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (retired_tx, retired) = mpsc::channel();
    let client = SessionService::spawn_internal(Controlled {
        started: started_tx,
        release: release_rx,
        retired: retired_tx,
    })
    .unwrap();
    (client, started, release, retired)
}
fn execute() -> Command {
    Command::ExecutePerformanceSourceAction {
        id: "1".repeat(32),
        revision: 1,
        payload_sha256: "a".repeat(64),
    }
}
fn reconcile() -> Command {
    Command::ReconcilePerformanceSourceAction {
        id: "1".repeat(32),
        revision: 2,
        payload_sha256: "b".repeat(64),
    }
}
fn wait(client: &mut SessionService) -> Completion {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = client.try_poll().unwrap() {
            return result;
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}
#[test]
fn source_action_controls_retain_credit_and_never_retarget_inspection_or_stale_ids() {
    let (mut client, started, release, _retired) = controlled();
    let first = client.try_submit(execute()).unwrap();
    let (kind, inspection, action) = started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(kind, CommandKind::ExecutePerformanceSourceAction);
    assert!(!inspection.load(Ordering::Acquire) && !action.load(Ordering::Acquire));
    assert!(!client.cancel_source_inspection(first));
    assert!(!client.cancel_source_action(RequestId {
        generation: first.generation + 1,
        sequence: first.sequence
    }));
    assert!(client.cancel_source_action(first));
    assert!(action.load(Ordering::Acquire) && !inspection.load(Ordering::Acquire));
    assert!(client.is_busy());
    assert_eq!(
        client.try_submit(reconcile()).unwrap_err().kind,
        SubmitFailure::Busy
    );
    assert!(action.load(Ordering::Acquire));
    release.send(()).unwrap();
    let result = wait(&mut client);
    assert_eq!(result.id, first);
    assert!(result.result.unwrap_err().outcome_unknown);
    assert!(!client.is_busy());
    let second = client.try_submit(reconcile()).unwrap();
    let (_, inspection, action) = started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!action.load(Ordering::Acquire) && !inspection.load(Ordering::Acquire));
    assert!(!client.cancel_source_action(first));
    assert!(client.cancel_source_action(second));
    assert!(!client.cancel_source_inspection(second));
    release.send(()).unwrap();
    let result = wait(&mut client);
    assert_eq!(result.id, second);
    assert!(!result.result.unwrap_err().outcome_unknown);
    let third = client
        .try_submit(Command::InspectPerformanceSource {
            stage_id: "2".repeat(32),
        })
        .unwrap();
    let (_, inspection, action) = started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!client.cancel_source_action(third));
    assert!(client.cancel_source_inspection(third));
    assert!(inspection.load(Ordering::Acquire) && !action.load(Ordering::Acquire));
    release.send(()).unwrap();
    assert_eq!(wait(&mut client).id, third);
}
#[test]
fn source_action_controls_client_drop_cancels_only_read_only_inspection_token() {
    for command in [execute(), reconcile()] {
        let (mut client, started, release, retired) = controlled();
        client.try_submit(command).unwrap();
        let (_, inspection, action) = started.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(client);
        assert!(inspection.load(Ordering::Acquire));
        assert!(
            !action.load(Ordering::Acquire),
            "caller Drop must not masquerade as explicit recovery cancellation"
        );
        release.send(()).unwrap();
        retired.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
#[test]
fn source_action_controls_request_shape_and_mutation_flags_are_exact() {
    assert!(execute().validate().is_ok() && execute().mutates() && execute().retry_safe());
    assert!(reconcile().validate().is_ok() && !reconcile().mutates() && reconcile().retry_safe());
    assert!(!Command::ListPerformanceSourceActionStates.mutates());
    for command in [
        Command::ExecutePerformanceSourceAction {
            id: "0".repeat(32),
            revision: 1,
            payload_sha256: "a".repeat(64),
        },
        Command::ExecutePerformanceSourceAction {
            id: "1".repeat(32),
            revision: 0,
            payload_sha256: "a".repeat(64),
        },
        Command::ReconcilePerformanceSourceAction {
            id: "1".repeat(32),
            revision: 4,
            payload_sha256: "a".repeat(64),
        },
        Command::ReconcilePerformanceSourceAction {
            id: "1".repeat(32),
            revision: 2,
            payload_sha256: "A".repeat(64),
        },
    ] {
        assert_eq!(command.validate().unwrap_err().kind, FailureKind::Rejected);
    }
}

#[test]
fn source_action_controls_default_backend_does_not_route_mutation_through_inspection_token() {
    struct DefaultControls;
    impl Backend for DefaultControls {
        fn handle(&mut self, command: &Command) -> Result<Reply, ServiceError> {
            assert!(matches!(
                command,
                Command::ExecutePerformanceSourceAction { .. }
                    | Command::ReconcilePerformanceSourceAction { .. }
            ));
            Err(ServiceError::rejected("Synthetic plain backend"))
        }
        fn handle_with_cancel(
            &mut self,
            _command: &Command,
            _inspection: &Arc<AtomicBool>,
        ) -> Result<Reply, ServiceError> {
            panic!("inspection token reached source execution")
        }
    }
    let inspection = Arc::new(AtomicBool::new(true));
    let action = Arc::new(AtomicBool::new(false));
    for command in [execute(), reconcile()] {
        assert_eq!(
            DefaultControls
                .handle_with_cancellations(&command, &inspection, &action)
                .unwrap_err()
                .kind,
            FailureKind::Rejected
        );
        assert!(!action.load(Ordering::Acquire));
    }
}

#[test]
fn source_action_controls_worker_loss_retains_prior_execution_uncertainty() {
    for command in [
        execute(),
        reconcile(),
        Command::InspectPerformanceSource {
            stage_id: "2".repeat(32),
        },
    ] {
        let expected_unknown = !matches!(command, Command::InspectPerformanceSource { .. });
        let (mut client, peer) = SessionService::test_pair();
        let id = client.try_submit(command.clone()).unwrap();
        assert_eq!(peer.requests.try_recv().unwrap().id, id);
        drop(peer);
        for _ in 0..2 {
            let error = client.try_poll().unwrap_err();
            assert_eq!(error.kind, FailureKind::Disconnected);
            assert_eq!(error.outcome_unknown, expected_unknown);
            assert_eq!(client.retry_command().unwrap().kind(), command.kind());
        }
        assert!(client.is_disconnected());
    }
}

#[test]
fn source_action_controls_bad_completions_keep_credit_and_recovery_uncertainty() {
    for command in [
        execute(),
        reconcile(),
        Command::InspectPerformanceSource {
            stage_id: "2".repeat(32),
        },
    ] {
        let expected_unknown = !matches!(command, Command::InspectPerformanceSource { .. });
        for mismatch in 0..3 {
            let (mut client, peer) = SessionService::test_pair();
            let id = client.try_submit(command.clone()).unwrap();
            let request = peer.requests.try_recv().unwrap();
            peer.responses
                .try_send(Completion {
                    id: if mismatch == 0 {
                        RequestId {
                            sequence: id.sequence + 1,
                            ..id
                        }
                    } else {
                        id
                    },
                    kind: if mismatch == 1 {
                        CommandKind::Refresh
                    } else {
                        request.command.kind()
                    },
                    result: if mismatch == 2 {
                        Ok(Reply::PerformanceSourceActionStates(Vec::new()))
                    } else {
                        Err(ServiceError::rejected("Wrong completion"))
                    },
                })
                .unwrap();
            let error = client.try_poll().unwrap_err();
            assert_eq!(error.kind, FailureKind::Protocol);
            assert_eq!(error.outcome_unknown, expected_unknown);
            assert!(client.is_busy());
            assert_eq!(
                client.try_submit(Command::Refresh).unwrap_err().kind,
                SubmitFailure::Busy
            );
            peer.responses
                .try_send(Completion {
                    id,
                    kind: request.command.kind(),
                    result: Err(ServiceError::rejected("Exact known refusal")),
                })
                .unwrap();
            let completion = client.try_poll().unwrap().unwrap();
            assert_eq!(completion.id, id);
            assert!(!completion.result.unwrap_err().outcome_unknown);
            assert!(!client.is_busy());
        }
    }
}
