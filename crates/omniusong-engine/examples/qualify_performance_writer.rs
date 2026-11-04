//! Device-free incremental performance-original qualification.
//! No stream, wall-clock capture, normal Finish, live catalog or GUI is used.
use omniusong_core::{
    ProjectDocument,
    contracts::{ClockEpochId, EndpointId, OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_engine::{
    LiveBatch, LiveClock, LiveEvent, LiveEventKind, LiveId, LivePatch, LiveScope,
    LiveSourceBinding, LiveSourceKind,
    live::recording_live_pair,
    performance_records::{
        LocalRecord, PerformanceConfig, PerformanceRecord, PerformanceSource, STREAM_IDS,
        checkpoint_sha256, decode_envelope, encode_record, from_descriptor,
    },
    performance_tap::{BatchDisposition, PerformancePacketKind, TapClearReason},
    performance_writer::{PerformanceWriter, inspect_performance, recover_performance},
};
use omniusong_recording::{CaptureReader, ChunkKind, SealedJournal, SourceRange, TakeState};
use omniusong_store::{NativeStore, TakePublication};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const RATE: u32 = 48_000;
const CHANNELS: u16 = 8;
const FRAMES: usize = 1_200_000;
const CALLBACK_FRAMES: usize = 16_384;
const EVENT_CALLBACKS: usize = 65;
const PANIC_FRAME: usize = EVENT_CALLBACKS * CALLBACK_FRAMES;
const OBSERVED_ORIGIN: u64 = 9_007_199_254_740_993;
const MAX_RECEIPT_BYTES: usize = 256 * 1024;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn digest(hash: &Sha256) -> String {
    format!("{:x}", hash.clone().finalize())
}
fn file_identity(path: &Path) -> Result<Value> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut length = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        length += read as u64;
        hash.update(&buffer[..read]);
    }
    Ok(json!({"path":path,"byteLength":length,"sha256":digest(&hash)}))
}
fn new_wav(path: &Path) -> Result<File> {
    let bytes = u32::try_from(FRAMES * usize::from(CHANNELS) * 4)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    // Fixed-size IEEE F32LE interleaved WAVE, with a fact chunk. No take-sized
    // buffer is needed to calculate its length or to write its samples.
    file.write_all(b"RIFF")?;
    file.write_all(&(bytes + 48).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?;
    file.write_all(&CHANNELS.to_le_bytes())?;
    file.write_all(&RATE.to_le_bytes())?;
    file.write_all(&(RATE * u32::from(CHANNELS) * 4).to_le_bytes())?;
    file.write_all(&(CHANNELS * 4).to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"fact")?;
    file.write_all(&4u32.to_le_bytes())?;
    file.write_all(&u32::try_from(FRAMES)?.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    Ok(file)
}
fn local(record: PerformanceRecord) -> LocalRecord {
    LocalRecord { version: 1, record }
}
#[derive(Clone, Default)]
struct Records {
    hash: Sha256,
    count: u64,
}
impl Records {
    fn add(&mut self, record: &LocalRecord) -> Result<()> {
        let bytes = serde_json::to_vec(record)?;
        self.hash.update((bytes.len() as u64).to_le_bytes());
        self.hash.update(bytes);
        self.count += 1;
        Ok(())
    }
    fn summary(&self) -> Value {
        json!({"count":self.count,"lengthPrefixedLocalRecordSha256":digest(&self.hash)})
    }
}
#[derive(Clone, Default)]
struct SourceRecords {
    records: Records,
    first_ticks: Option<u64>,
    last_ticks: Option<u64>,
    tick_sum: u128,
    note_on: u64,
    note_off: u64,
    sustain: u64,
    pulse: u64,
    zero_attack: u64,
    full_attack: u64,
    zero_release: u64,
    full_release: u64,
}
impl SourceRecords {
    fn add(&mut self, record: &LocalRecord) -> Result<()> {
        let PerformanceRecord::AppliedEvent { original, .. } = &record.record else {
            return Err("A GUI source stream contained a non-applied record".into());
        };
        assert!(original.observed_ticks > (1u64 << 53));
        assert!(
            self.last_ticks
                .is_none_or(|old| old <= original.observed_ticks)
        );
        self.first_ticks.get_or_insert(original.observed_ticks);
        self.last_ticks = Some(original.observed_ticks);
        self.tick_sum += u128::from(original.observed_ticks);
        let mut attack = None;
        let mut release = None;
        match original.event {
            LiveEventKind::NoteOn { velocity16, .. } => {
                self.note_on += 1;
                attack = Some(velocity16);
            }
            LiveEventKind::NoteOff {
                release_velocity16, ..
            } => {
                self.note_off += 1;
                release = Some(release_velocity16);
            }
            LiveEventKind::Sustain { .. } => self.sustain += 1,
            LiveEventKind::Pulse {
                velocity16,
                release_velocity16,
                ..
            } => {
                self.pulse += 1;
                attack = Some(velocity16);
                release = Some(release_velocity16);
            }
        }
        self.zero_attack += u64::from(attack == Some(0));
        self.full_attack += u64::from(attack == Some(u16::MAX));
        self.zero_release += u64::from(release == Some(0));
        self.full_release += u64::from(release == Some(u16::MAX));
        self.records.add(record)
    }
    fn summary(&self) -> Value {
        json!({"records":self.records.summary(),"firstObservedTicks":self.first_ticks,
            "lastObservedTicks":self.last_ticks,"observedTicksSumDecimal":self.tick_sum.to_string(),
            "noteOn":self.note_on,"noteOff":self.note_off,"sustain":self.sustain,"pulse":self.pulse,
            "zeroAttack":self.zero_attack,"fullAttack":self.full_attack,
            "zeroRelease":self.zero_release,"fullRelease":self.full_release})
    }
}
#[derive(Default)]
struct Evidence {
    sources: [SourceRecords; 4],
    control: Records,
}
impl Evidence {
    fn summary(&self) -> Value {
        json!({"guiSources":self.sources.iter().map(SourceRecords::summary).collect::<Vec<_>>(),
            "controlExcludingCheckpoints":self.control.summary()})
    }
}

fn fixture_event(ordinal: usize, frame: usize) -> LiveEvent {
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
    let pulse = |note, voice, key, velocity16, release_velocity16, patch, duration_frames| {
        LiveEventKind::Pulse {
            note,
            voice,
            key,
            velocity16,
            release_velocity16,
            patch,
            duration_frames,
        }
    };
    // Only the opening phrase creates voices (eight total). Repeated pitches,
    // source-owned sustain, exact zero/MAX/intermediate velocities, all four
    // procedural patches and callback-owned Pulse releases are retained.
    let (source, event) = match ordinal {
        0 => (1, on(1, 1, 60, u16::MAX, LivePatch::Keys)),
        1 => (1, on(2, 2, 60, 32_769, LivePatch::Keys)),
        2 => (1, LiveEventKind::Sustain { down: true }),
        3 => (1, off(1, u16::MAX)),
        4 => (1, off(2, 12_345)),
        5 => (1, on(3, 3, 67, 0, LivePatch::Keys)),
        6 => (1, off(3, 0)),
        7 => (2, on(1, 1, 48, 49_152, LivePatch::Bass)),
        8 => (3, pulse(1, 1, 36, 40_000, 50_000, LivePatch::Drums, 2_400)),
        9 => (4, pulse(1, 1, 72, 50_000, u16::MAX, LivePatch::Pad, 4_800)),
        10 => (1, LiveEventKind::Sustain { down: false }),
        11 => (3, pulse(2, 2, 38, u16::MAX, 0, LivePatch::Drums, 4_800)),
        12 => (4, pulse(2, 2, 74, 45_000, 65_000, LivePatch::Pad, 12_000)),
        _ => (
            ((ordinal % 4) + 1) as u8,
            LiveEventKind::Sustain {
                down: (ordinal / 4).is_multiple_of(2),
            },
        ),
    };
    LiveEvent {
        source,
        connection_generation: 1,
        // Opaque original GUI observations, deliberately >2^53. No conversion
        // to render time or physical clock alignment is asserted.
        observed_ticks: OBSERVED_ORIGIN + frame as u64 * 100 + ordinal as u64,
        event,
    }
}

fn verify_sealed(
    journal: &SealedJournal,
    config: &PerformanceConfig,
    expected: &Evidence,
    pcm_sha256: &str,
) -> Result<Value> {
    assert_eq!(&from_descriptor(journal.descriptor())?, config);
    assert_eq!(journal.manifest().body.streams.len(), 6);
    assert!(
        journal
            .manifest()
            .body
            .interrupted
            .as_ref()
            .is_some_and(|receipt| receipt.report.unknown_tail)
    );
    let mut actual = Evidence::default();
    let mut pcm_hash = Sha256::new();
    let mut pcm_bytes = 0u64;
    let mut next_frame = 0u64;
    let mut checkpoints = 0u64;
    let mut previous_checkpoint = None;
    let mut max_chunk_bytes = 0usize;
    let mut last_checkpoint = None;
    // A single bounded authenticated chunk and a single bounded envelope are
    // materialized at a time. No complete original or event list is collected.
    for ordinal in 1..=journal.manifest().body.chunk_count {
        let chunk = journal.read_ordinal(ordinal)?;
        max_chunk_bytes = max_chunk_bytes.max(chunk.payload.len());
        assert_eq!(chunk.header.kind, ChunkKind::Content);
        if chunk.header.stream_id == "audio" {
            let SourceRange::AudioFrames { start, count } = chunk.header.range else {
                return Err("Audio payload has an event range".into());
            };
            assert_eq!(start, next_frame);
            assert_eq!(chunk.payload.len() as u64, count * u64::from(CHANNELS) * 4);
            next_frame += count;
            pcm_bytes += chunk.payload.len() as u64;
            pcm_hash.update(&chunk.payload);
            continue;
        }
        let stream_index = STREAM_IDS
            .iter()
            .position(|stream| *stream == chunk.header.stream_id)
            .ok_or("Unknown stream in recovered performance")?;
        let SourceRange::Events { start, count } = chunk.header.range else {
            return Err("Raw payload has an audio range".into());
        };
        let mut remaining = chunk.payload.as_slice();
        let mut records = 0u64;
        while !remaining.is_empty() {
            let length = remaining.get(..4).ok_or("Truncated raw length prefix")?;
            let length = u32::from_le_bytes(length.try_into()?) as usize;
            let bytes = remaining
                .get(4..4 + length)
                .ok_or("Truncated raw envelope")?;
            let (envelope, record) = decode_envelope(bytes)?;
            assert_eq!(
                encode_record(
                    config,
                    stream_index,
                    &record,
                    envelope.payload.time.observed.ticks,
                    start + records
                )?,
                bytes
            );
            if stream_index == 5 {
                if let PerformanceRecord::Checkpoint { checkpoint } = &record.record {
                    checkpoints += 1;
                    assert_eq!(checkpoint.group_sequence, checkpoints);
                    assert_eq!(checkpoint.previous_checkpoint_sha256, previous_checkpoint);
                    previous_checkpoint = Some(checkpoint_sha256(bytes)?);
                    last_checkpoint = Some(checkpoint.clone());
                } else {
                    actual.control.add(&record)?;
                }
            } else {
                let PerformanceRecord::AppliedEvent { original, .. } = &record.record else {
                    return Err("Unexpected source record".into());
                };
                assert_eq!(usize::from(original.source), stream_index);
                assert_eq!(
                    envelope.payload.time.observed.ticks,
                    original.observed_ticks
                );
                actual.sources[stream_index - 1].add(&record)?;
            }
            records += 1;
            remaining = &remaining[4 + length..];
        }
        assert_eq!(records, count);
    }
    assert_eq!(next_frame, FRAMES as u64);
    assert_eq!(pcm_bytes, (FRAMES * usize::from(CHANNELS) * 4) as u64);
    assert_eq!(digest(&pcm_hash), pcm_sha256);
    assert_eq!(actual.summary(), expected.summary());
    let last = last_checkpoint.ok_or("No recovered performance checkpoint")?;
    assert_eq!(last.audio_end, FRAMES as u64);
    assert_eq!(last.prefixes, journal.manifest().body.streams);
    assert!(checkpoints > 1);
    Ok(
        json!({"pcmSha256":digest(&pcm_hash),"pcmBytes":pcm_bytes,"frames":next_frame,
        "checkpointCount":checkpoints,"lastCheckpointSha256":previous_checkpoint,
        "lastCheckpoint":last,"maximumMaterializedChunkBytes":max_chunk_bytes,
        "descriptorSha256":journal.manifest().body.descriptor_sha256,
        "manifestSha256":journal.manifest().sha256,"selectedChunks":journal.manifest().body.chunk_count,
        "chunkIndexSha256":journal.manifest().body.chunk_index_sha256,"records":actual.summary()}),
    )
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err("usage: qualify_performance_writer FRESH_DIRECTORY FRESH_RECEIPT_JSON".into());
    }
    let root = PathBuf::from(&arguments[0]);
    let receipt_path = PathBuf::from(&arguments[1]);
    match fs::symlink_metadata(&receipt_path) {
        Ok(_) => return Err("Receipt already exists, including links".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // Exclusive directory/file creation; no overwrite, cleanup, or live paths.
    // On any failure the new partial fixture remains for inspection.
    fs::create_dir(&root)?;
    let wav_path = root.join("synthetic-performance-f32.wav");
    let stage = root.join("interrupted-performance.sqlite3");
    let mut wav = new_wav(&wav_path)?;
    let scope = LiveScope {
        worker_generation: LiveId([0x31; 16]),
        session_id: LiveId([0x42; 16]),
        panic_generation: 1,
    };
    let (mut admission, mut renderer, mut tap) = recording_live_pair(scope, RATE, CHANNELS)?;
    let live_control = admission.control();
    let tap_control = tap.control();
    let kinds = [
        LiveSourceKind::ComputerKeys,
        LiveSourceKind::PointerKeys,
        LiveSourceKind::Pads,
        LiveSourceKind::AccessiblePulse,
    ];
    let config = PerformanceConfig {
        take_id: TakeId("synthetic-performance-writer-take-v1".into()),
        info: tap.info(),
        gui_clock: LiveClock {
            domain: "synthetic-gui-observation".into(),
            epoch: "synthetic-gui-epoch-v1".into(),
            // Exact ratio is reduced for the descriptor without changing any
            // observed ticks; this original unreduced declaration is retained.
            ticks_per_second_numerator: 1_000_000_000_000,
            ticks_per_second_denominator: 1_000,
        },
        output_epoch: ClockEpochId("synthetic-performance-output-epoch-v1".into()),
        sources: std::array::from_fn(|index| PerformanceSource {
            binding: LiveSourceBinding {
                kind: kinds[index],
                handle: (index + 1) as u8,
                connection_generation: 1,
            },
            endpoint: EndpointId(format!("synthetic-performance-source-{}", index + 1)),
        }),
    };
    let mut writer = PerformanceWriter::create(&stage, config.clone())?;
    let mut expected = Evidence::default();
    let mut output = vec![0.0f32; CALLBACK_FRAMES * usize::from(CHANNELS)];
    let mut output_bytes = vec![0u8; output.len() * 4];
    let mut rendered_hash = Sha256::new();
    let mut tapped_hash = Sha256::new();
    let mut next_sequence = 1u64;
    let mut event_ordinal = 0usize;
    let mut applied_batches = 0u64;
    let mut empty_batches = 0u64;
    let mut discarded_batches = 0u64;
    let mut panic_clears = 0u64;
    let mut automatic_releases = 0u64;
    let mut ended_voices = 0u64;
    let mut pcm_packets = 0u64;
    let mut packet_count = 0u64;
    let mut peak = 0.0f32;
    let mut offset = 0usize;
    let mut callback_index = 0u64;
    while offset < FRAMES {
        callback_index += 1;
        let count = (FRAMES - offset).min(CALLBACK_FRAMES);
        let before_scope = admission.scope();
        let panic_now = offset == PANIC_FRAME;
        let batch_count = if offset < PANIC_FRAME { 2 } else { 1 };
        let mut planned = Vec::with_capacity(2);
        for _ in 0..batch_count {
            let token = admission
                .lease_credit()
                .ok_or("Missing genuine one-use lease credit")?
                .token;
            let mut batch = LiveBatch::empty(before_scope, next_sequence, Some(token));
            if offset < PANIC_FRAME {
                for _ in 0..32 {
                    batch.try_push(fixture_event(event_ordinal, offset))?;
                    event_ordinal += 1;
                }
            } else if panic_now {
                batch.try_push(fixture_event(event_ordinal, offset))?;
            }
            let progress = admission.admit(batch)?;
            assert!(progress.fault.is_none());
            assert_eq!(progress.worker_accepted_sequence, Some(next_sequence));
            // The previous credit was consumed once; only a genuinely newly
            // issued token is ever used by the following admission.
            assert_ne!(admission.lease_credit().unwrap().token, token);
            planned.push((
                batch,
                if panic_now {
                    BatchDisposition::DiscardedPanic
                } else {
                    BatchDisposition::Applied
                },
            ));
            next_sequence += 1;
        }
        // Expected applied originals originate from admitted batches, not the
        // writer, recovered journal, or a synthetic manufactured tap stream.
        for (index, (batch, disposition)) in planned.iter().enumerate() {
            if *disposition == BatchDisposition::Applied {
                for (event_index, original) in
                    batch.events[..usize::from(batch.len)].iter().enumerate()
                {
                    let original = original.ok_or("Missing planned event")?;
                    expected.sources[usize::from(original.source - 1)].add(&local(
                        PerformanceRecord::AppliedEvent {
                            callback_index,
                            batch_ordinal: (index + 1) as u16,
                            event_index: event_index as u8,
                            scope: batch.scope,
                            batch_sequence: batch.sequence,
                            applied_frame: offset as u64,
                            original,
                        },
                    ))?;
                }
            }
        }
        let next_scope = if panic_now {
            Some(admission.begin_panic(before_scope)?)
        } else {
            None
        };
        let samples = &mut output[..count * usize::from(CHANNELS)];
        renderer.render_into(samples);
        assert!(
            samples
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
        );
        peak = samples
            .iter()
            .fold(peak, |old, sample| old.max(sample.abs()));
        if offset >= PANIC_FRAME {
            assert!(
                samples.iter().all(|sample| sample.to_bits() == 0),
                "Panic must leave exact final software-bus silence"
            );
        }
        for (sample, bytes) in samples.iter().zip(output_bytes.as_chunks_mut::<4>().0) {
            bytes.copy_from_slice(&sample.to_le_bytes());
        }
        let bytes = &output_bytes[..samples.len() * 4];
        rendered_hash.update(bytes);
        wav.write_all(bytes)?;
        let mut ordinal = 0u16;
        let mut seen_batches = 0usize;
        let mut pcm_at = offset;
        let mut saw_end = false;
        while let Some(packet) = tap.try_pop()? {
            assert_eq!(packet.callback_index, callback_index);
            assert_eq!(packet.ordinal, ordinal);
            assert!(!saw_end);
            packet_count += 1;
            ordinal += 1;
            match packet.kind {
                PerformancePacketKind::Begin(begin) => {
                    assert_eq!(packet.ordinal, 0);
                    assert_eq!(begin.scope, before_scope);
                    assert_eq!(begin.start_frame, offset as u64);
                    assert_eq!(usize::from(begin.frames), count);
                    assert_eq!(begin.sample_rate, RATE);
                    assert_eq!(begin.channels, CHANNELS);
                    expected.control.add(&local(PerformanceRecord::Begin {
                        callback_index,
                        begin,
                    }))?;
                }
                PerformancePacketKind::Batch { batch, disposition } => {
                    assert_eq!(
                        (batch, disposition),
                        *planned.get(seen_batches).ok_or("Unexpected tapped batch")?
                    );
                    seen_batches += 1;
                    match disposition {
                        BatchDisposition::Applied => {
                            applied_batches += 1;
                            empty_batches += u64::from(batch.len == 0);
                        }
                        BatchDisposition::DiscardedPanic => discarded_batches += 1,
                        _ => return Err("Unexpected healthy-fixture batch disposition".into()),
                    }
                    expected.control.add(&local(PerformanceRecord::Batch {
                        callback_index,
                        packet_ordinal: packet.ordinal,
                        batch: Box::new(batch),
                        disposition,
                    }))?;
                }
                PerformancePacketKind::Pcm(pcm) => {
                    assert_eq!(pcm.start_frame, pcm_at as u64);
                    assert_eq!(pcm.channels, CHANNELS);
                    let length = usize::from(pcm.frames) * usize::from(CHANNELS);
                    let start = (pcm_at - offset) * usize::from(CHANNELS);
                    assert!(start + length <= samples.len());
                    for (tapped, rendered) in pcm.samples[..length]
                        .iter()
                        .zip(&samples[start..start + length])
                    {
                        assert_eq!(tapped.to_bits(), rendered.to_bits());
                        tapped_hash.update(tapped.to_le_bytes());
                    }
                    assert!(
                        pcm.samples[length..]
                            .iter()
                            .all(|sample| sample.to_bits() == 0)
                    );
                    pcm_at += usize::from(pcm.frames);
                    pcm_packets += 1;
                }
                PerformancePacketKind::Finish(_) => panic!("Legacy fixture never requests Finish"),
                PerformancePacketKind::End(end) => {
                    assert_eq!(pcm_at, offset + count);
                    assert_eq!(end.end_frame, pcm_at as u64);
                    assert_eq!(end.scope_after, next_scope.unwrap_or(before_scope));
                    assert!(end.first_fault.is_none());
                    assert_eq!(
                        end.applied_sequence,
                        admission.progress().callback_applied_sequence
                    );
                    for clear in end.clears.into_iter().flatten() {
                        assert!(panic_now);
                        assert_eq!(clear.frame, PANIC_FRAME as u64);
                        assert_eq!(clear.scope, before_scope);
                        assert_eq!(
                            clear.reason,
                            TapClearReason::Panic {
                                next_generation: before_scope.panic_generation + 1
                            }
                        );
                        panic_clears += 1;
                        expected.control.add(&local(PerformanceRecord::Clear {
                            callback_index,
                            clear,
                        }))?;
                    }
                    for transition in end.transitions[..usize::from(end.transition_len)]
                        .iter()
                        .flatten()
                    {
                        match transition.kind {
                            omniusong_engine::performance_tap::TapTransitionKind::AutoRelease {
                                ..
                            } => automatic_releases += 1,
                            omniusong_engine::performance_tap::TapTransitionKind::VoiceEnded => {
                                ended_voices += 1
                            }
                        }
                        expected.control.add(&local(PerformanceRecord::Transition {
                            callback_index,
                            transition: *transition,
                        }))?;
                    }
                    expected.control.add(&local(PerformanceRecord::End {
                        callback_index,
                        end_frame: end.end_frame,
                        scope_after: end.scope_after,
                        applied_sequence: end.applied_sequence,
                        active_voices: end.active_voices,
                        release_tail_voices: end.release_tail_voices,
                        clear_count: end.clears.iter().flatten().count() as u16,
                        transition_count: end.transition_len,
                        first_fault: end.first_fault,
                    }))?;
                    saw_end = true;
                }
            }
            // Only this off-callback path serializes, hashes and performs SQL.
            // Drain after EVERY real renderer invocation, not after the take.
            writer.push_packet(packet)?;
        }
        assert!(saw_end);
        assert_eq!(seen_batches, planned.len());
        let snapshot = tap_control.snapshot()?;
        assert_eq!(snapshot.completed_callback_index, callback_index);
        assert_eq!(snapshot.completed_frames, (offset + count) as u64);
        assert!(snapshot.fault.is_none());
        if let Some(next) = next_scope {
            let progress = admission
                .finish_panic()?
                .ok_or("Missing applied Panic acknowledgment")?;
            assert_eq!(progress.scope, next);
            next_sequence = 1;
        }
        let progress = writer.progress();
        assert_eq!(progress.complete_frames, (offset + count) as u64);
        assert!(progress.first_error.is_none());
        assert!(progress.capture_fault.is_none());
        assert!(progress.complete_frames - progress.durable_frames < u64::from(RATE).div_ceil(4));
        offset += count;
    }
    assert_eq!(event_ordinal, 4_160);
    assert!(event_ordinal > 4_096);
    assert_eq!(callback_index, FRAMES.div_ceil(CALLBACK_FRAMES) as u64);
    assert_eq!(discarded_batches, 1);
    assert_eq!(panic_clears, 1);
    assert_eq!(automatic_releases, 4);
    assert!(ended_voices > 0 && peak > 0.0);
    assert!(
        expected
            .sources
            .iter()
            .all(|source| source.records.count > 0)
    );
    let render_sha256 = digest(&rendered_hash);
    assert_eq!(digest(&tapped_hash), render_sha256);
    assert!(!tap_control.snapshot()?.producer_destroyed);
    drop(renderer);
    // This is real renderer/tap destruction, but there was NO native error
    // closure or stream. It is not the future native quiescence/Finish fence.
    assert!(live_control.render_destroyed());
    assert!(!live_control.error_destroyed());
    let producer_snapshot = tap_control.snapshot()?;
    assert!(producer_snapshot.producer_destroyed);
    assert!(!producer_snapshot.cancelled && producer_snapshot.fault.is_none());
    assert!(tap.try_pop()?.is_none());
    wav.sync_all()?;
    drop(wav);
    let retained = writer.close_interrupted()?;
    assert!(retained.unknown_tail);
    assert_eq!(retained.progress.durable_frames, FRAMES as u64);
    assert_eq!(retained.progress.durable_callbacks, callback_index);
    assert!(!retained.progress.capture_fault_pending);
    let before_review = file_identity(&stage)?;
    let reader = CaptureReader::open(&stage)?;
    assert_ne!(reader.inspection().report.state, TakeState::Sealed);
    let inspection = inspect_performance(&reader)?;
    assert!(inspection.problem.is_none(), "{:?}", inspection.problem);
    assert!(inspection.unknown_tail);
    let checkpoint = inspection
        .checkpoint
        .as_ref()
        .ok_or("No verified checkpoint")?;
    assert_eq!(checkpoint.audio_end, FRAMES as u64);
    assert_eq!(checkpoint.last_callback_index, callback_index);
    assert_eq!(
        inspection.checkpoint_sha256,
        retained.progress.checkpoint_sha256
    );
    assert_eq!(
        inspection
            .selection
            .as_ref()
            .ok_or("No selected prefix")?
            .chunk_count,
        reader.inspection().report.verified_chunks
    );
    drop(reader);
    assert_eq!(
        file_identity(&stage)?,
        before_review,
        "Read-only inspection changed the original"
    );
    let reader = CaptureReader::open(&stage)?;
    let recovery_operation = uuid::Uuid::new_v4().to_string();
    let (manifest, replayed) = recover_performance(reader, &inspection, &recovery_operation)?;
    assert!(!replayed);
    let after_recovery = file_identity(&stage)?;
    let recovered_reader = CaptureReader::open(&stage)?;
    assert_eq!(
        recovered_reader.inspection().original_rows_sha256,
        inspection.original_rows_sha256
    );
    let (replayed_manifest, recovery_retry_replayed) =
        recover_performance(recovered_reader, &inspection, &recovery_operation)?;
    assert!(recovery_retry_replayed);
    assert_eq!(replayed_manifest, manifest);
    assert_eq!(file_identity(&stage)?, after_recovery);
    let sealed = SealedJournal::open(&stage)?;
    assert_eq!(sealed.manifest(), &manifest);
    let recovered_verification = verify_sealed(&sealed, &config, &expected, &render_sha256)?;
    assert!(FRAMES * usize::from(CHANNELS) * 4 > 30 * 1024 * 1024);

    // Explicit publication into FRESH isolated stores only. The complete
    // six-stream journal is an owned object; the audio clip selects one stream.
    let mut store = NativeStore::open(root.join("store"))?;
    let project = store.create(&ProjectDocument::new(
        "Synthetic interrupted performance writer qualification",
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
        track_name: "Recovered synthetic performance · uncalibrated solo placement".into(),
        clip: TakeClip {
            id: uuid::Uuid::new_v4().to_string(),
            source: reference.clone(),
            range: TakeRange::AudioFrames {
                start: 0,
                count: FRAMES as u64,
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
    let owned_view = store.open_take(&reference)?;
    assert_eq!(
        verify_sealed(&owned_view, &config, &expected, &render_sha256)?,
        recovered_verification
    );
    let undone = store.undo(project.id(), attached.revision())?;
    assert_eq!(undone.tracks(), project.tracks());
    let redone = store.redo(project.id(), undone.revision())?;
    assert_eq!(redone.tracks(), attached.tracks());
    let archive_path = root.join("synthetic-performance.omniusong-native");
    let archive = store.export_archive(project.id(), &archive_path)?;
    let mut restored = NativeStore::open(root.join("restored-store"))?;
    let restored_document = restored.restore_archive(&archive_path)?;
    assert_eq!(restored_document, redone);
    let restored_owned = restored.registered_take(&config.take_id)?;
    assert_eq!(restored_owned, owned);
    let restored_view = restored.open_take(&reference)?;
    assert_eq!(
        verify_sealed(&restored_view, &config, &expected, &render_sha256)?,
        recovered_verification
    );
    assert_eq!(
        file_identity(&stage)?,
        after_recovery,
        "Publication/archive changed the original stage"
    );
    for (label, document) in [
        ("initial", &project),
        ("attached", &attached),
        ("undone", &undone),
        ("redone", &redone),
        ("restored", &restored_document),
    ] {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(root.join(format!("project-{label}.json")))?;
        file.write_all(&document.to_json()?)?;
        file.sync_all()?;
    }
    let receipt = json!({
        "format":"omniusong.performance-writer-qualification","version":1,"syntheticOnly":true,
        "root":root,"config":config,
        "render":{"sampleRate":RATE,"channels":CHANNELS,"frames":FRAMES,
            "maximumCallbackFrames":CALLBACK_FRAMES,"callbacks":callback_index,"tailCallbackFrames":FRAMES % CALLBACK_FRAMES,
            "pcmPayloadBytes":FRAMES * usize::from(CHANNELS) * 4,"exceeds30MiB":true,
            "tapPackets":packet_count,"tapPcmPackets":pcm_packets,"peak":peak,"finalSilentFrames":FRAMES-PANIC_FRAME,
            "renderedPcmSha256":render_sha256,"tappedPcmSha256":digest(&tapped_hash),"wav":file_identity(&wav_path)?},
        "events":{"appliedLocalEvents":event_ordinal,"exceeds4096AppliedEvents":true,
            "appliedBatches":applied_batches,"emptyLeaseBatches":empty_batches,"panicDiscardedBatches":discarded_batches,
            "panicFrame":PANIC_FRAME,"panicClears":panic_clears,"automaticReleases":automatic_releases,"endedVoices":ended_voices,
            "expectedOriginalRecords":expected.summary()},
        "producerDestruction":{"completedCallbackIndex":producer_snapshot.completed_callback_index,
            "completedFrames":producer_snapshot.completed_frames,"producerDestroyed":producer_snapshot.producer_destroyed,
            "cancelled":producer_snapshot.cancelled,"fault":null,"rendererDestroyed":true,
            "nativeErrorClosureCreated":false,"nativeErrorClosureDestroyed":false},
        "recovery":{"retained":retained,"readOnlyInspection":inspection,"readOnlyOriginalUnchanged":true,
            "explicitRecoveryOperation":recovery_operation,"firstRecoveryReplayed":replayed,"exactRetryReplayed":recovery_retry_replayed,
            "originalBeforeRecovery":before_review,"originalAfterRecovery":after_recovery,"originalRowsPreserved":true,
            "verifiedSealedStreams":recovered_verification},
        "publication":{"ownedTake":owned,"intent":publication,"projectId":project.id(),
            "attachedRevision":attached.revision(),"undoRevision":undone.revision(),"redoRevision":redone.revision()},
        "archive":{"file":file_identity(&archive_path)?,"historyRows":archive.history_rows,"operationRows":archive.operation_rows,
            "takeCount":archive.take_count,"restoredProjectId":restored_document.id(),"restoredRevision":restored_document.revision(),
            "restoredOwnedTake":restored_owned,"completeSixStreamRestorationVerified":true},
        "bounds":{"callbackSampleBufferBytes":CALLBACK_FRAMES * usize::from(CHANNELS) * 4,
            "callbackByteBufferBytes":CALLBACK_FRAMES * usize::from(CHANNELS) * 4,
            "maximumPlannedBatches":2,"perSourceRecordAccumulators":4,"receiptLimitBytes":MAX_RECEIPT_BYTES},
        "limits":[
            "Device-free production renderer/tap/writer only; no native input/output streams, IPC or GUI recording.",
            "Frames are generated offline, not elapsed capture or a measured 30-minute reliability test.",
            "Renderer destruction acknowledges this synthetic producer only; no native stream/error-closure quiescence or normal Finish is claimed.",
            "Explicit checkpoint recovery retains unknown tail; it does not resume, infer missing duration, or fabricate silence/gaps.",
            "PCM is the logical F32 post-mix bus, not physical sound, listening, integer device conversion or calibrated alignment.",
            "Source clocks remain original observations; the new audio clip has explicit uncalibrated frame-zero solo placement.",
            "No editable-note derivation, human performance, MIDI/MPE, plugins, inference, Windows or release acceptance."
        ]
    });
    let bytes = serde_json::to_vec_pretty(&receipt)?;
    assert!(bytes.len() <= MAX_RECEIPT_BYTES);
    let mut receipt_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)?;
    receipt_file.write_all(&bytes)?;
    receipt_file.write_all(b"\n")?;
    receipt_file.sync_all()?;
    println!("{}", receipt_path.display());
    Ok(())
}
