//! Actual isolated metadata observations and separately injected credit checks.
//! All sources use the local synthetic fixture; no native inputs or live catalog.
#![cfg(unix)]

use super::tests as fixture;
use super::*;
use crate::validate_performance_source_reply;
use crate::{
    Backend, Command, Completion, PerformanceSourceDecisionReceipt, SessionService, SubmitFailure,
};
use omniusong_core::{
    contracts::{OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_store::TakePublication;
use rusqlite::{Connection, OpenFlags, types::ValueRef};

const ACTION: &str = "11111111111111111111111111111111";
const OTHER: &str = "22222222222222222222222222222222";
const CHOICE: &str = "33333333333333333333333333333333";
const NEXT: &str = "44444444444444444444444444444444";

fn reference(state: &PerformanceSourceActionState) -> PerformanceSourceActionReference {
    PerformanceSourceActionReference {
        id: state.prepared.action.id.clone(),
        revision: state.revision,
        payload_sha256: state.payload_sha256.clone(),
    }
}

fn read_command(requested: &PerformanceSourceActionReference) -> Command {
    Command::ReadPerformanceSourceAction {
        id: requested.id.clone(),
        revision: requested.revision,
        payload_sha256: requested.payload_sha256.clone(),
    }
}

fn read(
    backend: &mut NativeBackend,
    requested: &PerformanceSourceActionReference,
) -> Result<PerformanceSourceActionDetail, ServiceError> {
    let command = read_command(requested);
    let reply = backend.handle(&command)?;
    assert!(reply.matches_command(&command));
    let Reply::PerformanceSourceActionDetail(detail) = reply else {
        panic!("metadata detail reply")
    };
    detail.validate().unwrap();
    Ok(*detail)
}

// These snapshots use SQLite's cooperating read-only connection on this test's
// private catalog. They never raw-open an active SQLite file or sidecar.
// Every table value is framed by its SQLite storage type and exact value bytes.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    tables: BTreeMap<String, Vec<Vec<u8>>>,
    files: BTreeMap<PathBuf, FileTree>,
}
type FileTree = BTreeMap<PathBuf, Option<Vec<u8>>>;

fn table_rows(connection: &Connection, table: &str) -> Vec<Vec<u8>> {
    let quoted = table.replace('"', "\"\"");
    let mut statement = connection
        .prepare(&format!("SELECT * FROM \"{quoted}\""))
        .unwrap();
    let columns = statement.column_count();
    let mut rows = statement.query([]).unwrap();
    let mut result = Vec::new();
    while let Some(row) = rows.next().unwrap() {
        let mut bytes = Vec::new();
        for column in 0..columns {
            let (tag, value) = match row.get_ref(column).unwrap() {
                ValueRef::Null => (0, Vec::new()),
                ValueRef::Integer(value) => (1, value.to_le_bytes().to_vec()),
                ValueRef::Real(value) => (2, value.to_bits().to_le_bytes().to_vec()),
                ValueRef::Text(value) => (3, value.to_vec()),
                ValueRef::Blob(value) => (4, value.to_vec()),
            };
            bytes.push(tag);
            bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&value);
        }
        result.push(bytes);
    }
    result.sort();
    result
}

fn snapshot(fixture: &fixture::Fixture) -> Snapshot {
    let native = fixture.paths().native;
    let connection = Connection::open_with_flags(
        native.join("studio.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    connection.execute_batch("BEGIN").unwrap();
    let names = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut tables = BTreeMap::new();
    tables.insert(
        "sqlite_schema".into(),
        table_rows(&connection, "sqlite_schema"),
    );
    for name in names {
        tables.insert(name.clone(), table_rows(&connection, &name));
    }
    connection.execute_batch("COMMIT").unwrap();
    drop(connection);
    let files = [
        native.join("captures"),
        native.join("source-actions"),
        native.join("takes"),
        fixture.root.join("parked-original"),
    ]
    .into_iter()
    .map(|path| {
        let bytes = fixture::tree(&path);
        (path, bytes)
    })
    .collect();
    Snapshot { tables, files }
}

fn prepared_reference(
    prepared: &PreparedPerformanceSourceActionReceipt,
) -> PerformanceSourceActionReference {
    PerformanceSourceActionReference {
        id: prepared.action.id.clone(),
        revision: prepared.revision,
        payload_sha256: prepared.payload_sha256.clone(),
    }
}

fn save(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
    decision: PerformanceSourceDecision,
) -> (Command, PerformanceSourceDecisionReceipt) {
    let command = Command::SavePerformanceSourceDecision {
        id: state.prepared.action.id.clone(),
        revision: state.revision,
        payload_sha256: state.payload_sha256.clone(),
        decision: Box::new(decision),
    };
    let reply = backend.handle(&command).unwrap();
    assert!(reply.matches_command(&command));
    let Reply::PerformanceSourceDecision(receipt) = reply else {
        panic!("decision receipt")
    };
    assert_eq!(receipt.requested, reference(state));
    (command, *receipt)
}

fn choose(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
    source: &PerformanceSourceReview,
    id: &str,
    supersedes: Option<&str>,
) -> PerformanceSourceActionState {
    let target = fixture::projects(backend).remove(0).0;
    let Some(PerformanceSourceExecutionRevision {
        phase: PerformanceSourceExecutionPhase::Verified { result, .. },
        ..
    }) = &state.execution
    else {
        panic!("Verified source")
    };
    let choice = PerformanceSourceAttachmentChoice {
        id: id.into(),
        supersedes: supersedes.map(String::from),
        publication: TakePublication {
            operation_id: attachment_operation_id(&state.prepared.action.id, id).unwrap(),
            project_id: ProjectId(target.id),
            expected_revision: target.revision,
            track_id: TrackId(format!("detail-track-{id}")),
            track_name: "Recovered original metadata test".into(),
            clip: TakeClip {
                id: format!("detail-clip-{id}"),
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
    };
    save(
        backend,
        state,
        PerformanceSourceDecision::PrepareAttachment {
            choice: Box::new(choice),
        },
    )
    .1
    .state
}

fn choice_reference(state: &PerformanceSourceActionState) -> PerformanceSourceActionReference {
    let attachment = state.decisions.attachment.as_ref().unwrap();
    PerformanceSourceActionReference {
        id: state.prepared.action.id.clone(),
        revision: attachment.revision,
        payload_sha256: attachment.payload_sha256.clone(),
    }
}

fn attachment_command(requested: &PerformanceSourceActionReference, execute: bool) -> Command {
    if execute {
        Command::AttachPerformanceSource {
            id: requested.id.clone(),
            revision: requested.revision,
            payload_sha256: requested.payload_sha256.clone(),
        }
    } else {
        Command::ReconcilePerformanceSourceAttachment {
            id: requested.id.clone(),
            revision: requested.revision,
            payload_sha256: requested.payload_sha256.clone(),
        }
    }
}

#[test]
fn prepared_and_verified_detail_retain_exact_raw_bytes_and_canonical_root_without_mutation() {
    let (fixture, mut backend, source, raw, prepared) = fixture::setup(fixture::Mode::Tail);
    assert_ne!(
        raw,
        serde_json::to_vec(&source).unwrap(),
        "pretty retained bytes differ from canonical encoding"
    );
    fs::rename(
        fixture.source_path().parent().unwrap(),
        fixture.root.join("parked-original"),
    )
    .unwrap();
    let before = snapshot(&fixture);
    let prepared_detail = read(&mut backend, &prepared_reference(&prepared)).unwrap();
    assert_eq!(prepared_detail.version, 1);
    assert_eq!(
        prepared_detail.origin_root,
        fs::canonicalize(fixture.paths().native).unwrap()
    );
    assert_eq!(
        prepared_detail.origin_root.as_os_str(),
        fixture.paths().native.as_os_str()
    );
    assert_eq!(prepared_detail.review, raw);
    assert_eq!(prepared_detail.state.prepared, prepared);
    assert!(prepared_detail.state.execution.is_none());
    assert!(!fixture.source_path().exists());
    assert_eq!(snapshot(&fixture), before);

    fs::rename(
        fixture.root.join("parked-original"),
        fixture.source_path().parent().unwrap(),
    )
    .unwrap();
    let verified = fixture::execute(&mut backend, &prepared).unwrap();
    fs::rename(
        fixture.source_path().parent().unwrap(),
        fixture.root.join("parked-original"),
    )
    .unwrap();
    let before = snapshot(&fixture);
    for _ in 0..2 {
        let detail = read(&mut backend, &reference(&verified.state)).unwrap();
        assert_eq!(detail.state, verified.state);
        assert_eq!(detail.review, raw);
        assert_eq!(
            detail.origin_root.as_os_str(),
            fixture.paths().native.as_os_str()
        );
    }
    assert!(!fixture.source_path().exists());
    assert_eq!(snapshot(&fixture), before);
}

#[test]
fn metadata_read_requires_the_current_exact_head_and_refusals_leave_all_tables_and_artifacts() {
    let (fixture, mut backend, _, raw, prepared) = fixture::setup(fixture::Mode::Clean);
    let verified = fixture::execute(&mut backend, &prepared).unwrap();
    let (_, kept) = save(
        &mut backend,
        &verified.state,
        PerformanceSourceDecision::KeepLocally,
    );
    let head = reference(&kept.state);
    let before = snapshot(&fixture);
    let detail = read(&mut backend, &head).unwrap();
    assert_eq!(detail.state, kept.state);
    assert_eq!(detail.review, raw);
    let invalid = [
        prepared_reference(&prepared),
        reference(&verified.state),
        PerformanceSourceActionReference {
            payload_sha256: "a".repeat(64),
            ..head.clone()
        },
        PerformanceSourceActionReference {
            id: OTHER.into(),
            ..head.clone()
        },
        PerformanceSourceActionReference {
            revision: 0,
            ..head.clone()
        },
        PerformanceSourceActionReference {
            revision: u64::MAX,
            ..head.clone()
        },
        PerformanceSourceActionReference {
            id: "../capture".into(),
            ..head.clone()
        },
    ];
    for requested in invalid {
        let error = backend.handle(&read_command(&requested)).unwrap_err();
        assert_eq!(error.kind, FailureKind::Rejected);
        assert!(!error.outcome_unknown);
        let reply = Reply::PerformanceSourceActionDetail(Box::new(detail.clone()));
        assert!(!reply.matches_command(&read_command(&requested)));
    }
    assert_eq!(snapshot(&fixture), before);
}

fn rehash_prepared(detail: &mut PerformanceSourceActionDetail) {
    detail.state.prepared.payload_sha256 = digest(
        b"",
        &serde_json::to_vec(&detail.state.prepared.action).unwrap(),
    );
    if detail.state.revision == 1 {
        detail.state.payload_sha256 = detail.state.prepared.payload_sha256.clone();
    }
}

#[test]
fn detail_validation_rejects_bounds_malformed_review_and_root_admission_or_state_substitutions() {
    let (fixture, mut backend, _, _, prepared) = fixture::setup(fixture::Mode::Clean);
    let detail = read(&mut backend, &prepared_reference(&prepared)).unwrap();
    type Mutation = (&'static str, fn(&mut PerformanceSourceActionDetail));
    let mutations: Vec<Mutation> = vec![
        ("version", |d| d.version = 2),
        ("relative root", |d| d.origin_root = PathBuf::from("native")),
        ("different root", |d| {
            d.origin_root = d.origin_root.join("different")
        }),
        ("lexical root alias", |d| {
            d.origin_root = d.origin_root.join(".")
        }),
        ("oversized root", |d| {
            d.origin_root = PathBuf::from(format!("/{}", "a".repeat(4096)))
        }),
        ("control in root", |d| {
            d.origin_root = PathBuf::from("/native\nroot")
        }),
        ("non-UTF-8 root", |d| {
            use std::os::unix::ffi::OsStringExt;
            d.origin_root = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 255]));
        }),
        ("review byte substitution", |d| d.review[0] ^= 1),
        ("oversized detail", |d| {
            d.review = vec![b' '; super::super::MAX_SOURCE_ACTION_DETAIL_BYTES]
        }),
        ("malformed but rebound raw review", |d| {
            d.review = b"{}".to_vec();
            d.state.prepared.action.review_blob_sha256 = digest(b"", &d.review);
            rehash_prepared(d);
        }),
        ("action ID substitution", |d| {
            d.state.prepared.action.id = OTHER.into()
        }),
        ("rehashed admission substitution", |d| {
            d.state.prepared.action.admission_sha256 = "a".repeat(64);
            rehash_prepared(d);
        }),
        ("Prepared revision", |d| d.state.prepared.revision = 2),
        ("head payload substitution", |d| {
            d.state.payload_sha256 = "b".repeat(64)
        }),
        ("head revision overflow", |d| d.state.revision = u64::MAX),
        ("phase without execution", |d| d.state.revision = 3),
        ("decision before Verified", |d| {
            d.state.decisions.kept_locally = true
        }),
    ];
    let before = snapshot(&fixture);
    for (name, mutate) in mutations {
        let mut invalid = detail.clone();
        mutate(&mut invalid);
        assert!(invalid.validate().is_err(), "{name}");
        let reply = Reply::PerformanceSourceActionDetail(Box::new(invalid));
        let command = read_command(&prepared_reference(&prepared));
        assert!(
            validate_performance_source_reply(&reply, &command).is_err(),
            "{name}"
        );
    }
    assert_eq!(snapshot(&fixture), before);
    let verified = fixture::execute(&mut backend, &prepared).unwrap();
    let mut detail = read(&mut backend, &reference(&verified.state)).unwrap();
    let Some(PerformanceSourceExecutionRevision {
        phase: PerformanceSourceExecutionPhase::Verified { result, .. },
        ..
    }) = &mut detail.state.execution
    else {
        panic!("Verified")
    };
    result.descriptor_sha256 = "c".repeat(64);
    let execution = detail.state.execution.as_ref().unwrap();
    detail.state.payload_sha256 = digest(b"", &serde_json::to_vec(execution).unwrap());
    assert!(
        detail.validate().is_err(),
        "rehashed Verified result still must bind the retained review"
    );
    assert_eq!(
        before.tables["performance_source_reviews"],
        snapshot(&fixture).tables["performance_source_reviews"]
    );
}

#[test]
fn detail_and_reference_wire_shapes_refuse_unknown_fields_and_invalid_exact_references() {
    let (_fixture, mut backend, _, _, prepared) = fixture::setup(fixture::Mode::Clean);
    let detail = read(&mut backend, &prepared_reference(&prepared)).unwrap();
    let wire = serde_json::to_value(&detail).unwrap();
    for field in ["unexpected", "requested", "source_reopened"] {
        let mut changed = wire.clone();
        changed[field] = serde_json::json!(true);
        assert!(serde_json::from_value::<PerformanceSourceActionDetail>(changed).is_err());
    }
    let mut changed = wire.clone();
    changed["state"]["unretained"] = serde_json::json!(1);
    assert!(serde_json::from_value::<PerformanceSourceActionDetail>(changed).is_err());
    let mut changed = wire;
    changed["state"]["prepared"]["action"]["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<PerformanceSourceActionDetail>(changed).is_err());

    let valid = prepared_reference(&prepared);
    valid.validate().unwrap();
    for invalid in [
        PerformanceSourceActionReference {
            id: "0".repeat(32),
            ..valid.clone()
        },
        PerformanceSourceActionReference {
            revision: 0,
            ..valid.clone()
        },
        PerformanceSourceActionReference {
            revision: 17,
            ..valid.clone()
        },
        PerformanceSourceActionReference {
            payload_sha256: "A".repeat(64),
            ..valid.clone()
        },
        PerformanceSourceActionReference {
            payload_sha256: "f".repeat(63),
            ..valid.clone()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
    let mut changed = serde_json::to_value(&valid).unwrap();
    changed["other"] = serde_json::json!(0);
    assert!(serde_json::from_value::<PerformanceSourceActionReference>(changed).is_err());
    let mut changed = serde_json::to_value(&valid).unwrap();
    changed["revision"] = serde_json::json!(-1);
    assert!(serde_json::from_value::<PerformanceSourceActionReference>(changed).is_err());
    let receipt = PerformanceSourceDecisionReceipt {
        requested: valid,
        state: detail.state,
    };
    let mut changed = serde_json::to_value(receipt).unwrap();
    changed["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PerformanceSourceDecisionReceipt>(changed).is_err());
}

#[test]
fn fully_rehashed_legacy_lexical_alias_is_refused_by_detail_without_rewriting_retained_bytes() {
    let (fixture, mut backend, mut source, _, _) = fixture::setup(fixture::Mode::Clean);
    source.journal_path = fixture
        .paths()
        .native
        .join("captures")
        .join(".")
        .join(&source.stage_id)
        .join("take.sqlite3");
    assert_eq!(
        source.journal_path,
        fixture.source_path(),
        "legacy Path comparison treats this as an alias"
    );
    assert_ne!(
        source.journal_path.as_os_str(),
        fixture.source_path().as_os_str()
    );
    source.review_sha256.clear();
    source.review_sha256 = digest(
        b"OmniuSong.performance-source-review.v1\0",
        &serde_json::to_vec(&source).unwrap(),
    );
    let raw = serde_json::to_vec_pretty(&source).unwrap();
    let action = super::super::PreparedPerformanceSourceAction::prepare(
        OTHER.into(),
        &raw,
        OperationId("detail-test.legacy-alias".into()),
        None,
    )
    .unwrap();
    // The legacy decoder intentionally remains unchanged. Retain a fully valid
    // Prepared row with newly computed review, admission and payload hashes.
    let Reply::PreparedPerformanceSourceAction(prepared) = backend
        .save_prepared_source_action(None, &action, &raw)
        .unwrap()
    else {
        panic!("Prepared")
    };
    let before = snapshot(&fixture);
    let history = fixture::history(&mut backend);
    assert_eq!(
        history
            .reviews
            .iter()
            .find(|r| r.id == OTHER)
            .unwrap()
            .payload,
        raw
    );
    let error = backend
        .handle(&read_command(&prepared_reference(&prepared)))
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::Rejected);
    assert!(!error.outcome_unknown);
    assert_eq!(fixture::history(&mut backend), history);
    assert_eq!(snapshot(&fixture), before);
}

fn requested_mut(reply: &mut Reply) -> &mut PerformanceSourceActionReference {
    match reply {
        Reply::PerformanceSourceExecution(receipt) => &mut receipt.requested,
        Reply::PerformanceSourceDecision(receipt) => &mut receipt.requested,
        Reply::PerformanceSourceAttachment(receipt) => &mut receipt.requested,
        _ => panic!("receipt with exact requested triple"),
    }
}

fn returned_state_mut(reply: &mut Reply) -> &mut PerformanceSourceActionState {
    match reply {
        Reply::PerformanceSourceExecution(receipt) => &mut receipt.state,
        Reply::PerformanceSourceDecision(receipt) => &mut receipt.state,
        Reply::PerformanceSourceAttachment(receipt) => &mut receipt.state,
        _ => panic!("receipt with retained state"),
    }
}

// Injected wire/completion checks only: each baseline reply is first obtained
// from an actual backend. This is not a worker fault or physical storage test.
fn assert_requested_credit(command: Command, mut make_actual: impl FnMut() -> Reply) {
    let actual = make_actual();
    assert!(actual.matches_command(&command));
    let (mut service, peer) = SessionService::test_pair();
    let id = service.try_submit(command.clone()).unwrap();
    let pending = peer.requests.try_recv().unwrap();
    assert_eq!(pending.id, id);
    for field in 0..5 {
        let mut forged = make_actual();
        match field {
            0 => requested_mut(&mut forged).id = OTHER.into(),
            1 => requested_mut(&mut forged).revision += 1,
            2 => requested_mut(&mut forged).payload_sha256 = "e".repeat(64),
            3 => returned_state_mut(&mut forged).prepared.action.id = OTHER.into(),
            4 => {
                let state = returned_state_mut(&mut forged);
                *state = PerformanceSourceActionState {
                    prepared: state.prepared.clone(),
                    revision: 1,
                    payload_sha256: state.prepared.payload_sha256.clone(),
                    execution: None,
                    decisions: PerformanceSourceDecisionState::default(),
                };
            }
            _ => unreachable!(),
        }
        if forged.matches_command(&command) {
            // A matching reference with an invalid phase/body is delivered;
            // the native caller consumes it as bounded protocol uncertainty.
            assert!(validate_performance_source_reply(&forged, &command).is_err());
            let (mut semantic_service, semantic_peer) = SessionService::test_pair();
            let semantic_id = semantic_service.try_submit(command.clone()).unwrap();
            semantic_peer.requests.try_recv().unwrap();
            semantic_peer
                .responses
                .send(Completion {
                    id: semantic_id,
                    kind: command.kind(),
                    result: Ok(forged),
                })
                .unwrap();
            assert!(semantic_service.try_poll().unwrap().is_some());
            assert!(!semantic_service.is_busy());
            continue;
        }
        peer.responses
            .send(Completion {
                id,
                kind: command.kind(),
                result: Ok(forged),
            })
            .unwrap();
        assert_eq!(service.try_poll().unwrap_err().kind, FailureKind::Protocol);
        assert!(service.is_busy());
        assert!(!service.is_disconnected());
        assert_eq!(service.pending().unwrap().id, id);
        assert_eq!(
            service.try_submit(Command::Refresh).unwrap_err().kind,
            SubmitFailure::Busy
        );
    }
    peer.responses
        .send(Completion {
            id,
            kind: command.kind(),
            result: Ok(Reply::PerformanceSourceActionStates(Vec::new())),
        })
        .unwrap();
    assert_eq!(service.try_poll().unwrap_err().kind, FailureKind::Protocol);
    assert!(service.is_busy() && !service.is_disconnected());
    peer.responses
        .send(Completion {
            id,
            kind: command.kind(),
            result: Ok(actual),
        })
        .unwrap();
    let completion = service.try_poll().unwrap().unwrap();
    assert_eq!(completion.id, id);
    assert!(completion.result.is_ok());
    assert!(!service.is_busy() && !service.is_disconnected());
}

#[test]
fn actual_execution_decision_and_attachment_requested_triples_gate_injected_service_credit() {
    let (_fixture, mut backend, source, _, prepared) = fixture::setup(fixture::Mode::Clean);
    let execution = fixture::execute(&mut backend, &prepared).unwrap();
    assert_eq!(execution.requested, prepared_reference(&prepared));
    let command = Command::ExecutePerformanceSourceAction {
        id: ACTION.into(),
        revision: prepared.revision,
        payload_sha256: prepared.payload_sha256.clone(),
    };
    assert_requested_credit(command, || {
        Reply::PerformanceSourceExecution(Box::new(execution.clone()))
    });
    let (command, kept) = save(
        &mut backend,
        &execution.state,
        PerformanceSourceDecision::KeepLocally,
    );
    assert_requested_credit(command, || {
        Reply::PerformanceSourceDecision(Box::new(kept.clone()))
    });
    let (_, reviewing) = save(
        &mut backend,
        &kept.state,
        PerformanceSourceDecision::ReturnToReview,
    );
    let pending = choose(&mut backend, &reviewing.state, &source, CHOICE, None);
    let requested = choice_reference(&pending);
    let command = attachment_command(&requested, true);
    let actual = backend.handle(&command).unwrap();
    assert!(actual.matches_command(&command));
    let Reply::PerformanceSourceAttachment(receipt) = actual else {
        panic!("attachment")
    };
    assert!(receipt.committed);
    assert_eq!(receipt.requested, requested);
    assert!(receipt.state.revision > requested.revision);
    // Reconciliation returns the same actual historical attachment receipt and
    // current head, which must also correlate with its immutable Attach request.
    assert_requested_credit(command, || {
        backend
            .handle(&attachment_command(&requested, false))
            .unwrap()
    });
}

#[test]
fn started_reconcile_delivers_observation_without_verified_write_then_execute_records_verified() {
    let (fixture, mut backend, _, raw, prepared) = fixture::setup(fixture::Mode::Fault);
    let error = backend
        .run_source_action(
            Request {
                id: ACTION,
                revision: prepared.revision,
                payload_sha256: &prepared.payload_sha256,
            },
            fixture::token(),
            true,
            |boundary, _| {
                if boundary == Boundary::Recovered {
                    Err(ServiceError::backend(
                        "Synthetic lost recovered acknowledgment",
                        true,
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
    assert!(error.outcome_unknown);
    let history = fixture::history(&mut backend);
    assert_eq!(history.revisions.len(), 2);
    fs::rename(
        fixture.source_path().parent().unwrap(),
        fixture.root.join("parked-original"),
    )
    .unwrap();
    let before = snapshot(&fixture);
    let command = Command::ReconcilePerformanceSourceAction {
        id: ACTION.into(),
        revision: prepared.revision,
        payload_sha256: prepared.payload_sha256.clone(),
    };
    let actual = backend.handle(&command).unwrap();
    assert!(actual.matches_command(&command));
    let Reply::PerformanceSourceExecution(observed) = actual else {
        panic!("observation")
    };
    assert_eq!(observed.requested, prepared_reference(&prepared));
    assert_eq!(observed.state.revision, 2);
    assert!(matches!(
        observed.state.execution.as_ref().unwrap().phase,
        PerformanceSourceExecutionPhase::Started
    ));
    assert!(observed.replayed && !observed.original_reopened);
    let detail = read(&mut backend, &reference(&observed.state)).unwrap();
    assert_eq!(detail.review, raw);
    let execute_command = Command::ExecutePerformanceSourceAction {
        id: ACTION.into(),
        revision: prepared.revision,
        payload_sha256: prepared.payload_sha256.clone(),
    };
    let wrong_phase = Reply::PerformanceSourceExecution(observed.clone());
    assert!(wrong_phase.matches_command(&execute_command));
    assert!(validate_performance_source_reply(&wrong_phase, &execute_command).is_err());
    assert_requested_credit(command, || {
        Reply::PerformanceSourceExecution(observed.clone())
    });
    assert_eq!(snapshot(&fixture), before);
    assert_eq!(fixture::history(&mut backend), history);

    let actual = backend.handle(&execute_command).unwrap();
    assert!(actual.matches_command(&execute_command));
    let Reply::PerformanceSourceExecution(verified) = actual else {
        panic!("Verified")
    };
    assert_eq!(verified.state.revision, 3);
    assert_eq!(verified.observed, observed.observed);
    assert!(matches!(
        verified.state.execution.as_ref().unwrap().phase,
        PerformanceSourceExecutionPhase::Verified { .. }
    ));
    assert!(!verified.original_reopened);
    let history = fixture::history(&mut backend);
    assert_eq!(history.revisions.len(), 3);
    for row in &history.revisions {
        let command = Command::ReconcilePerformanceSourceAction {
            id: row.id.clone(),
            revision: row.revision,
            payload_sha256: row.payload_sha256.clone(),
        };
        let reply = backend.handle(&command).unwrap();
        assert!(reply.matches_command(&command));
        let Reply::PerformanceSourceExecution(receipt) = reply else {
            panic!("historical reply")
        };
        assert!(
            receipt
                .requested
                .matches(&row.id, row.revision, &row.payload_sha256)
        );
        assert_eq!(receipt.state, verified.state);
    }
    let mut premature = (*verified).clone();
    premature.requested.revision = verified.state.revision + 1;
    premature.requested.payload_sha256 = "d".repeat(64);
    let future = Command::ReconcilePerformanceSourceAction {
        id: ACTION.into(),
        revision: premature.requested.revision,
        payload_sha256: premature.requested.payload_sha256.clone(),
    };
    assert!(!Reply::PerformanceSourceExecution(Box::new(premature)).matches_command(&future));
    assert!(!fixture.source_path().exists());
}

#[test]
fn decision_replay_matches_current_and_single_append_but_refuses_stale_delta() {
    let (fixture, mut backend, _, _, prepared) = fixture::setup(fixture::Mode::Clean);
    let verified = fixture::execute(&mut backend, &prepared).unwrap();
    let (original, kept) = save(
        &mut backend,
        &verified.state,
        PerformanceSourceDecision::KeepLocally,
    );
    assert_eq!(kept.state.revision, kept.requested.revision + 1);
    let before = snapshot(&fixture);
    let replay = backend.handle(&original).unwrap();
    assert!(replay.matches_command(&original));
    let Reply::PerformanceSourceDecision(replay) = replay else {
        panic!("replay")
    };
    assert_eq!(replay.state, kept.state);
    assert_eq!(replay.requested, reference(&verified.state));
    let (current, current_receipt) = save(
        &mut backend,
        &kept.state,
        PerformanceSourceDecision::KeepLocally,
    );
    assert_eq!(current_receipt.state, kept.state);
    assert_eq!(current_receipt.requested, reference(&kept.state));
    assert_eq!(snapshot(&fixture), before);

    let (_, returned) = save(
        &mut backend,
        &kept.state,
        PerformanceSourceDecision::ReturnToReview,
    );
    let before = snapshot(&fixture);
    for command in [&original, &current] {
        assert_eq!(
            backend.handle(command).unwrap_err().kind,
            FailureKind::Rejected
        );
        let mut forged = kept.clone();
        forged.state = returned.state.clone();
        if let Command::SavePerformanceSourceDecision {
            id,
            revision,
            payload_sha256,
            ..
        } = command
        {
            forged.requested = PerformanceSourceActionReference {
                id: id.clone(),
                revision: *revision,
                payload_sha256: payload_sha256.clone(),
            };
        }
        let reply = Reply::PerformanceSourceDecision(Box::new(forged));
        assert!(reply.matches_command(command));
        assert!(validate_performance_source_reply(&reply, command).is_err());
    }
    assert_eq!(snapshot(&fixture), before);
}

#[test]
fn retired_earlier_attachment_reconciles_exact_choice_after_a_different_latest_choice() {
    let (fixture, mut backend, source, _, prepared) = fixture::setup(fixture::Mode::Clean);
    let verified = fixture::execute(&mut backend, &prepared).unwrap();
    let first = choose(&mut backend, &verified.state, &source, CHOICE, None);
    let old = choice_reference(&first);
    let old_publication = first
        .decisions
        .attachment
        .as_ref()
        .unwrap()
        .choice
        .publication
        .clone();
    let (_, retired) = save(
        &mut backend,
        &first,
        PerformanceSourceDecision::RetireAttachment,
    );
    let latest = choose(&mut backend, &retired.state, &source, NEXT, Some(CHOICE));
    let before = snapshot(&fixture);
    let command = attachment_command(&old, false);
    let reply = backend.handle(&command).unwrap();
    assert!(reply.matches_command(&command));
    let Reply::PerformanceSourceAttachment(receipt) = reply else {
        panic!("historical choice")
    };
    assert_eq!(receipt.requested, old);
    assert_eq!(receipt.state, latest);
    assert_eq!(receipt.publication, old_publication);
    assert_eq!(
        receipt
            .state
            .decisions
            .attachment
            .as_ref()
            .unwrap()
            .choice
            .id,
        NEXT
    );
    assert_ne!(
        receipt.publication,
        receipt
            .state
            .decisions
            .attachment
            .as_ref()
            .unwrap()
            .choice
            .publication
    );
    assert!(!receipt.committed);
    assert_requested_credit(command, || {
        backend.handle(&attachment_command(&old, false)).unwrap()
    });
    assert_eq!(snapshot(&fixture), before);
}

#[test]
fn injected_long_nested_catalog_failure_delivers_primary_then_retires_only_after_exact_correlation()
{
    let (_fixture, mut backend, source, _, prepared) = fixture::setup(fixture::Mode::Clean);
    let verified = fixture::execute(&mut backend, &prepared).unwrap();
    let pending = choose(&mut backend, &verified.state, &source, CHOICE, None);
    let requested = choice_reference(&pending);
    let attached = backend
        .handle(&attachment_command(&requested, true))
        .unwrap();
    assert!(attached.matches_command(&attachment_command(&requested, true)));
    let Reply::PerformanceSourceAttachment(attached) = attached else {
        panic!("committed")
    };
    let primary = attached.publication;
    let command = attachment_command(&requested, false);
    let make_injected = |backend: &mut NativeBackend| {
        let Reply::PerformanceSourceAttachment(mut receipt) = backend.handle(&command).unwrap()
        else {
            panic!("historical committed")
        };
        receipt.current_document = None;
        receipt.current_document_error = Some(ServiceError::storage_unavailable(
            "Synthetic nested catalog diagnostic ".repeat(2048),
            false,
        ));
        assert!(
            receipt
                .current_document_error
                .as_ref()
                .unwrap()
                .message
                .len()
                > 16 * 1024
        );
        Reply::PerformanceSourceAttachment(receipt)
    };
    // The primary is an actual committed historical receipt. Only its nested
    // current-document failure is injected; no physical database fault is claimed.
    let actual_with_injected_error = make_injected(&mut backend);
    assert!(actual_with_injected_error.matches_command(&command));
    let (mut service, peer) = SessionService::test_pair();
    let id = service.try_submit(command.clone()).unwrap();
    peer.requests.try_recv().unwrap();
    let mut wrong = make_injected(&mut backend);
    requested_mut(&mut wrong).payload_sha256 = "a".repeat(64);
    peer.responses
        .send(Completion {
            id,
            kind: command.kind(),
            result: Ok(wrong),
        })
        .unwrap();
    assert_eq!(service.try_poll().unwrap_err().kind, FailureKind::Protocol);
    assert!(service.is_busy() && !service.is_disconnected());
    peer.responses
        .send(Completion {
            id,
            kind: command.kind(),
            result: Ok(actual_with_injected_error),
        })
        .unwrap();
    let completion = service.try_poll().unwrap().unwrap();
    let Reply::PerformanceSourceAttachment(receipt) = completion.result.unwrap() else {
        panic!("primary receipt")
    };
    assert!(receipt.committed);
    assert_eq!(receipt.requested, requested);
    assert_eq!(receipt.publication, primary);
    let error = receipt.current_document_error.unwrap();
    assert_eq!(error.kind, FailureKind::StorageUnavailable);
    assert!(error.message.len() > 16 * 1024);
    assert!(!service.is_busy() && service.is_disconnected());
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Disconnected
    );
    assert_eq!(service.try_poll().unwrap_err(), error);
}
