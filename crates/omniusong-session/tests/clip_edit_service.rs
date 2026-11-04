//! Real session-worker regression using a fresh synthetic journal; no devices.
use omniusong_core::{
    AudioSourceWindow, ClipPatch, ClipTimelinePolicy, DocumentClipAddress, FieldChange,
    NativeAudioClipPatch, ProjectDocument, ValueChange,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_session::{
    Command, Completion, FailureKind, Paths, Reply, ServiceError, SessionService,
};
use omniusong_store::NativeStore;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
const TRACK: &str = "range-track";
const CLIP: &str = "range-clip";
struct Fixture {
    root: PathBuf,
    source: PathBuf,
    object: PathBuf,
}
impl Fixture {
    fn create() -> (Self, ProjectDocument) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/clip-edit-service")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let source = root.join("synthetic-original.sqlite3");
        let descriptor = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId("range-original".into()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                source_id: "synthetic-only".into(),
                source_name: "Independent frame bounds fixture".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("host".into()),
                    epoch: ClockEpochId("boot".into()),
                    ticks_per_second_numerator: 1_000_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 1,
                        encoding: PcmEncoding::S16Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("audio".into()),
                },
                initial_source_position: 100,
            }],
            provenance: BTreeMap::from([("fixture".into(), json!("no device input"))]),
        };
        let timestamp = HostTimestamp {
            domain: ClockDomainId("host".into()),
            epoch: ClockEpochId("boot".into()),
            ticks: 1_000,
        };
        let header = ChunkHeader {
            version: CHUNK_VERSION,
            take_id: descriptor.take_id.clone(),
            stream_id: "audio".into(),
            segment_id: "segment".into(),
            sequence: 0,
            range: SourceRange::AudioFrames {
                start: 100,
                count: 8,
            },
            interpretation: descriptor.streams[0].kind.clone(),
            observed_start: timestamp.clone(),
            observed_end: timestamp,
            clock_anchors: vec![],
            kind: ChunkKind::Content,
        };
        let mut journal = Journal::create(&source, &descriptor).unwrap();
        journal
            .append(&header, &[1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8, 0])
            .unwrap();
        journal.seal("seal-synthetic-original").unwrap();
        drop(journal);
        let sealed = SealedJournal::open(&source).unwrap();
        let mut store = NativeStore::open(root.join("store")).unwrap();
        let registered = store.ingest_take(&sealed).unwrap();
        let object = root
            .join("store/takes")
            .join(format!("{}.sqlite3", registered.object_sha256));
        let mut document = ProjectDocument::new("Verified range review")
            .unwrap()
            .upgrade_native()
            .unwrap();
        document
            .append_take_clip(
                TRACK,
                "Synthetic audio",
                TakeClip {
                    id: CLIP.into(),
                    source: TakeReference {
                        take_id: descriptor.take_id,
                        manifest_sha256: sealed.manifest().sha256.clone(),
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames {
                        start: 100,
                        count: 8,
                    },
                    position: TimelinePosition {
                        frame: -4,
                        sample_rate: 48_000,
                    },
                    fades: None,
                    edit_source: None,
                },
            )
            .unwrap();
        let document = store.create(&document).unwrap();

        (
            Self {
                root,
                source,
                object,
            },
            document,
        )
    }
    fn paths(&self) -> Paths {
        Paths {
            native: self.root.join("store"),
            legacy: self.root.join("unused-legacy"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Retained synthetic service fixture {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn call(
    service: &mut SessionService,
    command: Command,
) -> std::result::Result<Reply, ServiceError> {
    service.try_submit(command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(Completion { result, .. }) = service.try_poll().unwrap() {
            return result;
        }
        assert!(Instant::now() < deadline, "Synthetic clip worker timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn request(doc: &ProjectDocument, id: &str, start: u64, count: u64) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(doc.id().into()),
        expected_revision: doc.revision(),
        scope: EditScope::DocumentClip {
            address: DocumentClipAddress::NativeAudio {
                track_id: TRACK.into(),
                clip_id: CLIP.into(),
            },
        },
        edit: EditOperation::UpdateClip {
            patch: ClipPatch::NativeAudio(NativeAudioClipPatch {
                position_frame: ValueChange::Keep,
                source_window: ValueChange::Set {
                    value: AudioSourceWindow {
                        start_frame: start,
                        frame_count: count,
                    },
                },
                fades: FieldChange::Keep,
            }),
            timeline: ClipTimelinePolicy::PreserveLoop,
        },
        extensions: Default::default(),
    }
}
#[test]
fn verified_user_bounds_are_known_rejections_and_correctable_but_missing_source_is_uncertain() {
    let (fixture, original) = Fixture::create();
    let source = fs::read(&fixture.source).unwrap();
    let owned = fs::read(&fixture.object).unwrap();
    let mut service = SessionService::reconnect(fixture.paths()).unwrap();
    call(&mut service, Command::Initialize).unwrap();
    let invalid = request(&original, "outside-verified-prefix", 99, 8);
    for _ in 0..2 {
        let error = call(
            &mut service,
            Command::Edit {
                intent: invalid.clone(),
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::EditRejected);
        assert!(!error.outcome_unknown);
        assert!(!service.is_disconnected());
        let Some(Command::Edit { intent }) = service.retry_command() else {
            panic!("Exact rejected clip choice retained")
        };
        assert_eq!(intent, &invalid);
        let actual = NativeStore::open_existing(fixture.paths().native)
            .unwrap()
            .load(original.id())
            .unwrap();
        assert_eq!(actual, original);
        assert_eq!(fs::read(&fixture.source).unwrap(), source);
        assert_eq!(fs::read(&fixture.object).unwrap(), owned);
    }
    let Reply::Document { view, replayed, .. } = call(
        &mut service,
        Command::Edit {
            intent: request(&original, "corrected-window", 101, 6),
        },
    )
    .unwrap() else {
        panic!("Expected saved clip")
    };
    assert!(!replayed);
    assert_eq!(view.document.revision(), original.revision() + 1);
    assert_eq!(fs::read(&fixture.source).unwrap(), source);
    assert_eq!(fs::read(&fixture.object).unwrap(), owned);
    fs::remove_file(&fixture.object).unwrap();
    let error = call(
        &mut service,
        Command::Edit {
            intent: request(&view.document, "missing-source-bad-window", 99, 8),
        },
    )
    .unwrap_err();
    assert_ne!(error.kind, FailureKind::EditRejected);
    assert!(error.outcome_unknown);
    assert_eq!(fs::read(&fixture.source).unwrap(), source);
}
