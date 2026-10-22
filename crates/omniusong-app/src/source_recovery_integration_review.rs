//! Coordinator regressions with pure DTO fixtures, injected session replies and
//! fake audio ports. These tests establish no filesystem, device or native proof.
use super::performance_source_integration_review::{
    held_drafts, manual as source_manual, no_continuation, preserved, retained_metadata_retry,
};
use super::source_recovery_ui::{
    SourceRecoveryAction, SourceRecoveryOperation, SourceRecoveryRequest, SourceRecoveryUi,
    test_support::{self as fixture, Phase},
};
use super::*;
use omniusong_session::{
    Backend, PerformanceSourceActionDetail, PerformanceSourceActionReference,
    PerformanceSourceAttachmentReceipt, PerformanceSourceDecision,
    PerformanceSourceDecisionReceipt, PerformanceSourceExecutionPhase,
    PerformanceSourceExecutionReceipt, PerformanceSourceReview, PerformanceSourceScan, Request,
    ServiceError, TestPeer,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const CHOICE: &str = "33333333333333333333333333333333";
const OTHER: &str = "44444444444444444444444444444444";

fn prime(app: &mut StudioApp) {
    app.sync_source_recovery_context();
    let availability = app.source_recovery_availability();
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 1000.0),
            )),
            ..Default::default()
        },
        |ui| {
            let _ = app.source_recovery.ui(ui, &availability);
        },
    );
    output.textures_delta.clear(); // Headless fixture deliberately has no renderer.
}

fn manual() -> (StudioApp, TestPeer, audio_ui::test_support::Peer) {
    let (mut app, peer, engine) = source_manual();
    app.native_origin = Some(app.paths.native.clone());
    app.source_recovery
        .generation_changed(app.service.as_ref().unwrap().generation());
    prime(&mut app);
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

fn admit(app: &mut StudioApp, peer: &TestPeer, request: SourceRecoveryRequest) -> Request {
    app.handle_source_recovery_action(SourceRecoveryAction::Submit(request.clone()));
    let actual = peer.requests.try_recv().unwrap();
    assert!(request.matches_command(&actual.command));
    let active = app.active.as_ref().unwrap();
    assert_eq!(active.id, actual.id);
    assert!(matches!(active.action, Action::SourceRecovery(_)));
    assert!(active.after.is_none());
    assert!(app.busy() && app.service.as_ref().unwrap().is_busy());
    actual
}

fn load(app: &mut StudioApp, peer: &TestPeer, detail: &PerformanceSourceActionDetail) {
    let request = app.source_recovery.list_request().unwrap();
    let actual = admit(app, peer, request);
    answer(
        app,
        peer,
        &actual,
        Ok(Reply::PerformanceSourceActionStates(vec![
            detail.state.clone(),
        ])),
    );
    assert!(
        app.source_recovery
            .select_action(&detail.state.prepared.action.id)
    );
    let request = app.source_recovery.read_request().unwrap();
    let actual = admit(app, peer, request);
    answer(
        app,
        peer,
        &actual,
        Ok(Reply::PerformanceSourceActionDetail(Box::new(
            detail.clone(),
        ))),
    );
    assert!(!app.busy() && app.active.is_none());
    assert!(fixture::snapshot(&app.source_recovery).pending.is_none());
}

fn requested(request: &SourceRecoveryRequest) -> PerformanceSourceActionReference {
    match &request.operation {
        SourceRecoveryOperation::Read {
            id,
            revision,
            payload_sha256,
        }
        | SourceRecoveryOperation::Execute {
            id,
            revision,
            payload_sha256,
        }
        | SourceRecoveryOperation::Reconcile {
            id,
            revision,
            payload_sha256,
        }
        | SourceRecoveryOperation::Decision {
            id,
            revision,
            payload_sha256,
            ..
        }
        | SourceRecoveryOperation::Attach {
            id,
            revision,
            payload_sha256,
        }
        | SourceRecoveryOperation::ReconcileAttachment {
            id,
            revision,
            payload_sha256,
        } => PerformanceSourceActionReference {
            id: id.clone(),
            revision: *revision,
            payload_sha256: payload_sha256.clone(),
        },
        _ => panic!("exact retained reference"),
    }
}

fn execute_request(app: &mut StudioApp) -> SourceRecoveryRequest {
    let availability = app.source_recovery_availability();
    app.source_recovery.review_execute(&availability).unwrap();
    app.source_recovery.confirm_request().unwrap()
}

fn execution_reply(
    request: &SourceRecoveryRequest,
    detail: &PerformanceSourceActionDetail,
) -> Reply {
    Reply::PerformanceSourceExecution(Box::new(PerformanceSourceExecutionReceipt {
        requested: requested(request),
        state: detail.state.clone(),
        observed: fixture::result(),
        usage: Default::default(),
        replayed: matches!(request.operation, SourceRecoveryOperation::Reconcile { .. }),
        original_reopened: matches!(request.operation, SourceRecoveryOperation::Execute { .. }),
    }))
}

fn pending_detail(app: &StudioApp) -> PerformanceSourceActionDetail {
    let detail = fixture::detail(&app.paths.native, Phase::Verified);
    let target = app.source_recovery_availability().target.unwrap();
    let choice = fixture::choice(&detail, &target, CHOICE, 320);
    fixture::decided(
        &detail,
        &PerformanceSourceDecision::PrepareAttachment {
            choice: Box::new(choice),
        },
    )
}

fn attach_request(app: &mut StudioApp) -> SourceRecoveryRequest {
    app.source_recovery.set_upgrade_consent(true);
    let availability = app.source_recovery_availability();
    app.source_recovery.review_attach(&availability).unwrap();
    app.source_recovery.confirm_request().unwrap()
}

fn publication(detail: &PerformanceSourceActionDetail) -> &omniusong_store::TakePublication {
    &detail
        .state
        .decisions
        .attachment
        .as_ref()
        .unwrap()
        .choice
        .publication
}

fn published_document(app: &StudioApp, detail: &PerformanceSourceActionDetail) -> ProjectDocument {
    let source = app.document.as_ref().unwrap();
    let mut document = if source.schema_version() == 1 {
        source.upgrade_native().unwrap()
    } else {
        ProjectDocument::from_value(source.value().clone()).unwrap()
    };
    let publication = publication(detail);
    document
        .append_take_clip(
            &publication.track_id.0,
            &publication.track_name,
            publication.clip.clone(),
        )
        .unwrap();
    document
        .with_revision(publication.expected_revision + 1)
        .unwrap()
}

fn attachment_reply(
    request: &SourceRecoveryRequest,
    pending: &PerformanceSourceActionDetail,
    current: Option<ProjectDocument>,
) -> Reply {
    let publication = publication(pending).clone();
    let state = fixture::committed(pending, publication.expected_revision + 1).state;
    Reply::PerformanceSourceAttachment(Box::new(PerformanceSourceAttachmentReceipt {
        requested: requested(request),
        state,
        publication,
        committed: true,
        replayed: true,
        current_document: current.map(|document| DocumentView::prepare(document).unwrap()),
        current_document_error: None,
    }))
}

#[test]
fn copied_recovery_raw_mismatched_and_continued_routes_cannot_acquire_the_worker() {
    let (mut app, peer, engine) = manual();
    let detail = fixture::detail(&app.paths.native, Phase::Prepared);
    let request = app.source_recovery.list_request().unwrap();
    let before = preserved(&app);
    let reference = PerformanceSourceActionReference {
        id: detail.state.prepared.action.id.clone(),
        revision: 1,
        payload_sha256: detail.state.prepared.payload_sha256.clone(),
    };
    let raw = [
        Command::ListPreparedPerformanceSourceActions,
        Command::ListPerformanceSourceActionStates,
        Command::ReadPerformanceSourceAction {
            id: reference.id.clone(),
            revision: 1,
            payload_sha256: reference.payload_sha256.clone(),
        },
        Command::ExecutePerformanceSourceAction {
            id: reference.id.clone(),
            revision: 1,
            payload_sha256: reference.payload_sha256.clone(),
        },
        Command::ReconcilePerformanceSourceAction {
            id: reference.id.clone(),
            revision: 1,
            payload_sha256: reference.payload_sha256.clone(),
        },
        Command::SavePreparedPerformanceSourceAction {
            expected_revision: None,
            action: Box::new(detail.state.prepared.action.clone()),
            review: detail.review.clone(),
        },
        Command::SavePerformanceSourceDecision {
            id: reference.id.clone(),
            revision: 3,
            payload_sha256: reference.payload_sha256.clone(),
            decision: Box::new(PerformanceSourceDecision::KeepLocally),
        },
        Command::AttachPerformanceSource {
            id: reference.id.clone(),
            revision: 4,
            payload_sha256: reference.payload_sha256.clone(),
        },
        Command::ReconcilePerformanceSourceAttachment {
            id: reference.id,
            revision: 4,
            payload_sha256: reference.payload_sha256,
        },
    ];
    for command in raw {
        assert!(app.submit(command, Action::Refresh, None).is_err());
    }
    assert!(
        app.submit(
            Command::Refresh,
            Action::SourceRecovery(request.clone()),
            None
        )
        .is_err()
    );
    assert!(
        app.dispatch(
            Action::SourceRecovery(request.clone()),
            Some(Action::Create)
        )
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
        Ok(Reply::PerformanceSourceActionStates(Vec::new())),
    );
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}

#[test]
fn exact_list_and_read_keep_all_three_drafts_failed_choices_and_retained_musical_retry() {
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    held_drafts(&mut app);
    prime(&mut app);
    let detail = fixture::detail(&app.paths.native, Phase::Verified);
    let before = preserved(&app);
    load(&mut app, &peer, &detail);
    assert_eq!(preserved(&app), before);
    assert!(!app.source_recovery_availability().can_mutate);
    assert!(
        !fixture::snapshot(&app.source_recovery).proof,
        "metadata Read is never artifact proof"
    );
    let availability = app.source_recovery_availability();
    assert!(
        app.source_recovery
            .review_decision(PerformanceSourceDecision::KeepLocally, &availability)
            .is_err()
    );
    no_continuation(&peer, &engine);
}

#[test]
fn copied_recovery_rechecks_every_mutation_barrier_and_clean_prepaint_context() {
    for barrier in 0..15 {
        let (mut app, peer, engine) = manual();
        let detail = fixture::detail(&app.paths.native, Phase::Prepared);
        load(&mut app, &peer, &detail);
        let request = execute_request(&mut app);
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
            9 => retained_metadata_retry(&mut app, &peer),
            10 => held_drafts(&mut app),
            11 => app.selected_track += 1,
            12 => audio_ui::test_support::hold_output(&mut app.audio),
            13 => app.evidence_metadata_only = true,
            14 => {
                let paths = app.paths.clone();
                let target = app.source_recovery_availability().target.unwrap();
                let publication = fixture::choice(
                    &fixture::detail(&paths.native, Phase::Verified),
                    &target,
                    CHOICE,
                    0,
                )
                .publication;
                app.recovery.started(&RecoveryAction::Attach {
                    journal_path: paths.native.join("synthetic-unopened.sqlite3"),
                    publication: Box::new(publication),
                });
            }
            _ => unreachable!(),
        }
        let before = preserved(&app);
        assert!(
            app.dispatch(Action::SourceRecovery(request), None).is_err(),
            "barrier {barrier}"
        );
        assert!(!app.busy() && app.active.is_none(), "barrier {barrier}");
        assert_eq!(preserved(&app), before, "barrier {barrier}");
        no_continuation(&peer, &engine);
    }
}

#[test]
fn requested_reference_mismatches_keep_native_active_and_the_exact_pending_credit() {
    let (mut app, peer, engine) = manual();
    let prepared = fixture::detail(&app.paths.native, Phase::Prepared);
    let verified = fixture::detail(&app.paths.native, Phase::Verified);
    load(&mut app, &peer, &prepared);
    let request = execute_request(&mut app);
    let actual = admit(&mut app, &peer, request.clone());
    let before = preserved(&app);
    for mismatch in 0..3 {
        let Reply::PerformanceSourceExecution(mut receipt) = execution_reply(&request, &verified)
        else {
            unreachable!()
        };
        match mismatch {
            0 => receipt.requested.id = OTHER.into(),
            1 => receipt.requested.revision += 1,
            2 => receipt.requested.payload_sha256 = "f".repeat(64),
            _ => unreachable!(),
        }
        // Exercise both independent gates: direct native completion correlation
        // and the real session client's injected pre-credit correlation.
        app.complete(Completion {
            id: actual.id,
            kind: actual.command.kind(),
            result: Ok(Reply::PerformanceSourceExecution(receipt.clone())),
        });
        assert_eq!(app.active.as_ref().unwrap().id, actual.id);
        answer(
            &mut app,
            &peer,
            &actual,
            Ok(Reply::PerformanceSourceExecution(receipt)),
        );
        assert!(app.busy() && app.service.as_ref().unwrap().is_busy());
        assert_eq!(
            fixture::snapshot(&app.source_recovery).pending,
            Some(actual.id)
        );
        assert_eq!(preserved(&app), before);
    }
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(execution_reply(&request, &verified)),
    );
    assert!(!app.busy() && app.active.is_none());
    assert!(fixture::snapshot(&app.source_recovery).proof);
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}

#[test]
fn active_native_ownership_still_blocks_overwrite_after_service_credit_was_consumed() {
    let (mut app, peer, engine) = manual();
    let prepared = fixture::detail(&app.paths.native, Phase::Prepared);
    let verified = fixture::detail(&app.paths.native, Phase::Verified);
    load(&mut app, &peer, &prepared);
    let request = execute_request(&mut app);
    let actual = admit(&mut app, &peer, request.clone());
    peer.responses
        .send(Completion {
            id: actual.id,
            kind: actual.command.kind(),
            result: Ok(execution_reply(&request, &verified)),
        })
        .unwrap();
    let completion = app.service.as_mut().unwrap().try_poll().unwrap().unwrap();
    assert!(!app.service.as_ref().unwrap().is_busy());
    assert!(app.busy(), "the still-owned native Active is part of busy");
    assert!(app.submit(Command::Refresh, Action::Refresh, None).is_err());
    assert_eq!(app.active.as_ref().unwrap().id, actual.id);
    no_continuation(&peer, &engine);
    app.complete(completion);
    assert!(!app.busy() && app.active.is_none());
    no_continuation(&peer, &engine);
}

#[test]
fn started_reconciliation_observes_without_recording_verified_or_chaining_execute() {
    let (mut app, peer, engine) = manual();
    let started = fixture::detail(&app.paths.native, Phase::Started);
    load(&mut app, &peer, &started);
    let availability = app.source_recovery_availability();
    assert!(app.source_recovery.review_execute(&availability).is_err());
    let request = app.source_recovery.reconcile_request().unwrap();
    let before = preserved(&app);
    let actual = admit(&mut app, &peer, request.clone());
    let reply = execution_reply(&request, &started);
    let Reply::PerformanceSourceExecution(receipt) = &reply else {
        unreachable!()
    };
    assert!(matches!(
        receipt.state.execution.as_ref().unwrap().phase,
        PerformanceSourceExecutionPhase::Started
    ));
    assert!(!receipt.original_reopened);
    answer(&mut app, &peer, &actual, Ok(reply));
    let observed = fixture::snapshot(&app.source_recovery);
    assert!(observed.proof && observed.pending.is_none() && !observed.uncertain);
    assert!(
        observed
            .cards
            .last()
            .unwrap()
            .0
            .contains("recorded Started")
    );
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
    let request = execute_request(&mut app);
    let actual = admit(&mut app, &peer, request.clone());
    let Reply::PerformanceSourceExecution(mut receipt) = execution_reply(
        &request,
        &fixture::detail(&app.paths.native, Phase::Verified),
    ) else {
        unreachable!()
    };
    receipt.replayed = true;
    receipt.original_reopened = false;
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::PerformanceSourceExecution(receipt)),
    );
    assert!(!app.busy() && app.active.is_none());
    assert_eq!(preserved(&app), before);
    no_continuation(&peer, &engine);
}

#[test]
fn matching_reference_bad_result_decision_or_publication_is_terminal_protocol_uncertainty() {
    for case in 0..3 {
        let (mut app, peer, engine) = manual();
        let detail = match case {
            0 => fixture::detail(&app.paths.native, Phase::Prepared),
            1 => fixture::detail(&app.paths.native, Phase::Verified),
            2 => pending_detail(&app),
            _ => unreachable!(),
        };
        load(&mut app, &peer, &detail);
        let request = match case {
            0 => execute_request(&mut app),
            1 => {
                let availability = app.source_recovery_availability();
                app.source_recovery
                    .review_decision(PerformanceSourceDecision::KeepLocally, &availability)
                    .unwrap();
                app.source_recovery.confirm_request().unwrap()
            }
            2 => attach_request(&mut app),
            _ => unreachable!(),
        };
        let before = preserved(&app);
        let bad = match case {
            0 => {
                let Reply::PerformanceSourceExecution(mut receipt) = execution_reply(
                    &request,
                    &fixture::detail(&app.paths.native, Phase::Verified),
                ) else {
                    unreachable!()
                };
                receipt.observed.original_rows_sha256 = "0".repeat(64);
                Reply::PerformanceSourceExecution(receipt)
            }
            1 => Reply::PerformanceSourceDecision(Box::new(PerformanceSourceDecisionReceipt {
                requested: requested(&request),
                state: fixture::decided(&detail, &PerformanceSourceDecision::ReturnToReview).state,
            })),
            2 => {
                let current = published_document(&app, &detail);
                let Reply::PerformanceSourceAttachment(mut receipt) =
                    attachment_reply(&request, &detail, Some(current))
                else {
                    unreachable!()
                };
                receipt.publication.clip.position.frame += 1;
                Reply::PerformanceSourceAttachment(receipt)
            }
            _ => unreachable!(),
        };
        let bad = Ok(bad);
        assert!(source_recovery_integration::reference_matches(
            &request, &bad
        ));
        let actual = admit(&mut app, &peer, request.clone());
        answer(&mut app, &peer, &actual, bad);
        assert!(
            !app.busy() && app.active.is_none(),
            "case {case} consumed its correlated terminal"
        );
        assert!(app.service.as_ref().unwrap().pending().is_none());
        let observed = fixture::snapshot(&app.source_recovery);
        assert!(
            observed.pending.is_none() && observed.uncertain && !observed.proof,
            "case {case}"
        );
        assert!(observed.cards.last().unwrap().0.contains("not confirmed"));
        assert!(
            app.error
                .as_ref()
                .unwrap()
                .contains("correlated recovery response")
        );
        assert_eq!(preserved(&app), before);
        let availability = app.source_recovery_availability();
        assert!(app.source_recovery.review_execute(&availability).is_err());
        assert!(app.retry.is_none());
        no_continuation(&peer, &engine);
        // Explicit metadata remains available after terminal consumption while
        // the exact uncertain choice continues to block a new mutation.
        let request = app.source_recovery.list_request().unwrap();
        let next = admit(&mut app, &peer, request);
        answer(
            &mut app,
            &peer,
            &next,
            Ok(Reply::PerformanceSourceActionStates(vec![detail.state])),
        );
        assert!(!app.busy() && app.active.is_none());
        assert!(fixture::snapshot(&app.source_recovery).uncertain);
        assert_eq!(preserved(&app), before);
        no_continuation(&peer, &engine);
    }
}

#[test]
fn healthy_exact_attach_installs_current_document_only_for_unchanged_target_context() {
    for late_draft in [false, true] {
        let (mut app, peer, engine) = manual();
        let detail = pending_detail(&app);
        load(&mut app, &peer, &detail);
        let request = attach_request(&mut app);
        let document = published_document(&app, &detail);
        let expected = document.to_json().unwrap();
        let actual = admit(&mut app, &peer, request.clone());
        if late_draft {
            app.draft.name.push_str(" typed while Attach was pending");
        }
        let before = preserved(&app);
        answer(
            &mut app,
            &peer,
            &actual,
            Ok(attachment_reply(&request, &detail, Some(document))),
        );
        assert!(!app.busy() && app.active.is_none());
        assert!(
            fixture::snapshot(&app.source_recovery)
                .cards
                .iter()
                .any(|(_, committed, _)| *committed == Some(true))
        );
        if late_draft {
            assert_eq!(
                preserved(&app),
                before,
                "late draft and original target view stay intact"
            );
        } else {
            assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), expected);
            assert!(!app.dirty() && !app.musical_unresolved());
            assert_eq!(
                app.draft,
                Draft::from_document(app.document.as_ref().unwrap())
            );
        }
        no_continuation(&peer, &engine);
    }
}

#[test]
fn rejected_correlated_attachment_bodies_cannot_install_a_document_through_the_coordinator() {
    for changed_prepared in [false, true] {
        let (mut app, peer, engine) = manual();
        let pending = pending_detail(&app);
        load(&mut app, &peer, &pending);
        let request = attach_request(&mut app);
        let current = published_document(&app, &pending);
        let before = preserved(&app);
        let Reply::PerformanceSourceAttachment(mut receipt) =
            attachment_reply(&request, &pending, Some(current))
        else {
            unreachable!()
        };
        if changed_prepared {
            // A coherent different Prepared chain has the same action ID,
            // original review and publication. It must not replace the exact
            // immutable Prepared receipt pinned when this request was admitted.
            let mut different = fixture::detail(&app.paths.native, Phase::Verified);
            different.state.prepared.action.recovery_operation_id =
                OperationId("synthetic-different-copy-operation".into());
            different.state.prepared.payload_sha256 =
                fixture::hash(&serde_json::to_vec(&different.state.prepared.action).unwrap());
            different
                .state
                .execution
                .as_mut()
                .unwrap()
                .prepared_payload_sha256 = different.state.prepared.payload_sha256.clone();
            different.state.payload_sha256 = fixture::hash(
                &serde_json::to_vec(different.state.execution.as_ref().unwrap()).unwrap(),
            );
            different.validate().unwrap();
            let different = fixture::decided(
                &different,
                &PerformanceSourceDecision::PrepareAttachment {
                    choice: pending
                        .state
                        .decisions
                        .attachment
                        .as_ref()
                        .unwrap()
                        .choice
                        .clone(),
                },
            );
            receipt.state =
                fixture::committed(&different, receipt.publication.expected_revision + 1).state;
        } else {
            // The receipt claims commitment, but its exact current choice is
            // still Pending. A valid-looking separate document cannot repair it.
            receipt.state = pending.state.clone();
        }
        let result = Ok(Reply::PerformanceSourceAttachment(receipt));
        assert!(source_recovery_integration::reference_matches(
            &request, &result
        ));
        let actual = admit(&mut app, &peer, request);
        answer(&mut app, &peer, &actual, result);
        assert!(!app.busy() && app.active.is_none());
        assert!(app.service.as_ref().unwrap().pending().is_none());
        assert_eq!(
            preserved(&app),
            before,
            "changed Prepared: {changed_prepared}"
        );
        let observed = fixture::snapshot(&app.source_recovery);
        assert!(observed.uncertain && !observed.proof && observed.pending.is_none());
        assert!(
            observed
                .cards
                .iter()
                .any(|(title, committed, _)| title.contains("not installed") && committed.is_none())
                || observed
                    .cards
                    .iter()
                    .any(|(title, committed, _)| title.contains("not confirmed")
                        && committed.is_none())
        );
        assert!(app.retry.is_none());
        no_continuation(&peer, &engine);
    }
}

#[test]
fn attachment_reconciliation_never_requires_current_target_or_installs_its_current_document() {
    let (mut app, peer, engine) = manual();
    let verified = fixture::detail(&app.paths.native, Phase::Verified);
    let unrelated = ProjectDocument::new("Historical attachment belongs elsewhere").unwrap();
    let mut target = app.source_recovery_availability().target.unwrap();
    target.project_id = ProjectId(unrelated.id().into());
    let choice = fixture::choice(&verified, &target, CHOICE, 320);
    let detail = fixture::decided(
        &verified,
        &PerformanceSourceDecision::PrepareAttachment {
            choice: Box::new(choice),
        },
    );
    let mut current = unrelated.upgrade_native().unwrap();
    let publication = publication(&detail);
    current
        .append_take_clip(
            &publication.track_id.0,
            &publication.track_name,
            publication.clip.clone(),
        )
        .unwrap();
    let current = current
        .with_revision(publication.expected_revision + 1)
        .unwrap();
    load(&mut app, &peer, &detail);
    held_drafts(&mut app);
    app.draft.name.push_str(" held metadata");
    prime(&mut app);
    // Re-read after the explicit context change; this remains metadata only.
    load(&mut app, &peer, &detail);
    let request = app.source_recovery.reconcile_attachment_request().unwrap();
    let before = preserved(&app);
    let actual = admit(&mut app, &peer, request.clone());
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(attachment_reply(&request, &detail, Some(current))),
    );
    assert_eq!(preserved(&app), before);
    assert!(!app.busy() && app.active.is_none());
    assert!(
        fixture::snapshot(&app.source_recovery)
            .cards
            .iter()
            .any(|(_, committed, _)| *committed == Some(true))
    );
    no_continuation(&peer, &engine);
}

#[test]
fn long_nested_fatal_receipt_keeps_primary_and_every_new_draft_and_musical_retry_before_retirement()
{
    let (mut app, peer, engine) = manual();
    retained_metadata_retry(&mut app, &peer);
    let retained = app.retry.take().unwrap();
    let held_metadata = app.draft.clone();
    app.draft = Draft::from_document(app.document.as_ref().unwrap());
    prime(&mut app);
    let detail = pending_detail(&app);
    load(&mut app, &peer, &detail);
    let request = attach_request(&mut app);
    let actual = admit(&mut app, &peer, request.clone());
    app.retry = Some(retained);
    app.draft = held_metadata;
    held_drafts(&mut app);
    let before = preserved(&app);
    let Reply::PerformanceSourceAttachment(mut receipt) = attachment_reply(&request, &detail, None)
    else {
        unreachable!()
    };
    receipt.current_document_error = Some(ServiceError::storage_unavailable(
        "Injected nested catalog diagnostic ".repeat(2048),
        false,
    ));
    // This is a valid historical publication with an injected nested diagnostic,
    // not a physical SQLite fault or an actual recovery/publication operation.
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::PerformanceSourceAttachment(receipt)),
    );
    assert!(app.disconnected() && !app.ready && app.reconnect_existing);
    assert!(!app.busy() && app.active.is_none());
    assert_eq!(preserved(&app), before);
    let snapshot = fixture::snapshot(&app.source_recovery);
    assert!(snapshot.cards.iter().any(|(_, committed, nested)| {
        *committed == Some(true)
            && nested
                .as_ref()
                .is_some_and(|text| !text.is_empty() && text.len() <= 2048)
    }));
    assert!(!snapshot.proof);
    no_continuation(&peer, &engine);
}

#[test]
fn active_source_mutation_eof_pins_uncertainty_and_preserves_new_drafts_without_retry() {
    let (mut app, peer, engine) = manual();
    let detail = fixture::detail(&app.paths.native, Phase::Prepared);
    load(&mut app, &peer, &detail);
    let request = execute_request(&mut app);
    let actual = admit(&mut app, &peer, request.clone());
    app.draft.name.push_str(" new metadata after admission");
    held_drafts(&mut app);
    let before = preserved(&app);
    let TestPeer {
        requests,
        responses,
    } = peer;
    drop(responses);
    app.poll();
    assert!(app.disconnected() && app.active.is_none() && !app.busy());
    assert!(
        app.retry.is_none(),
        "an uncertain recovery is never a musical retry"
    );
    assert_eq!(preserved(&app), before);
    let snapshot = fixture::snapshot(&app.source_recovery);
    assert!(snapshot.uncertain && snapshot.pending.is_none() && !snapshot.proof);
    assert!(
        snapshot
            .cards
            .iter()
            .any(|(title, _, _)| title.contains("uncertain"))
    );
    assert!(requests.try_recv().is_err());
    assert!(engine.command().is_none() && engine.urgent().is_none());
    assert!(
        !app.source_recovery
            .authorizes(&request, actual.id.generation)
    );
}

fn source_offer(app: &mut StudioApp, peer: &TestPeer) {
    use performance_source_ui::{SourceAction, SourceRequest};
    app.handle_performance_source_action(SourceAction::Submit(SourceRequest::Scan));
    let actual = peer.requests.try_recv().unwrap();
    let sources = [fixture::STAGE, OTHER]
        .into_iter()
        .map(|stage_id| omniusong_session::PerformanceSource {
            stage_id: stage_id.into(),
            journal_path: app
                .paths
                .native
                .join("captures")
                .join(stage_id)
                .join("take.sqlite3"),
            problem: None,
        })
        .collect();
    answer(
        app,
        peer,
        &actual,
        Ok(Reply::PerformanceSources(PerformanceSourceScan {
            sources,
            examined_entries: 2,
            skipped_generic: 0,
            skipped_unknown: 0,
            truncated: false,
        })),
    );
    assert!(app.performance_sources.select(fixture::STAGE));
    let request = app.performance_sources.inspection_request().unwrap();
    app.handle_performance_source_action(SourceAction::Submit(request));
    let actual = peer.requests.try_recv().unwrap();
    let review: PerformanceSourceReview =
        serde_json::from_slice(&fixture::review(&app.paths.native)).unwrap();
    answer(
        app,
        peer,
        &actual,
        Ok(Reply::PerformanceSourceInspected(Box::new(review))),
    );
    prime(app);
}

#[test]
fn source_selection_round_trip_while_recovery_window_is_closed_invalidates_confirmation() {
    let (mut app, peer, engine) = manual();
    source_offer(&mut app, &peer);
    let detail = fixture::detail(&app.paths.native, Phase::Prepared);
    load(&mut app, &peer, &detail);
    let request = execute_request(&mut app);
    assert!(!app.source_recovery.open);
    assert!(app.performance_sources.select(OTHER));
    app.sync_source_recovery_context();
    assert!(app.performance_sources.select(fixture::STAGE));
    app.sync_source_recovery_context();
    assert!(app.source_recovery.confirm_request().is_none());
    assert!(
        !app.source_recovery
            .authorizes(&request, app.service.as_ref().unwrap().generation())
    );
    assert!(app.dispatch(Action::SourceRecovery(request), None).is_err());
    no_continuation(&peer, &engine);
}

#[test]
fn passive_empty_intent_list_and_explicit_prepare_do_not_claim_start_or_chain_execution() {
    let (mut app, peer, engine) = manual();
    app.dispatch(
        Action::Performance(Box::new(Command::ListPerformanceIntents)),
        None,
    )
    .unwrap();
    let actual = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::PerformanceIntents(Vec::new())),
    );
    let status = app.status.to_lowercase();
    for invented in ["intent saved", "request saved", "was saved"] {
        assert!(!status.contains(invented), "{status}");
    }
    no_continuation(&peer, &engine);
    source_offer(&mut app, &peer);
    let list = app.source_recovery.list_request().unwrap();
    let actual = admit(&mut app, &peer, list);
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::PerformanceSourceActionStates(Vec::new())),
    );
    let availability = app.source_recovery_availability();
    app.source_recovery.prepare_review(&availability).unwrap();
    let request = app.source_recovery.confirm_request().unwrap();
    let SourceRecoveryOperation::Prepare { action, review } = &request.operation else {
        panic!("Prepare")
    };
    assert!(action.origin.is_none());
    assert_eq!(action.review_blob_sha256, fixture::hash(review));
    let receipt = omniusong_session::PreparedPerformanceSourceActionReceipt {
        revision: 1,
        action: *action.clone(),
        payload_sha256: fixture::hash(&serde_json::to_vec(action).unwrap()),
    };
    let actual = admit(&mut app, &peer, request);
    answer(
        &mut app,
        &peer,
        &actual,
        Ok(Reply::PreparedPerformanceSourceAction(Box::new(receipt))),
    );
    assert!(!app.busy() && app.active.is_none());
    assert!(!fixture::snapshot(&app.source_recovery).proof);
    assert!(
        fixture::snapshot(&app.source_recovery)
            .cards
            .iter()
            .any(|(title, _, _)| title.contains("has not been requested"))
    );
    no_continuation(&peer, &engine);
}

struct HeldCancellation {
    captured: mpsc::SyncSender<[Arc<AtomicBool>; 3]>,
    release: mpsc::Receiver<()>,
}
impl Backend for HeldCancellation {
    fn handle(&mut self, _: &Command) -> Result<Reply, ServiceError> {
        Err(ServiceError::rejected("synthetic held worker only"))
    }
    fn handle_with_evidence_cancellation(
        &mut self,
        command: &Command,
        inspection: &Arc<AtomicBool>,
        source: &Arc<AtomicBool>,
        evidence: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        assert!(matches!(
            command,
            Command::ExecutePerformanceSourceAction { .. }
        ));
        self.captured
            .send([
                Arc::clone(inspection),
                Arc::clone(source),
                Arc::clone(evidence),
            ])
            .unwrap();
        self.release.recv_timeout(Duration::from_secs(5)).unwrap();
        Err(ServiceError {
            kind: FailureKind::InspectionCancelled,
            message: "Synthetic worker observed source-action cancellation".into(),
            outcome_unknown: true,
        })
    }
}

fn seed_ui_only(app: &mut StudioApp, detail: &PerformanceSourceActionDetail) {
    let generation = app.service.as_ref().unwrap().generation();
    let request = app.source_recovery.list_request().unwrap();
    let id = RequestId {
        generation,
        sequence: 100,
    };
    app.source_recovery.submitted(&request, id);
    app.source_recovery.completed(
        &request,
        id,
        Ok(&Reply::PerformanceSourceActionStates(vec![
            detail.state.clone(),
        ])),
    );
    assert!(
        app.source_recovery
            .select_action(&detail.state.prepared.action.id)
    );
    let request = app.source_recovery.read_request().unwrap();
    let id = RequestId {
        generation,
        sequence: 101,
    };
    app.source_recovery.submitted(&request, id);
    app.source_recovery.completed(
        &request,
        id,
        Ok(&Reply::PerformanceSourceActionDetail(Box::new(
            detail.clone(),
        ))),
    );
}

#[test]
fn exact_recovery_cancel_keeps_credit_and_sets_only_source_action_token_without_audio() {
    let (mut app, old_peer, engine) = manual();
    let (captured_tx, captured_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::channel();
    app.service = Some(
        SessionService::spawn_with_backend(HeldCancellation {
            captured: captured_tx,
            release: release_rx,
        })
        .unwrap(),
    );
    app.source_recovery = SourceRecoveryUi::default();
    app.source_recovery_context = None;
    prime(&mut app);
    let detail = fixture::detail(&app.paths.native, Phase::Prepared);
    // UI-only fixture installation; the operation under test goes through the
    // actual session worker and captures all three independent cancellation arcs.
    seed_ui_only(&mut app, &detail);
    let request = execute_request(&mut app);
    app.handle_source_recovery_action(SourceRecoveryAction::Submit(request));
    let id = app.active.as_ref().unwrap().id;
    let tokens = captured_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(tokens.iter().all(|token| !token.load(Ordering::Acquire)));
    app.handle_source_recovery_action(SourceRecoveryAction::Cancel(RequestId {
        generation: id.generation,
        sequence: id.sequence + 1,
    }));
    assert!(tokens.iter().all(|token| !token.load(Ordering::Acquire)));
    app.handle_source_recovery_action(SourceRecoveryAction::Cancel(id));
    assert!(!tokens[0].load(Ordering::Acquire));
    assert!(tokens[1].load(Ordering::Acquire));
    assert!(!tokens[2].load(Ordering::Acquire));
    assert!(app.busy() && app.service.as_ref().unwrap().is_busy());
    assert_eq!(app.source_recovery.pending_cancel(), Some(id));
    assert_eq!(app.service.as_ref().unwrap().pending().unwrap().id, id);
    assert!(engine.command().is_none() && engine.urgent().is_none());
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.active.is_some() {
        app.poll();
        assert!(
            Instant::now() < deadline,
            "synthetic held cancellation completion"
        );
        std::thread::yield_now();
    }
    assert!(!app.busy());
    assert!(!tokens[0].load(Ordering::Acquire) && !tokens[2].load(Ordering::Acquire));
    assert!(fixture::snapshot(&app.source_recovery).uncertain);
    no_continuation(&old_peer, &engine);
}

#[test]
fn historical_source_offer_cannot_dispatch_approved_prepare_before_repainting_closed_window() {
    use performance_source_ui::{SourceAction, SourceRequest};
    for admitted in [false, true] {
        let (mut app, peer, engine) = manual();
        source_offer(&mut app, &peer);
        let list = app.source_recovery.list_request().unwrap();
        let actual = admit(&mut app, &peer, list);
        answer(
            &mut app,
            &peer,
            &actual,
            Ok(Reply::PerformanceSourceActionStates(Vec::new())),
        );
        let availability = app.source_recovery_availability();
        app.source_recovery.prepare_review(&availability).unwrap();
        let request = app.source_recovery.confirm_request().unwrap();
        assert!(!app.source_recovery.open);
        let inspect = app.performance_sources.inspection_request().unwrap();
        assert!(matches!(inspect, SourceRequest::Inspect { .. }));
        if admitted {
            app.handle_performance_source_action(SourceAction::Submit(inspect));
            let actual = peer.requests.try_recv().unwrap();
            answer(
                &mut app,
                &peer,
                &actual,
                Err(ServiceError::rejected(
                    "Injected source-local inspection failure",
                )),
            );
        } else {
            app.performance_sources
                .submission_failed(&inspect, "Injected refused inspection admission");
        }
        assert!(app.performance_sources.recovery_offer().is_none());
        assert!(!app.busy());
        assert!(app.dispatch(Action::SourceRecovery(request), None).is_err());
        assert!(app.active.is_none() && !app.service.as_ref().unwrap().is_busy());
        no_continuation(&peer, &engine);
    }
}
