//! Independent software-only Panic progress observations. No native streams,
//! journals, files or normal-seal capabilities are created by these tests.
use omniusong_engine::{
    live::{live_pair, recording_live_pair},
    performance_tap::{PerformancePacketKind, TapClearReason},
    *,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct Allocator;
thread_local! {static ALLOCATIONS:Cell<(bool,usize,usize)>=const {Cell::new((false,0,0))};}
fn allocation(added: usize, removed: usize) {
    let _ = ALLOCATIONS.try_with(|state| {
        let (enabled, a, d) = state.get();
        if enabled {
            state.set((true, a + added, d + removed));
        }
    });
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation(1, 0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation(1, 0);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        allocation(0, 1);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation(1, 1);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
fn measured(work: impl FnOnce()) -> (usize, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ALLOCATIONS.with(|state| {
                let (_, a, d) = state.get();
                state.set((false, a, d));
            });
        }
    }
    ALLOCATIONS.with(|state| state.set((true, 0, 0)));
    let guard = Reset;
    work();
    drop(guard);
    ALLOCATIONS.with(|state| {
        let (_, a, d) = state.replace((false, 0, 0));
        (a, d)
    })
}
fn scope() -> LiveScope {
    LiveScope {
        worker_generation: LiveId([0x41; 16]),
        session_id: LiveId([0x42; 16]),
        panic_generation: 1,
    }
}
fn batch(scope: LiveScope, sequence: u64) -> LiveBatch {
    LiveBatch::empty(scope, sequence, None)
}

#[test]
fn pending_panic_keeps_the_published_old_applied_watermark_until_explicit_adoption() {
    let (mut admission, mut renderer) = live_pair(scope(), 48_000, 2).unwrap();
    let original_credit = admission.lease_credit().unwrap();
    admission.admit(batch(scope(), 1)).unwrap();
    renderer.render_into(&mut [0.0; 32]);
    assert_eq!(admission.progress().callback_applied_sequence, Some(1));
    for sequence in 2..=17 {
        admission.admit(batch(scope(), sequence)).unwrap();
    }
    let next = admission.begin_panic(scope()).unwrap();
    for callback in 0..4 {
        let before = admission.progress();
        assert!(before.is_valid());
        assert_eq!(before.scope, scope());
        assert_eq!(before.state, LiveState::Panicking);
        assert_eq!(before.worker_accepted_sequence, Some(17));
        assert_eq!(before.callback_applied_sequence, Some(1));
        assert!(before.lease_credit.is_none());
        assert_eq!(measured(|| renderer.render_into(&mut [0.0; 32])), (0, 0));
        // After the fourth callback the raw callback counter is reset for the
        // new generation, but this sole owner has not adopted it yet.
        assert_eq!(
            admission.begin_panic(scope()).unwrap(),
            next,
            "same request must not refresh the pin"
        );
        let after = admission.progress();
        assert!(after.is_valid());
        assert_eq!(after.scope, scope());
        assert_eq!(after.callback_applied_sequence, Some(1));
        assert_eq!(after.worker_accepted_sequence, Some(17));
        assert!(after.lease_credit.is_none());
        if callback < 3 {
            assert!(admission.finish_panic().unwrap().is_none());
        }
    }
    let adopted = admission.finish_panic().unwrap().unwrap();
    assert!(adopted.is_valid());
    assert_eq!(adopted.scope, next);
    assert_eq!(adopted.state, LiveState::Running);
    assert_eq!(adopted.worker_accepted_sequence, None);
    assert_eq!(adopted.callback_applied_sequence, None);
    let credit = adopted.lease_credit.unwrap();
    assert_eq!(credit.scope, next);
    assert_ne!(credit.token, original_credit.token);
    admission.admit(batch(next, 1)).unwrap();
    renderer.render_into(&mut [0.0; 32]);
    assert_eq!(admission.progress().callback_applied_sequence, Some(1));
    drop(renderer);
    drop(admission);
}

#[test]
fn recorded_tap_end_truthfully_resets_new_generation_while_pending_public_progress_remains_old() {
    let (mut admission, mut renderer, mut tap) = recording_live_pair(scope(), 48_000, 1).unwrap();
    admission.admit(batch(scope(), 1)).unwrap();
    renderer.render_into(&mut [0.0; 16]);
    while tap.try_pop().unwrap().is_some() {}
    admission.admit(batch(scope(), 2)).unwrap();
    let next = admission.begin_panic(scope()).unwrap();
    assert_eq!(measured(|| renderer.render_into(&mut [0.0; 16])), (0, 0));
    let mut end = None;
    while let Some(packet) = tap.try_pop().unwrap() {
        if let PerformancePacketKind::End(value) = packet.kind {
            end = Some(value);
        }
    }
    let end = end.expect("complete software callback evidence");
    assert_eq!(end.scope_after, next);
    assert_eq!(end.applied_sequence, None);
    assert!(end.clears.iter().flatten().any(|clear| clear.reason
        == TapClearReason::Panic {
            next_generation: next.panic_generation
        }));
    let pending = admission.progress();
    assert!(pending.is_valid());
    assert_eq!(pending.scope, scope());
    assert_eq!(pending.callback_applied_sequence, Some(1));
    assert_eq!(pending.worker_accepted_sequence, Some(2));
    assert!(pending.lease_credit.is_none());
    let adopted = admission.finish_panic().unwrap().unwrap();
    assert_eq!(adopted.scope, next);
    assert_eq!(adopted.callback_applied_sequence, None);
    assert_eq!(adopted.worker_accepted_sequence, None);
    drop(renderer);
    assert!(tap.control().snapshot().unwrap().producer_destroyed);
    drop(tap);
    drop(admission);
}

#[test]
fn pending_panic_pin_is_never_relabelled_as_a_different_fault_generation() {
    for callback_acknowledged in [false, true] {
        let (mut admission, mut renderer) = live_pair(scope(), 48_000, 1).unwrap();
        admission.admit(batch(scope(), 1)).unwrap();
        renderer.render_into(&mut [0.0; 16]);
        admission.admit(batch(scope(), 2)).unwrap();
        let next = admission.begin_panic(scope()).unwrap();
        if callback_acknowledged {
            renderer.render_into(&mut [0.0; 16]);
        }
        admission.control().fault(LiveFaultKind::Device, None);
        let faulted = admission.progress();
        assert!(faulted.is_valid());
        assert_eq!(faulted.state, LiveState::Faulted);
        assert!(faulted.lease_credit.is_none());
        let expected_scope = if callback_acknowledged { next } else { scope() };
        assert_eq!(faulted.scope, expected_scope);
        assert_eq!(faulted.fault.unwrap().scope, expected_scope);
        assert_eq!(
            faulted.callback_applied_sequence,
            if callback_acknowledged { None } else { Some(1) }
        );
        assert_eq!(
            faulted.worker_accepted_sequence,
            if callback_acknowledged { None } else { Some(2) }
        );
        assert_eq!(admission.finish_panic(), Err(LiveSubmitError::Faulted));
        assert_eq!(admission.scope(), scope());
        assert_eq!(admission.progress().fault, faulted.fault);
        drop(renderer);
        drop(admission);
    }
}
