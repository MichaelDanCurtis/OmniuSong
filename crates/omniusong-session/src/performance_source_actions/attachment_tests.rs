//! Unix-qualified synthetic attachment and receipt tests. No native devices.
#![cfg(unix)]

use super::super::{
    decisions::{PerformanceSourceAttachmentChoice, PerformanceSourceDecision},
    tests as fixture,
};
use super::*;
use crate::{Backend, Command, Completion, SessionService, SubmitFailure};
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const ACTION: &str = "11111111111111111111111111111111";
const OTHER: &str = "22222222222222222222222222222222";
const CHOICE: &str = "33333333333333333333333333333333";
const NEXT: &str = "44444444444444444444444444444444";

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn actual_six_stream_recovery_choices_archive_and_restore_as_inert_exact_evidence() {
    use omniusong_recording::{InspectionBudget, InspectionLimits};
    use omniusong_store::{EvidenceEntryKind, inspect_evidence_archive};
    let budget = || InspectionBudget::new(InspectionLimits::default(), fixture::token()).unwrap();
    for mode in [fixture::Mode::Tail, fixture::Mode::Finish] {
        let (f, mut backend, source, raw, verified) = verified(mode);
        let pending = choose(&mut backend, &verified, &source, CHOICE, None, 240);
        let receipt = attach(&mut backend, &pending).unwrap();
        assert!(receipt.committed);
        let kept = save(
            &mut backend,
            &receipt.state,
            PerformanceSourceDecision::KeepLocally,
        );
        let _reviewing = save(
            &mut backend,
            &kept,
            PerformanceSourceDecision::ReturnToReview,
        );
        let history = fixture::history(&mut backend);
        let projects = fixture::projects(&mut backend);
        let native = f.paths().native;
        let source_tree = fixture::tree(&native.join("captures"));
        let action_tree = fixture::tree(&native.join("source-actions"));
        let owned_tree = fixture::tree(&native.join("takes"));
        let archive = f.root.join("complete-recovered-source.osre");
        let exported = backend
            .store()
            .unwrap()
            .export_retained_evidence_full_root(&archive, OTHER, &budget())
            .unwrap();
        let inspected = inspect_evidence_archive(&archive, &budget()).unwrap();
        assert_eq!(inspected.receipt, exported);
        assert_eq!(exported.sha256, checksum(&fs::read(&archive).unwrap()));
        let restored = f.root.join("inert-evidence");
        backend
            .store()
            .unwrap()
            .restore_retained_evidence(&archive, &restored, &budget())
            .unwrap();
        for (prefix, tree) in [
            ("captures", &source_tree),
            ("source-actions", &action_tree),
            ("takes", &owned_tree),
        ] {
            for (relative, bytes) in tree {
                let path = format!("{prefix}/{}", relative.to_str().unwrap());
                let (index, entry) = inspected
                    .manifest
                    .entries
                    .iter()
                    .enumerate()
                    .find(|(_, e)| e.path == path)
                    .expect("every original/partial/sidecar/directory is inventoried");
                if let Some(bytes) = bytes {
                    assert_eq!(entry.kind, EvidenceEntryKind::File);
                    assert_eq!(
                        fs::read(restored.join("objects").join(format!("{index:04}.bin"))).unwrap(),
                        *bytes
                    );
                } else {
                    assert_eq!(entry.kind, EvidenceEntryKind::Directory);
                }
            }
        }
        let catalog_index = inspected
            .manifest
            .entries
            .iter()
            .position(|e| e.path == "catalog.sqlite3")
            .unwrap();
        let catalog = rusqlite::Connection::open_with_flags(
            restored
                .join("objects")
                .join(format!("{catalog_index:04}.bin")),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let review: Vec<u8> = catalog
            .query_row(
                "SELECT payload FROM performance_source_reviews WHERE id=?1",
                [ACTION],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(review, raw);
        let rows = catalog.prepare("SELECT id,revision,review_blob_sha256,payload,payload_sha256 FROM performance_source_actions ORDER BY id,revision").unwrap().query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,Vec<u8>>(3)?,r.get::<_,String>(4)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert_eq!(
            rows,
            history
                .revisions
                .iter()
                .map(|r| (
                    r.id.clone(),
                    r.revision as i64,
                    r.review_blob_sha256.clone(),
                    r.payload.clone(),
                    r.payload_sha256.clone()
                ))
                .collect::<Vec<_>>()
        );
        let operation = publication(&pending).operation_id.0.clone();
        let archived_operation: String = catalog
            .query_row(
                "SELECT operation_id FROM operations WHERE operation_id=?1",
                [&operation],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(archived_operation, operation);
        drop(catalog);
        assert!(!restored.join("studio.sqlite3").exists());
        assert!(!restored.join("captures").exists());
        assert!(!restored.join("source-actions").exists());
        assert_eq!(fixture::history(&mut backend), history);
        assert_eq!(fixture::projects(&mut backend), projects);
        assert_eq!(fixture::tree(&native.join("captures")), source_tree);
        assert_eq!(fixture::tree(&native.join("source-actions")), action_tree);
        assert_eq!(fixture::tree(&native.join("takes")), owned_tree);
    }
}
fn verified(
    mode: fixture::Mode,
) -> (
    fixture::Fixture,
    NativeBackend,
    PerformanceSourceReview,
    Vec<u8>,
    PerformanceSourceActionState,
) {
    let (fixture, mut backend, source, raw, prepared) = fixture::setup(mode);
    let state = fixture::execute(&mut backend, &prepared).unwrap().state;
    (fixture, backend, source, raw, state)
}
fn save(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
    decision: PerformanceSourceDecision,
) -> PerformanceSourceActionState {
    let Reply::PerformanceSourceDecision(state) = backend
        .save_source_decision(
            &state.prepared.action.id,
            state.revision,
            &state.payload_sha256,
            &decision,
        )
        .unwrap()
    else {
        panic!("decision")
    };
    *state
}
fn choose(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
    source: &PerformanceSourceReview,
    id: &str,
    supersedes: Option<&str>,
    frame: i64,
) -> PerformanceSourceActionState {
    let target = fixture::projects(backend).remove(0).0;
    let original = verified_result(state).unwrap();
    let choice = PerformanceSourceAttachmentChoice {
        id: id.into(),
        supersedes: supersedes.map(String::from),
        publication: TakePublication {
            operation_id: super::super::decisions::attachment_operation_id(
                &state.prepared.action.id,
                id,
            )
            .unwrap(),
            project_id: ProjectId(target.id),
            expected_revision: target.revision,
            track_id: TrackId(format!("attachment-track-{id}")),
            track_name: "Recovered original".into(),
            clip: TakeClip {
                id: format!("attachment-clip-{id}"),
                source: TakeReference {
                    take_id: TakeId(original.take_id.clone()),
                    manifest_sha256: original.manifest_sha256.clone(),
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 0,
                    count: source.performance.checkpoint.as_ref().unwrap().audio_end,
                },
                position: TimelinePosition {
                    frame,
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
}
fn publication(state: &PerformanceSourceActionState) -> &TakePublication {
    &state
        .decisions
        .attachment
        .as_ref()
        .unwrap()
        .choice
        .publication
}
fn request(state: &PerformanceSourceActionState) -> Request<'_> {
    let choice = state.decisions.attachment.as_ref().unwrap();
    Request {
        id: &state.prepared.action.id,
        revision: choice.revision,
        payload_sha256: &choice.payload_sha256,
    }
}
fn attach(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
) -> Result<PerformanceSourceAttachmentReceipt, ServiceError> {
    let choice = request(state);
    let Reply::PerformanceSourceAttachment(receipt) = backend.attach_source_choice(
        choice.id,
        choice.revision,
        choice.payload_sha256,
        fixture::token(),
    )?
    else {
        panic!("attachment")
    };
    Ok(*receipt)
}
fn reconcile(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
) -> Result<PerformanceSourceAttachmentReceipt, ServiceError> {
    let choice = request(state);
    let Reply::PerformanceSourceAttachment(receipt) = backend.reconcile_source_attachment(
        choice.id,
        choice.revision,
        choice.payload_sha256,
        fixture::token(),
    )?
    else {
        panic!("attachment reconciliation")
    };
    Ok(*receipt)
}
fn owned_path(
    backend: &mut NativeBackend,
    fixture: &fixture::Fixture,
    state: &PerformanceSourceActionState,
) -> PathBuf {
    let object = backend
        .store()
        .unwrap()
        .registered_take(&publication(state).clip.source.take_id)
        .unwrap();
    fixture
        .paths()
        .native
        .join("takes")
        .join(format!("{}.sqlite3", object.object_sha256))
}
fn rename_project(backend: &mut NativeBackend, id: &str, name: &str) -> ProjectDocument {
    let current = backend.store().unwrap().load(id).unwrap();
    let mut value = current.value().clone();
    value["name"] = name.into();
    let edited = ProjectDocument::from_value(value).unwrap();
    backend
        .store()
        .unwrap()
        .save(&edited, current.revision())
        .unwrap()
}
fn ingest_gap(backend: &mut NativeBackend, state: &PerformanceSourceActionState) -> ServiceError {
    backend
        .run_source_attachment(request(state), fixture::token(), true, |point, _| {
            if point == AttachmentBoundary::Ingested {
                Err(ServiceError::backend(
                    "Synthetic gap after retained ingestion",
                    true,
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err()
}

#[test]
fn actual_fault_tail_finish_sources_attach_full_selected_audio_and_keep_originals_through_undo_redo()
 {
    for mode in [
        fixture::Mode::Clean,
        fixture::Mode::Tail,
        fixture::Mode::Fault,
        fixture::Mode::Finish,
    ] {
        let (fixture, mut backend, source, raw, verified) = verified(mode);
        let source_tree = fixture::tree(fixture.source_path().parent().unwrap());
        let artifacts = fixture::tree(&fixture.action_parent());
        let state = choose(&mut backend, &verified, &source, CHOICE, None, 480);
        let receipt = attach(&mut backend, &state).unwrap();
        assert!(receipt.committed && !receipt.replayed);
        assert!(receipt.current_document_error.is_none());
        let current = &receipt.current_document.as_ref().unwrap().document;
        assert_eq!(current.revision(), 1);
        assert_eq!(
            current.take_clips().unwrap(),
            [publication(&state).clip.clone()]
        );
        assert_eq!(publication(&state).clip.position.frame, 480);
        assert_eq!(
            publication(&state).clip.range,
            TakeRange::AudioFrames {
                start: 0,
                count: source.performance.checkpoint.as_ref().unwrap().audio_end
            }
        );
        assert_eq!(receipt.state.execution, verified.execution);
        assert_eq!(fixture::history(&mut backend).reviews[0].payload, raw);
        let owned = owned_path(&mut backend, &fixture, &state);
        let working = fixture
            .action_parent()
            .join(ACTION)
            .join("recovery/take.sqlite3");
        let owned_bytes = fs::read(&owned).unwrap();
        assert_eq!(
            owned_bytes,
            fs::read(working).unwrap(),
            "owned ingestion is the full raw working image"
        );
        assert_eq!(
            checksum(&owned_bytes),
            owned.file_stem().unwrap().to_str().unwrap()
        );
        let sealed = backend
            .store()
            .unwrap()
            .open_take(&publication(&state).clip.source)
            .unwrap();
        assert!(sealed.manifest().body.interrupted.is_some());
        assert_eq!(
            sealed.inspect_snapshot().unwrap().original_rows_sha256,
            source.performance.original_rows_sha256
        );
        assert_eq!(sealed.descriptor().streams.len(), 6);
        drop(sealed);
        let undone = backend
            .store()
            .unwrap()
            .undo(&publication(&state).project_id.0, 1)
            .unwrap();
        assert!(undone.take_clips().unwrap().is_empty());
        let redone = backend
            .store()
            .unwrap()
            .redo(&publication(&state).project_id.0, undone.revision())
            .unwrap();
        assert_eq!(
            redone.take_clips().unwrap(),
            [publication(&state).clip.clone()]
        );
        let replay = reconcile(&mut backend, &state).unwrap();
        assert!(replay.committed && replay.replayed);
        assert_eq!(replay.current_document.unwrap().document, redone);
        assert_eq!(fs::read(owned).unwrap(), owned_bytes);
        assert_eq!(
            fixture::tree(fixture.source_path().parent().unwrap()),
            source_tree
        );
        assert_eq!(fixture::tree(&fixture.action_parent()), artifacts);
    }
}

#[test]
fn historical_receipt_replay_precedes_all_source_paths_and_preserves_current_document_failure() {
    let (fixture, mut backend, source, _, verified) = verified(fixture::Mode::Tail);
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 17);
    let first = attach(&mut backend, &state).unwrap();
    let later = rename_project(
        &mut backend,
        &publication(&state).project_id.0,
        "Later metadata is authoritative",
    );
    let replay = reconcile(&mut backend, &state).unwrap();
    assert_eq!(replay.current_document.unwrap().document, later);
    let owned = owned_path(&mut backend, &fixture, &state);
    let history = fixture::history(&mut backend);
    fs::remove_dir_all(fixture.source_path().parent().unwrap()).unwrap();
    fs::remove_dir_all(fixture.action_parent()).unwrap();
    fs::remove_file(owned).unwrap();
    drop(backend);
    let mut backend = fixture.backend();
    let files = fixture::tree(&fixture.paths().native);
    for execute in [false, true] {
        let observed = if execute {
            attach(&mut backend, &state)
        } else {
            reconcile(&mut backend, &state)
        }
        .unwrap();
        assert!(observed.committed && observed.replayed);
        assert_eq!(observed.state, first.state);
        assert_eq!(observed.publication, *publication(&state));
        assert!(observed.current_document.is_none());
        let error = observed.current_document_error.unwrap();
        assert_eq!(error.kind, FailureKind::SourceUnavailable);
    }
    assert_eq!(fixture::history(&mut backend), history);
    assert_eq!(fixture::tree(&fixture.paths().native), files);
    assert!(!fixture.action_parent().exists() && !fixture.source_path().exists());
}

#[test]
fn published_acknowledgement_gap_replays_one_atomic_publication_without_source_recovery() {
    let (fixture, mut backend, source, _, verified) = verified(fixture::Mode::Finish);
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 0);
    let error = backend
        .run_source_attachment(request(&state), fixture::token(), true, |point, _| {
            if point == AttachmentBoundary::Published {
                Err(ServiceError::backend(
                    "Synthetic lost publication acknowledgement",
                    true,
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert!(error.outcome_unknown);
    let before = fixture::history(&mut backend);
    assert_eq!(before.revisions.len(), 5);
    let document = backend
        .store()
        .unwrap()
        .load(&publication(&state).project_id.0)
        .unwrap();
    assert_eq!(document.take_clips().unwrap().len(), 1);
    assert!(
        backend
            .store()
            .unwrap()
            .take_publication_receipt(publication(&state))
            .unwrap()
            .is_some()
    );
    let replay = attach(&mut backend, &state).unwrap();
    assert!(replay.committed && replay.replayed);
    assert_eq!(replay.current_document.unwrap().document, document);
    assert_eq!(fixture::history(&mut backend), before);
    assert!(fixture.source_path().exists());
}

#[test]
fn owned_original_is_preferred_when_working_and_evidence_paths_are_gone() {
    let (fixture, mut backend, source, _, verified) = verified(fixture::Mode::Fault);
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 0);
    assert!(ingest_gap(&mut backend, &state).outcome_unknown);
    let owned = owned_path(&mut backend, &fixture, &state);
    let bytes = fs::read(&owned).unwrap();
    fs::remove_dir_all(fixture.action_parent().join(ACTION)).unwrap();
    fs::remove_dir_all(fixture.source_path().parent().unwrap()).unwrap();
    let lease_only = fixture::tree(&fixture.action_parent());
    let receipt = attach(&mut backend, &state).unwrap();
    assert!(receipt.committed && !receipt.replayed);
    assert_eq!(
        receipt
            .current_document
            .unwrap()
            .document
            .take_clips()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(fs::read(owned).unwrap(), bytes);
    assert_eq!(fixture::tree(&fixture.action_parent()), lease_only);
    assert!(!fixture.action_parent().join(ACTION).exists());
}

#[test]
fn damaged_or_different_registered_original_never_falls_back_to_valid_working_artifacts() {
    for different_manifest in [false, true] {
        let (fixture, mut backend, source, raw, verified) = verified(fixture::Mode::Tail);
        let state = choose(&mut backend, &verified, &source, CHOICE, None, 0);
        if different_manifest {
            let action = super::super::super::PreparedPerformanceSourceAction::prepare(
                OTHER.into(),
                &raw,
                OperationId("attachment-test.other-recovery".into()),
                None,
            )
            .unwrap();
            let Reply::PreparedPerformanceSourceAction(prepared) = backend
                .save_prepared_source_action(None, &action, &raw)
                .unwrap()
            else {
                panic!("other Prepared")
            };
            fixture::execute(&mut backend, &prepared).unwrap();
            let other = SealedJournal::open(
                fixture
                    .action_parent()
                    .join(OTHER)
                    .join("recovery/take.sqlite3"),
            )
            .unwrap();
            assert_ne!(
                other.manifest().sha256,
                publication(&state).clip.source.manifest_sha256
            );
            backend.store().unwrap().ingest_take(&other).unwrap();
        } else {
            ingest_gap(&mut backend, &state);
            let owned = owned_path(&mut backend, &fixture, &state);
            fs::OpenOptions::new()
                .append(true)
                .open(owned)
                .unwrap()
                .write_all(b"owned damage retained")
                .unwrap();
        }
        let before = fixture::history(&mut backend);
        let files = fixture::tree(&fixture.paths().native);
        let error = attach(&mut backend, &state).unwrap_err();
        assert_eq!(error.kind, FailureKind::SourceUnavailable);
        assert_eq!(fixture::history(&mut backend), before);
        assert_eq!(fixture::tree(&fixture.paths().native), files);
        assert!(
            backend
                .store()
                .unwrap()
                .load(&publication(&state).project_id.0)
                .unwrap()
                .take_clips()
                .unwrap()
                .is_empty()
        );
        backend.handle(&Command::Refresh).unwrap();
    }
}

#[test]
fn concurrent_retirement_or_keep_blocks_the_old_publisher_after_ingestion() {
    for retire in [false, true] {
        let (fixture, mut backend, source, _, verified) = verified(fixture::Mode::Clean);
        let state = choose(&mut backend, &verified, &source, CHOICE, None, 0);
        let originals = fixture::tree(fixture.source_path().parent().unwrap());
        let error = backend
            .run_source_attachment(request(&state), fixture::token(), true, |point, _| {
                if point == AttachmentBoundary::Ingested {
                    let mut concurrent = fixture.backend();
                    save(
                        &mut concurrent,
                        &state,
                        if retire {
                            PerformanceSourceDecision::RetireAttachment
                        } else {
                            PerformanceSourceDecision::KeepLocally
                        },
                    );
                }
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::RevisionConflict);
        assert!(
            error.outcome_unknown,
            "an owned original may already be retained"
        );
        assert!(
            backend
                .store()
                .unwrap()
                .has_registered_take(&publication(&state).clip.source.take_id)
                .unwrap()
        );
        assert!(
            backend
                .store()
                .unwrap()
                .take_publication_receipt(publication(&state))
                .unwrap()
                .is_none()
        );
        let document = backend
            .store()
            .unwrap()
            .load(&publication(&state).project_id.0)
            .unwrap();
        assert_eq!(document.revision(), 0);
        assert!(document.take_clips().unwrap().is_empty());
        let observed = reconcile(&mut backend, &state).unwrap();
        assert!(!observed.committed && observed.current_document.is_none());
        assert_eq!(observed.state.decisions.kept_locally, !retire);
        let files = fixture::tree(&fixture.paths().native);
        assert!(attach(&mut backend, &state).is_err());
        assert_eq!(fixture::tree(&fixture.paths().native), files);
        if !retire {
            let review = save(
                &mut backend,
                &observed.state,
                PerformanceSourceDecision::ReturnToReview,
            );
            let published = attach(&mut backend, &review).unwrap();
            assert!(published.committed);
        }
        assert_eq!(
            fixture::tree(fixture.source_path().parent().unwrap()),
            originals
        );
    }
}

#[test]
fn target_conflict_retains_owned_take_then_retirement_allows_an_explicit_new_choice() {
    let (fixture, mut backend, source, _, verified) = verified(fixture::Mode::Tail);
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 25);
    let edited = rename_project(
        &mut backend,
        &publication(&state).project_id.0,
        "Later project edit",
    );
    let before = fixture::history(&mut backend);
    let error = attach(&mut backend, &state).unwrap_err();
    assert_eq!(error.kind, FailureKind::RevisionConflict);
    assert!(error.outcome_unknown);
    assert_eq!(fixture::history(&mut backend), before);
    assert_eq!(
        backend
            .store()
            .unwrap()
            .load(&publication(&state).project_id.0)
            .unwrap(),
        edited
    );
    let owned = owned_path(&mut backend, &fixture, &state);
    let owned_bytes = fs::read(&owned).unwrap();
    let retired = save(
        &mut backend,
        &state,
        PerformanceSourceDecision::RetireAttachment,
    );
    assert!(attach(&mut backend, &state).is_err());
    let replacement = choose(&mut backend, &retired, &source, NEXT, Some(CHOICE), 999);
    assert_ne!(
        publication(&replacement).operation_id,
        publication(&state).operation_id
    );
    let receipt = attach(&mut backend, &replacement).unwrap();
    assert!(receipt.committed);
    let current = receipt.current_document.unwrap().document;
    assert_eq!(current.name(), edited.name());
    assert_eq!(current.revision(), 2);
    assert_eq!(
        current.take_clips().unwrap(),
        [publication(&replacement).clip.clone()]
    );
    assert_eq!(fs::read(owned).unwrap(), owned_bytes);
    assert!(
        backend
            .store()
            .unwrap()
            .take_publication_receipt(publication(&state))
            .unwrap()
            .is_none()
    );
    assert!(
        backend
            .store()
            .unwrap()
            .take_publication_receipt(publication(&replacement))
            .unwrap()
            .is_some()
    );
    assert!(fixture.source_path().exists());
}

#[test]
fn pending_reconciliation_is_read_only_even_when_every_artifact_is_unavailable() {
    let (fixture, mut backend, source, _, verified) = verified(fixture::Mode::Clean);
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 0);
    fs::remove_dir_all(fixture.source_path().parent().unwrap()).unwrap();
    fs::remove_dir_all(fixture.action_parent()).unwrap();
    let files = fixture::tree(&fixture.paths().native);
    let before = fixture::history(&mut backend);
    let receipt = reconcile(&mut backend, &state).unwrap();
    assert!(!receipt.committed && receipt.replayed);
    assert!(receipt.current_document.is_none() && receipt.current_document_error.is_none());
    assert_eq!(receipt.state, state);
    assert_eq!(fixture::history(&mut backend), before);
    assert_eq!(fixture::tree(&fixture.paths().native), files);
}

#[test]
fn attachment_cancellation_is_typed_local_and_post_ingestion_cancellation_keeps_the_owned_copy() {
    for after_ingest in [false, true] {
        let (fixture, mut backend, source, _, verified) = verified(fixture::Mode::Clean);
        let state = choose(&mut backend, &verified, &source, CHOICE, None, 0);
        let before = fixture::history(&mut backend);
        let project = fixture::projects(&mut backend);
        let cancel = fixture::token();
        if !after_ingest {
            cancel.store(true, Ordering::Release);
        }
        let error = backend
            .run_source_attachment(request(&state), Arc::clone(&cancel), true, |point, _| {
                if after_ingest && point == AttachmentBoundary::Ingested {
                    cancel.store(true, Ordering::Release);
                }
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::InspectionCancelled);
        assert_eq!(error.outcome_unknown, after_ingest);
        assert_eq!(fixture::history(&mut backend), before);
        assert_eq!(fixture::projects(&mut backend), project);
        assert_eq!(
            backend
                .store()
                .unwrap()
                .has_registered_take(&publication(&state).clip.source.take_id)
                .unwrap(),
            after_ingest
        );
        let receipt = attach(&mut backend, &state).unwrap();
        assert!(receipt.committed);
        assert!(fixture.source_path().exists());
    }
}

fn wait(service: &mut SessionService) -> Completion {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(completion) = service.try_poll().unwrap() {
            return completion;
        }
        assert!(Instant::now() < until, "bounded worker completion wait");
        std::thread::yield_now();
    }
}
struct FatalCurrentDocument {
    reply: Option<Reply>,
    calls: Arc<AtomicUsize>,
    retired: mpsc::Sender<()>,
}
impl Backend for FatalCurrentDocument {
    fn handle(&mut self, _command: &Command) -> Result<Reply, ServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .reply
            .take()
            .expect("the retired worker must never receive another request"))
    }
}
impl Drop for FatalCurrentDocument {
    fn drop(&mut self) {
        let _ = self.retired.send(());
    }
}

#[test]
fn structured_historical_commit_with_fatal_current_document_error_retires_the_session_worker() {
    let (_fixture, mut backend, source, _, verified) = verified(fixture::Mode::Clean);
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 0);
    attach(&mut backend, &state).unwrap();
    let mut receipt = reconcile(&mut backend, &state).unwrap();
    receipt.current_document = None;
    receipt.current_document_error = Some(ServiceError::storage_unavailable(
        "Synthetic main-catalog read failure after historical receipt",
        false,
    ));
    let calls = Arc::new(AtomicUsize::new(0));
    let (retired, retired_rx) = mpsc::channel();
    let mut service = SessionService::spawn_with_backend(FatalCurrentDocument {
        reply: Some(Reply::PerformanceSourceAttachment(Box::new(receipt))),
        calls: Arc::clone(&calls),
        retired,
    })
    .unwrap();
    let reference = request(&state);
    let id = service
        .try_submit(Command::ReconcilePerformanceSourceAttachment {
            id: reference.id.into(),
            revision: reference.revision,
            payload_sha256: reference.payload_sha256.into(),
        })
        .unwrap();
    let completion = wait(&mut service);
    assert_eq!(completion.id, id);
    let Reply::PerformanceSourceAttachment(receipt) = completion.result.unwrap() else {
        panic!("structured historical receipt")
    };
    assert!(receipt.committed);
    assert!(receipt.current_document.is_none());
    assert_eq!(
        receipt.current_document_error.unwrap().kind,
        FailureKind::StorageUnavailable
    );
    assert!(service.is_disconnected() && !service.is_busy());
    assert_eq!(
        service.try_submit(Command::Refresh).unwrap_err().kind,
        SubmitFailure::Disconnected
    );
    assert_eq!(
        service.try_poll().unwrap_err().kind,
        FailureKind::StorageUnavailable
    );
    retired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn actual_legacy_v1_checkpoint_attaches_as_recovered_without_manufacturing_finish() {
    use omniusong_core::contracts::{ClockEpochId, EndpointId};
    use omniusong_engine::{
        LiveClock, LiveId, LiveScope, LiveSourceBinding, LiveSourceKind,
        performance_records::{PerformanceConfig, PerformanceSource},
        performance_tap::TapInfo,
        performance_writer::PerformanceWriter,
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/performance-source-attachment-v1-tests")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root).unwrap();
    let fixture = fixture::Fixture {
        root: fs::canonicalize(root).unwrap(),
    };
    let mut backend = fixture.backend();
    backend
        .handle(&Command::Create {
            name: "Recovered v1 attachment target".into(),
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
            domain: "attachment-v1.gui".into(),
            epoch: "attachment-v1.epoch".into(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 1,
        },
        output_epoch: ClockEpochId("attachment-v1.output".into()),
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
            endpoint: EndpointId(format!("attachment-v1.source-{index}")),
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
    let source_tree = fixture::tree(path.parent().unwrap());
    let raw = serde_json::to_vec(&source).unwrap();
    let action = super::super::super::PreparedPerformanceSourceAction::prepare(
        ACTION.into(),
        &raw,
        OperationId("attachment-v1.recovery".into()),
        None,
    )
    .unwrap();
    let Reply::PreparedPerformanceSourceAction(prepared) = backend
        .save_prepared_source_action(None, &action, &raw)
        .unwrap()
    else {
        panic!("Prepared")
    };
    let verified = fixture::execute(&mut backend, &prepared).unwrap().state;
    let artifacts = fixture::tree(&fixture.action_parent());
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 10);
    let receipt = attach(&mut backend, &state).unwrap();
    assert!(receipt.committed);
    assert_eq!(
        receipt
            .current_document
            .unwrap()
            .document
            .take_clips()
            .unwrap(),
        [publication(&state).clip.clone()]
    );
    let sealed = backend
        .store()
        .unwrap()
        .open_take(&publication(&state).clip.source)
        .unwrap();
    assert!(sealed.manifest().body.interrupted.is_some());
    assert_eq!(
        sealed.inspect_snapshot().unwrap().original_rows_sha256,
        source.performance.original_rows_sha256
    );
    assert_eq!(sealed.descriptor().streams.len(), 6);
    assert_eq!(fixture::tree(path.parent().unwrap()), source_tree);
    assert_eq!(fixture::tree(&fixture.action_parent()), artifacts);
}
