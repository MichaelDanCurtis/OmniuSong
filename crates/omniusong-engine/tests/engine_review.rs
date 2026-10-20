//! Independent device-free audio boundary tests. Never enumerate or open a device.
use omniusong_engine::realtime::{
    INVALID_INPUT, MAX_CALLBACK_FRAMES, MAX_CHANNELS, PACKET_FRAMES, RING_SLOTS, RenderCursor,
    TRANSFER_OVERFLOW, UNKNOWN_DEVICE_FAULT, capture_queue,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

// Count only this test thread's explicitly scoped hot path, so parallel test
// setup/teardown and assertion formatting cannot masquerade as callback work.
struct CountingAllocator;
thread_local! {
    static ALLOCATIONS: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}
fn count(allocations: usize, deallocations: usize) {
    let _ = ALLOCATIONS.try_with(|cell| {
        let (enabled, alloc, dealloc) = cell.get();
        if enabled {
            cell.set((true, alloc + allocations, dealloc + deallocations));
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count(0, 1);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(1, 1);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn hot_path(work: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.with(|cell| cell.set((true, 0, 0)));
    work();
    ALLOCATIONS.with(|cell| {
        let (_, alloc, dealloc) = cell.replace((false, 0, 0));
        (alloc, dealloc)
    })
}

#[test]
fn invalid_queue_configurations_fail_before_activation() {
    for (channels, rate, slots) in [
        (0, 48_000, 1),
        ((MAX_CHANNELS + 1) as u16, 48_000, 1),
        (1, 7_999, 1),
        (1, 768_001, 1),
        (1, 48_000, 0),
        (1, 48_000, RING_SLOTS + 1),
    ] {
        assert!(capture_queue(channels, rate, slots).is_err());
    }
}

#[test]
fn application_capture_and_output_hot_paths_allocate_and_deallocate_nothing() {
    let (mut input, mut consumer, _meters) = capture_queue(2, 48_000, 8).unwrap();
    let (mut overflow, _full_queue, _overflow_meters) = capture_queue(1, 48_000, 1).unwrap();
    let (mut renderer, _output_meters) = RenderCursor::new(vec![0.25; 40], 2).unwrap();
    let f32_input = [0.25; 2 * (PACKET_FRAMES + 3)];
    let overflow_input = [0.5; 2 * PACKET_FRAMES];
    let mut output_f32 = [99.0; 64];
    let mut output_i16 = [99; 16];
    let mut output_u16 = [99; 16];
    let counts = hot_path(|| {
        input.ingest_f32(&f32_input, 1_000_000);
        input.ingest_i16(&[i16::MIN, i16::MAX, 0, 0], 10_000_000);
        input.ingest_u16(&[0, u16::MAX, 32768, 32768], 20_000_000);
        while consumer.pop().is_ok() {}
        overflow.ingest_f32(&overflow_input, 1_000_000);
        overflow.ingest_f32(&overflow_input, 20_000_000);
        renderer.render_into(&mut output_f32);
        renderer.render_i16(&mut output_i16);
        renderer.render_u16(&mut output_u16);
    });
    assert_eq!(
        counts,
        (0, 0),
        "Application callback allocation/deallocation"
    );
    assert!(output_f32[40..].iter().all(|sample| *sample == 0.0));
    assert!(output_i16.iter().all(|sample| *sample == 0));
    assert!(output_u16.iter().all(|sample| *sample == 32768));
}

#[test]
fn variable_callback_packets_preserve_channels_samples_and_source_positions() {
    let (mut input, mut consumer, meters) = capture_queue(2, 48_000, 8).unwrap();
    let samples: Vec<_> = (0..2 * (PACKET_FRAMES + 3))
        .map(|index| index as f32 / 128.0 - 2.0)
        .collect();
    input.ingest_f32(&samples, 1_000_000);
    input.ingest_i16(&[i16::MIN, i16::MAX, 0, -16384], 20_000_000);
    input.ingest_u16(&[0, u16::MAX], 30_000_000);
    let first = consumer.pop().unwrap();
    assert_eq!(
        (first.start_frame, first.frames, first.channels),
        (0, 256, 2)
    );
    assert_eq!(&first.samples[..512], &samples[..512]);
    assert_eq!(first.clock_start_ns, 1_000_000);
    assert_eq!(first.clock_end_ns, 1_000_000 + 256 * 1_000_000_000 / 48_000);
    let tail = consumer.pop().unwrap();
    assert_eq!((tail.start_frame, tail.frames, tail.channels), (256, 3, 2));
    assert_eq!(&tail.samples[..6], &samples[512..]);
    assert!(tail.samples[6..].iter().all(|sample| *sample == 0.0));
    let signed = consumer.pop().unwrap();
    assert_eq!((signed.start_frame, signed.frames), (259, 2));
    assert_eq!(&signed.samples[..4], &[-1.0, 32767.0 / 32768.0, 0.0, -0.5]);
    let unsigned = consumer.pop().unwrap();
    assert_eq!((unsigned.start_frame, unsigned.frames), (261, 1));
    assert_eq!(&unsigned.samples[..2], &[-1.0, 32767.0 / 32768.0]);
    assert!(consumer.pop().is_err());
    let snapshot = meters.snapshot();
    assert_eq!(
        (snapshot.frames, snapshot.dropped_frames, snapshot.faults),
        (262, 0, 0)
    );
    assert!(!snapshot.producer_destroyed);
    // Values outside [-1,1] are retained as original finite float PCM.
    assert!(first.samples.iter().any(|sample| *sample > 1.0));
}

#[test]
fn overflow_retains_original_prefix_and_counts_every_later_rejected_frame() {
    let (mut input, mut consumer, meters) = capture_queue(1, 48_000, 2).unwrap();
    input.ingest_f32(&[0.125; 3 * PACKET_FRAMES], 1_000_000);
    let first = consumer.pop().unwrap();
    assert_eq!(first.start_frame, 0);
    // Freeing a slot does not silently resume this faulted capture.
    input.ingest_f32(&[0.875; 31], 20_000_000);
    let second = consumer.pop().unwrap();
    assert_eq!(second.start_frame, PACKET_FRAMES as u64);
    assert!(
        second.samples[..PACKET_FRAMES]
            .iter()
            .all(|sample| *sample == 0.125)
    );
    assert!(consumer.pop().is_err());
    assert!(!meters.snapshot().producer_destroyed);
    drop(input);
    let final_state = meters.snapshot();
    assert!(final_state.producer_destroyed);
    assert_eq!(final_state.frames, (3 * PACKET_FRAMES + 31) as u64);
    assert_eq!(final_state.dropped_frames, (PACKET_FRAMES + 31) as u64);
    assert_ne!(final_state.faults & TRANSFER_OVERFLOW, 0);
    assert_eq!(
        final_state.frames - final_state.dropped_frames,
        2 * PACKET_FRAMES as u64
    );
}

#[test]
fn empty_ring_is_not_producer_destruction_and_ack_carries_final_extent() {
    let (mut input, mut consumer, meters) = capture_queue(1, 48_000, 2).unwrap();
    assert!(consumer.pop().is_err());
    assert!(!meters.snapshot().producer_destroyed);
    input.ingest_f32(&[0.5; 17], 100);
    assert_eq!(consumer.pop().unwrap().frames, 17);
    assert!(consumer.pop().is_err());
    assert!(!meters.snapshot().producer_destroyed);
    std::thread::spawn(move || {
        input.ingest_f32(&[0.75; 23], 1_000_000);
        drop(input);
    })
    .join()
    .unwrap();
    let final_state = meters.snapshot();
    assert!(final_state.producer_destroyed);
    assert_eq!((final_state.frames, final_state.dropped_frames), (40, 0));
    let tail = consumer.pop().unwrap();
    assert_eq!((tail.start_frame, tail.frames), (17, 23));
    assert!(consumer.pop().is_err());
}

#[test]
fn native_failure_sentinel_nonfinite_input_and_oversize_never_become_valid_audio() {
    let (mut input, mut consumer, meters) = capture_queue(1, 48_000, 2).unwrap();
    input.ingest_f32(&[], 0);
    input.ingest_f32(&[0.5; 10], 1_000_000);
    assert!(consumer.pop().is_err());
    assert_ne!(meters.snapshot().faults & UNKNOWN_DEVICE_FAULT, 0);
    assert_eq!(meters.snapshot().dropped_frames, 10);

    let (mut input, mut consumer, meters) = capture_queue(2, 48_000, 2).unwrap();
    input.ingest_f32(&[0.5, f32::NAN], 0);
    assert!(consumer.pop().is_err());
    assert_ne!(meters.snapshot().faults & INVALID_INPUT, 0);
    assert_eq!(meters.snapshot().frames, 1);

    let (mut input, mut consumer, meters) = capture_queue(1, 48_000, 2).unwrap();
    let data = vec![0.25; MAX_CALLBACK_FRAMES + 1];
    input.ingest_f32(&data, 0);
    assert!(consumer.pop().is_err());
    assert_ne!(meters.snapshot().faults & UNKNOWN_DEVICE_FAULT, 0);
    assert_eq!(meters.snapshot().frames, data.len() as u64);
    assert_eq!(meters.snapshot().dropped_frames, data.len() as u64);
}

#[test]
fn render_tail_preserves_channel_order_and_fills_every_sample_after_end() {
    let (mut renderer, meters) =
        RenderCursor::new(vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3], 2).unwrap();
    let mut first = [9.0; 4];
    renderer.render_into(&mut first);
    assert_eq!(first, [0.1, -0.1, 0.2, -0.2]);
    assert!(!renderer.is_finished());
    let mut final_block = [9.0; 8];
    renderer.render_into(&mut final_block);
    assert_eq!(final_block, [0.3, -0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(renderer.frame_count(), 3);
    assert!(renderer.is_finished());
    assert_eq!(meters.snapshot().frames, 3);
    renderer.render_into(&mut final_block);
    assert_eq!(final_block, [0.0; 8]);
    assert_eq!(meters.snapshot().frames, 3);
    assert!(RenderCursor::new(vec![0.0; 3], 2).is_err());
    assert!(RenderCursor::new(vec![f32::INFINITY], 1).is_err());
}

fn capture_fixture() -> (std::path::PathBuf, omniusong_recording::TakeDescriptor) {
    use omniusong_recording::*;
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/engine-review")
        .join(uuid::Uuid::new_v4().to_string())
        .join("take.sqlite3");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let epoch = ClockEpochId(uuid::Uuid::new_v4().to_string());
    let descriptor = TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "review-segment".into(),
            source_id: "synthetic-review-input".into(),
            source_name: "Device-free reviewer fixture".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("review-clock".into()),
                epoch: epoch.clone(),
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
                audio_epoch: epoch,
            },
            initial_source_position: 0,
        }],
        provenance: Default::default(),
    };
    (path, descriptor)
}
fn discard_fixture(path: &std::path::Path) {
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn journal_exact_transfer_tail_seals_with_explicit_gap_and_original_samples() {
    use omniusong_engine::capture::CaptureJournal;
    use omniusong_recording::{ChunkKind, GapReason, SealedJournal};
    let (path, descriptor) = capture_fixture();
    let mut journal = CaptureJournal::create(&path, descriptor).unwrap();
    let (mut ingress, mut consumer, meters) = capture_queue(1, 48_000, 2).unwrap();
    ingress.ingest_f32(&[1.5; 3 * PACKET_FRAMES], 1_000_000);
    while let Ok(packet) = consumer.pop() {
        journal.append_packet(&packet).unwrap();
    }
    drop(ingress);
    let receipt = journal.finish(meters.snapshot(), true).unwrap().unwrap();
    assert_eq!((receipt.frames, receipt.gap_frames), (768, 256));
    let sealed = SealedJournal::open(&path).unwrap();
    let prefix = &sealed.manifest().body.streams[0];
    assert_eq!(
        (
            prefix.recorded_units,
            prefix.gap_units,
            prefix.source_end.value()
        ),
        (512, 256, 768)
    );
    assert_eq!(sealed.manifest().body.chunk_count, 2);
    let content = sealed.read_chunk("audio", 0).unwrap();
    assert_eq!(content.payload.len(), 512 * 4);
    assert!(
        content
            .payload
            .as_chunks::<4>()
            .0
            .iter()
            .all(|sample| f32::from_le_bytes(*sample) == 1.5)
    );
    let tail = sealed.read_chunk("audio", 1).unwrap();
    assert_eq!(tail.header.range.start(), 512);
    assert_eq!(tail.header.range.count(), 256);
    assert_eq!(
        tail.header.kind,
        ChunkKind::Gap {
            reason: GapReason::TransferOverflow
        }
    );
    assert!(tail.payload.is_empty());
    drop(sealed);
    discard_fixture(&path);
}

#[test]
fn undrained_accepted_tail_is_not_fabricated_as_a_transfer_gap() {
    use omniusong_engine::capture::CaptureJournal;
    use omniusong_recording::{Journal, SealedJournal};
    let (path, descriptor) = capture_fixture();
    let mut journal = CaptureJournal::create(&path, descriptor).unwrap();
    let (mut ingress, mut consumer, meters) = capture_queue(1, 48_000, 4).unwrap();
    ingress.ingest_f32(&[0.25; 512], 0);
    journal.append_packet(&consumer.pop().unwrap()).unwrap();
    drop(ingress);
    assert_eq!(meters.snapshot().dropped_frames, 0);
    assert!(
        journal
            .finish(meters.snapshot(), true)
            .unwrap_err()
            .contains("Undrained")
    );
    assert!(SealedJournal::open(&path).is_err());
    let interrupted = Journal::open(&path).unwrap();
    let report = interrupted.recovery_report().unwrap();
    assert!(report.unknown_tail);
    assert_eq!(report.streams[0].recorded_units, 256);
    assert_eq!(
        report.streams[0].gap_units, 0,
        "Never label an accepted undrained packet as physical transfer loss"
    );
    drop(interrupted);
    discard_fixture(&path);
}

#[test]
fn abort_unknown_fault_and_missing_lifetime_ack_all_remain_unsealed() {
    use omniusong_engine::capture::CaptureJournal;
    use omniusong_recording::{Journal, SealedJournal};
    for scenario in 0..4 {
        let (path, descriptor) = capture_fixture();
        let mut journal = CaptureJournal::create(&path, descriptor).unwrap();
        let (mut ingress, mut consumer, meters) = capture_queue(1, 48_000, 2).unwrap();
        ingress.ingest_f32(&[0.25; 17], if scenario == 3 { 10_000_000 } else { 0 });
        journal.append_packet(&consumer.pop().unwrap()).unwrap();
        if scenario == 1 {
            ingress.ingest_f32(&[], 1_000_000);
        }
        if scenario == 3 {
            ingress.ingest_f32(&[0.5; 17], 1_000_000);
            assert!(consumer.pop().is_err());
            assert_eq!(meters.snapshot().frames, 34);
            assert_eq!(meters.snapshot().dropped_frames, 17);
            assert_ne!(meters.snapshot().faults & INVALID_INPUT, 0);
        }
        let live = meters.snapshot();
        drop(ingress);
        let snapshot = if scenario == 2 {
            live
        } else {
            meters.snapshot()
        };
        let result = journal.finish(snapshot, scenario != 0);
        if scenario == 0 {
            assert!(result.unwrap().is_none());
        } else {
            assert!(result.is_err());
        }
        assert!(SealedJournal::open(&path).is_err());
        let interrupted = Journal::open(&path).unwrap();
        let report = interrupted.recovery_report().unwrap();
        assert!(report.unknown_tail);
        assert_eq!(report.streams[0].gap_units, 0);
        drop(interrupted);
        discard_fixture(&path);
    }
}

#[test]
fn journal_batches_small_callbacks_and_rejects_malformed_packets_without_mutating_pending_audio() {
    use omniusong_engine::capture::CaptureJournal;
    use omniusong_recording::SealedJournal;
    let (path, descriptor) = capture_fixture();
    let mut journal = CaptureJournal::create(&path, descriptor).unwrap();
    let (mut ingress, mut consumer, meters) = capture_queue(1, 48_000, 4).unwrap();
    ingress.ingest_f32(&[0.125; 256], 0);
    let packet = consumer.pop().unwrap();
    journal.append_packet(&packet).unwrap();
    let mut invalid = packet;
    invalid.start_frame = 256;
    invalid.samples[100] = f32::NAN;
    assert!(journal.append_packet(&invalid).is_err());
    ingress.ingest_f32(&[0.25; 256], 6_000_000);
    journal.append_packet(&consumer.pop().unwrap()).unwrap();
    drop(ingress);
    journal.finish(meters.snapshot(), true).unwrap().unwrap();
    let sealed = SealedJournal::open(&path).unwrap();
    assert_eq!(
        sealed.manifest().body.chunk_count,
        1,
        "Small callbacks aggregate outside the RT queue"
    );
    let content = sealed.read_chunk("audio", 0).unwrap();
    assert_eq!(content.payload.len(), 512 * 4);
    assert_eq!(
        f32::from_le_bytes(content.payload[256 * 4..257 * 4].try_into().unwrap()),
        0.25
    );
    drop(sealed);
    discard_fixture(&path);
}

fn wave(tag: u16, bits: u16, channels: u16, samples: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVEfmt ");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&tag.to_le_bytes());
    body.extend_from_slice(&channels.to_le_bytes());
    body.extend_from_slice(&48_000u32.to_le_bytes());
    let align = channels * (bits / 8);
    body.extend_from_slice(&(48_000u32 * align as u32).to_le_bytes());
    body.extend_from_slice(&align.to_le_bytes());
    body.extend_from_slice(&bits.to_le_bytes());
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    body.extend_from_slice(samples);
    if !samples.len().is_multiple_of(2) {
        body.push(0);
    }
    let mut bytes = Vec::from(b"RIFF".as_slice());
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}
#[test]
fn wav_decoding_preserves_integer_sign_float_amplitude_and_channel_order() {
    use omniusong_engine::render::{inspect_wav, wav_sample};
    let cases = [
        (1, 16, vec![0x00, 0x80, 0x00, 0x40], -1.0, 0.5),
        (1, 24, vec![0x00, 0x00, 0x80, 0x00, 0x00, 0x40], -1.0, 0.5),
        (
            1,
            32,
            vec![0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x40],
            -1.0,
            0.5,
        ),
        (
            3,
            32,
            [(-1.5f32).to_le_bytes(), 0.25f32.to_le_bytes()].concat(),
            -1.5,
            0.25,
        ),
    ];
    for (tag, bits, samples, left, right) in cases {
        let bytes = wave(tag, bits, 2, &samples);
        let info = inspect_wav(&bytes).unwrap();
        assert_eq!(
            (info.frames, info.channels, info.sample_rate),
            (1, 2, 48_000)
        );
        assert_eq!(wav_sample(&bytes, &info, 0, 0).unwrap(), left);
        assert_eq!(wav_sample(&bytes, &info, 0, 1).unwrap(), right);
        assert!(wav_sample(&bytes, &info, 1, 0).is_err());
        assert!(wav_sample(&bytes, &info, 0, 2).is_err());
    }
    let bytes = wave(1, 24, 1, &[0, 0, 0x40]);
    assert_eq!(
        inspect_wav(&bytes).unwrap().frames,
        1,
        "Odd RIFF chunks require padding, not a fabricated sample"
    );
}
#[test]
fn malformed_wav_and_forged_decode_extents_reject_without_panics() {
    use omniusong_engine::render::{WavInfo, inspect_wav, wav_sample};
    let valid = wave(1, 16, 1, &[0, 0]);
    let mut invalid = Vec::new();
    let mut a = valid.clone();
    a[4..8].copy_from_slice(&0u32.to_le_bytes());
    invalid.push(a);
    let mut a = valid.clone();
    a[20..22].copy_from_slice(&0xfffeu16.to_le_bytes());
    invalid.push(a);
    let mut a = valid.clone();
    a[28..32].copy_from_slice(&1u32.to_le_bytes());
    invalid.push(a);
    let mut a = valid.clone();
    a[32..34].copy_from_slice(&1u16.to_le_bytes());
    invalid.push(a);
    let mut a = valid.clone();
    a[40..44].copy_from_slice(&999u32.to_le_bytes());
    invalid.push(a);
    let mut a = wave(1, 24, 1, &[0, 0, 0]);
    a.pop();
    let size = (a.len() - 8) as u32;
    a[4..8].copy_from_slice(&size.to_le_bytes());
    invalid.push(a);
    for bytes in invalid {
        assert!(inspect_wav(&bytes).is_err());
    }
    let good = inspect_wav(&valid).unwrap();
    for info in [
        WavInfo {
            data_offset: usize::MAX,
            ..good
        },
        WavInfo {
            frames: u64::MAX,
            ..good
        },
        WavInfo { bits: 0, ..good },
        WavInfo {
            channels: 0,
            ..good
        },
        WavInfo {
            data_bytes: usize::MAX,
            ..good
        },
    ] {
        assert!(wav_sample(&valid, &info, 0, 0).is_err());
    }
    let bytes = wave(3, 32, 1, &f32::NAN.to_le_bytes());
    assert!(wav_sample(&bytes, &inspect_wav(&bytes).unwrap(), 0, 0).is_err());
}
#[test]
fn sealed_pcm_planar_and_interleaved_interpretations_are_distinct() {
    use omniusong_engine::render::pcm_sample;
    use omniusong_recording::{PcmEncoding, PcmFormat, PcmLayout};
    let bytes: Vec<_> = [0.1f32, 0.2, 0.3, 0.4]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let mut format = PcmFormat {
        sample_rate: 48_000,
        channels: 2,
        encoding: PcmEncoding::F32Le,
        layout: PcmLayout::Planar,
    };
    assert_eq!(pcm_sample(&bytes, &format, 2, 0, 1).unwrap(), 0.3);
    format.layout = PcmLayout::Interleaved;
    assert_eq!(pcm_sample(&bytes, &format, 2, 0, 1).unwrap(), 0.2);
    assert!(pcm_sample(&bytes[..15], &format, 2, 0, 0).is_err());
    assert!(pcm_sample(&bytes, &format, u64::MAX, 0, 0).is_err());
}
#[test]
fn malformed_output_callback_shape_silences_without_consuming_prepared_frames() {
    let (mut cursor, meters) = RenderCursor::new(vec![0.25, 0.5, 0.75, 1.0], 2).unwrap();
    let mut malformed = [9.0; 3];
    cursor.render_into(&mut malformed);
    assert_eq!(malformed, [0.0; 3]);
    assert_eq!(meters.snapshot().frames, 0);
    assert_ne!(meters.snapshot().faults, 0);
    let mut valid = [0.0; 4];
    cursor.render_into(&mut valid);
    assert_eq!(valid, [0.25, 0.5, 0.75, 1.0]);
}

#[test]
fn playback_preparation_uses_saved_timing_mute_gain_and_rejects_unsupported_or_oversize_material() {
    use omniusong_core::ProjectDocument;
    use omniusong_engine::render::prepare;
    use omniusong_store::NativeStore;
    use serde_json::json;
    let (path, _) = capture_fixture();
    let store = NativeStore::open(path.parent().unwrap().join("store")).unwrap();
    let mut value = ProjectDocument::new("Reviewer playback fixture")
        .unwrap()
        .value()
        .clone();
    value["tempo"] = json!(120);
    value["tracks"][0]["gain"] = json!(1.0);
    value["tracks"][0]["notes"] = json!([{ "id":"tone", "pitch":69, "startTick":960, "durationTicks":960, "velocity16":65535 }]);
    let document = ProjectDocument::from_value(value.clone()).unwrap();
    let pcm = prepare(&store, &document, 8_000, 2).unwrap();
    assert_eq!(pcm.len(), 8_000 * 2);
    assert!(pcm[..4_000 * 2].iter().all(|sample| *sample == 0.0));
    assert!(pcm[4_000 * 2..].iter().any(|sample| sample.abs() > 0.1));
    assert!(pcm.as_chunks::<2>().0.iter().all(|pair| pair[0] == pair[1]));
    let mut expressive = value.clone();
    expressive["tracks"][0]["notes"][0]["expression"] =
        json!([{ "type":"pressure", "points":[{ "tick":0,"value32":1234 }] }]);
    assert!(
        prepare(
            &store,
            &ProjectDocument::from_value(expressive.clone()).unwrap(),
            8_000,
            1
        )
        .unwrap_err()
        .contains("expression")
    );
    expressive["tracks"][0]["muted"] = json!(true);
    expressive["tracks"][1]["notes"] = value["tracks"][0]["notes"].clone();
    assert!(
        prepare(
            &store,
            &ProjectDocument::from_value(expressive).unwrap(),
            8_000,
            1
        )
        .is_ok()
    );
    let mut sampler = value.clone();
    sampler["tracks"][0]["assetId"] = json!(format!("{}.wav", "a".repeat(64)));
    assert!(
        prepare(
            &store,
            &ProjectDocument::from_value(sampler).unwrap(),
            8_000,
            1
        )
        .unwrap_err()
        .contains("sampler")
    );
    let mut long = value.clone();
    long["tracks"][0]["notes"][0]["startTick"] = json!(0);
    long["tracks"][0]["notes"][0]["durationTicks"] = json!(232_320);
    assert!(
        prepare(
            &store,
            &ProjectDocument::from_value(long.clone()).unwrap(),
            8_000,
            1
        )
        .unwrap_err()
        .contains("120 seconds")
    );
    long["tracks"][0]["notes"][0]["durationTicks"] = json!(230_400);
    assert!(
        prepare(
            &store,
            &ProjectDocument::from_value(long).unwrap(),
            192_000,
            2
        )
        .unwrap_err()
        .contains("64 MiB")
    );
    drop(store);
    discard_fixture(&path);
}

#[test]
fn sealed_clip_preparation_preserves_source_window_signed_placement_fade_and_original_file() {
    use omniusong_core::{
        ProjectDocument,
        native::{AudioFades, FadeCurve, TakeClip, TakeRange, TakeReference, TimelinePosition},
    };
    use omniusong_engine::{capture::CaptureJournal, render::prepare};
    use omniusong_recording::SealedJournal;
    use omniusong_store::NativeStore;
    let (path, descriptor) = capture_fixture();
    let mut journal = CaptureJournal::create(&path, descriptor).unwrap();
    let (mut ingress, mut consumer, meters) = capture_queue(1, 48_000, 1).unwrap();
    ingress.ingest_f32(&[0.0, 0.25, 0.5, 0.75, 1.0, 1.25], 0);
    journal.append_packet(&consumer.pop().unwrap()).unwrap();
    drop(ingress);
    journal.finish(meters.snapshot(), true).unwrap().unwrap();
    let original = std::fs::read(&path).unwrap();
    let sealed = SealedJournal::open(&path).unwrap();
    let mut store = NativeStore::open(path.parent().unwrap().join("store")).unwrap();
    store.ingest_take(&sealed).unwrap();
    let mut document = ProjectDocument::new("Native original fixture")
        .unwrap()
        .upgrade_native()
        .unwrap();
    let clip = TakeClip {
        edit_source: None,
        id: "native-clip".into(),
        source: TakeReference {
            take_id: sealed.descriptor().take_id.clone(),
            manifest_sha256: sealed.manifest().sha256.clone(),
            stream_id: "audio".into(),
        },
        range: TakeRange::AudioFrames { start: 1, count: 4 },
        position: TimelinePosition {
            frame: -1,
            sample_rate: 48_000,
        },
        fades: Some(AudioFades {
            in_frames: 0,
            out_frames: 2,
            curve: FadeCurve::Linear,
        }),
    };
    document
        .append_take_clip("native-track", "Native original", clip)
        .unwrap();
    let pcm = prepare(&store, &document, 48_000, 1).unwrap();
    // Source indexes 2/3/4 appear at arrangement 0/1/2. Fade uses source-relative indexes 1/2/3.
    assert_eq!(pcm, [0.5, 0.375, 0.0]);
    assert!(
        prepare(&store, &document, 44_100, 1)
            .unwrap_err()
            .contains("rate")
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let mut raw = document.value().clone();
    raw["tracks"][2]["recording"]["clips"][0]["range"] =
        serde_json::json!({ "unit":"events","start":0,"count":1 });
    raw["tracks"][2]["recording"]["clips"][0]
        .as_object_mut()
        .unwrap()
        .remove("fades");
    assert!(
        prepare(
            &store,
            &ProjectDocument::from_value(raw).unwrap(),
            48_000,
            1
        )
        .unwrap_err()
        .contains("raw event")
    );
    drop(sealed);
    drop(store);
    discard_fixture(&path);
}

#[test]
fn rendered_output_tail_waits_for_predicted_presentation_time_without_rewriting_endpoint() {
    use std::sync::atomic::Ordering;
    let (mut cursor, meters) = RenderCursor::new(vec![0.5; 48], 1).unwrap();
    let mut first = [0.0; 64];
    cursor.render_into_at(&mut first, 1_000_000, 4_000_000, 48_000);
    assert!(meters.render_finished.load(Ordering::Acquire));
    assert_eq!(meters.playback_end_ns.load(Ordering::Acquire), 5_000_000);
    assert!(
        meters.callback_clock_ns.load(Ordering::Acquire)
            < meters.playback_end_ns.load(Ordering::Acquire)
    );
    let mut silence = [9.0; 64];
    cursor.render_into_at(&mut silence, 5_100_000, 8_100_000, 48_000);
    assert_eq!(silence, [0.0; 64]);
    assert_eq!(meters.playback_end_ns.load(Ordering::Acquire), 5_000_000);
    assert!(
        meters.callback_clock_ns.load(Ordering::Acquire)
            >= meters.playback_end_ns.load(Ordering::Acquire)
    );
}
