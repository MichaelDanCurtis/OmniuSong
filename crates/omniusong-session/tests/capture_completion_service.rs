//! Real session-worker capture completion over temporary synthetic PCM only.
//! No devices, default catalogs, retained user originals or native UI are used.
use omniusong_core::{
    ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_session::{
    CaptureAttachmentCommitment, CaptureAttachmentCompletion, CaptureAttachmentOutcome,
    CaptureAttachmentProblemKind, CaptureAttachmentReply, CaptureAttachmentRequest,
    Command, FailureKind, Paths, Reply, ServiceError, SessionService, SubmitFailure,
    MAX_CAPTURE_ATTACHMENT_REPLY_BYTES,
};
use omniusong_store::{NativeStore, TakePublication};
use rusqlite::Connection;
use std::{fs, path::PathBuf, time::{Duration, Instant}};

struct Fixture {
    root: PathBuf,
    paths: Paths,
    project: ProjectDocument,
    request: CaptureAttachmentRequest,
    staged_bytes: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("omniusong-capture-completion-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let paths = Paths { native: root.join("store"), legacy: root.join("legacy") };
        let mut store = NativeStore::open(&paths.native).unwrap();
        let project = store.create(&ProjectDocument::new("Synthetic capture target").unwrap()).unwrap();
        let origin_root = store.root().to_path_buf();
        drop(store);
        let descriptor: TakeDescriptor = serde_json::from_value(serde_json::json!({
            "version":1,"take_id":"synthetic-completion-take","streams":[{
                "stream_id":"audio","segment_id":"segment","source_id":"synthetic",
                "source_name":"Software-only completion fixture","source_connection_generation":1,
                "host_clock":{"domain":"host","epoch":"epoch","ticks_per_second_numerator":1000000000u64,"ticks_per_second_denominator":1},
                "kind":{"kind":"audio","format":{"sample_rate":48000,"channels":1,"encoding":"f32_le","layout":"interleaved"},"audio_epoch":"epoch"},
                "initial_source_position":0}],"provenance":{"test":"synthetic; no device"}
        })).unwrap();
        let journal_path = root.join("capture.sqlite3");
        let mut journal = Journal::create(&journal_path, &descriptor).unwrap();
        let stamp = HostTimestamp { domain: ClockDomainId("host".into()), epoch: ClockEpochId("epoch".into()), ticks: 0 };
        journal.append(&ChunkHeader {
            version: 1, take_id: descriptor.take_id.clone(), stream_id: "audio".into(), segment_id: "segment".into(), sequence: 0,
            range: SourceRange::AudioFrames { start: 0, count: 4 }, interpretation: descriptor.streams[0].kind.clone(),
            observed_start: stamp.clone(), observed_end: stamp, clock_anchors: vec![], kind: ChunkKind::Content,
        }, &[0; 16]).unwrap();
        let manifest = journal.seal("synthetic-completion-seal").unwrap();
        drop(journal);
        let staged_bytes = fs::read(&journal_path).unwrap();
        let request = CaptureAttachmentRequest {
            version: 1, origin_root, journal_path,
            publication: TakePublication {
                operation_id: OperationId("same-capture-completion".into()), project_id: ProjectId(project.id().into()), expected_revision: 0,
                track_id: TrackId("take-track".into()), track_name: "Synthetic original".into(),
                clip: TakeClip {
                    edit_source: None, id: "take-clip".into(), source: TakeReference { take_id: descriptor.take_id, manifest_sha256: manifest.sha256, stream_id: "audio".into() },
                    range: TakeRange::AudioFrames { start: 0, count: 4 }, position: TimelinePosition { frame: 0, sample_rate: 48000 }, fades: None,
                },
            },
        };
        request.validate().unwrap();
        Self { root, paths, project, request, staged_bytes }
    }
    fn worker(&self) -> SessionService {
        let mut service = SessionService::reconnect(self.paths.clone()).unwrap();
        call(&mut service, Command::Initialize).unwrap();
        service
    }
    fn unchanged_stage(&self) {
        assert_eq!(fs::read(&self.request.journal_path).unwrap(), self.staged_bytes, "Attachment changed the original staged journal bytes");
    }
    fn owned_originals(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let directory = self.paths.native.join("takes");
        if !directory.exists() { return Vec::new(); }
        let mut originals: Vec<_> = fs::read_dir(directory).unwrap().map(|entry| {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        }).collect();
        originals.sort_by(|a, b| a.0.cmp(&b.0));
        originals
    }
    fn current_row(&self) -> (String, u64) {
        let connection = Connection::open(self.paths.native.join("studio.sqlite3")).unwrap();
        connection.query_row("SELECT payload,revision FROM projects WHERE id=?1", [self.project.id()], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
}
fn call(service: &mut SessionService, command: Command) -> Result<Reply, ServiceError> {
    service.try_submit(command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(completion) = service.try_poll().unwrap() { return completion.result; }
        assert!(Instant::now() < deadline, "Synthetic capture completion worker did not reply");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn attach(service: &mut SessionService, request: &CaptureAttachmentRequest) -> CaptureAttachmentReply {
    let reply = call(service, Command::AttachCapture { request: Box::new(request.clone()) }).unwrap();
    let Reply::CaptureAttachment(reply) = reply else { panic!("Capture attachment returned a document/library or another reply") };
    assert!(reply.matches_request_reference(request));
    reply.validate_for(request).unwrap();
    assert!(serde_json::to_vec(reply.as_ref()).unwrap().len() <= MAX_CAPTURE_ATTACHMENT_REPLY_BYTES);
    *reply
}
fn commitment(reply: &CaptureAttachmentReply) -> &CaptureAttachmentCommitment {
    let CaptureAttachmentOutcome::Committed { commitment } = &reply.outcome else { panic!("Expected compact commitment: {reply:?}") };
    assert!(reply.store_invoked);
    assert!(reply.catalog_retirement.is_none());
    commitment
}
fn rename(document: &ProjectDocument, operation: &str, name: &str) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(operation.into()), project_id: ProjectId(document.id().into()), expected_revision: document.revision(),
        scope: EditScope::Project, edit: EditOperation::Rename { name: name.into() }, extensions: Default::default(),
    }
}
fn navigate(document: &ProjectDocument, operation: &str, edit: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(operation.into()), project_id: ProjectId(document.id().into()), expected_revision: document.revision(),
        scope: EditScope::Project, edit, extensions: Default::default(),
    }
}

#[test]
fn ordinary_fresh_commit_and_exact_replay_do_not_install_a_historical_snapshot() {
    let fixture = Fixture::new();
    let mut service = fixture.worker();
    let command = Command::AttachCapture { request: Box::new(fixture.request.clone()) };
    assert!(command.retry_safe() && command.mutates());
    let fresh = attach(&mut service, &fixture.request);
    let saved = commitment(&fresh);
    assert_eq!(saved.operation_id, fixture.request.publication.operation_id);
    assert_eq!(saved.project_id, fixture.request.publication.project_id);
    assert_eq!((saved.previous_revision, saved.committed_revision), (0, 1));
    assert_eq!(saved.completion, CaptureAttachmentCompletion::FreshCommit);
    fixture.unchanged_stage();
    let originals = fixture.owned_originals();
    assert_eq!(originals.len(), 1);

    // Current usability is requested explicitly after accepting commitment.
    let Reply::Document { view, .. } = call(&mut service, Command::Open { project_id: fixture.request.publication.project_id.clone() }).unwrap() else { panic!("Missing independent current read") };
    assert_eq!(view.document.revision(), 1);
    let Reply::Document { view, .. } = call(&mut service, Command::Edit { intent: navigate(&view.document, "undo-capture", EditOperation::Undo) }).unwrap() else { panic!("Missing capture Undo") };
    assert_eq!(view.document.revision(), 2);
    assert!(view.document.take_clips().unwrap().is_empty());
    let undone = fixture.current_row();
    let replay = attach(&mut service, &fixture.request);
    assert_eq!(commitment(&replay).completion, CaptureAttachmentCompletion::RecordedReplay);
    assert_eq!(commitment(&replay).committed_revision, 1);
    assert_eq!(fixture.current_row(), undone, "Historical replay changed the current Undo state");
    let Reply::Document { view, .. } = call(&mut service, Command::Open { project_id: fixture.request.publication.project_id.clone() }).unwrap() else { panic!("Missing independent current read after Undo replay") };
    assert_eq!(view.document.revision(), 2);
    assert!(view.document.take_clips().unwrap().is_empty(), "Current Open reinstalled the historical capture placement");
    let Reply::Document { view, .. } = call(&mut service, Command::Edit { intent: navigate(&view.document, "redo-capture", EditOperation::Redo) }).unwrap() else { panic!("Missing capture Redo") };
    assert_eq!(view.document.revision(), 3);
    let Reply::Document { view, .. } = call(&mut service, Command::Edit { intent: rename(&view.document, "later-name", "Later saved arrangement") }).unwrap() else { panic!("Missing later edit") };
    assert_eq!(view.document.revision(), 4);
    let later = fixture.current_row();
    let replay = attach(&mut service, &fixture.request);
    assert_eq!(commitment(&replay).completion, CaptureAttachmentCompletion::RecordedReplay);
    assert_eq!(commitment(&replay).committed_revision, 1);
    assert_eq!(fixture.current_row(), later, "Replay replaced the newer current arrangement");
    assert_eq!(fixture.owned_originals(), originals);
    fixture.unchanged_stage();
    let Reply::Document { view, .. } = call(&mut service, Command::Open { project_id: fixture.request.publication.project_id.clone() }).unwrap() else { panic!("Missing independent current read after replay") };
    assert_eq!(view.document.name(), "Later saved arrangement");
    assert_eq!(view.document.revision(), 4);

    // Historical receipt lookup needs neither the old locator nor the owned
    // source. A failed later Open cannot alter this recorded commitment.
    fs::remove_file(&fixture.request.journal_path).unwrap();
    for (path, _) in originals { fs::remove_file(path).unwrap(); }
    let replay = attach(&mut service, &fixture.request);
    assert_eq!(commitment(&replay).completion, CaptureAttachmentCompletion::RecordedReplay);
    assert_eq!(fixture.current_row(), later);
    assert!(call(&mut service, Command::Open { project_id: fixture.request.publication.project_id.clone() }).is_err());
    assert_eq!(commitment(&replay).committed_revision, 1);
    assert_eq!(fixture.current_row(), later);
}

#[test]
fn discarded_first_acknowledgement_is_reconciled_by_the_entire_exact_request() {
    let fixture = Fixture::new();
    let mut store = NativeStore::open_existing(&fixture.paths.native).unwrap();
    let first = store.attach_capture(&fixture.request.journal_path, &fixture.request.publication).unwrap();
    assert!(!first.replayed);
    drop(first); // Models a committed operation whose acknowledgement was lost.
    drop(store);
    let before = fixture.current_row();
    let originals = fixture.owned_originals();
    let mut service = fixture.worker();
    let replay = attach(&mut service, &fixture.request);
    assert_eq!(commitment(&replay).completion, CaptureAttachmentCompletion::RecordedReplay);
    assert_eq!(fixture.current_row(), before);
    assert_eq!(fixture.owned_originals(), originals);
    fixture.unchanged_stage();

    let mut different_locator = fixture.request.clone();
    different_locator.journal_path = fixture.root.join("different-missing-locator.sqlite3");
    assert_ne!(different_locator.fingerprint().unwrap(), fixture.request.fingerprint().unwrap());
    // The persisted publication receipt can still attest musical commitment;
    // its separate compact response correlates only to this changed request.
    let alternate = attach(&mut service, &different_locator);
    assert_eq!(commitment(&alternate).completion, CaptureAttachmentCompletion::RecordedReplay);
    assert!(!alternate.matches_request_reference(&fixture.request));
    assert_eq!(fixture.current_row(), before);
}

#[test]
fn wrong_origin_and_unopened_store_are_proven_preinvocation_refusals() {
    let fixture = Fixture::new();
    let mut unopened = SessionService::reconnect(fixture.paths.clone()).unwrap();
    let refused = attach(&mut unopened, &fixture.request);
    assert!(!refused.store_invoked);
    assert!(matches!(refused.outcome, CaptureAttachmentOutcome::Rejected { .. }));
    assert!(!refused.failure_error().unwrap().outcome_unknown);
    assert!(fixture.owned_originals().is_empty());
    drop(unopened);
    let mut service = fixture.worker();
    let mut wrong = fixture.request.clone();
    wrong.origin_root = fixture.root.join("another-store");
    let refused = attach(&mut service, &wrong);
    assert!(!refused.store_invoked);
    assert_eq!(refused.failure_error().unwrap().kind, FailureKind::Rejected);
    assert_eq!(fixture.current_row().1, 0);
    assert!(fixture.owned_originals().is_empty());
    fixture.unchanged_stage();
}

#[test]
fn stale_revision_refuses_publication_but_preserves_the_ingested_original() {
    let fixture = Fixture::new();
    let mut store = NativeStore::open_existing(&fixture.paths.native).unwrap();
    store.apply_edit(&rename(&fixture.project, "before-capture", "Changed before capture")).unwrap();
    drop(store);
    let before = fixture.current_row();
    let mut service = fixture.worker();
    let refused = attach(&mut service, &fixture.request);
    assert!(refused.store_invoked);
    let CaptureAttachmentOutcome::Rejected { problem } = &refused.outcome else { panic!("Expected typed revision conflict") };
    assert_eq!(problem.kind, CaptureAttachmentProblemKind::Conflict);
    assert_eq!(refused.failure_error().unwrap().kind, FailureKind::RevisionConflict);
    assert!(!refused.failure_error().unwrap().outcome_unknown);
    assert_eq!(fixture.current_row(), before);
    let originals = fixture.owned_originals();
    assert_eq!(originals.len(), 1, "No project commit must not be reported as no ingestion effects");
    let refused_again = attach(&mut service, &fixture.request);
    assert!(matches!(refused_again.outcome, CaptureAttachmentOutcome::Rejected { .. }));
    assert_eq!(fixture.owned_originals(), originals);
    fixture.unchanged_stage();
}

#[test]
fn operation_reuse_does_not_rebase_or_replace_the_saved_publication() {
    let fixture = Fixture::new();
    let mut service = fixture.worker();
    let saved = attach(&mut service, &fixture.request);
    commitment(&saved);
    let before = fixture.current_row();
    let originals = fixture.owned_originals();
    let mut reused = fixture.request.clone();
    reused.publication.track_name = "Changed request with same operation identity".into();
    let refused = attach(&mut service, &reused);
    assert!(refused.store_invoked);
    assert!(matches!(refused.outcome, CaptureAttachmentOutcome::Uncertain { .. }));
    assert!(refused.failure_error().unwrap().outcome_unknown);
    assert_eq!(fixture.current_row(), before);
    assert_eq!(fixture.owned_originals(), originals);
    let replay = attach(&mut service, &fixture.request);
    assert_eq!(commitment(&replay).completion, CaptureAttachmentCompletion::RecordedReplay);
    fixture.unchanged_stage();
}

#[test]
fn capture_commitment_does_not_depend_on_a_postcommit_library_load() {
    let fixture = Fixture::new();
    let mut store = NativeStore::open_existing(&fixture.paths.native).unwrap();
    for index in 0..512 { store.create(&ProjectDocument::new(&format!("Synthetic library entry {index}")).unwrap()).unwrap(); }
    assert!(store.list().unwrap_err().to_string().contains("512"));
    drop(store);
    let mut service = SessionService::reconnect(fixture.paths.clone()).unwrap();
    // Initialize opens the actual store before its ordinary library load fails.
    assert!(call(&mut service, Command::Initialize).is_err());
    assert!(!service.is_disconnected());
    let saved = attach(&mut service, &fixture.request);
    assert_eq!(commitment(&saved).completion, CaptureAttachmentCompletion::FreshCommit);
    assert_eq!(fixture.current_row().1, 1);
    assert!(call(&mut service, Command::Refresh).is_err());
    assert_eq!(commitment(&saved).committed_revision, 1);
    let replay = attach(&mut service, &fixture.request);
    assert_eq!(commitment(&replay).completion, CaptureAttachmentCompletion::RecordedReplay);
    fixture.unchanged_stage();
}

#[test]
fn malformed_capture_requests_never_reach_worker_storage() {
    let fixture = Fixture::new();
    let mut service = fixture.worker();
    for request in [
        { let mut value = fixture.request.clone(); value.version = 2; value },
        { let mut value = fixture.request.clone(); value.publication.operation_id.0 = "a".repeat(97); value },
        { let mut value = fixture.request.clone(); value.publication.expected_revision = u64::MAX; value },
        { let mut value = fixture.request.clone(); value.journal_path = fixture.root.join("../aliased-stage.sqlite3"); value },
        { let mut value = fixture.request.clone(); value.journal_path = PathBuf::from("relative.sqlite3"); value },
    ] {
        let error = service.try_submit(Command::AttachCapture { request: Box::new(request) }).unwrap_err();
        assert_eq!(error.kind, SubmitFailure::Invalid);
    }
    assert_eq!(fixture.current_row().1, 0);
    assert!(fixture.owned_originals().is_empty());
    fixture.unchanged_stage();
}

#[test]
fn genuine_projection_project_accepts_normal_capture_with_a_compact_worker_commitment() {
    use omniusong_core::{NoteTimelinePolicy, contracts::{ClockEpochId as ContractClockEpochId, EndpointId, TakeId}, performance_projection::{ProjectionInstrument, ProjectionLaneChoice}};
    use omniusong_engine::{
        LiveBatch, LiveClock, LiveEvent, LiveEventKind, LiveId, LivePatch, LiveScope, LiveSourceBinding, LiveSourceKind,
        live::{LiveAdmission, LiveRenderer, recording_live_pair}, performance_records::{PerformanceConfig, PerformanceSource},
        performance_tap::PerformanceTapOwner,
        performance_writer::{PerformanceWriter, inspect_performance, inspect_sealed_performance, recover_performance},
    };
    use omniusong_performance::projection::{AuthenticatedPerformance, PerformanceFrameRegion, PerformanceProjectionDuration, PerformanceProjectionPolicy, PerformanceProjectionRequest, PerformanceProjectionRounding};
    use omniusong_store::{ProjectionPublication, projection_operation_id};
    use std::sync::{Arc, atomic::AtomicBool};

    fn budget() -> InspectionBudget {
        InspectionBudget::new(InspectionLimits::default(), Arc::new(AtomicBool::new(false))).unwrap()
    }
    fn send(admission: &mut LiveAdmission, kind: LiveEventKind, ticks: u64) {
        let progress = admission.progress();
        let mut batch = LiveBatch::empty(progress.scope, progress.next_sequence().unwrap(), admission.lease_credit().map(|credit| credit.token));
        batch.try_push(LiveEvent { source: 1, connection_generation: 1, observed_ticks: ticks, event: kind }).unwrap();
        admission.admit(batch).unwrap();
    }
    fn render(renderer: &mut LiveRenderer, owner: &mut PerformanceTapOwner, writer: &mut PerformanceWriter, frames: usize) {
        renderer.render_into(&mut vec![0.0; frames]);
        while let Some(packet) = owner.try_pop().unwrap() { writer.push_packet(packet).unwrap(); }
        writer.flush().unwrap();
    }

    // This is an actual authenticated performance projection, not a fabricated
    // schema-3 document. Its whole closure is retained while normal PCM attaches.
    let mut fixture = Fixture::new();
    let projection_path = fixture.root.join("synthetic-performance.sqlite3");
    let config = PerformanceConfig {
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        info: omniusong_engine::performance_tap::TapInfo {
            scope: LiveScope { worker_generation: LiveId([71; 16]), session_id: LiveId([72; 16]), panic_generation: 1 }, sample_rate: 8000, channels: 1,
        },
        gui_clock: LiveClock { domain: "capture-completion.gui".into(), epoch: "capture-completion.gui-epoch".into(), ticks_per_second_numerator: 1_000_000_000, ticks_per_second_denominator: 1 },
        output_epoch: ContractClockEpochId("capture-completion.render-epoch".into()),
        sources: std::array::from_fn(|index| PerformanceSource {
            binding: LiveSourceBinding {
                kind: [LiveSourceKind::ComputerKeys, LiveSourceKind::PointerKeys, LiveSourceKind::Pads, LiveSourceKind::AccessiblePulse][index],
                handle: index as u8 + 1, connection_generation: 1,
            },
            endpoint: EndpointId(format!("capture-completion.source-{}", index + 1)),
        }),
    };
    let mut writer = PerformanceWriter::create(&projection_path, config.clone()).unwrap();
    let (mut admission, mut renderer, mut owner) = recording_live_pair(config.info.scope, 8000, 1).unwrap();
    send(&mut admission, LiveEventKind::NoteOn { note: 1, voice: 1, key: 60, velocity16: 0, patch: LivePatch::Keys }, 1);
    render(&mut renderer, &mut owner, &mut writer, 80);
    send(&mut admission, LiveEventKind::NoteOff { voice: 1, release_velocity16: 65535 }, 2);
    render(&mut renderer, &mut owner, &mut writer, 2000);
    drop(renderer);
    writer.close_interrupted().unwrap();
    let reader = CaptureReader::open(&projection_path).unwrap();
    let inspection = inspect_performance(&reader).unwrap();
    assert!(inspection.problem.is_none() && inspection.selection.is_some());
    recover_performance(reader, &inspection, "capture-completion.recovered").unwrap();
    let staged_projection_bytes = fs::read(&projection_path).unwrap();
    let original = SealedJournal::open(&projection_path).unwrap();
    let mut store = NativeStore::open_existing(&fixture.paths.native).unwrap();
    let object = store.ingest_take(&original).unwrap();
    drop(original);
    let reference = TakeReference { take_id: object.take_id, manifest_sha256: object.manifest_sha256, stream_id: "audio".into() };
    let governor = budget();
    let reader = store.open_take_with_budget(&reference, &governor).unwrap();
    let authenticated = AuthenticatedPerformance::open(&reader).unwrap();
    let inspection = inspect_sealed_performance(&reader).unwrap();
    let request = PerformanceProjectionRequest {
        version: 1, source: authenticated.source_pin().clone(), region: PerformanceFrameRegion { start: 0, end: inspection.checkpoint.unwrap().audio_end },
        sources: vec![1], tempo_bits: fixture.project.tempo().to_bits(), destination_start_tick: 1000,
        rounding: PerformanceProjectionRounding::NearestTiesUp, duration: PerformanceProjectionDuration::ReleaseGate,
        collapsed: PerformanceProjectionPolicy::Reject, incomplete: PerformanceProjectionPolicy::Reject, region_crossing: PerformanceProjectionPolicy::Reject,
    };
    let review = authenticated.project(&request).unwrap();
    assert_eq!(review.notes.len(), 1);
    let project_id = ProjectId(fixture.project.id().into());
    let publication = ProjectionPublication {
        version: 1, operation_id: projection_operation_id(&project_id, "capture-completion-projection").unwrap(), project_id,
        expected_revision: fixture.project.revision(), projection_id: "capture-completion-projection".into(), request,
        expected_review_sha256: review.review_sha256,
        lanes: vec![ProjectionLaneChoice { source: 1, instrument: ProjectionInstrument::Keys, track_id: "capture-completion-note-lane".into(), track_name: "Synthetic projected notes".into() }],
        timeline: NoteTimelinePolicy::PreserveLoop, allow_projection_schema_upgrade: true,
    };
    drop(authenticated);
    drop(reader);
    store.publish_performance_projection(&publication, &budget()).unwrap();
    fixture.project = store.load(fixture.project.id()).unwrap();
    assert_eq!((fixture.project.schema_version(), fixture.project.revision()), (3, 1));
    let projection_before = fixture.project.value()["performanceProjections"].clone();
    drop(store);
    fixture.request.publication.expected_revision = 1;
    let original_projection_files = fixture.owned_originals();
    assert_eq!(original_projection_files.len(), 1);

    let mut service = fixture.worker();
    let saved = attach(&mut service, &fixture.request);
    assert_eq!((commitment(&saved).previous_revision, commitment(&saved).committed_revision), (1, 2));
    assert_eq!(commitment(&saved).completion, CaptureAttachmentCompletion::FreshCommit);
    let Reply::Document { view, .. } = call(&mut service, Command::Open { project_id: fixture.request.publication.project_id.clone() }).unwrap() else { panic!("Missing explicit current projection read") };
    assert_eq!((view.document.schema_version(), view.document.revision()), (3, 2));
    assert_eq!(view.document.value()["performanceProjections"], projection_before);
    assert_eq!(view.document.take_clips().unwrap(), vec![fixture.request.publication.clip.clone()]);
    assert_eq!(fixture.owned_originals().len(), 2);
    for (path, bytes) in original_projection_files { assert_eq!(fs::read(path).unwrap(), bytes); }
    assert_eq!(fs::read(&projection_path).unwrap(), staged_projection_bytes);
    fixture.unchanged_stage();
    let replay = attach(&mut service, &fixture.request);
    assert_eq!(commitment(&replay).completion, CaptureAttachmentCompletion::RecordedReplay);
    assert_eq!(commitment(&replay).committed_revision, 2);
}
