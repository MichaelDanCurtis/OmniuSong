use super::*;
use omniusong_session::{
    PerformanceSourceAttachmentReceipt, PerformanceSourceAttachmentState,
    PerformanceSourceDecisionEvent, PerformanceSourceDecisionReceipt,
    PerformanceSourceDecisionRevision, PerformanceSourceExecutionReceipt,
    PerformanceSourceExecutionRevision, PreparedPerformanceSourceActionReceipt,
};
use serde_json::json;
use sha2::{Digest, Sha256};

pub(crate) mod test_support {
    use super::*;
    pub(crate) const ACTION: &str = "11111111111111111111111111111111";
    pub(crate) const STAGE: &str = "02020202020202020202020202020202";
    #[derive(Clone, Copy)]
    pub(crate) enum Phase {
        Prepared,
        Started,
        Verified,
    }
    pub(crate) struct Snapshot {
        pub(crate) uncertain: bool,
        pub(crate) proof: bool,
        pub(crate) pending: Option<RequestId>,
        pub(crate) cards: Vec<(String, Option<bool>, Option<String>)>,
    }
    pub(crate) fn snapshot(model: &SourceRecoveryUi) -> Snapshot {
        Snapshot {
            uncertain: model.uncertain.is_some(),
            proof: model.proof.is_some(),
            pending: model.pending.as_ref().map(|p| p.id),
            cards: model
                .cards
                .iter()
                .map(|c| {
                    (
                        c.title.clone(),
                        c.committed,
                        c.nested.as_ref().map(|(_, _, text)| text.clone()),
                    )
                })
                .collect(),
        }
    }
    pub(crate) fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
    pub(crate) fn review(origin: &Path) -> Vec<u8> {
        let scope = json!({"worker_generation":vec![1u8;16],"session_id":vec![2u8;16],"panic_generation":1});
        let streams = ["audio", "gui-1", "gui-2", "gui-3", "gui-4", "control"];
        let prefixes: Vec<_> = streams.iter().map(|id| json!({"stream_id":id,"next_sequence":1,"source_end":{"unit":if *id=="audio" {"audio_frames"} else {"events"},"end":if *id=="audio" {1024} else {1}},"recorded_units":if *id=="audio" {1024} else {1},"gap_units":0,"gap_chunks":0})).collect();
        let chunks: Vec<_> = streams
            .iter()
            .map(|id| json!({"stream_id":id,"sequence":0,"sha256":"7".repeat(64)}))
            .collect();
        let mut source: PerformanceSourceReview = serde_json::from_value(json!({
            "version":1,"stage_id":STAGE,"journal_path":origin.join("captures").join(STAGE).join("take.sqlite3"),"take_id":format!("performance-{STAGE}"),
            "profile_version":2,"state":"interrupted","byte_length":8192,"manifest_sha256":null,"manifest_interrupted":false,"verified_payload_bytes":4096,"generic_problems":[],
            "config":{"take_id":format!("performance-{STAGE}"),"info":{"scope":scope,"sample_rate":48000,"channels":2},
                "gui_clock":{"domain":"synthetic-gui","epoch":"synthetic-clock","ticks_per_second_numerator":1000000000u64,"ticks_per_second_denominator":1},"output_epoch":"synthetic-output",
                "sources":[{"binding":{"kind":"computer_keys","handle":1,"connection_generation":1},"endpoint":"source-1"},{"binding":{"kind":"pointer_keys","handle":2,"connection_generation":1},"endpoint":"source-2"},{"binding":{"kind":"pads","handle":3,"connection_generation":1},"endpoint":"source-3"},{"binding":{"kind":"accessible_pulse","handle":4,"connection_generation":1},"endpoint":"source-4"}]},
            "performance":{"descriptor_sha256":"a".repeat(64),"original_rows_sha256":"b".repeat(64),"journal_sha256_at_review":"c".repeat(64),"selection":{"chunk_count":6,"chunk_index_sha256":"9".repeat(64)},"checkpoint_sha256":"8".repeat(64),
                "checkpoint":{"group_sequence":1,"previous_checkpoint_sha256":null,"first_callback_index":1,"last_callback_index":2,"audio_end":1024,"scope_after":scope,"chunks":chunks,"prefixes":prefixes},
                "received_generic_chunks":6,"unknown_tail":true,"problem":null,"capture_fault":null,"capture_fault_pending":false},
            "usage":{"file_bytes":8192,"physical_rows":6,"work_bytes":32768,"row_visits":12,"chunk_visits":6,"record_visits":6,"groups":1,"vm_steps":10},"review_sha256":""
        })).unwrap();
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.performance-source-review.v1\0");
        hash.update(serde_json::to_vec(&source).unwrap());
        source.review_sha256 = format!("{:x}", hash.finalize());
        // Preserve a byte-distinct, valid review instead of canonicalizing it.
        format!(" \n{}\n", serde_json::to_string(&source).unwrap()).into_bytes()
    }
    pub(crate) fn offer(origin: &Path) -> SourceRecoveryOffer {
        SourceRecoveryOffer {
            generation: 1,
            epoch: 7,
            serial: 9,
            review: review(origin),
        }
    }
    pub(crate) fn detail(origin: &Path, phase: Phase) -> PerformanceSourceActionDetail {
        let raw = review(origin);
        let action = PreparedPerformanceSourceAction::prepare(
            ACTION.into(),
            &raw,
            OperationId("synthetic-copy-operation".into()),
            None,
        )
        .unwrap();
        let payload_sha256 = hash(&serde_json::to_vec(&action).unwrap());
        let prepared = PreparedPerformanceSourceActionReceipt {
            revision: 1,
            action,
            payload_sha256: payload_sha256.clone(),
        };
        let mut state = PerformanceSourceActionState {
            prepared,
            revision: 1,
            payload_sha256,
            execution: None,
            decisions: Default::default(),
        };
        if !matches!(phase, Phase::Prepared) {
            let execution = PerformanceSourceExecutionRevision {
                version: 1,
                id: ACTION.into(),
                prepared_payload_sha256: state.prepared.payload_sha256.clone(),
                phase: if matches!(phase, Phase::Started) {
                    PerformanceSourceExecutionPhase::Started
                } else {
                    PerformanceSourceExecutionPhase::Verified {
                        result: Box::new(result()),
                        usage: Default::default(),
                    }
                },
            };
            state.revision = if matches!(phase, Phase::Started) {
                2
            } else {
                3
            };
            state.payload_sha256 = hash(&serde_json::to_vec(&execution).unwrap());
            state.execution = Some(execution);
        }
        let dto = PerformanceSourceActionDetail {
            version: 1,
            origin_root: origin.into(),
            state,
            review: raw,
        };
        dto.validate().unwrap();
        dto
    }
    pub(crate) fn result() -> RecoveredPerformanceSourceResult {
        RecoveredPerformanceSourceResult {
            take_id: format!("performance-{STAGE}"),
            descriptor_sha256: "a".repeat(64),
            original_rows_sha256: "b".repeat(64),
            evidence_sha256: "c".repeat(64),
            evidence_bytes: 8192,
            working_sha256: "d".repeat(64),
            working_bytes: 12288,
            manifest_sha256: "e".repeat(64),
            selected_chunk_count: 6,
            selected_chunk_index_sha256: "9".repeat(64),
            unknown_tail: true,
        }
    }
    pub(crate) fn choice(
        dto: &PerformanceSourceActionDetail,
        target: &SourceRecoveryTarget,
        id: &str,
        frame: i64,
    ) -> PerformanceSourceAttachmentChoice {
        PerformanceSourceAttachmentChoice {
            id: id.into(),
            supersedes: None,
            publication: TakePublication {
                operation_id: attachment_operation_id(&dto.state.prepared.action.id, id).unwrap(),
                project_id: target.project_id.clone(),
                expected_revision: target.revision,
                track_id: TrackId(format!("track-{id}")),
                track_name: "Recovered checkpoint".into(),
                clip: TakeClip {
                    id: format!("clip-{id}"),
                    source: TakeReference {
                        take_id: TakeId(result().take_id),
                        manifest_sha256: result().manifest_sha256,
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames {
                        start: 0,
                        count: 1024,
                    },
                    position: TimelinePosition {
                        frame,
                        sample_rate: 48000,
                    },
                    fades: None,
                    edit_source: None,
                },
            },
        }
    }
    fn append(
        dto: &PerformanceSourceActionDetail,
        event: PerformanceSourceDecisionEvent,
    ) -> PerformanceSourceActionDetail {
        let mut next = dto.clone();
        let row = PerformanceSourceDecisionRevision {
            version: 1,
            id: ACTION.into(),
            prepared_payload_sha256: dto.state.prepared.payload_sha256.clone(),
            verified_payload_sha256: hash(
                &serde_json::to_vec(dto.state.execution.as_ref().unwrap()).unwrap(),
            ),
            decision: event,
        };
        next.state.revision += 1;
        next.state.payload_sha256 = hash(&serde_json::to_vec(&row).unwrap());
        match row.decision {
            PerformanceSourceDecisionEvent::KeptLocally => next.state.decisions.kept_locally = true,
            PerformanceSourceDecisionEvent::ReturnedToReview => {
                next.state.decisions.kept_locally = false
            }
            PerformanceSourceDecisionEvent::AttachmentPrepared { choice } => {
                next.state.decisions.attachment = Some(PerformanceSourceAttachmentState {
                    revision: next.state.revision,
                    payload_sha256: next.state.payload_sha256.clone(),
                    choice,
                    status: PerformanceSourceAttachmentStatus::Pending,
                })
            }
            PerformanceSourceDecisionEvent::AttachmentRetired { .. } => {
                next.state.decisions.attachment.as_mut().unwrap().status =
                    PerformanceSourceAttachmentStatus::Retired
            }
            PerformanceSourceDecisionEvent::AttachmentCommitted {
                project_revision, ..
            } => {
                next.state.decisions.attachment.as_mut().unwrap().status =
                    PerformanceSourceAttachmentStatus::Committed { project_revision }
            }
        }
        next.validate().unwrap();
        next
    }
    pub(crate) fn decided(
        dto: &PerformanceSourceActionDetail,
        decision: &PerformanceSourceDecision,
    ) -> PerformanceSourceActionDetail {
        let event = match decision {
            PerformanceSourceDecision::KeepLocally => PerformanceSourceDecisionEvent::KeptLocally,
            PerformanceSourceDecision::ReturnToReview => {
                PerformanceSourceDecisionEvent::ReturnedToReview
            }
            PerformanceSourceDecision::PrepareAttachment { choice } => {
                PerformanceSourceDecisionEvent::AttachmentPrepared {
                    choice: choice.clone(),
                }
            }
            PerformanceSourceDecision::RetireAttachment => {
                PerformanceSourceDecisionEvent::AttachmentRetired {
                    choice_id: dto
                        .state
                        .decisions
                        .attachment
                        .as_ref()
                        .unwrap()
                        .choice
                        .id
                        .clone(),
                }
            }
        };
        append(dto, event)
    }
    pub(crate) fn committed(
        dto: &PerformanceSourceActionDetail,
        revision: u64,
    ) -> PerformanceSourceActionDetail {
        append(
            dto,
            PerformanceSourceDecisionEvent::AttachmentCommitted {
                choice_id: dto
                    .state
                    .decisions
                    .attachment
                    .as_ref()
                    .unwrap()
                    .choice
                    .id
                    .clone(),
                project_revision: revision,
            },
        )
    }
}

use test_support::{ACTION, Phase, detail};
const ORIGIN: &str = "/synthetic/studio";
fn available() -> SourceRecoveryAvailability {
    SourceRecoveryAvailability {
        can_submit: true,
        can_mutate: true,
        can_read_artifacts: true,
        generation: Some(1),
        origin: Some(ORIGIN.into()),
        review: Some(test_support::offer(Path::new(ORIGIN))),
        target: Some(SourceRecoveryTarget {
            project_id: ProjectId("target-project".into()),
            revision: 4,
            requires_native_audio_upgrade: false,
        }),
        unavailable_reason: None,
    }
}
fn model() -> SourceRecoveryUi {
    let mut model = SourceRecoveryUi::default();
    model.sync(&available());
    model
}
fn token(request: &SourceRecoveryRequest) -> RequestId {
    RequestId {
        generation: request.generation,
        sequence: request.serial,
    }
}
fn answer(model: &mut SourceRecoveryUi, request: &SourceRecoveryRequest, reply: &Reply) {
    assert!(model.authorizes(request, request.generation));
    model.submitted(request, token(request));
    assert!(model.pending.is_some());
    assert!(request.matches_reply(reply));
    assert!(
        model.completed(request, token(request), Ok(reply)),
        "accepted current body"
    );
    assert!(model.pending.is_none());
}
fn list(model: &mut SourceRecoveryUi, states: Vec<PerformanceSourceActionState>) {
    let request = model.list_request().unwrap();
    answer(
        model,
        &request,
        &Reply::PerformanceSourceActionStates(states),
    );
}
fn install(model: &mut SourceRecoveryUi, dto: PerformanceSourceActionDetail) {
    list(model, vec![dto.state.clone()]);
    assert!(model.select_action(&dto.state.prepared.action.id));
    let request = model.read_request().unwrap();
    answer(
        model,
        &request,
        &Reply::PerformanceSourceActionDetail(Box::new(dto)),
    );
    assert!(model.current_detail().is_some());
}
fn decision_reply(request: &SourceRecoveryRequest, prior: &PerformanceSourceActionDetail) -> Reply {
    let SourceRecoveryOperation::Decision { decision, .. } = &request.operation else {
        panic!()
    };
    let next = test_support::decided(prior, decision);
    Reply::PerformanceSourceDecision(Box::new(PerformanceSourceDecisionReceipt {
        requested: request.reference().unwrap(),
        state: next.state,
    }))
}
fn execution_reply(request: &SourceRecoveryRequest, phase: Phase) -> Reply {
    Reply::PerformanceSourceExecution(Box::new(PerformanceSourceExecutionReceipt {
        requested: request.reference().unwrap(),
        state: detail(Path::new(ORIGIN), phase).state,
        observed: test_support::result(),
        usage: Default::default(),
        replayed: matches!(request.operation, SourceRecoveryOperation::Reconcile { .. }),
        original_reopened: matches!(request.operation, SourceRecoveryOperation::Execute { .. }),
    }))
}

#[test]
fn performance_action_prepare_requires_explicit_list_reuses_identity_and_never_chains() {
    let mut model = model();
    assert!(model.prepare_review(&available()).is_err());
    list(&mut model, Vec::new());
    model.prepare_review(&available()).unwrap();
    let action = match &model.confirmation.as_ref().unwrap().request.operation {
        SourceRecoveryOperation::Prepare { action, .. } => action.clone(),
        _ => panic!(),
    };
    assert!(!model.confirmation.as_ref().unwrap().approved);
    model.prepare_review(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    let SourceRecoveryOperation::Prepare {
        action: actual,
        review,
    } = &request.operation
    else {
        panic!()
    };
    assert_eq!(actual, &action);
    assert_eq!(review, &available().review.unwrap().review);
    assert_eq!(action.origin, None);
    assert!(model.authorizes(&request, 1));
    let receipt = PreparedPerformanceSourceActionReceipt {
        revision: 1,
        action: *action.clone(),
        payload_sha256: test_support::hash(&serde_json::to_vec(&action).unwrap()),
    };
    answer(
        &mut model,
        &request,
        &Reply::PreparedPerformanceSourceAction(Box::new(receipt)),
    );
    assert!(
        model.current_detail().is_some()
            && model.confirmation.is_none()
            && model.offered.is_none()
            && model.pending.is_none()
    );
    assert!(
        model
            .cards
            .back()
            .unwrap()
            .title
            .contains("has not been requested")
    );
    assert!(
        model.prepare_review(&available()).is_err(),
        "successful save requires an explicit list again"
    );
    let saved = model.detail.as_ref().unwrap().dto.state.clone();
    list(&mut model, vec![saved]);
    assert!(
        model.prepare_review(&available()).is_err(),
        "known same-stage action is not replaced"
    );
}

#[test]
fn performance_action_execution_and_started_reconciliation_are_separate_explicit_steps() {
    let mut model = model();
    install(&mut model, detail(Path::new(ORIGIN), Phase::Prepared));
    model.review_execute(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    assert!(request.is_mutation() && request.is_artifact());
    answer(
        &mut model,
        &request,
        &execution_reply(&request, Phase::Verified),
    );
    assert!(verified(&model.current_detail().unwrap().dto.state).is_some());
    assert!(model.confirmation.is_none() && model.offered.is_none());
    let mut model = SourceRecoveryUi::default();
    model.sync(&available());
    install(&mut model, detail(Path::new(ORIGIN), Phase::Started));
    assert!(model.review_execute(&available()).is_err());
    let request = model.reconcile_request().unwrap();
    assert!(request.is_artifact() && !request.is_mutation());
    answer(
        &mut model,
        &request,
        &execution_reply(&request, Phase::Started),
    );
    assert!(verified(&model.current_detail().unwrap().dto.state).is_none());
    assert!(model.proof.is_some());
    assert!(
        model
            .cards
            .back()
            .unwrap()
            .title
            .contains("recorded Started")
    );
    model.review_execute(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    assert!(request.proof_serial.is_some());
    answer(
        &mut model,
        &request,
        &execution_reply(&request, Phase::Verified),
    );
    assert!(verified(&model.current_detail().unwrap().dto.state).is_some());
}

#[test]
fn performance_action_keep_return_pending_retirement_and_replacement_bind_exact_choices() {
    let mut model = model();
    let mut dto = detail(Path::new(ORIGIN), Phase::Verified);
    install(&mut model, dto.clone());
    model
        .review_decision(PerformanceSourceDecision::KeepLocally, &available())
        .unwrap();
    let request = model.confirm_request().unwrap();
    let reply = decision_reply(&request, &dto);
    dto = test_support::decided(&dto, &PerformanceSourceDecision::KeepLocally);
    answer(&mut model, &request, &reply);
    assert!(
        model
            .current_detail()
            .unwrap()
            .dto
            .state
            .decisions
            .kept_locally
    );
    assert!(model.review_attachment(&available()).is_err());
    model
        .review_decision(PerformanceSourceDecision::ReturnToReview, &available())
        .unwrap();
    let request = model.confirm_request().unwrap();
    let reply = decision_reply(&request, &dto);
    dto = test_support::decided(&dto, &PerformanceSourceDecision::ReturnToReview);
    answer(&mut model, &request, &reply);
    model.set_track_name("Recovered piano".into());
    model.set_timeline_frame("12345".into());
    model.review_attachment(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    let SourceRecoveryOperation::Decision { decision, .. } = &request.operation else {
        panic!()
    };
    let PerformanceSourceDecision::PrepareAttachment { choice } = decision.as_ref() else {
        panic!()
    };
    let first = choice.clone();
    assert!(first.supersedes.is_none());
    assert_eq!(first.publication.clip.position.frame, 12345);
    assert_eq!(first.publication.clip.position.sample_rate, 48000);
    assert_eq!(
        first.publication.clip.range,
        TakeRange::AudioFrames {
            start: 0,
            count: 1024
        }
    );
    assert_eq!(
        first.publication.project_id,
        available().target.unwrap().project_id
    );
    let reply = decision_reply(&request, &dto);
    dto = test_support::decided(&dto, decision);
    answer(&mut model, &request, &reply);
    assert!(model.confirmation.is_none() && model.offered.is_none());
    assert!(model.review_attachment(&available()).is_err());
    model
        .review_decision(PerformanceSourceDecision::KeepLocally, &available())
        .unwrap();
    let request = model.confirm_request().unwrap();
    let reply = decision_reply(&request, &dto);
    dto = test_support::decided(&dto, &PerformanceSourceDecision::KeepLocally);
    answer(&mut model, &request, &reply);
    assert_eq!(
        model
            .current_detail()
            .unwrap()
            .dto
            .state
            .decisions
            .attachment
            .as_ref()
            .unwrap()
            .choice,
        first
    );
    assert!(model.review_attach(&available()).is_err());
    model
        .review_decision(PerformanceSourceDecision::ReturnToReview, &available())
        .unwrap();
    let request = model.confirm_request().unwrap();
    let reply = decision_reply(&request, &dto);
    dto = test_support::decided(&dto, &PerformanceSourceDecision::ReturnToReview);
    answer(&mut model, &request, &reply);
    model
        .review_decision(PerformanceSourceDecision::RetireAttachment, &available())
        .unwrap();
    let request = model.confirm_request().unwrap();
    let reply = decision_reply(&request, &dto);
    answer(&mut model, &request, &reply);
    assert!(
        model.review_attachment(&available()).is_err(),
        "atomic retirement still needs a fresh explicit catalog Read"
    );
    let retired = test_support::decided(&dto, &PerformanceSourceDecision::RetireAttachment);
    model.context_changed();
    let read = model.read_request().unwrap();
    assert_eq!(read.reference(), Some(reference(&retired.state)));
    answer(
        &mut model,
        &read,
        &Reply::PerformanceSourceActionDetail(Box::new(retired)),
    );
    model.review_attachment(&available()).unwrap();
    let replacement = model.confirm_request().unwrap();
    let SourceRecoveryOperation::Decision { decision, .. } = &replacement.operation else {
        panic!()
    };
    let PerformanceSourceDecision::PrepareAttachment { choice } = decision.as_ref() else {
        panic!()
    };
    assert_eq!(choice.supersedes.as_deref(), Some(first.id.as_str()));
    assert_ne!(choice.id, first.id);
    assert_ne!(
        choice.publication.operation_id,
        first.publication.operation_id
    );
    assert_ne!(choice.publication.clip.id, first.publication.clip.id);
    assert_ne!(choice.publication.track_id, first.publication.track_id);
}

#[test]
fn performance_action_target_placement_consent_and_field_round_trips_invalidate_confirmation() {
    let mut available = available();
    available
        .target
        .as_mut()
        .unwrap()
        .requires_native_audio_upgrade = true;
    let mut model = model();
    model.sync(&available);
    install(&mut model, detail(Path::new(ORIGIN), Phase::Verified));
    model.set_track_name("Recovered original".into());
    model.set_timeline_frame("0".into());
    assert!(model.review_attachment(&available).is_err());
    model.set_upgrade_consent(true);
    model.review_attachment(&available).unwrap();
    let request = model.confirm_request().unwrap();
    assert!(request.permits_native_audio_upgrade());
    model.review_attachment(&available).unwrap();
    assert!(model.confirm_request().unwrap().same(&request));
    model.set_timeline_frame("1".into());
    model.set_timeline_frame("0".into());
    assert!(!model.authorizes(&request, 1));
    assert!(model.current_detail().is_some());
    for invalid in [
        "-1",
        "1.5",
        " 1",
        "9223372036854775808",
        "9223372036854775807",
    ] {
        model.set_timeline_frame(invalid.into());
        assert!(model.review_attachment(&available).is_err());
    }
    model.set_timeline_frame("0".into());
    model.review_attachment(&available).unwrap();
    let request = model.confirm_request().unwrap();
    model.context_changed();
    assert!(!model.authorizes(&request, 1));
    assert!(!model.upgrade_consent && model.current_detail().is_none());
    install(&mut model, detail(Path::new(ORIGIN), Phase::Verified));
    model.set_upgrade_consent(true);
    model.review_attachment(&available).unwrap();
    let request = model.confirm_request().unwrap();
    available.target.as_mut().unwrap().revision += 1;
    model.sync(&available);
    assert!(!model.authorizes(&request, 1));
    assert!(!model.upgrade_consent && model.current_detail().is_none());
}

fn pending_choice() -> PerformanceSourceActionDetail {
    let dto = detail(Path::new(ORIGIN), Phase::Verified);
    let choice = test_support::choice(
        &dto,
        &available().target.unwrap(),
        "33333333333333333333333333333333",
        123,
    );
    test_support::decided(
        &dto,
        &PerformanceSourceDecision::PrepareAttachment {
            choice: Box::new(choice),
        },
    )
}
fn attachment_reply(
    request: &SourceRecoveryRequest,
    dto: &PerformanceSourceActionDetail,
    committed: bool,
    error: Option<ServiceError>,
) -> Reply {
    Reply::PerformanceSourceAttachment(Box::new(PerformanceSourceAttachmentReceipt {
        requested: request.reference().unwrap(),
        state: dto.state.clone(),
        publication: request
            .expected_choice
            .as_ref()
            .unwrap()
            .publication
            .clone(),
        committed,
        replayed: true,
        current_document: None,
        current_document_error: error,
    }))
}

#[test]
fn performance_action_accepted_failed_reads_retire_proof_refused_submissions_preserve_it() {
    let mut model = model();
    install(&mut model, detail(Path::new(ORIGIN), Phase::Started));
    let request = model.reconcile_request().unwrap();
    answer(
        &mut model,
        &request,
        &execution_reply(&request, Phase::Started),
    );
    assert!(model.proof.is_some());
    let read = model.read_request().unwrap();
    model.submission_failed(&read, "busy");
    assert!(model.current_detail().is_some() && model.proof.is_some());
    let read = model.read_request().unwrap();
    model.submitted(&read, token(&read));
    assert!(model.current_detail().is_none() && model.proof.is_none());
    model.completed(
        &read,
        token(&read),
        Err(&ServiceError::backend("read failed", false)),
    );
    assert!(model.current_detail().is_none() && model.proof.is_none());
    assert!(model.review_execute(&available()).is_err());
    install(&mut model, detail(Path::new(ORIGIN), Phase::Started));
    let request = model.reconcile_request().unwrap();
    model.submitted(&request, token(&request));
    model.completed(
        &request,
        token(&request),
        Err(&ServiceError {
            kind: FailureKind::InspectionLimit,
            message: "bounded work stopped".into(),
            outcome_unknown: false,
        }),
    );
    assert!(model.proof.is_none() && model.current_detail().is_none());
    assert!(model.cards.back().unwrap().detail.contains("policy limit"));
}

#[test]
fn performance_action_wrong_reference_or_publication_keeps_pending_and_stale_success_is_historical()
{
    let mut model = model();
    let dto = pending_choice();
    install(&mut model, dto.clone());
    model.review_attach(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    assert_eq!(
        request.reference().unwrap().revision,
        dto.state.decisions.attachment.as_ref().unwrap().revision
    );
    model.submitted(&request, token(&request));
    let committed = test_support::committed(&dto, 5);
    let mut wrong = attachment_reply(&request, &committed, true, None);
    if let Reply::PerformanceSourceAttachment(reply) = &mut wrong {
        reply.requested.payload_sha256 = "f".repeat(64);
    }
    model.completed(&request, token(&request), Ok(&wrong));
    assert!(model.pending.is_some());
    let mut wrong = attachment_reply(&request, &committed, true, None);
    if let Reply::PerformanceSourceAttachment(reply) = &mut wrong {
        reply.publication.clip.position.frame += 1;
    }
    assert!(!request.matches_reply(&wrong));
    model.completed(&request, token(&request), Ok(&wrong));
    assert!(model.pending.is_some());
    let wrong_id = RequestId {
        sequence: token(&request).sequence + 1,
        ..token(&request)
    };
    let reply = attachment_reply(&request, &committed, true, None);
    model.completed(&request, wrong_id, Ok(&reply));
    assert!(model.pending.is_some());
    model.context_changed();
    assert_eq!(model.pending_cancel(), Some(token(&request)));
    assert!(!model.completed(&request, token(&request), Ok(&reply)));
    assert!(model.pending.is_none() && model.current_detail().is_none());
    let card = model.cards.back().unwrap();
    assert!(card.historical && card.committed == Some(true));
    let row = model
        .rows
        .iter()
        .find(|row| row.reference.id == ACTION)
        .unwrap();
    assert_eq!(row.reference, reference(&dto.state));
    assert_eq!(
        row.phase,
        phase(&dto.state),
        "a stale result does not advance the known row"
    );
    assert!(model.confirmation.is_none() && model.offered.is_none());
}

#[test]
fn performance_action_accepted_outcomes_advance_known_rows_before_context_invalidates_detail() {
    let mut model = model();
    let verified = detail(Path::new(ORIGIN), Phase::Verified);
    let kept = test_support::decided(&verified, &PerformanceSourceDecision::KeepLocally);
    install(&mut model, kept.clone());
    assert_eq!(model.rows[0].reference.revision, 4);
    let listed_generation = model.list_generation;

    model
        .review_decision(PerformanceSourceDecision::ReturnToReview, &available())
        .unwrap();
    let request = model.confirm_request().unwrap();
    let returned = test_support::decided(&kept, &PerformanceSourceDecision::ReturnToReview);
    answer(&mut model, &request, &decision_reply(&request, &kept));
    assert_eq!(model.rows[0].reference, reference(&returned.state));
    assert_eq!(model.rows[0].phase, phase(&returned.state));
    assert_eq!(model.list_generation, listed_generation);

    model.set_track_name("Recovered checkpoint".into());
    model.set_timeline_frame("24680".into());
    model.review_attachment(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    let SourceRecoveryOperation::Decision { decision, .. } = &request.operation else {
        panic!()
    };
    let pending = test_support::decided(&returned, decision);
    answer(&mut model, &request, &decision_reply(&request, &returned));
    assert_eq!(model.rows.len(), 1);
    assert_eq!(model.rows[0].reference, reference(&pending.state));
    assert_eq!(model.rows[0].reference.revision, 6);
    assert_eq!(model.rows[0].phase, "Attachment pending");
    assert_eq!(model.rows[0].stage, pending.state.prepared.action.stage_id);
    assert_eq!(model.rows[0].take, pending.state.prepared.action.take_id);
    assert_eq!(
        model.list_generation, listed_generation,
        "a current row update is not a fresh catalog List"
    );

    let mut target_changed = available();
    target_changed.target.as_mut().unwrap().revision += 1;
    model.sync(&target_changed);
    assert!(model.current_detail().is_none());
    assert!(model.offered.is_none() && model.confirmation.is_none() && model.pending.is_none());
    let read = model.read_request().unwrap();
    assert_eq!(read.reference(), Some(reference(&pending.state)));
    assert!(matches!(
        read.command(),
        Command::ReadPerformanceSourceAction { revision: 6, .. }
    ));
    answer(
        &mut model,
        &read,
        &Reply::PerformanceSourceActionDetail(Box::new(pending)),
    );
    assert!(model.current_detail().unwrap().catalog_read);
    assert_eq!(
        model
            .current_detail()
            .unwrap()
            .dto
            .state
            .decisions
            .attachment
            .as_ref()
            .unwrap()
            .choice
            .publication
            .expected_revision,
        4
    );
    assert!(model.offered.is_none() && model.confirmation.is_none() && model.pending.is_none());
}

#[test]
fn performance_action_new_prepared_and_verified_rows_block_duplicate_ids_after_context_changes() {
    let mut model = model();
    list(&mut model, Vec::new());
    model.prepare_review(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    let SourceRecoveryOperation::Prepare { action, .. } = &request.operation else {
        panic!()
    };
    let receipt = PreparedPerformanceSourceActionReceipt {
        revision: 1,
        action: *action.clone(),
        payload_sha256: test_support::hash(&serde_json::to_vec(action).unwrap()),
    };
    answer(
        &mut model,
        &request,
        &Reply::PreparedPerformanceSourceAction(Box::new(receipt)),
    );
    let saved = model.current_detail().unwrap().dto.clone();
    assert_eq!(model.rows.len(), 1);
    assert_eq!(model.rows[0].reference, reference(&saved.state));
    assert_eq!(model.rows[0].phase, "Prepared · not executed");
    assert_eq!(
        model.list_generation, None,
        "a new choice does not fabricate list freshness"
    );
    list(&mut model, vec![saved.state.clone()]);
    model.context_changed();
    assert!(model.current_detail().is_none());
    let error = model.prepare_review(&available()).unwrap_err();
    assert!(error.contains("saved choice already refers"));
    assert!(model.confirmation.is_none() && model.offered.is_none());
    let read = model.read_request().unwrap();
    assert_eq!(read.reference(), Some(reference(&saved.state)));
    answer(
        &mut model,
        &read,
        &Reply::PerformanceSourceActionDetail(saved.clone()),
    );

    model.review_execute(&available()).unwrap();
    let execute = model.confirm_request().unwrap();
    let mut verified = *saved;
    let mut execution = detail(Path::new(ORIGIN), Phase::Verified)
        .state
        .execution
        .unwrap();
    execution.id = verified.state.prepared.action.id.clone();
    execution.prepared_payload_sha256 = verified.state.prepared.payload_sha256.clone();
    verified.state.revision = 3;
    verified.state.payload_sha256 = test_support::hash(&serde_json::to_vec(&execution).unwrap());
    verified.state.execution = Some(execution);
    verified.validate().unwrap();
    let reply = Reply::PerformanceSourceExecution(Box::new(PerformanceSourceExecutionReceipt {
        requested: execute.reference().unwrap(),
        state: verified.state.clone(),
        observed: test_support::result(),
        usage: Default::default(),
        replayed: false,
        original_reopened: true,
    }));
    answer(&mut model, &execute, &reply);
    assert_eq!(model.rows.len(), 1);
    assert_eq!(model.rows[0].reference, reference(&verified.state));
    assert_eq!(model.rows[0].phase, phase(&verified.state));
    model.context_changed();
    assert!(model.current_detail().is_none());
    assert!(
        model
            .prepare_review(&available())
            .unwrap_err()
            .contains("saved choice already refers")
    );
    assert!(model.confirmation.is_none() && model.offered.is_none());
    let read = model.read_request().unwrap();
    assert_eq!(read.reference(), Some(reference(&verified.state)));
    assert!(matches!(
        read.command(),
        Command::ReadPerformanceSourceAction { revision: 3, .. }
    ));
    assert!(
        model.pending.is_none(),
        "no automatic artifact request follows the current row update"
    );
}

#[test]
fn performance_action_exhausted_proof_counter_does_not_partially_advance_a_known_row() {
    let mut model = model();
    let prepared = detail(Path::new(ORIGIN), Phase::Prepared);
    install(&mut model, prepared.clone());
    model.review_execute(&available()).unwrap();
    let execute = model.confirm_request().unwrap();
    model.submitted(&execute, token(&execute));
    model.serial = u64::MAX - 1;
    let reply = execution_reply(&execute, Phase::Verified);
    assert!(!model.completed(&execute, token(&execute), Ok(&reply)));
    assert!(model.exhausted && model.pending.is_none());
    assert_eq!(model.rows.len(), 1);
    assert_eq!(model.rows[0].reference, reference(&prepared.state));
    assert_eq!(model.rows[0].phase, "Prepared · not executed");
    assert!(model.current_detail().is_none() && model.proof.is_none());
    assert!(
        model
            .uncertain
            .as_ref()
            .is_some_and(|pinned| pinned.same(&execute))
    );
}

#[test]
fn performance_action_protocol_uncertainty_pins_exact_request_beyond_card_turnover_and_replays_only_metadata()
 {
    let mut model = model();
    list(&mut model, Vec::new());
    model.prepare_review(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    model.submitted(&request, token(&request));
    model.completed(
        &request,
        token(&request),
        Err(&ServiceError {
            kind: FailureKind::Protocol,
            message: "matching identity had a malformed body".into(),
            outcome_unknown: false,
        }),
    );
    assert!(model.pending.is_none());
    assert!(model.uncertain.as_ref().unwrap().same(&request));
    for _ in 0..20 {
        list(&mut model, Vec::new());
    }
    assert_eq!(model.cards.len(), MAX_CARDS);
    assert!(model.uncertain.as_ref().unwrap().same(&request));
    assert!(model.prepare_review(&available()).is_err());
    model.review_uncertain(&available()).unwrap();
    let replay = model.confirm_request().unwrap();
    assert_eq!(replay.operation, request.operation);
    assert!(model.authorizes(&replay, 1));
    let SourceRecoveryOperation::Prepare { action, .. } = &replay.operation else {
        panic!()
    };
    let receipt = PreparedPerformanceSourceActionReceipt {
        revision: 1,
        action: *action.clone(),
        payload_sha256: test_support::hash(&serde_json::to_vec(action).unwrap()),
    };
    answer(
        &mut model,
        &replay,
        &Reply::PreparedPerformanceSourceAction(Box::new(receipt)),
    );
    assert!(model.uncertain.is_none());
}

#[test]
fn performance_action_source_epoch_raw_bytes_generation_and_hidden_context_never_revive_confirmation()
 {
    let mut model = model();
    list(&mut model, Vec::new());
    model.prepare_review(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    let mut changed = available();
    changed.review.as_mut().unwrap().serial += 1;
    model.sync(&changed);
    model.sync(&available());
    assert!(!model.authorizes(&request, 1));
    model.prepare_review(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    let mut changed = available();
    changed.review.as_mut().unwrap().review.push(b' ');
    model.sync(&changed);
    model.sync(&available());
    assert!(!model.authorizes(&request, 1));
    model.prepare_review(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    let mut forged = request.clone();
    if let SourceRecoveryOperation::Prepare { review, .. } = &mut forged.operation {
        review.push(b' ');
    }
    assert!(!model.authorizes(&forged, 1));
    model.context_changed();
    assert!(!model.authorizes(&request, 1));
    model.prepare_review(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    model.generation_changed(2);
    assert!(!model.authorizes(&request, 2));
    assert!(model.confirmation.is_none() && model.offered.is_none());
}

#[test]
fn performance_action_all_four_cancel_tokens_are_exact_idempotent_and_do_not_release_credit() {
    for mode in 0..4 {
        let mut model = model();
        let dto = if mode < 2 {
            detail(
                Path::new(ORIGIN),
                if mode == 0 {
                    Phase::Prepared
                } else {
                    Phase::Started
                },
            )
        } else {
            pending_choice()
        };
        install(&mut model, dto);
        let request = match mode {
            0 => {
                model.review_execute(&available()).unwrap();
                model.confirm_request().unwrap()
            }
            1 => model.reconcile_request().unwrap(),
            2 => {
                model.review_attach(&available()).unwrap();
                model.confirm_request().unwrap()
            }
            _ => model.reconcile_attachment_request().unwrap(),
        };
        model.submitted(&request, token(&request));
        assert_eq!(model.pending_cancel(), Some(token(&request)));
        model.cancel_requested(
            RequestId {
                generation: 2,
                ..token(&request)
            },
            true,
        );
        assert!(!model.pending.as_ref().unwrap().cancel_attempted);
        model.cancel_requested(token(&request), true);
        model.cancel_requested(token(&request), false);
        assert!(model.pending.as_ref().unwrap().cancel_requested);
        assert!(model.pending.is_some() && !model.authorizes(&request, 1));
        model.completed(
            &request,
            token(&request),
            Err(&ServiceError {
                kind: FailureKind::InspectionCancelled,
                message: "stop observed".into(),
                outcome_unknown: request.is_mutation(),
            }),
        );
        assert!(model.pending.is_none());
        assert!(
            model
                .cards
                .back()
                .unwrap()
                .detail
                .contains("Cancellation was observed")
        );
    }
    let mut model = model();
    install(&mut model, detail(Path::new(ORIGIN), Phase::Verified));
    model
        .review_decision(PerformanceSourceDecision::KeepLocally, &available())
        .unwrap();
    let request = model.confirm_request().unwrap();
    model.submitted(&request, token(&request));
    assert!(model.pending_cancel().is_none());
    model.cancel_requested(token(&request), true);
    assert!(!model.pending.as_ref().unwrap().cancel_attempted);
}

#[test]
fn performance_action_completion_before_cancel_and_long_nested_fatal_keep_commitment_and_kind() {
    let mut model = model();
    let dto = pending_choice();
    install(&mut model, dto.clone());
    model.review_attach(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    model.submitted(&request, token(&request));
    model.cancel_requested(token(&request), true);
    let committed = test_support::committed(&dto, 5);
    let error =
        ServiceError::storage_unavailable("long current document failure ".repeat(2048), true);
    let reply = attachment_reply(&request, &committed, true, Some(error));
    assert!(request.matches_reply(&reply));
    assert!(
        !model.completed(&request, token(&request), Ok(&reply)),
        "retirement cannot authorize document installation"
    );
    let card = model.cards.back().unwrap();
    assert_eq!(card.committed, Some(true));
    assert!(card.choice.is_some());
    assert!(card.detail.contains("nothing was rolled back"));
    let (kind, unknown, text) = card.nested.as_ref().unwrap();
    assert_eq!(*kind, FailureKind::StorageUnavailable);
    assert!(*unknown && text.len() <= MESSAGE_BYTES && text.contains("message shortened"));
    assert_eq!(model.lost_generation, Some(1));
    assert!(model.confirmation.is_none() && model.current_detail().is_none());
}

#[test]
fn performance_action_worker_loss_and_bad_retirement_never_authorize_replacement() {
    let mut model = model();
    let dto = pending_choice();
    install(&mut model, dto.clone());
    model
        .review_decision(PerformanceSourceDecision::RetireAttachment, &available())
        .unwrap();
    let request = model.confirm_request().unwrap();
    model.submitted(&request, token(&request));
    model.worker_lost(1, "injected EOF");
    assert!(model.pending.is_none() && model.uncertain.is_some());
    let count = model.cards.len();
    model.worker_lost(1, "repeat");
    assert_eq!(model.cards.len(), count);
    let mut a = available();
    a.generation = Some(2);
    a.review.as_mut().unwrap().generation = 2;
    model.sync(&a);
    install(&mut model, dto);
    model.set_track_name("Replacement".into());
    model.set_timeline_frame("0".into());
    assert!(model.review_attachment(&a).is_err());
    assert!(model.uncertain.is_some());
    assert!(
        model
            .current_detail()
            .unwrap()
            .dto
            .state
            .decisions
            .attachment
            .as_ref()
            .unwrap()
            .status
            == PerformanceSourceAttachmentStatus::Pending
    );
}

#[test]
fn performance_action_eof_before_started_resolves_only_the_exact_current_prepared_read() {
    let mut model = model();
    let prepared = detail(Path::new(ORIGIN), Phase::Prepared);
    install(&mut model, prepared.clone());
    model.review_execute(&available()).unwrap();
    let lost = model.confirm_request().unwrap();
    model.submitted(&lost, token(&lost));
    model.worker_lost(1, "EOF before the Started acknowledgement");
    assert!(model.uncertain.as_ref().is_some_and(|p| p.same(&lost)));
    assert!(model.pending.is_none());

    let mut availability = available();
    availability.generation = Some(2);
    availability.review.as_mut().unwrap().generation = 2;
    model.sync(&availability);
    assert!(model.offered.is_none() && model.confirmation.is_none());
    assert!(model.review_execute(&availability).is_err());

    let mut other_action = prepared.clone();
    other_action.state.prepared.action.id = "44444444444444444444444444444444".into();
    other_action.state.prepared.action.recovery_operation_id = OperationId("other-copy".into());
    other_action.state.prepared.payload_sha256 =
        test_support::hash(&serde_json::to_vec(&other_action.state.prepared.action).unwrap());
    other_action.state.payload_sha256 = other_action.state.prepared.payload_sha256.clone();
    other_action.validate().unwrap();
    install(&mut model, other_action);
    assert!(model.uncertain.as_ref().is_some_and(|p| p.same(&lost)));

    let mut other_reference = prepared.clone();
    other_reference.state.prepared.action.recovery_operation_id =
        OperationId("changed-prepared-reference".into());
    other_reference.state.prepared.payload_sha256 =
        test_support::hash(&serde_json::to_vec(&other_reference.state.prepared.action).unwrap());
    other_reference.state.payload_sha256 = other_reference.state.prepared.payload_sha256.clone();
    other_reference.validate().unwrap();
    install(&mut model, other_reference);
    assert!(model.uncertain.as_ref().is_some_and(|p| p.same(&lost)));
    model.resolve_uncertain(
        &detail(Path::new("/synthetic/other-studio"), Phase::Prepared),
        false,
    );
    assert!(model.uncertain.as_ref().is_some_and(|p| p.same(&lost)));

    // These are explicit metadata requests in the healthy new generation.
    // Their exact original head resolves the local doubt without artifact work.
    install(&mut model, prepared.clone());
    assert!(model.uncertain.is_none());
    assert!(
        model
            .current_detail()
            .unwrap()
            .dto
            .state
            .execution
            .is_none()
    );
    assert!(
        model
            .cards
            .iter()
            .any(|card| card.title == "Previous worker result is uncertain")
    );
    assert!(model.offered.is_none() && model.confirmation.is_none() && model.pending.is_none());
    assert!(model.proof.is_none() && model.reconcile_request().is_none());
    assert!(!model.authorizes(&lost, 2));

    model.review_execute(&availability).unwrap();
    assert!(!model.confirmation.as_ref().unwrap().approved);
    assert!(model.offered.is_none() && model.pending.is_none());
    let newly_confirmed = model.confirm_request().unwrap();
    assert_eq!(newly_confirmed.reference(), lost.reference());
    assert_eq!(newly_confirmed.generation, 2);
    assert_ne!(newly_confirmed.serial, lost.serial);
    assert!(model.authorizes(&newly_confirmed, 2));
    assert_eq!(
        model.current_detail().unwrap().dto.state.prepared,
        prepared.state.prepared
    );
    assert!(
        model.pending.is_none(),
        "confirmation itself performs no artifact work"
    );
}

#[test]
fn performance_action_oversized_or_malformed_reply_is_not_installed_and_counters_do_not_wrap() {
    let mut model = model();
    let request = model.list_request().unwrap();
    let repeated = vec![detail(Path::new(ORIGIN), Phase::Prepared).state; 33];
    assert!(!request.matches_reply(&Reply::PerformanceSourceActionStates(repeated)));
    install(&mut model, detail(Path::new(ORIGIN), Phase::Verified));
    let request = model.read_request().unwrap();
    model.submitted(&request, token(&request));
    let mut invalid = detail(Path::new(ORIGIN), Phase::Verified);
    invalid.review = vec![b' '; MAX_PERFORMANCE_SOURCE_REVIEW_BYTES + 1];
    assert!(!request.matches_reply(&Reply::PerformanceSourceActionDetail(Box::new(invalid))));
    model.completed(
        &request,
        token(&request),
        Err(&ServiceError {
            kind: FailureKind::Protocol,
            message: "bounded detail refused".into(),
            outcome_unknown: false,
        }),
    );
    assert!(model.current_detail().is_none());
    model.epoch = u64::MAX;
    model.context_changed();
    assert!(model.exhausted && model.list_request().is_none());
}

#[test]
fn performance_action_maximum_raw_review_uses_whole_detail_bound_and_keeps_exact_bytes() {
    let mut dto = detail(Path::new(ORIGIN), Phase::Prepared);
    let mut source: PerformanceSourceReview = serde_json::from_slice(&dto.review).unwrap();
    source.performance.problem = Some("€".repeat(18_000));
    source.review_sha256.clear();
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.performance-source-review.v1\0");
    hash.update(serde_json::to_vec(&source).unwrap());
    source.review_sha256 = format!("{:x}", hash.finalize());
    let mut raw = serde_json::to_vec(&source).unwrap();
    assert!(raw.len() < MAX_PERFORMANCE_SOURCE_REVIEW_BYTES);
    raw.resize(MAX_PERFORMANCE_SOURCE_REVIEW_BYTES, b' ');
    let action = PreparedPerformanceSourceAction::prepare(
        ACTION.into(),
        &raw,
        OperationId("synthetic-copy-operation".into()),
        None,
    )
    .unwrap();
    let payload = test_support::hash(&serde_json::to_vec(&action).unwrap());
    dto.state.prepared.action = action;
    dto.state.prepared.payload_sha256 = payload.clone();
    dto.state.payload_sha256 = payload;
    dto.review = raw.clone();
    dto.validate().unwrap();
    let whole = serde_json::to_vec(&dto).unwrap().len();
    assert!(whole > raw.len() * 3 && whole <= MAX_SOURCE_ACTION_DETAIL_BYTES);
    let mut model = model();
    install(&mut model, dto);
    assert_eq!(model.current_detail().unwrap().dto.review, raw);
    assert!(
        model
            .current_detail()
            .unwrap()
            .facts
            .diagnostics
            .iter()
            .all(|line| line.len() <= MESSAGE_BYTES)
    );
}

fn frame(
    context: &egui::Context,
    model: &mut SourceRecoveryUi,
    availability: &SourceRecoveryAvailability,
    width: f32,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<SourceRecoveryAction>, f32) {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, 450.0),
        )),
        events,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(1.0);
    let mut action = None;
    let mut content_width = 0.;
    let mut output = context.run_ui(input, |ui| {
        action = model.ui(ui, availability);
        content_width = ui.min_rect().width();
    });
    output.textures_delta.clear();
    (output, action, content_width)
}

#[test]
fn performance_action_construction_paint_selection_edit_and_reconnect_emit_no_automatic_command() {
    let context = egui::Context::default();
    let mut model = model();
    assert!(
        frame(&context, &mut model, &available(), 350., Vec::new())
            .1
            .is_none()
    );
    assert!(model.offered.is_none() && model.confirmation.is_none());
    install(&mut model, detail(Path::new(ORIGIN), Phase::Verified));
    model.set_track_name("Edited name".into());
    model.set_timeline_frame("0".into());
    assert!(model.select_action(ACTION));
    assert!(
        frame(&context, &mut model, &available(), 350., Vec::new())
            .1
            .is_none()
    );
    assert!(model.offered.is_none() && model.confirmation.is_none());
    model.generation_changed(2);
    let mut a = available();
    a.generation = Some(2);
    a.review.as_mut().unwrap().generation = 2;
    assert!(
        frame(&context, &mut model, &a, 350., Vec::new())
            .1
            .is_none()
    );
    assert!(
        model.current_detail().is_none() && model.offered.is_none() && model.confirmation.is_none()
    );
}

#[test]
fn performance_action_light_dark_narrow_pending_cancel_stays_above_scrolling_collapsed_history() {
    for dark in [false, true] {
        for width in [240., 480.] {
            let context = egui::Context::default();
            context.enable_accesskit();
            context.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            context.all_styles_mut(|s| s.animation_time = 0.);
            let mut model = model();
            let mut states = Vec::new();
            for n in 1..=32 {
                let mut state = detail(Path::new(ORIGIN), Phase::Prepared).state;
                if n < 32 {
                    state.prepared.action.id = format!("{n:032x}");
                    state.prepared.action.recovery_operation_id =
                        OperationId(format!("synthetic-copy-{n}"));
                    state.prepared.payload_sha256 =
                        test_support::hash(&serde_json::to_vec(&state.prepared.action).unwrap());
                    state.payload_sha256 = state.prepared.payload_sha256.clone();
                }
                states.push(state);
            }
            install(&mut model, detail(Path::new(ORIGIN), Phase::Prepared));
            list(&mut model, states);
            let request = model.reconcile_request();
            assert!(request.is_none());
            model.review_execute(&available()).unwrap();
            let request = model.confirm_request().unwrap();
            model.submitted(&request, token(&request));
            let (output, action, content_width) =
                frame(&context, &mut model, &available(), width, Vec::new());
            assert!(action.is_none());
            assert!(
                content_width <= width + 1.,
                "overflow {content_width} at {width}"
            );
            let tree = output.platform_output.accesskit_update.unwrap();
            let cancel = tree
                .nodes
                .iter()
                .find(|(_, n)| n.label() == Some("Cancel recovery request"))
                .expect("fixed cancel above collapsed details");
            assert!(cancel.1.bounds().is_some_and(|r| r.y1 <= 450.));
            let target = cancel.0;
            let event = egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: tree.tree_id,
                target_node: target,
                data: None,
            });
            let (_, action, _) = frame(&context, &mut model, &available(), width, vec![event]);
            assert!(matches!(action,Some(SourceRecoveryAction::Cancel(id)) if id==token(&request)));
            assert!(model.pending.is_some());
        }
    }
}

#[test]
fn performance_action_completion_authorization_requires_the_whole_current_model_body() {
    for changed_prepared in [false, true] {
        let mut model = model();
        let dto = pending_choice();
        install(&mut model, dto.clone());
        model.review_attach(&available()).unwrap();
        let request = model.confirm_request().unwrap();
        model.submitted(&request, token(&request));
        let mut returned = if changed_prepared {
            test_support::committed(&dto, 5)
        } else {
            dto.clone()
        };
        if changed_prepared {
            returned.state.prepared.action.recovery_operation_id =
                OperationId("different-copy-operation".into());
            returned.state.prepared.payload_sha256 =
                test_support::hash(&serde_json::to_vec(&returned.state.prepared.action).unwrap());
            returned
                .state
                .execution
                .as_mut()
                .unwrap()
                .prepared_payload_sha256 = returned.state.prepared.payload_sha256.clone();
        }
        let reply = attachment_reply(&request, &returned, true, None);
        assert!(
            request.matches_reply(&reply),
            "transport and bounded body checks alone do not prove the pinned model postcondition"
        );
        assert!(!model.completed(&request, token(&request), Ok(&reply)));
        assert!(
            model.pending.is_none() && model.current_detail().is_none() && model.proof.is_none()
        );
        assert!(
            model
                .uncertain
                .as_ref()
                .is_some_and(|pinned| pinned.same(&request))
        );
        assert_eq!(model.cards.back().unwrap().committed, None);
    }
    let mut model = model();
    let dto = pending_choice();
    install(&mut model, dto.clone());
    model.review_attach(&available()).unwrap();
    let request = model.confirm_request().unwrap();
    model.submitted(&request, token(&request));
    let reply = attachment_reply(&request, &test_support::committed(&dto, 5), true, None);
    assert!(!model.completed(
        &request,
        RequestId {
            sequence: token(&request).sequence + 1,
            ..token(&request)
        },
        Ok(&reply)
    ));
    assert!(model.pending.is_some());
    assert!(model.completed(&request, token(&request), Ok(&reply)));
}

#[test]
fn performance_action_collapsed_headers_wrap_complete_text_and_remain_explicitly_expandable() {
    let context = egui::Context::default();
    context.enable_accesskit();
    context.all_styles_mut(|s| s.animation_time = 0.);
    let label = "Retained original and checkpoint details · full identity and bounded inspection resource usage";
    let paint = |events: Vec<egui::Event>| {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(240., 450.),
            )),
            events,
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(1.);
        let mut width = 0.;
        let mut opened = false;
        let mut output = context.run_ui(input, |ui| {
            SourceRecoveryUi::details(ui, label, |ui| {
                opened = true;
                ui.add(egui::Label::new("Explicitly expanded details").wrap());
            });
            width = ui.min_rect().width();
        });
        output.textures_delta.clear();
        (output, width, opened)
    };
    let (output, width, opened) = paint(Vec::new());
    assert!(
        width <= 241. && !opened,
        "wrapped closed header width {width}"
    );
    let tree = output.platform_output.accesskit_update.unwrap();
    let target = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label))
        .expect("full accessible header text")
        .0;
    let event = egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target_tree: tree.tree_id,
        target_node: target,
        data: None,
    });
    let (_, width, opened) = paint(vec![event]);
    assert!(
        width <= 241. && opened,
        "explicit wrapped header expansion width {width}"
    );
}
