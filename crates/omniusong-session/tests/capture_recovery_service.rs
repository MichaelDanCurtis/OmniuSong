//! Actual background-service recovery flow. Synthetic PCM only; no devices.
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_session::{Command, FailureKind, Paths, Reply, ServiceError, SessionService};
use omniusong_store::{CaptureStageState, NativeStore, TakePublication};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/capture-recovery-service")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn paths(&self) -> Paths {
        Paths {
            native: self.0.join("store"),
            legacy: self.0.join("legacy"),
        }
    }
    fn journal(&self) -> (String, Journal) {
        let stage = uuid::Uuid::new_v4().to_string();
        let path = self
            .paths()
            .native
            .join("captures")
            .join(&stage)
            .join("take.sqlite3");
        let descriptor:TakeDescriptor=serde_json::from_value(serde_json::json!({
            "version":1,"take_id":"service-retained-original","streams":[{
                "stream_id":"audio","segment_id":"segment","source_id":"synthetic",
                "source_name":"Software-only recovery fixture","source_connection_generation":1,
                "host_clock":{"domain":"host","epoch":"epoch","ticks_per_second_numerator":1000000000u64,"ticks_per_second_denominator":1},
                "kind":{"kind":"audio","format":{"sample_rate":48000,"channels":1,"encoding":"f32_le","layout":"interleaved"},"audio_epoch":"epoch"},
                "initial_source_position":0}],"provenance":{"test":"synthetic; no device"}})).unwrap();
        let mut journal = Journal::create(path, &descriptor).unwrap();
        let stamp = HostTimestamp {
            domain: ClockDomainId("host".into()),
            epoch: ClockEpochId("epoch".into()),
            ticks: 0,
        };
        journal
            .append(
                &ChunkHeader {
                    version: 1,
                    take_id: descriptor.take_id,
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
        (stage, journal)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn call(
    service: &mut SessionService,
    command: Command,
) -> std::result::Result<Reply, ServiceError> {
    service.try_submit(command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(c) = service.try_poll().unwrap() {
            return c.result;
        }
        assert!(
            Instant::now() < deadline,
            "background service did not reply"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn retained_capture_is_reviewed_recovered_retried_attached_and_archived_on_real_worker() {
    let f = Fixture::new();
    let paths = f.paths();
    let mut store = NativeStore::open(&paths.native).unwrap();
    let project = store
        .create(&ProjectDocument::new("Explicit recovery target").unwrap())
        .unwrap();
    drop(store);
    let mut service = SessionService::reconnect(paths.clone()).unwrap();
    call(&mut service, Command::Initialize).unwrap();
    let Reply::CaptureStages(empty) = call(&mut service, Command::ScanCaptures).unwrap() else {
        panic!("wrong scan reply")
    };
    assert!(empty.stages.is_empty());
    assert!(!paths.native.join("captures").exists());
    let (stage, journal) = f.journal();
    let journal_path = journal.path().to_path_buf();
    let failure = call(
        &mut service,
        Command::InspectCapture {
            stage_id: stage.clone(),
        },
    )
    .unwrap_err();
    assert_eq!(failure.kind, FailureKind::Backend);
    assert!(!service.is_disconnected());
    drop(journal);
    let original = fs::read(&journal_path).unwrap();
    let Reply::CaptureInspected(review) = call(
        &mut service,
        Command::InspectCapture {
            stage_id: stage.clone(),
        },
    )
    .unwrap() else {
        panic!("wrong inspect reply")
    };
    assert_eq!(review.state, CaptureStageState::Recording);
    assert!(review.unknown_tail);
    assert_eq!(review.audio[0].frames, 4);
    assert_eq!(
        fs::read(&journal_path).unwrap(),
        original,
        "review changed journal bytes"
    );
    let recover = Command::RecoverCapture {
        inspection: review,
        operation_id: OperationId("explicit-recover".into()),
    };
    assert!(recover.mutates() && recover.retry_safe());
    let Reply::CaptureRecovered(receipt) = call(&mut service, recover.clone()).unwrap() else {
        panic!("wrong recovery reply")
    };
    assert!(receipt.unknown_tail && !receipt.replayed);
    let Reply::CaptureRecovered(replay) = call(&mut service, recover).unwrap() else {
        panic!("wrong replay reply")
    };
    assert!(replay.replayed);
    assert_eq!(receipt.manifest_sha256, replay.manifest_sha256);
    let audio = &receipt.audio[0];
    let publication = TakePublication {
        operation_id: OperationId("attach-recovered-once".into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: 0,
        track_id: TrackId("recovered-track".into()),
        track_name: "Recovered software original".into(),
        clip: TakeClip {
            edit_source: None,
            id: "recovered-clip".into(),
            source: TakeReference {
                take_id: TakeId(receipt.take_id.clone()),
                manifest_sha256: receipt.manifest_sha256.clone(),
                stream_id: audio.stream_id.clone(),
            },
            range: TakeRange::AudioFrames {
                start: audio.start_frame,
                count: audio.frames,
            },
            position: TimelinePosition {
                frame: 0,
                sample_rate: audio.sample_rate,
            },
            fades: None,
        },
    };
    let Reply::Document { view, .. } = call(
        &mut service,
        Command::AttachCapture {
            journal_path: journal_path.clone(),
            publication: publication.clone(),
        },
    )
    .unwrap() else {
        panic!("wrong attachment reply")
    };
    assert_eq!(view.document.revision(), 1);
    let destination = f.0.join("recovered.omniusong-native");
    let Reply::Exported(_) = call(
        &mut service,
        Command::ExportArchive {
            project_id: ProjectId(project.id().into()),
            destination: destination.clone(),
        },
    )
    .unwrap() else {
        panic!("wrong archive reply")
    };
    let restored_path = f.0.join("restored");
    let mut restored = NativeStore::open(&restored_path).unwrap();
    let restored_project = restored.restore_archive(&destination).unwrap();
    assert_eq!(restored_project.tracks(), view.document.tracks());
    let source = restored.open_take(&publication.clip.source).unwrap();
    assert_eq!(source.read_chunk("audio", 0).unwrap().payload, vec![0; 16]);
    assert!(source.manifest().body.interrupted.is_some());
    let reread = Journal::open(&journal_path).unwrap();
    assert_eq!(reread.state().unwrap(), TakeState::Sealed);
    assert!(reread.recovery_report().unwrap().unknown_tail);
}

#[test]
fn invalid_stage_database_never_retires_the_main_store_generation() {
    let f = Fixture::new();
    let paths = f.paths();
    let store = NativeStore::open(&paths.native).unwrap();
    drop(store);
    let (stage, journal) = f.journal();
    let path = journal.path().to_path_buf();
    drop(journal);
    let mut bytes = fs::read(&path).unwrap();
    // Keep the SQLite/rollback header and owner sidecar valid, then corrupt the
    // first B-tree page. This reaches a typed journal SQLITE_CORRUPT, rather
    // than only exercising an early missing-sidecar/header rejection.
    bytes[100] = 0xff;
    fs::write(&path, &bytes).unwrap();
    let store = NativeStore::open_existing(&paths.native).unwrap();
    match store.inspect_capture(&stage) {
        Err(omniusong_store::StoreError::Recording(RecordingError::Database(error))) => {
            assert_eq!(error.sqlite_error().unwrap().extended_code & 0xff, 11);
        }
        other => panic!("Expected typed journal SQLITE_CORRUPT, got {other:?}"),
    }
    drop(store);
    let mut service = SessionService::reconnect(paths).unwrap();
    call(&mut service, Command::Initialize).unwrap();
    let error = call(&mut service, Command::InspectCapture { stage_id: stage }).unwrap_err();
    assert_eq!(error.kind, FailureKind::Backend);
    assert!(!service.is_disconnected());
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert!(matches!(
        call(&mut service, Command::Refresh).unwrap(),
        Reply::Library(_)
    ));
    assert!(
        Command::InspectCapture {
            stage_id: "../other".into()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn typed_revision_conflict_keeps_original_and_a_fresh_choice_is_explicit() {
    let f = Fixture::new();
    let paths = f.paths();
    let mut store = NativeStore::open(&paths.native).unwrap();
    let project = store
        .create(&ProjectDocument::new("Before concurrent edit").unwrap())
        .unwrap();
    let (stage, mut journal) = f.journal();
    let path = journal.path().to_path_buf();
    let manifest = journal.seal("explicit-finish").unwrap();
    drop(journal);
    let mut changed = project.clone();
    changed.rename("Saved by another writer").unwrap();
    let latest = store.save(&changed, 0).unwrap();
    let publication = TakePublication {
        operation_id: OperationId("rejected-choice".into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: 0,
        track_id: TrackId("rejected-track".into()),
        track_name: "Retained source".into(),
        clip: TakeClip {
            edit_source: None,
            id: "rejected-clip".into(),
            source: TakeReference {
                take_id: TakeId("service-retained-original".into()),
                manifest_sha256: manifest.sha256,
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames { start: 0, count: 4 },
            position: TimelinePosition {
                frame: 0,
                sample_rate: 48_000,
            },
            fades: None,
        },
    };
    drop(store);
    let mut service = SessionService::reconnect(paths.clone()).unwrap();
    call(&mut service, Command::Initialize).unwrap();
    let rejected = Command::AttachCapture {
        journal_path: path.clone(),
        publication: publication.clone(),
    };
    for command in [rejected.clone(), rejected] {
        let error = call(&mut service, command).unwrap_err();
        assert_eq!(error.kind, FailureKind::RevisionConflict);
        assert!(!error.outcome_unknown);
        assert!(!service.is_disconnected());
    }
    let store = NativeStore::open_existing(&paths.native).unwrap();
    assert_eq!(store.load(project.id()).unwrap(), latest);
    assert!(
        store
            .registered_take(&publication.clip.source.take_id)
            .is_ok()
    );
    assert!(
        store
            .inspect_capture(&stage)
            .unwrap()
            .manifest_sha256
            .is_some()
    );
    drop(store);
    // This fresh choice is a separate explicit command after observing the
    // latest saved revision, never a changed payload under the rejected ID.
    let mut fresh = publication;
    fresh.operation_id = OperationId("new-explicit-choice".into());
    fresh.expected_revision = latest.revision();
    fresh.track_id = TrackId("new-explicit-track".into());
    fresh.clip.id = "new-explicit-clip".into();
    let Reply::Document {
        view,
        replayed: false,
        ..
    } = call(
        &mut service,
        Command::AttachCapture {
            journal_path: path,
            publication: fresh,
        },
    )
    .unwrap()
    else {
        panic!("Missing explicit fresh attachment");
    };
    assert_eq!(view.document.revision(), 2);
    assert_eq!(view.document.name(), "Saved by another writer");
    assert_eq!(view.document.tracks().len(), project.tracks().len() + 1);
}
