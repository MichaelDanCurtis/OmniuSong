use super::*;
use omniusong_session::{EvidenceJobSummary, EvidenceProblem, EvidenceResourceOrigin};
use omniusong_store::{EvidenceEntry, EvidenceManifest, EvidenceRestoreReceipt, EvidenceRoot};

const ORIGIN: &str = "/synthetic/studio";
const ARCHIVE: &str = "/synthetic/source.osre";
const DESTINATION: &str = "/synthetic/export.osre";
const RESTORE: &str = "/synthetic/review";
const JOB: &str = "11111111111111111111111111111111";
const EXPORT: &str = "22222222222222222222222222222222";

fn availability() -> EvidenceAvailability {
    EvidenceAvailability {
        can_submit: true,
        can_read_artifacts: true,
        can_run: true,
        generation: Some(1),
        origin: Some(ORIGIN.into()),
        unavailable_reason: None,
    }
}
fn model() -> EvidenceUi {
    let mut model = EvidenceUi::default();
    model.sync_context(&availability());
    model.set_export_path(DESTINATION.into());
    model.set_archive_path(ARCHIVE.into());
    model.set_restore_directory(RESTORE.into());
    model
}
fn id(request: &EvidenceUiRequest) -> RequestId {
    RequestId {
        generation: request.context().1,
        sequence: request.context().2,
    }
}
fn answer(model: &mut EvidenceUi, request: &EvidenceUiRequest, reply: &Reply) {
    assert!(model.authorizes(request, request.context().1));
    model.submitted(request, id(request));
    assert!(model.pending.is_some());
    model.completed(request, id(request), Ok(reply));
    assert!(model.pending.is_none());
}
fn archive_inspection() -> EvidenceArchiveInspection {
    let manifest = EvidenceManifest {
        version: 1,
        export_id: EXPORT.into(),
        scope: EVIDENCE_SCOPE.into(),
        origin_root: "/synthetic/previous-studio".into(),
        catalog_schema: omniusong_store::STORE_SCHEMA_VERSION,
        roots: ["captures", "source-actions", "takes"]
            .into_iter()
            .map(|path| EvidenceRoot {
                path: path.into(),
                present: false,
            })
            .collect(),
        absences: Vec::new(),
        entries: vec![EvidenceEntry {
            path: "catalog.sqlite3".into(),
            kind: EvidenceEntryKind::File,
            byte_length: 100,
            sha256: Some("b".repeat(64)),
        }],
    };
    let receipt = EvidenceArchiveReceipt {
        path: ARCHIVE.into(),
        export_id: EXPORT.into(),
        sha256: "a".repeat(64),
        byte_length: 56 + serde_json::to_vec(&manifest).unwrap().len() as u64 + 100,
        file_count: 1,
        directory_count: 0,
        payload_bytes: 100,
    };
    EvidenceArchiveInspection { receipt, manifest }
}
fn inspect(model: &mut EvidenceUi) {
    let request = model.inspect_request().unwrap();
    answer(
        model,
        &request,
        &Reply::RetainedEvidenceInspected(Box::new(archive_inspection())),
    );
    assert!(model.inspection_current().is_some());
}
fn export_request() -> EvidenceJobRequest {
    EvidenceJobRequest {
        version: 1,
        job_id: JOB.into(),
        origin_root: ORIGIN.into(),
        operation: EvidenceOperation::Export {
            scope: EVIDENCE_SCOPE.into(),
            export_id: EXPORT.into(),
            destination: DESTINATION.into(),
        },
    }
}
fn view() -> EvidenceJobView {
    let request = export_request();
    EvidenceJobView {
        job_id: JOB.into(),
        directory: format!("/synthetic/journal/{JOB}").into(),
        request_sha256: Some(request.fingerprint().unwrap()),
        request: Some(request),
        history_sha256: "d".repeat(64),
        started: false,
        terminal: None,
        observations: Vec::new(),
        occupied_observations: 0,
        metadata_problems: Vec::new(),
    }
}
fn list(count: usize) -> EvidenceJobList {
    EvidenceJobList {
        jobs: (1..=count)
            .map(|n| EvidenceJobSummary {
                job_id: format!("{n:032x}"),
                request_sha256: Some("b".repeat(64)),
                history_sha256: "d".repeat(64),
                phase: EvidenceJobPhase::Unresolved,
                latest_observation: None,
                metadata_problem_count: 0,
            })
            .collect(),
    }
}
fn install_view(model: &mut EvidenceUi, view: EvidenceJobView) {
    let mut jobs = list(0);
    jobs.jobs.push(EvidenceJobSummary {
        job_id: view.job_id.clone(),
        request_sha256: view.request_sha256.clone(),
        history_sha256: view.history_sha256.clone(),
        phase: EvidenceJobPhase::Unresolved,
        latest_observation: None,
        metadata_problem_count: 0,
    });
    let request = model.list_request().unwrap();
    answer(model, &request, &Reply::EvidenceJobs(jobs));
    assert!(model.select_job(&view.job_id));
    let request = model.read_request().unwrap();
    answer(model, &request, &Reply::EvidenceJob(Box::new(view)));
    assert!(model.current_view().is_some());
}
fn successful_outcome(request: &EvidenceJobRequest) -> EvidenceOutcome {
    let mut receipt = archive_inspection().receipt;
    match &request.operation {
        EvidenceOperation::Export {
            export_id,
            destination,
            ..
        } => {
            receipt.export_id = export_id.clone();
            receipt.path = destination.clone();
            EvidenceOutcome::Exported { receipt }
        }
        EvidenceOperation::Restore {
            archive_path,
            directory,
            expected_archive_sha256,
        } => {
            receipt.path = archive_path.clone();
            receipt.sha256 = expected_archive_sha256.clone();
            EvidenceOutcome::Restored {
                receipt: EvidenceRestoreReceipt {
                    directory: directory.clone(),
                    archive: receipt,
                },
            }
        }
    }
}
fn job_reply(request: &EvidenceJobRequest, outcome: Option<EvidenceOutcome>) -> Reply {
    Reply::EvidenceJobOutcome(Box::new(EvidenceJobReply {
        job_id: request.job_id.clone(),
        request_sha256: request.fingerprint().unwrap(),
        history_sha256: Some("e".repeat(64)),
        attempted: true,
        outcome,
        metadata_problem: None,
        catalog_retirement: None,
    }))
}
fn frame(
    context: &egui::Context,
    model: &mut EvidenceUi,
    availability: &EvidenceAvailability,
    width: f32,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<EvidenceAction>, f32) {
    let mut action = None;
    let mut content_width = 0.0;
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, 420.0),
        )),
        events,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(1.0);
    let mut output = context.run_ui(input, |ui| {
        action = model.ui(ui, availability);
        content_width = ui.min_rect().width();
    });
    output.textures_delta.clear();
    (output, action, content_width)
}
fn click(context: &egui::Context, model: &mut EvidenceUi, label: &str) -> Option<EvidenceAction> {
    let _ = frame(context, model, &availability(), 350.0, Vec::new());
    let tree = frame(context, model, &availability(), 350.0, Vec::new())
        .0
        .platform_output
        .accesskit_update
        .unwrap();
    let target = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label))
        .unwrap_or_else(|| panic!("missing explicit action {label}"))
        .0;
    frame(
        context,
        model,
        &availability(),
        350.0,
        vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: tree.tree_id,
                target_node: target,
                data: None,
            },
        )],
    )
    .1
}

#[test]
fn construction_paint_edits_selection_and_generation_never_emit_commands() {
    let context = egui::Context::default();
    let mut model = model();
    for page in [Page::Export, Page::Restore, Page::History] {
        model.page = page;
        assert!(
            frame(&context, &mut model, &availability(), 350.0, Vec::new())
                .1
                .is_none()
        );
        assert!(model.offered.is_none() && model.confirmation.is_none() && model.pending.is_none());
    }
    model.set_archive_path("/synthetic/changed.osre".into());
    model.set_archive_path(ARCHIVE.into());
    assert!(!model.select_job(JOB));
    model.context_changed();
    model.generation_changed(2);
    let mut available = availability();
    available.generation = Some(2);
    assert!(
        frame(&context, &mut model, &available, 350.0, Vec::new())
            .1
            .is_none()
    );
    assert!(model.offered.is_none() && model.confirmation.is_none());
    let request = model.list_request().unwrap();
    answer(&mut model, &request, &Reply::EvidenceJobs(list(128)));
    assert!(model.select_job("00000000000000000000000000000001"));
    assert!(
        frame(&context, &mut model, &available, 350.0, Vec::new())
            .1
            .is_none()
    );
    assert!(model.offered.is_none());
}

#[test]
fn export_review_mints_once_confirmation_is_exact_and_acceptance_consumes_it() {
    let mut model = model();
    model.review_export(&availability()).unwrap();
    let fingerprint = model.confirmation.as_ref().unwrap().fingerprint.clone();
    let job = model.confirmation.as_ref().unwrap().request.job_id.clone();
    assert!(model.run_request().is_none());
    model.review_export(&availability()).unwrap();
    assert_eq!(model.confirmation.as_ref().unwrap().request.job_id, job);
    let request = model.confirm_request().unwrap();
    assert_eq!(
        model.confirmation.as_ref().unwrap().fingerprint,
        fingerprint
    );
    assert!(model.authorizes(&request, 1));
    assert!(!model.authorizes(&request, 2));
    let mut changed = request.clone();
    if let EvidenceUiRequest::Run { request, .. } = &mut changed {
        request.origin_root.push_str("-different");
    }
    assert!(!model.authorizes(&changed, 1));
    let mut changed = request.clone();
    if let EvidenceUiRequest::Run {
        confirmation_serial,
        ..
    } = &mut changed
    {
        *confirmation_serial += 1;
    }
    assert!(!model.authorizes(&changed, 1));
    model.submission_failed(&request, "Capacity is busy");
    assert!(model.run_request().is_none());
    let retried_confirmation = model.confirm_request().unwrap();
    assert!(retried_confirmation.same(&request));
    model.submitted(&request, id(&request));
    assert!(model.confirmation.is_none() && model.run_request().is_none());
    assert!(!model.authorizes(&request, 1));
}

#[test]
fn restore_binds_review_path_hash_serial_epoch_and_generation() {
    let mut model = model();
    assert!(model.review_restore(&availability()).is_err());
    inspect(&mut model);
    model.review_restore(&availability()).unwrap();
    let request = model.confirm_request().unwrap();
    let EvidenceUiRequest::Run { request: bound, .. } = &request else {
        panic!()
    };
    assert!(
        matches!(&bound.operation, EvidenceOperation::Restore { archive_path, directory, expected_archive_sha256 } if archive_path.as_os_str() == Path::new(ARCHIVE).as_os_str() && directory.as_os_str() == Path::new(RESTORE).as_os_str() && expected_archive_sha256 == &"a".repeat(64))
    );
    let mut changed = request.clone();
    if let EvidenceUiRequest::Run { request, .. } = &mut changed
        && let EvidenceOperation::Restore {
            expected_archive_sha256,
            ..
        } = &mut request.operation
    {
        *expected_archive_sha256 = "b".repeat(64);
    }
    assert!(!model.authorizes(&changed, 1));
    model.inspection.as_mut().unwrap().serial += 1;
    assert!(
        !model.authorizes(&request, 1),
        "a newer observation cannot renew an earlier confirmation"
    );
    model.review_restore(&availability()).unwrap();
    let request = model.confirm_request().unwrap();
    model.set_restore_directory("/synthetic/other".into());
    model.set_restore_directory(RESTORE.into());
    assert!(!model.authorizes(&request, 1));
    assert!(model.confirm_request().is_none());
    model.review_restore(&availability()).unwrap();
    let request = model.confirm_request().unwrap();
    model.set_archive_path("/synthetic/other.osre".into());
    model.set_archive_path(ARCHIVE.into());
    assert!(!model.authorizes(&request, 1));
    assert!(model.review_restore(&availability()).is_err());
    inspect(&mut model);
    model.review_restore(&availability()).unwrap();
    let request = model.confirm_request().unwrap();
    model.generation_changed(2);
    assert!(!model.authorizes(&request, 2));
    assert!(model.inspection_current().is_none());
}

#[test]
fn accepted_inspect_and_read_retire_old_proofs_but_refusal_does_not() {
    let mut model = model();
    inspect(&mut model);
    model.review_restore(&availability()).unwrap();
    let request = model.inspect_request().unwrap();
    assert!(model.inspection_current().is_some() && model.confirmation.is_some());
    model.submission_failed(&request, "busy");
    assert!(model.inspection_current().is_some() && model.confirmation.is_some());
    let request = model.inspect_request().unwrap();
    model.submitted(&request, id(&request));
    assert!(model.inspection_current().is_none() && model.confirmation.is_none());
    model.completed(
        &request,
        id(&request),
        Err(&ServiceError::backend("inspection cancelled", false)),
    );
    assert!(model.inspection.is_some() && model.inspection_current().is_none());
    assert!(model.review_restore(&availability()).is_err());
    install_view(&mut model, view());
    let request = model.read_request().unwrap();
    model.submission_failed(&request, "busy");
    assert!(model.current_view().is_some());
    let request = model.read_request().unwrap();
    model.submitted(&request, id(&request));
    assert!(model.current_view().is_none());
    model.completed(
        &request,
        id(&request),
        Err(&ServiceError::backend("read failed", false)),
    );
    assert!(model.selected_view.is_some() && model.current_view().is_none());
    assert!(model.reconcile_request().is_none());
}

#[test]
fn context_change_keeps_exact_pending_and_correct_late_completion_is_historical() {
    let mut model = model();
    let request = model.inspect_request().unwrap();
    model.submitted(&request, id(&request));
    model.context_changed();
    assert_eq!(model.pending_cancel(), Some(id(&request)));
    model.completed(
        &request,
        id(&request),
        Ok(&Reply::RetainedEvidenceInspected(Box::new(
            archive_inspection(),
        ))),
    );
    assert!(model.pending.is_none() && model.inspection_current().is_none());
    assert!(
        model.cards.back().unwrap().historical && model.cards.back().unwrap().inspection.is_some()
    );
    install_view(&mut model, view());
    let request = model.read_request().unwrap();
    model.submitted(&request, id(&request));
    model.context_changed();
    model.completed(
        &request,
        id(&request),
        Ok(&Reply::EvidenceJob(Box::new(view()))),
    );
    assert!(model.current_view().is_none() && model.cards.back().unwrap().historical);
    assert!(model.reconcile_request().is_none());
}

#[test]
fn wrong_token_variant_or_identity_keeps_the_matching_pending_request() {
    let mut model = model();
    let request = model.inspect_request().unwrap();
    model.submitted(&request, id(&request));
    let reply = Reply::RetainedEvidenceInspected(Box::new(archive_inspection()));
    model.completed(
        &request,
        RequestId {
            sequence: id(&request).sequence + 1,
            ..id(&request)
        },
        Ok(&reply),
    );
    assert!(model.pending.is_some());
    model.completed(&request, id(&request), Ok(&Reply::EvidenceJobs(list(0))));
    assert!(model.pending.is_some());
    let mut wrong = archive_inspection();
    wrong.receipt.path = "/synthetic/another.osre".into();
    model.completed(
        &request,
        id(&request),
        Ok(&Reply::RetainedEvidenceInspected(Box::new(wrong))),
    );
    assert!(model.pending.is_some());
    model.completed(&request, id(&request), Ok(&reply));
    assert!(model.pending.is_none() && model.inspection_current().is_some());
}

#[test]
fn explicit_commands_require_the_offered_identity_and_reconcile_requires_fresh_read() {
    let mut model = model();
    let inspect = model.inspect_request().unwrap();
    assert!(inspect.reads_artifact() && !inspect.is_run());
    assert!(inspect.matches_command(&Command::InspectRetainedEvidence {
        path: ARCHIVE.into()
    }));
    let list = model.list_request().unwrap();
    assert!(!model.authorizes(&inspect, 1));
    assert!(model.authorizes(&list, 1));
    assert!(list.matches_command(&Command::ListEvidenceJobs) && !list.reads_artifact());
    install_view(&mut model, view());
    let request = model.reconcile_request().unwrap();
    assert!(request.reads_artifact() && !request.is_run());
    assert!(
        matches!(request.command(), Command::ReconcileEvidenceJob { expected_history_sha256, .. } if expected_history_sha256 == "d".repeat(64))
    );
    let mut forged = request.clone();
    if let EvidenceUiRequest::Reconcile {
        expected_history_sha256,
        ..
    } = &mut forged
    {
        *expected_history_sha256 = "f".repeat(64);
    }
    assert!(!model.authorizes(&forged, 1));
    let expected = export_request();
    answer(
        &mut model,
        &request,
        &job_reply(&expected, Some(successful_outcome(&expected))),
    );
    assert_eq!(
        model.cards.back().unwrap().history_sha.as_deref(),
        Some("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
    );
    assert!(
        model.current_view().is_none() && model.reconcile_request().is_none(),
        "compact outcome must not renew the old full-view history hash"
    );
    let mut updated = view();
    updated.history_sha256 = "e".repeat(64);
    let read = model.read_request().unwrap();
    answer(&mut model, &read, &Reply::EvidenceJob(Box::new(updated)));
    let reconcile = model.reconcile_request().unwrap();
    model.submitted(&reconcile, id(&reconcile));
    model.completed(
        &reconcile,
        id(&reconcile),
        Err(&ServiceError::backend("observation failed", true)),
    );
    assert!(model.current_view().is_none() && model.reconcile_request().is_none());
}

#[test]
fn cancellation_is_exact_idempotent_and_preserves_completion_before_cancel() {
    let mut model = model();
    model.review_export(&availability()).unwrap();
    let request = model.confirm_request().unwrap();
    let EvidenceUiRequest::Run {
        request: expected, ..
    } = &request
    else {
        panic!()
    };
    model.submitted(&request, id(&request));
    model.cancel_requested(
        RequestId {
            sequence: id(&request).sequence + 1,
            ..id(&request)
        },
        true,
    );
    assert!(!model.pending.as_ref().unwrap().cancel_attempted);
    model.cancel_requested(id(&request), true);
    model.cancel_requested(id(&request), false);
    assert!(
        model.pending.as_ref().unwrap().cancel_requested
            && model.pending_cancel() == Some(id(&request))
    );
    model.completed(
        &request,
        id(&request),
        Ok(&job_reply(expected, Some(successful_outcome(expected)))),
    );
    assert!(model.pending.is_none());
    let card = model.cards.back().unwrap();
    assert!(
        card.outcome
            .as_ref()
            .is_some_and(EvidenceOutcome::completed)
    );
    assert!(card.detail.contains("Nothing was rolled back"));
    let request = model.list_request().unwrap();
    model.submitted(&request, id(&request));
    assert!(model.pending_cancel().is_none());
    model.cancel_requested(id(&request), true);
    assert!(!model.pending.as_ref().unwrap().cancel_attempted);
}

#[test]
fn worker_loss_keeps_exact_uncertain_request_and_never_prepares_a_retry() {
    let mut model = model();
    model.review_export(&availability()).unwrap();
    let request = model.confirm_request().unwrap();
    let EvidenceUiRequest::Run {
        request: expected, ..
    } = &request
    else {
        panic!()
    };
    let fingerprint = expected.fingerprint().unwrap();
    model.submitted(&request, id(&request));
    model.worker_lost(1, "connection lost");
    let card = model.cards.back().unwrap();
    assert!(card.historical);
    assert_eq!(card.request_sha.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(card.request.as_ref().unwrap().job_id, expected.job_id);
    assert!(
        card.detail
            .contains("Partial or complete output may remain")
    );
    assert!(model.pending.is_none() && model.offered.is_none() && model.confirmation.is_none());
    let cards = model.cards.len();
    model.worker_lost(1, "duplicate");
    assert_eq!(model.cards.len(), cards);
    model.generation_changed(2);
    assert!(model.offered.is_none() && model.confirmation.is_none());
    assert!(!model.authorizes(&request, 2));
}

#[test]
fn list_view_inspection_and_outcome_bounds_are_checked_before_installation() {
    let mut model = model();
    let request = model.list_request().unwrap();
    answer(&mut model, &request, &Reply::EvidenceJobs(list(129)));
    assert!(model.jobs.is_none());
    install_view(&mut model, view());
    let mut invalid = view();
    invalid.request_sha256 = Some("f".repeat(64));
    let request = model.read_request().unwrap();
    answer(&mut model, &request, &Reply::EvidenceJob(Box::new(invalid)));
    assert!(model.current_view().is_none());
    let mut huge = view();
    huge.metadata_problems
        .push(omniusong_session::EvidenceMetadataProblem {
            slot: "terminal.json".into(),
            detail: EvidenceMessage {
                message: "x".repeat(MAX_EVIDENCE_JOB_VIEW_BYTES),
                message_truncated: false,
            },
        });
    let request = model.read_request().unwrap();
    answer(&mut model, &request, &Reply::EvidenceJob(Box::new(huge)));
    assert!(model.current_view().is_none());
    let mut dto = archive_inspection();
    dto.manifest.entries[0].path = "x".repeat(MAX_EVIDENCE_MANIFEST_BYTES + 1);
    let request = model.inspect_request().unwrap();
    answer(
        &mut model,
        &request,
        &Reply::RetainedEvidenceInspected(Box::new(dto)),
    );
    assert!(model.inspection_current().is_none());
    model.review_export(&availability()).unwrap();
    let request = model.confirm_request().unwrap();
    let EvidenceUiRequest::Run {
        request: expected, ..
    } = &request
    else {
        panic!()
    };
    let mut outcome = successful_outcome(expected);
    if let EvidenceOutcome::Exported { receipt } = &mut outcome {
        receipt.path = PathBuf::from(format!("/{}", "x".repeat(MAX_EVIDENCE_OUTCOME_REPLY_BYTES)));
    }
    answer(&mut model, &request, &job_reply(expected, Some(outcome)));
    assert!(model.cards.back().unwrap().outcome.is_none());
}

#[test]
fn bounded_history_cards_and_consumed_observation_capacity_do_not_grow() {
    let mut model = model();
    for _ in 0..20 {
        let request = model.list_request().unwrap();
        answer(&mut model, &request, &Reply::EvidenceJobs(list(128)));
    }
    assert_eq!(model.jobs.as_ref().unwrap().jobs.len(), 128);
    assert_eq!(model.cards.len(), 8);
    let mut full = view();
    full.occupied_observations = MAX_EVIDENCE_JOB_OBSERVATIONS;
    install_view(&mut model, full);
    assert!(model.reconcile_request().is_none());
    assert!(
        model.current_view().is_some(),
        "holes consume capacity without inventing observations"
    );
    model.epoch = u64::MAX;
    model.context_changed();
    assert!(model.exhausted);
    assert!(model.list_request().is_none() && model.review_export(&availability()).is_err());
}

#[test]
fn near_maximum_manifest_and_escaped_receipt_path_fit_the_derived_inspection_cap() {
    let mut dto = archive_inspection();
    dto.receipt.path = format!("/{}", "\"".repeat(4094)).into();
    let target = MAX_EVIDENCE_MANIFEST_BYTES - 2;
    let mut size = serde_json::to_vec(&dto.manifest).unwrap().len();
    for n in 0..MAX_EVIDENCE_ENTRIES - 1 {
        let prefix = format!("takes/{n:04}/");
        let mut entry = EvidenceEntry {
            path: format!("{prefix}{}", "\"".repeat(4096 - prefix.len())),
            kind: EvidenceEntryKind::File,
            byte_length: 0,
            sha256: Some("b".repeat(64)),
        };
        let contribution = serde_json::to_vec(&entry).unwrap().len() + 1;
        if size + contribution > target {
            let remaining = target.saturating_sub(size + contribution - 2 * (4096 - prefix.len()));
            entry.path = format!("{prefix}{}", "\"".repeat(remaining / 2));
            let contribution = serde_json::to_vec(&entry).unwrap().len() + 1;
            if size + contribution <= target {
                size += contribution;
                dto.manifest.entries.push(entry);
            }
            break;
        }
        size += contribution;
        dto.manifest.entries.push(entry);
    }
    assert_eq!(size, serde_json::to_vec(&dto.manifest).unwrap().len());
    assert!(size > MAX_EVIDENCE_MANIFEST_BYTES - 8);
    dto.receipt.file_count = dto.manifest.entries.len() as u64;
    dto.receipt.byte_length = 56 + size as u64 + dto.receipt.payload_bytes;
    assert!(bounded!(&dto, MAX_EVIDENCE_MANIFEST_BYTES + 8192).is_err());
    assert!(prepare_inspection(&dto, &dto.receipt.path, 1, 1).is_ok());
}

#[test]
fn narrow_light_and_dark_views_keep_cancel_above_collapsed_scrolling_history() {
    for dark in [false, true] {
        for width in [240.0, 350.0] {
            let context = egui::Context::default();
            context.enable_accesskit();
            context.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            context.all_styles_mut(|style| style.animation_time = 0.0);
            let mut model = model();
            model.page = Page::History;
            let request = model.list_request().unwrap();
            answer(&mut model, &request, &Reply::EvidenceJobs(list(128)));
            let request = model.inspect_request().unwrap();
            model.submitted(&request, id(&request));
            let (output, action, content_width) =
                frame(&context, &mut model, &availability(), width, Vec::new());
            assert!(action.is_none());
            assert!(content_width <= width + 1.0, "{width}: {content_width}");
            let tree = output.platform_output.accesskit_update.unwrap();
            let cancel = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some("Cancel evidence request"))
                .expect("cancel outside collapsed details");
            assert!(cancel.1.bounds().is_some_and(|bounds| bounds.y1 <= 420.0));
            let rows = tree
                .nodes
                .iter()
                .filter(|(_, node)| node.label().is_some_and(valid_id))
                .count();
            assert!(rows < 32, "bounded visible row count {rows}");
            assert!(
                matches!(click(&context, &mut model, "Cancel evidence request"), Some(EvidenceAction::Cancel(actual)) if actual == id(&request))
            );
            assert!(
                model.pending.is_some(),
                "paint returns a cancellation intent, never changes service credit"
            );
        }
    }
}

#[test]
fn explicit_metadata_connect_and_review_buttons_emit_no_hidden_command() {
    let context = egui::Context::default();
    context.enable_accesskit();
    let mut model = model();
    let unavailable = EvidenceAvailability {
        can_submit: false,
        can_read_artifacts: false,
        can_run: false,
        generation: None,
        origin: Some(ORIGIN.into()),
        unavailable_reason: Some("Open history without initializing a studio".into()),
    };
    let _ = frame(&context, &mut model, &unavailable, 350.0, Vec::new());
    let tree = frame(&context, &mut model, &unavailable, 350.0, Vec::new())
        .0
        .platform_output
        .accesskit_update
        .unwrap();
    let target = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Connect history"))
        .unwrap()
        .0;
    let action = frame(
        &context,
        &mut model,
        &unavailable,
        350.0,
        vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: tree.tree_id,
                target_node: target,
                data: None,
            },
        )],
    )
    .1;
    assert!(matches!(action, Some(EvidenceAction::MetadataConnect)));
    assert!(model.offered.is_none() && model.pending.is_none() && model.confirmation.is_none());
    assert!(click(&context, &mut model, "Review export").is_none());
    assert!(model.confirmation.is_some() && model.pending.is_none());
}

#[test]
fn original_sqlite_cause_governor_stop_and_worker_retirement_render_separately() {
    for stop in [
        EvidenceBudgetStop::Cancelled,
        serde_json::from_value(
            serde_json::json!({"limit":{"resource":"WorkBytes","limit":10,"attempted":11}}),
        )
        .unwrap(),
    ] {
        let problem = EvidenceProblem {
            kind: EvidenceProblemKind::Sqlite,
            origin: EvidenceResourceOrigin::LiveCatalog,
            detail: EvidenceMessage {
                message: "Original live catalog SQLite failure".into(),
                message_truncated: false,
            },
            sqlite_code: Some(10),
            budget_stop: Some(stop),
        };
        let outcome = EvidenceOutcome::Stopped { problem };
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 500.0),
                )),
                ..Default::default()
            },
            |ui| EvidenceUi::outcome_details(ui, &outcome),
        );
        output.textures_delta.clear();
        let tree = output.platform_output.accesskit_update.unwrap();
        let labels: Vec<&str> = tree
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().or_else(|| node.value()))
            .collect();
        assert!(
            labels.contains(&"Original live catalog SQLite failure"),
            "{labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|text| text.contains("also observed") || text.contains("also observed by"))
        );
        if matches!(
            &outcome,
            EvidenceOutcome::Stopped {
                problem: EvidenceProblem {
                    budget_stop: Some(EvidenceBudgetStop::Limit { .. }),
                    ..
                }
            }
        ) {
            assert!(labels.iter().any(|text| text.contains("limit 10")
                && text.contains("attempted 11")
                && text.contains("WorkBytes")));
        }
        let mut model = model();
        model.review_export(&availability()).unwrap();
        let request = model.confirm_request().unwrap();
        let EvidenceUiRequest::Run {
            request: expected, ..
        } = &request
        else {
            panic!()
        };
        let mut reply = job_reply(expected, Some(outcome));
        if let Reply::EvidenceJobOutcome(reply) = &mut reply {
            reply.metadata_problem = Some(EvidenceMessage {
                message: "Terminal history could not be synced".into(),
                message_truncated: false,
            });
            reply.catalog_retirement = Some(ServiceError::storage_unavailable(
                "Storage worker stopped",
                true,
            ));
        }
        answer(&mut model, &request, &reply);
        let card = model.cards.back().unwrap();
        assert!(card.outcome.is_some() && card.metadata.is_some() && card.retirement.is_some());
        assert_eq!(model.lost_generation, Some(1));
    }
}
