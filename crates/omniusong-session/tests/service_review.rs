#![cfg(feature = "test-hooks")]
//! Independent bounded transport and real storage-worker regressions.
use omniusong_core::{
    ProjectDocument,
    contracts::{EditOperation, EditScope, Extensions, OperationId, ProjectEditIntent, ProjectId},
};
use omniusong_session::*;
use omniusong_store::NativeStore;
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender, TrySendError},
    },
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/session-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn paths(&self) -> Paths {
        Paths {
            native: self.0.join("native"),
            legacy: self.0.join("legacy"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Session review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn poll(service: &mut SessionService) -> Completion {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(reply) = service.try_poll().unwrap() {
            return reply;
        }
        assert!(Instant::now() < deadline, "Worker reply deadline");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn document(completion: Completion) -> (ProjectDocument, bool) {
    match completion.result.unwrap() {
        Reply::Document { view, replayed, .. } => (view.document, replayed),
        _ => panic!("Expected document reply"),
    }
}
fn intent(project: &ProjectDocument, id: &str, edit: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        scope: EditScope::Project,
        edit,
        extensions: Extensions::new(),
    }
}
fn valid(peer: &TestPeer, request: &Request, reply: Reply) {
    peer.responses
        .send(Completion {
            id: request.id,
            kind: request.command.kind(),
            result: Ok(reply),
        })
        .unwrap();
}

#[test]
fn admission_credit_and_both_channel_bounds_survive_unconsumed_completion() {
    let (mut service, peer) = SessionService::test_pair();
    let first = service.try_submit(Command::Initialize).unwrap();
    let request = peer.requests.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(request.id, first);
    valid(&peer, &request, Reply::Library(Library::default()));
    assert!(matches!(
        peer.responses.try_send(Completion {
            id: first,
            kind: CommandKind::Initialize,
            result: Ok(Reply::Library(Library::default()))
        }),
        Err(TrySendError::Full(_))
    ));
    for _ in 0..32 {
        let rejected = service.try_submit(Command::Refresh).unwrap_err();
        assert_eq!(rejected.kind, SubmitFailure::Busy);
        assert!(matches!(rejected.command, Command::Refresh));
        assert_eq!(service.pending().unwrap().id, first);
    }
    assert!(service.is_busy());
    assert_eq!(poll(&mut service).id, first);
    assert!(!service.is_busy());
    let next = service.try_submit(Command::Refresh).unwrap();
    assert_eq!(next.sequence, first.sequence + 1);
}

#[test]
fn invalid_submission_and_stale_generation_sequence_kind_or_reply_preserve_pending_identity() {
    let (mut service, peer) = SessionService::test_pair();
    let invalid = service
        .try_submit(Command::InspectArchive {
            path: PathBuf::new(),
        })
        .unwrap_err();
    assert_eq!(invalid.kind, SubmitFailure::Invalid);
    assert!(!service.is_busy());
    let id = service.try_submit(Command::Initialize).unwrap();
    assert_eq!(id.sequence, 1);
    let request = peer.requests.recv_timeout(Duration::from_secs(1)).unwrap();
    let mismatches = [
        (
            RequestId {
                generation: id.generation + 1,
                sequence: id.sequence,
            },
            CommandKind::Initialize,
            false,
        ),
        (
            RequestId {
                generation: id.generation,
                sequence: id.sequence + 1,
            },
            CommandKind::Initialize,
            false,
        ),
        (id, CommandKind::Refresh, false),
        (id, CommandKind::Initialize, true),
    ];
    for (wrong, kind, wrong_reply) in mismatches {
        let reply = if wrong_reply {
            Reply::Document {
                view: DocumentView::prepare(ProjectDocument::new("Wrong reply kind").unwrap())
                    .unwrap(),
                library: Library::default(),
                replayed: false,
            }
        } else {
            Reply::Library(Library::default())
        };
        peer.responses
            .send(Completion {
                id: wrong,
                kind,
                result: Ok(reply),
            })
            .unwrap();
        let error = service.try_poll().unwrap_err();
        assert_eq!(error.kind, FailureKind::Protocol);
        assert_eq!(service.pending().unwrap().id, id);
        assert!(service.is_busy());
        assert!(!service.is_disconnected());
    }
    valid(&peer, &request, Reply::Library(Library::default()));
    assert_eq!(poll(&mut service).id, id);
    assert!(!service.is_busy());
    let (second, _) = SessionService::test_pair();
    assert_ne!(service.generation(), second.generation());
}

#[test]
fn known_failure_is_a_completion_while_disconnection_preserves_only_safe_retry_intents() {
    let project = ProjectDocument::new("Safe retry").unwrap();
    let edit = intent(
        &project,
        "stable-edit",
        EditOperation::UpdateSession {
            name: "Together".into(),
            tempo: 118.5,
        },
    );
    let (mut service, peer) = SessionService::test_pair();
    let id = service
        .try_submit(Command::Edit {
            intent: edit.clone(),
        })
        .unwrap();
    let request = peer.requests.recv_timeout(Duration::from_secs(1)).unwrap();
    peer.responses
        .send(Completion {
            id,
            kind: CommandKind::Edit,
            result: Err(ServiceError::backend("known operation rejection", false)),
        })
        .unwrap();
    let failure = poll(&mut service);
    assert_eq!(failure.result.unwrap_err().kind, FailureKind::Backend);
    assert!(!service.is_disconnected());
    assert!(!service.is_busy());
    match service.retry_command().unwrap() {
        Command::Edit { intent } => assert_eq!(intent, &edit),
        _ => panic!("Lost safe retry"),
    };
    let retry = service.retry_command().unwrap().clone();
    let retried = service.try_submit(retry).unwrap();
    assert_ne!(retried, id);
    let retry_request = peer.requests.recv_timeout(Duration::from_secs(1)).unwrap();
    match &retry_request.command {
        Command::Edit { intent } => assert_eq!(intent, &edit),
        _ => panic!("Changed retry command"),
    };
    drop(peer);
    let disconnected = service.try_poll().unwrap_err();
    assert_eq!(disconnected.kind, FailureKind::Disconnected);
    assert!(disconnected.outcome_unknown);
    assert!(service.retry_command().is_some());
    assert!(!service.is_busy());
    assert!(service.is_disconnected());
    assert_eq!(request.id, id);
    for command in [
        Command::Create {
            name: "Uncertain creation".into(),
        },
        Command::ImportLegacy {
            path: "source.json".into(),
        },
        Command::ExportArchive {
            project_id: ProjectId(project.id().into()),
            destination: "backup".into(),
        },
    ] {
        let (mut service, peer) = SessionService::test_pair();
        service.try_submit(command).unwrap();
        drop(peer);
        let error = service.try_poll().unwrap_err();
        assert_eq!(error.kind, FailureKind::Disconnected);
        assert!(error.outcome_unknown);
        assert!(service.retry_command().is_none());
    }
}

struct HeldBackend {
    started: Sender<()>,
    release: Receiver<()>,
    finished: Sender<()>,
}
impl Backend for HeldBackend {
    fn handle(&mut self, _: &Command) -> Result<Reply, ServiceError> {
        let _ = self.started.send(());
        self.release
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| ServiceError::backend(error.to_string(), false))?;
        let _ = self.finished.send(());
        Ok(Reply::Library(Library::default()))
    }
}

#[test]
fn dropping_a_real_service_does_not_wait_for_a_held_backend() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let mut service = SessionService::spawn_with_backend(HeldBackend {
        started: started_tx,
        release: release_rx,
        finished: finished_tx,
    })
    .unwrap();
    service.try_submit(Command::Initialize).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (closed_tx, closed_rx) = mpsc::channel();
    let closer = std::thread::spawn(move || {
        drop(service);
        let _ = closed_tx.send(());
    });
    let closed = closed_rx.recv_timeout(Duration::from_secs(1));
    // Release before asserting so even a failing join-on-drop implementation
    // cannot strand the synthetic helper thread or hang the test process.
    release_tx.send(()).unwrap();
    closer.join().unwrap();
    finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(closed.is_ok(), "Drop waited for held worker");
}

#[test]
fn actual_storage_worker_keeps_name_tempo_atomic_and_replayed_view_current() {
    let fixture = Fixture::new();
    let mut service = SessionService::spawn(fixture.paths()).unwrap();
    service.try_submit(Command::Initialize).unwrap();
    assert!(matches!(
        poll(&mut service).result.unwrap(),
        Reply::Library(_)
    ));
    service
        .try_submit(Command::Create {
            name: "Original".into(),
        })
        .unwrap();
    let (original, _) = document(poll(&mut service));
    let first = intent(
        &original,
        "worker-pair",
        EditOperation::UpdateSession {
            name: "First".into(),
            tempo: 117.25,
        },
    );
    service
        .try_submit(Command::Edit {
            intent: first.clone(),
        })
        .unwrap();
    let (first_document, replayed) = document(poll(&mut service));
    assert!(!replayed);
    assert_eq!(
        (first_document.name(), first_document.tempo()),
        ("First", 117.25)
    );
    let mut external = NativeStore::open(fixture.paths().native).unwrap();
    let later = external
        .apply_edit(&intent(
            &first_document,
            "external-pair",
            EditOperation::UpdateSession {
                name: "Newer".into(),
                tempo: 130.5,
            },
        ))
        .unwrap()
        .document;
    service.try_submit(Command::Edit { intent: first }).unwrap();
    let (current, replayed) = document(poll(&mut service));
    assert!(replayed);
    assert_eq!(current, later);
    let undo = intent(&current, "worker-undo", EditOperation::Undo);
    service
        .try_submit(Command::Edit {
            intent: undo.clone(),
        })
        .unwrap();
    let (undone, _) = document(poll(&mut service));
    assert_eq!((undone.name(), undone.tempo()), ("First", 117.25));
    service.try_submit(Command::Edit { intent: undo }).unwrap();
    let (same, replayed) = document(poll(&mut service));
    assert!(replayed);
    assert_eq!(same, undone);
    let redo = intent(&undone, "worker-redo", EditOperation::Redo);
    service
        .try_submit(Command::Edit {
            intent: redo.clone(),
        })
        .unwrap();
    let (redone, _) = document(poll(&mut service));
    assert_eq!((redone.name(), redone.tempo()), ("Newer", 130.5));
    service.try_submit(Command::Edit { intent: redo }).unwrap();
    let (same, replayed) = document(poll(&mut service));
    assert!(replayed);
    assert_eq!(same, redone);
    assert_eq!(external.load(original.id()).unwrap(), redone);
}

#[test]
fn actual_backend_operation_error_does_not_disconnect_or_block_the_next_action() {
    let fixture = Fixture::new();
    let mut service = SessionService::spawn(fixture.paths()).unwrap();
    service.try_submit(Command::Initialize).unwrap();
    poll(&mut service).result.unwrap();
    service
        .try_submit(Command::Open {
            project_id: ProjectId("missing-project".into()),
        })
        .unwrap();
    let error = poll(&mut service).result.unwrap_err();
    assert_eq!(error.kind, FailureKind::Backend);
    assert!(!error.outcome_unknown);
    assert!(!service.is_disconnected());
    assert!(matches!(
        service.retry_command(),
        Some(Command::Open { .. })
    ));
    service.try_submit(Command::Refresh).unwrap();
    assert!(matches!(
        poll(&mut service).result.unwrap(),
        Reply::Library(_)
    ));
    assert!(service.retry_command().is_none());
}

#[test]
fn mismatched_fatal_packet_cannot_poison_the_genuine_pending_request_or_hide_its_completion() {
    let project = ProjectDocument::new("Pending durable action").unwrap();
    let edit = intent(
        &project,
        "fatal-correlation",
        EditOperation::Rename {
            name: "Retained draft".into(),
        },
    );
    let (mut service, peer) = SessionService::test_pair();
    let id = service
        .try_submit(Command::Edit {
            intent: edit.clone(),
        })
        .unwrap();
    let TestPeer {
        requests,
        responses,
    } = peer;
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(1)).unwrap().id,
        id
    );
    for wrong_id in [
        RequestId {
            generation: id.generation + 1,
            sequence: id.sequence,
        },
        RequestId {
            generation: id.generation,
            sequence: id.sequence + 1,
        },
    ] {
        responses
            .send(Completion {
                id: wrong_id,
                kind: CommandKind::Edit,
                result: Err(ServiceError::storage_unavailable(
                    "Stale terminal packet",
                    true,
                )),
            })
            .unwrap();
        assert_eq!(service.try_poll().unwrap_err().kind, FailureKind::Protocol);
        assert!(!service.is_disconnected());
        assert_eq!(service.pending().unwrap().id, id);
    }
    responses
        .send(Completion {
            id,
            kind: CommandKind::Refresh,
            result: Err(ServiceError::storage_unavailable(
                "Wrong-kind terminal packet",
                true,
            )),
        })
        .unwrap();
    assert_eq!(service.try_poll().unwrap_err().kind, FailureKind::Protocol);
    assert!(!service.is_disconnected());
    responses
        .send(Completion {
            id,
            kind: CommandKind::Edit,
            result: Err(ServiceError::storage_unavailable(
                "Known terminal result",
                true,
            )),
        })
        .unwrap();
    drop(responses); // EOF is already pending behind the known completion.
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Busy
    );
    let completion = service
        .try_poll()
        .unwrap()
        .expect("EOF overwrote terminal completion");
    assert_eq!(completion.id, id);
    let error = completion.result.unwrap_err();
    assert_eq!(error.kind, FailureKind::StorageUnavailable);
    assert_eq!(error.message, "Known terminal result");
    assert!(error.outcome_unknown);
    assert!(service.is_disconnected());
    assert!(!service.is_busy());
    for command in [
        Command::Refresh,
        Command::Edit {
            intent: edit.clone(),
        },
    ] {
        assert_eq!(
            service.try_submit(command).unwrap_err().kind,
            SubmitFailure::Disconnected
        );
    }
    assert!(
        requests.try_recv().is_err(),
        "Retired generation admitted another command"
    );
    assert!(matches!(service.retry_command(),Some(Command::Edit { intent }) if intent==&edit));
}

struct CommitThenTerminal {
    store: NativeStore,
    calls: Arc<AtomicUsize>,
    dropped: Sender<()>,
}
impl Backend for CommitThenTerminal {
    fn handle(&mut self, command: &Command) -> Result<Reply, ServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let Command::Edit { intent } = command else {
            return Err(ServiceError::rejected("Only the synthetic edit is allowed"));
        };
        self.store
            .apply_edit(intent)
            .map_err(|e| ServiceError::backend(e.to_string(), true))?;
        Err(ServiceError::storage_unavailable(
            "Synthetic fatal acknowledgement after commit",
            true,
        ))
    }
}
impl Drop for CommitThenTerminal {
    fn drop(&mut self) {
        let _ = self.dropped.send(());
    }
}

#[test]
fn committed_terminal_failure_requires_new_generation_and_explicit_same_id_retry_without_duplicate_effect()
 {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    let mut store = NativeStore::open(&paths.native).unwrap();
    let initial = store
        .create(&ProjectDocument::new("Before uncertain acknowledgement").unwrap())
        .unwrap();
    let edit = intent(
        &initial,
        "committed-terminal-edit",
        EditOperation::UpdateSession {
            name: "Committed once".into(),
            tempo: 119.25,
        },
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let (drop_tx, drop_rx) = mpsc::channel();
    let mut old = SessionService::spawn_with_backend(CommitThenTerminal {
        store,
        calls: calls.clone(),
        dropped: drop_tx,
    })
    .unwrap();
    let old_generation = old.generation();
    let id = old
        .try_submit(Command::Edit {
            intent: edit.clone(),
        })
        .unwrap();
    drop_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(old.is_busy());
    let completion = old
        .try_poll()
        .unwrap()
        .expect("Producer EOF hid committed failure");
    assert_eq!(completion.id, id);
    let failure = completion.result.unwrap_err();
    assert_eq!(failure.kind, FailureKind::StorageUnavailable);
    assert!(failure.outcome_unknown);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(old.is_disconnected());
    let retry = old.retry_command().unwrap().clone();
    assert!(matches!(&retry,Command::Edit { intent } if intent==&edit));
    assert_eq!(
        old.try_submit(retry.clone()).unwrap_err().kind,
        SubmitFailure::Disconnected
    );
    drop(old);
    let mut fresh = SessionService::reconnect(paths.clone()).unwrap();
    assert_ne!(fresh.generation(), old_generation);
    fresh.try_submit(Command::Initialize).unwrap();
    assert!(matches!(
        poll(&mut fresh).result.unwrap(),
        Reply::Library(_)
    ));
    let check = NativeStore::open(&paths.native).unwrap();
    let committed = check.load(initial.id()).unwrap();
    assert_eq!(committed.revision(), 1);
    drop(check);
    assert!(
        !fresh.is_busy(),
        "Reconnect automatically retried accepted work"
    );
    let transport = fresh.try_submit(retry).unwrap();
    assert_ne!(transport.generation, id.generation);
    let (returned, replayed) = document(poll(&mut fresh));
    assert!(replayed);
    assert_eq!(returned, committed);
    let mut check = NativeStore::open(&paths.native).unwrap();
    assert_eq!(check.load(initial.id()).unwrap(), committed);
    let undone = check.undo(initial.id(), 1).unwrap();
    assert_eq!(undone.name(), initial.name());
    assert_eq!(undone.tempo(), initial.tempo());
    assert!(
        check.undo(initial.id(), undone.revision()).is_err(),
        "Same-ID retry added a second undo step"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn reconnect_missing_database_keeps_absent_and_existing_directories_without_creating_a_catalog() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    let mut absent = SessionService::reconnect(paths.clone()).unwrap();
    absent.try_submit(Command::Initialize).unwrap();
    assert_eq!(
        poll(&mut absent).result.unwrap_err().kind,
        FailureKind::StorageUnavailable
    );
    assert!(absent.is_disconnected());
    assert!(!paths.native.exists());
    drop(absent);
    fs::create_dir_all(&paths.native).unwrap();
    let sentinel = paths.native.join("retained-original");
    fs::write(&sentinel, b"unchanged").unwrap();
    let mut existing = SessionService::reconnect(paths.clone()).unwrap();
    existing.try_submit(Command::Initialize).unwrap();
    assert_eq!(
        poll(&mut existing).result.unwrap_err().kind,
        FailureKind::StorageUnavailable
    );
    assert!(existing.is_disconnected());
    assert!(!paths.native.join("studio.sqlite3").exists());
    assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
    assert_eq!(fs::read_dir(&paths.native).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn reconnect_rejects_symlink_database_without_touching_its_valid_target() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    let outside = fixture.0.join("separate-store");
    let mut store = NativeStore::open(&outside).unwrap();
    let doc = store
        .create(&ProjectDocument::new("Separate retained catalog").unwrap())
        .unwrap();
    drop(store);
    let database = outside.join("studio.sqlite3");
    let before = fs::read(&database).unwrap();
    fs::create_dir_all(&paths.native).unwrap();
    let alias = paths.native.join("studio.sqlite3");
    std::os::unix::fs::symlink(&database, &alias).unwrap();
    let mut service = SessionService::reconnect(paths).unwrap();
    service.try_submit(Command::Initialize).unwrap();
    assert_eq!(
        poll(&mut service).result.unwrap_err().kind,
        FailureKind::StorageUnavailable
    );
    assert!(service.is_disconnected());
    assert!(
        fs::symlink_metadata(alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&database).unwrap(), before);
    assert_eq!(
        NativeStore::open(outside).unwrap().load(doc.id()).unwrap(),
        doc
    );
}

#[test]
fn ordinary_malformed_archive_and_conflicting_intent_leave_generation_and_store_usable() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    let mut service = SessionService::spawn(paths.clone()).unwrap();
    service.try_submit(Command::Initialize).unwrap();
    poll(&mut service).result.unwrap();
    service
        .try_submit(Command::Create {
            name: "Healthy native storage".into(),
        })
        .unwrap();
    let (initial, _) = document(poll(&mut service));
    let edit = intent(
        &initial,
        "ordinary-stable-edit",
        EditOperation::Rename {
            name: "Valid committed edit".into(),
        },
    );
    service
        .try_submit(Command::Edit {
            intent: edit.clone(),
        })
        .unwrap();
    let (saved, _) = document(poll(&mut service));
    let invalid = fixture.0.join("invalid-backup");
    fs::write(&invalid, b"bounded invalid external archive").unwrap();
    let mut conflicting = edit.clone();
    conflicting.edit = EditOperation::Rename {
        name: "Different content under same ID".into(),
    };
    let generation = service.generation();
    for command in [
        Command::InspectArchive {
            path: invalid.clone(),
        },
        Command::RestoreCopy {
            path: invalid.clone(),
            intent: RestoreCopyIntent {
                operation_id: OperationId("ordinary-bad-copy".into()),
                expected_archive_sha256: "a".repeat(64),
            },
        },
        Command::Edit {
            intent: conflicting,
        },
    ] {
        service.try_submit(command).unwrap();
        let error = poll(&mut service).result.unwrap_err();
        assert_eq!(error.kind, FailureKind::Backend);
        assert!(!service.is_disconnected());
        assert_eq!(service.generation(), generation);
        service.try_submit(Command::Refresh).unwrap();
        assert!(matches!(
            poll(&mut service).result.unwrap(),
            Reply::Library(_)
        ));
    }
    assert_eq!(
        NativeStore::open(paths.native)
            .unwrap()
            .load(initial.id())
            .unwrap(),
        saved
    );
    assert_eq!(
        fs::read(invalid).unwrap(),
        b"bounded invalid external archive"
    );
}
