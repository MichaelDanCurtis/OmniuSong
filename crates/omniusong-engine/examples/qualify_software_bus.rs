//! Device-free internal render/capture loopback. Never opens a microphone.
use omniusong_core::{
    ProjectDocument,
    contracts::{ClockDomainId, ClockEpochId, OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_engine::{
    capture::CaptureJournal,
    realtime::{PACKET_FRAMES, RenderCursor, capture_queue},
    render,
};
use omniusong_recording::{
    HostClock, PcmEncoding, PcmFormat, PcmLayout, SealedJournal, StreamDescriptor, StreamKind,
    TakeDescriptor,
};
use omniusong_store::{NativeStore, TakePublication};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

fn pcm_bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn wav(path: &Path, samples: &[f32], rate: u32, channels: u16) -> Result<(), Box<dyn Error>> {
    let data = pcm_bytes(samples);
    let len = u32::try_from(data.len())?;
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(len + 36).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?;
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * channels as u32 * 4).to_le_bytes())?;
    f.write_all(&(channels * 4).to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&len.to_le_bytes())?;
    f.write_all(&data)?;
    f.sync_all()?;
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 {
        return Err(
            "Usage: qualify_software_bus EXISTING_STORE SOURCE_PROJECT FRESH_OUTPUT NEW_RECEIPT"
                .into(),
        );
    }
    let root = PathBuf::from(&args[2]);
    let receipt = PathBuf::from(&args[3]);
    if root.exists() || receipt.exists() {
        return Err("Qualification output/receipt must be new".into());
    }
    fs::create_dir(&root)?;
    let mut store = NativeStore::open_existing(&args[0])?;
    let project_id = args[1].to_str().ok_or("Non-UTF8 project identity")?;
    let source = store.load(project_id)?;
    let source_json = source.to_json()?;
    let rate = 48_000;
    let channels = 2;
    let prepared = render::prepare(&store, &source, rate, channels)?;
    let frames = prepared.len() / channels as usize;
    if frames == 0 {
        return Err("Fixture has no audio frames".into());
    }
    let (mut cursor, _) = RenderCursor::new(prepared, channels)?;
    let (mut producer, mut consumer, meters) = capture_queue(channels, rate, 8)?;
    let id = uuid::Uuid::new_v4().to_string();
    let epoch = ClockEpochId(uuid::Uuid::new_v4().to_string());
    let descriptor = TakeDescriptor {
        version: 1,
        take_id: TakeId(id.clone()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "loopback-segment".into(),
            source_id: "internal-software-bus".into(),
            source_name: "Deterministic internal software loopback · no device input".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("synthetic-frame-clock".into()),
                epoch: epoch.clone(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            kind: StreamKind::Audio {
                format: PcmFormat {
                    sample_rate: rate,
                    channels,
                    encoding: PcmEncoding::F32Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: epoch,
            },
            initial_source_position: 0,
        }],
        provenance: BTreeMap::from([(
            "omniusong:qualification".into(),
            json!({"origin":"internal-software-loopback","actualDeviceInput":false,"elapsedRealtimeRecording":false,"sourceProject":source.id(),"sourceRevision":source.revision()}),
        )]),
    };
    let path = root.join("software-loopback.sqlite3");
    let mut journal = CaptureJournal::create(&path, descriptor)?;
    let mut rendered = Vec::with_capacity(frames * channels as usize);
    let mut block = [0.0f32; PACKET_FRAMES * 2];
    let mut next = 0usize;
    while next < frames {
        let count = (frames - next).min(PACKET_FRAMES);
        let data = &mut block[..count * channels as usize];
        cursor.render_into(data);
        rendered.extend_from_slice(data);
        producer.ingest_f32(data, next as u64 * 1_000_000_000 / rate as u64);
        while let Ok(packet) = consumer.pop() {
            journal.append_packet(&packet)?;
        }
        next += count;
    }
    drop(producer);
    while let Ok(packet) = consumer.pop() {
        journal.append_packet(&packet)?;
    }
    let capture = journal
        .finish(meters.snapshot(), true)?
        .ok_or("Missing sealed loopback")?;
    assert_eq!(capture.frames, frames as u64);
    assert_eq!(capture.gap_frames, 0);
    let sealed = SealedJournal::open(&path)?;
    assert_eq!(
        sealed.manifest().body.payload_bytes,
        rendered.len() as u64 * 4
    );
    drop(sealed);
    let mut target = ProjectDocument::new("Native software loopback · recorded original")?
        .value()
        .clone();
    target["omniusongAudioQualification"] =
        json!({"origin":"software_bus","actualDeviceInput":false,"sourceProject":source.id()});
    let target = store.create(&ProjectDocument::from_value(target)?)?;
    let publication = TakePublication {
        operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
        project_id: ProjectId(target.id().into()),
        expected_revision: target.revision(),
        track_id: TrackId(uuid::Uuid::new_v4().to_string()),
        track_name: "Recorded internal bus · synthetic source".into(),
        clip: TakeClip {
            id: uuid::Uuid::new_v4().to_string(),
            source: TakeReference {
                take_id: TakeId(capture.take_id.clone()),
                manifest_sha256: capture.manifest_sha256.clone(),
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames {
                start: 0,
                count: capture.frames,
            },
            position: TimelinePosition {
                frame: 0,
                sample_rate: rate,
            },
            fades: None,
        },
    };
    let attached = store.attach_capture(&path, &publication)?;
    assert_eq!(
        render::prepare(&store, &attached.document, rate, channels)?,
        rendered
    );
    let undone = store.undo(target.id(), attached.document.revision())?;
    let redone = store.redo(target.id(), undone.revision())?;
    assert_eq!(render::prepare(&store, &redone, rate, channels)?, rendered);
    let replay = store.attach_capture(root.join("missing-stage.sqlite3"), &publication)?;
    assert!(replay.replayed);
    assert_eq!(store.load(target.id())?, redone);
    let archive_path = root.join("software-loopback.omniusong-native");
    let archive = store.export_archive(target.id(), &archive_path)?;
    let mut restored = NativeStore::open(root.join("restored-store"))?;
    let restored_doc = restored.restore_archive(&archive_path)?;
    assert_eq!(restored_doc, redone);
    assert_eq!(
        render::prepare(&restored, &restored_doc, rate, channels)?,
        rendered
    );
    assert_eq!(store.load(source.id())?.to_json()?, source_json);
    let wav_path = root.join("software-loopback.wav");
    wav(&wav_path, &rendered, rate, channels)?;
    let out = json!({"format":"omniusong.native-software-loopback-qualification","version":1,"actualDeviceInput":false,"physicalMicrophoneOpened":false,"elapsedRealtimeRecording":false,"sourceProjectId":source.id(),"sourceProjectSha256":digest(&source_json),"sourceProjectUnchanged":true,"recordedProjectId":target.id(),"finalRevision":redone.revision(),"sampleRate":rate,"channels":channels,"frames":capture.frames,"gapFrames":capture.gap_frames,"pcmSha256":digest(&pcm_bytes(&rendered)),"journal":path,"manifestSha256":capture.manifest_sha256,"archive":{"projectId":archive.project_id,"historyRows":archive.history_rows,"operationRows":archive.operation_rows,"takeCount":archive.take_count,"assetCount":archive.asset_count},"archivePath":archive_path,"archiveByteLength":fs::metadata(&archive_path)?.len(),"archiveSha256":digest(&fs::read(&archive_path)?),"wav":wav_path,"wavSha256":digest(&fs::read(&wav_path)?),"exactCapturePlaybackUndoRedoReplayAndArchiveRestore":true,"limits":["Synthetic internal bus; no native hardware/input callback or real-time elapsed-capture qualification.","No listening, physical latency, thirty-minute stability, cross-device alignment, MIDI, plugins, models or Windows acceptance."]});
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(receipt)?;
    f.write_all(serde_json::to_string_pretty(&out)?.as_bytes())?;
    f.sync_all()?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
