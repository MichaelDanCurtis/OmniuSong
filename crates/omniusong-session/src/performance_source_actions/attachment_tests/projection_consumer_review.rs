//! Actual recovered-source attachment into authenticated projection metadata.
//! The same genuine owned performance is deliberately used in both roles.
//! No fabricated approval, second-writer coverage or native device claim.
use super::*;
use omniusong_core::{
    NoteTimelinePolicy,
    contracts::{EditOperation, EditScope, ProjectEditIntent},
    performance_projection::{ProjectionInstrument, ProjectionLaneChoice},
};
use omniusong_performance::projection::{
    AuthenticatedPerformance, PerformanceFrameRegion, PerformanceProjectionDuration,
    PerformanceProjectionPolicy, PerformanceProjectionRequest, PerformanceProjectionRounding,
};
use omniusong_recording::{InspectionBudget, InspectionLimits, SealedJournal};
use omniusong_store::{ProjectionPublication, projection_operation_id};
use rusqlite::{Connection, types::ValueRef};
use serde_json::Value;
use std::collections::BTreeMap;

fn budget() -> InspectionBudget {
    InspectionBudget::new(InspectionLimits::default(), fixture::token()).unwrap()
}

fn publish_initial_projection(
    f: &fixture::Fixture,
    backend: &mut NativeBackend,
    source: &PerformanceSourceReview,
    verified: &PerformanceSourceActionState,
) -> (ProjectDocument, PathBuf) {
    let history = fixture::history(backend);
    let expected = verified_result(verified).unwrap();
    let working = f.action_parent().join(ACTION).join("recovery/take.sqlite3");
    let sealed = SealedJournal::open(&working).unwrap();
    let object = backend.store().unwrap().ingest_take(&sealed).unwrap();
    assert_eq!(object.take_id.0, expected.take_id);
    assert_eq!(object.manifest_sha256, expected.manifest_sha256);
    drop(sealed);
    let reference = TakeReference {
        take_id: object.take_id,
        manifest_sha256: object.manifest_sha256,
        stream_id: "audio".into(),
    };
    let owned = f
        .paths()
        .native
        .join("takes")
        .join(format!("{}.sqlite3", object.object_sha256));
    let current = fixture::projects(backend).remove(0).0;
    let current = backend.store().unwrap().load(&current.id).unwrap();
    let governor = budget();
    let reader = backend
        .store()
        .unwrap()
        .open_take_with_budget(&reference, &governor)
        .unwrap();
    let held = AuthenticatedPerformance::open(&reader).unwrap();
    let request = PerformanceProjectionRequest {
        version: 1,
        source: held.source_pin().clone(),
        region: PerformanceFrameRegion {
            start: 0,
            end: source.performance.checkpoint.as_ref().unwrap().audio_end,
        },
        sources: vec![1],
        tempo_bits: current.tempo().to_bits(),
        destination_start_tick: 960,
        rounding: PerformanceProjectionRounding::NearestTiesUp,
        duration: PerformanceProjectionDuration::ReleaseGate,
        collapsed: PerformanceProjectionPolicy::Reject,
        incomplete: PerformanceProjectionPolicy::Reject,
        region_crossing: PerformanceProjectionPolicy::Reject,
    };
    let review = held.project(&request).unwrap();
    assert_eq!(
        review.notes.len(),
        1,
        "fixture must publish a real observed note"
    );
    assert_eq!(review.notes[0].values.velocity16, u16::MAX);
    let project_id = ProjectId(current.id().into());
    let publication = ProjectionPublication {
        version: 1,
        operation_id: projection_operation_id(&project_id, "source-attachment-original").unwrap(),
        project_id,
        expected_revision: current.revision(),
        projection_id: "source-attachment-original".into(),
        request,
        expected_review_sha256: review.review_sha256,
        lanes: vec![ProjectionLaneChoice {
            source: 1,
            instrument: ProjectionInstrument::Keys,
            track_id: "source-attachment-note-lane".into(),
            track_name: "Authenticated original notes".into(),
        }],
        timeline: NoteTimelinePolicy::PreserveLoop,
        allow_projection_schema_upgrade: true,
    };
    drop(held);
    drop(reader);
    backend
        .store()
        .unwrap()
        .publish_performance_projection(&publication, &budget())
        .unwrap();
    let projected = backend.store().unwrap().load(current.id()).unwrap();
    assert_eq!(projected.schema_version(), 3);
    assert_eq!(projected.revision(), current.revision() + 1);
    assert_eq!(
        projected.value()["performanceProjections"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fixture::history(backend),
        history,
        "projection does not manufacture source decisions"
    );
    (projected, owned)
}

fn typed_attachment(
    backend: &mut NativeBackend,
    state: &PerformanceSourceActionState,
    reconcile: bool,
) -> Result<PerformanceSourceAttachmentReceipt, ServiceError> {
    let exact = request(state);
    let command = if reconcile {
        Command::ReconcilePerformanceSourceAttachment {
            id: exact.id.into(),
            revision: exact.revision,
            payload_sha256: exact.payload_sha256.into(),
        }
    } else {
        Command::AttachPerformanceSource {
            id: exact.id.into(),
            revision: exact.revision,
            payload_sha256: exact.payload_sha256.into(),
        }
    };
    let Reply::PerformanceSourceAttachment(receipt) = backend.handle(&command)? else {
        panic!("typed source attachment reply")
    };
    assert_eq!(receipt.requested.id, exact.id);
    assert_eq!(receipt.requested.revision, exact.revision);
    assert_eq!(receipt.requested.payload_sha256, exact.payload_sha256);
    Ok(*receipt)
}

fn assert_registry(document: &ProjectDocument, registry: &Value) {
    assert_eq!(&document.value()["performanceProjections"], registry);
    assert_eq!(document.schema_version(), 3);
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Catalog {
    application: i64,
    version: i64,
    schema: Vec<Vec<Cell>>,
    tables: BTreeMap<String, Vec<Vec<Cell>>>,
}
// Independent cell/type oracle for this test's isolated live catalog. No raw
// file descriptor is opened on the catalog and no retained path is followed.
fn catalog(f: &fixture::Fixture) -> Catalog {
    let connection = Connection::open(f.paths().native.join("studio.sqlite3")).unwrap();
    let mut names = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        names.len(),
        13,
        "oracle covers every schema-6 application table"
    );
    names.push("sqlite_schema".into());
    let mut tables = BTreeMap::new();
    for name in names {
        let mut statement = connection
            .prepare(&format!(
                "SELECT * FROM \"{}\" NOT INDEXED",
                name.replace('"', "\"\"")
            ))
            .unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        let mut values = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            values.push(
                (0..columns)
                    .map(|column| match row.get_ref(column).unwrap() {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(value) => Cell::Integer(value),
                        ValueRef::Real(value) => Cell::Real(value.to_bits()),
                        ValueRef::Text(value) => Cell::Text(value.to_vec()),
                        ValueRef::Blob(value) => Cell::Blob(value.to_vec()),
                    })
                    .collect::<Vec<_>>(),
            );
        }
        values.sort();
        tables.insert(name, values);
    }
    Catalog {
        application: connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap(),
        version: connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap(),
        schema: tables.remove("sqlite_schema").unwrap(),
        tables,
    }
}

#[test]
fn actual_typed_recovered_attachment_preserves_authenticated_projection_edits_and_historical_commit()
 {
    let (f, mut backend, source, raw, verified) = verified(fixture::Mode::Finish);
    let source_tree = fixture::tree(f.source_path().parent().unwrap());
    let artifacts = fixture::tree(&f.action_parent());
    let (projected, owned) = publish_initial_projection(&f, &mut backend, &source, &verified);
    let registry = projected.value()["performanceProjections"].clone();
    let owned_bytes = fs::read(&owned).unwrap();
    let owned_tree = fixture::tree(&f.paths().native.join("takes"));
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 480);
    assert_eq!(publication(&state).expected_revision, projected.revision());
    let absent = typed_attachment(&mut backend, &state, true).unwrap();
    assert!(!absent.committed && absent.replayed && absent.current_document.is_none());
    let first = typed_attachment(&mut backend, &state, false).unwrap();
    assert!(first.committed && !first.replayed && first.current_document_error.is_none());
    assert_eq!(first.state.execution, verified.execution);
    let attached = first.current_document.as_ref().unwrap().document.clone();
    assert_eq!(attached.revision(), projected.revision() + 1);
    assert_registry(&attached, &registry);
    assert_eq!(
        attached.take_clips().unwrap(),
        [publication(&state).clip.clone()]
    );
    let history = fixture::history(&mut backend);
    assert_eq!(history.revisions.len(), 5);
    assert_eq!(history.reviews[0].payload, raw);
    let summary = backend.store().unwrap().list().unwrap().remove(0);
    assert_eq!(summary.problem, None);
    assert!(summary.source_authentication_required);

    let undone = backend
        .store()
        .unwrap()
        .undo(attached.id(), attached.revision())
        .unwrap();
    assert!(undone.take_clips().unwrap().is_empty());
    assert_registry(&undone, &registry);
    let redone = backend
        .store()
        .unwrap()
        .redo(undone.id(), undone.revision())
        .unwrap();
    assert_eq!(redone.take_clips().unwrap(), attached.take_clips().unwrap());
    assert_registry(&redone, &registry);
    assert_eq!(
        fixture::history(&mut backend),
        history,
        "Undo/Redo does not rewrite source commitments"
    );

    let edit = ProjectEditIntent {
        operation_id: OperationId("projection-attachment-later-human-name".into()),
        project_id: ProjectId(redone.id().into()),
        expected_revision: redone.revision(),
        scope: EditScope::Project,
        edit: EditOperation::Rename {
            name: "Later human metadata remains current".into(),
        },
        extensions: Default::default(),
    };
    let Reply::Document { view, .. } = backend.handle(&Command::Edit { intent: edit }).unwrap()
    else {
        panic!("human edit")
    };
    let later = view.document;
    assert_registry(&later, &registry);
    for reconcile in [false, true] {
        let replay = typed_attachment(&mut backend, &state, reconcile).unwrap();
        assert!(replay.committed && replay.replayed);
        assert_eq!(replay.state, first.state);
        assert_eq!(replay.publication, *publication(&state));
        assert_eq!(replay.current_document.unwrap().document, later);
    }
    assert_eq!(fs::read(&owned).unwrap(), owned_bytes);
    assert_eq!(
        fixture::tree(f.source_path().parent().unwrap()),
        source_tree
    );
    assert_eq!(fixture::tree(&f.action_parent()), artifacts);
    assert_eq!(fixture::tree(&f.paths().native.join("takes")), owned_tree);
    assert_eq!(fixture::history(&mut backend), history);

    // Destroy only this isolated fixture's availability. Historical commitment
    // must survive independently from its now-unusable current document.
    fs::remove_dir_all(f.source_path().parent().unwrap()).unwrap();
    fs::remove_dir_all(f.action_parent()).unwrap();
    fs::remove_file(&owned).unwrap();
    drop(backend);
    let mut backend = f.backend();
    let before = catalog(&f);
    for reconcile in [false, true] {
        let replay = typed_attachment(&mut backend, &state, reconcile).unwrap();
        assert!(replay.committed && replay.replayed);
        assert_eq!(replay.state, first.state);
        assert_eq!(replay.publication, *publication(&state));
        assert!(replay.current_document.is_none());
        assert_eq!(
            replay.current_document_error.unwrap().kind,
            FailureKind::SourceUnavailable
        );
    }
    assert_eq!(catalog(&f), before);
    assert_eq!(fixture::history(&mut backend), history);
    assert!(!owned.exists() && !f.source_path().exists() && !f.action_parent().exists());
}

#[test]
fn actual_projection_source_bridge_rejects_silent_trigger_corruption_of_full_catalog_postimage() {
    let (f, mut backend, source, _, verified) = verified(fixture::Mode::Finish);
    let (projected, owned) = publish_initial_projection(&f, &mut backend, &source, &verified);
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 240);
    let connection = Connection::open(f.paths().native.join("studio.sqlite3")).unwrap();
    connection.execute_batch(&format!("CREATE TRIGGER corrupt_projection_source_bridge AFTER INSERT ON performance_source_actions WHEN NEW.id='{ACTION}' AND NEW.revision=5 BEGIN UPDATE projects SET cursor=0; END;")).unwrap();
    let before = catalog(&f);
    let history = fixture::history(&mut backend);
    let source_tree = fixture::tree(f.source_path().parent().unwrap());
    let artifacts = fixture::tree(&f.action_parent());
    let owned_bytes = fs::read(&owned).unwrap();
    let error = typed_attachment(&mut backend, &state, false).unwrap_err();
    assert!(
        error.outcome_unknown,
        "an attempted mutating bridge remains explicit about uncertainty"
    );
    assert_eq!(
        catalog(&f),
        before,
        "all 13 tables, SQL types and schema roll back"
    );
    assert_eq!(fixture::history(&mut backend), history);
    assert_eq!(
        backend.store().unwrap().load(projected.id()).unwrap(),
        projected
    );
    assert!(
        backend
            .store()
            .unwrap()
            .take_publication_receipt(publication(&state))
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read(&owned).unwrap(), owned_bytes);
    assert_eq!(
        fixture::tree(f.source_path().parent().unwrap()),
        source_tree
    );
    assert_eq!(fixture::tree(&f.action_parent()), artifacts);
    connection
        .execute_batch("DROP TRIGGER corrupt_projection_source_bridge")
        .unwrap();
    drop(connection);
    let committed = typed_attachment(&mut backend, &state, false).unwrap();
    assert!(committed.committed && !committed.replayed);
    assert_registry(
        &committed.current_document.unwrap().document,
        &projected.value()["performanceProjections"],
    );
}

#[test]
fn actual_projection_source_bridge_fences_a_later_legal_whole_history_decision() {
    let (f, mut backend, source, _, verified) = verified(fixture::Mode::Finish);
    let (projected, owned) = publish_initial_projection(&f, &mut backend, &source, &verified);
    let state = choose(&mut backend, &verified, &source, CHOICE, None, 0);
    let owned_bytes = fs::read(&owned).unwrap();
    let mut kept = None;
    let mut after_competing_decision = None;
    // A deterministic boundary control, not a claim to execute a thread race:
    // the competing decision is real typed history on a separate connection.
    let error = backend
        .run_source_attachment(request(&state), fixture::token(), true, |point, _| {
            if point == AttachmentBoundary::Ingested {
                let mut concurrent = f.backend();
                kept = Some(save(
                    &mut concurrent,
                    &state,
                    PerformanceSourceDecision::KeepLocally,
                ));
                after_competing_decision = Some(catalog(&f));
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::RevisionConflict);
    assert!(error.outcome_unknown);
    assert_eq!(
        catalog(&f),
        after_competing_decision.unwrap(),
        "no publication may cross the changed whole history"
    );
    assert_eq!(
        backend.store().unwrap().load(projected.id()).unwrap(),
        projected
    );
    assert!(
        backend
            .store()
            .unwrap()
            .take_publication_receipt(publication(&state))
            .unwrap()
            .is_none()
    );
    let absent = typed_attachment(&mut backend, &state, true).unwrap();
    assert!(!absent.committed && absent.current_document.is_none());
    assert!(absent.state.decisions.kept_locally);
    assert!(typed_attachment(&mut backend, &state, false).is_err());
    let returned = save(
        &mut backend,
        &kept.unwrap(),
        PerformanceSourceDecision::ReturnToReview,
    );
    assert!(!returned.decisions.kept_locally);
    let committed = typed_attachment(&mut backend, &state, false).unwrap();
    assert!(committed.committed && !committed.replayed);
    let current = committed.current_document.unwrap().document;
    assert_eq!(current.revision(), projected.revision() + 1);
    assert_registry(&current, &projected.value()["performanceProjections"]);
    assert_eq!(fs::read(owned).unwrap(), owned_bytes);
}
