//! Capture publication retries retain the original independently of its stage.
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, StoreError, TakePublication};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/capture-attachment")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn source(&self) -> SealedJournal {
        let descriptor = TakeDescriptor {
            version: 1,
            take_id: TakeId("captured-source".into()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                source_id: "synthetic-input".into(),
                source_name: "Synthetic capture fixture".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("capture-clock".into()),
                    epoch: ClockEpochId("epoch".into()),
                    ticks_per_second_numerator: 1_000_000_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 1,
                        encoding: PcmEncoding::F32Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("epoch".into()),
                },
                initial_source_position: 0,
            }],
            provenance: BTreeMap::new(),
        };
        let path = self.0.join("source.sqlite3");
        let mut journal = Journal::create(&path, &descriptor).unwrap();
        let stamp = HostTimestamp {
            domain: ClockDomainId("capture-clock".into()),
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
                    range: SourceRange::AudioFrames { start: 0, count: 2 },
                    interpretation: descriptor.streams[0].kind.clone(),
                    observed_start: stamp.clone(),
                    observed_end: stamp,
                    clock_anchors: vec![],
                    kind: ChunkKind::Content,
                },
                &[0; 8],
            )
            .unwrap();
        journal.seal("seal").unwrap();
        drop(journal);
        SealedJournal::open(path).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn publication(project: &ProjectDocument, source: &SealedJournal) -> TakePublication {
    TakePublication {
        operation_id: OperationId("attach-once".into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        track_id: TrackId("recorded-track".into()),
        track_name: "Original microphone take".into(),
        clip: TakeClip {
            id: "recorded-clip".into(),
            source: TakeReference {
                take_id: source.descriptor().take_id.clone(),
                manifest_sha256: source.manifest().sha256.clone(),
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames { start: 0, count: 2 },
            position: TimelinePosition {
                frame: 0,
                sample_rate: 48_000,
            },
            fades: None,
        },
    }
}

#[test]
fn attachment_replay_after_source_disappears_preserves_later_project_edits() {
    let f = Fixture::new();
    let source = f.source();
    let mut store = NativeStore::open(f.0.join("store")).unwrap();
    let project = store
        .create(&ProjectDocument::new("Before recording").unwrap())
        .unwrap();
    let intent = publication(&project, &source);
    let original_manifest = source.manifest().clone();
    let original_chunk = source.read_chunk("audio", 0).unwrap();
    let first = store.attach_capture(source.path(), &intent).unwrap();
    assert_eq!(first.document.revision(), 1);
    let mut next = first.document.clone();
    next.rename("Later edit").unwrap();
    let latest = store.save(&next, 1).unwrap();
    let path = source.path().to_path_buf();
    drop(source);
    fs::remove_file(&path).unwrap();
    let replay = store.attach_capture(&path, &intent).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, first.document);
    assert_eq!(store.load(project.id()).unwrap(), latest);
    let take = store.open_take(&intent.clip.source).unwrap();
    // SQLite backup may change file layout; the authenticated original content
    // and semantic manifest must survive exactly.
    assert_eq!(take.manifest(), &original_manifest);
    assert_eq!(take.read_chunk("audio", 0).unwrap(), original_chunk);
    assert_eq!(
        store.undo(project.id(), latest.revision()).unwrap().name(),
        first.document.name()
    );
}

#[test]
fn failed_attachment_retains_durable_unassigned_original_for_same_id_retry() {
    let f = Fixture::new();
    let source = f.source();
    let mut store = NativeStore::open(f.0.join("store")).unwrap();
    let project = store
        .create(&ProjectDocument::new("Recording target").unwrap())
        .unwrap();
    let intent = publication(&project, &source);
    let mut next = project.clone();
    next.rename("Concurrent edit").unwrap();
    store.save(&next, 0).unwrap();
    assert!(matches!(
        store.attach_capture(source.path(), &intent),
        Err(StoreError::Conflict)
    ));
    assert!(store.registered_take(&intent.clip.source.take_id).is_ok());
    assert_eq!(store.load(project.id()).unwrap().revision(), 1);
    let path = source.path().to_path_buf();
    drop(source);
    fs::remove_file(&path).unwrap();
    let mut reviewed = intent;
    reviewed.expected_revision = 1;
    let result = store.attach_capture(&path, &reviewed).unwrap();
    assert!(!result.replayed);
    assert_eq!(result.document.revision(), 2);
}

#[test]
fn mismatched_capture_manifest_never_registers_or_changes_project() {
    let f = Fixture::new();
    let source = f.source();
    let mut store = NativeStore::open(f.0.join("store")).unwrap();
    let project = store
        .create(&ProjectDocument::new("Untouched target").unwrap())
        .unwrap();
    let mut intent = publication(&project, &source);
    intent.clip.source.manifest_sha256 = "0".repeat(64);
    assert!(store.attach_capture(source.path(), &intent).is_err());
    assert_eq!(store.load(project.id()).unwrap(), project);
    assert!(store.registered_take(&intent.clip.source.take_id).is_err());
    assert!(SealedJournal::open(source.path()).is_ok());
}
