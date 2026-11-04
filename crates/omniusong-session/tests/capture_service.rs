use omniusong_core::{
    ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_session::{Command, Paths, Reply, SessionService};
use omniusong_store::{NativeStore, TakePublication};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

fn call(service: &mut SessionService, command: Command) -> Reply {
    service.try_submit(command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(reply) = service.try_poll().unwrap() {
            return reply.result.unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn capture_attachment_routes_through_storage_worker_and_retry_returns_latest_session() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/capture-service")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root).unwrap();
    let paths = Paths {
        native: root.join("store"),
        legacy: root.join("legacy"),
    };
    let mut store = NativeStore::open(&paths.native).unwrap();
    let project = store
        .create(&ProjectDocument::new("Capture target").unwrap())
        .unwrap();
    drop(store);
    let descriptor:TakeDescriptor=serde_json::from_value(serde_json::json!({"version":1,"take_id":"test-capture","streams":[{"stream_id":"audio","segment_id":"segment","source_id":"synthetic","source_name":"Synthetic service fixture","source_connection_generation":1,"host_clock":{"domain":"host","epoch":"epoch","ticks_per_second_numerator":1000000000u64,"ticks_per_second_denominator":1},"kind":{"kind":"audio","format":{"sample_rate":48000,"channels":1,"encoding":"f32_le","layout":"interleaved"},"audio_epoch":"epoch"},"initial_source_position":0}],"provenance":{}})).unwrap();
    let path = root.join("capture.sqlite3");
    let mut journal = Journal::create(&path, &descriptor).unwrap();
    let stamp = HostTimestamp {
        domain: ClockDomainId("host".into()),
        epoch: ClockEpochId("epoch".into()),
        ticks: 0,
    };
    journal
        .append(
            &ChunkHeader {
                version: 1,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                sequence: 0,
                range: SourceRange::AudioFrames { start: 0, count: 4 },
                interpretation: descriptor.streams[0].kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            },
            &[0; 16],
        )
        .unwrap();
    let manifest = journal.seal("finish").unwrap();
    drop(journal);
    let publication = TakePublication {
        operation_id: OperationId("same-capture-admission".into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: 0,
        track_id: TrackId("take-track".into()),
        track_name: "Original take".into(),
        clip: TakeClip {
            edit_source: None,
            id: "take-clip".into(),
            source: TakeReference {
                take_id: descriptor.take_id,
                manifest_sha256: manifest.sha256,
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames { start: 0, count: 4 },
            position: TimelinePosition {
                frame: 0,
                sample_rate: 48000,
            },
            fades: None,
        },
    };
    let command = Command::AttachCapture {
        journal_path: path.clone(),
        publication,
    };
    assert!(command.retry_safe() && command.mutates());
    let mut service = SessionService::reconnect(paths.clone()).unwrap();
    call(&mut service, Command::Initialize);
    let attached = match call(&mut service, command.clone()) {
        Reply::Document {
            view,
            replayed: false,
            ..
        } => view.document,
        _ => panic!("Missing capture publication"),
    };
    assert_eq!(attached.revision(), 1);
    let edit = ProjectEditIntent {
        operation_id: OperationId("later-name".into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: 1,
        scope: EditScope::Project,
        edit: EditOperation::Rename {
            name: "Later saved name".into(),
        },
        extensions: Default::default(),
    };
    call(&mut service, Command::Edit { intent: edit });
    fs::remove_file(path).unwrap();
    match call(&mut service, command) {
        Reply::Document {
            view,
            replayed: true,
            ..
        } => {
            assert_eq!(view.document.name(), "Later saved name");
            assert_eq!(view.document.revision(), 2);
            assert_eq!(view.document.tracks().len(), attached.tracks().len());
        }
        _ => panic!("Missing safe replay"),
    }
    // A service drop is nonblocking; all originals remain in the owned store.
    drop(service);
    let store = NativeStore::open_existing(&paths.native).unwrap();
    assert_eq!(store.load(project.id()).unwrap().revision(), 2);
    let _ = fs::remove_dir_all(root);
}
