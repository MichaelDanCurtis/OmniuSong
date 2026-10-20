//! New synthetic data for native-window inspection; never a device/capture test.
//! Usage: prepare_native_ui_fixture NATIVE_STORE_ROOT NEW_SOURCE_PATH RECEIPT_JSON
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, TrackId},
    native::{AudioFades, FadeCurve, TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, TakePublication};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

type Outcome<T> = std::result::Result<T, Box<dyn Error>>;

fn file_hash(path: &Path) -> Outcome<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn opaque_fields(track: &Value) -> Outcome<Vec<u8>> {
    let fields: BTreeMap<_, _> = ["notes", "audio", "instrument", "provenance"]
        .into_iter()
        .map(|key| (key, &track[key]))
        .collect();
    Ok(serde_json::to_vec(&fields)?)
}

fn main() -> Outcome<()> {
    let arguments: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [store_root, source_path, receipt_path] = arguments.as_slice() else {
        return Err(
            "Usage: prepare_native_ui_fixture NATIVE_STORE_ROOT NEW_SOURCE_PATH RECEIPT_JSON"
                .into(),
        );
    };
    match fs::symlink_metadata(source_path) {
        Ok(_) => return Err("NEW_SOURCE_PATH already exists; nothing was overwritten".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // Reserve the requested receipt exclusively before any project creation.
    // An interrupted example retains an empty/partial receipt and its new data.
    let mut receipt_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(receipt_path)?;
    let kind = StreamKind::Audio {
        format: PcmFormat {
            sample_rate: 48_000,
            channels: 2,
            encoding: PcmEncoding::S16Le,
            layout: PcmLayout::Interleaved,
        },
        audio_epoch: ClockEpochId("synthetic-ui-audio-epoch".into()),
    };
    let descriptor = TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![StreamDescriptor {
            stream_id: "synthetic-stereo".into(),
            segment_id: "synthetic-segment".into(),
            source_id: "synthetic-ui-fixture".into(),
            source_name: "Generated four-frame UI fixture; no device input".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("synthetic-ui-clock".into()),
                epoch: ClockEpochId("synthetic-ui-clock-epoch".into()),
                ticks_per_second_numerator: 48_000,
                ticks_per_second_denominator: 1,
            },
            kind: kind.clone(),
            initial_source_position: 100,
        }],
        provenance: BTreeMap::from([(
            "uiInspectionFixture".into(),
            json!({
                "synthetic": true, "capturedFromDevice": false, "audioPlayed": false,
                "description": "Generated four stereo S16 frames solely for native UI inspection",
            }),
        )]),
    };
    let observed = |ticks| HostTimestamp {
        domain: descriptor.streams[0].host_clock.domain.clone(),
        epoch: descriptor.streams[0].host_clock.epoch.clone(),
        ticks,
    };
    let header = ChunkHeader {
        version: CHUNK_VERSION,
        take_id: descriptor.take_id.clone(),
        stream_id: "synthetic-stereo".into(),
        segment_id: "synthetic-segment".into(),
        sequence: 0,
        range: SourceRange::AudioFrames {
            start: 100,
            count: 4,
        },
        interpretation: kind,
        observed_start: observed(1_000),
        observed_end: observed(1_004),
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    };
    let pcm = [0_u8, 0, 0, 0, 1, 0, 255, 255, 2, 0, 254, 255, 0, 0, 0, 0];
    let mut journal = Journal::create(source_path, &descriptor)?;
    journal.append(&header, &pcm)?;
    let manifest = journal.seal(&uuid::Uuid::new_v4().to_string())?;
    drop(journal);
    let source_hash = file_hash(source_path)?;
    let source = SealedJournal::open(source_path)?;
    let mut store = NativeStore::open(store_root)?;
    let object = store.ingest_take(&source)?;

    let mut initial = ProjectDocument::new("Synthetic native recording inspection")?
        .value()
        .clone();
    // A single recording track is immediately selected when the app opens it.
    initial["tracks"] = json!([]);
    let initial = store.create(&ProjectDocument::from_value(initial)?)?;
    let operation_id = OperationId(uuid::Uuid::new_v4().to_string());
    let published = store
        .publish_take(&TakePublication {
            operation_id: operation_id.clone(),
            project_id: ProjectId(initial.id().into()),
            expected_revision: initial.revision(),
            track_id: TrackId("synthetic-native-recording".into()),
            track_name: "Synthetic sealed original · inspection only".into(),
            clip: TakeClip {
                edit_source: None,
                id: "synthetic-four-frame-clip".into(),
                source: TakeReference {
                    take_id: descriptor.take_id.clone(),
                    manifest_sha256: manifest.sha256.clone(),
                    stream_id: "synthetic-stereo".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 100,
                    count: 4,
                },
                position: TimelinePosition {
                    frame: -48_000,
                    sample_rate: 48_000,
                },
                fades: Some(AudioFades {
                    in_frames: 1,
                    out_frames: 1,
                    curve: FadeCurve::EqualPower,
                }),
            },
        })?
        .document;
    let mut value = published.value().clone();
    let track = &mut value["tracks"][0];
    track["notes"] =
        json!([null, {"startTick": u64::MAX, "durationTicks": u64::MAX}, ["foreign"], "opaque"]);
    track["audio"] = json!(["opaque-audio", {"durationSeconds": "uninterpreted"}]);
    track["instrument"] = json!(["opaque-instrument"]);
    track["provenance"] =
        json!({"input": ["opaque-provider-provenance"], "fixture": "retained verbatim"});
    let opaque_bytes = opaque_fields(track)?;
    let candidate = ProjectDocument::from_value(value)?;
    let saved = store.save(&candidate, published.revision())?;
    assert_eq!(opaque_fields(&saved.tracks()[0])?, opaque_bytes);
    let reloaded = store.load(saved.id())?;
    assert_eq!(opaque_fields(&reloaded.tracks()[0])?, opaque_bytes);
    assert_eq!(reloaded, saved);
    let summary = store
        .list()?
        .into_iter()
        .find(|entry| entry.id == saved.id())
        .ok_or("The new fixture was not listed")?;
    assert_eq!(
        summary.note_count, 0,
        "Opaque notes on recording tracks are not musical notes"
    );
    assert_eq!(
        source_hash,
        file_hash(source_path)?,
        "Source originals changed"
    );
    let receipt = json!({
        "fixture": "synthetic-native-recording-ui-inspection", "synthetic": true,
        "capturedFromDevice": false, "audioPlayed": false, "engineCaptureExercised": false,
        "qualification": "UI inspection fixture only; no microphone/controller capture or audible playback",
        "projectId": saved.id(), "projectName": saved.name(), "revision": saved.revision(),
        "trackId": "synthetic-native-recording", "clipId": "synthetic-four-frame-clip",
        "operationId": operation_id.0, "noteCount": summary.note_count,
        "takeId": object.take_id.0, "manifestSha256": object.manifest_sha256,
        "objectSha256": object.object_sha256, "objectByteLength": object.byte_length,
        "sourceSha256": source_hash, "sourcePath": source_path,
        "opaqueMetadataSha256": format!("{:x}", Sha256::digest(&opaque_bytes)),
        "frames": 4, "channels": 2, "encoding": "s16_le", "sampleRate": 48_000,
    });
    receipt_file.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    receipt_file.write_all(b"\n")?;
    receipt_file.sync_all()?;
    #[cfg(unix)]
    fs::File::open(
        receipt_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?
    .sync_all()?;
    println!("{}", serde_json::to_string(&receipt)?);
    Ok(())
}
