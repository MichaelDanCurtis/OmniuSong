use super::*;
use crate::{
    live_types::{LiveEvent, LiveId, LiveSourceBinding, LiveSourceKind},
    performance_records::PerformanceSource,
    performance_tap::TapInfo,
};
use omniusong_core::contracts::{ClockEpochId, EndpointId, TakeId};
use omniusong_recording::InspectionLimits;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn budget() -> InspectionBudget {
    InspectionBudget::new(
        InspectionLimits::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
fn config() -> PerformanceConfig {
    let kinds = [
        LiveSourceKind::ComputerKeys,
        LiveSourceKind::PointerKeys,
        LiveSourceKind::Pads,
        LiveSourceKind::AccessiblePulse,
    ];
    PerformanceConfig {
        take_id: TakeId("projection-test-take".into()),
        info: TapInfo {
            scope: LiveScope {
                worker_generation: LiveId([1; 16]),
                session_id: LiveId([2; 16]),
                panic_generation: 1,
            },
            sample_rate: 8000,
            channels: 1,
        },
        gui_clock: LiveClock {
            domain: "test-gui".into(),
            epoch: "test-epoch".into(),
            ticks_per_second_numerator: 1_000_000,
            ticks_per_second_denominator: 1,
        },
        output_epoch: ClockEpochId("test-output".into()),
        sources: std::array::from_fn(|i| PerformanceSource {
            binding: LiveSourceBinding {
                kind: kinds[i],
                handle: i as u8 + 1,
                connection_generation: 1,
            },
            endpoint: EndpointId(format!("source-{}", i + 1)),
        }),
    }
}
fn at(ordinal: u64) -> PerformanceRecordLocation {
    PerformanceRecordLocation {
        stream_id: "control".into(),
        stream_event_ordinal: ordinal,
        chunk_ordinal: 2,
        record_index: ordinal as u32,
        callback_index: 1,
        batch_ordinal: None,
        event_index: None,
        applied_source_ordinal: None,
    }
}
fn on(note: u64, patch: LivePatch) -> LiveEventKind {
    LiveEventKind::NoteOn {
        note,
        voice: note,
        key: 60,
        velocity16: 0,
        patch,
    }
}
fn off(voice: u64, velocity: u16) -> LiveEventKind {
    LiveEventKind::NoteOff {
        voice,
        release_velocity16: velocity,
    }
}
fn events(
    model: &mut Lifecycle,
    frame: u64,
    source: u8,
    kinds: &[LiveEventKind],
    ordinal: u64,
    governor: &InspectionBudget,
) -> Result<()> {
    model.advance(frame, governor)?;
    let mut batch = LiveBatch::empty(model.scope, ordinal + 1, None);
    for kind in kinds {
        batch
            .try_push(LiveEvent {
                source,
                connection_generation: 1,
                observed_ticks: frame,
                event: *kind,
            })
            .unwrap();
    }
    model.batch(&batch, frame, 1, &at(ordinal), governor)
}
fn end(
    model: Lifecycle,
    request: &PerformanceProjectionRequest,
    frame: u64,
    governor: &InspectionBudget,
) -> Result<PerformanceProjectionReview> {
    let mut model = model;
    model.advance(frame, governor)?;
    model.frame = frame;
    model.next_callback = 2;
    let checkpoint = PerformanceCheckpoint {
        group_sequence: 1,
        previous_checkpoint_sha256: None,
        first_callback_index: 1,
        last_callback_index: 1,
        audio_end: frame,
        scope_after: model.scope,
        chunks: Vec::new(),
        prefixes: Vec::new(),
    };
    model.finish(
        request,
        2,
        &config().gui_clock,
        &checkpoint,
        false,
        governor,
    )
}
fn model(req: &PerformanceProjectionRequest, governor: &InspectionBudget) -> Lifecycle {
    Lifecycle::new(&config(), req, governor).unwrap()
}

#[test]
fn applied_after_rejected_batch_cannot_become_a_performed_observation() {
    let governor = budget();
    let req = super::super::tests::request();
    let mut state = model(&req, &governor);
    state
        .record(
            PerformanceRecord::Begin {
                callback_index: 1,
                begin: TapBegin {
                    scope: state.scope,
                    start_frame: 0,
                    frames: 8,
                    sample_rate: 8000,
                    channels: 1,
                },
            },
            at(0),
            &governor,
        )
        .unwrap();
    // A malformed rejected fixed packet is evidence, never an event array to
    // index. The renderer stops healthy application immediately on rejection.
    let mut rejected = LiveBatch::empty(state.scope, 9, None);
    rejected.len = u8::MAX;
    state
        .record(
            PerformanceRecord::Batch {
                callback_index: 1,
                packet_ordinal: 1,
                batch: Box::new(rejected),
                disposition: BatchDisposition::Rejected {
                    fault: crate::live_types::LiveFaultKind::SourceOwnership,
                },
            },
            at(1),
            &governor,
        )
        .unwrap();
    let mut applied = LiveBatch::empty(state.scope, 1, None);
    applied
        .try_push(LiveEvent {
            source: 1,
            connection_generation: 1,
            observed_ticks: 0,
            event: on(1, LivePatch::Keys),
        })
        .unwrap();
    let result = state.record(
        PerformanceRecord::Batch {
            callback_index: 1,
            packet_ordinal: 2,
            batch: Box::new(applied),
            disposition: BatchDisposition::Applied,
        },
        at(2),
        &governor,
    );
    assert!(
        matches!(result, Err(RecordingError::Invalid(message)) if message.contains("after a rejected batch"))
    );
    assert!(state.observations.is_empty());
    assert_eq!(state.counts.applied_events, 0);
    assert_eq!(state.counts.rejected_batches, 1);
}

#[test]
fn sustain_gate_preserves_physical_key_up_and_later_tail_cut() {
    let governor = budget();
    let req = super::super::tests::request();
    let mut state = model(&req, &governor);
    events(&mut state, 0, 1, &[on(1, LivePatch::Keys)], 0, &governor).unwrap();
    events(
        &mut state,
        16,
        1,
        &[LiveEventKind::Sustain { down: true }, off(1, 34567)],
        1,
        &governor,
    )
    .unwrap();
    events(
        &mut state,
        32,
        1,
        &[LiveEventKind::Sustain { down: false }],
        2,
        &governor,
    )
    .unwrap();
    state
        .cut(40, PerformanceGateEndCause::Finish {}, &at(3), &governor)
        .unwrap();
    let result = end(state, &req, 40, &governor).unwrap();
    let note = &result.notes[0];
    assert_eq!(
        (
            note.onset_frame,
            note.gate_end_frame,
            note.values.start_tick,
            note.values.duration_ticks
        ),
        (0, 32, 0, 8)
    );
    assert_eq!(note.values.velocity16, 0);
    assert_eq!(
        note.physical_key_up
            .as_ref()
            .map(|r| (r.frame, r.release_velocity16)),
        Some((16, 34567))
    );
    assert!(matches!(
        note.gate_end.cause,
        PerformanceGateEndCause::SustainUp {}
    ));
    assert_eq!(note.gate_end.release_velocity16, Some(34567));
    assert!(note.voice_ended.is_none());
    assert_eq!(note.hard_cut.as_ref().unwrap().frame, 40);
    assert!(
        result
            .disclosures
            .contains(&PerformanceProjectionDisclosure::ObservedHardCutsRetained)
    );
    assert!(
        !result
            .disclosures
            .contains(&PerformanceProjectionDisclosure::UnknownTailExcluded)
    );
}

#[test]
fn actual_pulse_release_keeps_first_later_physical_key_up_without_restarting_gate() {
    let governor = budget();
    let mut req = super::super::tests::request();
    req.region.end = 16;
    let mut state = model(&req, &governor);
    events(
        &mut state,
        0,
        1,
        &[LiveEventKind::Pulse {
            note: 1,
            voice: 1,
            key: 61,
            velocity16: 0,
            release_velocity16: 123,
            patch: LivePatch::Keys,
            duration_frames: 8,
        }],
        0,
        &governor,
    )
    .unwrap();
    let mut order = None;
    state
        .transition(
            TapTransition {
                scope: state.scope,
                source: 1,
                voice: 1,
                frame: 8,
                kind: TapTransitionKind::AutoRelease {
                    release_velocity16: 123,
                },
            },
            &at(1),
            &mut order,
            &governor,
        )
        .unwrap();
    events(&mut state, 12, 1, &[off(1, 4444)], 2, &governor).unwrap();
    events(&mut state, 14, 1, &[off(1, 5555)], 3, &governor).unwrap();
    state
        .cut(16, PerformanceGateEndCause::Finish {}, &at(4), &governor)
        .unwrap();
    let result = end(state, &req, 16, &governor).unwrap();
    let note = &result.notes[0];
    assert_eq!(note.gate_end_frame, 8);
    assert!(matches!(
        note.gate_end.cause,
        PerformanceGateEndCause::AutoRelease {}
    ));
    assert_eq!(note.gate_end.release_velocity16, Some(123));
    assert_eq!(
        note.physical_key_up
            .as_ref()
            .map(|r| (r.frame, r.release_velocity16)),
        Some((12, 4444))
    );
    assert_eq!(note.pulse.unwrap().duration_frames, 8);
}

#[test]
fn physical_key_up_after_voice_ended_is_retained_as_a_distinct_late_observation() {
    let governor = budget();
    let mut req = super::super::tests::request();
    req.region.end = 400;
    let mut state = model(&req, &governor);
    events(
        &mut state,
        0,
        1,
        &[LiveEventKind::Pulse {
            note: 1,
            voice: 1,
            key: 60,
            velocity16: 700,
            release_velocity16: 65535,
            patch: LivePatch::Keys,
            duration_frames: 4,
        }],
        0,
        &governor,
    )
    .unwrap();
    state
        .transition(
            TapTransition {
                scope: state.scope,
                source: 1,
                voice: 1,
                frame: 4,
                kind: TapTransitionKind::AutoRelease {
                    release_velocity16: 65535,
                },
            },
            &at(1),
            &mut None,
            &governor,
        )
        .unwrap();
    // Independently count the short renderer tail, whose endpoint is separate
    // from both the physical key and the musical gate.
    let mut envelope = 0.0f32;
    for _ in 0..4 {
        envelope = (envelope + 1.0f32 / (0.003f32 * 8000.0)).min(1.0);
    }
    let step = envelope / (0.035f32 * 8000.0);
    let mut ended = 4;
    loop {
        envelope -= step;
        if envelope <= 0.0 {
            break;
        }
        ended += 1;
    }
    state
        .transition(
            TapTransition {
                scope: state.scope,
                source: 1,
                voice: 1,
                frame: ended,
                kind: TapTransitionKind::VoiceEnded,
            },
            &at(2),
            &mut None,
            &governor,
        )
        .unwrap();
    assert!(state.voices.iter().all(Option::is_none));
    events(&mut state, ended + 1, 1, &[off(1, 2222)], 3, &governor).unwrap();
    events(&mut state, ended + 2, 1, &[off(1, 3333)], 4, &governor).unwrap();
    let result = end(state, &req, 400, &governor).unwrap();
    let note = &result.notes[0];
    assert_eq!(note.gate_end_frame, 4);
    assert_eq!(note.voice_ended.as_ref().unwrap().frame, ended);
    assert_eq!(
        note.physical_key_up
            .as_ref()
            .map(|r| (r.frame, r.release_velocity16)),
        Some((ended + 1, 2222))
    );
    assert!(note.hard_cut.is_none());
    assert_eq!(result.counts.late_note_offs, 2);
}

#[test]
fn end_count_forgery_refuses_and_malformed_discarded_packets_are_never_indexed() {
    let governor = budget();
    let req = super::super::tests::request();
    let begin = |scope| PerformanceRecord::Begin {
        callback_index: 1,
        begin: TapBegin {
            scope,
            start_frame: 0,
            frames: 8,
            sample_rate: 8000,
            channels: 1,
        },
    };
    let ending = |scope| PerformanceRecord::End {
        callback_index: 1,
        end_frame: 8,
        scope_after: scope,
        applied_sequence: None,
        active_voices: 0,
        release_tail_voices: 0,
        clear_count: 0,
        transition_count: 0,
        first_fault: None,
    };
    let mut state = model(&req, &governor);
    let scope = state.scope;
    state.record(begin(scope), at(0), &governor).unwrap();
    events(&mut state, 0, 1, &[on(1, LivePatch::Keys)], 1, &governor).unwrap();
    assert!(state.record(ending(scope), at(2), &governor).is_err());

    let mut state = model(&req, &governor);
    state.record(begin(scope), at(0), &governor).unwrap();
    let mut malformed = LiveBatch::empty(scope, 0, None);
    malformed.len = 255;
    malformed.events[0] = Some(LiveEvent {
        source: 255,
        connection_generation: 0,
        observed_ticks: u64::MAX,
        event: on(99, LivePatch::Keys),
    });
    state
        .record(
            PerformanceRecord::Batch {
                callback_index: 1,
                packet_ordinal: 1,
                batch: Box::new(malformed),
                disposition: BatchDisposition::DiscardedStale,
            },
            at(1),
            &governor,
        )
        .unwrap();
    state.record(ending(scope), at(2), &governor).unwrap();
    assert_eq!(state.counts.discarded_batches, 1);
    assert_eq!(state.counts.observations, 0);
    assert!(state.indices.is_empty());
    assert!(state.observations.is_empty());
}

#[test]
fn impossible_voice_counters_and_auto_release_transitions_refuse() {
    let governor = budget();
    let req = super::super::tests::request();
    let mut state = model(&req, &governor);
    assert!(events(&mut state, 0, 1, &[on(2, LivePatch::Keys)], 0, &governor).is_err());
    let mut state = model(&req, &governor);
    assert!(events(&mut state, 0, 1, &[off(1, 0)], 0, &governor).is_err());
    let mut state = model(&req, &governor);
    let pulse = LiveEventKind::Pulse {
        note: 1,
        voice: 1,
        key: 60,
        velocity16: 500,
        release_velocity16: 0,
        patch: LivePatch::Keys,
        duration_frames: 8,
    };
    events(&mut state, 0, 1, &[pulse], 0, &governor).unwrap();
    let scope = state.scope;
    assert!(
        state
            .transition(
                TapTransition {
                    scope,
                    source: 1,
                    voice: 1,
                    frame: 7,
                    kind: TapTransitionKind::AutoRelease {
                        release_velocity16: 0
                    }
                },
                &at(1),
                &mut None,
                &governor
            )
            .is_err()
    );
    let mut state = model(&req, &governor);
    events(&mut state, 0, 1, &[pulse], 0, &governor).unwrap();
    events(&mut state, 8, 1, &[off(1, 100)], 1, &governor).unwrap();
    assert!(
        state
            .transition(
                TapTransition {
                    scope,
                    source: 1,
                    voice: 1,
                    frame: 8,
                    kind: TapTransitionKind::AutoRelease {
                        release_velocity16: 0
                    }
                },
                &at(2),
                &mut None,
                &governor
            )
            .is_err()
    );
    let mut state = model(&req, &governor);
    events(&mut state, 0, 1, &[pulse], 0, &governor).unwrap();
    assert!(state.advance(9, &governor).is_err());
}

#[test]
fn region_crossing_zero_duration_and_incomplete_policies_never_trim_or_pad() {
    for policy in [
        PerformanceProjectionPolicy::Reject,
        PerformanceProjectionPolicy::OmitWithDisclosure,
    ] {
        let governor = budget();
        let mut req = super::super::tests::request();
        req.collapsed = policy;
        let mut state = model(&req, &governor);
        events(
            &mut state,
            0,
            1,
            &[on(1, LivePatch::Keys), off(1, 1)],
            0,
            &governor,
        )
        .unwrap();
        state
            .transition(
                TapTransition {
                    scope: state.scope,
                    source: 1,
                    voice: 1,
                    frame: 0,
                    kind: TapTransitionKind::VoiceEnded,
                },
                &at(1),
                &mut None,
                &governor,
            )
            .unwrap();
        let result = end(state, &req, 40, &governor);
        if policy == PerformanceProjectionPolicy::Reject {
            assert!(result.is_err());
        } else {
            let result = result.unwrap();
            assert!(result.notes.is_empty());
            assert_eq!(
                result.omissions[0].reason,
                PerformanceProjectionOmissionReason::CollapsedTicks
            );
            assert_eq!(result.omissions[0].gate_end_frame, Some(0));
        }

        let governor = budget();
        let mut req = super::super::tests::request();
        req.region = PerformanceFrameRegion { start: 10, end: 20 };
        req.region_crossing = policy;
        let mut state = model(&req, &governor);
        events(&mut state, 0, 1, &[on(1, LivePatch::Keys)], 0, &governor).unwrap();
        events(&mut state, 15, 1, &[off(1, 1)], 1, &governor).unwrap();
        let result = end(state, &req, 20, &governor);
        if policy == PerformanceProjectionPolicy::Reject {
            assert!(result.is_err());
        } else {
            let result = result.unwrap();
            assert!(result.notes.is_empty());
            assert_eq!(
                result.omissions[0].reason,
                PerformanceProjectionOmissionReason::RegionCrossing
            );
            assert_eq!(
                (
                    result.omissions[0].onset_frame,
                    result.omissions[0].gate_end_frame
                ),
                (0, Some(15))
            );
        }

        let governor = budget();
        let mut req = super::super::tests::request();
        req.incomplete = policy;
        let mut state = model(&req, &governor);
        events(&mut state, 0, 1, &[on(1, LivePatch::Pad)], 0, &governor).unwrap();
        let result = end(state, &req, 40, &governor);
        if policy == PerformanceProjectionPolicy::Reject {
            assert!(result.is_err());
        } else {
            let result = result.unwrap();
            assert!(result.notes.is_empty());
            assert_eq!(
                result.omissions[0].reason,
                PerformanceProjectionOmissionReason::Incomplete
            );
            assert_eq!(result.omissions[0].gate_end_frame, None);
        }
    }
}

#[test]
fn omitted_crossing_gate_retains_actual_fault_cut_cause_and_record() {
    let governor = budget();
    let mut req = super::super::tests::request();
    req.region.end = 20;
    req.region_crossing = PerformanceProjectionPolicy::OmitWithDisclosure;
    let mut state = model(&req, &governor);
    events(&mut state, 0, 1, &[on(1, LivePatch::Pad)], 0, &governor).unwrap();
    let cause = PerformanceGateEndCause::Clear {
        reason: TapClearReason::FaultPending,
    };
    state.cut(40, cause.clone(), &at(1), &governor).unwrap();
    let result = end(state, &req, 40, &governor).unwrap();
    assert!(result.notes.is_empty());
    let omission = &result.omissions[0];
    assert_eq!(
        omission.reason,
        PerformanceProjectionOmissionReason::RegionCrossing
    );
    assert_eq!(omission.gate_end.as_ref().unwrap().cause, cause);
    assert_eq!(omission.hard_cut.as_ref().unwrap().frame, 40);
    assert_eq!(omission.hard_cut.as_ref().unwrap().record, at(1));
    assert!(
        result
            .disclosures
            .contains(&PerformanceProjectionDisclosure::ObservedHardCutsRetained)
    );
}

#[test]
fn more_than_twenty_thousand_outside_onsets_leave_capacity_for_a_tiny_selection() {
    let governor = budget();
    let mut req = super::super::tests::request();
    req.region = PerformanceFrameRegion { start: 10, end: 20 };
    let mut state = model(&req, &governor);
    for id in 1..=MAX_PROJECTION_OBSERVATIONS as u64 + 1 {
        events(
            &mut state,
            0,
            1,
            &[on(id, LivePatch::Keys), off(id, 0)],
            id,
            &governor,
        )
        .unwrap();
        state
            .transition(
                TapTransition {
                    scope: state.scope,
                    source: 1,
                    voice: id,
                    frame: 0,
                    kind: TapTransitionKind::VoiceEnded,
                },
                &at(id),
                &mut None,
                &governor,
            )
            .unwrap();
    }
    assert!(state.observations.is_empty());
    assert!(state.indices.is_empty());
    assert!(state.provisional.iter().all(Option::is_none));
    let selected = MAX_PROJECTION_OBSERVATIONS as u64 + 2;
    events(
        &mut state,
        10,
        1,
        &[on(selected, LivePatch::Keys)],
        selected,
        &governor,
    )
    .unwrap();
    events(
        &mut state,
        16,
        1,
        &[off(selected, 0)],
        selected + 1,
        &governor,
    )
    .unwrap();
    state
        .cut(
            20,
            PerformanceGateEndCause::Finish {},
            &at(selected + 2),
            &governor,
        )
        .unwrap();
    let result = end(state, &req, 20, &governor).unwrap();
    assert_eq!(result.notes.len(), 1);
    assert!(result.omissions.is_empty());
    assert_eq!(
        result.counts.outside_region,
        MAX_PROJECTION_OBSERVATIONS as u64 + 1
    );
    assert_eq!(
        result.counts.observations,
        MAX_PROJECTION_OBSERVATIONS as u64 + 2
    );
    assert!(sha(&result.unselected_observations_sha256));
}

#[test]
fn long_steady_hold_is_event_driven_and_cancelled_emission_has_no_review() {
    let flag = Arc::new(AtomicBool::new(false));
    let governor = InspectionBudget::new(InspectionLimits::default(), flag.clone()).unwrap();
    let mut req = super::super::tests::request();
    req.region.end = 8_000 * 60 * 30;
    let mut state = model(&req, &governor);
    events(&mut state, 0, 1, &[on(1, LivePatch::Pad)], 0, &governor).unwrap();
    let before = governor.usage().work_bytes;
    state.advance(req.region.end, &governor).unwrap();
    assert!(governor.usage().work_bytes - before < 300_000);
    state
        .cut(
            req.region.end,
            PerformanceGateEndCause::Finish {},
            &at(1),
            &governor,
        )
        .unwrap();
    flag.store(true, Ordering::Release);
    assert!(matches!(
        end(state, &req, req.region.end, &governor),
        Err(RecordingError::InspectionCancelled)
    ));
}

#[test]
fn placement_changes_review_identity_but_not_original_voice_identity() {
    let render = |offset| {
        let governor = budget();
        let mut req = super::super::tests::request();
        req.destination_start_tick = offset;
        let mut state = model(&req, &governor);
        events(&mut state, 0, 1, &[on(1, LivePatch::Keys)], 0, &governor).unwrap();
        state
            .cut(40, PerformanceGateEndCause::Finish {}, &at(1), &governor)
            .unwrap();
        end(state, &req, 40, &governor).unwrap()
    };
    let a = render(0);
    let b = render(1920);
    assert_ne!(a.review_sha256, b.review_sha256);
    assert_eq!(a.notes[0].identity_sha256, b.notes[0].identity_sha256);
    assert_eq!(a.notes[0].onset_frame, b.notes[0].onset_frame);
    assert_eq!(b.notes[0].values.start_tick, 1920);
}
