//! Synthetic metadata choices; actual recovery fixtures are isolated and local.
#![cfg(unix)]

use super::super::tests::{self as fixture, Mode};
use super::*;
use crate::{Backend, Command, PerformanceSourceIntentReference};
use omniusong_core::{
    contracts::{ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeReference, TimelinePosition},
};
use omniusong_engine::LiveId;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const ACTION: &str = "11111111111111111111111111111111";
const OTHER: &str = "22222222222222222222222222222222";
const CHOICE: &str = "33333333333333333333333333333333";
const NEXT: &str = "44444444444444444444444444444444";

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn recovered(
    mode: Mode,
) -> (
    fixture::Fixture,
    NativeBackend,
    PerformanceSourceReview,
    Vec<u8>,
    PerformanceSourceActionState,
) {
    let (fixture, mut backend, source, raw, prepared) = fixture::setup(mode);
    let state = fixture::execute(&mut backend, &prepared).unwrap().state;
    assert_eq!(state.revision, 3);
    (fixture, backend, source, raw, state)
}
fn choice(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
    source: &PerformanceSourceReview,
    id: &str,
    supersedes: Option<&str>,
) -> Box<PerformanceSourceAttachmentChoice> {
    let target = fixture::projects(backend).remove(0).0;
    let Some(super::super::PerformanceSourceExecutionRevision {
        phase: PerformanceSourceExecutionPhase::Verified { result, .. },
        ..
    }) = &state.execution
    else {
        panic!("Verified")
    };
    Box::new(PerformanceSourceAttachmentChoice {
        id: id.into(),
        supersedes: supersedes.map(String::from),
        publication: TakePublication {
            operation_id: attachment_operation_id(&state.prepared.action.id, id).unwrap(),
            project_id: ProjectId(target.id),
            expected_revision: target.revision,
            track_id: TrackId(format!("source-track-{id}")),
            track_name: "Recovered original".into(),
            clip: TakeClip {
                id: format!("source-clip-{id}"),
                source: TakeReference {
                    take_id: TakeId(result.take_id.clone()),
                    manifest_sha256: result.manifest_sha256.clone(),
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 0,
                    count: source.performance.checkpoint.as_ref().unwrap().audio_end,
                },
                position: TimelinePosition {
                    frame: 128,
                    sample_rate: source.config.info.sample_rate,
                },
                fades: None,
                edit_source: None,
            },
        },
    })
}
fn save(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
    decision: &PerformanceSourceDecision,
) -> Result<PerformanceSourceActionState, ServiceError> {
    let Reply::PerformanceSourceDecision(state) = backend.save_source_decision(
        &state.prepared.action.id,
        state.revision,
        &state.payload_sha256,
        decision,
    )?
    else {
        panic!("decision receipt")
    };
    Ok(state.state)
}
fn checked(
    history: &PerformanceSourceActionHistory,
) -> Result<Vec<PerformanceSourceActionState>, ServiceError> {
    let prepared = history
        .revisions
        .iter()
        .filter(|r| r.revision == 1)
        .map(|r| {
            Ok(super::super::PreparedPerformanceSourceActionReceipt {
                revision: 1,
                action: serde_json::from_slice(&r.payload)
                    .map_err(|e| ServiceError::rejected(e.to_string()))?,
                payload_sha256: r.payload_sha256.clone(),
            })
        })
        .collect::<Result<Vec<_>, ServiceError>>()?;
    super::super::validate_history(history, &prepared)
}
fn appended(
    history: &PerformanceSourceActionHistory,
    state: &PerformanceSourceActionState,
    payload: Vec<u8>,
) -> PerformanceSourceActionHistory {
    let mut proposed = history.clone();
    proposed.revisions.push(StoredPerformanceSourceAction {
        id: state.prepared.action.id.clone(),
        revision: state.revision + 1,
        review_blob_sha256: state.prepared.action.review_blob_sha256.clone(),
        payload_sha256: checksum(&payload),
        payload,
    });
    proposed
        .revisions
        .sort_by(|a, b| a.id.cmp(&b.id).then(a.revision.cmp(&b.revision)));
    proposed
}
type FileTree = BTreeMap<PathBuf, Option<Vec<u8>>>;
fn source_files(fixture: &fixture::Fixture) -> (FileTree, FileTree) {
    (
        fixture::tree(fixture.source_path().parent().unwrap()),
        fixture::tree(&fixture.action_parent()),
    )
}

#[test]
fn operation_framing_and_request_serde_are_exact_and_strict() {
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.source-attachment.v1\0");
    hash.update(32u64.to_le_bytes());
    hash.update(ACTION.as_bytes());
    hash.update(32u64.to_le_bytes());
    hash.update(CHOICE.as_bytes());
    assert_eq!(
        attachment_operation_id(ACTION, CHOICE).unwrap().0,
        format!("os-src-attach:{:x}", hash.finalize())
    );
    assert_ne!(
        attachment_operation_id(ACTION, CHOICE).unwrap(),
        attachment_operation_id(CHOICE, ACTION).unwrap()
    );
    for invalid in [
        "",
        "00000000000000000000000000000000",
        "../bad",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        assert!(attachment_operation_id(invalid, CHOICE).is_err());
        assert!(attachment_operation_id(ACTION, invalid).is_err());
    }
    assert!(
        serde_json::from_str::<PerformanceSourceDecision>(
            r#"{"kind":"keep_locally","extra":true}"#
        )
        .is_err()
    );
    for kind in ["keep_locally", "return_to_review", "retire_attachment"] {
        let json = format!(r#"{{"kind":"{kind}","extra":true}}"#);
        assert!(serde_json::from_str::<PerformanceSourceDecision>(&json).is_err());
    }
    for state in ["kept_locally", "returned_to_review"] {
        let json = format!(r#"{{"state":"{state}","extra":true}}"#);
        assert!(serde_json::from_str::<PerformanceSourceDecisionEvent>(&json).is_err());
    }
    for state in ["pending", "retired"] {
        let json = format!(r#"{{"state":"{state}","extra":true}}"#);
        assert!(serde_json::from_str::<PerformanceSourceAttachmentStatus>(&json).is_err());
    }
    assert!(serde_json::from_str::<PerformanceSourceDecision>(r#"{"kind":"attachment_committed","choice_id":"33333333333333333333333333333333","project_revision":1}"#).is_err());
    assert_eq!(
        serde_json::to_string(&PerformanceSourceDecision::KeepLocally).unwrap(),
        r#"{"kind":"keep_locally"}"#
    );
}

#[test]
fn actual_recovered_fault_tail_finish_decisions_preserve_every_source_and_project_byte() {
    for mode in [Mode::Clean, Mode::Tail, Mode::Fault, Mode::Finish] {
        let (fixture, mut backend, source, raw, original) = recovered(mode);
        let files = source_files(&fixture);
        let projects = fixture::projects(&mut backend);
        let before = fixture::history(&mut backend);
        let kept = save(
            &mut backend,
            &original,
            &PerformanceSourceDecision::KeepLocally,
        )
        .unwrap();
        assert!(kept.decisions.kept_locally);
        let review = save(
            &mut backend,
            &kept,
            &PerformanceSourceDecision::ReturnToReview,
        )
        .unwrap();
        let selected = choice(&mut backend, &review, &source, CHOICE, None);
        let prepared = save(
            &mut backend,
            &review,
            &PerformanceSourceDecision::PrepareAttachment {
                choice: selected.clone(),
            },
        )
        .unwrap();
        let attachment = prepared.decisions.attachment.as_ref().unwrap();
        assert_eq!(attachment.revision, 6);
        assert_eq!(attachment.payload_sha256, prepared.payload_sha256);
        assert_eq!(attachment.choice, selected);
        let retired = save(
            &mut backend,
            &prepared,
            &PerformanceSourceDecision::RetireAttachment,
        )
        .unwrap();
        assert_eq!(retired.decisions.attachment.as_ref().unwrap().revision, 6);
        assert_eq!(
            retired.decisions.attachment.as_ref().unwrap().status,
            PerformanceSourceAttachmentStatus::Retired
        );
        let kept = save(
            &mut backend,
            &retired,
            &PerformanceSourceDecision::KeepLocally,
        )
        .unwrap();
        assert_eq!(kept.execution, original.execution);
        assert_eq!(kept.prepared, original.prepared);
        assert_eq!(files, source_files(&fixture));
        assert_eq!(projects, fixture::projects(&mut backend));
        let after = fixture::history(&mut backend);
        assert_eq!(after.reviews, before.reviews);
        assert_eq!(after.reviews[0].payload, raw);
        assert_eq!(&after.revisions[..3], &before.revisions[..]);
        assert_eq!(checked(&after).unwrap(), [kept]);
    }
}

#[test]
fn metadata_choices_do_not_require_or_recreate_capture_or_artifact_paths() {
    let (fixture, mut backend, source, _, state) = recovered(Mode::Tail);
    let original = fs::read(fixture.source_path()).unwrap();
    let artifact_files = fixture::tree(&fixture.action_parent());
    let captures = fixture.paths().native.join("captures");
    fs::rename(&captures, fixture.root.join("parked-captures")).unwrap();
    fs::rename(
        fixture.action_parent(),
        fixture.root.join("parked-artifacts"),
    )
    .unwrap();
    let kept = save(
        &mut backend,
        &state,
        &PerformanceSourceDecision::KeepLocally,
    )
    .unwrap();
    let state = save(
        &mut backend,
        &kept,
        &PerformanceSourceDecision::ReturnToReview,
    )
    .unwrap();
    let selected = choice(&mut backend, &state, &source, CHOICE, None);
    let prepared = save(
        &mut backend,
        &state,
        &PerformanceSourceDecision::PrepareAttachment { choice: selected },
    )
    .unwrap();
    save(
        &mut backend,
        &prepared,
        &PerformanceSourceDecision::RetireAttachment,
    )
    .unwrap();
    assert!(!captures.exists() && !fixture.action_parent().exists());
    assert_eq!(
        artifact_files,
        fixture::tree(&fixture.root.join("parked-artifacts"))
    );
    assert_eq!(
        original,
        fs::read(
            fixture
                .root
                .join("parked-captures/02020202020202020202020202020202/take.sqlite3")
        )
        .unwrap()
    );
}

#[test]
fn exact_current_replay_survives_lost_ack_but_ancient_same_event_cannot_reapply() {
    let (_fixture, mut backend, _, _, base) = recovered(Mode::Clean);
    let keep = PerformanceSourceDecision::KeepLocally;
    let kept = save(&mut backend, &base, &keep).unwrap();
    assert_eq!(save(&mut backend, &base, &keep).unwrap(), kept);
    assert_eq!(save(&mut backend, &kept, &keep).unwrap(), kept);
    let returned = save(
        &mut backend,
        &kept,
        &PerformanceSourceDecision::ReturnToReview,
    )
    .unwrap();
    let later = save(&mut backend, &returned, &keep).unwrap();
    let before = fixture::history(&mut backend);
    assert!(
        save(&mut backend, &base, &keep).is_err(),
        "same event bytes must not authorize ancient Keep"
    );
    assert!(
        save(
            &mut backend,
            &kept,
            &PerformanceSourceDecision::ReturnToReview
        )
        .is_err()
    );
    assert_eq!(save(&mut backend, &returned, &keep).unwrap(), later);
    assert_eq!(save(&mut backend, &later, &keep).unwrap(), later);
    assert!(
        backend
            .save_source_decision(ACTION, later.revision, &"a".repeat(64), &keep)
            .is_err()
    );
    assert_eq!(fixture::history(&mut backend), before);
}

#[test]
fn pending_choice_is_immutable_and_replacement_requires_exact_retired_predecessor() {
    let (fixture, mut backend, source, _, base) = recovered(Mode::Clean);
    let first = choice(&mut backend, &base, &source, CHOICE, None);
    let pending = save(
        &mut backend,
        &base,
        &PerformanceSourceDecision::PrepareAttachment {
            choice: first.clone(),
        },
    )
    .unwrap();
    let mut second = choice(&mut backend, &pending, &source, NEXT, Some(CHOICE));
    assert!(
        save(
            &mut backend,
            &pending,
            &PerformanceSourceDecision::PrepareAttachment {
                choice: second.clone()
            }
        )
        .is_err()
    );
    let kept = save(
        &mut backend,
        &pending,
        &PerformanceSourceDecision::KeepLocally,
    )
    .unwrap();
    let retired = save(
        &mut backend,
        &kept,
        &PerformanceSourceDecision::RetireAttachment,
    )
    .unwrap();
    assert!(retired.decisions.kept_locally);
    assert!(
        save(
            &mut backend,
            &retired,
            &PerformanceSourceDecision::PrepareAttachment {
                choice: second.clone()
            }
        )
        .is_err()
    );
    let review = save(
        &mut backend,
        &retired,
        &PerformanceSourceDecision::ReturnToReview,
    )
    .unwrap();
    second.supersedes = Some(OTHER.into());
    assert!(
        save(
            &mut backend,
            &review,
            &PerformanceSourceDecision::PrepareAttachment {
                choice: second.clone()
            }
        )
        .is_err()
    );
    second.supersedes = Some(CHOICE.into());
    second.publication.clip.id = first.publication.clip.id;
    assert!(
        save(
            &mut backend,
            &review,
            &PerformanceSourceDecision::PrepareAttachment {
                choice: second.clone()
            }
        )
        .is_err()
    );
    second.publication.clip.id = "explicit-new-clip".into();
    let replacement = save(
        &mut backend,
        &review,
        &PerformanceSourceDecision::PrepareAttachment {
            choice: second.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        replacement.decisions.attachment.as_ref().unwrap().choice,
        second
    );
    assert!(fixture.action_parent().join(ACTION).exists());
}

#[test]
fn caller_forged_publication_fields_and_oversized_requests_leave_history_unchanged() {
    let (fixture, mut backend, source, _, state) = recovered(Mode::Clean);
    let valid = choice(&mut backend, &state, &source, CHOICE, None);
    let before = fixture::history(&mut backend);
    let files = source_files(&fixture);
    for case in 0..16 {
        let mut invalid = valid.clone();
        match case {
            0 => invalid.id = "0".repeat(32),
            1 => invalid.supersedes = Some(OTHER.into()),
            2 => invalid.publication.operation_id = OperationId("ordinary-operation".into()),
            3 => invalid.publication.operation_id = attachment_operation_id(OTHER, CHOICE).unwrap(),
            4 => invalid.publication.clip.source.manifest_sha256 = "a".repeat(64),
            5 => invalid.publication.clip.source.take_id = TakeId("another-original".into()),
            6 => invalid.publication.clip.source.stream_id = "gui-1".into(),
            7 => invalid.publication.clip.range = TakeRange::AudioFrames { start: 1, count: 1 },
            8 => invalid.publication.clip.range = TakeRange::AudioFrames { start: 0, count: 0 },
            9 => invalid.publication.clip.position.sample_rate = 48_000,
            10 => invalid.publication.clip.position.frame = -1,
            11 => {
                invalid.publication.clip.edit_source =
                    Some(serde_json::json!({"claimed":"snapshot"}))
            }
            12 => invalid.publication.track_name = "x".repeat(161),
            13 => invalid.publication.project_id = ProjectId(String::new()),
            14 => {
                invalid.publication.clip.edit_source =
                    Some(serde_json::json!({"huge":"x".repeat(16384)}))
            }
            _ => {
                invalid.publication.clip.fades = Some(omniusong_core::native::AudioFades {
                    in_frames: 0,
                    out_frames: 0,
                    curve: omniusong_core::native::FadeCurve::Linear,
                })
            }
        }
        assert!(
            save(
                &mut backend,
                &state,
                &PerformanceSourceDecision::PrepareAttachment { choice: invalid }
            )
            .is_err(),
            "forged case {case}"
        );
        assert_eq!(fixture::history(&mut backend), before);
    }
    assert_eq!(files, source_files(&fixture));
}

#[test]
fn malformed_envelopes_and_impossible_historical_transitions_are_rejected() {
    let (fixture, mut backend, _, _, state) = recovered(Mode::Clean);
    let history = fixture::history(&mut backend);
    let valid = envelope(&state, PerformanceSourceDecisionEvent::KeptLocally).unwrap();
    for case in 0..9 {
        let mut invalid = valid.clone();
        match case {
            0 => invalid.version = 2,
            1 => invalid.id = OTHER.into(),
            2 => invalid.prepared_payload_sha256 = "a".repeat(64),
            3 => invalid.verified_payload_sha256 = state.prepared.payload_sha256.clone(),
            5 => invalid.decision = PerformanceSourceDecisionEvent::ReturnedToReview,
            6 => {
                invalid.decision = PerformanceSourceDecisionEvent::AttachmentRetired {
                    choice_id: CHOICE.into(),
                }
            }
            7 => {
                invalid.decision = PerformanceSourceDecisionEvent::AttachmentCommitted {
                    choice_id: CHOICE.into(),
                    project_revision: 1,
                }
            }
            _ => (),
        }
        let payload = if case == 4 {
            let mut value = serde_json::to_value(&valid).unwrap();
            value["extra"] = true.into();
            assert!(
                serde_json::from_value::<PerformanceSourceDecisionRevision>(value.clone()).is_err()
            );
            serde_json::to_vec(&value).unwrap()
        } else if case == 8 {
            serde_json::to_vec_pretty(&valid).unwrap()
        } else {
            serde_json::to_vec(&invalid).unwrap()
        };
        assert!(
            checked(&appended(&history, &state, payload)).is_err(),
            "malformed case {case}"
        );
    }
    let kept = save(
        &mut backend,
        &state,
        &PerformanceSourceDecision::KeepLocally,
    )
    .unwrap();
    let kept_history = fixture::history(&mut backend);
    assert!(
        checked(&appended(
            &kept_history,
            &kept,
            serde_json::to_vec(&valid).unwrap()
        ))
        .is_err()
    );
    assert_eq!(
        fixture.source_path().metadata().unwrap().len(),
        state
            .execution
            .as_ref()
            .map(|e| match &e.phase {
                PerformanceSourceExecutionPhase::Verified { result, .. } => result.evidence_bytes,
                _ => 0,
            })
            .unwrap()
    );
}

#[test]
fn capacity_reserves_pending_settlement_and_full_retirement_replay_does_not_append() {
    let (_fixture, mut backend, source, _, mut state) = recovered(Mode::Clean);
    for _ in 0..10 {
        let decision = if state.decisions.kept_locally {
            PerformanceSourceDecision::ReturnToReview
        } else {
            PerformanceSourceDecision::KeepLocally
        };
        state = save(&mut backend, &state, &decision).unwrap();
    }
    assert_eq!(state.revision, 13);
    let selected = choice(&mut backend, &state, &source, CHOICE, None);
    let prepared = save(
        &mut backend,
        &state,
        &PerformanceSourceDecision::PrepareAttachment { choice: selected },
    )
    .unwrap();
    assert_eq!(prepared.revision, 14);
    let kept = save(
        &mut backend,
        &prepared,
        &PerformanceSourceDecision::KeepLocally,
    )
    .unwrap();
    assert_eq!(kept.revision, 15);
    assert!(
        save(
            &mut backend,
            &kept,
            &PerformanceSourceDecision::ReturnToReview
        )
        .is_err(),
        "pending choice needs its final settlement row"
    );
    let retired = save(
        &mut backend,
        &kept,
        &PerformanceSourceDecision::RetireAttachment,
    )
    .unwrap();
    assert_eq!(retired.revision, 16);
    let before = fixture::history(&mut backend);
    assert_eq!(
        save(
            &mut backend,
            &kept,
            &PerformanceSourceDecision::RetireAttachment
        )
        .unwrap(),
        retired
    );
    assert_eq!(
        save(
            &mut backend,
            &retired,
            &PerformanceSourceDecision::RetireAttachment
        )
        .unwrap(),
        retired
    );
    assert!(
        save(
            &mut backend,
            &retired,
            &PerformanceSourceDecision::ReturnToReview
        )
        .is_err()
    );
    assert_eq!(fixture::history(&mut backend), before);
}

#[test]
fn committed_payload_binds_pending_choice_project_revision_and_original_prepared_reference() {
    let (_fixture, mut backend, source, _, base) = recovered(Mode::Finish);
    assert!(committed_payload(&base, 1).is_err());
    let selected = choice(&mut backend, &base, &source, CHOICE, None);
    let pending = save(
        &mut backend,
        &base,
        &PerformanceSourceDecision::PrepareAttachment { choice: selected },
    )
    .unwrap();
    assert!(committed_payload(&pending, 2).is_err());
    let payload = committed_payload(&pending, 1).unwrap();
    let revision: PerformanceSourceDecisionRevision = serde_json::from_slice(&payload).unwrap();
    assert_eq!(revision.verified_payload_sha256, base.payload_sha256);
    let candidate = appended(&fixture::history(&mut backend), &pending, payload);
    let committed = checked(&candidate).unwrap().remove(0);
    assert_eq!(
        committed.decisions.attachment.as_ref().unwrap().revision,
        pending.revision
    );
    assert_eq!(
        committed
            .decisions
            .attachment
            .as_ref()
            .unwrap()
            .payload_sha256,
        pending.payload_sha256
    );
    assert_eq!(
        committed.decisions.attachment.as_ref().unwrap().status,
        PerformanceSourceAttachmentStatus::Committed {
            project_revision: 1
        }
    );
    assert!(committed_payload(&committed, 1).is_err());
    let retired = envelope(
        &committed,
        PerformanceSourceDecisionEvent::AttachmentRetired {
            choice_id: CHOICE.into(),
        },
    )
    .unwrap();
    assert!(
        checked(&appended(
            &candidate,
            &committed,
            serde_json::to_vec(&retired).unwrap()
        ))
        .is_err()
    );
    let kept = envelope(&committed, PerformanceSourceDecisionEvent::KeptLocally).unwrap();
    let kept = checked(&appended(
        &candidate,
        &committed,
        serde_json::to_vec(&kept).unwrap(),
    ))
    .unwrap()
    .remove(0);
    assert_eq!(kept.decisions.attachment, committed.decisions.attachment);
    assert_eq!(kept.execution, base.execution);
}

#[test]
fn whole_history_origin_and_copy_operation_conflicts_refuse_unrelated_decisions() {
    for forged_origin in [false, true] {
        let (fixture, mut backend, source, raw, base) = recovered(Mode::Clean);
        let selected = choice(&mut backend, &base, &source, CHOICE, None);
        let other = super::super::super::PreparedPerformanceSourceAction::prepare(
            OTHER.into(),
            &raw,
            if forged_origin {
                OperationId("different-recovery-operation".into())
            } else {
                selected.publication.operation_id.clone()
            },
            forged_origin.then(|| PerformanceSourceIntentReference {
                id: LiveId([77; 16]),
                revision: 1,
                payload_sha256: "a".repeat(64),
            }),
        )
        .unwrap();
        backend
            .store()
            .unwrap()
            .save_performance_source_action(OTHER, None, &raw, &serde_json::to_vec(&other).unwrap())
            .unwrap();
        let before = fixture::history(&mut backend);
        assert!(
            save(
                &mut backend,
                &base,
                &PerformanceSourceDecision::PrepareAttachment { choice: selected }
            )
            .is_err()
        );
        assert_eq!(fixture::history(&mut backend), before);
        assert!(fixture.source_path().exists());
    }
}

#[test]
fn actual_legacy_v1_recovery_supports_the_same_metadata_policy_without_finish_authority() {
    use omniusong_core::contracts::{ClockEpochId, EndpointId};
    use omniusong_engine::{
        LiveClock, LiveScope, LiveSourceBinding, LiveSourceKind,
        performance_records::{PerformanceConfig, PerformanceSource},
        performance_tap::TapInfo,
        performance_writer::PerformanceWriter,
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/performance-source-decision-v1-tests")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root).unwrap();
    let fixture = fixture::Fixture {
        root: fs::canonicalize(root).unwrap(),
    };
    let mut backend = fixture.backend();
    backend
        .handle(&Command::Create {
            name: "Unchanged v1 target".into(),
        })
        .unwrap();
    let path = fixture.source_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let config = PerformanceConfig {
        take_id: TakeId("performance-02020202020202020202020202020202".into()),
        info: TapInfo {
            scope: LiveScope {
                worker_generation: LiveId([1; 16]),
                session_id: LiveId([2; 16]),
                panic_generation: 1,
            },
            sample_rate: 8000,
            channels: 1,
        },
        gui_clock: LiveClock {
            domain: "decision-v1.gui".into(),
            epoch: "decision-v1.epoch".into(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 1,
        },
        output_epoch: ClockEpochId("decision-v1.output".into()),
        sources: std::array::from_fn(|index| PerformanceSource {
            binding: LiveSourceBinding {
                kind: [
                    LiveSourceKind::ComputerKeys,
                    LiveSourceKind::PointerKeys,
                    LiveSourceKind::Pads,
                    LiveSourceKind::AccessiblePulse,
                ][index],
                handle: index as u8 + 1,
                connection_generation: 1,
            },
            endpoint: EndpointId(format!("decision-v1.source-{index}")),
        }),
    };
    let mut writer = PerformanceWriter::create(&path, config.clone()).unwrap();
    let (admission, mut renderer, mut owner) =
        omniusong_engine::live::recording_live_pair(config.info.scope, 8000, 1).unwrap();
    renderer.render_into(&mut [0.; 64]);
    while let Some(packet) = owner.try_pop().unwrap() {
        writer.push_packet(packet).unwrap();
    }
    writer.flush().unwrap();
    drop((admission, renderer, owner));
    writer.close_interrupted().unwrap();
    let Reply::PerformanceSourceInspected(source) = backend
        .handle(&Command::InspectPerformanceSource {
            stage_id: "02020202020202020202020202020202".into(),
        })
        .unwrap()
    else {
        panic!("v1 source")
    };
    assert_eq!(source.profile_version, 1);
    assert!(source.performance.finish_cut.is_none());
    let raw = serde_json::to_vec(&source).unwrap();
    let action = super::super::super::PreparedPerformanceSourceAction::prepare(
        ACTION.into(),
        &raw,
        OperationId("decision-v1.recovery".into()),
        None,
    )
    .unwrap();
    let Reply::PreparedPerformanceSourceAction(prepared) = backend
        .save_prepared_source_action(None, &action, &raw)
        .unwrap()
    else {
        panic!("Prepared")
    };
    let state = fixture::execute(&mut backend, &prepared).unwrap().state;
    let originals = source_files(&fixture);
    let selected = choice(&mut backend, &state, &source, CHOICE, None);
    let pending = save(
        &mut backend,
        &state,
        &PerformanceSourceDecision::PrepareAttachment { choice: selected },
    )
    .unwrap();
    let retired = save(
        &mut backend,
        &pending,
        &PerformanceSourceDecision::RetireAttachment,
    )
    .unwrap();
    assert_eq!(retired.execution, state.execution);
    assert_eq!(originals, source_files(&fixture));
    assert_eq!(fixture::history(&mut backend).reviews[0].payload, raw);
}
