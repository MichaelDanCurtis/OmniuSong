//! Synthetic library/worker evidence only. No GUI, native input or device claims.
use omniusong_core::{
    DocumentNoteAddress, NoteTimelinePolicy, NoteValues, ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
};
use omniusong_session::{
    Command, Completion, FailureKind, Paths, Reply, ServiceError, SessionService, SubmitFailure,
};
use omniusong_store::NativeStore;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

const TRACK: &str = "鍵盤\0source";
const NOTE: &str = "\0captured-note";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.runtime/note-edit-service");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn paths(&self) -> Paths {
        Paths {
            native: self.0.join("native"),
            legacy: self.0.join("unused-synthetic-legacy"),
        }
    }
    fn seed(&self, locked: bool) -> ProjectDocument {
        let mut value = ProjectDocument::new("Synthetic note worker evidence")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["id"] = json!(TRACK);
        value["tracks"][0]["locked"] = json!(locked);
        value["tracks"][0]["notes"] = json!([{"id":NOTE,"pitch":60,"startTick":37,"durationTicks":960,"velocity16":65001,"expression":[{"type":"pitch","points":[{"tick":0,"value32":2147483648u32},{"tick":960,"value32":4294967295u32}],"vendor:lane":"unchanged"}],"input":{"sourceId":"synthetic-controller","sourceName":"Software fixture; no device","protocol":"midi2","group":3,"channel":5,"receivedTimeMs":123.5,"releaseVelocity16":60001,"keyDurationTicks":960,"vendor:clock":"18446744073709551615"},"vendor:model":{"role":"opaque fixture, no inference","id":"keep-exact"}}]);
        value["tracks"][0]["vendor:opaque"] =
            serde_json::from_str("{\"integer\":1844674407370955161601,\"decimal\":1.2300}")
                .unwrap();
        value["inputCapture"] = json!({"version":1,"droppedEvents":0,"events":[{"trackId":TRACK,"tick":37,"sourceId":"synthetic-controller","sourceName":"Software fixture; no device","receivedTimeMs":123.5,"hostTime":"18446744073709551615","format":"ump","words":[1083194368u32,4294967295u32],"vendor:packet":"exact"}]});
        let mut store = NativeStore::open(self.paths().native).unwrap();
        store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Synthetic evidence retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn address() -> DocumentNoteAddress {
    DocumentNoteAddress {
        track_id: TRACK.into(),
        note_id: NOTE.into(),
    }
}
fn values() -> NoteValues {
    NoteValues {
        pitch: 67,
        start_tick: 123,
        duration_ticks: 1920,
        velocity16: 0,
    }
}
fn intent(document: &ProjectDocument, operation: &str, edit: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(operation.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        scope: if matches!(edit, EditOperation::Undo | EditOperation::Redo) {
            EditScope::Project
        } else {
            EditScope::DocumentNote { address: address() }
        },
        edit,
        extensions: Default::default(),
    }
}
fn poll(service: &mut SessionService) -> Completion {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(completion) = service.try_poll().unwrap() {
            return completion;
        }
        assert!(
            Instant::now() < deadline,
            "Background note action timed out"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn call(service: &mut SessionService, command: Command) -> Result<Reply, ServiceError> {
    service.try_submit(command).unwrap();
    poll(service).result
}
fn document(reply: Reply) -> (ProjectDocument, bool) {
    match reply {
        Reply::Document { view, replayed, .. } => (view.document, replayed),
        _ => panic!("Expected prepared document"),
    }
}
fn edit(
    service: &mut SessionService,
    document: &ProjectDocument,
    id: &str,
    kind: EditOperation,
) -> ProjectDocument {
    document_from(
        call(
            service,
            Command::Edit {
                intent: intent(document, id, kind),
            },
        )
        .unwrap(),
    )
}
fn document_from(reply: Reply) -> ProjectDocument {
    document(reply).0
}
fn unchanged_source(original: &ProjectDocument, current: &ProjectDocument) {
    assert_eq!(
        current.value()["inputCapture"],
        original.value()["inputCapture"]
    );
    assert_eq!(
        current.tracks()[0]["vendor:opaque"],
        original.tracks()[0]["vendor:opaque"]
    );
    assert_eq!(current.tracks()[1], original.tracks()[1]);
}

#[test]
fn real_worker_preserves_expressive_original_through_edit_delete_history_restore_and_reopen() {
    let fixture = Fixture::new();
    let source = fixture.seed(false);
    let original_note = source.tracks()[0]["notes"][0].clone();
    let mut service = SessionService::reconnect(fixture.paths()).unwrap();
    call(&mut service, Command::Initialize).unwrap();
    let first = edit(
        &mut service,
        &source,
        "note-worker-first",
        EditOperation::UpdateNote {
            values: values(),
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    );
    assert_eq!(first.revision(), 1);
    unchanged_source(&source, &first);
    let snapshot = first.tracks()[0]["notes"][0]["editSource"].clone();
    assert_eq!(snapshot["pitch"], original_note["pitch"]);
    assert_eq!(snapshot["expression"], original_note["expression"]);
    assert_eq!(
        first.tracks()[0]["notes"][0]["input"],
        original_note["input"]
    );
    let second = edit(
        &mut service,
        &first,
        "note-worker-second",
        EditOperation::UpdateNote {
            values: NoteValues {
                pitch: 72,
                velocity16: u16::MAX,
                ..values()
            },
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    );
    assert_eq!(second.tracks()[0]["notes"][0]["editSource"], snapshot);
    let deleted = edit(
        &mut service,
        &second,
        "note-worker-delete",
        EditOperation::DeleteNote,
    );
    assert!(deleted.tracks()[0]["notes"].as_array().unwrap().is_empty());
    unchanged_source(&source, &deleted);
    let undone = edit(
        &mut service,
        &deleted,
        "note-worker-undo",
        EditOperation::Undo,
    );
    assert_eq!(undone.tracks(), second.tracks());
    let redone = edit(
        &mut service,
        &undone,
        "note-worker-redo",
        EditOperation::Redo,
    );
    assert_eq!(redone.tracks(), deleted.tracks());
    let restored_deleted = edit(
        &mut service,
        &redone,
        "note-worker-undo-again",
        EditOperation::Undo,
    );
    let restored = edit(
        &mut service,
        &restored_deleted,
        "note-worker-restore-original",
        EditOperation::RestoreNote {
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    );
    let mut expected = original_note;
    expected["editSource"] = snapshot;
    assert_eq!(restored.tracks()[0]["notes"][0], expected);
    unchanged_source(&source, &restored);
    assert_eq!(restored.bars(), source.bars());
    assert_eq!(restored.value()["history"], source.value()["history"]);
    drop(service);
    let mut reopened = SessionService::reconnect(fixture.paths()).unwrap();
    call(&mut reopened, Command::Initialize).unwrap();
    let actual = document_from(
        call(
            &mut reopened,
            Command::Open {
                project_id: ProjectId(source.id().into()),
            },
        )
        .unwrap(),
    );
    assert_eq!(actual, restored);
}

#[test]
fn lost_add_ack_has_bounded_credit_and_explicit_same_intent_retry_returns_actual_latest_document() {
    let fixture = Fixture::new();
    let source = fixture.seed(false);
    let mut service = SessionService::reconnect(fixture.paths()).unwrap();
    call(&mut service, Command::Initialize).unwrap();
    let generation = service.generation();
    let mut request = intent(
        &source,
        "lost-note-add",
        EditOperation::AddNote {
            values: values(),
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    );
    request.scope = EditScope::DocumentNote {
        address: DocumentNoteAddress {
            track_id: TRACK.into(),
            note_id: uuid::Uuid::new_v4().to_string(),
        },
    };
    let canonical = serde_json::to_vec(&request).unwrap();
    let id = service
        .try_submit(Command::Edit {
            intent: request.clone(),
        })
        .unwrap();
    let mut external = NativeStore::open(fixture.paths().native).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let committed = loop {
        let doc = external.load(source.id()).unwrap();
        if doc.revision() == 1 {
            break doc;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let before = Instant::now();
    for _ in 0..32 {
        let rejected = service.try_submit(Command::Refresh).unwrap_err();
        assert_eq!(rejected.kind, SubmitFailure::Busy);
        assert!(matches!(rejected.command, Command::Refresh));
        assert_eq!(service.pending().unwrap().id, id);
    }
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(service.is_busy());
    drop(service);
    let later = external
        .apply_edit(&ProjectEditIntent {
            operation_id: OperationId("intervening-note-name".into()),
            project_id: ProjectId(source.id().into()),
            expected_revision: committed.revision(),
            scope: EditScope::Project,
            edit: EditOperation::Rename {
                name: "Newer saved name after lost note acknowledgement".into(),
            },
            extensions: Default::default(),
        })
        .unwrap()
        .document;
    let mut fresh = SessionService::reconnect(fixture.paths()).unwrap();
    assert_ne!(fresh.generation(), generation);
    call(&mut fresh, Command::Initialize).unwrap();
    let opened = document_from(
        call(
            &mut fresh,
            Command::Open {
                project_id: ProjectId(source.id().into()),
            },
        )
        .unwrap(),
    );
    assert_eq!(opened, later);
    assert_eq!(opened.revision(), 2);
    assert_eq!(opened.tracks()[0]["notes"].as_array().unwrap().len(), 2);
    assert_eq!(serde_json::to_vec(&request).unwrap(), canonical);
    let retry_id = fresh.try_submit(Command::Edit { intent: request }).unwrap();
    assert_ne!(id, retry_id);
    let (current, replayed) = document(poll(&mut fresh).result.unwrap());
    assert!(replayed);
    assert_eq!(current, later);
    assert_eq!(external.load(source.id()).unwrap(), later);
    unchanged_source(&source, &current);
}

#[test]
fn locked_expression_and_noop_rejections_are_known_and_do_not_retire_or_advance_worker() {
    let fixture = Fixture::new();
    let source = fixture.seed(false);
    let locked = fixture.seed(true);
    let mut service = SessionService::reconnect(fixture.paths()).unwrap();
    call(&mut service, Command::Initialize).unwrap();
    let cases = [
        (&locked, "locked-note", values()),
        (
            &source,
            "expression-tail",
            NoteValues {
                duration_ticks: 959,
                ..values()
            },
        ),
        (
            &source,
            "no-op-note",
            NoteValues {
                pitch: 60,
                start_tick: 37,
                duration_ticks: 960,
                velocity16: 65001,
            },
        ),
    ];
    for (project, id, values) in cases {
        let request = intent(
            project,
            id,
            EditOperation::UpdateNote {
                values,
                timeline: NoteTimelinePolicy::PreserveLoop,
            },
        );
        let error = call(
            &mut service,
            Command::Edit {
                intent: request.clone(),
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::EditRejected);
        assert!(!error.outcome_unknown);
        assert!(!service.is_disconnected());
        let Command::Edit { intent: retained } = service.retry_command().unwrap() else {
            panic!("Safe exact retry intent retained")
        };
        assert_eq!(retained, &request);
        let actual = document_from(
            call(
                &mut service,
                Command::Open {
                    project_id: ProjectId(project.id().into()),
                },
            )
            .unwrap(),
        );
        assert_eq!(&actual, project);
    }
    let changed = edit(
        &mut service,
        &source,
        "valid-after-rejections",
        EditOperation::UpdateNote {
            values: values(),
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    );
    assert_eq!(changed.revision(), 1);
}

#[test]
fn malformed_synthetic_catalog_is_storage_unavailable_not_known_note_rejection() {
    let fixture = Fixture::new();
    let _ = fixture.seed(false);
    fs::write(
        fixture.paths().native.join("studio.sqlite3"),
        b"synthetic malformed SQLite; no original catalog",
    )
    .unwrap();
    let mut service = SessionService::reconnect(fixture.paths()).unwrap();
    let error = call(&mut service, Command::Initialize).unwrap_err();
    assert_eq!(error.kind, FailureKind::StorageUnavailable);
    assert_ne!(error.kind, FailureKind::EditRejected);
    assert!(service.is_disconnected());
}

#[cfg(feature = "test-hooks")]
#[test]
fn held_real_note_backend_runs_off_caller_and_drop_never_waits_for_storage() {
    use omniusong_session::{Backend, DocumentView, Library};
    use std::sync::mpsc;
    struct Held {
        store: NativeStore,
        entered: mpsc::Sender<std::thread::ThreadId>,
        release: mpsc::Receiver<()>,
        finished: mpsc::Sender<()>,
    }
    impl Backend for Held {
        fn handle(&mut self, command: &Command) -> Result<Reply, ServiceError> {
            self.entered.send(std::thread::current().id()).unwrap();
            self.release
                .recv_timeout(Duration::from_secs(5))
                .map_err(|e| ServiceError::backend(e.to_string(), false))?;
            let Command::Edit { intent } = command else {
                return Err(ServiceError::rejected("Expected note edit"));
            };
            let result = self
                .store
                .apply_edit(intent)
                .map_err(|e| ServiceError::backend(e.to_string(), true))?;
            let view = DocumentView::prepare(result.document)?;
            self.finished.send(()).unwrap();
            Ok(Reply::Document {
                view,
                library: Library::default(),
                replayed: result.replayed,
            })
        }
    }
    let fixture = Fixture::new();
    let source = fixture.seed(false);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let caller = std::thread::current().id();
    let mut service = SessionService::spawn_with_backend(Held {
        store: NativeStore::open(fixture.paths().native).unwrap(),
        entered: entered_tx,
        release: release_rx,
        finished: finished_tx,
    })
    .unwrap();
    let request = intent(
        &source,
        "held-note-worker",
        EditOperation::UpdateNote {
            values: values(),
            timeline: NoteTimelinePolicy::PreserveLoop,
        },
    );
    let id = service
        .try_submit(Command::Edit { intent: request })
        .unwrap();
    assert_ne!(
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        caller
    );
    let before = Instant::now();
    assert!(service.try_poll().unwrap().is_none());
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Busy
    );
    assert_eq!(service.pending().unwrap().id, id);
    assert!(before.elapsed() < Duration::from_millis(200));
    let (closed_tx, closed_rx) = mpsc::channel();
    let closer = std::thread::spawn(move || {
        drop(service);
        closed_tx.send(()).unwrap();
    });
    let closed = closed_rx.recv_timeout(Duration::from_secs(1));
    release_tx.send(()).unwrap();
    closer.join().unwrap();
    finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(closed.is_ok(), "Client Drop waited for held worker");
    let current = NativeStore::open(fixture.paths().native)
        .unwrap()
        .load(source.id())
        .unwrap();
    assert_eq!(current.revision(), 1);
    unchanged_source(&source, &current);
}
