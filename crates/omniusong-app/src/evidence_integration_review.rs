//! Native admission and draft preservation, using the real session client with
//! injected replies/engine ports. No native device or user data is accessed.
use super::evidence_ui::EvidenceAction;
use super::performance_source_integration_review::{
    held_drafts, manual as source_manual, no_continuation, preserved, retained_metadata_retry,
};
use super::*;
use omniusong_session::{EvidenceJobList, Request, ServiceError, TestPeer};

fn manual() -> (StudioApp, TestPeer, audio_ui::test_support::Peer) {
    let (mut app, peer, engine) = source_manual();
    app.native_origin = Some(app.paths.native.clone());
    let generation = app.service.as_ref().unwrap().generation();
    app.evidence.generation_changed(generation);
    app.sync_evidence_context();
    (app, peer, engine)
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

#[test]
fn canonical_library_origin_is_retained_without_changing_the_musical_document() {
    let (mut app, peer, engine) = manual();
    let before = preserved(&app);
    let canonical = if cfg!(windows) {
        PathBuf::from(r"C:\synthetic\canonical-native")
    } else {
        PathBuf::from("/synthetic/canonical-native")
    };
    app.library(Library {
        native_root: Some(canonical.clone()),
        ..Default::default()
    });
    assert_eq!(app.paths.native, canonical);
    assert_eq!(app.native_origin.as_ref(), Some(&canonical));
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}

const JOB: &str = "11111111111111111111111111111111";
fn view(app: &StudioApp) -> omniusong_session::EvidenceJobView {
    let request = omniusong_session::EvidenceJobRequest {
        version: 1,
        job_id: JOB.into(),
        origin_root: app.paths.native.to_str().unwrap().into(),
        operation: omniusong_session::EvidenceOperation::Export {
            scope: omniusong_store::EVIDENCE_SCOPE.into(),
            export_id: JOB.into(),
            destination: app.paths.native.with_file_name("synthetic-evidence.osre"),
        },
    };
    omniusong_session::EvidenceJobView {
        job_id: JOB.into(),
        directory: app.paths.native.with_file_name("synthetic-job"),
        request_sha256: Some(request.fingerprint().unwrap()),
        request: Some(request),
        history_sha256: "a".repeat(64),
        started: true,
        terminal: None,
        observations: vec![],
        occupied_observations: 0,
        metadata_problems: vec![],
    }
}
fn list(view: &omniusong_session::EvidenceJobView) -> Reply {
    Reply::EvidenceJobs(EvidenceJobList {
        jobs: vec![omniusong_session::EvidenceJobSummary {
            job_id: view.job_id.clone(),
            request_sha256: view.request_sha256.clone(),
            history_sha256: view.history_sha256.clone(),
            phase: omniusong_session::EvidenceJobPhase::Started,
            latest_observation: None,
            metadata_problem_count: 0,
        }],
    })
}
fn admit(app: &mut StudioApp, peer: &TestPeer, request: EvidenceUiRequest) -> Request {
    app.handle_evidence_action(EvidenceAction::Submit(request.clone()));
    let actual = peer.requests.try_recv().unwrap();
    assert!(request.matches_command(&actual.command));
    assert_eq!(app.active.as_ref().unwrap().id, actual.id);
    assert!(app.active.as_ref().unwrap().after.is_none() && app.busy());
    actual
}
#[test]
fn explicit_metadata_and_artifact_reads_preserve_every_musical_draft_failure_and_retry() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    held_drafts(&mut app);
    app.sync_evidence_context();
    let before = preserved(&app);
    let observed = view(&app);
    let request = app.evidence.list_request().unwrap();
    let actual = admit(&mut app, &peer, request);
    answer(&mut app, &peer, &actual, Ok(list(&observed)));
    assert_eq!(preserved(&app), before);
    assert!(app.evidence.select_job(JOB));
    let request = app.evidence.read_request().unwrap();
    let actual = admit(&mut app, &peer, request);
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::EvidenceJob(Box::new(observed))),
    );
    assert_eq!(preserved(&app), before);
    let request = app.evidence.reconcile_request().unwrap();
    let actual = admit(&mut app, &peer, request);
    answer(
        &mut app,
        &peer,
        &actual,
        Err(ServiceError::backend(
            "injected uncertain reconciliation",
            true,
        )),
    );
    assert_eq!(preserved(&app), before);
    app.evidence.set_archive_path(
        app.paths
            .native
            .with_file_name("synthetic-review.osre")
            .to_str()
            .unwrap()
            .into(),
    );
    let request = app.evidence.inspect_request().unwrap();
    let actual = admit(&mut app, &peer, request);
    answer(
        &mut app,
        &peer,
        &actual,
        Err(ServiceError {
            kind: FailureKind::SourceUnavailable,
            message: "injected local archived-catalog refusal".into(),
            outcome_unknown: false,
        }),
    );
    assert_eq!(preserved(&app), before);
    assert!(app.ready && !app.disconnected() && !app.busy());
    no_continuation(&peer, &engine);
}
#[test]
fn raw_or_mismatched_evidence_routes_and_any_continuation_cannot_acquire_credit() {
    let (mut app, peer, engine) = manual();
    let before = preserved(&app);
    let request = app.evidence.list_request().unwrap();
    assert!(
        app.submit(Command::ListEvidenceJobs, Action::Refresh, None)
            .is_err()
    );
    assert!(
        app.submit(Command::Refresh, Action::Evidence(request.clone()), None)
            .is_err()
    );
    assert!(
        app.dispatch(Action::Evidence(request.clone()), Some(Action::Create))
            .is_err()
    );
    assert_eq!(preserved(&app), before);
    assert!(!app.busy() && app.active.is_none());
    no_continuation(&peer, &engine);
    let actual = admit(&mut app, &peer, request);
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::EvidenceJobs(EvidenceJobList::default())),
    );
    assert_eq!(preserved(&app), before);
}
#[test]
fn exact_cancel_keeps_both_credits_and_does_not_control_the_audio_worker() {
    let (mut app, peer, engine) = manual();
    app.evidence.set_archive_path(
        app.paths
            .native
            .with_file_name("synthetic-inspect.osre")
            .to_str()
            .unwrap()
            .into(),
    );
    let before = preserved(&app);
    let request = app.evidence.inspect_request().unwrap();
    let actual = admit(&mut app, &peer, request);
    app.handle_evidence_action(EvidenceAction::Cancel(RequestId {
        generation: actual.id.generation,
        sequence: actual.id.sequence + 1,
    }));
    assert_eq!(app.evidence.pending_cancel(), Some(actual.id));
    app.handle_evidence_action(EvidenceAction::Cancel(actual.id));
    assert!(app.busy() && app.active.as_ref().unwrap().id == actual.id);
    assert!(app.service.as_ref().unwrap().pending().is_some());
    assert!(
        app.service
            .as_mut()
            .unwrap()
            .try_submit(Command::Refresh)
            .is_err()
    );
    assert!(engine.command().is_none() && engine.urgent().is_none());
    answer(
        &mut app,
        &peer,
        &actual,
        Err(ServiceError {
            kind: FailureKind::InspectionCancelled,
            message: "injected cancelled read".into(),
            outcome_unknown: false,
        }),
    );
    assert!(!app.busy() && app.active.is_none());
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}
#[test]
fn evidence_payload_mismatch_cannot_consume_the_current_native_action() {
    let (mut app, peer, engine) = manual();
    let request = app.evidence.list_request().unwrap();
    let actual = admit(&mut app, &peer, request);
    app.complete(Completion {
        id: actual.id,
        kind: actual.command.kind(),
        result: Ok(Reply::Library(Library::default())),
    });
    assert_eq!(app.active.as_ref().unwrap().id, actual.id);
    assert!(app.busy());
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::EvidenceJobs(EvidenceJobList::default())),
    );
    assert!(!app.busy() && app.active.is_none());
    no_continuation(&peer, &engine);
}
#[test]
fn metadata_only_reconnect_preserves_drafts_and_sends_no_initialize_or_retry() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    held_drafts(&mut app);
    let before = preserved(&app);
    let old_generation = app.service.as_ref().unwrap().generation();
    drop(peer);
    app.poll();
    assert!(app.disconnected());
    app.connect_evidence_history().unwrap();
    assert_ne!(app.service.as_ref().unwrap().generation(), old_generation);
    assert!(app.evidence_metadata_only && !app.ready && !app.busy());
    assert!(app.service.as_ref().unwrap().pending().is_none() && app.active.is_none());
    assert_eq!(preserved(&app), before);
    assert!(app.evidence_availability().can_submit && !app.evidence_availability().can_run);
    assert!(engine.command().is_none() && engine.urgent().is_none());
}

fn export_request(app: &mut StudioApp) -> EvidenceUiRequest {
    app.sync_evidence_context();
    app.evidence.set_export_path(
        app.paths
            .native
            .with_file_name("fresh-synthetic.osre")
            .to_str()
            .unwrap()
            .into(),
    );
    let availability = app.evidence_availability();
    app.evidence.review_export(&availability).unwrap();
    app.evidence.confirm_request().unwrap()
}
#[test]
fn new_evidence_jobs_recheck_all_native_barriers_without_saving_or_discarding() {
    for barrier in 0..12 {
        let (mut app, peer, engine) = manual();
        let request = export_request(&mut app);
        match barrier {
            0 => app.draft.name.push_str(" unsaved"),
            1 => app.ready = false,
            2 => app.pending = Some(Action::Create),
            3 => app.closing = true,
            4 => app.audio_close_requested = true,
            5 => app.recovery_close_requested = true,
            6 => app.note_close_requested = true,
            7 => app.clip_close_requested = true,
            8 => app.native_origin = None,
            9 => {
                retained_metadata_retry(&mut app, &peer);
            }
            10 => {
                held_drafts(&mut app);
            }
            11 => app.selected_track += 1, // Clean but changed musical context.
            _ => unreachable!(),
        }
        let before = preserved(&app);
        assert!(
            app.dispatch(Action::Evidence(request), None).is_err(),
            "barrier {barrier}"
        );
        assert!(!app.busy() && app.active.is_none());
        assert_eq!(preserved(&app), before, "barrier {barrier}");
        no_continuation(&peer, &engine);
    }
}
#[test]
fn context_changes_while_evidence_window_is_closed_require_a_new_explicit_review() {
    let (mut app, peer, engine) = manual();
    let request = export_request(&mut app);
    assert!(!app.evidence.open);
    let original = app.draft.name.clone();
    app.draft.name.push_str(" changed");
    app.sync_evidence_context();
    app.draft.name = original;
    app.sync_evidence_context();
    assert!(app.evidence.run_request().is_none());
    assert!(app.dispatch(Action::Evidence(request), None).is_err());
    no_continuation(&peer, &engine);
    let request = export_request(&mut app);
    assert!(
        app.validate_evidence_submission(
            &request.command(),
            &Action::Evidence(request.clone()),
            None
        )
        .is_ok()
    );
}
#[test]
fn unrelated_terminal_storage_failure_invalidates_an_offered_evidence_confirmation() {
    let (mut app, peer, engine) = manual();
    let request = export_request(&mut app);
    let before = preserved(&app);
    app.dispatch(Action::PerformanceSource(SourceRequest::Scan), None)
        .unwrap();
    let actual = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &actual,
        Err(ServiceError::storage_unavailable(
            "injected current catalog failure",
            false,
        )),
    );
    assert!(app.evidence.run_request().is_none());
    assert!(!app.evidence.authorizes(&request, actual.id.generation));
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}
#[test]
fn resolved_initialize_retry_no_longer_blocks_a_fresh_review_after_metadata_reconnect() {
    let (mut app, peer, engine) = manual();
    app.ready = false;
    app.native_origin = None;
    app.execute_initialize();
    let actual = peer.requests.try_recv().unwrap();
    assert!(matches!(actual.command, Command::Initialize));
    answer(
        &mut app,
        &peer,
        &actual,
        Err(ServiceError::storage_unavailable(
            "injected initial unavailable catalog",
            false,
        )),
    );
    assert!(
        app.retry
            .as_ref()
            .is_some_and(|retry| matches!(retry.command, Command::Initialize))
    );
    app.connect_evidence_history().unwrap();
    assert!(app.service.as_ref().unwrap().pending().is_none());
    // Replace only the unopened worker transport to inject the user's later
    // explicit successful Open catalog; no real filesystem operation is claimed.
    let (service, fresh_peer) = SessionService::test_pair();
    let generation = service.generation();
    app.service = Some(service);
    app.evidence.generation_changed(generation);
    app.performance_sources.generation_changed(generation);
    app.execute_initialize();
    let actual = fresh_peer.requests.try_recv().unwrap();
    let root = app.paths.native.clone();
    answer(
        &mut app,
        &fresh_peer,
        &actual,
        Ok(Reply::Library(Library {
            native_root: Some(root),
            ..Default::default()
        })),
    );
    assert!(app.ready && app.retry.is_none() && !app.evidence_metadata_only);
    let request = export_request(&mut app);
    assert!(app.evidence_availability().can_run);
    assert!(
        app.validate_evidence_submission(
            &request.command(),
            &Action::Evidence(request.clone()),
            None
        )
        .is_ok()
    );
    no_continuation(&fresh_peer, &engine);
}
#[test]
fn fresh_metadata_connection_refuses_alias_configuration_without_inferring_a_new_origin() {
    let (mut app, peer, engine) = manual();
    app.service = None;
    app.native_origin = None;
    app.paths.native = PathBuf::from("relative-original-studio");
    let error = app.connect_evidence_history().unwrap_err().to_string();
    assert!(error.contains("exact original absolute") && error.contains("--data-dir"));
    assert!(app.service.is_none() && app.native_origin.is_none());
    no_continuation(&peer, &engine);
}
#[test]
fn native_history_window_paints_and_lists_explicitly_without_catalog_initialization() {
    use super::performance_source_integration_review::{click, frame};
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    held_drafts(&mut app);
    app.ready = false;
    app.native_origin = None;
    app.evidence_metadata_only = true;
    app.evidence.open = true;
    let before = preserved(&app);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    for _ in 0..3 {
        let _ = frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
    click(&ctx, &mut app, "Job history");
    no_continuation(&peer, &engine);
    click(&ctx, &mut app, "List jobs");
    let actual = peer.requests.try_recv().unwrap();
    assert!(matches!(actual.command, Command::ListEvidenceJobs));
    assert!(app.active.as_ref().unwrap().after.is_none());
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::EvidenceJobs(EvidenceJobList::default())),
    );
    assert_eq!(preserved(&app), before);
    assert!(!app.ready && app.evidence_metadata_only);
    no_continuation(&peer, &engine);
}
#[test]
fn active_output_blocks_new_jobs_and_artifact_reads_but_keeps_metadata_available() {
    let (mut app, peer, engine) = manual();
    let request = export_request(&mut app);
    audio_ui::test_support::hold_output(&mut app.audio);
    assert!(app.audio.blocks_session());
    let before = preserved(&app);
    assert!(app.dispatch(Action::Evidence(request), None).is_err());
    app.evidence.set_archive_path(
        app.paths
            .native
            .with_file_name("synthetic-busy.osre")
            .to_str()
            .unwrap()
            .into(),
    );
    let request = app.evidence.inspect_request().unwrap();
    assert!(app.dispatch(Action::Evidence(request), None).is_err());
    no_continuation(&peer, &engine);
    let request = app.evidence.list_request().unwrap();
    let actual = admit(&mut app, &peer, request);
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::EvidenceJobs(EvidenceJobList::default())),
    );
    assert!(app.audio.blocks_session());
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}
#[test]
fn positive_native_job_route_retains_primary_metadata_and_retirement_before_disconnect() {
    use omniusong_session::{
        EvidenceBudgetStop, EvidenceJobReply, EvidenceMessage, EvidenceOutcome, EvidenceProblem,
        EvidenceProblemKind, EvidenceResourceOrigin,
    };
    let (mut app, peer, engine) = manual();
    let request = export_request(&mut app);
    let Command::RunEvidenceJob { request: exact } = request.command() else {
        panic!("run")
    };
    let actual = admit(&mut app, &peer, request);
    app.draft.name = "A newer unsaved draft while the result is pending".into();
    let before = preserved(&app);
    let outcome = EvidenceOutcome::Stopped {
        problem: EvidenceProblem {
            kind: EvidenceProblemKind::Sqlite,
            origin: EvidenceResourceOrigin::LiveCatalog,
            detail: EvidenceMessage {
                message: "injected original live SQLite cause".into(),
                message_truncated: false,
            },
            sqlite_code: Some(10),
            budget_stop: Some(EvidenceBudgetStop::Cancelled),
        },
    };
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::EvidenceJobOutcome(Box::new(EvidenceJobReply {
            job_id: exact.job_id.clone(),
            request_sha256: exact.fingerprint().unwrap(),
            history_sha256: None,
            attempted: true,
            outcome: Some(outcome.clone()),
            metadata_problem: Some(EvidenceMessage {
                message: "injected terminal metadata failure".into(),
                message_truncated: false,
            }),
            catalog_retirement: Some(ServiceError::storage_unavailable(
                "injected current retirement",
                true,
            )),
        }))),
    );
    assert!(app.disconnected() && !app.busy() && app.active.is_none());
    assert!(!app.ready && app.retry.is_none() && app.evidence.run_request().is_none());
    let cards = app.evidence.cards_for_test();
    let card = cards
        .iter()
        .find(|card| card.job_id.as_deref() == Some(&exact.job_id) && card.outcome.is_some())
        .unwrap();
    assert_eq!(card.outcome.as_ref(), Some(&outcome));
    assert!(
        card.metadata
            .as_ref()
            .unwrap()
            .message
            .contains("metadata failure")
    );
    assert!(card.retirement.as_ref().unwrap().contains("retirement") && card.historical);
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
    app.connect_evidence_history().unwrap();
    assert!(app.service.as_ref().unwrap().pending().is_none() && app.evidence_metadata_only);
    assert_eq!(preserved(&app), before);
}
#[test]
fn native_evidence_eof_retains_the_exact_job_without_generic_retry_or_continuation() {
    let (mut app, peer, engine) = manual();
    let request = export_request(&mut app);
    let Command::RunEvidenceJob { request: exact } = request.command() else {
        panic!("run")
    };
    let _actual = admit(&mut app, &peer, request);
    let before = preserved(&app);
    drop(peer);
    app.poll();
    assert!(app.disconnected() && !app.busy() && app.active.is_none() && app.retry.is_none());
    let cards = app.evidence.cards_for_test();
    let card = cards
        .iter()
        .find(|card| card.job_id.as_deref() == Some(&exact.job_id))
        .unwrap();
    assert_eq!(card.request.as_deref(), Some(exact.as_ref()));
    assert!(card.outcome.is_none() && card.historical);
    assert_eq!(preserved(&app), before);
    assert!(engine.command().is_none() && engine.urgent().is_none());
}
#[test]
fn explicit_cold_history_launch_never_initializes_or_recreates_an_absent_studio() {
    use super::performance_source_integration_review::frame;
    use std::fs;
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/evidence-ui-cold-tests")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&parent).unwrap();
    let parent = fs::canonicalize(parent).unwrap();
    let origin = parent.join("absent-original-studio");
    let (args, history_only) = launch_arguments(vec![
        "--evidence-history".into(),
        "--data-dir".into(),
        origin.to_str().unwrap().into(),
    ])
    .unwrap();
    let paths = Paths::parse(args).unwrap().unwrap();
    let mut app = StudioApp::new(paths).with_evidence_history(history_only);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    for _ in 0..5 {
        let _ = frame(&ctx, &mut app, vec![]);
    }
    assert!(app.evidence_metadata_only && app.evidence.open && !app.ready && !app.busy());
    assert!(
        app.service.is_some()
            && app.active.is_none()
            && app.service.as_ref().unwrap().pending().is_none()
    );
    assert!(!origin.exists() && fs::read_dir(&parent).unwrap().next().is_none());
    drop(app);
    fs::remove_dir_all(parent).unwrap();
}
