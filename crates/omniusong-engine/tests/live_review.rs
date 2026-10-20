//! Independent live audition tests. No device enumeration, stream or live catalog access.
use omniusong_engine::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct CountingAllocator;
thread_local! {
    static RT_ALLOCATIONS: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}
fn count(allocations: usize, deallocations: usize) {
    let _ = RT_ALLOCATIONS.try_with(|cell| {
        let (enabled, a, d) = cell.get();
        if enabled {
            cell.set((true, a + allocations, d + deallocations));
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
    RT_ALLOCATIONS.with(|cell| cell.set((true, 0, 0)));
    work();
    RT_ALLOCATIONS.with(|cell| {
        let (_, a, d) = cell.replace((false, 0, 0));
        (a, d)
    })
}
fn scope() -> LiveScope {
    LiveScope {
        worker_generation: LiveId([1; 16]),
        session_id: LiveId([2; 16]),
        panic_generation: 1,
    }
}
fn event(source: u8, voice: u64) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: voice,
        event: LiveEventKind::NoteOn {
            note: voice,
            voice,
            key: 60,
            velocity16: u16::MAX,
            patch: LivePatch::Keys,
        },
    }
}
fn progress() -> LiveProgress {
    LiveProgress {
        scope: scope(),
        state: LiveState::Running,
        worker_accepted_sequence: None,
        callback_applied_sequence: None,
        active_voices: 0,
        release_tail_voices: 0,
        late_releases: 0,
        rejected_batches: 0,
        overflow_batches: 0,
        frames: 0,
        peak: 0.0,
        lease_deadline_frame: 48_000,
        lease_credit: Some(LeaseCredit {
            scope: scope(),
            token: LiveId([3; 16]),
            issued_after_sequence: None,
        }),
        fault: None,
    }
}
fn fault() -> LiveFault {
    LiveFault {
        scope: scope(),
        kind: LiveFaultKind::LeaseExpired,
        frame: 48_000,
        sequence: None,
    }
}

#[test]
fn fixed_packet_rejects_hidden_slots_holes_invalid_scope_and_bounds_without_allocating() {
    assert!(!std::mem::needs_drop::<LiveBatch>());
    for len in 0..=LIVE_BATCH_EVENTS {
        let mut batch = LiveBatch::empty(scope(), 1, Some(LiveId([3; 16])));
        for index in 0..len {
            batch.try_push(event(1, index as u64 + 1)).unwrap();
        }
        let mut result = Err(LiveSubmitError::Invalid);
        assert_eq!(hot_path(|| result = batch.validate_shape(48_000)), (0, 0));
        assert_eq!(result, Ok(()));
        if len < LIVE_BATCH_EVENTS {
            let mut hidden = batch;
            hidden.events[len] = Some(event(1, 99));
            assert_eq!(hidden.validate_shape(48_000), Err(LiveSubmitError::Invalid));
        }
        if len > 0 {
            let mut hole = batch;
            hole.events[len - 1] = None;
            assert_eq!(hole.validate_shape(48_000), Err(LiveSubmitError::Invalid));
        }
    }
    let base = LiveBatch::empty(scope(), 1, None);
    for index in 0..7 {
        let mut invalid = base;
        match index {
            0 => invalid.len = 33,
            1 => invalid.scope.worker_generation = LiveId([0; 16]),
            2 => invalid.scope.session_id = LiveId([0; 16]),
            3 => invalid.scope.panic_generation = 0,
            4 => invalid.sequence = 0,
            5 => invalid.lease_token = Some(LiveId([0; 16])),
            _ => {
                invalid.try_push(event(17, 1)).unwrap();
            }
        }
        assert_eq!(
            invalid.validate_shape(48_000),
            Err(LiveSubmitError::Invalid)
        );
    }
}

#[test]
fn pulse_and_release_values_roundtrip_without_midi_one_zero_velocity_reinterpretation() {
    let mut batch = LiveBatch::empty(scope(), u64::MAX, None);
    let mut zero = event(1, 1);
    zero.event = LiveEventKind::NoteOn {
        note: 1,
        voice: 1,
        key: 0,
        velocity16: 0,
        patch: LivePatch::Pad,
    };
    batch.try_push(zero).unwrap();
    let mut pulse = event(4, 1);
    pulse.event = LiveEventKind::Pulse {
        note: 1,
        voice: 1,
        key: 127,
        velocity16: u16::MAX,
        release_velocity16: u16::MAX,
        patch: LivePatch::Drums,
        duration_frames: 24_000,
    };
    batch.try_push(pulse).unwrap();
    let mut release = event(1, 1);
    release.event = LiveEventKind::NoteOff {
        voice: 1,
        release_velocity16: 12_345,
    };
    batch.try_push(release).unwrap();
    assert_eq!(batch.validate_shape(48_000), Ok(()));
    let encoded = serde_json::to_vec(&batch).unwrap();
    assert_eq!(
        serde_json::from_slice::<LiveBatch>(&encoded).unwrap(),
        batch
    );
    for duration_frames in [0, 24_001, u32::MAX] {
        let mut invalid = batch;
        if let LiveEventKind::Pulse {
            duration_frames: duration,
            ..
        } = &mut invalid.events[1].as_mut().unwrap().event
        {
            *duration = duration_frames;
        }
        assert_eq!(
            invalid.validate_shape(48_000),
            Err(LiveSubmitError::Invalid)
        );
    }
    let mut json = serde_json::to_value(batch).unwrap();
    json["unrecognized"] = true.into();
    assert!(serde_json::from_value::<LiveBatch>(json).is_err());
}

#[test]
fn progress_rejects_healthy_fault_or_expired_credit_and_impossible_watermarks() {
    assert!(progress().is_valid());
    for index in 0..9 {
        let mut invalid = progress();
        match index {
            0 => invalid.fault = Some(fault()),
            1 => invalid.lease_deadline_frame = 0,
            2 => invalid.lease_credit = None,
            3 => invalid.callback_applied_sequence = Some(1),
            4 => invalid.worker_accepted_sequence = Some(0),
            5 => invalid.lease_credit.as_mut().unwrap().issued_after_sequence = Some(1),
            6 => {
                invalid
                    .lease_credit
                    .as_mut()
                    .unwrap()
                    .scope
                    .panic_generation = 2
            }
            7 => invalid.active_voices = 65,
            _ => invalid.release_tail_voices = 1,
        }
        assert!(!invalid.is_valid(), "Malformed progress case {index}");
    }
    let mut panicking = progress();
    panicking.state = LiveState::Panicking;
    assert!(!panicking.is_valid());
    panicking.lease_credit = None;
    assert!(panicking.is_valid());
    panicking.fault = Some(fault());
    assert!(!panicking.is_valid());
    let mut ended = progress();
    ended.state = LiveState::Faulted;
    ended.fault = Some(fault());
    ended.lease_credit = None;
    assert!(ended.is_valid());
    ended.state = LiveState::Stopped;
    assert!(ended.is_valid());
    ended.active_voices = 1;
    assert!(!ended.is_valid());
}

#[test]
fn urgent_acknowledgement_cannot_cross_session_or_claim_a_later_uncompleted_stop() {
    let ticket = UrgentTicket {
        worker_generation: scope().worker_generation,
        id: 2,
    };
    let next = LiveScope {
        panic_generation: 2,
        ..scope()
    };
    let mut update = EngineUpdate {
        worker_generation: scope().worker_generation,
        revision: 1,
        live: None,
        urgent: UrgentProgress {
            stop: None,
            panic: Some(PanicAck {
                ticket,
                requested_scope: scope(),
                outcome: PanicOutcome::Applied { new_scope: next },
            }),
        },
        first_fault: None,
    };
    assert!(update.is_valid());
    update.urgent.panic.as_mut().unwrap().outcome = PanicOutcome::Applied {
        new_scope: LiveScope {
            session_id: LiveId([7; 16]),
            ..next
        },
    };
    assert!(!update.is_valid());
    let stop = UrgentTicket { id: 3, ..ticket };
    update.urgent.panic.as_mut().unwrap().outcome =
        PanicOutcome::SupersededByStop { stop_ticket: stop };
    assert!(
        !update.is_valid(),
        "A superseded result requires the completed Stop receipt"
    );
    update.urgent.stop = Some(StopAck {
        ticket: stop,
        cancelled_through_control: 9,
    });
    assert!(update.is_valid());
    update.urgent.stop.as_mut().unwrap().ticket.id = 1;
    assert!(!update.is_valid());
}

fn pair() -> (live::LiveAdmission, live::LiveRenderer) {
    live::live_pair(scope(), 8_000, 1).unwrap()
}
fn packet(scope: LiveScope, sequence: u64, events: &[LiveEvent]) -> LiveBatch {
    let mut batch = LiveBatch::empty(scope, sequence, None);
    for event in events {
        batch.try_push(*event).unwrap();
    }
    batch
}
fn control_event(source: u8, ticks: u64, event: LiveEventKind) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: ticks,
        event,
    }
}
fn off(source: u8, ticks: u64, voice: u64, velocity: u16) -> LiveEvent {
    control_event(
        source,
        ticks,
        LiveEventKind::NoteOff {
            voice,
            release_velocity16: velocity,
        },
    )
}
fn sustain(source: u8, ticks: u64, down: bool) -> LiveEvent {
    control_event(source, ticks, LiveEventKind::Sustain { down })
}

#[test]
fn independent_native_callback_packet_budget_includes_empty_packets_and_all_subblocks() {
    for frames in [1, 2_047, 2_048, 2_049, 16_384] {
        let (mut admission, mut renderer) = live::live_pair(scope(), 48_000, 2).unwrap();
        for n in 1..=16 {
            admission.admit(LiveBatch::empty(scope(), n, None)).unwrap();
        }
        let mut out = vec![1.0; frames * 2];
        assert_eq!(hot_path(|| renderer.render_into(&mut out)), (0, 0));
        assert_eq!(
            admission.progress().callback_applied_sequence,
            Some(4),
            "frames={frames}"
        );
        // Refill consumed capacity: each invocation still gets four, including empties.
        for n in 17..=20 {
            admission.admit(LiveBatch::empty(scope(), n, None)).unwrap();
        }
        renderer.render_into(&mut [0.0; 2]);
        assert_eq!(admission.progress().callback_applied_sequence, Some(8));
        assert!(out.iter().all(|sample| *sample == 0.0));
    }
}

#[test]
fn callback_shape_fault_silences_all_formats_without_allocating_or_consuming_events() {
    for samples in [3, (LIVE_MAX_CALLBACK_FRAMES + 1) * 2] {
        let (mut admission, mut renderer) = live::live_pair(scope(), 48_000, 2).unwrap();
        admission.admit(packet(scope(), 1, &[event(1, 1)])).unwrap();
        let mut out = vec![0.75; samples];
        assert_eq!(hot_path(|| renderer.render_into(&mut out)), (0, 0));
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(admission.progress().callback_applied_sequence, None);
        assert_eq!(
            admission.progress().fault.unwrap().kind,
            LiveFaultKind::CallbackShape
        );
        let mut signed = [1_i16; 8];
        let mut unsigned = [1_u16; 8];
        assert_eq!(
            hot_path(|| {
                renderer.render_i16(&mut signed);
                renderer.render_u16(&mut unsigned);
            }),
            (0, 0)
        );
        assert_eq!(signed, [0; 8]);
        assert_eq!(unsigned, [32768; 8]);
    }
}

#[test]
fn later_invalid_event_prevents_entire_packet_from_sounding_and_latches_first_fault() {
    let (mut admission, mut renderer) = pair();
    admission
        .admit(packet(scope(), 1, &[event(1, 1), off(1, 2, 99, 0)]))
        .unwrap();
    let mut out = [1.0; 64];
    assert_eq!(hot_path(|| renderer.render_into(&mut out)), (0, 0));
    assert_eq!(out, [0.0; 64]);
    let fault = admission.progress().fault.unwrap();
    assert_eq!(fault.kind, LiveFaultKind::SourceOwnership);
    assert_eq!(fault.sequence, Some(1));
    assert_eq!(admission.progress().active_voices, 0);
    assert_eq!(admission.progress().callback_applied_sequence, None);
    admission.control().fault(LiveFaultKind::Device, None);
    assert_eq!(admission.progress().fault, Some(fault));
    assert_eq!(
        admission.begin_panic(scope()),
        Err(LiveSubmitError::Faulted)
    );
}

#[test]
fn repeated_pitch_voices_and_sustain_are_owned_by_source() {
    let (mut admission, mut renderer) = pair();
    admission
        .admit(packet(scope(), 1, &[event(1, 1), event(1, 2), event(2, 1)]))
        .unwrap();
    renderer.render_into(&mut [0.0; 100]);
    assert_eq!(admission.progress().active_voices, 3);
    admission
        .admit(packet(
            scope(),
            2,
            &[
                sustain(1, 3, true),
                off(1, 4, 1, u16::MAX),
                off(1, 5, 2, u16::MAX),
                off(2, 2, 1, u16::MAX),
            ],
        ))
        .unwrap();
    renderer.render_into(&mut [0.0; 400]);
    assert_eq!(admission.progress().active_voices, 2);
    assert_eq!(admission.progress().release_tail_voices, 0);
    // A different source's pedal does not release source one's sustained voices.
    admission
        .admit(packet(scope(), 3, &[sustain(2, 3, false)]))
        .unwrap();
    renderer.render_into(&mut [0.0; 400]);
    assert_eq!(admission.progress().active_voices, 2);
    admission
        .admit(packet(scope(), 4, &[sustain(1, 6, false)]))
        .unwrap();
    renderer.render_into(&mut [0.0; 400]);
    assert_eq!(
        admission.progress().active_voices,
        0,
        "Sustain release must retain each NoteOff's maximum release velocity"
    );
}

#[test]
fn pulse_auto_releases_without_gui_and_preserves_release_velocity() {
    for (release_velocity16, expected_active) in [(0, 1), (u16::MAX, 0)] {
        let (mut admission, mut renderer) = pair();
        let pulse = control_event(
            4,
            1,
            LiveEventKind::Pulse {
                note: 1,
                voice: 1,
                key: 69,
                velocity16: u16::MAX,
                release_velocity16,
                patch: LivePatch::Keys,
                duration_frames: 100,
            },
        );
        admission.admit(packet(scope(), 1, &[pulse])).unwrap();
        let mut output = [0.0; 500];
        assert_eq!(hot_path(|| renderer.render_into(&mut output)), (0, 0));
        assert!(output.iter().any(|sample| *sample != 0.0));
        assert_eq!(admission.progress().active_voices, expected_active);
        renderer.render_into(&mut [0.0; 1500]);
        assert_eq!(admission.progress().active_voices, 0);
        admission
            .admit(packet(scope(), 2, &[off(4, 2, 1, 12)]))
            .unwrap();
        renderer.render_into(&mut [0.0; 1]);
        assert_eq!(admission.progress().late_releases, 1);
        assert!(admission.progress().fault.is_none());
    }
}

#[test]
fn releasing_voices_keep_their_slots_and_polyphony_exhaustion_fails_closed() {
    let (mut admission, mut renderer) = pair();
    for seq in 1..=2 {
        let events: Vec<_> = ((seq - 1) * 32 + 1..=seq * 32)
            .map(|voice| event(1, voice))
            .collect();
        admission.admit(packet(scope(), seq, &events)).unwrap();
    }
    renderer.render_into(&mut [0.0; 100]);
    assert_eq!(admission.progress().active_voices, 64);
    for seq in 3..=4 {
        let events: Vec<_> = ((seq - 3) * 32 + 1..=(seq - 2) * 32)
            .map(|voice| off(1, 64 + voice, voice, 0))
            .collect();
        admission.admit(packet(scope(), seq, &events)).unwrap();
    }
    renderer.render_into(&mut [0.0; 1]);
    assert_eq!(admission.progress().release_tail_voices, 64);
    let mut extra = event(1, 65);
    extra.observed_ticks = 129;
    admission.admit(packet(scope(), 5, &[extra])).unwrap();
    let mut output = [1.0; 64];
    renderer.render_into(&mut output);
    assert_eq!(output, [0.0; 64]);
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::Polyphony
    );
    assert_eq!(admission.progress().callback_applied_sequence, Some(4));
}

#[test]
fn queue_overflow_and_sequence_gap_reject_without_advancing_watermark() {
    let (mut admission, mut renderer) = pair();
    for n in 1..=16 {
        admission.admit(LiveBatch::empty(scope(), n, None)).unwrap();
    }
    assert_eq!(
        admission.admit(LiveBatch::empty(scope(), 17, None)),
        Err(LiveSubmitError::Saturated)
    );
    assert_eq!(admission.progress().worker_accepted_sequence, Some(16));
    assert_eq!(admission.progress().overflow_batches, 1);
    renderer.render_into(&mut [0.0; 1]);
    assert_eq!(admission.progress().callback_applied_sequence, None);
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::QueueOverflow
    );
    let (mut admission, _) = pair();
    assert_eq!(
        admission.admit(LiveBatch::empty(scope(), 2, None)),
        Err(LiveSubmitError::Invalid)
    );
    assert_eq!(admission.progress().worker_accepted_sequence, None);
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::Sequence
    );
}

#[test]
fn panic_ack_waits_for_callback_owned_clear_and_bounded_old_queue_drain() {
    let (mut admission, mut renderer) = pair();
    admission.admit(packet(scope(), 1, &[event(1, 1)])).unwrap();
    renderer.render_into(&mut [0.0; 32]);
    for n in 2..=17 {
        admission.admit(LiveBatch::empty(scope(), n, None)).unwrap();
    }
    let next = admission.begin_panic(scope()).unwrap();
    assert_eq!(admission.begin_panic(scope()).unwrap(), next);
    assert!(admission.finish_panic().unwrap().is_none());
    assert_eq!(
        admission.progress().active_voices,
        1,
        "Control must not directly clear callback voices"
    );
    assert_eq!(
        admission.admit(LiveBatch::empty(scope(), 18, None)),
        Err(LiveSubmitError::PanicPending)
    );
    for n in 0..4 {
        let mut out = [1.0; 1];
        assert_eq!(hot_path(|| renderer.render_into(&mut out)), (0, 0));
        assert_eq!(out, [0.0]);
        let completion = admission.finish_panic().unwrap();
        if n < 3 {
            assert!(completion.is_none());
        } else {
            assert_eq!(completion.unwrap().scope, next);
        }
    }
    assert_eq!(admission.progress().next_sequence(), Some(1));
    assert_eq!(
        admission.admit(packet(scope(), 18, &[off(1, 200, 1, 0)])),
        Err(LiveSubmitError::StaleScope)
    );
    assert!(admission.progress().fault.is_none());
    admission.admit(packet(next, 1, &[event(1, 1)])).unwrap();
    renderer.render_into(&mut [0.0; 32]);
    assert_eq!(admission.progress().active_voices, 1);
}

#[test]
fn panic_counter_exhaustion_is_sticky_and_never_reuses_generation() {
    let maximum = LiveScope {
        panic_generation: u64::MAX,
        ..scope()
    };
    let (mut admission, _) = live::live_pair(maximum, 8000, 1).unwrap();
    assert_eq!(
        admission.begin_panic(maximum),
        Err(LiveSubmitError::Faulted)
    );
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::CounterExhausted
    );
    assert_eq!(admission.scope(), maximum);
    assert!(admission.lease_credit().is_none());
}

#[test]
fn exact_deadline_cannot_be_extended_by_late_credit_before_next_callback() {
    let (mut admission, mut renderer) = pair();
    let credit = admission.lease_credit().unwrap();
    admission.admit(packet(scope(), 1, &[event(1, 1)])).unwrap();
    renderer.render_into(&mut [0.0; 8000]);
    assert_eq!(admission.progress().frames, 8000);
    assert_eq!(
        admission.admit(LiveBatch::empty(scope(), 2, Some(credit.token))),
        Err(LiveSubmitError::Faulted)
    );
    assert_eq!(admission.progress().lease_deadline_frame, 8000);
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::LeaseExpired
    );
    let mut out = [1.0; 1];
    renderer.render_into(&mut out);
    assert_eq!(out, [0.0]);
}

#[test]
fn queued_reused_credit_and_status_reads_cannot_renew_a_frozen_gui_indefinitely() {
    let (mut admission, mut renderer) = pair();
    let credit = admission.lease_credit().unwrap();
    renderer.render_into(&mut [0.0; 1000]);
    for n in 1..=16 {
        admission
            .admit(LiveBatch::empty(scope(), n, Some(credit.token)))
            .unwrap();
    }
    assert_eq!(admission.progress().lease_deadline_frame, 9000);
    for _ in 0..100 {
        assert_eq!(admission.progress().lease_deadline_frame, 9000);
    }
    renderer.render_into(&mut [0.0; 7999]);
    assert_eq!(admission.progress().lease_deadline_frame, 9000);
    renderer.render_into(&mut [0.0; 2]);
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::LeaseExpired
    );
    assert!(admission.lease_credit().is_none());
    assert_eq!(
        admission.begin_panic(scope()),
        Err(LiveSubmitError::Faulted)
    );
}

#[test]
fn independently_destroyed_render_and_error_closures_keep_backing_owners_on_control() {
    let (admission, renderer) = pair();
    let control = admission.control();
    let guard = live::LiveErrorGuard::new(control.clone());
    assert!(!control.render_destroyed());
    assert!(!control.error_destroyed());
    assert!(admission.backing_owner_count() >= 2);
    assert_eq!(hot_path(|| drop(renderer)), (0, 0));
    assert!(control.render_destroyed());
    assert!(!control.error_destroyed());
    assert_eq!(admission.backing_owner_count(), 1);
    assert_eq!(hot_path(|| drop(guard)), (0, 0));
    assert!(control.error_destroyed());
}

#[test]
fn zero_velocity_is_a_silent_owned_voice_and_all_patches_render_without_heap_work() {
    let (mut admission, mut renderer) = pair();
    let mut silent = event(1, 1);
    if let LiveEventKind::NoteOn { velocity16, .. } = &mut silent.event {
        *velocity16 = 0;
    }
    admission.admit(packet(scope(), 1, &[silent])).unwrap();
    let mut out = [1.0; 64];
    renderer.render_into(&mut out);
    assert_eq!(out, [0.0; 64]);
    assert_eq!(admission.progress().active_voices, 1);
    admission
        .admit(packet(scope(), 2, &[off(1, 2, 1, u16::MAX)]))
        .unwrap();
    renderer.render_into(&mut [0.0; 400]);
    assert_eq!(admission.progress().active_voices, 0);
    let (mut admission, mut renderer) = live::live_pair(scope(), 48_000, 2).unwrap();
    for sequence in 1..=2 {
        let events: Vec<_> = ((sequence - 1) * 32 + 1..=sequence * 32)
            .map(|voice| {
                let mut e = event(1, voice);
                if let LiveEventKind::NoteOn { patch, .. } = &mut e.event {
                    *patch = [
                        LivePatch::Keys,
                        LivePatch::Bass,
                        LivePatch::Pad,
                        LivePatch::Drums,
                    ][voice as usize % 4];
                }
                e
            })
            .collect();
        admission.admit(packet(scope(), sequence, &events)).unwrap();
    }
    let mut f32_out = vec![0.0; LIVE_MAX_CALLBACK_FRAMES * 2];
    let mut i16_out = [0_i16; 2049 * 2];
    let mut u16_out = [0_u16; 2049 * 2];
    assert_eq!(
        hot_path(|| {
            renderer.render_into(&mut f32_out);
            renderer.render_i16(&mut i16_out);
            renderer.render_u16(&mut u16_out);
        }),
        (0, 0)
    );
    assert!(
        f32_out
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
    );
    assert!(f32_out.iter().any(|sample| *sample != 0.0));
    assert!(admission.progress().fault.is_none());
}

#[test]
fn stale_session_is_inert_but_invalid_current_source_or_clock_faults_without_admission() {
    let (mut admission, _) = pair();
    let mut stale = scope();
    stale.worker_generation = LiveId([9; 16]);
    assert_eq!(
        admission.admit(packet(stale, 1, &[event(1, 1)])),
        Err(LiveSubmitError::StaleScope)
    );
    assert_eq!(admission.progress().worker_accepted_sequence, None);
    assert!(admission.progress().fault.is_none());
    for (source, generation) in [(5, 1), (1, 2)] {
        let (mut admission, _) = pair();
        let mut invalid = event(source, 1);
        invalid.connection_generation = generation;
        assert_eq!(
            admission.admit(packet(scope(), 1, &[invalid])),
            Err(LiveSubmitError::Invalid)
        );
        assert_eq!(admission.progress().worker_accepted_sequence, None);
        assert_eq!(
            admission.progress().fault.unwrap().kind,
            LiveFaultKind::SourceOwnership
        );
    }
    let (mut admission, _) = pair();
    let mut first = event(1, 1);
    first.observed_ticks = 100;
    admission.admit(packet(scope(), 1, &[first])).unwrap();
    assert_eq!(
        admission.admit(packet(scope(), 2, &[off(1, 99, 1, 0)])),
        Err(LiveSubmitError::Invalid)
    );
    assert_eq!(admission.progress().worker_accepted_sequence, Some(1));
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::Clock
    );
}

#[test]
fn panic_resets_voice_handles_but_never_rewinds_same_epoch_source_clock() {
    let (mut admission, mut renderer) = pair();
    let mut first = event(1, 1);
    first.observed_ticks = 1000;
    admission.admit(packet(scope(), 1, &[first])).unwrap();
    renderer.render_into(&mut [0.0; 64]);
    let next = admission.begin_panic(scope()).unwrap();
    renderer.render_into(&mut [0.0; 1]);
    assert_eq!(admission.finish_panic().unwrap().unwrap().scope, next);
    let mut stale = event(1, 1);
    stale.observed_ticks = 999;
    assert_eq!(
        admission.admit(packet(next, 1, &[stale])),
        Err(LiveSubmitError::Invalid)
    );
    assert_eq!(admission.progress().worker_accepted_sequence, None);
    assert_eq!(
        admission.progress().fault.unwrap().kind,
        LiveFaultKind::Clock
    );
    assert_eq!(admission.progress().fault.unwrap().scope, next);
}
