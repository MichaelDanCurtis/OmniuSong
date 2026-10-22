//! Independent parent routing/credit/draft tests. All documents, replies and
//! audio ports are synthetic. No session backend, filesystem or device is used.
use super::*;
use crate::clip_ui::{ClipAction, ClipDraftFields, ClipSelection, FieldMode};
use crate::performance_source_ui::{SourceAction, SourceRequest};
use omniusong_core::{DocumentClipAddress, DocumentNoteAddress};
use omniusong_session::{
    CommandKind, PerformanceSource, PerformanceSourceReview, PerformanceSourceScan, Request,
    ServiceError, TestPeer,
};
use serde_json::json;

const A: &str = "11111111111111111111111111111111";
const B: &str = "22222222222222222222222222222222";
const NATIVE: &str = if cfg!(windows) {
    r"C:\synthetic\performance-source-integration\no-files\native"
} else {
    "/synthetic/performance-source-integration/no-files/native"
};

fn document() -> ProjectDocument {
    let mut value = ProjectDocument::new("Synthetic source routing review")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["id"] = "original-notes".into();
    value["tracks"][0]["notes"] = json!([{
        "id":"original-note", "pitch":60, "startTick":17, "durationTicks":961,
        "velocity16":32769, "expression":null,
        "input":{"sourceId":"original-keys","sourceName":"Original keys","protocol":"midi2",
            "group":15,"channel":15,"receivedTimeMs":1.25,"releaseVelocity16":60001,"keyDurationTicks":300}
    }]);
    value["tracks"][1]["id"] = "original-audio".into();
    value["tracks"][1]["kind"] = "audio".into();
    value["tracks"][1]["assetId"] = format!("{}.wav", "a".repeat(64)).into();
    value["tracks"][1]["audio"] = json!({
        "origin":"import", "mimeType":"audio/wav", "durationSeconds":2.0,
        "byteLength":100, "trimStartSeconds":0.125, "vendor":{"original":"retained"}
    });
    ProjectDocument::from_value(value).unwrap()
}

fn manual() -> (StudioApp, TestPeer, audio_ui::test_support::Peer) {
    let (service, peer) = SessionService::test_pair();
    let generation = service.generation();
    let mut app = StudioApp::new(Paths {
        native: NATIVE.into(),
        legacy: PathBuf::from(NATIVE).with_file_name("legacy"),
    });
    assert!(app.service.is_none(), "construction must remain pure");
    let (audio, engine) = audio_ui::test_support::pair();
    app.audio = audio;
    app.service = Some(service);
    app.ready = true;
    app.performance_sources.generation_changed(generation);
    app.install_document(DocumentView::prepare(document()).unwrap());
    (app, peer, engine)
}

fn scan() -> PerformanceSourceScan {
    PerformanceSourceScan {
        sources: [A, B]
            .into_iter()
            .map(|stage| PerformanceSource {
                stage_id: stage.into(),
                journal_path: PathBuf::from(NATIVE)
                    .join("captures")
                    .join(stage)
                    .join("take.sqlite3"),
                problem: None,
            })
            .collect(),
        examined_entries: 4,
        skipped_generic: 1,
        skipped_unknown: 1,
        truncated: false,
    }
}

fn review(stage: &str) -> PerformanceSourceReview {
    // A valid bounded observation of an empty v2 original. It deliberately
    // supplies no complete checkpoint, manifest, native permit or publication.
    let session_id = if stage == A {
        vec![17u8; 16]
    } else {
        vec![34u8; 16]
    };
    let config: omniusong_engine::performance_records::PerformanceConfig =
        serde_json::from_value(json!({
            "take_id":format!("performance-{stage}"),
            "info":{"scope":{"worker_generation":vec![33u8;16],"session_id":session_id,"panic_generation":1},"sample_rate":48000,"channels":2},
            "gui_clock":{"domain":"synthetic-gui","epoch":"synthetic-gui-clock","ticks_per_second_numerator":1000000000u64,"ticks_per_second_denominator":1},
            "output_epoch":"synthetic-render-clock",
            "sources":[
                {"binding":{"kind":"computer_keys","handle":1,"connection_generation":1},"endpoint":"synthetic-source-1"},
                {"binding":{"kind":"pointer_keys","handle":2,"connection_generation":1},"endpoint":"synthetic-source-2"},
                {"binding":{"kind":"pads","handle":3,"connection_generation":1},"endpoint":"synthetic-source-3"},
                {"binding":{"kind":"accessible_pulse","handle":4,"connection_generation":1},"endpoint":"synthetic-source-4"}
            ]
        })).unwrap();
    config.validate().unwrap();
    let descriptor = omniusong_engine::performance_records::create_descriptor_for(
        &config,
        omniusong_engine::performance_records::PerformanceProfile::FinishV2,
    )
    .unwrap();
    serde_json::from_value(json!({
        "version":1, "stage_id":stage,
        "journal_path":PathBuf::from(NATIVE).join("captures").join(stage).join("take.sqlite3"),
        "take_id":format!("performance-{stage}"), "profile_version":2,
        "config":config, "state":"recording", "byte_length":4096,
        "manifest_sha256":null, "manifest_interrupted":false,
        "verified_payload_bytes":0, "generic_problems":[],
        "performance":{
            "descriptor_sha256":descriptor.sha256().unwrap(),
            "original_rows_sha256":"b".repeat(64),
            "journal_sha256_at_review":"c".repeat(64),
            "selection":null,"checkpoint_sha256":null,"checkpoint":null,
            "received_generic_chunks":0,"unknown_tail":true,"problem":null,
            "capture_fault":null,"capture_fault_pending":false,"finish_cut":null
        },
        "usage":{"file_bytes":4096,"physical_rows":0,"work_bytes":0,"row_visits":0,
            "chunk_visits":0,"record_visits":0,"groups":0,"vm_steps":0},
        "review_sha256":"d".repeat(64)
    }))
    .unwrap()
}

fn source_reply(request: &SourceRequest) -> Reply {
    match request {
        SourceRequest::Scan => Reply::PerformanceSources(scan()),
        SourceRequest::RefreshRequests => Reply::PerformanceIntents(vec![]),
        SourceRequest::Inspect { stage_id, .. } => {
            Reply::PerformanceSourceInspected(Box::new(review(stage_id)))
        }
    }
}

fn answer(
    app: &mut StudioApp,
    peer: &TestPeer,
    request: &Request,
    result: Result<Reply, ServiceError>,
) {
    peer.responses
        .try_send(Completion {
            id: request.id,
            kind: request.command.kind(),
            result,
        })
        .unwrap();
    app.poll();
}

fn admitted(app: &mut StudioApp, peer: &TestPeer, request: SourceRequest) -> Request {
    app.handle_performance_source_action(SourceAction::Submit(request.clone()));
    let accepted = peer.requests.try_recv().unwrap();
    assert_eq!(accepted.command.kind(), request.command().kind());
    let active = app.active.as_ref().unwrap();
    assert_eq!(active.id, accepted.id);
    assert!(
        active.after.is_none(),
        "a source read cannot carry a mutation continuation"
    );
    assert!(app.busy());
    if matches!(request, SourceRequest::Inspect { .. }) {
        assert_eq!(
            app.performance_sources.pending_inspection(),
            Some(accepted.id)
        );
    }
    accepted
}

fn seed(app: &mut StudioApp, peer: &TestPeer) {
    let request = admitted(app, peer, SourceRequest::Scan);
    answer(app, peer, &request, Ok(Reply::PerformanceSources(scan())));
    assert!(app.performance_sources.select(A));
    assert!(!app.busy() && app.active.is_none());
}

fn inspect(app: &StudioApp) -> SourceRequest {
    app.performance_sources.inspection_request().unwrap()
}

fn retained_metadata_retry(app: &mut StudioApp, peer: &TestPeer) {
    app.draft.name = "Exact unsaved metadata choice".into();
    app.draft.tempo = "123.5".into();
    app.save().unwrap();
    let request = peer.requests.try_recv().unwrap();
    assert!(matches!(request.command, Command::Edit { .. }));
    answer(
        app,
        peer,
        &request,
        Err(ServiceError::backend("retained uncertain mutation", true)),
    );
    assert!(app.retry.is_some());
    // A pending navigation continuation belongs to that exact musical choice,
    // never to an intervening source read.
    app.retry.as_mut().unwrap().after = Some(Action::Open("retained-navigation-target".into()));
}

fn held_drafts(app: &mut StudioApp) {
    let document = app.document.as_ref().unwrap();
    app.notes
        .select(
            document,
            NoteSelection {
                project_id: document.id().into(),
                address: DocumentNoteAddress {
                    track_id: "original-notes".into(),
                    note_id: "original-note".into(),
                },
            },
        )
        .unwrap();
    app.notes.draft.as_mut().unwrap().velocity = "60001".into();
    let note_intent = app.notes.intent(document, &NoteAction::Save).unwrap();
    app.notes
        .failed(note_intent, "held exact note failure".into(), false);
    app.notes.draft.as_mut().unwrap().velocity = "newer invalid note text".into();
    app.clips
        .select(
            document,
            ClipSelection {
                project_id: document.id().into(),
                address: DocumentClipAddress::LegacyAudio {
                    track_id: "original-audio".into(),
                },
            },
        )
        .unwrap();
    let ClipDraftFields::LegacyAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        unreachable!()
    };
    fields.trim_start_seconds.mode = FieldMode::Set;
    fields.trim_start_seconds.text = "0.25".into();
    let clip_intent = app.clips.intent(document, &ClipAction::Save).unwrap();
    app.clips
        .failed(clip_intent, "held exact clip failure".into(), true);
    let ClipDraftFields::LegacyAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        unreachable!()
    };
    fields.trim_start_seconds.text = "newer invalid clip text".into();
    assert!(app.dirty() && app.musical_unresolved());
}

#[derive(Debug, PartialEq, Eq)]
struct HeldChoice {
    intent: Vec<u8>,
    draft: String,
    message: String,
    known_rejected: bool,
    suspended: bool,
}
#[derive(Debug, PartialEq, Eq)]
struct RetryState {
    id: RequestId,
    command: String,
    context: Option<(String, u64)>,
    draft: Draft,
    notes: NoteContext,
    clips: ClipContext,
    after: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
struct Preserved {
    document: Vec<u8>,
    draft: Draft,
    notes: NoteContext,
    clips: ClipContext,
    note_failure: Option<HeldChoice>,
    clip_failure: Option<HeldChoice>,
    retry: Option<RetryState>,
    track: usize,
    pending: bool,
    recovery_guard: bool,
}
fn preserved(app: &StudioApp) -> Preserved {
    Preserved {
        document: app.document.as_ref().unwrap().to_json().unwrap(),
        draft: app.draft.clone(),
        notes: app.notes.context(),
        clips: app.clips.context(),
        note_failure: app.notes.failure.as_ref().map(|f| HeldChoice {
            intent: serde_json::to_vec(&f.intent).unwrap(),
            draft: format!("{:?}", f.draft),
            message: f.message.clone(),
            known_rejected: f.known_rejected,
            suspended: f.suspended,
        }),
        clip_failure: app.clips.failure.as_ref().map(|f| HeldChoice {
            intent: serde_json::to_vec(&f.intent).unwrap(),
            draft: format!("{:?}", f.draft),
            message: f.message.clone(),
            known_rejected: f.known_rejected,
            suspended: f.suspended,
        }),
        retry: app.retry.as_ref().map(|r| RetryState {
            id: r.id,
            command: format!("{:?}", r.command),
            context: r.context.clone(),
            draft: r.draft.clone(),
            notes: r.note_context.clone(),
            clips: r.clip_context.clone(),
            after: r.after.as_ref().map(|a| match a {
                Action::Open(id) => format!("open:{id}"),
                _ => "other continuation".into(),
            }),
        }),
        track: app.selected_track,
        pending: app.pending.is_some(),
        recovery_guard: app.recovery.blocks_session(),
    }
}

fn no_continuation(peer: &TestPeer, engine: &audio_ui::test_support::Peer) {
    assert!(
        peer.requests.try_recv().is_err(),
        "no storage continuation or generic recovery"
    );
    assert!(
        engine.command().is_none(),
        "source reads never start or continue audio"
    );
    assert!(engine.urgent().is_none());
}

#[test]
fn prepared_backend_commands_cannot_enter_raw_ui_routes_or_continue_into_audio() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    held_drafts(&mut app);
    let before = preserved(&app);
    let choice = omniusong_session::PreparedPerformanceSourceAction {
        version: 1,
        id: A.into(),
        kind: omniusong_session::PerformanceSourceActionKind::RecoverCopy,
        stage_id: B.into(),
        take_id: format!("performance-{B}"),
        review_blob_sha256: "a".repeat(64),
        admission_sha256: "b".repeat(64),
        recovery_operation_id: omniusong_core::contracts::OperationId(
            "synthetic-prepared-ui-refusal".into(),
        ),
        policy_version: 1,
        origin: None,
    };
    for command in [
        Command::ListPreparedPerformanceSourceActions,
        Command::SavePreparedPerformanceSourceAction {
            expected_revision: None,
            action: Box::new(choice),
            review: b"{}".to_vec(),
        },
    ] {
        for action in [
            Action::Refresh,
            Action::Performance(Box::new(command.clone())),
            Action::PerformanceSource(SourceRequest::Scan),
        ] {
            for after in [None, Some(Action::Refresh)] {
                let error = app
                    .submit(command.clone(), action.clone(), after)
                    .unwrap_err();
                assert!(error.to_string().contains("backend-only"));
                assert_eq!(preserved(&app), before);
                assert!(app.active.is_none() && app.pending.is_none());
                no_continuation(&peer, &engine);
            }
        }
    }
}

#[test]
fn explicit_scan_refresh_and_inspect_preserve_dirty_drafts_failed_choices_and_exact_mutation_retry()
{
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    held_drafts(&mut app);
    let before = preserved(&app);
    seed(&mut app, &peer);
    assert_eq!(preserved(&app), before);
    for request in [
        SourceRequest::Scan,
        SourceRequest::RefreshRequests,
        inspect(&app),
    ] {
        let accepted = admitted(&mut app, &peer, request.clone());
        assert_eq!(preserved(&app), before);
        assert!(
            app.pending.is_none(),
            "a read must not open a save/discard prompt"
        );
        answer(&mut app, &peer, &accepted, Ok(source_reply(&request)));
        assert!(!app.busy() && app.active.is_none());
        assert_eq!(app.performance_sources.pending_inspection(), None);
        assert_eq!(preserved(&app), before);
        no_continuation(&peer, &engine);
    }
    assert_eq!(
        app.performance_sources.visible_review().unwrap().stage_id,
        A
    );
}

#[test]
fn source_local_errors_finish_only_the_read_and_keep_a_healthy_generation_and_prior_evidence() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    held_drafts(&mut app);
    seed(&mut app, &peer);
    let request = inspect(&app);
    let first = admitted(&mut app, &peer, request.clone());
    answer(&mut app, &peer, &first, Ok(source_reply(&request)));
    let observed = app.performance_sources.last_review().unwrap().clone();
    let before = preserved(&app);
    let generation = app.service.as_ref().unwrap().generation();
    for kind in [
        FailureKind::SourceBusy,
        FailureKind::SourceUnavailable,
        FailureKind::InspectionLimit,
        FailureKind::InspectionCancelled,
        FailureKind::Protocol,
    ] {
        let request = inspect(&app);
        let accepted = admitted(&mut app, &peer, request);
        answer(
            &mut app,
            &peer,
            &accepted,
            Err(ServiceError {
                kind,
                message: format!("exact source-local {kind:?}"),
                outcome_unknown: false,
            }),
        );
        assert!(!app.busy() && !app.disconnected() && app.ready);
        assert_eq!(app.service.as_ref().unwrap().generation(), generation);
        assert!(app.active.is_none());
        assert_eq!(app.performance_sources.pending_inspection(), None);
        assert_eq!(app.performance_sources.last_review(), Some(&observed));
        assert_eq!(preserved(&app), before);
        no_continuation(&peer, &engine);
    }
}

#[test]
fn retained_note_clip_and_generic_attachment_retry_payloads_are_not_replaced_by_source_reads() {
    for family in 0..3 {
        let (mut app, peer, engine) = manual();
        retained_metadata_retry(&mut app, &peer);
        held_drafts(&mut app);
        let (command, action) = match family {
            0 => (
                Command::Edit {
                    intent: app.notes.failure.as_ref().unwrap().intent.clone(),
                },
                Action::Note(NoteAction::Retry),
            ),
            1 => (
                Command::Edit {
                    intent: app.clips.failure.as_ref().unwrap().intent.clone(),
                },
                Action::Clip(ClipAction::Retry),
            ),
            _ => {
                let publication:omniusong_store::TakePublication=serde_json::from_value(json!({
                    "operation_id":uuid::Uuid::new_v4().to_string(),
                    "project_id":app.document.as_ref().unwrap().id(),"expected_revision":0,
                    "track_id":"synthetic-retained-track","track_name":"Synthetic retained attachment",
                    "clip":{"id":"synthetic-retained-clip",
                        "source":{"takeId":"synthetic-retained-take","manifestSha256":"e".repeat(64),"streamId":"audio"},
                        "range":{"unit":"audio_frames","start":100,"count":4},
                        "position":{"frame":0,"sampleRate":48000}}
                })).unwrap();
                publication.validate().unwrap();
                let path = PathBuf::from(if cfg!(windows) {
                    r"C:\synthetic\retained-generic-original\take.sqlite3"
                } else {
                    "/synthetic/retained-generic-original/take.sqlite3"
                });
                (
                    Command::AttachCapture {
                        journal_path: path.clone(),
                        publication: publication.clone(),
                    },
                    Action::AttachCapture {
                        journal_path: path,
                        publication: Box::new(publication),
                    },
                )
            }
        };
        // Inject only the retained retry payload, never run its mutation or
        // turn an unbound synthetic take into a publication receipt.
        let retry = app.retry.as_mut().unwrap();
        retry.command = command;
        retry.action = action;
        let before = preserved(&app);
        seed(&mut app, &peer);
        let request = inspect(&app);
        let read = admitted(&mut app, &peer, request.clone());
        answer(&mut app, &peer, &read, Ok(source_reply(&request)));
        let refresh = admitted(&mut app, &peer, SourceRequest::RefreshRequests);
        answer(
            &mut app,
            &peer,
            &refresh,
            Err(ServiceError {
                kind: FailureKind::SourceUnavailable,
                message: "requests could not be loaded".into(),
                outcome_unknown: false,
            }),
        );
        assert_eq!(preserved(&app), before);
        no_continuation(&peer, &engine);
    }
}

#[test]
fn stale_generation_sequence_kind_and_reply_variant_keep_both_real_pending_credits() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    seed(&mut app, &peer);
    let before = preserved(&app);
    let request = inspect(&app);
    let accepted = admitted(&mut app, &peer, request.clone());
    for variant in 0..4 {
        let id = match variant {
            0 => RequestId {
                generation: accepted.id.generation + 1,
                ..accepted.id
            },
            1 => RequestId {
                sequence: accepted.id.sequence + 1,
                ..accepted.id
            },
            _ => accepted.id,
        };
        peer.responses
            .try_send(Completion {
                id,
                kind: if variant == 2 {
                    CommandKind::ScanPerformanceSources
                } else {
                    accepted.command.kind()
                },
                result: Ok(if variant == 3 {
                    Reply::Library(Library::default())
                } else {
                    source_reply(&request)
                }),
            })
            .unwrap();
        app.poll();
        assert!(app.busy() && !app.disconnected());
        assert_eq!(app.active.as_ref().unwrap().id, accepted.id);
        assert_eq!(
            app.service.as_ref().unwrap().pending().unwrap().id,
            accepted.id
        );
        assert_eq!(
            app.performance_sources.pending_inspection(),
            Some(accepted.id)
        );
        assert_eq!(preserved(&app), before);
    }
    // The parent is defensive even when a direct caller bypasses the service fence.
    app.complete(Completion {
        id: accepted.id,
        kind: CommandKind::Refresh,
        result: Ok(Reply::Library(Library::default())),
    });
    assert_eq!(app.active.as_ref().unwrap().id, accepted.id);
    assert_eq!(
        app.performance_sources.pending_inspection(),
        Some(accepted.id)
    );
    app.complete(Completion {
        id: accepted.id,
        kind: accepted.command.kind(),
        result: Ok(Reply::Library(Library::default())),
    });
    assert_eq!(app.active.as_ref().unwrap().id, accepted.id);
    assert_eq!(
        app.performance_sources.pending_inspection(),
        Some(accepted.id)
    );
    answer(&mut app, &peer, &accepted, Ok(source_reply(&request)));
    assert!(!app.busy() && app.active.is_none());
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}

#[test]
fn cancellation_holds_credit_until_matching_completion_and_a_stale_token_cannot_cancel_next_work() {
    for success_wins in [false, true] {
        let (mut app, peer, engine) = manual();
        retained_metadata_retry(&mut app, &peer);
        held_drafts(&mut app);
        seed(&mut app, &peer);
        let before = preserved(&app);
        let request = inspect(&app);
        let accepted = admitted(&mut app, &peer, request.clone());
        app.handle_performance_source_action(SourceAction::CancelInspection(accepted.id));
        assert!(app.busy());
        assert_eq!(app.active.as_ref().unwrap().id, accepted.id);
        assert_eq!(
            app.performance_sources.pending_inspection(),
            Some(accepted.id)
        );
        app.handle_performance_source_action(SourceAction::CancelInspection(accepted.id));
        for blocked in [
            SourceRequest::Scan,
            SourceRequest::RefreshRequests,
            request.clone(),
        ] {
            app.handle_performance_source_action(SourceAction::Submit(blocked));
            assert!(peer.requests.try_recv().is_err());
            assert_eq!(app.active.as_ref().unwrap().id, accepted.id);
            assert_eq!(
                app.performance_sources.pending_inspection(),
                Some(accepted.id)
            );
        }
        assert_eq!(preserved(&app), before);
        let outcome = if success_wins {
            Ok(source_reply(&request))
        } else {
            Err(ServiceError {
                kind: FailureKind::InspectionCancelled,
                message: "deterministic inspection cancellation".into(),
                outcome_unknown: false,
            })
        };
        answer(&mut app, &peer, &accepted, outcome);
        assert!(!app.busy() && app.active.is_none());
        assert_eq!(app.performance_sources.pending_inspection(), None);
        if success_wins {
            assert_eq!(
                app.performance_sources.visible_review().unwrap().stage_id,
                A
            );
        }
        let next_request = inspect(&app);
        let next = admitted(&mut app, &peer, next_request.clone());
        assert_ne!(next.id, accepted.id);
        app.handle_performance_source_action(SourceAction::CancelInspection(accepted.id));
        assert!(
            !app.service
                .as_ref()
                .unwrap()
                .cancel_source_inspection(accepted.id)
        );
        assert_eq!(app.performance_sources.pending_inspection(), Some(next.id));
        assert_eq!(app.active.as_ref().unwrap().id, next.id);
        assert!(app.busy() && peer.requests.try_recv().is_err());
        answer(&mut app, &peer, &next, Ok(source_reply(&next_request)));
        assert_eq!(preserved(&app), before);
        no_continuation(&peer, &engine);
    }
}

#[test]
fn scan_and_request_refresh_cannot_be_cancelled_as_inspections() {
    let (mut app, peer, engine) = manual();
    for request in [SourceRequest::Scan, SourceRequest::RefreshRequests] {
        let accepted = admitted(&mut app, &peer, request.clone());
        assert_eq!(app.performance_sources.pending_inspection(), None);
        app.handle_performance_source_action(SourceAction::CancelInspection(accepted.id));
        assert!(
            !app.service
                .as_ref()
                .unwrap()
                .cancel_source_inspection(accepted.id)
        );
        assert_eq!(app.active.as_ref().unwrap().id, accepted.id);
        assert!(app.busy());
        answer(&mut app, &peer, &accepted, Ok(source_reply(&request)));
        no_continuation(&peer, &engine);
    }
}

#[test]
fn a_to_b_to_a_selection_does_not_make_an_old_inspection_current_again() {
    let (mut app, peer, engine) = manual();
    seed(&mut app, &peer);
    let old_request = inspect(&app);
    let accepted = admitted(&mut app, &peer, old_request.clone());
    assert!(app.performance_sources.select(B));
    assert!(app.performance_sources.select(A));
    assert!(
        app.performance_sources.inspection_request().is_none(),
        "the old worker still owns its credit"
    );
    answer(&mut app, &peer, &accepted, Ok(source_reply(&old_request)));
    assert!(!app.busy() && app.active.is_none());
    assert!(app.performance_sources.visible_review().is_none());
    assert_eq!(app.performance_sources.last_review().unwrap().stage_id, A);
    let fresh_request = inspect(&app);
    assert_ne!(old_request, fresh_request);
    assert_eq!(inspect(&app), fresh_request);
    no_continuation(&peer, &engine);
    let fresh = admitted(&mut app, &peer, fresh_request.clone());
    answer(&mut app, &peer, &fresh, Ok(source_reply(&fresh_request)));
    assert_eq!(
        app.performance_sources.visible_review().unwrap().stage_id,
        A
    );
}

#[test]
fn consumed_source_payload_identity_errors_cannot_install_a_document_or_erase_previous_review() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    held_drafts(&mut app);
    seed(&mut app, &peer);
    let request = inspect(&app);
    let good = admitted(&mut app, &peer, request.clone());
    answer(&mut app, &peer, &good, Ok(source_reply(&request)));
    let observed = app.performance_sources.last_review().unwrap().clone();
    let before = preserved(&app);
    for invalid in 0..3 {
        let request = inspect(&app);
        let accepted = admitted(&mut app, &peer, request);
        let mut malformed = review(A);
        match invalid {
            0 => malformed.stage_id = B.into(),
            1 => malformed.take_id = format!("performance-{B}"),
            _ => malformed.version = 99,
        }
        answer(
            &mut app,
            &peer,
            &accepted,
            Ok(Reply::PerformanceSourceInspected(Box::new(malformed))),
        );
        assert!(!app.busy() && !app.disconnected() && app.active.is_none());
        assert_eq!(app.performance_sources.pending_inspection(), None);
        assert_eq!(app.performance_sources.last_review(), Some(&observed));
        assert_eq!(preserved(&app), before);
        no_continuation(&peer, &engine);
    }
}

#[test]
fn disconnect_during_cancel_and_fatal_storage_failure_preserve_the_exact_mutation_retry() {
    for disconnect in [false, true] {
        let (mut app, peer, engine) = manual();
        retained_metadata_retry(&mut app, &peer);
        held_drafts(&mut app);
        seed(&mut app, &peer);
        let before = preserved(&app);
        let request = inspect(&app);
        let accepted = admitted(&mut app, &peer, request);
        app.handle_performance_source_action(SourceAction::CancelInspection(accepted.id));
        if disconnect {
            let TestPeer {
                requests,
                responses,
            } = peer;
            drop(responses);
            app.poll();
            assert!(app.disconnected() && !app.busy());
            assert!(app.active.is_none());
            assert_eq!(app.performance_sources.pending_inspection(), None);
            assert_eq!(preserved(&app), before);
            assert!(requests.try_recv().is_err());
        } else {
            answer(
                &mut app,
                &peer,
                &accepted,
                Err(ServiceError::storage_unavailable(
                    "fatal catalog evidence",
                    false,
                )),
            );
            assert!(app.disconnected() && !app.busy());
            assert!(app.active.is_none());
            assert_eq!(app.performance_sources.pending_inspection(), None);
            assert_eq!(preserved(&app), before);
            assert!(peer.requests.try_recv().is_err());
        }
        assert!(engine.command().is_none() && engine.urgent().is_none());
    }
}

fn frame(ctx: &egui::Context, app: &mut StudioApp, events: Vec<egui::Event>) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1400.0),
            )),
            events,
            ..Default::default()
        },
        |ui| app.native_ui(ui),
    );
    output.textures_delta.clear();
    output
}
fn click(ctx: &egui::Context, app: &mut StudioApp, label: &str) {
    let _ = frame(ctx, app, vec![]);
    let tree = frame(ctx, app, vec![])
        .platform_output
        .accesskit_update
        .unwrap();
    let target = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label))
        .unwrap_or_else(|| panic!("missing {label}"))
        .0;
    let _ = frame(
        ctx,
        app,
        vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: tree.tree_id,
                target_node: target,
                data: None,
            },
        )],
    );
}

#[test]
fn construction_initialize_passive_request_listing_and_fresh_generation_emit_no_source_or_engine_work()
 {
    let (mut app, peer, engine) = manual();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    assert!(peer.requests.try_recv().is_err());
    app.ready = false;
    app.submit(Command::Initialize, Action::Refresh, None)
        .unwrap();
    let initialize = peer.requests.try_recv().unwrap();
    assert!(matches!(initialize.command, Command::Initialize));
    let _ = frame(&ctx, &mut app, vec![]);
    assert!(peer.requests.try_recv().is_err());
    answer(
        &mut app,
        &peer,
        &initialize,
        Ok(Reply::Library(Library::default())),
    );
    for _ in 0..3 {
        let _ = frame(&ctx, &mut app, vec![]);
    }
    no_continuation(&peer, &engine);
    // Existing recording-card listing may fan out a passive snapshot, but it
    // cannot select a saved card or turn that list into a Scan/Inspect/Start.
    app.dispatch(
        Action::Performance(Box::new(Command::ListPerformanceIntents)),
        None,
    )
    .unwrap();
    let list = peer.requests.try_recv().unwrap();
    assert!(matches!(list.command, Command::ListPerformanceIntents));
    answer(
        &mut app,
        &peer,
        &list,
        Ok(Reply::PerformanceIntents(vec![])),
    );
    for _ in 0..3 {
        let _ = frame(&ctx, &mut app, vec![]);
    }
    no_continuation(&peer, &engine);
    retained_metadata_retry(&mut app, &peer);
    let before_reconnect = preserved(&app);
    let old_generation = app.service.as_ref().unwrap().generation();
    let TestPeer {
        requests,
        responses,
    } = peer;
    drop(responses);
    app.poll();
    assert!(app.disconnected());
    assert!(requests.try_recv().is_err());
    let (service, fresh) = SessionService::test_pair();
    let generation = service.generation();
    assert_ne!(generation, old_generation);
    app.service = Some(service);
    app.ready = false;
    app.performance_sources.generation_changed(generation);
    app.submit(Command::Initialize, Action::Refresh, None)
        .unwrap();
    let initialize = fresh.requests.try_recv().unwrap();
    answer(
        &mut app,
        &fresh,
        &initialize,
        Ok(Reply::Library(Library::default())),
    );
    for _ in 0..3 {
        let _ = frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(preserved(&app), before_reconnect);
    no_continuation(&fresh, &engine);
}

#[test]
fn raw_source_command_mismatches_continuations_and_stale_selection_never_acquire_service_credit() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    seed(&mut app, &peer);
    let before = preserved(&app);
    for command in [
        Command::Create {
            name: "forbidden source mutation".into(),
        },
        Command::ScanCaptures,
        Command::InspectCapture {
            stage_id: "00000000-0000-4000-8000-000000000001".into(),
        },
        Command::ListPerformanceIntents,
    ] {
        assert!(
            app.submit(
                command,
                Action::PerformanceSource(SourceRequest::Scan),
                None
            )
            .is_err()
        );
        assert!(!app.busy() && app.active.is_none());
        assert_eq!(preserved(&app), before);
    }
    assert!(
        app.dispatch(
            Action::PerformanceSource(SourceRequest::Scan),
            Some(Action::Save)
        )
        .is_err()
    );
    assert!(
        app.submit(
            Command::ScanPerformanceSources,
            Action::PerformanceSource(SourceRequest::Scan),
            Some(Action::Save)
        )
        .is_err()
    );
    assert!(
        app.submit(Command::ScanPerformanceSources, Action::Save, None)
            .is_err()
    );
    assert!(
        app.submit(
            Command::InspectPerformanceSource { stage_id: A.into() },
            Action::Refresh,
            None
        )
        .is_err()
    );
    let stale = inspect(&app);
    assert!(app.performance_sources.select(B));
    assert!(
        app.dispatch(Action::PerformanceSource(stale), None)
            .is_err()
    );
    assert!(!app.busy() && app.performance_sources.pending_inspection().is_none());
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}

#[test]
fn rejected_initialize_cannot_replace_an_existing_mutation_retry() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    let before = preserved(&app);
    let accepted = admitted(&mut app, &peer, SourceRequest::Scan);
    assert!(
        app.submit(Command::Initialize, Action::Refresh, None)
            .is_err()
    );
    assert_eq!(app.active.as_ref().unwrap().id, accepted.id);
    assert_eq!(preserved(&app), before);
    answer(
        &mut app,
        &peer,
        &accepted,
        Ok(Reply::PerformanceSources(scan())),
    );
    let TestPeer {
        requests,
        responses,
    } = peer;
    drop(requests);
    assert!(
        app.submit(Command::Initialize, Action::Refresh, None)
            .is_err()
    );
    assert!(app.disconnected() && app.active.is_none());
    assert_eq!(preserved(&app), before);
    drop(responses);
    assert!(engine.command().is_none() && engine.urgent().is_none());
}

#[test]
fn explicit_retry_last_action_cannot_replay_a_source_read() {
    let (mut app, peer, engine) = manual();
    let request = admitted(&mut app, &peer, SourceRequest::Scan);
    let read = app.active.as_ref().unwrap().clone();
    answer(
        &mut app,
        &peer,
        &request,
        Ok(Reply::PerformanceSources(scan())),
    );
    app.retry = Some(read); // Defensive injection of an old incorrectly retained read.
    app.retry_last();
    assert!(!app.busy() && app.active.is_none());
    no_continuation(&peer, &engine);
}

#[test]
fn source_admission_obeys_audio_guard_and_stop_remains_urgent_with_injected_held_storage_credit() {
    let (mut app, peer, engine) = manual();
    seed(&mut app, &peer);
    let selected = inspect(&app);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    click(&ctx, &mut app, "Start live audition");
    let Some(omniusong_engine::EngineCommand::StartLive { gui_clock, .. }) = engine.command()
    else {
        panic!("explicit synthetic live start")
    };
    engine.reply(Ok(omniusong_engine::EngineResult::LiveStarted(
        crate::live_ui::test_support::ready(gui_clock),
    )));
    let _ = frame(&ctx, &mut app, vec![]);
    assert!(app.audio.blocks_session() && app.audio.live_guard());
    for request in [
        SourceRequest::Scan,
        SourceRequest::RefreshRequests,
        selected,
    ] {
        app.handle_performance_source_action(SourceAction::Submit(request));
        assert!(!app.busy() && app.active.is_none());
        assert!(peer.requests.try_recv().is_err());
    }
    // Inject the simultaneous active-output/held-storage state explicitly. The
    // admission rejection above remains independent of emergency Stop routing.
    let id = app
        .service
        .as_mut()
        .unwrap()
        .try_submit(Command::ScanPerformanceSources)
        .unwrap();
    let request = peer.requests.try_recv().unwrap();
    app.active = Some(Active {
        id,
        command: request.command.clone(),
        action: Action::PerformanceSource(SourceRequest::Scan),
        context: app.context(),
        draft: app.draft.clone(),
        note_context: app.notes.context(),
        clip_context: app.clips.context(),
        after: None,
    });
    app.performance_sources.submitted(&SourceRequest::Scan, id);
    click(&ctx, &mut app, "Stop audio");
    let (_, scope) = engine
        .urgent()
        .expect("Stop reaches the urgent audio port while storage is held");
    assert!(scope.is_none());
    assert!(app.busy() && app.active.as_ref().unwrap().id == id);
    assert!(peer.requests.try_recv().is_err());
    answer(
        &mut app,
        &peer,
        &request,
        Ok(Reply::PerformanceSources(scan())),
    );
    assert!(!app.busy());
    assert!(
        engine.command().is_none(),
        "a source completion cannot continue audio"
    );
}
