//! Device-free production-renderer/tap qualification, not a recording engine.
//! Fixed 2.25-second synthetic phrase; creates only a new F32 WAV and receipt.
use omniusong_engine::{
    LiveBatch, LiveEvent, LiveEventKind, LiveId, LivePatch, LiveScope,
    live::recording_live_pair,
    performance_tap::{BatchDisposition, PerformancePacketKind, TapClearReason, TapTransitionKind},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const FRAMES: usize = 108_000;
const CALLBACK_FRAMES: usize = 512;
const MAX_RECEIPT_BYTES: usize = 1024 * 1024;
const OBSERVED_ORIGIN: u64 = 9_007_199_254_740_993;

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_identity(path: &Path) -> Result<Value, Box<dyn Error>> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        hash.update(&buffer[..read]);
    }
    Ok(json!({"path":path,"byteLength":count,"sha256":format!("{:x}", hash.finalize())}))
}
fn new_wav(path: &Path) -> Result<File, Box<dyn Error>> {
    let bytes = u32::try_from(FRAMES * usize::from(CHANNELS) * 4)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    // IEEE float WAVE with a fact chunk and an exact, fixed data size.
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
    file.write_all(&(FRAMES as u32).to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    Ok(file)
}
fn event(source: u8, frame: usize, kind: LiveEventKind) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        // Deliberately opaque, >2^53 source ticks. They are NOT audio frames or
        // an asserted host-to-device calibration.
        observed_ticks: OBSERVED_ORIGIN + frame as u64 * 100 + u64::from(source),
        event: kind,
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
    match frame {
        0 => vec![event(1, frame, on(1, 1, 60, u16::MAX, LivePatch::Keys))],
        1_024 => vec![event(1, frame, on(2, 2, 60, 32_769, LivePatch::Keys))],
        4_096 => vec![
            event(1, frame, LiveEventKind::Sustain { down: true }),
            event(1, frame, off(1, u16::MAX)),
            event(2, frame, on(1, 1, 48, 49_152, LivePatch::Bass)),
        ],
        5_120 => vec![
            event(1, frame, off(2, 12_345)),
            event(2, frame, LiveEventKind::Sustain { down: true }),
        ],
        6_144 => vec![event(2, frame, off(1, 1))],
        8_192 => vec![event(
            3,
            frame,
            pulse(1, 1, 36, 40_000, 50_000, LivePatch::Drums, 2_400),
        )],
        12_288 => vec![
            event(1, frame, LiveEventKind::Sustain { down: false }),
            event(
                4,
                frame,
                pulse(1, 1, 72, 50_000, u16::MAX, LivePatch::Pad, 4_800),
            ),
        ],
        16_384 => vec![event(2, frame, LiveEventKind::Sustain { down: false })],
        20_480 => vec![event(1, frame, on(3, 3, 67, 0, LivePatch::Keys))],
        21_504 => vec![event(1, frame, off(3, 0))],
        24_576 => vec![event(
            3,
            frame,
            pulse(2, 2, 38, u16::MAX, 0, LivePatch::Drums, 4_800),
        )],
        28_672 => vec![event(
            4,
            frame,
            pulse(2, 2, 72, 45_000, u16::MAX, LivePatch::Pad, 12_000),
        )],
        // The next two scopes intentionally reuse local note/voice ordinals.
        // Their full scope/source identity must remain in the original trace.
        33_280 => vec![
            event(1, frame, on(1, 1, 64, 65_001, LivePatch::Keys)),
            event(2, frame, on(1, 1, 48, 49_152, LivePatch::Bass)),
        ],
        34_816 => vec![event(
            3,
            frame,
            pulse(1, 1, 42, 49_152, 12_345, LivePatch::Drums, 4_800),
        )],
        36_864 => vec![event(1, frame, LiveEventKind::Sustain { down: true })],
        37_888 => vec![event(1, frame, off(1, u16::MAX))],
        40_960 => vec![
            event(1, frame, LiveEventKind::Sustain { down: false }),
            event(2, frame, off(1, 32_768)),
        ],
        45_056 => vec![event(
            4,
            frame,
            pulse(1, 1, 72, 40_001, 65_000, LivePatch::Pad, 2_400),
        )],
        61_440 => vec![event(2, frame, on(2, 2, 36, u16::MAX, LivePatch::Bass))],
        _ => vec![],
    }
}
fn disposition_json(disposition: BatchDisposition) -> Value {
    match disposition {
        BatchDisposition::Applied => json!({"kind":"applied"}),
        BatchDisposition::DiscardedPanic => json!({"kind":"discarded_panic"}),
        BatchDisposition::DiscardedStale => json!({"kind":"discarded_stale"}),
        BatchDisposition::Rejected { fault } => {
            json!({"kind":"rejected","fault":format!("{fault:?}")})
        }
    }
}
fn clear_json(clear: omniusong_engine::performance_tap::TapClear) -> Value {
    let reason = match clear.reason {
        TapClearReason::Panic { next_generation } => {
            json!({"kind":"panic","nextGeneration":next_generation})
        }
        TapClearReason::Fault { kind } => json!({"kind":"fault","fault":format!("{kind:?}")}),
        TapClearReason::FaultPending => json!({"kind":"fault_pending"}),
    };
    json!({"scope":clear.scope,"frame":clear.frame,"reason":reason})
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err("usage: qualify_performance_tap FRESH_DIRECTORY FRESH_RECEIPT_JSON".into());
    }
    let root = PathBuf::from(&arguments[0]);
    let receipt_path = PathBuf::from(&arguments[1]);
    match fs::symlink_metadata(&receipt_path) {
        Ok(_) => return Err("Receipt path already exists (including links)".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // Exclusive allocation rejects existing directories, files and dangling
    // links. Failure retains partial new fixture outputs; it deletes nothing.
    fs::create_dir(&root)?;
    let wav_path = root.join("synthetic-performance-tap-f32.wav");
    let mut wav = new_wav(&wav_path)?;
    let initial_scope = LiveScope {
        worker_generation: LiveId([0x11; 16]),
        session_id: LiveId([0x22; 16]),
        panic_generation: 1,
    };
    let (mut admission, mut renderer, mut tap) =
        recording_live_pair(initial_scope, RATE, CHANNELS)?;
    let control = admission.control();
    let tap_control = tap.control();
    assert_eq!(tap.info().scope, initial_scope);
    assert_eq!(tap.info().sample_rate, RATE);
    assert_eq!(tap.info().channels, CHANNELS);
    let mut callback_trace = Vec::with_capacity(FRAMES.div_ceil(CALLBACK_FRAMES));
    let mut normalized_batches = Vec::new();
    let mut automatic_releases = Vec::new();
    let mut clears_seen = Vec::new();
    let mut render_hash = Sha256::new();
    let mut tap_hash = Sha256::new();
    let mut samples = [0.0f32; CALLBACK_FRAMES * CHANNELS as usize];
    let mut bytes = [0u8; CALLBACK_FRAMES * CHANNELS as usize * 4];
    let mut next_sequence = 1u64;
    let mut callback_index = 0u64;
    let mut tap_pcm_packets = 0usize;
    let mut applied_batches = 0usize;
    let mut discarded_panic_batches = 0usize;
    let mut event_count = 0usize;
    let mut peak = 0.0f32;
    let mut offset = 0usize;
    while offset < FRAMES {
        let before_scope = admission.scope();
        let count = (FRAMES - offset).min(CALLBACK_FRAMES);
        let panic_now = matches!(offset, 32_768 | 65_536);
        let events = if panic_now {
            // Admission succeeds, then Panic fences the batch before callback
            // application. The tap must preserve it as discarded, never played.
            vec![event(
                1,
                offset,
                LiveEventKind::NoteOn {
                    note: if offset == 32_768 { 4 } else { 2 },
                    voice: if offset == 32_768 { 4 } else { 2 },
                    key: 77,
                    velocity16: 54_321,
                    patch: LivePatch::Keys,
                },
            )]
        } else {
            phrase(offset)
        };
        let mut expected_batch = None;
        if !events.is_empty() || offset.is_multiple_of(12_288) {
            let mut batch = LiveBatch::empty(
                before_scope,
                next_sequence,
                admission.lease_credit().map(|credit| credit.token),
            );
            for event in events {
                batch.try_push(event)?;
                event_count += 1;
            }
            let progress = admission.admit(batch)?;
            assert_eq!(progress.worker_accepted_sequence, Some(next_sequence));
            assert!(progress.fault.is_none());
            expected_batch = Some((
                batch,
                if panic_now {
                    BatchDisposition::DiscardedPanic
                } else {
                    BatchDisposition::Applied
                },
            ));
            normalized_batches.push(json!({"scope":batch.scope,"sequence":batch.sequence,"events":&batch.events[..usize::from(batch.len)],"plannedCallbackFrame":offset,"expectedDisposition":disposition_json(expected_batch.unwrap().1)}));
            next_sequence = next_sequence
                .checked_add(1)
                .ok_or("Fixture sequence exhausted")?;
        }
        let next_scope = if panic_now {
            Some(admission.begin_panic(before_scope)?)
        } else {
            None
        };
        let output = &mut samples[..count * usize::from(CHANNELS)];
        renderer.render_into(output);
        callback_index += 1;
        assert!(
            output
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
        );
        peak = output
            .iter()
            .fold(peak, |peak, sample| peak.max(sample.abs()));
        if offset >= 65_536 {
            assert!(
                output.iter().all(|sample| sample.to_bits() == 0),
                "Final Panic must leave exact software-bus silence"
            );
        }
        for (sample, destination) in output.iter().zip(bytes.as_chunks_mut::<4>().0) {
            destination.copy_from_slice(&sample.to_le_bytes());
        }
        let rendered_bytes = &bytes[..output.len() * 4];
        render_hash.update(rendered_bytes);
        wav.write_all(rendered_bytes)?;

        let mut packet_trace = Vec::with_capacity(5);
        let mut ordinal = 0u16;
        let mut pcm_at = offset;
        let mut saw_begin = false;
        let mut saw_end = false;
        let mut saw_batch = false;
        while let Some(packet) = tap.try_pop()? {
            assert_eq!(packet.callback_index, callback_index);
            assert_eq!(packet.ordinal, ordinal);
            ordinal += 1;
            assert!(!saw_end, "No packet may follow callback End");
            match packet.kind {
                PerformancePacketKind::Begin(begin) => {
                    assert!(!saw_begin && packet.ordinal == 0);
                    saw_begin = true;
                    assert_eq!(begin.scope, before_scope);
                    assert_eq!(begin.start_frame, offset as u64);
                    assert_eq!(usize::from(begin.frames), count);
                    assert_eq!(begin.sample_rate, RATE);
                    assert_eq!(begin.channels, CHANNELS);
                    packet_trace.push(json!({"ordinal":packet.ordinal,"kind":"begin","scope":begin.scope,"startFrame":begin.start_frame,"frames":begin.frames,"sampleRate":begin.sample_rate,"channels":begin.channels}));
                }
                PerformancePacketKind::Batch { batch, disposition } => {
                    assert!(saw_begin && pcm_at == offset && !saw_batch);
                    saw_batch = true;
                    assert_eq!(
                        Some((batch, disposition)),
                        expected_batch,
                        "Exact original batch, including lease token, must survive tap delivery"
                    );
                    match disposition {
                        BatchDisposition::Applied => applied_batches += 1,
                        BatchDisposition::DiscardedPanic => discarded_panic_batches += 1,
                        _ => return Err("Unexpected disposition in healthy fixture".into()),
                    }
                    packet_trace.push(json!({"ordinal":packet.ordinal,"kind":"batch","applicationBoundaryFrame":offset,"batch":batch,"disposition":disposition_json(disposition)}));
                }
                PerformancePacketKind::Pcm(pcm) => {
                    assert!(saw_begin);
                    assert_eq!(pcm.start_frame, pcm_at as u64);
                    assert_eq!(pcm.channels, CHANNELS);
                    assert!(pcm.frames > 0 && usize::from(pcm.frames) <= 256);
                    let length = usize::from(pcm.frames) * usize::from(CHANNELS);
                    let start = (pcm_at - offset) * usize::from(CHANNELS);
                    assert!(pcm_at + usize::from(pcm.frames) <= offset + count);
                    assert!(
                        pcm.samples[..length]
                            .iter()
                            .zip(&output[start..start + length])
                            .all(|(tapped, rendered)| tapped.to_bits() == rendered.to_bits()),
                        "Tap PCM must equal returned render_into PCM bit for bit"
                    );
                    assert!(
                        pcm.samples[length..]
                            .iter()
                            .all(|sample| sample.to_bits() == 0)
                    );
                    let mut pcm_hash = Sha256::new();
                    for sample in &pcm.samples[..length] {
                        let value = sample.to_le_bytes();
                        tap_hash.update(value);
                        pcm_hash.update(value);
                    }
                    packet_trace.push(json!({"ordinal":packet.ordinal,"kind":"pcm","startFrame":pcm.start_frame,"frames":pcm.frames,"channels":pcm.channels,"f32leSha256":format!("{:x}", pcm_hash.finalize())}));
                    tap_pcm_packets += 1;
                    pcm_at += usize::from(pcm.frames);
                }
                PerformancePacketKind::End(end) => {
                    assert!(saw_begin);
                    saw_end = true;
                    assert_eq!(pcm_at, offset + count);
                    assert_eq!(end.end_frame, pcm_at as u64);
                    assert!(end.first_fault.is_none());
                    assert_eq!(end.scope_after, next_scope.unwrap_or(before_scope));
                    let live = admission.progress();
                    assert_eq!(end.applied_sequence, live.callback_applied_sequence);
                    assert_eq!(end.active_voices, live.active_voices);
                    assert_eq!(end.release_tail_voices, live.release_tail_voices);
                    let clears: Vec<_> = end.clears.into_iter().flatten().map(clear_json).collect();
                    clears_seen.extend(clears.iter().cloned());
                    if panic_now {
                        assert_eq!(end.active_voices, 0);
                        assert_eq!(end.release_tail_voices, 0);
                        assert_eq!(end.applied_sequence, None);
                        assert_eq!(end.clears[0].unwrap().scope, before_scope);
                        assert_eq!(end.clears[0].unwrap().frame, offset as u64);
                        assert_eq!(
                            end.clears[0].unwrap().reason,
                            TapClearReason::Panic {
                                next_generation: before_scope.panic_generation + 1
                            }
                        );
                        assert!(end.clears[1].is_none());
                    } else {
                        assert!(clears.is_empty());
                    }
                    let mut transitions = Vec::with_capacity(usize::from(end.transition_len));
                    for (index, transition) in end.transitions.into_iter().enumerate() {
                        if index >= usize::from(end.transition_len) {
                            assert!(transition.is_none());
                            continue;
                        }
                        let transition =
                            transition.ok_or("Missing transition in declared prefix")?;
                        assert!(
                            transition.frame >= offset as u64 && transition.frame <= end.end_frame
                        );
                        let kind = match transition.kind {
                            TapTransitionKind::AutoRelease { release_velocity16 } => {
                                automatic_releases.push((
                                    transition.scope.panic_generation,
                                    transition.source,
                                    transition.voice,
                                    transition.frame,
                                    release_velocity16,
                                ));
                                json!({"kind":"auto_release","releaseVelocity16":release_velocity16})
                            }
                            TapTransitionKind::VoiceEnded => json!({"kind":"voice_ended"}),
                        };
                        transitions.push(json!({"scope":transition.scope,"source":transition.source,"voice":transition.voice,"frame":transition.frame,"transition":kind}));
                    }
                    packet_trace.push(json!({"ordinal":packet.ordinal,"kind":"end","endFrame":end.end_frame,"scopeAfter":end.scope_after,"appliedSequence":end.applied_sequence,"activeVoices":end.active_voices,"releaseTailVoices":end.release_tail_voices,"clears":clears,"transitions":transitions,"firstFault":null}));
                }
            }
        }
        assert!(saw_begin && saw_end);
        assert_eq!(saw_batch, expected_batch.is_some());
        let snapshot = tap_control.snapshot()?;
        assert_eq!(snapshot.completed_callback_index, callback_index);
        assert_eq!(snapshot.completed_frames, (offset + count) as u64);
        assert!(snapshot.fault.is_none() && !snapshot.cancelled && !snapshot.producer_destroyed);
        callback_trace.push(json!({"callbackIndex":callback_index,"packets":packet_trace}));
        if let Some(next) = next_scope {
            let progress = admission
                .finish_panic()?
                .ok_or("Panic did not complete after its bounded callback")?;
            assert_eq!(progress.scope, next);
            assert!(progress.fault.is_none());
            assert_eq!(progress.active_voices, 0);
            assert!(progress.lease_credit.is_some());
            next_sequence = 1;
        }
        offset += count;
    }
    assert_eq!(callback_index as usize, FRAMES.div_ceil(CALLBACK_FRAMES));
    assert_eq!(tap_pcm_packets, 2 * callback_index as usize);
    assert_eq!(discarded_panic_batches, 2);
    assert_eq!(clears_seen.len(), 2);
    assert!(peak > 0.01);
    for expected in [
        (1, 3, 1, 10_592, 50_000),
        (1, 4, 1, 17_088, u16::MAX),
        (1, 3, 2, 29_376, 0),
        (2, 3, 1, 39_616, 12_345),
        (2, 4, 1, 47_456, 65_000),
    ] {
        assert!(
            automatic_releases.contains(&expected),
            "Exact callback-owned Pulse release missing: {expected:?}"
        );
    }
    let final_progress = admission.progress();
    assert_eq!(final_progress.scope.panic_generation, 3);
    assert_eq!(final_progress.frames, FRAMES as u64);
    assert_eq!(final_progress.active_voices, 0);
    assert_eq!(final_progress.release_tail_voices, 0);
    assert!(final_progress.fault.is_none());
    let rendered_sha = format!("{:x}", render_hash.finalize());
    let tapped_sha = format!("{:x}", tap_hash.finalize());
    assert_eq!(rendered_sha, tapped_sha);
    drop(renderer);
    assert!(control.render_destroyed());
    assert!(
        !control.error_destroyed(),
        "No native error closure was constructed in this fixture"
    );
    assert!(tap.try_pop()?.is_none());
    let final_snapshot = tap_control.snapshot()?;
    assert!(
        final_snapshot.producer_destroyed
            && !final_snapshot.cancelled
            && final_snapshot.fault.is_none()
    );
    assert_eq!(final_snapshot.completed_callback_index, callback_index);
    assert_eq!(final_snapshot.completed_frames, FRAMES as u64);
    // Admission retains callback backing through the whole renderer destructor.
    drop(admission);
    wav.sync_all()?;
    drop(wav);
    let trace_bytes = serde_json::to_vec(&callback_trace)?;
    let music_bytes = serde_json::to_vec(&normalized_batches)?;
    let receipt = json!({"format":"omniusong.performance-tap-qualification","version":1,
        "syntheticOnly":true,"humanPerformance":false,"deviceOpened":false,"nativeInputOpened":false,"nativeOutputOpened":false,"ipcUsed":false,"journalCreated":false,"storeCreated":false,"projectModified":false,
        "frames":FRAMES,"sampleRate":RATE,"channels":CHANNELS,"durationSeconds":2.25,"callbackFrames":CALLBACK_FRAMES,"callbacks":callback_index,"pcmPackets":tap_pcm_packets,"eventCount":event_count,"appliedBatches":applied_batches,"discardedPanicBatches":discarded_panic_batches,"peak":peak,
        "initialScope":initial_scope,"finalProgress":final_progress,"renderedF32leSha256":rendered_sha,"tappedF32leSha256":tapped_sha,"bitwiseTapMatchesRendered":true,"finalSilentSourceFrames":{"start":65536,"count":FRAMES-65536},
        "observedClock":{"origin":OBSERVED_ORIGIN,"source":"synthetic opaque ticks","calibratedToAudio":false},
        "preparedSources":[{"handle":1,"kind":"computer_keys","connectionGeneration":1},{"handle":2,"kind":"pointer_keys","connectionGeneration":1},{"handle":3,"kind":"pads","connectionGeneration":1},{"handle":4,"kind":"accessible_pulse","connectionGeneration":1}],
        "callbackTraceSha256":sha(&trace_bytes),"normalizedMusicalPlanSha256":sha(&music_bytes),"callbackTrace":callback_trace,"normalizedMusicalPlan":normalized_batches,"wav":file_identity(&wav_path)?,
        "ownership":{"rendererDestroyed":true,"tapProducerDestroyed":true,"nativeErrorClosureConstructed":false,"nativeErrorClosureDestroyed":false,"nativeCallbackQuiescenceQualified":false},
        "determinism":"Fixed musical plan and PCM; actual one-use lease tokens are random and retained in the trace, so the exact trace hash identifies this run.",
        "limits":["Production renderer and callback transfer only; no incremental journal writer, worker/UI recording, native stream, elapsed recording or durable original was exercised.","No physical latency, heard-audio alignment, MIDI/MPE transport, inference, plugin, listening or Windows acceptance.","Rendered F32 software bus is before native output conversion; no speaker/device bytes are claimed.","WAV and JSON are bounded synthetic fixture outputs; a partial failing run is retained and never overwritten."]});
    let encoded = serde_json::to_vec_pretty(&receipt)?;
    if encoded.len() > MAX_RECEIPT_BYTES {
        return Err("Bounded fixture receipt exceeded 1 MiB".into());
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&receipt_path)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    println!(
        "{}",
        serde_json::to_string(
            &json!({"receipt":receipt_path,"receiptSha256":sha(&encoded),"frames":FRAMES,"bitwiseTapMatchesRendered":true,"syntheticOnly":true})
        )?
    );
    Ok(())
}
