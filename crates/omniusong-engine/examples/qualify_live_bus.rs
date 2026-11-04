//! Offline synthetic live-audition -> PCM -> original journal -> archive proof.
//! Requires a fresh isolated directory. Never opens devices or the live catalog.
use omniusong_core::{
    ProjectDocument,
    contracts::{ClockDomainId, ClockEpochId, OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_engine::{
    LiveBatch, LiveEvent, LiveEventKind, LiveId, LivePatch, LiveScope,
    capture::CaptureJournal,
    live::live_pair,
    realtime::{PACKET_FRAMES, capture_queue},
    render,
};
use omniusong_recording::{
    HostClock, PcmEncoding, PcmFormat, PcmLayout, StreamDescriptor, StreamKind, TakeDescriptor,
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

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn pcm_bytes(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}
fn wav(path: &Path, samples: &[f32], rate: u32, channels: u16) -> Result<(), Box<dyn Error>> {
    let data = pcm_bytes(samples);
    let length = u32::try_from(data.len())?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(length + 36).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?;
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&(rate * u32::from(channels) * 4).to_le_bytes())?;
    file.write_all(&(channels * 4).to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&length.to_le_bytes())?;
    file.write_all(&data)?;
    file.sync_all()?;
    Ok(())
}
fn event(source: u8, frame: u64, event: LiveEventKind) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: frame,
        event,
    }
}
fn phrase(frame: usize) -> Vec<LiveEvent> {
    let on = |note, voice, key, velocity16, patch| LiveEventKind::NoteOn {
        note,
        voice,
        key,
        velocity16,
        patch,
    };
    let off = |voice, release_velocity16| LiveEventKind::NoteOff {
        voice,
        release_velocity16,
    };
    let pulse = |note, voice, key, patch, duration_frames| LiveEventKind::Pulse {
        note,
        voice,
        key,
        velocity16: 40_000,
        release_velocity16: 50_000,
        patch,
        duration_frames,
    };
    let ticks = frame as u64;
    match frame {
        0 => vec![event(1, ticks, on(1, 1, 60, 49_152, LivePatch::Keys))],
        1_024 => vec![event(1, ticks, on(2, 2, 60, 32_768, LivePatch::Keys))],
        4_096 => vec![
            event(1, ticks, off(1, 65_535)),
            event(2, ticks, on(1, 1, 48, 45_000, LivePatch::Bass)),
        ],
        5_120 => vec![event(1, ticks, LiveEventKind::Sustain { down: true })],
        6_144 => vec![event(1, ticks, off(2, 12_345))],
        8_192 => vec![event(3, ticks, pulse(1, 1, 36, LivePatch::Drums, 4_800))],
        10_240 => vec![event(3, ticks, pulse(2, 2, 42, LivePatch::Drums, 2_400))],
        12_288 => vec![event(4, ticks, pulse(1, 1, 72, LivePatch::Pad, 12_000))],
        16_384 => vec![event(1, ticks, LiveEventKind::Sustain { down: false })],
        24_576 => vec![event(2, ticks, off(1, 65_535))],
        32_768 => vec![event(1, ticks, on(3, 3, 67, 0, LivePatch::Keys))],
        33_280 => vec![event(1, ticks, off(3, 65_535))],
        _ => vec![],
    }
}
fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err("usage: qualify_live_bus FRESH_DIRECTORY FRESH_RECEIPT_JSON".into());
    }
    let root = PathBuf::from(&arguments[0]);
    let receipt = PathBuf::from(&arguments[1]);
    if root.exists() || receipt.exists() {
        return Err("Output and receipt must be new".into());
    }
    fs::create_dir(&root)?;
    let mut store = NativeStore::open(root.join("store"))?;
    let project = store.create(&ProjectDocument::new(
        "Live audition · synthetic bus proof",
    )?)?;
    let initial_project = project.to_json()?;
    let initial_catalog = fs::read(root.join("store/studio.sqlite3"))?;
    let scope = LiveScope {
        worker_generation: LiveId::random(),
        session_id: LiveId::random(),
        panic_generation: 1,
    };
    let rate = 48_000;
    let channels = 2;
    let frames = 108_000usize;
    let (mut admission, mut renderer) = live_pair(scope, rate, channels)?;
    let control = admission.control();
    let mut samples = Vec::with_capacity(frames * usize::from(channels));
    let mut block = [0.0f32; 512 * 2];
    let mut sequence = 1u64;
    let mut accepted = 0u64;
    let mut event_count = 0usize;
    let mut peak = 0.0f32;
    let mut offset = 0usize;
    while offset < frames {
        let events = phrase(offset);
        if !events.is_empty() || offset.is_multiple_of(12_288) {
            let mut batch =
                LiveBatch::empty(scope, sequence, admission.lease_credit().map(|c| c.token));
            event_count += events.len();
            for event in events {
                batch.try_push(event)?;
            }
            let progress = admission.admit(batch)?;
            assert_eq!(progress.worker_accepted_sequence, Some(sequence));
            assert!(progress.fault.is_none());
            accepted = sequence;
            sequence = sequence
                .checked_add(1)
                .ok_or("Synthetic sequence exhausted")?;
        }
        let count = (frames - offset).min(512);
        let output = &mut block[..count * usize::from(channels)];
        renderer.render_into(output);
        assert!(
            output
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
        );
        peak = output
            .iter()
            .fold(peak, |value, sample| value.max(sample.abs()));
        samples.extend_from_slice(output);
        offset += count;
    }
    let live_progress = admission.progress();
    assert_eq!(live_progress.worker_accepted_sequence, Some(accepted));
    assert_eq!(live_progress.callback_applied_sequence, Some(accepted));
    assert_eq!(live_progress.active_voices, 0);
    assert!(live_progress.fault.is_none());
    assert!(peak > 0.01);
    assert!(
        samples[48_000 * usize::from(channels)..]
            .iter()
            .all(|sample| *sample == 0.0)
    );
    assert_eq!(store.load(project.id())?.to_json()?, initial_project);
    assert_eq!(
        fs::read(root.join("store/studio.sqlite3"))?,
        initial_catalog
    );
    drop(renderer);
    assert!(control.render_destroyed());
    // No native error closure was constructed in this device-free fixture.
    assert!(!control.error_destroyed());

    let epoch = ClockEpochId(uuid::Uuid::new_v4().to_string());
    let take_id = uuid::Uuid::new_v4().to_string();
    let descriptor = TakeDescriptor {
        version: 1,
        take_id: TakeId(take_id.clone()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "synthetic-live-bus".into(),
            source_id: "synthetic-live-qualifier".into(),
            source_name: "Offline procedural keys/pads; no human or device".into(),
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
            "omniusong:liveAuditionQualification".into(),
            json!({
                "origin":"offline procedural live-render fixture","human":false,"deviceOpened":false,
                "nativeInputOpened":false,"elapsedRecording":false,"eventCount":event_count,
                "captureWasSeparateExplicitQualifierStep":true,
            }),
        )]),
    };
    let stage = root.join("synthetic-live-bus.sqlite3");
    let mut journal = CaptureJournal::create(&stage, descriptor)?;
    let (mut producer, mut consumer, meters) = capture_queue(channels, rate, 8)?;
    let mut frame = 0usize;
    while frame < frames {
        let count = (frames - frame).min(PACKET_FRAMES);
        producer.ingest_f32(
            &samples[frame * usize::from(channels)..(frame + count) * usize::from(channels)],
            frame as u64 * 1_000_000_000 / u64::from(rate),
        );
        while let Ok(packet) = consumer.pop() {
            journal.append_packet(&packet)?;
        }
        frame += count;
    }
    drop(producer);
    while let Ok(packet) = consumer.pop() {
        journal.append_packet(&packet)?;
    }
    let captured = journal
        .finish(meters.snapshot(), true)?
        .ok_or("Missing sealed synthetic take")?;
    assert_eq!(captured.frames, frames as u64);
    assert_eq!(captured.gap_frames, 0);
    let publication = TakePublication {
        operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        track_id: TrackId(uuid::Uuid::new_v4().to_string()),
        track_name: "Synthetic audition capture · separate qualifier".into(),
        clip: TakeClip {
            edit_source: None,
            id: uuid::Uuid::new_v4().to_string(),
            source: TakeReference {
                take_id: TakeId(take_id),
                manifest_sha256: captured.manifest_sha256.clone(),
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames {
                start: 0,
                count: captured.frames,
            },
            position: TimelinePosition {
                frame: 0,
                sample_rate: rate,
            },
            fades: None,
        },
    };
    let attached = store.attach_capture(&stage, &publication)?.document;
    assert_eq!(render::prepare(&store, &attached, rate, channels)?, samples);
    let undone = store.undo(project.id(), attached.revision())?;
    assert_eq!(undone.tracks(), project.tracks());
    let redone = store.redo(project.id(), undone.revision())?;
    assert_eq!(render::prepare(&store, &redone, rate, channels)?, samples);
    let archive = root.join("synthetic-live-bus.omniusong-native");
    store.export_archive(project.id(), &archive)?;
    let mut restored = NativeStore::open(root.join("restored-store"))?;
    let restored_project = restored.restore_archive(&archive)?;
    assert_eq!(restored_project, redone);
    assert_eq!(
        render::prepare(&restored, &restored_project, rate, channels)?,
        samples
    );
    let wav_path = root.join("synthetic-live-bus.wav");
    wav(&wav_path, &samples, rate, channels)?;
    let mut result = json!({"format":"omniusong.native-live-bus-qualification","version":1,
        "syntheticOnly":true,"humanPerformance":false,"nativeInputOpened":false,"deviceOpened":false,
        "physicalMicrophoneOpened":false,"elapsedRecording":false,"root":root,
        "sampleRate":rate,"channels":channels,"frames":frames,"peak":peak,
        "eventCount":event_count,"acceptedBatches":accepted,
        "finalLiveProgress":live_progress,"auditionDidNotMutateProjectOrCatalog":true,
        "repeatedPitchAndSustain":true,"fourProceduralPatches":true,
        "zeroVelocityPreserved":true,"callbackOwnedPulses":true,"last60000FramesSilent":true,
    });
    let output = json!({"pcmSha256":digest(&pcm_bytes(&samples)),"captureGapFrames":0,
        "capturedManifestSha256":captured.manifest_sha256,"journal":stage,
        "projectId":project.id(),"finalRevision":redone.revision(),
        "captureAttachmentUndoRedoArchiveRestorationExact":true,
        "archive":archive,"archiveByteLength":fs::metadata(&archive)?.len(),"archiveSha256":digest(&fs::read(&archive)?),
        "wav":wav_path,"wavSha256":digest(&fs::read(&wav_path)?),
        "limits":["No native devices, real IPC, GUI event delivery, human recording or elapsed capture is exercised.",
            "No listening, physical latency, MIDI/MPE, plugin, inference, installer or Windows acceptance."]});
    result
        .as_object_mut()
        .ok_or("Receipt must be an object")?
        .extend(
            output
                .as_object()
                .ok_or("Output must be an object")?
                .clone(),
        );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(receipt)?;
    file.write_all(&serde_json::to_vec_pretty(&result)?)?;
    file.sync_all()?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
