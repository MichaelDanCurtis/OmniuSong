//! Offline callback Finish/v2 original/archive qualification. Opens no devices.
//! A callback Finish cut is deliberately recovered as interrupted evidence here;
//! this example cannot supply actual native lifecycle normal-seal authority.
use omniusong_core::{
    ProjectDocument,
    contracts::{ClockEpochId, EndpointId, OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_engine::{
    live::recording_live_pair,
    performance_records::{
        PerformanceConfig, PerformanceProfile, PerformanceRecord, PerformanceSource,
        create_descriptor_for, decode_record, descriptor_profile,
    },
    performance_tap::{BatchDisposition, PerformancePacketKind, TapFinish},
    performance_writer::{PerformanceWriter, inspect_performance, recover_performance},
    *,
};
use omniusong_recording::{CaptureReader, SealedJournal};
use omniusong_store::{NativeStore, TakePublication};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const CALLBACKS: [usize; 4] = [512, 256, 1024, 768];
const FRAMES: u64 = 2560;
fn fp(path: &Path) -> Result<Value> {
    let mut file = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buffer = [0; 65536];
    let mut length = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        h.update(&buffer[..read]);
        length += read;
    }
    Ok(json!({"path":path,"byteLength":length,"sha256":format!("{:x}",h.finalize())}))
}
fn verify(
    journal: &SealedJournal,
    config: &PerformanceConfig,
    expected_originals: &[LiveEvent],
    expected_batches: &[LiveBatch],
    expected_finish: &TapFinish,
    pcm: &str,
) -> Result<Value> {
    assert_eq!(
        descriptor_profile(journal.descriptor())?,
        PerformanceProfile::FinishV2
    );
    assert_eq!(
        journal.descriptor(),
        &create_descriptor_for(config, PerformanceProfile::FinishV2)?
    );
    assert!(journal.manifest().body.interrupted.is_some());
    let mut h = Sha256::new();
    let mut bytes = 0;
    let mut records = [0u64; 6];
    let mut originals = 0;
    let mut batch_count = 0;
    let mut source_counts = [0usize; 4];
    let mut finishes = 0;
    let mut record_hashes: [Sha256; 6] = std::array::from_fn(|_| Sha256::new());
    for ordinal in 1..=journal.manifest().body.chunk_count {
        let chunk = journal.read_ordinal(ordinal)?;
        let index = performance_records::STREAM_IDS
            .iter()
            .position(|s| *s == chunk.header.stream_id)
            .ok_or("Unknown stream")?;
        if index == 0 {
            h.update(&chunk.payload);
            bytes += chunk.payload.len();
            continue;
        }
        let mut at = 0;
        while at < chunk.payload.len() {
            let length = u32::from_le_bytes(chunk.payload[at..at + 4].try_into()?) as usize;
            at += 4;
            let raw = &chunk.payload[at..at + length];
            let record = decode_record(raw)?;
            assert_eq!(record.version, 2);
            record_hashes[index].update((length as u64).to_le_bytes());
            record_hashes[index].update(raw);
            records[index] += 1;
            match record.record {
                PerformanceRecord::AppliedEvent {
                    original,
                    callback_index,
                    batch_sequence,
                    applied_frame,
                    ..
                } => {
                    assert!((1..=4).contains(&index));
                    assert_eq!(usize::from(original.source), index);
                    assert!(original.observed_ticks > (1u64 << 53));
                    let expected = expected_originals
                        .iter()
                        .filter(|e| usize::from(e.source) == index)
                        .nth(source_counts[index - 1])
                        .ok_or("Unexpected applied original")?;
                    assert_eq!(&original, expected);
                    source_counts[index - 1] += 1;
                    assert_eq!(callback_index, (batch_sequence - 1) / 4 + 1);
                    assert_eq!(
                        applied_frame,
                        CALLBACKS[..callback_index as usize - 1]
                            .iter()
                            .sum::<usize>() as u64
                    );
                    originals += 1;
                }
                PerformanceRecord::Batch {
                    disposition, batch, ..
                } => {
                    assert_eq!(disposition, BatchDisposition::Applied);
                    assert_eq!(
                        batch.as_ref(),
                        expected_batches
                            .get(batch_count)
                            .ok_or("Unexpected retained batch")?
                    );
                    batch_count += 1;
                }
                PerformanceRecord::Finish { finish, .. } => {
                    assert_eq!(index, 5);
                    assert_eq!(finish.cut_frame, FRAMES);
                    assert_eq!(finish.fence.accepted_sequence, Some(16));
                    assert_eq!(finish.voices_cut, 16);
                    assert_eq!(&finish, expected_finish);
                    finishes += 1;
                }
                _ => {}
            }
            at += length;
        }
    }
    assert_eq!(bytes, FRAMES as usize * usize::from(CHANNELS) * 4);
    assert_eq!(format!("{:x}", h.finalize()), pcm);
    assert_eq!((originals, batch_count, finishes), (16, 16, 1));
    assert_eq!(source_counts, [4; 4]);
    assert!(records[1..5].iter().all(|c| *c == 4));
    Ok(
        json!({"pcmBytes":bytes,"pcmSha256":pcm,"records":records,"appliedOriginals":originals,
        "appliedBatches":batch_count,"finishes":finishes,"manifestSha256":journal.manifest().sha256,
        "rawEnvelopeDigests":record_hashes.map(|h|format!("{:x}",h.finalize()))}),
    )
}
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let requested = PathBuf::from(args.next().ok_or("Expected fresh fixture directory")?);
    let receipt = PathBuf::from(args.next().ok_or("Expected new receipt path")?);
    assert!(args.next().is_none());
    assert!(!receipt.exists(), "Receipt path must be new");
    fs::create_dir(&requested)?;
    let root = fs::canonicalize(requested)?;
    let stage = root.join("callback-finish-interrupted.sqlite3");
    let scope = LiveScope {
        worker_generation: LiveId::random(),
        session_id: LiveId::random(),
        panic_generation: 1,
    };
    let (mut admission, mut renderer, mut tap) = recording_live_pair(scope, RATE, CHANNELS)?;
    let config = PerformanceConfig {
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        info: tap.info(),
        gui_clock: LiveClock {
            domain: "qualify.finish.gui".into(),
            epoch: "qualify.finish.gui.epoch".into(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 3,
        },
        output_epoch: ClockEpochId("qualify.finish.output".into()),
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
            endpoint: EndpointId(format!("qualify.finish.source{}", index + 1)),
        }),
    };
    // Stage identity exists before any event is admitted. This is an offline
    // construction check, not a worker/UI Start acknowledgment.
    let mut writer = PerformanceWriter::create_finishable(&stage, config.clone())?;
    let descriptor_sha = writer.descriptor().sha256()?;
    let mut expected_originals = Vec::new();
    let mut expected_batches = Vec::new();
    for sequence in 1..=16 {
        let index = (sequence - 1) % 4;
        let event = LiveEvent {
            source: index as u8 + 1,
            connection_generation: 1,
            observed_ticks: (1u64 << 53) + sequence,
            event: LiveEventKind::NoteOn {
                note: (sequence - 1) / 4 + 1,
                voice: (sequence - 1) / 4 + 1,
                key: 60 + index as u8,
                velocity16: if sequence == 1 {
                    u16::MAX
                } else {
                    4096 + sequence as u16
                },
                patch: LivePatch::Keys,
            },
        };
        let mut batch = LiveBatch::empty(scope, sequence, None);
        batch.try_push(event)?;
        admission.admit(batch)?;
        expected_batches.push(batch);
        expected_originals.push(event);
    }
    let finish_id = LiveId::random();
    let fence = admission.begin_finish(scope, finish_id)?;
    assert_eq!(admission.begin_finish(scope, finish_id)?, fence);
    assert_eq!(fence.accepted_sequence, Some(16));
    let wav_path = root.join("callback-finish-f32.wav");
    let pcm_bytes = FRAMES as u32 * u32::from(CHANNELS) * 4;
    let mut wav = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&wav_path)?;
    wav.write_all(b"RIFF")?;
    wav.write_all(&(pcm_bytes + 48).to_le_bytes())?;
    wav.write_all(b"WAVEfmt ")?;
    wav.write_all(&16u32.to_le_bytes())?;
    wav.write_all(&3u16.to_le_bytes())?;
    wav.write_all(&CHANNELS.to_le_bytes())?;
    wav.write_all(&RATE.to_le_bytes())?;
    wav.write_all(&(RATE * u32::from(CHANNELS) * 4).to_le_bytes())?;
    wav.write_all(&(CHANNELS * 4).to_le_bytes())?;
    wav.write_all(&32u16.to_le_bytes())?;
    wav.write_all(b"fact")?;
    wav.write_all(&4u32.to_le_bytes())?;
    wav.write_all(&(FRAMES as u32).to_le_bytes())?;
    wav.write_all(b"data")?;
    wav.write_all(&pcm_bytes.to_le_bytes())?;
    let mut rendered = Sha256::new();
    let mut tapped = Sha256::new();
    let mut applied = Vec::new();
    let mut packet_count = 0;
    for frames in CALLBACKS {
        let mut bus = vec![0.0; frames * usize::from(CHANNELS)];
        renderer.render_into(&mut bus);
        assert!(bus.iter().all(|v| v.is_finite()) && bus.iter().any(|v| v.abs() > 0.0));
        for sample in &bus {
            let bytes = sample.to_le_bytes();
            rendered.update(bytes);
            wav.write_all(&bytes)?;
        }
        while let Some(packet) = tap.try_pop()? {
            packet_count += 1;
            match packet.kind {
                PerformancePacketKind::Pcm(pcm) => {
                    for value in &pcm.samples[..usize::from(pcm.frames) * usize::from(CHANNELS)] {
                        tapped.update(value.to_le_bytes());
                    }
                }
                PerformancePacketKind::Batch { batch, disposition } => {
                    assert_eq!(disposition, BatchDisposition::Applied);
                    applied.extend(
                        batch.events[..usize::from(batch.len)]
                            .iter()
                            .flatten()
                            .copied(),
                    );
                }
                _ => {}
            }
            writer.push_packet(packet)?;
        }
    }
    let ack = admission
        .finish_ack()?
        .ok_or("No callback Finish acknowledgment")?;
    assert_eq!(ack.fence, fence);
    assert_eq!(ack.cut_frame, FRAMES);
    assert_eq!(ack.callback_index, 4);
    assert_eq!(ack.voices_cut, 16);
    assert_eq!(applied, expected_originals);
    assert_eq!(writer.progress().finish_cut, Some(ack));
    assert_eq!(writer.progress().durable_frames, FRAMES);
    let mut silent = [1.0; 512];
    renderer.render_into(&mut silent);
    assert!(silent.iter().all(|v| v.to_bits() == 0));
    assert!(tap.try_pop()?.is_none());
    assert_eq!(admission.control().frames(), FRAMES);
    let pcm_sha = format!("{:x}", rendered.finalize());
    assert_eq!(format!("{:x}", tapped.finalize()), pcm_sha);
    wav.sync_all()?;
    drop(wav);
    let live_control = admission.control();
    let tap_control = tap.control();
    drop(renderer);
    assert!(live_control.render_destroyed());
    assert!(!live_control.error_destroyed());
    assert!(tap_control.snapshot()?.producer_destroyed);
    let retained = writer.close_interrupted()?;
    assert!(retained.unknown_tail);
    let before = fp(&stage)?;
    let reader = CaptureReader::open(&stage)?;
    let inspection = inspect_performance(&reader)?;
    assert!(inspection.problem.is_none(), "{:?}", inspection.problem);
    assert_eq!(inspection.finish_cut, Some(ack));
    assert!(inspection.unknown_tail);
    drop(reader);
    assert_eq!(before, fp(&stage)?);
    let operation = uuid::Uuid::new_v4().to_string();
    let (manifest, replayed) =
        recover_performance(CaptureReader::open(&stage)?, &inspection, &operation)?;
    assert!(!replayed);
    let (again, replayed) =
        recover_performance(CaptureReader::open(&stage)?, &inspection, &operation)?;
    assert!(replayed);
    assert_eq!(again, manifest);
    let after_recovery = fp(&stage)?;
    let recovered_review = CaptureReader::open(&stage)?;
    assert_eq!(
        recovered_review.inspection().original_rows_sha256,
        inspection.original_rows_sha256
    );
    assert_eq!(
        recovered_review.inspection().descriptor,
        create_descriptor_for(&config, PerformanceProfile::FinishV2)?
    );
    drop(recovered_review);
    let sealed = SealedJournal::open(&stage)?;
    let original_verified = verify(
        &sealed,
        &config,
        &expected_originals,
        &expected_batches,
        &ack,
        &pcm_sha,
    )?;
    let mut store = NativeStore::open(root.join("store"))?;
    let project = store.create(&ProjectDocument::new(
        "Synthetic callback Finish · recovered unknown tail",
    )?)?;
    let owned = store.ingest_take(&sealed)?;
    let reference = TakeReference {
        take_id: config.take_id.clone(),
        manifest_sha256: manifest.sha256.clone(),
        stream_id: "audio".into(),
    };
    let publication = TakePublication {
        operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        track_id: TrackId(uuid::Uuid::new_v4().to_string()),
        track_name: "Recovered synthetic Finish cut · uncalibrated solo placement".into(),
        clip: TakeClip {
            id: uuid::Uuid::new_v4().to_string(),
            source: reference.clone(),
            range: TakeRange::AudioFrames {
                start: 0,
                count: FRAMES,
            },
            position: TimelinePosition {
                frame: 0,
                sample_rate: RATE,
            },
            fades: None,
            edit_source: None,
        },
    };
    let attached = store.publish_take(&publication)?.document;
    let undone = store.undo(project.id(), attached.revision())?;
    assert_eq!(undone.tracks(), project.tracks());
    let redone = store.redo(project.id(), undone.revision())?;
    assert_eq!(redone.tracks(), attached.tracks());
    let archive_path = root.join("callback-finish.omniusong-native");
    let archive = store.export_archive(project.id(), &archive_path)?;
    let mut restored = NativeStore::open(root.join("restored-store"))?;
    let document = restored.restore_archive(&archive_path)?;
    assert_eq!(document, redone);
    assert_eq!(restored.registered_take(&config.take_id)?, owned);
    assert_eq!(
        verify(
            &restored.open_take(&reference)?,
            &config,
            &expected_originals,
            &expected_batches,
            &ack,
            &pcm_sha
        )?,
        original_verified
    );
    assert_eq!(
        fp(&stage)?,
        after_recovery,
        "Archive/publication must not change the original stage"
    );
    for (label, document) in [
        ("initial", &project),
        ("attached", &attached),
        ("undone", &undone),
        ("redone", &redone),
        ("restored", &document),
    ] {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(format!("project-{label}.json")))?;
        file.write_all(&document.to_json()?)?;
        file.sync_all()?;
    }
    let result = json!({"format":"omniusong.callback-finish-qualification","version":1,"syntheticOnly":true,
        "root":root,"config":config,"descriptorSha256":descriptor_sha,"fence":fence,"callbackAck":ack,
        "frames":FRAMES,"sampleRate":RATE,"channels":CHANNELS,"callbacks":CALLBACKS,"packetCount":packet_count,
        "pcmSha256":pcm_sha,"wav":fp(&wav_path)?,"exactOriginals":expected_originals,"exactBatches":expected_batches,"silentAfterCut":true,
        "retained":retained,"inspection":inspection,"explicitRecoveryOperation":operation,"recoveryReplay":true,
        "verifiedOriginal":original_verified,"ownedTake":owned,"originalAfterRecovery":after_recovery,"originalRowsPreserved":true,"archive":fp(&archive_path)?,"archiveCounts":{"history":archive.history_rows,"operations":archive.operation_rows,"takes":archive.take_count},
        "nativeInputOrOutputOpened":false,"nativeErrorClosureCreated":false,"normalSealAuthority":false,
        "limits":["Offline real renderer/tap only, not a native stream/worker/UI Finish.","Explicit recovery remains interrupted with unknown tail even after a valid callback cut.","No native error-closure destruction, normal seal, human performance, listening, latency, thirty-minute reliability, MIDI/MPE, models/plugins, installer or Windows acceptance."]});
    let bytes = serde_json::to_vec_pretty(&result)?;
    assert!(bytes.len() < 128 * 1024);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    println!("{}", receipt.display());
    Ok(())
}
