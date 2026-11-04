//! Synthetic retained-stage qualification. Never opens devices or existing music.
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{CaptureStageState, NativeStore, TakePublication};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn raw_rows(path: &Path) -> Vec<(i64, Vec<u8>, Vec<u8>, String)> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.prepare("SELECT ordinal,header,payload,sha256 FROM chunks ORDER BY ordinal")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: qualify_capture_recovery FRESH_DESTINATION RECEIPT_JSON".into());
    }
    let root = PathBuf::from(&args[0]);
    fs::create_dir(&root)?;
    let native = root.join("store");
    let mut store = NativeStore::open(&native)?;
    assert!(store.scan_captures().stages.is_empty());
    assert!(!native.join("captures").exists());
    let project = store.create(&ProjectDocument::new(
        "Recovered synthetic prefix · no microphone",
    )?)?;
    let stage = uuid::Uuid::new_v4().to_string();
    let journal_path = native.join("captures").join(&stage).join("take.sqlite3");
    let descriptor = TakeDescriptor {
        version: 1,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "software-segment".into(),
            source_id: "software-fixture".into(),
            source_name: "Synthetic retained capture; no device".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("synthetic-clock".into()),
                epoch: ClockEpochId("synthetic-epoch".into()),
                ticks_per_second_numerator: 48_000,
                ticks_per_second_denominator: 1,
            },
            kind: StreamKind::Audio {
                format: PcmFormat {
                    sample_rate: 48_000,
                    channels: 1,
                    encoding: PcmEncoding::F32Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: ClockEpochId("synthetic-epoch".into()),
            },
            initial_source_position: 0,
        }],
        provenance: BTreeMap::from([(
            "omniusong:qualification".into(),
            json!({"origin":"procedural synthetic PCM","human":false,"deviceOpened":false}),
        )]),
    };
    let mut journal = Journal::create(&journal_path, &descriptor)?;
    let pcm: Vec<u8> = (0..12_000)
        .flat_map(|frame| {
            let t = frame as f32 / 48_000.0;
            let envelope =
                (frame as f32 / 480.0).min(1.0) * ((12_000 - frame) as f32 / 480.0).min(1.0);
            (0.1 * envelope * (std::f32::consts::TAU * 440.0 * t).sin()).to_le_bytes()
        })
        .collect();
    for sequence in 0..2 {
        let start = sequence * 12_000;
        let stamp = |ticks| HostTimestamp {
            domain: ClockDomainId("synthetic-clock".into()),
            epoch: ClockEpochId("synthetic-epoch".into()),
            ticks,
        };
        journal.append(
            &ChunkHeader {
                version: 1,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "software-segment".into(),
                sequence,
                range: SourceRange::AudioFrames {
                    start,
                    count: 12_000,
                },
                interpretation: descriptor.streams[0].kind.clone(),
                observed_start: stamp(start),
                observed_end: stamp(start + 12_000),
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            },
            &pcm,
        )?;
    }
    // A live writer cannot be inspected/taken over; scan itself opens no journal.
    assert_eq!(store.scan_captures().stages.len(), 1);
    assert!(store.inspect_capture(&stage).is_err());
    drop(journal);
    let db = Connection::open(&journal_path)?;
    db.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(length(payload)) WHERE sequence=1;")?;
    drop(db);
    let raw = raw_rows(&journal_path);
    assert_eq!(raw.len(), 2);
    let before = fs::read(&journal_path)?;
    let inspection = store.inspect_capture(&stage)?;
    assert_eq!(fs::read(&journal_path)?, before);
    assert_eq!(inspection.state, CaptureStageState::Recording);
    assert_eq!(inspection.verified_chunks, 1);
    assert!(inspection.unknown_tail && !inspection.problems.is_empty());
    assert_eq!(inspection.audio[0].frames, 12_000);
    let recovered = store.recover_capture(&inspection, "qualification-recover-once")?;
    assert!(!recovered.replayed && recovered.unknown_tail);
    assert_eq!(raw_rows(&journal_path), raw);
    let replay = store.recover_capture(&inspection, "qualification-recover-once")?;
    assert!(replay.replayed);
    assert_eq!(recovered.manifest_sha256, replay.manifest_sha256);
    assert!(
        store
            .recover_capture(&inspection, "different-recovery")
            .is_err()
    );
    let a = &recovered.audio[0];
    let publication = TakePublication {
        operation_id: OperationId("qualification-attach-once".into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: 0,
        track_id: TrackId("recovered-original".into()),
        track_name: "Recovered software prefix · unknown tail".into(),
        clip: TakeClip {
            edit_source: None,
            id: "recovered-prefix".into(),
            source: TakeReference {
                take_id: TakeId(recovered.take_id.clone()),
                manifest_sha256: recovered.manifest_sha256.clone(),
                stream_id: a.stream_id.clone(),
            },
            range: TakeRange::AudioFrames {
                start: a.start_frame,
                count: a.frames,
            },
            position: TimelinePosition {
                frame: 0,
                sample_rate: a.sample_rate,
            },
            fades: None,
        },
    };
    let attached = store.attach_capture(&journal_path, &publication)?.document;
    let undone = store.undo(project.id(), attached.revision())?;
    assert_eq!(undone.tracks(), project.tracks());
    let redone = store.redo(project.id(), undone.revision())?;
    assert_eq!(redone.tracks(), attached.tracks());
    let archive_path = root.join("recovered-prefix.omniusong-native");
    let archive = store.export_archive(project.id(), &archive_path)?;
    let mut restored = NativeStore::open(root.join("restored-store"))?;
    let restored_project = restored.restore_archive(&archive_path)?;
    assert_eq!(restored_project.tracks(), redone.tracks());
    let object = restored.registered_take(&publication.clip.source.take_id)?;
    let restored_original = root
        .join("restored-store/takes")
        .join(format!("{}.sqlite3", object.object_sha256));
    assert_eq!(
        raw_rows(&restored_original),
        raw,
        "archive lost retained damaged original rows"
    );
    let view = restored.open_take(&publication.clip.source)?;
    assert_eq!(view.read_chunk("audio", 0)?.payload, pcm);
    assert!(view.read_chunk("audio", 1).is_err());
    assert!(view.manifest().body.interrupted.is_some());
    let archive_bytes = fs::read(&archive_path)?;
    let receipt = json!({"format":"omniusong.native-capture-recovery-qualification","version":1,"syntheticOnly":true,"physicalMicrophoneOpened":false,"nativeInputOpened":false,
        "nativeStore":native,"stageId":stage,"journalPath":journal_path,"sourceProjectId":project.id(),"revision":redone.revision(),
        "reviewDidNotChangeBytes":true,"reviewedOriginalSha256":sha(&before),"liveOwnerRefused":true,"scanDoesNotCreateCaptureDirectory":true,
        "reviewedInspectionSha256":inspection.inspection_sha256,"verifiedChunks":1,"retainedRawChunks":2,"frames":12_000,"sampleRate":48_000,"channels":1,
        "knownGapFrames":a.gap_frames,"unknownTail":true,"manifestSha256":recovered.manifest_sha256,"pcmSha256":sha(&pcm),
        "sameRequestRecoveryReplayed":true,"differentRequestRecoveryRefused":true,"rawDamagedRowsExactAfterRecoveryAndArchive":true,"undoRedoAndRestoreExact":true,
        "archivePath":archive.path,"archiveByteLength":archive_bytes.len(),"archiveSha256":sha(&archive_bytes),
        "limits":["Synthetic retained journal; no elapsed/native input, physical listening/latency or power-loss qualification.","Actual native UI still needs separate verification; no combined archive of unrelated unassigned stages."]});
    fs::write(
        PathBuf::from(&args[1]),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
