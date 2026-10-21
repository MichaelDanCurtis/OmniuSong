//! Independent tests of the production live renderer's recording tap.
//! No device discovery, streams, journals, files, GUI or live catalog access.
use omniusong_engine::{
    live::{LiveErrorGuard, live_pair, recording_live_pair},
    performance_tap::{
        BatchDisposition, PerformancePacket, PerformancePacketKind, PerformanceTapOwner,
        TAP_CALLBACK_PACKETS, TAP_CHANNELS, TAP_PCM_FRAMES, TAP_SLOTS, TapClearReason, TapEnd,
        TapFaultKind, TapTransitionKind,
    },
    *,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct CallbackAllocator;
thread_local! {
    static CALLBACK_ALLOCATIONS: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}
fn count(allocations: usize, deallocations: usize) {
    let _ = CALLBACK_ALLOCATIONS.try_with(|state| {
        let (enabled, a, d) = state.get();
        if enabled {
            state.set((true, a + allocations, d + deallocations));
        }
    });
}
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count(0, 1);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(1, 1);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

fn callback(work: impl FnOnce()) -> (usize, usize) {
    CALLBACK_ALLOCATIONS.with(|state| state.set((true, 0, 0)));
    work();
    CALLBACK_ALLOCATIONS.with(|state| {
        let (_, allocations, deallocations) = state.replace((false, 0, 0));
        (allocations, deallocations)
    })
}
fn scope() -> LiveScope {
    LiveScope {
        worker_generation: LiveId([73; 16]),
        session_id: LiveId([29; 16]),
        panic_generation: 1,
    }
}
fn on(source: u8, voice: u64, ticks: u64, patch: LivePatch, velocity: u16) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: ticks,
        event: LiveEventKind::NoteOn {
            note: voice,
            voice,
            key: 60,
            velocity16: velocity,
            patch,
        },
    }
}
fn event(source: u8, ticks: u64, kind: LiveEventKind) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: ticks,
        event: kind,
    }
}
fn batch(scope: LiveScope, sequence: u64, events: &[LiveEvent]) -> LiveBatch {
    let mut batch = LiveBatch::empty(scope, sequence, None);
    for event in events {
        batch.try_push(*event).unwrap();
    }
    batch
}
fn drain(owner: &mut PerformanceTapOwner) -> Vec<PerformancePacket> {
    let mut packets = Vec::new();
    while let Some(packet) = owner.try_pop().unwrap() {
        packets.push(packet);
    }
    packets
}
fn groups(packets: Vec<PerformancePacket>) -> Vec<Vec<PerformancePacket>> {
    let mut groups: Vec<Vec<PerformancePacket>> = Vec::new();
    for packet in packets {
        if matches!(packet.kind, PerformancePacketKind::Begin(_)) {
            groups.push(Vec::new());
        }
        groups
            .last_mut()
            .expect("Every group begins with Begin")
            .push(packet);
    }
    groups
}
fn check_group(
    packets: &[PerformancePacket],
    callback_index: u64,
    start: u64,
    frames: usize,
    channels: usize,
) -> (Vec<f32>, Vec<(LiveBatch, BatchDisposition)>, TapEnd) {
    assert!(!packets.is_empty());
    assert!(packets.len() <= TAP_CALLBACK_PACKETS);
    let PerformancePacketKind::Begin(begin) = packets[0].kind else {
        panic!("Callback must start with Begin");
    };
    assert_eq!(begin.start_frame, start);
    assert_eq!(usize::from(begin.frames), frames);
    assert_eq!(usize::from(begin.channels), channels);
    let mut pcm = Vec::new();
    let mut batches = Vec::new();
    let mut pcm_started = false;
    let mut expected_frame = start;
    for (ordinal, packet) in packets.iter().enumerate() {
        assert_eq!(packet.callback_index, callback_index);
        assert_eq!(usize::from(packet.ordinal), ordinal);
        match &packet.kind {
            PerformancePacketKind::Begin(_) => assert_eq!(ordinal, 0),
            PerformancePacketKind::Batch { batch, disposition } => {
                assert!(!pcm_started, "Batches precede PCM");
                batches.push((*batch, *disposition));
            }
            PerformancePacketKind::Pcm(value) => {
                pcm_started = true;
                assert_eq!(value.start_frame, expected_frame);
                assert_eq!(usize::from(value.channels), channels);
                assert!((1..=TAP_PCM_FRAMES).contains(&usize::from(value.frames)));
                let samples = usize::from(value.frames) * channels;
                assert!(
                    value.samples[..samples]
                        .iter()
                        .all(|sample| sample.is_finite())
                );
                assert!(
                    value.samples[samples..]
                        .iter()
                        .all(|sample| sample.to_bits() == 0)
                );
                pcm.extend_from_slice(&value.samples[..samples]);
                expected_frame += u64::from(value.frames);
            }
            PerformancePacketKind::End(_) => assert_eq!(ordinal, packets.len() - 1),
        }
    }
    assert!(batches.len() <= LIVE_CALLBACK_PACKET_BUDGET);
    assert_eq!(expected_frame, start + frames as u64);
    assert_eq!(pcm.len(), frames * channels);
    let PerformancePacketKind::End(end) = packets.last().unwrap().kind else {
        panic!("Callback must finish with End");
    };
    assert_eq!(end.end_frame, expected_frame);
    assert!(end.release_tail_voices <= end.active_voices);
    assert!(end.active_voices <= LIVE_VOICES as u16);
    assert!(usize::from(end.transition_len) <= end.transitions.len());
    assert!(
        end.transitions[..usize::from(end.transition_len)]
            .iter()
            .all(Option::is_some)
    );
    assert!(
        end.transitions[usize::from(end.transition_len)..]
            .iter()
            .all(Option::is_none)
    );
    (pcm, batches, end)
}
fn assert_pcm_exact(left: &[f32], right: &[f32]) {
    assert_eq!(left.len(), right.len());
    for (index, (left, right)) in left.iter().zip(right).enumerate() {
        assert_eq!(left.to_bits(), right.to_bits(), "PCM sample {index}");
    }
}

#[test]
fn admitted_is_not_applied_and_original_clock_values_are_not_inferred() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 48_000, 2).unwrap();
    let original = batch(
        scope(),
        1,
        &[
            on(1, 1, u64::MAX, LivePatch::Keys, 0),
            on(2, 1, 7, LivePatch::Bass, u16::MAX),
            on(3, 1, 19, LivePatch::Pad, 32_769),
            on(4, 1, 0, LivePatch::Drums, 1),
        ],
    );
    admission.admit(original).unwrap();
    assert_eq!(admission.progress().worker_accepted_sequence, Some(1));
    assert_eq!(admission.progress().callback_applied_sequence, None);
    assert!(owner.try_pop().unwrap().is_none());
    assert_eq!(owner.control().snapshot().unwrap().completed_frames, 0);
    assert_eq!(owner.info().scope, scope());
    let mut output = [0.0; 64 * 2];
    assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
    let packets = drain(&mut owner);
    let (pcm, records, end) = check_group(&packets, 1, 0, 64, 2);
    assert_eq!(records, vec![(original, BatchDisposition::Applied)]);
    assert_pcm_exact(&pcm, &output);
    assert_eq!(end.applied_sequence, Some(1));
    assert_eq!(
        end.active_voices, 4,
        "Zero velocity is not an implicit MIDI1 NoteOff"
    );
    assert_eq!(owner.control().snapshot().unwrap().completed_frames, 64);
}

#[test]
fn recorded_f32_is_bit_exact_production_bus_for_variable_callbacks_and_all_patches() {
    for channels in [1, 2, TAP_CHANNELS as u16] {
        let (mut plain_admission, mut plain) = live_pair(scope(), 48_000, channels).unwrap();
        let (mut admission, mut renderer, mut owner) =
            recording_live_pair(scope(), 48_000, channels).unwrap();
        let original = batch(
            scope(),
            1,
            &[
                on(1, 1, 2, LivePatch::Keys, 65_534),
                on(2, 1, 2, LivePatch::Bass, 41_001),
                on(3, 1, 2, LivePatch::Pad, 32_767),
                on(4, 1, 2, LivePatch::Drums, 22_002),
            ],
        );
        plain_admission.admit(original).unwrap();
        admission.admit(original).unwrap();
        let mut start = 0;
        for (index, frames) in [1, 256, 2047, 2048, 2049, 16_384].into_iter().enumerate() {
            let mut expected = vec![0.0; frames * usize::from(channels)];
            let mut output = vec![0.0; expected.len()];
            assert_eq!(callback(|| plain.render_into(&mut expected)), (0, 0));
            assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
            let packets = drain(&mut owner);
            let (pcm, records, _) = check_group(
                &packets,
                index as u64 + 1,
                start,
                frames,
                usize::from(channels),
            );
            assert_pcm_exact(&output, &expected);
            assert_pcm_exact(&pcm, &expected);
            assert_eq!(records.len(), usize::from(index == 0));
            start += frames as u64;
        }
        assert!(admission.progress().fault.is_none());
    }
}

#[test]
fn integer_output_conversion_does_not_change_original_f32_recording() {
    let (mut a, mut f32_renderer, mut f32_owner) = recording_live_pair(scope(), 48_000, 2).unwrap();
    let (mut b, mut i16_renderer, mut i16_owner) = recording_live_pair(scope(), 48_000, 2).unwrap();
    let (mut c, mut u16_renderer, mut u16_owner) = recording_live_pair(scope(), 48_000, 2).unwrap();
    let original = batch(scope(), 1, &[on(1, 1, 23, LivePatch::Bass, 65_535)]);
    a.admit(original).unwrap();
    b.admit(original).unwrap();
    c.admit(original).unwrap();
    let mut f32_output = [0.0; 1024];
    let mut i16_output = [0_i16; 1024];
    let mut u16_output = [0_u16; 1024];
    assert_eq!(
        callback(|| f32_renderer.render_into(&mut f32_output)),
        (0, 0)
    );
    assert_eq!(
        callback(|| i16_renderer.render_i16(&mut i16_output)),
        (0, 0)
    );
    assert_eq!(
        callback(|| u16_renderer.render_u16(&mut u16_output)),
        (0, 0)
    );
    let f32_packets = drain(&mut f32_owner);
    let i16_packets = drain(&mut i16_owner);
    let u16_packets = drain(&mut u16_owner);
    let (pcm, _, _) = check_group(&f32_packets, 1, 0, 512, 2);
    assert_pcm_exact(&pcm, &f32_output);
    assert_pcm_exact(&check_group(&i16_packets, 1, 0, 512, 2).0, &pcm);
    assert_pcm_exact(&check_group(&u16_packets, 1, 0, 512, 2).0, &pcm);
    for ((value, signed), unsigned) in pcm.iter().zip(i16_output).zip(u16_output) {
        assert_eq!(signed, (value * 32767.0).round() as i16);
        assert_eq!(unsigned, ((value * 32767.0) + 32768.0).round() as u16);
    }
}

#[test]
fn callback_rejection_preserves_whole_original_and_no_partial_note_sounds() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
    let original = batch(
        scope(),
        1,
        &[
            on(1, 1, 1, LivePatch::Keys, 55_555),
            event(
                1,
                2,
                LiveEventKind::NoteOff {
                    voice: 999,
                    release_velocity16: 65_534,
                },
            ),
        ],
    );
    admission.admit(original).unwrap();
    let mut output = [1.0; 257];
    assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
    assert!(output.iter().all(|sample| *sample == 0.0));
    let packets = drain(&mut owner);
    let (_, records, end) = check_group(&packets, 1, 0, 257, 1);
    assert_eq!(
        records,
        vec![(
            original,
            BatchDisposition::Rejected {
                fault: LiveFaultKind::SourceOwnership
            }
        )]
    );
    assert_eq!(end.applied_sequence, None);
    assert_eq!(end.active_voices, 0);
    assert_eq!(
        end.first_fault.unwrap().kind,
        LiveFaultKind::SourceOwnership
    );
    assert!(end.clears.iter().flatten().any(|clear| clear.frame == 0
        && clear.reason
            == TapClearReason::Fault {
                kind: LiveFaultKind::SourceOwnership
            }));
    assert_eq!(
        end.transition_len, 0,
        "Rejected NoteOn cannot create an automatic lifecycle"
    );
}

#[test]
fn four_total_ingress_packets_bound_every_callback_including_empty_batches() {
    for frames in [1, 2047, 2048, 2049, 16_384] {
        let (mut admission, mut renderer, mut owner) =
            recording_live_pair(scope(), 768_000, 8).unwrap();
        for sequence in 1..=16 {
            admission
                .admit(LiveBatch::empty(scope(), sequence, None))
                .unwrap();
        }
        let mut output = vec![0.0; frames * 8];
        assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
        let packets = drain(&mut owner);
        let (_, records, end) = check_group(&packets, 1, 0, frames, 8);
        assert_eq!(records.len(), 4);
        assert_eq!(
            records.iter().map(|(b, _)| b.sequence).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(
            records
                .iter()
                .all(|(b, d)| b.len == 0 && *d == BatchDisposition::Applied)
        );
        assert_eq!(end.applied_sequence, Some(4));
        assert_eq!(admission.progress().worker_accepted_sequence, Some(16));
        if frames == 16_384 {
            assert_eq!(packets.len(), TAP_CALLBACK_PACKETS);
        }
    }
}

#[test]
fn ring_overflow_faults_before_new_batch_is_applied_and_retained_prefix_drains() {
    let (mut admission, mut renderer, mut owner) =
        recording_live_pair(scope(), 768_000, 1).unwrap();
    let mut output = vec![0.0; 16_384];
    for _ in 0..3 {
        assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
    }
    let original = batch(scope(), 1, &[on(1, 1, 9, LivePatch::Keys, 65_535)]);
    admission.admit(original).unwrap();
    output.fill(1.0);
    assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
    assert!(output.iter().all(|sample| *sample == 0.0));
    let snapshot = owner.control().snapshot().unwrap();
    assert_eq!(snapshot.completed_callback_index, 3);
    assert_eq!(snapshot.completed_frames, 49_152);
    assert_eq!(snapshot.fault.unwrap().kind, TapFaultKind::Overflow);
    assert_eq!(snapshot.fault.unwrap().frame, 49_152);
    assert_eq!(admission.progress().worker_accepted_sequence, Some(1));
    assert_eq!(admission.progress().callback_applied_sequence, None);
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::RecordingTap
    );
    let retained = groups(drain(&mut owner));
    assert_eq!(
        retained.len(),
        3,
        "Failed reservation emits no partial Begin"
    );
    for (index, packets) in retained.iter().enumerate() {
        let (_, records, _) =
            check_group(packets, index as u64 + 1, index as u64 * 16_384, 16_384, 1);
        assert!(records.is_empty());
    }
    renderer.render_into(&mut output);
    assert!(
        owner.try_pop().unwrap().is_none(),
        "Drain does not restart a faulted recording"
    );
    assert_eq!(admission.progress().callback_applied_sequence, None);
}

#[test]
fn healthy_panic_retains_discarded_originals_and_requires_four_bounded_callbacks() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
    admission
        .admit(batch(scope(), 1, &[on(1, 1, 100, LivePatch::Keys, 60_000)]))
        .unwrap();
    renderer.render_into(&mut [0.0; 1]);
    drain(&mut owner);
    let mut queued = Vec::new();
    for voice in 2..=17 {
        let original = batch(
            scope(),
            voice,
            &[on(1, voice, 99 + voice, LivePatch::Keys, voice as u16)],
        );
        admission.admit(original).unwrap();
        queued.push(original);
    }
    let next_scope = admission.begin_panic(scope()).unwrap();
    let mut discarded = Vec::new();
    for index in 0..4 {
        let mut output = [1.0; 1];
        assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
        assert_eq!(output, [0.0]);
        let packets = drain(&mut owner);
        let (_, records, end) = check_group(&packets, index + 2, index + 1, 1, 1);
        assert_eq!(records.len(), 4);
        assert!(
            records
                .iter()
                .all(|(_, disposition)| *disposition == BatchDisposition::DiscardedPanic)
        );
        discarded.extend(records.into_iter().map(|(batch, _)| batch));
        assert!(end.clears.iter().flatten().any(|clear| clear.reason
            == TapClearReason::Panic {
                next_generation: next_scope.panic_generation
            }));
        if index < 3 {
            assert!(admission.finish_panic().unwrap().is_none());
        }
    }
    assert_eq!(discarded, queued);
    assert_eq!(admission.finish_panic().unwrap().unwrap().scope, next_scope);
    let new = batch(next_scope, 1, &[on(1, 1, 117, LivePatch::Bass, 10_001)]);
    admission.admit(new).unwrap();
    renderer.render_into(&mut [0.0; 64]);
    let packets = drain(&mut owner);
    let (_, records, end) = check_group(&packets, 6, 5, 64, 1);
    assert_eq!(records, vec![(new, BatchDisposition::Applied)]);
    assert_eq!(end.scope_after, next_scope);
    assert_eq!(end.active_voices, 1);
}

#[test]
fn stale_admission_is_inert_and_does_not_invent_callback_receipt() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
    let mut foreign = batch(scope(), 1, &[on(1, 1, 1, LivePatch::Keys, 10)]);
    foreign.scope.session_id = LiveId([91; 16]);
    assert_eq!(admission.admit(foreign), Err(LiveSubmitError::StaleScope));
    assert!(owner.try_pop().unwrap().is_none());
    assert!(admission.progress().fault.is_none());
    let actual = batch(scope(), 1, &[on(1, 1, 2, LivePatch::Keys, 10)]);
    admission.admit(actual).unwrap();
    renderer.render_into(&mut [0.0; 8]);
    let packets = drain(&mut owner);
    assert_eq!(
        check_group(&packets, 1, 0, 8, 1).1,
        vec![(actual, BatchDisposition::Applied)]
    );
}

#[test]
fn lease_clear_is_at_exact_frame_and_is_not_forged_human_note_off() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 8000, 1).unwrap();
    let original = batch(scope(), 1, &[on(1, 1, 999_999, LivePatch::Keys, 65_535)]);
    admission.admit(original).unwrap();
    let mut output = vec![0.0; 8001];
    assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
    let packets = drain(&mut owner);
    let (pcm, originals, end) = check_group(&packets, 1, 0, 8001, 1);
    assert_eq!(originals, vec![(original, BatchDisposition::Applied)]);
    assert_pcm_exact(&pcm, &output);
    assert!(pcm[..8000].iter().any(|sample| *sample != 0.0));
    assert_eq!(pcm[8000], 0.0);
    let fault = end.first_fault.unwrap();
    assert_eq!(
        (fault.kind, fault.frame),
        (LiveFaultKind::LeaseExpired, 8000)
    );
    let clears = end.clears.iter().flatten().collect::<Vec<_>>();
    assert_eq!(clears.len(), 1);
    assert_eq!(
        (clears[0].frame, clears[0].reason),
        (
            8000,
            TapClearReason::Fault {
                kind: LiveFaultKind::LeaseExpired
            }
        )
    );
    assert_eq!(end.transition_len, 0);
    admission.control().fault(LiveFaultKind::Device, None);
    assert_eq!(admission.progress().fault.unwrap(), fault);
    renderer.render_into(&mut [1.0; 1]);
    let next = drain(&mut owner);
    let (_, originals, next_end) = check_group(&next, 2, 8001, 1, 1);
    assert!(originals.is_empty());
    assert!(
        next_end.clears.iter().all(Option::is_none),
        "Existing fault is not a new clear"
    );
}

#[test]
fn pulse_and_sustain_keep_exact_release_originals_separate_from_engine_transitions() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 8000, 1).unwrap();
    let original = batch(
        scope(),
        1,
        &[
            on(1, 1, 1, LivePatch::Keys, 60_001),
            event(1, 2, LiveEventKind::Sustain { down: true }),
            event(
                2,
                1,
                LiveEventKind::Pulse {
                    note: 1,
                    voice: 1,
                    key: 60,
                    velocity16: 40_003,
                    release_velocity16: 65_534,
                    patch: LivePatch::Keys,
                    duration_frames: 16,
                },
            ),
        ],
    );
    admission.admit(original).unwrap();
    renderer.render_into(&mut [0.0; 16]);
    let first = drain(&mut owner);
    assert_eq!(check_group(&first, 1, 0, 16, 1).2.transition_len, 0);
    let release = batch(
        scope(),
        2,
        &[event(
            1,
            3,
            LiveEventKind::NoteOff {
                voice: 1,
                release_velocity16: 43_210,
            },
        )],
    );
    admission.admit(release).unwrap();
    renderer.render_into(&mut [0.0; 1]);
    let second = drain(&mut owner);
    let (_, records, end) = check_group(&second, 2, 16, 1, 1);
    assert_eq!(records, vec![(release, BatchDisposition::Applied)]);
    assert_eq!(end.active_voices, 2);
    assert_eq!(
        end.release_tail_voices, 1,
        "Only Pulse releases; other source is sustained"
    );
    let transition = end.transitions[0].unwrap();
    assert_eq!(
        (transition.source, transition.voice, transition.frame),
        (2, 1, 16)
    );
    assert_eq!(
        transition.kind,
        TapTransitionKind::AutoRelease {
            release_velocity16: 65_534
        }
    );
    let other_sustain = batch(
        scope(),
        3,
        &[event(2, 2, LiveEventKind::Sustain { down: false })],
    );
    admission.admit(other_sustain).unwrap();
    renderer.render_into(&mut [0.0; 1]);
    let third = drain(&mut owner);
    assert_eq!(check_group(&third, 3, 17, 1, 1).2.release_tail_voices, 1);
    let own_sustain = batch(
        scope(),
        4,
        &[event(1, 4, LiveEventKind::Sustain { down: false })],
    );
    admission.admit(own_sustain).unwrap();
    renderer.render_into(&mut [0.0; 2000]);
    let fourth = drain(&mut owner);
    let (_, records, end) = check_group(&fourth, 4, 18, 2000, 1);
    assert_eq!(records, vec![(own_sustain, BatchDisposition::Applied)]);
    assert_eq!(end.active_voices, 0);
    assert_eq!(
        end.transitions
            .iter()
            .flatten()
            .filter(|t| t.kind == TapTransitionKind::VoiceEnded)
            .count(),
        2
    );
    assert!(
        !end.transitions
            .iter()
            .flatten()
            .any(|t| t.source == 1 && matches!(t.kind, TapTransitionKind::AutoRelease { .. })),
        "Human sustained release remains its actual source event"
    );
}

#[test]
fn all_sixty_four_voices_can_emit_both_automatic_transitions_in_one_callback() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 8000, 1).unwrap();
    for sequence in 1..=2 {
        let mut original = LiveBatch::empty(scope(), sequence, None);
        for offset in 1..=32 {
            let voice = (sequence - 1) * 32 + offset;
            original
                .try_push(event(
                    1,
                    voice,
                    LiveEventKind::Pulse {
                        note: voice,
                        voice,
                        key: 60,
                        velocity16: 65_535,
                        release_velocity16: 65_535,
                        patch: LivePatch::Keys,
                        duration_frames: 1,
                    },
                ))
                .unwrap();
        }
        admission.admit(original).unwrap();
    }
    let mut output = [0.0; 512];
    assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
    let packets = drain(&mut owner);
    let (_, originals, end) = check_group(&packets, 1, 0, 512, 1);
    assert_eq!(originals.len(), 2);
    assert_eq!(end.transition_len, 128);
    assert_eq!(end.active_voices, 0);
    assert!(end.first_fault.is_none());
    assert!(owner.control().snapshot().unwrap().fault.is_none());
    for voice in 1..=64 {
        let transitions = end
            .transitions
            .iter()
            .flatten()
            .filter(|t| t.voice == voice)
            .collect::<Vec<_>>();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].frame, 1);
        assert_eq!(
            transitions[0].kind,
            TapTransitionKind::AutoRelease {
                release_velocity16: 65_535
            }
        );
        assert_eq!(transitions[1].kind, TapTransitionKind::VoiceEnded);
        assert!(transitions[1].frame > transitions[0].frame);
    }
}

#[test]
fn receiver_abandonment_cannot_free_ring_or_sound_new_note_on_callback() {
    let (mut admission, mut renderer, owner) = recording_live_pair(scope(), 48_000, 2).unwrap();
    let control = owner.control();
    let live_control = admission.control();
    let error_guard = LiveErrorGuard::new(live_control.clone());
    admission
        .admit(batch(scope(), 1, &[on(1, 1, 1, LivePatch::Keys, 65_535)]))
        .unwrap();
    drop(owner);
    assert_eq!(
        control.snapshot().unwrap().fault.unwrap().kind,
        TapFaultKind::ReceiverAbandoned
    );
    let mut output = [1.0; 512];
    assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
    assert!(output.iter().all(|sample| *sample == 0.0));
    assert_eq!(admission.progress().callback_applied_sequence, None);
    assert_eq!(
        callback(|| drop(renderer)),
        (0, 0),
        "Admission retains the ring through producer destruction"
    );
    assert!(control.snapshot().unwrap().producer_destroyed);
    assert!(live_control.render_destroyed());
    assert!(!live_control.error_destroyed());
    assert_eq!(callback(|| drop(error_guard)), (0, 0));
    assert!(live_control.error_destroyed());
    drop(admission);
}

#[test]
fn explicit_tap_cancel_retains_original_prefix_and_cause_until_teardown() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
    let original = batch(scope(), 1, &[on(1, 1, 1, LivePatch::Pad, 32_768)]);
    admission.admit(original).unwrap();
    renderer.render_into(&mut [0.0; 64]);
    let control = owner.control();
    control.cancel();
    let first_fault = control.snapshot().unwrap().fault.unwrap();
    assert_eq!(first_fault.kind, TapFaultKind::Aborted);
    let mut output = [1.0; 64];
    assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
    assert!(output.iter().all(|v| *v == 0.0));
    let packets = drain(&mut owner);
    assert_eq!(
        check_group(&packets, 1, 0, 64, 1).1,
        vec![(original, BatchDisposition::Applied)]
    );
    assert_eq!(callback(|| drop(renderer)), (0, 0));
    assert!(control.snapshot().unwrap().producer_destroyed);
    drop(owner);
    assert_eq!(control.snapshot().unwrap().fault, Some(first_fault));
}

#[test]
fn malformed_callback_silences_and_cannot_claim_complete_recorded_frames() {
    for samples in [0, 3, (16_384 + 1) * 2] {
        let (mut admission, mut renderer, mut owner) =
            recording_live_pair(scope(), 48_000, 2).unwrap();
        admission
            .admit(batch(scope(), 1, &[on(1, 1, 1, LivePatch::Keys, 1)]))
            .unwrap();
        let mut output = vec![1.0; samples];
        assert_eq!(callback(|| renderer.render_into(&mut output)), (0, 0));
        assert!(output.iter().all(|sample| *sample == 0.0));
        assert_eq!(admission.progress().callback_applied_sequence, None);
        assert_eq!(
            admission.progress().fault.unwrap().kind,
            LiveFaultKind::CallbackShape
        );
        assert!(owner.try_pop().unwrap().is_none());
        assert_eq!(owner.control().snapshot().unwrap().completed_frames, 0);
    }
}

#[test]
fn capture_packets_have_fixed_storage_and_no_drop_owners() {
    assert!(!std::mem::needs_drop::<PerformancePacket>());
    assert!(std::mem::size_of::<PerformancePacket>() <= 16 * 1024);
    assert!(std::mem::size_of::<PerformancePacket>() * TAP_SLOTS <= 4 * 1024 * 1024);
    assert_eq!(TAP_CALLBACK_PACKETS, 2 + 4 + 16_384 / TAP_PCM_FRAMES);
}
