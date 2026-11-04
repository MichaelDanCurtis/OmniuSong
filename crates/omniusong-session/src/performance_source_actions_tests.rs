//! Synthetic storage/service checks only. No native inputs, executor or recovery.
use super::{
    PerformanceSourceIntentReference, PreparedPerformanceSourceAction,
    PreparedPerformanceSourceActionReceipt,
};
use crate::{
    Backend, Command, CommandKind, Completion, FailureKind, LegacyScan, NativeBackend, Paths,
    PerformanceIntent, PerformanceIntentReceipt, PerformanceSourceReview, Reply, RequestId,
    ServiceError, SessionService, SubmitFailure,
};
use omniusong_core::{
    ProjectDocument,
    contracts::{ClockEpochId, EndpointId, OperationId, ProjectId, TakeId},
};
use omniusong_engine::{
    LiveClock, LiveId, LiveScope, LiveSourceBinding, LiveSourceKind, PerformanceStart,
    performance_records::{LocalRecord, PerformanceConfig, PerformanceRecord, PerformanceSource},
    performance_tap::TapInfo,
    performance_writer::PerformanceWriter,
};
use omniusong_recording::TakeState;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const STAGE: &str = "02020202020202020202020202020202";
const ACTION: &str = "11111111111111111111111111111111";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-source-action-session-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn paths(&self) -> Paths {
        Paths {
            native: self.0.join("native"),
            legacy: self.0.join("legacy"),
        }
    }
    fn backend(&self) -> NativeBackend {
        let mut backend = NativeBackend {
            paths: self.paths(),
            store: None,
            legacy: LegacyScan::default(),
            legacy_notice: None,
            require_existing: false,
        };
        backend.handle(&Command::Initialize).unwrap();
        backend
    }
    fn source(&self) {
        let directory = self.paths().native.join("captures").join(STAGE);
        fs::create_dir_all(&directory).unwrap();
        let config = PerformanceConfig {
            take_id: TakeId(format!("performance-{STAGE}")),
            info: TapInfo {
                scope: LiveScope {
                    worker_generation: LiveId([1; 16]),
                    session_id: LiveId([2; 16]),
                    panic_generation: 1,
                },
                sample_rate: 48_000,
                channels: 1,
            },
            gui_clock: LiveClock {
                domain: "synthetic-gui".into(),
                epoch: "synthetic-epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            output_epoch: ClockEpochId("synthetic-output".into()),
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
                endpoint: EndpointId(format!("synthetic-source-{index}")),
            }),
        };
        let mut writer =
            PerformanceWriter::create_finishable(&directory.join("take.sqlite3"), config.clone())
                .unwrap();
        let (_admission, mut renderer, mut owner) =
            omniusong_engine::live::recording_live_pair(config.info.scope, 48_000, 1).unwrap();
        renderer.render_into(&mut [0.; 128]);
        while let Some(packet) = owner.try_pop().unwrap() {
            writer.push_packet(packet).unwrap();
        }
        writer.flush().unwrap();
        drop((renderer, owner, writer));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Independent implementation of the contract's preexisting DTO hash algorithm.
// The raw blob hash below deliberately hashes the unmodified original bytes.
fn retained_bytes(mut review: PerformanceSourceReview) -> Vec<u8> {
    review.review_sha256.clear();
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.performance-source-review.v1\0");
    hash.update(serde_json::to_vec(&review).unwrap());
    review.review_sha256 = format!("{:x}", hash.finalize());
    serde_json::to_vec(&review).unwrap()
}

fn setup() -> (Fixture, NativeBackend, PerformanceSourceReview, Vec<u8>) {
    let fixture = Fixture::new();
    let mut backend = fixture.backend();
    fixture.source();
    let Reply::PerformanceSourceInspected(review) = backend
        .handle(&Command::InspectPerformanceSource {
            stage_id: STAGE.into(),
        })
        .unwrap()
    else {
        panic!("bounded source review")
    };
    assert!(review.performance.checkpoint.as_ref().unwrap().audio_end > 0);
    assert!(review.manifest_sha256.is_none());
    let raw = serde_json::to_vec(&review).unwrap();
    assert_eq!(
        retained_bytes((*review).clone()),
        raw,
        "independent DTO hash oracle"
    );
    (fixture, backend, *review, raw)
}

fn prepare(id: &str, bytes: &[u8], operation: &str) -> PreparedPerformanceSourceAction {
    PreparedPerformanceSourceAction::prepare(id.into(), bytes, OperationId(operation.into()), None)
        .unwrap()
}
fn command(action: &PreparedPerformanceSourceAction, raw: &[u8], expected: Option<u64>) -> Command {
    Command::SavePreparedPerformanceSourceAction {
        expected_revision: expected,
        action: Box::new(action.clone()),
        review: raw.to_vec(),
    }
}
fn saved(
    backend: &mut NativeBackend,
    action: &PreparedPerformanceSourceAction,
    raw: &[u8],
    expected: Option<u64>,
) -> PreparedPerformanceSourceActionReceipt {
    let Reply::PreparedPerformanceSourceAction(receipt) =
        backend.handle(&command(action, raw, expected)).unwrap()
    else {
        panic!("Prepared receipt")
    };
    *receipt
}
fn listed(backend: &mut NativeBackend) -> Vec<PreparedPerformanceSourceActionReceipt> {
    let Reply::PreparedPerformanceSourceActions(receipts) = backend
        .handle(&Command::ListPreparedPerformanceSourceActions)
        .unwrap()
    else {
        panic!("Prepared listing")
    };
    receipts
}

// Complete names and bytes outside SQLite's mutable catalog and its own
// journals. A created source-actions/ directory or recovery artifact is visible.
fn source_snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.parent() == Some(root)
                && [
                    "studio.sqlite3",
                    "studio.sqlite3-journal",
                    "studio.sqlite3-wal",
                    "studio.sqlite3-shm",
                ]
                .iter()
                .any(|name| path.file_name().unwrap() == *name)
            {
                continue;
            }
            if path.is_dir() {
                output.insert(relative, None);
                visit(root, &path, output);
            } else {
                output.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

#[test]
fn actual_bounded_review_prepared_save_replay_list_and_reopen_preserve_every_source_byte() {
    let (fixture, mut backend, review, raw) = setup();
    let root = fixture.paths().native;
    let before = source_snapshot(&root);
    let action = prepare(ACTION, &raw, "synthetic-recover-copy-1");
    assert_eq!(action.review_blob_sha256, sha(&raw));
    assert_ne!(action.review_blob_sha256, review.review_sha256);
    let first = saved(&mut backend, &action, &raw, None);
    assert_eq!(first.revision, 1);
    assert_eq!(
        first.payload_sha256,
        sha(&serde_json::to_vec(&action).unwrap())
    );
    for expected in [None, Some(0), Some(999)] {
        assert_eq!(saved(&mut backend, &action, &raw, expected), first);
    }
    assert_eq!(listed(&mut backend), vec![first.clone()]);
    let history = backend
        .store()
        .unwrap()
        .list_performance_source_actions()
        .unwrap();
    assert_eq!(history.reviews.len(), 1);
    assert_eq!(history.reviews[0].payload, raw);
    assert_eq!(history.reviews[0].payload_sha256, sha(&raw));
    assert_eq!(history.revisions.len(), 1);
    assert!(serde_json::to_vec(&listed(&mut backend)).unwrap().len() < raw.len());
    drop(backend);
    let mut reopened = fixture.backend();
    assert_eq!(listed(&mut reopened), vec![first.clone()]);
    assert_eq!(
        reopened
            .store()
            .unwrap()
            .list_performance_source_actions()
            .unwrap(),
        history
    );
    assert_eq!(source_snapshot(&root), before);
    assert!(!root.join("source-actions").exists());
    // Retained metadata must not become a fresh source-open operation or a
    // claim of present availability. Park only this synthetic source directory.
    fs::rename(
        root.join("captures").join(STAGE),
        fixture.0.join("parked-synthetic-source"),
    )
    .unwrap();
    let before = source_snapshot(&root);
    assert_eq!(saved(&mut reopened, &action, &raw, None), first);
    assert_eq!(listed(&mut reopened).len(), 1);
    assert_eq!(source_snapshot(&root), before);
    assert!(!root.join("source-actions").exists());
}

#[test]
fn usage_changes_exact_blob_and_dto_hash_but_not_admission_and_cannot_replace_retained_bytes() {
    let (fixture, mut backend, mut review, raw) = setup();
    let before = source_snapshot(&fixture.paths().native);
    let first = prepare(ACTION, &raw, "synthetic-usage-1");
    review.usage.vm_steps += 1;
    let changed = retained_bytes(review);
    let changed_review: PerformanceSourceReview = serde_json::from_slice(&changed).unwrap();
    let original_review: PerformanceSourceReview = serde_json::from_slice(&raw).unwrap();
    assert_ne!(changed_review.review_sha256, original_review.review_sha256);
    let second = prepare(
        "22222222222222222222222222222222",
        &changed,
        "synthetic-usage-2",
    );
    assert_eq!(first.admission_sha256, second.admission_sha256);
    assert_ne!(first.review_blob_sha256, second.review_blob_sha256);
    saved(&mut backend, &first, &raw, None);
    let wrong_replay = prepare(ACTION, &changed, "synthetic-usage-1");
    assert!(
        backend
            .handle(&command(&wrong_replay, &changed, None))
            .is_err()
    );
    saved(&mut backend, &second, &changed, None);
    assert_eq!(listed(&mut backend).len(), 2);
    let history = backend
        .store()
        .unwrap()
        .list_performance_source_actions()
        .unwrap();
    assert_eq!(
        history
            .reviews
            .iter()
            .map(|row| row.payload.clone())
            .collect::<Vec<_>>(),
        vec![raw, changed]
    );
    assert_eq!(source_snapshot(&fixture.paths().native), before);
}

#[test]
fn source_semantics_are_bound_separately_from_diagnostic_usage() {
    let (_fixture, _backend, original, raw) = setup();
    let baseline = prepare(ACTION, &raw, "synthetic-semantic");
    let mut variations = Vec::new();
    let mut changed = original.clone();
    changed.byte_length += 1;
    variations.push(("file length", changed));
    let mut changed = original.clone();
    changed.verified_payload_bytes += 1;
    variations.push(("verified bytes", changed));
    let mut changed = original.clone();
    changed.state = if original.state == TakeState::Recording {
        TakeState::Interrupted
    } else {
        TakeState::Recording
    };
    variations.push(("state", changed));
    let mut changed = original.clone();
    changed.config.output_epoch.0.push_str("-changed");
    variations.push(("configuration", changed));
    let mut changed = original.clone();
    changed.performance.descriptor_sha256 = "e".repeat(64);
    variations.push(("descriptor", changed));
    let mut changed = original.clone();
    changed.performance.original_rows_sha256 = "e".repeat(64);
    variations.push(("all original rows", changed));
    let mut changed = original.clone();
    changed.performance.journal_sha256_at_review = "e".repeat(64);
    variations.push(("physical source", changed));
    let mut changed = original.clone();
    changed.performance.capture_fault_pending = !changed.performance.capture_fault_pending;
    variations.push(("fault evidence", changed));
    let mut changed = original.clone();
    changed.performance.problem = Some("synthetic diagnostic semantic problem".into());
    variations.push(("semantic problem", changed));
    let mut changed = original;
    changed
        .performance
        .checkpoint
        .as_mut()
        .unwrap()
        .last_callback_index += 1;
    variations.push(("complete checkpoint", changed));
    for (field, review) in variations {
        let action = prepare(ACTION, &retained_bytes(review), "synthetic-semantic");
        assert_ne!(
            action.admission_sha256, baseline.admission_sha256,
            "unbound {field}"
        );
    }
}

#[test]
fn whitespace_and_escape_distinct_review_is_retained_without_canonicalizing_an_observation() {
    let (fixture, mut backend, review, canonical) = setup();
    let mut pretty = serde_json::to_string_pretty(&review)
        .unwrap()
        .replace("synthetic-gui", r"synthetic\u002dgui")
        .into_bytes();
    pretty.resize(65_536, b' ');
    assert_ne!(pretty, canonical);
    assert_eq!(
        serde_json::from_slice::<PerformanceSourceReview>(&pretty).unwrap(),
        review
    );
    let action = prepare(ACTION, &pretty, "synthetic-exact-raw");
    let canonical_action = prepare(ACTION, &canonical, "synthetic-exact-raw");
    assert_eq!(action.admission_sha256, canonical_action.admission_sha256);
    assert_eq!(action.review_blob_sha256, sha(&pretty));
    assert_ne!(action.review_blob_sha256, sha(&canonical));
    let receipt = saved(&mut backend, &action, &pretty, None);
    assert!(
        backend
            .handle(&command(&canonical_action, &canonical, Some(1)))
            .is_err()
    );
    drop(backend);
    let mut reopened = fixture.backend();
    assert_eq!(listed(&mut reopened), vec![receipt]);
    assert_eq!(
        reopened
            .store()
            .unwrap()
            .list_performance_source_actions()
            .unwrap()
            .reviews[0]
            .payload,
        pretty
    );
}

#[test]
fn forged_digest_root_manifest_checkpoint_version_policy_and_origin_shapes_refuse_without_writes() {
    let (fixture, mut backend, original, raw) = setup();
    let before = source_snapshot(&fixture.paths().native);
    let valid = prepare(ACTION, &raw, "synthetic-rejections");
    let mut bad = original.clone();
    bad.review_sha256 = "0".repeat(64);
    assert!(
        PreparedPerformanceSourceAction::prepare(
            ACTION.into(),
            &serde_json::to_vec(&bad).unwrap(),
            valid.recovery_operation_id.clone(),
            None
        )
        .is_err()
    );
    let mut bad = original.clone();
    bad.version = 2;
    assert!(
        PreparedPerformanceSourceAction::prepare(
            ACTION.into(),
            &retained_bytes(bad),
            valid.recovery_operation_id.clone(),
            None
        )
        .is_err()
    );
    let mut bad = original.clone();
    bad.state = TakeState::Sealed;
    bad.manifest_sha256 = Some("f".repeat(64));
    assert!(
        PreparedPerformanceSourceAction::prepare(
            ACTION.into(),
            &retained_bytes(bad),
            valid.recovery_operation_id.clone(),
            None
        )
        .is_err()
    );
    let mut bad = original.clone();
    bad.performance.selection = None;
    assert!(
        PreparedPerformanceSourceAction::prepare(
            ACTION.into(),
            &retained_bytes(bad),
            valid.recovery_operation_id.clone(),
            None
        )
        .is_err()
    );
    let mut bad = original.clone();
    bad.performance.checkpoint.as_mut().unwrap().audio_end = 0;
    assert!(
        PreparedPerformanceSourceAction::prepare(
            ACTION.into(),
            &retained_bytes(bad),
            valid.recovery_operation_id.clone(),
            None
        )
        .is_err()
    );
    let shaped = serde_json::to_value(&original).unwrap();
    // Every forged DTO below carries a freshly recomputed, independently
    // calculated review digest. Failure must come from shape/policy validation.
    for (case, pointer, value) in [
        ("tail authority", "/performance/unknown_tail", json!(false)),
        ("zero file", "/byte_length", json!(0)),
        ("oversize file", "/byte_length", json!(536_870_913u64)),
        ("file usage", "/usage/file_bytes", json!(536_870_913u64)),
        ("row usage", "/usage/physical_rows", json!(262_145u64)),
        ("work usage", "/usage/work_bytes", json!(1_073_741_825u64)),
        ("row visits", "/usage/row_visits", json!(4_194_305u64)),
        ("chunk visits", "/usage/chunk_visits", json!(524_289u64)),
        ("record visits", "/usage/record_visits", json!(2_097_153u64)),
        ("group usage", "/usage/groups", json!(65_537u64)),
        ("VM usage", "/usage/vm_steps", json!(32_000_001u64)),
        ("empty members", "/performance/checkpoint/chunks", json!([])),
        (
            "zero group",
            "/performance/checkpoint/group_sequence",
            json!(0),
        ),
        (
            "wrong prefix unit",
            "/performance/checkpoint/prefixes/0/source_end/unit",
            json!("events"),
        ),
        (
            "invalid worker",
            "/performance/checkpoint/scope_after/worker_generation",
            json!(vec![0u8; 16]),
        ),
        (
            "other worker",
            "/performance/checkpoint/scope_after/worker_generation",
            json!(vec![3u8; 16]),
        ),
        (
            "other session",
            "/performance/checkpoint/scope_after/session_id",
            json!(vec![3u8; 16]),
        ),
        (
            "old Panic",
            "/performance/checkpoint/scope_after/panic_generation",
            json!(0),
        ),
    ] {
        let mut forged = shaped.clone();
        *forged.pointer_mut(pointer).unwrap() = value;
        let forged = retained_bytes(serde_json::from_value(forged).unwrap());
        assert!(
            PreparedPerformanceSourceAction::prepare(
                ACTION.into(),
                &forged,
                valid.recovery_operation_id.clone(),
                None
            )
            .is_err(),
            "accepted forged {case}"
        );
    }
    let mut invalid_finish = shaped.clone();
    invalid_finish["performance"]["finish_cut"] = json!({
        "fence":{"id":vec![3u8;16],"scope":original.config.info.scope,"accepted_sequence":1},
        "callback_index":0,"cut_frame":128,"voices_cut":0,"release_tails_cut":0,
        "policy":"cut_at_complete_callback_end"
    });
    assert!(
        PreparedPerformanceSourceAction::prepare(
            ACTION.into(),
            &retained_bytes(serde_json::from_value(invalid_finish).unwrap()),
            valid.recovery_operation_id.clone(),
            None
        )
        .is_err()
    );
    for mismatch in ["worker", "session", "older Panic"] {
        let mut forged = shaped.clone();
        let mut scope = original.config.info.scope;
        match mismatch {
            "worker" => scope.worker_generation = LiveId([3; 16]),
            "session" => scope.session_id = LiveId([3; 16]),
            _ => {
                // Both scopes are intrinsically valid; only their temporal
                // relationship contradicts this retained initial scope.
                forged["config"]["info"]["scope"]["panic_generation"] = json!(2);
                forged["performance"]["checkpoint"]["scope_after"]["panic_generation"] = json!(2);
            }
        }
        forged["performance"]["finish_cut"] = json!({
            "fence":{"id":vec![3u8;16],"scope":scope,"accepted_sequence":null},
            "callback_index":1,"cut_frame":128,"voices_cut":0,"release_tails_cut":0,
            "policy":"cut_at_complete_callback_end"
        });
        let forged: PerformanceSourceReview = serde_json::from_value(forged).unwrap();
        let finish = forged.performance.finish_cut.unwrap();
        LocalRecord {
            version: forged.profile_version,
            record: PerformanceRecord::Finish {
                callback_index: finish.callback_index,
                finish,
            },
        }
        .validate()
        .expect("scope mismatch must retain an intrinsically valid Finish");
        assert!(
            PreparedPerformanceSourceAction::prepare(
                ACTION.into(),
                &retained_bytes(forged),
                valid.recovery_operation_id.clone(),
                None
            )
            .is_err(),
            "accepted Finish from {mismatch}"
        );
    }
    let mut invalid_stage = original.clone();
    invalid_stage.stage_id = "0".repeat(32);
    let invalid_stage_raw = retained_bytes(invalid_stage);
    // Submission shape remains valid; only the backend's retained DTO checks
    // see this rehashed invalid source identity.
    let error = backend
        .handle(&command(&valid, &invalid_stage_raw, None))
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::IntentRejected);
    assert!(!error.outcome_unknown);
    let mut wrong_root = original;
    wrong_root.journal_path = fixture
        .0
        .join("different-root/captures")
        .join(STAGE)
        .join("take.sqlite3");
    let wrong_raw = retained_bytes(wrong_root);
    let wrong_action = prepare(ACTION, &wrong_raw, "synthetic-rejections");
    let error = backend
        .handle(&command(&wrong_action, &wrong_raw, None))
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::IntentRejected);
    assert!(!error.outcome_unknown);
    for field in 0..4 {
        let mut action = valid.clone();
        match field {
            0 => action.version = 2,
            1 => action.policy_version = 2,
            2 => action.admission_sha256 = "e".repeat(64),
            _ => action.review_blob_sha256 = "e".repeat(64),
        }
        let error = backend.handle(&command(&action, &raw, None)).unwrap_err();
        assert_eq!(
            error.kind,
            if field < 2 {
                FailureKind::Rejected
            } else {
                FailureKind::IntentRejected
            }
        );
        assert!(!error.outcome_unknown);
    }
    for (id, revision, payload_sha256) in [
        (LiveId([0; 16]), 1, "e".repeat(64)),
        (LiveId([2; 16]), 0, "e".repeat(64)),
        (LiveId([2; 16]), 17, "e".repeat(64)),
        (LiveId([2; 16]), 1, "not-a-hash".into()),
    ] {
        let mut action = valid.clone();
        action.origin = Some(PerformanceSourceIntentReference {
            id,
            revision,
            payload_sha256,
        });
        assert!(action.validate().is_err());
    }
    let mut unknown = serde_json::to_value(&valid).unwrap();
    unknown["successful_result"] = json!({"path":"forged"});
    assert!(serde_json::from_value::<PreparedPerformanceSourceAction>(unknown).is_err());
    let mut kind = serde_json::to_value(&valid).unwrap();
    kind["kind"] = json!("recovered");
    assert!(serde_json::from_value::<PreparedPerformanceSourceAction>(kind).is_err());
    assert!(
        backend
            .store()
            .unwrap()
            .list_performance_source_actions()
            .unwrap()
            .reviews
            .is_empty()
    );
    assert_eq!(source_snapshot(&fixture.paths().native), before);
}

fn save_intent(
    backend: &mut NativeBackend,
    intent: &PerformanceIntent,
    expected: Option<u64>,
) -> PerformanceIntentReceipt {
    let Reply::PerformanceIntent(receipt) = backend
        .handle(&Command::SavePerformanceIntent {
            expected_revision: expected,
            intent: Box::new(intent.clone()),
        })
        .unwrap()
    else {
        panic!("historical recording intent")
    };
    *receipt
}

#[test]
fn exact_historical_intent_reference_accepts_old_revision_and_retains_release_and_conflicts() {
    let (fixture, mut backend, review, raw) = setup();
    let project = backend
        .store()
        .unwrap()
        .create(&ProjectDocument::new("Synthetic linked source").unwrap())
        .unwrap();
    let mut intent = PerformanceIntent {
        version: 1,
        start: PerformanceStart {
            id: LiveId([2; 16]),
            output_device: None,
            gui_clock: review.config.gui_clock.clone(),
        },
        project_id: ProjectId(project.id().into()),
        journal_path: review.journal_path.clone(),
        descriptor_sha256: None,
        finish: None,
        attachment: None,
        released: false,
    };
    let historical = save_intent(&mut backend, &intent, None);
    intent.descriptor_sha256 = Some(review.performance.descriptor_sha256.clone());
    let described = save_intent(&mut backend, &intent, Some(1));
    intent.released = true;
    save_intent(&mut backend, &intent, Some(2));
    let before = backend.store().unwrap().list_performance_intents().unwrap();
    let sources = source_snapshot(&fixture.paths().native);
    let origin = PerformanceSourceIntentReference {
        id: LiveId([2; 16]),
        revision: historical.revision,
        payload_sha256: historical.payload_sha256,
    };
    let action = PreparedPerformanceSourceAction::prepare(
        ACTION.into(),
        &raw,
        OperationId("synthetic-linked-history".into()),
        Some(origin.clone()),
    )
    .unwrap();
    let receipt = saved(&mut backend, &action, &raw, None);
    assert_eq!(listed(&mut backend), vec![receipt]);
    let mut wrong_hash = action.clone();
    wrong_hash.id = "33333333333333333333333333333333".into();
    wrong_hash.origin.as_mut().unwrap().payload_sha256 = "e".repeat(64);
    assert!(backend.handle(&command(&wrong_hash, &raw, None)).is_err());
    let mut conflicting = review.clone();
    conflicting.config.gui_clock.epoch.push_str("-conflicting");
    let conflict_raw = retained_bytes(conflicting);
    let conflict = PreparedPerformanceSourceAction::prepare(
        "44444444444444444444444444444444".into(),
        &conflict_raw,
        OperationId("synthetic-clock-conflict".into()),
        Some(origin),
    )
    .unwrap();
    assert!(
        backend
            .handle(&command(&conflict, &conflict_raw, None))
            .is_err()
    );
    let mut conflicting = review;
    conflicting.performance.descriptor_sha256 = "e".repeat(64);
    let conflict_raw = retained_bytes(conflicting);
    let conflict = PreparedPerformanceSourceAction::prepare(
        "55555555555555555555555555555555".into(),
        &conflict_raw,
        OperationId("synthetic-descriptor-conflict".into()),
        Some(PerformanceSourceIntentReference {
            id: LiveId([2; 16]),
            revision: 2,
            payload_sha256: described.payload_sha256,
        }),
    )
    .unwrap();
    assert!(
        backend
            .handle(&command(&conflict, &conflict_raw, None))
            .is_err()
    );
    assert_eq!(
        backend.store().unwrap().list_performance_intents().unwrap(),
        before
    );
    assert_eq!(source_snapshot(&fixture.paths().native), sources);
    assert_eq!(listed(&mut backend).len(), 1);
}

#[test]
fn two_handles_fence_stale_validation_and_typed_operations_remain_unique_without_invented_intents()
{
    let (fixture, mut backend, _, raw) = setup();
    let mut other = fixture.backend();
    let observed_empty = other
        .store()
        .unwrap()
        .list_performance_source_actions()
        .unwrap();
    let action = prepare(ACTION, &raw, "synthetic-one-operation");
    saved(&mut backend, &action, &raw, None);
    assert!(
        backend
            .store()
            .unwrap()
            .list_performance_intents()
            .unwrap()
            .is_empty()
    );
    let mut changed = action.clone();
    changed.recovery_operation_id = OperationId("synthetic-replacement".into());
    assert!(backend.handle(&command(&changed, &raw, Some(1))).is_err());
    let mut shared = action.clone();
    shared.id = "22222222222222222222222222222222".into();
    let stale_new = other
        .store()
        .unwrap()
        .save_performance_source_action_if_history_matches(
            &shared.id,
            None,
            &raw,
            &serde_json::to_vec(&shared).unwrap(),
            &observed_empty,
        )
        .unwrap_err();
    assert!(matches!(stale_new, omniusong_store::StoreError::Conflict));
    let stale_replay = other
        .store()
        .unwrap()
        .save_performance_source_action_if_history_matches(
            &action.id,
            Some(999),
            &raw,
            &serde_json::to_vec(&action).unwrap(),
            &observed_empty,
        )
        .unwrap_err();
    assert!(matches!(
        stale_replay,
        omniusong_store::StoreError::Conflict
    ));
    // Once the second backend observes the winner, typed validation rejects the
    // duplicate operation instead of retrying against a fresh opaque history.
    assert!(other.handle(&command(&shared, &raw, None)).is_err());
    let history = backend
        .store()
        .unwrap()
        .list_performance_source_actions()
        .unwrap();
    assert_eq!(history.reviews.len(), 1);
    assert_eq!(history.revisions.len(), 1);
    assert_eq!(
        other
            .store()
            .unwrap()
            .list_performance_source_actions()
            .unwrap(),
        history
    );
}

#[test]
fn opaque_valid_but_unsupported_rehashed_or_later_typed_history_blocks_list_and_unrelated_save() {
    for scenario in 0..5 {
        let (fixture, mut backend, _, raw) = setup();
        let action = prepare(ACTION, &raw, "synthetic-opaque-history");
        let canonical = serde_json::to_vec(&action).unwrap();
        let mut first_payload = canonical.clone();
        if scenario == 1 {
            first_payload = serde_json::to_vec_pretty(&action).unwrap();
        }
        if scenario == 2 {
            let mut forged = action.clone();
            forged.admission_sha256 = "e".repeat(64);
            first_payload = serde_json::to_vec(&forged).unwrap();
        }
        if scenario == 3 {
            let mut future: Value = serde_json::from_slice(&canonical).unwrap();
            future["phase"] = json!("recovered");
            first_payload = serde_json::to_vec(&future).unwrap();
        }
        let first = backend
            .store()
            .unwrap()
            .save_performance_source_action(ACTION, None, &raw, &first_payload)
            .unwrap();
        assert_eq!(first.payload_sha256, sha(&first_payload));
        if scenario == 0 {
            let mut changed = action.clone();
            changed.recovery_operation_id = OperationId("synthetic-next-phase".into());
            backend
                .store()
                .unwrap()
                .save_performance_source_action(
                    ACTION,
                    Some(1),
                    &raw,
                    &serde_json::to_vec(&changed).unwrap(),
                )
                .unwrap();
        }
        if scenario == 4 {
            let mut shared = action;
            shared.id = "22222222222222222222222222222222".into();
            backend
                .store()
                .unwrap()
                .save_performance_source_action(
                    &shared.id,
                    None,
                    &raw,
                    &serde_json::to_vec(&shared).unwrap(),
                )
                .unwrap();
        }
        let before = backend
            .store()
            .unwrap()
            .list_performance_source_actions()
            .unwrap();
        let sources = source_snapshot(&fixture.paths().native);
        assert!(
            backend
                .handle(&Command::ListPreparedPerformanceSourceActions)
                .is_err(),
            "scenario {scenario}"
        );
        let unrelated = prepare(
            "99999999999999999999999999999999",
            &raw,
            "synthetic-unrelated",
        );
        assert!(
            backend.handle(&command(&unrelated, &raw, None)).is_err(),
            "scenario {scenario}"
        );
        assert_eq!(
            backend
                .store()
                .unwrap()
                .list_performance_source_actions()
                .unwrap(),
            before
        );
        assert_eq!(source_snapshot(&fixture.paths().native), sources);
    }
}

fn completion(service: &mut SessionService) -> Completion {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(completion) = service.try_poll().unwrap() {
            return completion;
        }
        assert!(Instant::now() < deadline, "bounded synthetic session reply");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn sole_session_worker_keeps_credit_and_retries_lost_prepared_ack_without_duplicate_or_engine_work()
{
    struct Watched {
        inner: NativeBackend,
        calls: Arc<Mutex<Vec<(CommandKind, std::thread::ThreadId)>>>,
        lost: bool,
    }
    impl Backend for Watched {
        fn handle(&mut self, command: &Command) -> Result<Reply, ServiceError> {
            self.calls
                .lock()
                .unwrap()
                .push((command.kind(), std::thread::current().id()));
            let reply = self.inner.handle(command)?;
            if matches!(command, Command::SavePreparedPerformanceSourceAction { .. }) && !self.lost
            {
                self.lost = true;
                return Err(ServiceError::backend(
                    "Injected postcommit acknowledgement loss",
                    true,
                ));
            }
            Ok(reply)
        }
    }
    let (fixture, backend, _, raw) = setup();
    let before = source_snapshot(&fixture.paths().native);
    let action = prepare(ACTION, &raw, "synthetic-worker-retry");
    let save = command(&action, &raw, None);
    assert!(save.mutates() && save.retry_safe());
    assert!(!Command::ListPreparedPerformanceSourceActions.mutates());
    assert!(Command::ListPreparedPerformanceSourceActions.retry_safe());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut service = SessionService::spawn_with_backend(Watched {
        inner: backend,
        calls: Arc::clone(&calls),
        lost: false,
    })
    .unwrap();
    let id = service.try_submit(save.clone()).unwrap();
    assert!(!service.cancel_source_inspection(id));
    assert_eq!(
        service
            .try_submit(Command::ListPreparedPerformanceSourceActions)
            .unwrap_err()
            .kind,
        SubmitFailure::Busy
    );
    let reply = completion(&mut service);
    assert_eq!(reply.id, id);
    assert_eq!(reply.kind, CommandKind::SavePreparedPerformanceSourceAction);
    assert!(reply.result.unwrap_err().outcome_unknown);
    let retry = service.retry_command().unwrap().clone();
    let Command::SavePreparedPerformanceSourceAction {
        expected_revision,
        action: retry_action,
        review: retry_review,
    } = &retry
    else {
        panic!("exact Prepared retry")
    };
    assert_eq!(*expected_revision, None);
    assert_eq!(retry_action.as_ref(), &action);
    assert_eq!(*retry_review, raw);
    let id = service.try_submit(retry).unwrap();
    let reply = completion(&mut service);
    assert_eq!(reply.id, id);
    let Reply::PreparedPerformanceSourceAction(saved) = reply.result.unwrap() else {
        panic!("retried receipt")
    };
    assert_eq!(saved.revision, 1);
    let id = service
        .try_submit(Command::ListPreparedPerformanceSourceActions)
        .unwrap();
    let reply = completion(&mut service);
    assert_eq!(reply.id, id);
    assert_eq!(
        reply.kind,
        CommandKind::ListPreparedPerformanceSourceActions
    );
    let Reply::PreparedPerformanceSourceActions(list) = reply.result.unwrap() else {
        panic!("worker listing")
    };
    assert_eq!(list, vec![*saved]);
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|call| call.0).collect::<Vec<_>>(),
        vec![
            CommandKind::SavePreparedPerformanceSourceAction,
            CommandKind::SavePreparedPerformanceSourceAction,
            CommandKind::ListPreparedPerformanceSourceActions
        ]
    );
    assert!(
        calls
            .iter()
            .all(|call| call.1 != std::thread::current().id())
    );
    assert!(calls.iter().all(|call| call.1 == calls[0].1));
    assert_eq!(source_snapshot(&fixture.paths().native), before);
}

#[test]
fn stale_ids_kinds_and_reply_variants_cannot_complete_prepared_service_credit() {
    let (_fixture, _backend, _, raw) = setup();
    let action = prepare(ACTION, &raw, "synthetic-correlated");
    let receipt = PreparedPerformanceSourceActionReceipt {
        revision: 1,
        payload_sha256: sha(&serde_json::to_vec(&action).unwrap()),
        action: action.clone(),
    };
    let (mut service, requests, responses) = SessionService::channels();
    let id = service.try_submit(command(&action, &raw, None)).unwrap();
    assert_eq!(requests.try_recv().unwrap().id, id);
    let kind = CommandKind::SavePreparedPerformanceSourceAction;
    for (wrong_id, wrong_kind, reply) in [
        (
            RequestId {
                generation: id.generation + 1,
                ..id
            },
            kind,
            Reply::PreparedPerformanceSourceAction(Box::new(receipt.clone())),
        ),
        (
            RequestId {
                sequence: id.sequence + 1,
                ..id
            },
            kind,
            Reply::PreparedPerformanceSourceAction(Box::new(receipt.clone())),
        ),
        (
            id,
            CommandKind::ListPreparedPerformanceSourceActions,
            Reply::PreparedPerformanceSourceActions(vec![receipt.clone()]),
        ),
        (id, kind, Reply::PerformanceIntents(vec![])),
    ] {
        responses
            .try_send(Completion {
                id: wrong_id,
                kind: wrong_kind,
                result: Ok(reply),
            })
            .unwrap();
        let error = service.try_poll().unwrap_err();
        assert_eq!(error.kind, FailureKind::Protocol);
        assert!(error.outcome_unknown);
        assert_eq!(service.pending().unwrap().id, id);
        assert_eq!(
            service
                .try_submit(Command::ListPreparedPerformanceSourceActions)
                .unwrap_err()
                .kind,
            SubmitFailure::Busy
        );
    }
    responses
        .try_send(Completion {
            id,
            kind,
            result: Ok(Reply::PreparedPerformanceSourceAction(Box::new(
                receipt.clone(),
            ))),
        })
        .unwrap();
    assert_eq!(service.try_poll().unwrap().unwrap().id, id);
    assert!(!service.is_busy());
    let id = service
        .try_submit(Command::ListPreparedPerformanceSourceActions)
        .unwrap();
    requests.try_recv().unwrap();
    assert!(!service.cancel_source_inspection(id));
    responses
        .try_send(Completion {
            id,
            kind: CommandKind::ListPreparedPerformanceSourceActions,
            result: Ok(Reply::PreparedPerformanceSourceAction(Box::new(
                receipt.clone(),
            ))),
        })
        .unwrap();
    assert_eq!(service.try_poll().unwrap_err().kind, FailureKind::Protocol);
    assert!(service.is_busy());
    responses
        .try_send(Completion {
            id,
            kind: CommandKind::ListPreparedPerformanceSourceActions,
            result: Ok(Reply::PreparedPerformanceSourceActions(vec![receipt])),
        })
        .unwrap();
    assert!(service.try_poll().unwrap().unwrap().result.is_ok());
    assert!(!service.is_busy());
}
