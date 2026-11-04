//! Actual session-worker archive routes over software-rendered, recovered
//! performance originals. No device, native UI, playback or inference claim.
use omniusong_core::{
    NoteTimelinePolicy, ProjectDocument,
    contracts::{
        ClockEpochId, EditOperation, EditScope, EndpointId, OperationId, ProjectEditIntent,
        ProjectId, TakeId,
    },
    native::TakeReference,
    performance_projection::{ProjectionInstrument, ProjectionLaneChoice},
};
use omniusong_engine::{
    LiveBatch, LiveClock, LiveEvent, LiveEventKind, LiveId, LivePatch, LiveScope,
    LiveSourceBinding, LiveSourceKind,
    live::{LiveAdmission, LiveRenderer, recording_live_pair},
    performance_records::{PerformanceConfig, PerformanceSource},
    performance_tap::PerformanceTapOwner,
    performance_writer::{
        PerformanceWriter, inspect_performance, inspect_sealed_performance, recover_performance,
    },
};
use omniusong_performance::projection::{
    AuthenticatedPerformance, PerformanceFrameRegion, PerformanceProjectionDuration,
    PerformanceProjectionPolicy, PerformanceProjectionRequest, PerformanceProjectionRounding,
};
use omniusong_recording::{CaptureReader, InspectionBudget, InspectionLimits, SealedJournal};
use omniusong_session::{
    Command, Paths, ProjectArchiveAction, ProjectArchiveCopyCompletion, ProjectArchiveOperation,
    ProjectArchiveOutcome, ProjectArchivePhase, ProjectArchiveProblemKind, ProjectArchiveReply,
    ProjectArchiveRequest, Reply, ServiceError, SessionService, SubmitFailure,
};
use omniusong_store::{
    NativeStore, ProjectionPublication, RestoreCopyIntent, projection_operation_id,
};
use rusqlite::{Connection, types::ValueRef};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

const GUARDED_SUMMARY: &str = "Performance projection ownership requires authenticated publication and archive validation on this path";

fn budget() -> InspectionBudget {
    InspectionBudget::new(
        InspectionLimits::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn primary(error: &omniusong_store::StoreError) -> &omniusong_store::StoreError {
    match error {
        omniusong_store::StoreError::Archive(failure) => primary(failure.operation_error()),
        omniusong_store::StoreError::ArchiveSql(failure) => primary(failure.operation_error()),
        omniusong_store::StoreError::Catalog(failure) => primary(failure.operation_error()),
        other => other,
    }
}
fn send(admission: &mut LiveAdmission, kind: LiveEventKind, ticks: u64) {
    let progress = admission.progress();
    let mut batch = LiveBatch::empty(
        progress.scope,
        progress.next_sequence().unwrap(),
        admission.lease_credit().map(|credit| credit.token),
    );
    batch
        .try_push(LiveEvent {
            source: 1,
            connection_generation: 1,
            observed_ticks: ticks,
            event: kind,
        })
        .unwrap();
    admission.admit(batch).unwrap();
}
fn render(
    renderer: &mut LiveRenderer,
    owner: &mut PerformanceTapOwner,
    writer: &mut PerformanceWriter,
    frames: usize,
) {
    renderer.render_into(&mut vec![0.0; frames]);
    while let Some(packet) = owner.try_pop().unwrap() {
        writer.push_packet(packet).unwrap();
    }
    writer.flush().unwrap();
}

struct Fixture {
    root: PathBuf,
    paths: Paths,
    staged: PathBuf,
    project: ProjectDocument,
    publication: ProjectionPublication,
}
impl Fixture {
    fn new(v2: bool) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/project-archive-service")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("staged")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let staged = root.join("staged/take.sqlite3");
        let paths = Paths {
            native: root.join("store"),
            legacy: root.join("legacy"),
        };
        let config = PerformanceConfig {
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            info: omniusong_engine::performance_tap::TapInfo {
                scope: LiveScope {
                    worker_generation: LiveId([71; 16]),
                    session_id: LiveId([72; 16]),
                    panic_generation: 1,
                },
                sample_rate: 8000,
                channels: 1,
            },
            gui_clock: LiveClock {
                domain: "archive-service.gui".into(),
                epoch: "archive-service.gui-epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            output_epoch: ClockEpochId("archive-service.render-epoch".into()),
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
                endpoint: EndpointId(format!("archive-service.source-{}", index + 1)),
            }),
        };
        let mut writer = if v2 {
            PerformanceWriter::create_finishable(&staged, config.clone())
        } else {
            PerformanceWriter::create(&staged, config.clone())
        }
        .unwrap();
        let (mut admission, mut renderer, mut owner) =
            recording_live_pair(config.info.scope, 8000, 1).unwrap();
        send(
            &mut admission,
            LiveEventKind::NoteOn {
                note: 1,
                voice: 1,
                key: 60,
                velocity16: 0,
                patch: LivePatch::Keys,
            },
            1,
        );
        render(&mut renderer, &mut owner, &mut writer, 80);
        send(
            &mut admission,
            LiveEventKind::NoteOff {
                voice: 1,
                release_velocity16: 65535,
            },
            2,
        );
        render(&mut renderer, &mut owner, &mut writer, 2000);
        if v2 {
            admission
                .begin_finish(admission.progress().scope, LiveId([73; 16]))
                .unwrap();
            render(&mut renderer, &mut owner, &mut writer, 64);
        }
        drop(renderer);
        writer.close_interrupted().unwrap();
        let reader = CaptureReader::open(&staged).unwrap();
        let inspection = inspect_performance(&reader).unwrap();
        assert!(inspection.problem.is_none() && inspection.selection.is_some());
        recover_performance(reader, &inspection, "archive-service.recovered").unwrap();
        let original = SealedJournal::open(&staged).unwrap();
        assert!(original.manifest().body.interrupted.is_some());
        let mut store = NativeStore::open(&paths.native).unwrap();
        let object = store.ingest_take(&original).unwrap();
        drop(original);
        let mut value = ProjectDocument::new("Actual service archive")
            .unwrap()
            .value()
            .clone();
        value["com.example.archive-service"] =
            json!({"unchanged":"π", "project_id":"opaque-not-remapped"});
        let project = store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap();
        // Derive the physical pin AFTER ingest's SQLite backup, from the actual
        // store-owned original rather than assuming staged and owned bytes match.
        let reference = TakeReference {
            take_id: object.take_id,
            manifest_sha256: object.manifest_sha256,
            stream_id: "audio".into(),
        };
        let governor = budget();
        let reader = store.open_take_with_budget(&reference, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let inspection = inspect_sealed_performance(&reader).unwrap();
        let request = PerformanceProjectionRequest {
            version: 1,
            source: held.source_pin().clone(),
            region: PerformanceFrameRegion {
                start: 0,
                end: inspection.checkpoint.unwrap().audio_end,
            },
            sources: vec![1],
            tempo_bits: project.tempo().to_bits(),
            destination_start_tick: 1000,
            rounding: PerformanceProjectionRounding::NearestTiesUp,
            duration: PerformanceProjectionDuration::ReleaseGate,
            collapsed: PerformanceProjectionPolicy::Reject,
            incomplete: PerformanceProjectionPolicy::Reject,
            region_crossing: PerformanceProjectionPolicy::Reject,
        };
        let review = held.project(&request).unwrap();
        assert_eq!(review.notes.len(), 1);
        assert_eq!(review.notes[0].values.velocity16, 0);
        let project_id = ProjectId(project.id().into());
        let publication = ProjectionPublication {
            version: 1,
            operation_id: projection_operation_id(&project_id, "service-projection").unwrap(),
            project_id,
            expected_revision: project.revision(),
            projection_id: "service-projection".into(),
            request,
            expected_review_sha256: review.review_sha256,
            lanes: vec![ProjectionLaneChoice {
                source: 1,
                instrument: ProjectionInstrument::Keys,
                track_id: "service-original-note-lane".into(),
                track_name: "Original zero velocity".into(),
            }],
            timeline: NoteTimelinePolicy::PreserveLoop,
            allow_projection_schema_upgrade: true,
        };
        drop(held);
        drop(reader);
        store
            .publish_performance_projection(&publication, &budget())
            .unwrap();
        let project = store.load(project.id()).unwrap();
        assert_eq!((project.schema_version(), project.revision()), (3, 1));
        drop(store);
        Self {
            root,
            paths,
            staged,
            project,
            publication,
        }
    }
    fn archive_request(
        &self,
        root: &Path,
        operation: ProjectArchiveOperation,
    ) -> ProjectArchiveRequest {
        ProjectArchiveRequest {
            version: 1,
            origin_root: root.into(),
            operation,
        }
    }
    fn service(&self, paths: Paths) -> SessionService {
        let expected_root = fs::canonicalize(&paths.native).unwrap();
        let mut service = SessionService::reconnect(paths).unwrap();
        // Initialize opens the store and reports structural metadata. The
        // required-authentication notice grants no source/archive authority.
        let Reply::Library(library) = call(&mut service, Command::Initialize).unwrap() else {
            panic!("Initialize returned another reply");
        };
        assert_eq!(library.native_root.as_ref(), Some(&expected_root));
        if expected_root == self.paths.native {
            let summary = library
                .sessions
                .iter()
                .find(|summary| summary.id == self.project.id())
                .unwrap();
            assert!(summary.problem.is_none());
            assert!(summary.source_authentication_required);
        } else {
            assert!(library.sessions.is_empty());
        }
        service
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Archive service fixture retained at {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn call(service: &mut SessionService, command: Command) -> Result<Reply, ServiceError> {
    let kind = command.kind();
    let id = service.try_submit(command).unwrap();
    assert!(service.is_busy());
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(completion) = service.try_poll().unwrap() {
            assert_eq!(completion.id, id);
            assert_eq!(completion.kind, kind);
            assert!(!service.is_busy());
            assert!(
                !service.cancel_project_archive(id),
                "a completed identity has no cancellation credit"
            );
            return completion.result;
        }
        assert!(
            Instant::now() < deadline,
            "actual archive worker did not reply"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn run(service: &mut SessionService, request: &ProjectArchiveRequest) -> ProjectArchiveReply {
    let Reply::ProjectArchive(reply) = call(
        service,
        Command::RunProjectArchive {
            request: Box::new(request.clone()),
        },
    )
    .unwrap() else {
        panic!("archive worker returned a document/library or another reply");
    };
    reply.validate_for(request).unwrap();
    assert!(reply.matches_request_reference(request));
    assert_eq!(reply.version, 1);
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"OmniuSong.ProjectArchiveRequest.v1\0");
    fingerprint.update(serde_json::to_vec(request).unwrap());
    assert_eq!(
        reply.request_sha256,
        format!("{:x}", fingerprint.finalize())
    );
    assert_eq!(reply.request_sha256, request.fingerprint().unwrap());
    assert!(
        serde_json::to_vec(&*reply).unwrap().len()
            <= omniusong_session::MAX_PROJECT_ARCHIVE_REPLY_BYTES
    );
    assert!(reply.catalog_retirement.is_none());
    assert!(!service.is_disconnected());
    assert!(service.retry_command().is_none());
    *reply
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
#[derive(Debug, PartialEq, Eq)]
struct Catalog {
    application: i64,
    version: i64,
    tables: BTreeMap<String, Vec<Vec<Cell>>>,
}
fn catalog(root: &Path) -> Catalog {
    let connection = Connection::open(root.join("studio.sqlite3")).unwrap();
    let mut tables = BTreeMap::new();
    let mut names = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names.push("sqlite_schema".into());
    for name in names {
        let mut statement = connection
            .prepare(&format!(
                "SELECT * FROM \"{}\" NOT INDEXED",
                name.replace('"', "\"\"")
            ))
            .unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        let mut cells = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            cells.push(
                (0..columns)
                    .map(|column| match row.get_ref(column).unwrap() {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(v) => Cell::Integer(v),
                        ValueRef::Real(v) => Cell::Real(v.to_bits()),
                        ValueRef::Text(v) => Cell::Text(v.to_vec()),
                        ValueRef::Blob(v) => Cell::Blob(v.to_vec()),
                    })
                    .collect::<Vec<_>>(),
            );
        }
        cells.sort();
        tables.insert(name, cells);
    }
    Catalog {
        application: connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap(),
        version: connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap(),
        tables,
    }
}

fn originals(root: &Path) -> BTreeMap<String, (String, String, Vec<u8>)> {
    let connection = Connection::open(root.join("studio.sqlite3")).unwrap();
    let mut statement = connection.prepare("SELECT take_id,manifest_sha256,object_sha256,byte_length,manifest_json FROM native_takes ORDER BY take_id").unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .into_iter()
        .map(|(id, manifest, object, length, manifest_json)| {
            let bytes = fs::read(root.join("takes").join(format!("{object}.sqlite3"))).unwrap();
            assert_eq!(bytes.len() as i64, length);
            assert_eq!(sha(&bytes), object);
            (id, (manifest, manifest_json, bytes))
        })
        .collect()
}

fn staged_bytes(fixture: &Fixture) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(fixture.staged.parent().unwrap())
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_str().unwrap().into(),
                fs::read(path).unwrap(),
            )
        })
        .collect()
}

fn rename(document: &ProjectDocument, id: &str, name: &str) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        scope: EditScope::Project,
        edit: EditOperation::Rename { name: name.into() },
        extensions: Default::default(),
    }
}

fn destination_paths(fixture: &Fixture, name: &str) -> Paths {
    let paths = Paths {
        native: fixture.root.join(name),
        legacy: fixture.root.join(format!("{name}-legacy")),
    };
    drop(NativeStore::open(&paths.native).unwrap());
    paths
}
fn assert_cleanup(cleanup: &omniusong_session::ProjectArchiveCleanup) {
    assert!(cleanup.issues.is_empty());
    assert_eq!(cleanup.omitted, 0);
}
fn assert_other_references_refuse(
    reply: &ProjectArchiveReply,
    request: &ProjectArchiveRequest,
    other_root: &Path,
) {
    let mut other = request.clone();
    other.origin_root = other_root.into();
    other.validate().unwrap();
    assert!(!reply.matches_request_reference(&other));
    assert!(reply.validate_for(&other).is_err());
    let mut other = request.clone();
    other.operation = ProjectArchiveOperation::RestoreIdentity {
        archive_path: other_root.join("another.osar"),
    };
    other.validate().unwrap();
    assert!(!reply.matches_request_reference(&other));
    assert!(reply.validate_for(&other).is_err());
    let mut other = request.clone();
    match &mut other.operation {
        ProjectArchiveOperation::Export { destination, .. } => {
            *destination = other_root.join("different-output.osar")
        }
        ProjectArchiveOperation::RestoreIdentity { archive_path } => {
            *archive_path = other_root.join("different-input.osar")
        }
        ProjectArchiveOperation::RestoreCopy { intent, .. } => {
            intent.operation_id = OperationId("different-copy-reference".into());
            intent.expected_archive_sha256 = "f".repeat(64);
        }
    }
    other.validate().unwrap();
    assert!(!reply.matches_request_reference(&other));
    assert!(reply.validate_for(&other).is_err());
}

#[test]
fn actual_v1_v2_published_projection_uses_all_three_compact_archive_commands_on_real_workers() {
    for v2 in [false, true] {
        let f = Fixture::new(v2);
        let before = catalog(&f.paths.native);
        let owned = originals(&f.paths.native);
        let staged = staged_bytes(&f);
        let store = NativeStore::open_existing(&f.paths.native).unwrap();
        let legacy_output = f.root.join("legacy-must-stay-closed.osar");
        let refusal = store
            .export_archive(f.project.id(), &legacy_output)
            .unwrap_err();
        assert!(
            matches!(primary(&refusal), omniusong_store::StoreError::Invalid(message)
            if message == GUARDED_SUMMARY)
        );
        assert!(!legacy_output.exists());
        drop(store);
        let mut source = f.service(f.paths.clone());
        let archive = f.root.join("real-service.osar");
        let request = f.archive_request(
            &f.paths.native,
            ProjectArchiveOperation::Export {
                project_id: ProjectId(f.project.id().into()),
                destination: archive.clone(),
            },
        );
        let reply = run(&mut source, &request);
        assert!(reply.invoked);
        assert_other_references_refuse(&reply, &request, &f.root);
        let ProjectArchiveOutcome::Exported { receipt, cleanup } = reply.outcome else {
            panic!("real export did not return its compact durable receipt");
        };
        assert_cleanup(&cleanup);
        assert_eq!(receipt.path, archive);
        assert_eq!(receipt.project_id.0, f.project.id());
        assert_eq!(receipt.sha256, sha(&fs::read(&archive).unwrap()));
        assert_eq!(receipt.byte_length, fs::metadata(&archive).unwrap().len());
        assert_eq!(receipt.history_rows, before.tables["edits"].len() as u64);
        assert_eq!(
            receipt.operation_rows,
            before.tables["operations"].len() as u64
        );
        assert_eq!((receipt.asset_count, receipt.take_count), (0, 1));
        assert_eq!(catalog(&f.paths.native), before);
        assert_eq!(originals(&f.paths.native), owned);
        let identity_paths = destination_paths(&f, "identity");
        let mut identity = f.service(identity_paths.clone());
        let request = f.archive_request(
            &identity_paths.native,
            ProjectArchiveOperation::RestoreIdentity {
                archive_path: archive.clone(),
            },
        );
        let reply = run(&mut identity, &request);
        assert!(reply.invoked);
        assert_other_references_refuse(&reply, &request, &f.root);
        let ProjectArchiveOutcome::RestoredIdentity { project, cleanup } = reply.outcome else {
            panic!("real identity restore returned no compact commitment");
        };
        assert_cleanup(&cleanup);
        assert_eq!(project.project_id.0, f.project.id());
        assert_eq!(project.revision, f.project.revision());
        // Independent post-operation assertions belong to this test, not to
        // the backend's completion path. No failure of a later load is claimed.
        let restored = NativeStore::open_existing(&identity_paths.native).unwrap();
        assert_eq!(restored.load(f.project.id()).unwrap(), f.project);
        assert_eq!(originals(&identity_paths.native), owned);
        let summary = restored.list().unwrap().remove(0);
        assert!(summary.problem.is_none());
        assert!(summary.source_authentication_required);
        drop(restored);
        let copy_paths = destination_paths(&f, "copy");
        let mut copy = f.service(copy_paths.clone());
        let intent = RestoreCopyIntent {
            operation_id: OperationId("real-service-copy".into()),
            expected_archive_sha256: receipt.sha256.clone(),
        };
        let request = f.archive_request(
            &copy_paths.native,
            ProjectArchiveOperation::RestoreCopy {
                archive_path: archive.clone(),
                intent: intent.clone(),
            },
        );
        let reply = run(&mut copy, &request);
        assert!(reply.invoked);
        assert_other_references_refuse(&reply, &request, &f.root);
        let ProjectArchiveOutcome::RestoredCopy {
            operation_id,
            source_project_id,
            source_archive_sha256,
            project,
            completion,
            cleanup,
        } = reply.outcome
        else {
            panic!("real copy returned no compact commitment");
        };
        assert_cleanup(&cleanup);
        assert_eq!(completion, ProjectArchiveCopyCompletion::CatalogCommitted);
        assert_eq!(operation_id, intent.operation_id);
        assert_eq!(source_project_id.0, f.project.id());
        assert_eq!(source_archive_sha256, receipt.sha256);
        assert_ne!(project.project_id.0, f.project.id());
        assert_eq!(project.revision, f.project.revision());
        let copied_store = NativeStore::open_existing(&copy_paths.native).unwrap();
        let copied = copied_store.load(&project.project_id.0).unwrap();
        let mut normalized = copied.value().clone();
        normalized["id"] = f.project.id().into();
        assert_eq!(ProjectDocument::from_value(normalized).unwrap(), f.project);
        assert_eq!(
            copied.performance_projections().unwrap(),
            f.project.performance_projections().unwrap()
        );
        let expected_operation =
            projection_operation_id(&project.project_id, &f.publication.projection_id).unwrap();
        let connection = Connection::open(copy_paths.native.join("studio.sqlite3")).unwrap();
        let actual_request: String = connection
            .query_row(
                "SELECT request_json FROM operations WHERE operation_id=?1 AND project_id=?2",
                [&expected_operation.0, &project.project_id.0],
                |row| row.get(0),
            )
            .unwrap();
        let mut expected_publication = f.publication.clone();
        expected_publication.project_id = project.project_id;
        expected_publication.operation_id = expected_operation;
        assert_eq!(
            actual_request,
            expected_publication.canonical_json().unwrap()
        );
        assert!(
            connection
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query([])
                .unwrap()
                .next()
                .unwrap()
                .is_none()
        );
        drop(connection);
        assert_eq!(originals(&copy_paths.native), owned);
        assert_eq!(catalog(&f.paths.native), before);
        assert_eq!(originals(&f.paths.native), owned);
        assert_eq!(staged_bytes(&f), staged);
        assert_eq!(sha(&fs::read(&archive).unwrap()), receipt.sha256);
    }
}

#[test]
fn actual_copy_replay_after_input_removal_returns_recorded_revision_without_reactivating_old_document()
 {
    let f = Fixture::new(true);
    let before_source = catalog(&f.paths.native);
    let owned = originals(&f.paths.native);
    let staged = staged_bytes(&f);
    let mut source = f.service(f.paths.clone());
    let archive = f.root.join("replay-source.osar");
    let export_request = f.archive_request(
        &f.paths.native,
        ProjectArchiveOperation::Export {
            project_id: ProjectId(f.project.id().into()),
            destination: archive.clone(),
        },
    );
    let ProjectArchiveOutcome::Exported { receipt, .. } = run(&mut source, &export_request).outcome
    else {
        panic!("actual export failed");
    };
    let copy_paths = destination_paths(&f, "replay-copy");
    let mut worker = f.service(copy_paths.clone());
    let request = f.archive_request(
        &copy_paths.native,
        ProjectArchiveOperation::RestoreCopy {
            archive_path: archive.clone(),
            intent: RestoreCopyIntent {
                operation_id: OperationId("retained-service-copy".into()),
                expected_archive_sha256: receipt.sha256.clone(),
            },
        },
    );
    let first = run(&mut worker, &request);
    let ProjectArchiveOutcome::RestoredCopy {
        project,
        completion,
        ..
    } = &first.outcome
    else {
        panic!("actual fresh copy failed");
    };
    assert_eq!(*completion, ProjectArchiveCopyCompletion::CatalogCommitted);
    let mut store = NativeStore::open_existing(&copy_paths.native).unwrap();
    let copied = store.load(&project.project_id.0).unwrap();
    let edited = store
        .apply_edit(&rename(
            &copied,
            "human-after-copy",
            "Keep this later human name",
        ))
        .unwrap()
        .document;
    assert_eq!(edited.revision(), project.revision + 1);
    let expected = catalog(&copy_paths.native);
    let expected_originals = originals(&copy_paths.native);
    drop(store);
    fs::remove_file(&archive).unwrap();
    assert!(!archive.exists());
    let replay = run(&mut worker, &request);
    assert!(replay.invoked);
    let ProjectArchiveOutcome::RestoredCopy {
        operation_id,
        source_project_id,
        source_archive_sha256,
        project: replayed,
        completion,
        cleanup,
    } = replay.outcome
    else {
        panic!("exact copy replay did not report its recorded fact");
    };
    assert_cleanup(&cleanup);
    assert_eq!(completion, ProjectArchiveCopyCompletion::RecordedReplay);
    assert_eq!(operation_id.0, "retained-service-copy");
    assert_eq!(source_project_id.0, f.project.id());
    assert_eq!(source_archive_sha256, receipt.sha256);
    assert_eq!(&replayed, project);
    assert_eq!(
        catalog(&copy_paths.native),
        expected,
        "replay allocates no new IDs or history"
    );
    assert_eq!(originals(&copy_paths.native), expected_originals);
    let store = NativeStore::open_existing(&copy_paths.native).unwrap();
    assert_eq!(store.load(&project.project_id.0).unwrap(), edited);
    assert_eq!(catalog(&f.paths.native), before_source);
    assert_eq!(originals(&f.paths.native), owned);
    assert_eq!(staged_bytes(&f), staged);
}

#[test]
fn real_worker_refuses_wrong_origin_before_invocation_and_keeps_exact_good_request_usable() {
    let f = Fixture::new(false);
    let before = catalog(&f.paths.native);
    let owned = originals(&f.paths.native);
    let staged = staged_bytes(&f);
    let mut worker = f.service(f.paths.clone());
    let output = f.root.join("exact-root.osar");
    let wrong = f.archive_request(
        &f.root,
        ProjectArchiveOperation::Export {
            project_id: ProjectId(f.project.id().into()),
            destination: output.clone(),
        },
    );
    let rejected = run(&mut worker, &wrong);
    assert!(!rejected.invoked);
    let ProjectArchiveOutcome::Failed { context, problem } = rejected.outcome else {
        panic!("wrong root was admitted");
    };
    assert_eq!(context.action, ProjectArchiveAction::Export);
    assert_eq!(context.phase, ProjectArchivePhase::NotStarted);
    assert_eq!(context.destination, output);
    assert!(
        context.staging.is_none() && context.source.is_none() && context.operation_id.is_none()
    );
    assert!(context.archive_sha256.is_none() && context.expected_archive_sha256.is_none());
    assert_eq!(context.confirmed_objects, 0);
    assert_eq!(problem.kind, ProjectArchiveProblemKind::Rejected);
    assert_eq!(
        problem.detail.text,
        "Project archive origin does not match the open store"
    );
    assert!(!problem.detail.truncated);
    assert!(problem.sqlite.is_none() && problem.governor_stop.is_none());
    assert!(!output.exists());
    assert_eq!(catalog(&f.paths.native), before);
    assert_eq!(originals(&f.paths.native), owned);
    let request = f.archive_request(
        &f.paths.native,
        ProjectArchiveOperation::Export {
            project_id: ProjectId(f.project.id().into()),
            destination: output.clone(),
        },
    );
    let mut alias = request.clone();
    alias.origin_root = f.paths.native.join(".");
    let refused = worker
        .try_submit(Command::RunProjectArchive {
            request: Box::new(alias),
        })
        .unwrap_err();
    assert_eq!(refused.kind, SubmitFailure::Invalid);
    assert!(!worker.is_busy() && !output.exists());
    let reply = run(&mut worker, &request);
    assert!(reply.invoked);
    let ProjectArchiveOutcome::Exported { receipt, cleanup } = reply.outcome else {
        panic!("correct root did not remain usable after refusals");
    };
    assert_cleanup(&cleanup);
    assert_eq!(receipt.path, output);
    assert_eq!(receipt.sha256, sha(&fs::read(&output).unwrap()));
    assert_eq!(catalog(&f.paths.native), before);
    assert_eq!(originals(&f.paths.native), owned);
    assert_eq!(staged_bytes(&f), staged);
}
