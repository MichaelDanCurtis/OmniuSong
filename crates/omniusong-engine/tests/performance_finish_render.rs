//! Real device-free renderer callbacks. No worker, device or normal-seal proof.
use omniusong_engine::{
    live::{live_pair, recording_live_pair},
    live_types::*,
    performance_tap::*,
};

fn scope() -> LiveScope {
    LiveScope {
        worker_generation: LiveId([1; 16]),
        session_id: LiveId([2; 16]),
        panic_generation: 1,
    }
}
fn id() -> LiveId {
    LiveId([3; 16])
}
fn event(kind: LiveEventKind, ticks: u64) -> LiveEvent {
    LiveEvent {
        source: 1,
        connection_generation: 1,
        observed_ticks: ticks,
        event: kind,
    }
}
fn on(voice: u64, velocity16: u16) -> LiveEventKind {
    LiveEventKind::NoteOn {
        note: voice,
        voice,
        key: 60,
        velocity16,
        patch: LivePatch::Keys,
    }
}
fn batch(scope: LiveScope, sequence: u64, events: &[LiveEventKind]) -> LiveBatch {
    let mut b = LiveBatch::empty(scope, sequence, None);
    for kind in events {
        b.try_push(event(*kind, sequence)).unwrap();
    }
    b
}
fn drain(owner: &mut PerformanceTapOwner) -> Vec<PerformancePacket> {
    let mut out = Vec::new();
    while let Some(packet) = owner.try_pop().unwrap() {
        out.push(packet);
    }
    out
}

#[test]
fn sixteen_batches_finish_only_after_complete_pcm_and_final_end_then_freeze() {
    let (mut admission, mut renderer, mut owner) = recording_live_pair(scope(), 48_000, 2).unwrap();
    let mut originals = Vec::new();
    for sequence in 1..=16 {
        let events = match sequence {
            1 => vec![on(1, 0)],
            2 => vec![on(2, u16::MAX)],
            _ => vec![],
        };
        let b = batch(scope(), sequence, &events);
        admission.admit(b).unwrap();
        originals.push(b);
    }
    let fence = admission.begin_finish(scope(), id()).unwrap();
    assert_eq!(fence.accepted_sequence, Some(16));
    assert!(admission.lease_credit().is_none());
    assert!(admission.progress().is_valid());
    assert_eq!(
        admission.admit(batch(scope(), 17, &[])),
        Err(LiveSubmitError::FinishPending)
    );
    assert_eq!(
        admission.begin_panic(scope()),
        Err(LiveSubmitError::FinishPending)
    );
    let mut packets = Vec::new();
    let mut output = vec![0.0; 257 * 2];
    for callback in 1..=4 {
        renderer.render_into(&mut output);
        let current = drain(&mut owner);
        assert_eq!(
            current
                .iter()
                .filter(|p| matches!(p.kind, PerformancePacketKind::Batch { .. }))
                .count(),
            4
        );
        assert!(current.iter().all(|p| p.callback_index == callback));
        if callback < 4 {
            assert!(admission.finish_ack().unwrap().is_none());
            assert!(
                !current
                    .iter()
                    .any(|p| matches!(p.kind, PerformancePacketKind::Finish(_)))
            );
        }
        packets.extend(current);
    }
    let ack = admission.finish_ack().unwrap().unwrap();
    assert_eq!(
        (
            ack.callback_index,
            ack.cut_frame,
            ack.voices_cut,
            ack.release_tails_cut
        ),
        (4, 1028, 2, 0)
    );
    assert_eq!(ack.fence, fence);
    assert_eq!(ack.policy, LiveFinishPolicy::CutAtCompleteCallbackEnd);
    assert!(output.iter().any(|v| *v != 0.0));
    let captured: Vec<_> = packets
        .iter()
        .filter_map(|p| {
            if let PerformancePacketKind::Batch { batch, disposition } = p.kind {
                assert_eq!(disposition, BatchDisposition::Applied);
                Some(batch)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(captured, originals);
    assert_eq!(
        packets[packets.len() - 2].kind,
        PerformancePacketKind::Finish(ack)
    );
    let PerformancePacketKind::End(end) = packets.last().unwrap().kind else {
        panic!("Missing final End")
    };
    assert_eq!((end.active_voices, end.release_tail_voices), (0, 0));
    assert_eq!(end.first_fault, None);
    assert!(end.clears.iter().all(Option::is_none));
    assert_eq!(end.applied_sequence, Some(16));
    let before = owner.control().snapshot().unwrap();
    for _ in 0..3 {
        output.fill(1.0);
        renderer.render_into(&mut output);
        assert!(output.iter().all(|v| v.to_bits() == 0));
        assert!(drain(&mut owner).is_empty());
        assert_eq!(owner.control().snapshot().unwrap(), before);
        assert_eq!(admission.progress().frames, 1028);
    }
    assert_eq!(admission.progress().state, LiveState::FinishedCut);
    assert!(admission.progress().is_valid());
    assert_eq!(admission.finish_ack().unwrap(), Some(ack));
    drop(renderer);
}

#[test]
fn finish_counts_sustained_held_and_release_tail_voices_without_fake_noteoffs() {
    let (mut a, mut r, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
    a.admit(batch(
        scope(),
        1,
        &[
            on(1, u16::MAX),
            on(2, 32_768),
            on(3, 1),
            LiveEventKind::Sustain { down: true },
            LiveEventKind::NoteOff {
                voice: 1,
                release_velocity16: 12_345,
            },
        ],
    ))
    .unwrap();
    r.render_into(&mut [0.0; 256]);
    let mut packets = drain(&mut owner);
    a.admit(batch(
        scope(),
        2,
        &[
            LiveEventKind::Sustain { down: false },
            LiveEventKind::NoteOff {
                voice: 2,
                release_velocity16: u16::MAX,
            },
        ],
    ))
    .unwrap();
    r.render_into(&mut [0.0; 1]);
    packets.extend(drain(&mut owner));
    assert_eq!(
        (a.progress().active_voices, a.progress().release_tail_voices),
        (3, 2)
    );
    a.begin_finish(scope(), id()).unwrap();
    r.render_into(&mut [0.0; 257]);
    packets.extend(drain(&mut owner));
    let finish = a.finish_ack().unwrap().unwrap();
    assert_eq!((finish.voices_cut, finish.release_tails_cut), (3, 2));
    let mut offs = 0;
    for p in packets {
        match p.kind {
            PerformancePacketKind::Batch { batch, .. } => {
                for e in batch.events[..usize::from(batch.len)].iter().flatten() {
                    if matches!(e.event, LiveEventKind::NoteOff { .. }) {
                        offs += 1;
                    }
                }
            }
            PerformancePacketKind::End(end) => assert_eq!(end.transition_len, 0),
            _ => {}
        }
    }
    assert_eq!(offs, 2);
    drop(r);
}

#[test]
fn empty_fence_is_not_sequence_zero_and_all_formats_silence_after_cut() {
    let (mut a, mut r, mut owner) = recording_live_pair(scope(), 48_000, 2).unwrap();
    let fence = a.begin_finish(scope(), id()).unwrap();
    assert_eq!(fence.accepted_sequence, None);
    r.render_into(&mut [0.0; 2]);
    let ack = a.finish_ack().unwrap().unwrap();
    assert_eq!(ack.cut_frame, 1);
    assert_eq!(ack.voices_cut, 0);
    let packets = drain(&mut owner);
    assert_eq!(packets.len(), 4);
    assert!(matches!(packets[2].kind, PerformancePacketKind::Finish(_)));
    let mut i16s = [1i16; 32];
    r.render_i16(&mut i16s);
    assert_eq!(i16s, [0; 32]);
    let mut u16s = [1u16; 32];
    r.render_u16(&mut u16s);
    assert_eq!(u16s, [32768; 32]);
    assert!(drain(&mut owner).is_empty());
    assert_eq!(a.progress().frames, 1);
    r.render_into(&mut [1.0; 3]);
    assert_eq!(a.finish_ack(), Err(LiveSubmitError::Faulted));
    assert_eq!(
        a.progress().fault.unwrap().kind,
        LiveFaultKind::CallbackShape
    );
    assert_eq!(a.progress().frames, 1);
    drop(r);
}

#[test]
fn finish_identity_scope_panic_and_ordinary_audition_guards_are_explicit() {
    let (mut plain, plain_renderer) = live_pair(scope(), 48_000, 1).unwrap();
    assert_eq!(
        plain.begin_finish(scope(), id()),
        Err(LiveSubmitError::NotLive)
    );
    assert_eq!(plain.finish_ack(), Err(LiveSubmitError::NotLive));
    assert!(plain.admit(batch(scope(), 1, &[])).is_ok());
    drop(plain_renderer);
    let (mut a, mut r, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
    assert_eq!(
        a.begin_finish(scope(), LiveId([0; 16])),
        Err(LiveSubmitError::Invalid)
    );
    let foreign = LiveScope {
        worker_generation: LiveId([9; 16]),
        ..scope()
    };
    assert_eq!(
        a.begin_finish(foreign, id()),
        Err(LiveSubmitError::StaleScope)
    );
    let next = a.begin_panic(scope()).unwrap();
    assert_eq!(
        a.begin_finish(scope(), id()),
        Err(LiveSubmitError::PanicPending)
    );
    r.render_into(&mut [0.0; 1]);
    drain(&mut owner);
    assert_eq!(a.finish_panic().unwrap().unwrap().scope, next);
    assert_eq!(
        a.begin_finish(scope(), id()),
        Err(LiveSubmitError::StaleScope)
    );
    let fence = a.begin_finish(next, id()).unwrap();
    let deadline = a.progress().lease_deadline_frame;
    assert_eq!(a.begin_finish(next, id()).unwrap(), fence);
    assert_eq!(a.progress().lease_deadline_frame, deadline);
    assert_eq!(
        a.begin_finish(next, LiveId([8; 16])),
        Err(LiveSubmitError::FinishPending)
    );
    r.render_into(&mut [0.0; 1]);
    drain(&mut owner);
    assert_eq!(a.finish_ack().unwrap().unwrap().fence, fence);
    assert_eq!(a.begin_finish(next, id()).unwrap(), fence);
    assert_eq!(a.progress().lease_deadline_frame, deadline);
    drop(r);
}

#[test]
fn finish_extends_only_a_still_live_lease_once_and_drains_four_max_callbacks() {
    let (mut a, mut r, mut owner) = recording_live_pair(scope(), 8_000, 1).unwrap();
    r.render_into(&mut vec![0.0; 7_999]);
    drain(&mut owner);
    for sequence in 1..=16 {
        a.admit(batch(scope(), sequence, &[])).unwrap();
    }
    a.begin_finish(scope(), id()).unwrap();
    let deadline = a.progress().lease_deadline_frame;
    assert_eq!(deadline, 7_999 + (6 * LIVE_MAX_CALLBACK_FRAMES) as u64);
    assert_eq!(
        a.begin_finish(scope(), id()).unwrap().accepted_sequence,
        Some(16)
    );
    assert_eq!(a.progress().lease_deadline_frame, deadline);
    for callback in 0..4 {
        r.render_into(&mut vec![0.0; LIVE_MAX_CALLBACK_FRAMES]);
        drain(&mut owner);
        if callback < 3 {
            assert!(a.finish_ack().unwrap().is_none());
        }
    }
    assert_eq!(
        a.finish_ack().unwrap().unwrap().cut_frame,
        7_999 + (4 * LIVE_MAX_CALLBACK_FRAMES) as u64
    );
    drop(r);
    let (mut a, mut r, mut owner) = recording_live_pair(scope(), 8_000, 1).unwrap();
    r.render_into(&mut vec![0.0; 8_000]);
    drain(&mut owner);
    assert_eq!(
        a.progress().fault.unwrap().kind,
        LiveFaultKind::LeaseExpired
    );
    assert_eq!(a.begin_finish(scope(), id()), Err(LiveSubmitError::Faulted));
    assert_eq!(a.finish_ack(), Err(LiveSubmitError::Faulted));
    drop(r);
}

#[test]
fn actual_fault_or_cancel_before_during_or_after_ack_never_yields_healthy_finish() {
    for timing in 0..3 {
        let (mut a, mut r, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
        if timing == 0 {
            a.control().fault(LiveFaultKind::Device, None);
            assert_eq!(a.begin_finish(scope(), id()), Err(LiveSubmitError::Faulted));
        } else {
            a.begin_finish(scope(), id()).unwrap();
            if timing == 1 {
                a.control().fault(LiveFaultKind::Device, None);
            }
            r.render_into(&mut [0.0; 128]);
            if timing == 2 {
                assert!(a.finish_ack().unwrap().is_some());
                a.control().fault(LiveFaultKind::Device, None);
            }
            assert_eq!(a.finish_ack(), Err(LiveSubmitError::Faulted));
        }
        let packets = drain(&mut owner);
        if timing != 2 {
            assert!(
                !packets
                    .iter()
                    .any(|p| matches!(p.kind, PerformancePacketKind::Finish(_)))
            );
        }
        drop(r);
    }
    for after in [false, true] {
        let (mut a, mut r, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
        a.begin_finish(scope(), id()).unwrap();
        if after {
            r.render_into(&mut [0.0; 128]);
            assert!(a.finish_ack().unwrap().is_some());
        }
        owner.control().cancel();
        if !after {
            r.render_into(&mut [0.0; 128]);
        }
        assert_eq!(a.finish_ack(), Err(LiveSubmitError::Faulted));
        drain(&mut owner);
        drop(r);
    }
}

#[test]
fn full_tap_reservation_and_live_queue_saturation_cannot_forge_finish_ack() {
    let (mut a, mut r, mut owner) = recording_live_pair(scope(), 768_000, 1).unwrap();
    for _ in 0..3 {
        r.render_into(&mut vec![0.0; LIVE_MAX_CALLBACK_FRAMES]);
    }
    assert!(a.progress().fault.is_none());
    a.begin_finish(scope(), id()).unwrap();
    r.render_into(&mut vec![0.0; LIVE_MAX_CALLBACK_FRAMES]);
    assert_eq!(a.finish_ack(), Err(LiveSubmitError::Faulted));
    assert_eq!(
        a.progress().fault.unwrap().kind,
        LiveFaultKind::RecordingTap
    );
    assert_eq!(
        owner.control().snapshot().unwrap().fault.unwrap().kind,
        TapFaultKind::Overflow
    );
    assert!(
        !drain(&mut owner)
            .iter()
            .any(|p| matches!(p.kind, PerformancePacketKind::Finish(_)))
    );
    drop(r);
    let (mut a, mut r, mut owner) = recording_live_pair(scope(), 48_000, 1).unwrap();
    for sequence in 1..=16 {
        a.admit(batch(scope(), sequence, &[])).unwrap();
    }
    assert_eq!(
        a.admit(batch(scope(), 17, &[])),
        Err(LiveSubmitError::Saturated)
    );
    assert_eq!(a.begin_finish(scope(), id()), Err(LiveSubmitError::Faulted));
    r.render_into(&mut [0.0; 1]);
    assert!(
        !drain(&mut owner)
            .iter()
            .any(|p| matches!(p.kind, PerformancePacketKind::Finish(_)))
    );
    drop(r);
}
