//! Independent presentation regressions. All sources and completions are synthetic;
//! painting must not become an audio or storage continuation.
use super::{test_support::*, *};
use serde_json::{Value, json};

const PROJECT: &str = "synthetic-project";
const BEGIN_COPY: &str = "Saving the recording choice before opening output";
const FINISH_COPY: &str = "Saving the exact Finish choice";
const STOP_COPY: &str = "Stop requested. Saving an intent";

fn task_snapshot(task: &Option<Task>) -> Value {
    match task {
        None => Value::Null,
        Some(task) => {
            let command = match &task.command {
                Command::ListPerformanceIntents => json!({"kind": "list"}),
                Command::SavePerformanceIntent {
                    expected_revision,
                    intent,
                } => {
                    json!({"kind": "save", "expected_revision": expected_revision, "intent": intent})
                }
                Command::AttachPerformance { id, revision } => {
                    json!({"kind": "attach", "id": id, "revision": revision})
                }
                command => panic!("unexpected performance command: {command:?}"),
            };
            json!({"effect": format!("{:?}", task.effect), "command": command})
        }
    }
}

fn snapshot(p: &PerformanceUi) -> Value {
    let card = p.card.as_ref().map(|card| {
        json!({
            "intent": card.intent,
            "receipt": card.receipt,
            "status": card.status,
            "activated": card.activated,
            "started_here": card.started_here,
            "cancelled": card.cancelled,
            "stopped": card.stopped,
            "engine_retry": card.engine_retry,
            "attached": card.attached,
            "first_error": card.first_error,
            "prior_unknown": card.prior_unknown,
            "unstarted_rejected": card.unstarted_rejected,
        })
    });
    json!({
        "root": p.root,
        "listed": p.listed,
        "retained": p.retained,
        "card": card,
        "queued": task_snapshot(&p.queued),
        "active": task_snapshot(&p.active),
        "retry": task_snapshot(&p.retry),
        "engine": p.engine,
        "error": p.error,
        "message": p.message,
        "local_cancelled": p.local_cancelled,
        "guards": {
            "busy": p.busy(),
            "blocks": p.blocks(),
            "engaged": p.engaged(),
            "pending_status": p.pending_status(),
            "can_finish": p.can_finish(),
            "wants_release_stop": p.wants_release_stop(),
            "status_request": p.status_request(),
        },
    })
}

struct Painted {
    text: String,
    labels: Vec<String>,
    content_width: f32,
}

fn paint(p: &mut PerformanceUi, width: f32, dark: bool) -> Painted {
    let before = snapshot(p);
    let expected_text = {
        let view = p.presentation();
        [
            Some(view.heading),
            Some(view.body),
            view.pending_notice,
            view.storage_notice,
            view.history_notice,
            view.first_audio_error,
            view.error,
        ]
        .into_iter()
        .flatten()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>()
    };
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.set_visuals(if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    });
    let context = (PROJECT.to_owned(), 7);
    let mut final_output = None;
    let mut content_width = 0.0;
    for _ in 0..9 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 2200.0),
            )),
            time: Some(ctx.cumulative_frame_nr() as f64 / 30.0),
            ..Default::default()
        };
        // Supply native device scale as backend input. A pending pre-frame
        // zoom change can otherwise replace the strict viewport bounds.
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(1.5);
        let mut output = ctx.run_ui(input, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                assert!(ui.available_width() <= width + 1.0);
                p.ui(ui, true, true, Some(&context));
                content_width = ui.min_rect().width();
            });
        });
        output.textures_delta.clear();
        final_output = Some(output);
        assert_eq!(snapshot(p), before, "paint changed exact recording work");
    }
    let tree = final_output
        .unwrap()
        .platform_output
        .accesskit_update
        .expect("real AX tree");
    let labels: Vec<_> = tree
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == egui::accesskit::Role::Label)
        .filter_map(|(_, node)| node.value().or_else(|| node.label()).map(str::to_owned))
        .collect();
    let painted = Painted {
        text: labels.join("\n"),
        labels,
        content_width,
    };
    for expected in expected_text {
        assert!(
            painted.text.contains(&expected),
            "presenter text missing from actual AX labels: {expected:?}; {:?}",
            painted.labels
        );
    }
    painted
}

fn assert_copy(p: &mut PerformanceUi, required: &[&str], forbidden: &[&str]) {
    for dark in [false, true] {
        for width in [240.0, 480.0, 1080.0] {
            let painted = paint(p, width, dark);
            for expected in required {
                assert!(
                    painted.text.contains(expected),
                    "missing {expected:?} at {width}, dark={dark}: {:?}",
                    painted.labels
                );
            }
            for stale in forbidden {
                assert!(
                    !painted.text.contains(stale),
                    "stale {stale:?} at {width}, dark={dark}: {:?}",
                    painted.labels
                );
            }
            assert!(
                painted.content_width <= width + 1.0,
                "card overflowed {width}: {}",
                painted.content_width
            );
        }
    }
}

fn shows_outcome_uncertainty(painted: &Painted) -> bool {
    painted
        .labels
        .iter()
        .filter(|label| !label.starts_with("Parent transfer-loss count:"))
        .any(|label| {
            let lower = label.to_lowercase();
            lower.contains("uncertain")
                || lower.contains("unknown")
                || lower.contains("unconfirmed")
                || lower.contains("not confirmed")
        })
}

fn prepared() -> PerformanceUi {
    let mut p = PerformanceUi::new();
    p.set_root(PathBuf::from("/synthetic/performance-store"));
    p.begin(
        PROJECT.to_owned(),
        None,
        LiveClock {
            domain: "presentation-gui".into(),
            epoch: "presentation-clock".into(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 1,
        },
    )
    .unwrap();
    p
}

fn save(p: &mut PerformanceUi, revision: u64) -> Command {
    let command = p.take_command().expect("explicit queued choice");
    p.completed(&command, Ok(&receipt(&command, revision)));
    command
}

fn recording() -> (PerformanceUi, LiveScope) {
    let mut p = prepared();
    save(&mut p, 1);
    assert!(matches!(
        p.take_engine(),
        Some(EngineCommand::StartPerformance { .. })
    ));
    let intent = p.card.as_ref().unwrap().intent.clone();
    let ready = p
        .observe(status(&intent, 2))
        .unwrap()
        .expect("accepted real Ready fixture");
    (p, ready.scope)
}

fn finish_sent() -> PerformanceUi {
    let (mut p, scope) = recording();
    p.finish(scope);
    save(&mut p, 2);
    assert!(matches!(
        p.take_engine(),
        Some(EngineCommand::FinishPerformance { .. })
    ));
    p
}

fn sealed_card() -> PerformanceUi {
    let mut p = finish_sent();
    let intent = p.card.as_ref().unwrap().intent.clone();
    p.observe(sealed(&intent, 3)).unwrap();
    p
}

#[test]
fn accepted_recording_does_not_keep_the_pre_start_saving_message() {
    let (mut p, _) = recording();
    assert!(p.card.as_ref().unwrap().activated);
    assert!(p.queued.is_none() && p.active.is_none() && p.retry.is_none());
    assert_copy(
        &mut p,
        &[
            "Recording keys and pads",
            "Last worker status: Recording",
            "Synthetic output only · 48000 Hz · 2 channels · f32",
            "Parent transfer-loss count: unavailable / unknown.",
        ],
        &[BEGIN_COPY, FINISH_COPY, STOP_COPY],
    );
}

#[test]
fn confirmed_seal_survives_automatic_output_stop_without_stop_request_prose() {
    let mut p = sealed_card();
    assert!(p.wants_release_stop());
    // This is the existing AudioUi post-Sealed retirement path. It must not
    // change the presentation into an interrupted, unconfirmed final tail.
    assert!(p.cancel());
    assert_copy(
        &mut p,
        &[
            "Recording saved locally",
            "Output release is still pending. The saved original alone does not confirm Stop.",
        ],
        &[
            BEGIN_COPY,
            FINISH_COPY,
            STOP_COPY,
            "without a promised final tail",
        ],
    );
    p.stopped();
    assert_copy(
        &mut p,
        &["Recording saved locally"],
        &[
            BEGIN_COPY,
            FINISH_COPY,
            STOP_COPY,
            "without a promised final tail",
            "Output release is still pending",
        ],
    );
    assert!(
        p.card
            .as_ref()
            .unwrap()
            .status
            .as_ref()
            .unwrap()
            .sealed
            .is_some()
    );
}

#[test]
fn failed_interrupted_and_uncertain_seals_keep_the_actual_first_cause_visible() {
    for phase in [
        PerformancePhase::Failed,
        PerformancePhase::Interrupted,
        PerformancePhase::SealUncertain,
    ] {
        let mut p = finish_sent();
        let intent = p.card.as_ref().unwrap().intent.clone();
        let mut observed = sealed(&intent, 3);
        observed.sealed = None;
        observed.phase = phase;
        observed.error =
            Some("Synthetic callback cause: original ring stopped accepting packets".into());
        assert!(observed.is_valid());
        p.observe(observed).unwrap();
        p.engine_failed("Synthetic later transport retirement".into());
        assert_eq!(
            p.card.as_ref().unwrap().first_error.as_deref(),
            Some("Synthetic callback cause: original ring stopped accepting packets")
        );
        assert_copy(
            &mut p,
            &[
                "First audio failure: Synthetic callback cause: original ring stopped accepting packets",
                match phase {
                    PerformancePhase::Failed => "Recording failed",
                    PerformancePhase::Interrupted => "Recording interrupted",
                    PerformancePhase::SealUncertain => "Seal outcome uncertain",
                    _ => unreachable!(),
                },
            ],
            &[
                BEGIN_COPY,
                FINISH_COPY,
                "Original saved locally",
                "Synthetic later transport retirement",
            ],
        );
        let painted = paint(&mut p, 360.0, true);
        assert!(
            shows_outcome_uncertainty(&painted),
            "failure must not promise a complete tail: {:?}",
            painted.labels
        );
    }
}

#[test]
fn unknown_start_save_and_later_rejection_do_not_erase_prior_uncertainty_or_exact_retry() {
    let mut p = prepared();
    let first = p.take_command().unwrap();
    p.completed(
        &first,
        Err(&ServiceError::backend(
            "Synthetic storage acknowledgement lost",
            true,
        )),
    );
    assert!(p.card.as_ref().unwrap().prior_unknown);
    assert!(PerformanceUi::matches(
        &first,
        &p.retry.as_ref().unwrap().command
    ));
    assert_copy(
        &mut p,
        &["Synthetic storage acknowledgement lost"],
        &["Original saved locally", "No original exists"],
    );
    let painted = paint(&mut p, 360.0, false);
    assert!(shows_outcome_uncertainty(&painted));

    // An explicit retry of the same request is rejected before admission. The
    // older unknown write outcome still exists and is not a known cancellation.
    p.queued = p.retry.take();
    let retry = p.take_command().unwrap();
    assert!(PerformanceUi::matches(&first, &retry));
    p.completed(
        &retry,
        Err(&ServiceError {
            kind: omniusong_session::FailureKind::IntentRejected,
            message: "Synthetic later known rejection".into(),
            outcome_unknown: false,
        }),
    );
    assert!(p.card.as_ref().unwrap().prior_unknown);
    assert!(!p.card.as_ref().unwrap().unstarted_rejected);
    assert_copy(
        &mut p,
        &["Synthetic later known rejection"],
        &["Original saved locally", "No original exists"],
    );
    let painted = paint(&mut p, 360.0, true);
    assert!(
        shows_outcome_uncertainty(&painted),
        "a later rejection must not erase an older unknown outcome: {:?}",
        painted.labels
    );
}

#[test]
fn pre_ready_audio_failure_retains_the_exact_start_retry_without_a_recording_claim() {
    let mut p = prepared();
    save(&mut p, 1);
    let command = p.take_engine().expect("Start was explicitly sent");
    p.engine_failed("Synthetic output failed before Ready".into());
    assert_eq!(
        serde_json::to_value(&command).unwrap(),
        serde_json::to_value(&p.card.as_ref().unwrap().engine_retry).unwrap()
    );
    assert!(!p.card.as_ref().unwrap().activated);
    assert_copy(
        &mut p,
        &["Synthetic output failed before Ready"],
        &[BEGIN_COPY, "Original saved locally"],
    );
}

fn reopen_retained(receipt: PerformanceIntentReceipt) -> PerformanceUi {
    let mut p = PerformanceUi::new();
    p.set_root(PathBuf::from("/synthetic/performance-store"));
    let command = p.take_command().unwrap();
    p.completed(
        &command,
        Ok(&Reply::PerformanceIntents(vec![receipt.clone()])),
    );
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let context = (PROJECT.to_owned(), 7);
    for target_label in [
        "Retained recording choices".to_owned(),
        format!(
            "Review {} · choice r{}{}",
            receipt.intent.start.take_id(),
            receipt.revision,
            if receipt.intent.released {
                " · released"
            } else {
                ""
            }
        ),
    ] {
        let mut output = None;
        for _ in 0..9 {
            let mut frame_output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 2200.0),
                    )),
                    time: Some(ctx.cumulative_frame_nr() as f64 / 30.0),
                    ..Default::default()
                },
                |ui| p.ui(ui, true, true, Some(&context)),
            );
            frame_output.textures_delta.clear();
            output = Some(frame_output);
        }
        let tree = output.unwrap().platform_output.accesskit_update.unwrap();
        let target = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(target_label.as_str()))
            .unwrap_or_else(|| panic!("missing explicit review choice {target_label}"))
            .0;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1080.0, 2200.0),
                )),
                time: Some(ctx.cumulative_frame_nr() as f64 / 30.0),
                events: vec![egui::Event::AccessKitActionRequest(
                    egui::accesskit::ActionRequest {
                        action: egui::accesskit::Action::Click,
                        target_tree: tree.tree_id,
                        target_node: target,
                        data: None,
                    },
                )],
                ..Default::default()
            },
            |ui| p.ui(ui, true, true, Some(&context)),
        );
        output.textures_delta.clear();
    }
    assert!(p.has_card());
    p
}

#[test]
fn reopened_historical_intent_is_review_evidence_and_never_claims_current_recording_or_seal() {
    for kind in ["attachment", "finish", "released"] {
        let mut receipt = retained_attachment();
        if kind != "attachment" {
            receipt.intent.attachment = None;
        }
        receipt.intent.released = kind == "released";
        receipt.intent.validate().unwrap();
        let mut p = reopen_retained(receipt.clone());
        let card = p.card.as_ref().unwrap();
        assert_eq!(card.receipt.as_ref(), Some(&receipt));
        assert!(!card.started_here && !card.activated && card.stopped);
        assert!(card.status.is_none());
        assert!(
            p.engine.is_none() && p.queued.is_none() && p.active.is_none() && p.retry.is_none()
        );
        assert_copy(
            &mut p,
            &[
                "Saved recording choice",
                "source has not been inspected",
                "did not start or resume recording",
            ],
            &[
                BEGIN_COPY,
                FINISH_COPY,
                STOP_COPY,
                "Recording saved locally",
                "Original saved locally",
                "Original attached",
                "Attachment operation confirmed",
            ],
        );
    }
}

#[test]
fn start_presentation_distinguishes_queued_active_and_saved_before_real_ready() {
    let mut p = prepared();
    assert_copy(
        &mut p,
        &[
            "Preparing recording choice",
            "Start choice queued for storage.",
        ],
        &["Saved recording choice", "Original saved locally"],
    );
    let command = p.take_command().unwrap();
    assert_copy(
        &mut p,
        &["Preparing recording choice", "Saving Start choice."],
        &["Saved recording choice", "Original saved locally"],
    );
    p.completed(&command, Ok(&receipt(&command, 1)));
    assert!(
        p.engine.is_some(),
        "paint must preserve the exact queued Start continuation"
    );
    assert_copy(
        &mut p,
        &["Preparing recording output"],
        &[
            BEGIN_COPY,
            "Saving Start choice.",
            "source has not been inspected",
            "Recording keys and pads",
            "Original saved locally",
        ],
    );
    assert!(matches!(
        p.take_engine(),
        Some(EngineCommand::StartPerformance { .. })
    ));
    let intent = p.card.as_ref().unwrap().intent.clone();
    let mut starting = status(&intent, 1);
    starting.phase = PerformancePhase::Starting;
    starting.ready = None;
    starting.live = None;
    starting.native_sample_format = None;
    starting.identity.descriptor_sha256 = None;
    assert!(starting.is_valid());
    assert!(p.observe(starting).unwrap().is_none());
    assert_copy(
        &mut p,
        &["Preparing recording output", "Last worker status: Starting"],
        &[
            BEGIN_COPY,
            "Recording keys and pads",
            "Original saved locally",
        ],
    );
}

#[test]
fn stop_before_start_save_is_unstarted_until_a_matching_receipt_arrives() {
    let mut p = prepared();
    let command = p.take_command().unwrap();
    assert!(!p.cancel());
    assert_copy(
        &mut p,
        &["Unstarted recording choice", "Saving Start choice."],
        &[
            "Saved recording choice",
            "No original exists",
            "Original saved locally",
        ],
    );
    p.completed(&command, Ok(&receipt(&command, 1)));
    assert!(p.engine.is_none());
    assert_copy(
        &mut p,
        &["Saved recording choice"],
        &[
            BEGIN_COPY,
            STOP_COPY,
            "Preparing recording output",
            "Original saved locally",
        ],
    );
}

#[test]
fn local_finish_request_is_distinct_from_queued_active_retry_and_sent_finish() {
    let (mut p, scope) = recording();
    p.finish(scope);
    let finish_identity = p.card.as_ref().unwrap().intent.finish.clone();
    assert_copy(
        &mut p,
        &[
            "Finish requested",
            "Finish choice queued for storage.",
            "Last worker status: Recording",
        ],
        &[
            BEGIN_COPY,
            FINISH_COPY,
            "Recording keys and pads",
            "Original saved locally",
        ],
    );
    let command = p.take_command().unwrap();
    assert_copy(
        &mut p,
        &["Finish requested", "Saving Finish choice."],
        &["Finishing recording", "Original saved locally"],
    );
    p.completed(
        &command,
        Err(&ServiceError::backend(
            "Synthetic Finish save acknowledgement lost",
            true,
        )),
    );
    assert_copy(
        &mut p,
        &[
            "Finish requested",
            "Finish choice retained for explicit retry.",
            "Synthetic Finish save acknowledgement lost",
            "An earlier recording storage outcome was uncertain.",
        ],
        &[
            "Saving Finish choice.",
            FINISH_COPY,
            "Finishing recording",
            "Original saved locally",
        ],
    );
    assert_eq!(p.card.as_ref().unwrap().intent.finish, finish_identity);
    assert!(PerformanceUi::matches(
        &command,
        &p.retry.as_ref().unwrap().command
    ));

    p.queued = p.retry.take();
    let same = p.take_command().unwrap();
    p.completed(&same, Ok(&receipt(&same, 2)));
    assert!(p.engine.is_some());
    assert_copy(
        &mut p,
        &[
            "Finish requested",
            "An earlier recording storage outcome was uncertain.",
        ],
        &[
            "Saving Finish choice.",
            "Finish choice retained for explicit retry.",
            "Finishing recording",
            "Original saved locally",
        ],
    );
    assert_eq!(p.card.as_ref().unwrap().intent.finish, finish_identity);
}

#[test]
fn worker_finish_retirement_and_sealing_observations_override_old_action_prose() {
    use omniusong_engine::NativeRetirementStatus;
    let variants = [
        (
            PerformancePhase::Finishing,
            false,
            NativeRetirementStatus::default(),
            "Finishing recording",
        ),
        (
            PerformancePhase::Stopping,
            true,
            NativeRetirementStatus::default(),
            "Stopping recording",
        ),
        (
            PerformancePhase::Stopping,
            true,
            NativeRetirementStatus {
                pause_succeeded: true,
                renderer_destroyed: true,
                error_destroyed: false,
                producer_destroyed: false,
            },
            "Stopping recording",
        ),
        (
            PerformancePhase::Stopping,
            true,
            NativeRetirementStatus {
                pause_succeeded: true,
                renderer_destroyed: true,
                error_destroyed: true,
                producer_destroyed: true,
            },
            "Stopping recording",
        ),
        (
            PerformancePhase::Sealing,
            true,
            NativeRetirementStatus {
                pause_succeeded: true,
                renderer_destroyed: true,
                error_destroyed: true,
                producer_destroyed: true,
            },
            "Saving recorded original",
        ),
    ];
    for (phase, callback_cut, native, heading) in variants {
        let mut p = finish_sent();
        let intent = p.card.as_ref().unwrap().intent.clone();
        let mut observed = sealed(&intent, 3);
        observed.sealed = None;
        observed.phase = phase;
        observed.native = native;
        if !callback_cut {
            observed.cut = None;
            observed.writer.as_mut().unwrap().finish_cut = None;
        }
        assert!(observed.is_valid());
        p.observe(observed).unwrap();
        let worker = format!("Last worker status: {phase:?}");
        assert_copy(
            &mut p,
            &[
                heading,
                &worker,
                "Rendered 4 frames · durable 4 frames · 1 complete callbacks",
            ],
            &[
                BEGIN_COPY,
                FINISH_COPY,
                STOP_COPY,
                "Recording keys and pads",
                "Original saved locally",
                "Stop confirmed",
            ],
        );
        assert!(
            !p.card.as_ref().unwrap().stopped,
            "native flags are not an urgent Stop acknowledgement"
        );
    }
}

#[test]
fn explicit_stop_and_typed_retirement_override_an_old_recording_observation() {
    let (mut p, _) = recording();
    assert!(p.cancel());
    assert_copy(
        &mut p,
        &[
            "Stop requested · confirmation pending",
            "Last worker status: Recording",
        ],
        &[
            BEGIN_COPY,
            STOP_COPY,
            "Recording keys and pads",
            "Original saved locally",
            "Stop confirmed",
        ],
    );
    p.stopped();
    assert_copy(
        &mut p,
        &["Recording inactive", "Last worker status: Recording"],
        &[
            BEGIN_COPY,
            STOP_COPY,
            "Stop requested · confirmation pending",
            "Recording keys and pads",
            "Original saved locally",
            "Stop confirmed",
        ],
    );

    let (mut retired, _) = recording();
    retired.retired("Synthetic actor retirement first cause".into());
    assert_copy(
        &mut retired,
        &[
            "Recording inactive",
            "Last worker status: Recording",
            "Synthetic actor retirement first cause",
        ],
        &[
            BEGIN_COPY,
            STOP_COPY,
            "Recording keys and pads",
            "Original saved locally",
            "Stop confirmed",
        ],
    );
}

#[test]
fn attachment_choice_and_publication_errors_remain_separate_from_the_saved_original() {
    for outcome_unknown in [false, true] {
        let mut p = sealed_card();
        let original = p
            .card
            .as_ref()
            .unwrap()
            .status
            .as_ref()
            .unwrap()
            .sealed
            .clone();
        p.stopped();
        p.attach(PROJECT, 7);
        let publication = p.card.as_ref().unwrap().intent.attachment.clone().unwrap();
        assert_copy(
            &mut p,
            &[
                "Recording saved locally",
                "Attachment choice queued for storage.",
            ],
            &[
                "Attachment operation confirmed",
                "Original attached",
                "Applying stored attachment.",
            ],
        );
        let persist = p.take_command().unwrap();
        assert_copy(
            &mut p,
            &["Recording saved locally", "Saving Attachment choice."],
            &[
                "Attachment operation confirmed",
                "Original attached",
                "Applying stored attachment.",
            ],
        );
        p.completed(&persist, Ok(&receipt(&persist, 3)));
        assert_copy(
            &mut p,
            &[
                "Recording saved locally",
                "Stored attachment queued for storage.",
            ],
            &[
                "Attachment operation confirmed",
                "Original attached",
                "Applying stored attachment.",
            ],
        );
        let attach = p.take_command().unwrap();
        assert!(matches!(
            attach,
            Command::AttachPerformance { revision: 3, .. }
        ));
        assert_copy(
            &mut p,
            &["Recording saved locally", "Applying stored attachment."],
            &["Attachment operation confirmed", "Original attached"],
        );
        let error = if outcome_unknown {
            "Synthetic publication acknowledgement lost; operation may have committed"
        } else {
            "Synthetic publication revision conflict; original retained"
        };
        p.completed(&attach, Err(&ServiceError::backend(error, outcome_unknown)));
        let mut forbidden = vec![
            "Applying stored attachment.",
            "Saving Attachment choice.",
            "Attachment operation confirmed",
            "Original attached",
        ];
        if outcome_unknown {
            forbidden.push("not attached");
        }
        assert_copy(
            &mut p,
            &[
                "Recording saved locally",
                "Original saved locally · attachment not confirmed here",
                "Stored attachment retained for explicit retry.",
                error,
            ],
            &forbidden,
        );
        assert_eq!(
            p.card.as_ref().unwrap().intent.attachment.as_ref(),
            Some(&publication)
        );
        assert!(PerformanceUi::matches(
            &attach,
            &p.retry.as_ref().unwrap().command
        ));
        assert_eq!(p.error.as_deref(), Some(error));
        assert_eq!(p.card.as_ref().unwrap().prior_unknown, outcome_unknown);
        assert!(!p.card.as_ref().unwrap().attached);
        assert_eq!(
            p.card.as_ref().unwrap().status.as_ref().unwrap().sealed,
            original
        );
    }
}

#[test]
fn confirmed_attachment_and_failed_release_do_not_claim_current_song_placement() {
    let mut p = sealed_card();
    p.stopped();
    p.attach(PROJECT, 7);
    save(&mut p, 3);
    let attach = p.take_command().unwrap();
    let reply = Reply::Document {
        view: omniusong_session::DocumentView::prepare(
            omniusong_core::ProjectDocument::new("Synthetic attachment replay").unwrap(),
        )
        .unwrap(),
        library: omniusong_session::Library::default(),
        replayed: true,
    };
    p.completed(&attach, Ok(&reply));
    assert!(p.card.as_ref().unwrap().attached);
    assert_copy(
        &mut p,
        &[
            "Attachment operation confirmed",
            "Return-to-session choice queued for storage.",
        ],
        &["Original attached", "Applying stored attachment."],
    );
    let release = p.take_command().unwrap();
    assert_copy(
        &mut p,
        &[
            "Attachment operation confirmed",
            "Saving Return-to-session choice.",
        ],
        &["Original attached", "Applying stored attachment."],
    );
    p.completed(
        &release,
        Err(&ServiceError::backend(
            "Synthetic release acknowledgement lost",
            true,
        )),
    );
    assert_copy(
        &mut p,
        &[
            "Attachment operation confirmed",
            "Return-to-session choice retained for explicit retry.",
            "An earlier recording storage outcome was uncertain.",
            "Synthetic release acknowledgement lost",
        ],
        &["Original attached", "Saving Return-to-session choice."],
    );
}

#[test]
fn return_to_session_waits_for_exact_release_receipt_and_never_continues_from_paint() {
    let mut p = sealed_card();
    p.stopped();
    p.release();
    assert_copy(
        &mut p,
        &[
            "Recording saved locally",
            "Return-to-session choice queued for storage.",
        ],
        &["session editing is available"],
    );
    let release = p.take_command().unwrap();
    assert_copy(
        &mut p,
        &[
            "Recording saved locally",
            "Saving Return-to-session choice.",
        ],
        &["session editing is available"],
    );
    p.completed(
        &release,
        Err(&ServiceError::backend(
            "Synthetic return choice save failure",
            false,
        )),
    );
    assert_copy(
        &mut p,
        &[
            "Recording saved locally",
            "Return-to-session choice retained for explicit retry.",
            "Synthetic return choice save failure",
        ],
        &[
            "Saving Return-to-session choice.",
            "session editing is available",
        ],
    );
    p.queued = p.retry.take();
    let retry = p.take_command().unwrap();
    assert!(PerformanceUi::matches(&release, &retry));
    p.completed(&retry, Ok(&receipt(&retry, 3)));
    assert!(!p.has_card());
    assert_copy(
        &mut p,
        &["Recorded output", "session editing is available"],
        &[BEGIN_COPY, FINISH_COPY, STOP_COPY],
    );
}

#[test]
fn stopped_unsaved_metadata_is_a_retained_choice_operation_not_a_saved_original() {
    let (mut p, _) = recording();
    p.cancel();
    p.stopped();
    p.release();
    assert_eq!(p.queued.as_ref().unwrap().effect, Effect::PrepareRelease);
    assert_copy(
        &mut p,
        &[
            "Recording inactive",
            "Retained recording choice queued for storage.",
        ],
        &[
            BEGIN_COPY,
            STOP_COPY,
            "Recording saved locally",
            "Original saved locally",
        ],
    );
    let metadata = p.take_command().unwrap();
    assert_copy(
        &mut p,
        &["Recording inactive", "Saving Retained recording choice."],
        &["Recording saved locally", "Original saved locally"],
    );
    p.completed(
        &metadata,
        Err(&ServiceError::backend(
            "Synthetic retained metadata save failure",
            false,
        )),
    );
    assert_copy(
        &mut p,
        &[
            "Recording inactive",
            "Retained recording choice retained for explicit retry.",
            "Synthetic retained metadata save failure",
        ],
        &[
            "Saving Retained recording choice.",
            "Recording saved locally",
            "Original saved locally",
        ],
    );
}

#[test]
fn saved_choice_listing_reports_loading_without_inventing_a_recording() {
    let mut p = PerformanceUi::new();
    p.set_root(PathBuf::from("/synthetic/performance-store"));
    let list = p.take_command().unwrap();
    assert!(matches!(list, Command::ListPerformanceIntents));
    assert_copy(
        &mut p,
        &["Recorded output", "Loading saved recording choices."],
        &[
            BEGIN_COPY,
            "Preparing recording output",
            "Recording keys and pads",
            "Recording saved locally",
        ],
    );
}

#[test]
fn old_or_invalid_status_cannot_replace_the_accepted_recording_presentation() {
    let (mut p, _) = recording();
    let intent = p.card.as_ref().unwrap().intent.clone();
    let before = snapshot(&p);
    let mut older = status(&intent, 1);
    older.phase = PerformancePhase::Starting;
    assert!(p.observe(older).unwrap().is_none());
    assert_eq!(snapshot(&p), before);
    let mut changed_at_same_revision = status(&intent, 2);
    changed_at_same_revision.phase = PerformancePhase::Failed;
    changed_at_same_revision.error = Some("Synthetic unaccepted same-revision fault".into());
    assert!(p.observe(changed_at_same_revision).is_err());
    assert_eq!(snapshot(&p), before);
    assert_copy(
        &mut p,
        &["Recording keys and pads", "Last worker status: Recording"],
        &[
            BEGIN_COPY,
            "Preparing recording output",
            "Recording failed",
            "Synthetic unaccepted same-revision fault",
        ],
    );
}

#[test]
fn a_confirmed_seal_keeps_first_audio_failure_and_current_storage_error_separate() {
    let mut p = finish_sent();
    p.engine_failed("Synthetic original audio failure before the observed seal".into());
    let intent = p.card.as_ref().unwrap().intent.clone();
    p.observe(sealed(&intent, 3)).unwrap();
    p.stopped();
    p.attach(PROJECT, 7);
    let persist = p.take_command().unwrap();
    p.completed(
        &persist,
        Err(&ServiceError::backend(
            "Synthetic current attachment storage failure",
            true,
        )),
    );
    assert_copy(
        &mut p,
        &[
            "Recording saved locally",
            "First audio failure: Synthetic original audio failure before the observed seal",
            "Synthetic current attachment storage failure",
            "Attachment choice retained for explicit retry.",
            "An earlier recording storage outcome was uncertain.",
        ],
        &[
            BEGIN_COPY,
            FINISH_COPY,
            STOP_COPY,
            "Saving Attachment choice.",
            "Attachment operation confirmed",
        ],
    );
}

#[test]
fn inconsistent_effect_command_shape_is_disclosed_without_consuming_or_repairing_it() {
    let mut p = sealed_card();
    let id = p.card.as_ref().unwrap().intent.start.id;
    p.queued = Some(Task {
        effect: Effect::Finish,
        command: Command::AttachPerformance { id, revision: 2 },
    });
    let painted = paint(&mut p, 360.0, true);
    assert!(
        painted.text.to_lowercase().contains("inconsistent"),
        "malformed task shape was presented as a valid operation: {:?}",
        painted.labels
    );
    assert!(!painted.text.contains("Applying stored attachment."));
    assert!(!painted.text.contains("Finish choice queued for storage."));
}

#[test]
fn malformed_coexisting_tasks_show_current_work_before_retained_retry_without_consuming_either() {
    let mut p = sealed_card();
    p.stopped();
    p.attach(PROJECT, 7);
    let command = p.take_command().unwrap();
    p.retry = Some(Task {
        command: command.clone(),
        effect: Effect::Attach,
    });
    assert_copy(
        &mut p,
        &["Recording saved locally", "Saving Attachment choice."],
        &[
            "Attachment choice retained for explicit retry.",
            "Attachment operation confirmed",
        ],
    );
    assert!(p.active.is_some() && p.retry.is_some());
    p.queued = p.active.take();
    assert_copy(
        &mut p,
        &[
            "Recording saved locally",
            "Attachment choice queued for storage.",
        ],
        &[
            "Saving Attachment choice.",
            "Attachment choice retained for explicit retry.",
            "Attachment operation confirmed",
        ],
    );
    assert!(p.queued.is_some() && p.retry.is_some());
    assert!(PerformanceUi::matches(
        &command,
        &p.queued.as_ref().unwrap().command
    ));
    assert!(PerformanceUi::matches(
        &command,
        &p.retry.as_ref().unwrap().command
    ));
}
