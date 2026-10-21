//! Device-free bounded live synthesis. Only `LiveRenderer` mutates voices.
//! GUI ticks are retained observations; events apply at the next callback boundary.
use crate::live_types::*;
use crate::performance_tap::{
    self, BatchDisposition, PerformanceTapOwner, PerformanceTapProducer, PerformanceTapRetention,
    TapBegin, TapClear, TapClearReason, TapControl, TapEnd, TapFaultKind, TapFinish, TapInfo,
    TapPcm, TapTransition, TapTransitionKind,
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
const EXPIRED: u64 = 1 << 63;
const TABLE: usize = 2048;

struct FinishPublication {
    request_ready: AtomicBool,
    id_low: AtomicU64,
    id_high: AtomicU64,
    generation: AtomicU64,
    sequence: AtomicU64,
    ack_ready: AtomicBool,
    callback_index: AtomicU64,
    cut_frame: AtomicU64,
    cut_counts: AtomicU32,
}
impl FinishPublication {
    fn new() -> Self {
        Self {
            request_ready: AtomicBool::new(false),
            id_low: AtomicU64::new(0),
            id_high: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            ack_ready: AtomicBool::new(false),
            callback_index: AtomicU64::new(0),
            cut_frame: AtomicU64::new(0),
            cut_counts: AtomicU32::new(0),
        }
    }
    fn request(&self, base: LiveScope) -> Option<LiveFinishFence> {
        if !self.request_ready.load(Ordering::Acquire) {
            return None;
        }
        let mut id = [0u8; 16];
        id[..8].copy_from_slice(&self.id_low.load(Ordering::Relaxed).to_le_bytes());
        id[8..].copy_from_slice(&self.id_high.load(Ordering::Relaxed).to_le_bytes());
        let sequence = self.sequence.load(Ordering::Relaxed);
        Some(LiveFinishFence {
            id: LiveId(id),
            scope: LiveScope {
                panic_generation: self.generation.load(Ordering::Relaxed),
                ..base
            },
            accepted_sequence: (sequence != 0).then_some(sequence),
        })
    }
    fn publish(&self, fence: LiveFinishFence) {
        self.id_low.store(
            u64::from_le_bytes(fence.id.0[..8].try_into().expect("fixed UUID half")),
            Ordering::Relaxed,
        );
        self.id_high.store(
            u64::from_le_bytes(fence.id.0[8..].try_into().expect("fixed UUID half")),
            Ordering::Relaxed,
        );
        self.generation
            .store(fence.scope.panic_generation, Ordering::Relaxed);
        self.sequence
            .store(fence.accepted_sequence.unwrap_or(0), Ordering::Relaxed);
        self.request_ready.store(true, Ordering::Release);
    }
    fn acknowledge(&self, finish: TapFinish) {
        self.callback_index
            .store(finish.callback_index, Ordering::Relaxed);
        self.cut_frame.store(finish.cut_frame, Ordering::Relaxed);
        self.cut_counts.store(
            u32::from(finish.voices_cut) | (u32::from(finish.release_tails_cut) << 16),
            Ordering::Relaxed,
        );
        self.ack_ready.store(true, Ordering::Release);
    }
    fn acknowledgment(&self, base: LiveScope) -> Option<TapFinish> {
        if !self.ack_ready.load(Ordering::Acquire) {
            return None;
        }
        let counts = self.cut_counts.load(Ordering::Relaxed);
        Some(TapFinish {
            fence: self.request(base)?,
            callback_index: self.callback_index.load(Ordering::Relaxed),
            cut_frame: self.cut_frame.load(Ordering::Relaxed),
            voices_cut: (counts & 0xffff) as u16,
            release_tails_cut: (counts >> 16) as u16,
            policy: LiveFinishPolicy::CutAtCompleteCallbackEnd,
        })
    }
}

fn code(kind: LiveFaultKind) -> u32 {
    match kind {
        LiveFaultKind::InvalidBatch => 1,
        LiveFaultKind::Sequence => 2,
        LiveFaultKind::QueueOverflow => 3,
        LiveFaultKind::Polyphony => 4,
        LiveFaultKind::SourceOwnership => 5,
        LiveFaultKind::LeaseExpired => 6,
        LiveFaultKind::CallbackShape => 7,
        LiveFaultKind::Device => 8,
        LiveFaultKind::Clock => 9,
        LiveFaultKind::CounterExhausted => 10,
        LiveFaultKind::CallbackQuiescence => 11,
        LiveFaultKind::RecordingTap => 12,
    }
}
fn kind(code: u32) -> LiveFaultKind {
    match code {
        1 => LiveFaultKind::InvalidBatch,
        2 => LiveFaultKind::Sequence,
        3 => LiveFaultKind::QueueOverflow,
        4 => LiveFaultKind::Polyphony,
        5 => LiveFaultKind::SourceOwnership,
        6 => LiveFaultKind::LeaseExpired,
        7 => LiveFaultKind::CallbackShape,
        8 => LiveFaultKind::Device,
        9 => LiveFaultKind::Clock,
        10 => LiveFaultKind::CounterExhausted,
        12 => LiveFaultKind::RecordingTap,
        _ => LiveFaultKind::CallbackQuiescence,
    }
}
/// Fixed atomics shared by the off-callback admission owner and callback.
/// All counters reject exhaustion; the high lease bit is a permanent expiry latch.
pub struct LiveControl {
    base: LiveScope,
    rate: u32,
    frames: AtomicU64,
    deadline: AtomicU64,
    requested_generation: AtomicU64,
    applied_generation: AtomicU64,
    accepted: AtomicU64,
    applied: AtomicU64,
    active: AtomicU32,
    late: AtomicU64,
    rejected: AtomicU64,
    overflow: AtomicU64,
    peak: AtomicU32,
    fault_code: AtomicU32,
    fault_frame: AtomicU64,
    fault_sequence: AtomicU64,
    fault_generation: AtomicU64,
    render_done: AtomicBool,
    error_done: AtomicBool,
    finish: FinishPublication,
}
impl LiveControl {
    /// Observe only this renderer's immutable Finish publication, with faults
    /// and tap cancellation vetoing even a previously retained acknowledgment.
    pub(crate) fn finish_ack(
        &self,
        tap: &crate::performance_tap::TapControl,
    ) -> Result<Option<TapFinish>, LiveSubmitError> {
        if self.faulted() || tap.status().is_err() {
            return Err(LiveSubmitError::Faulted);
        }
        let ack = self.finish.acknowledgment(self.base);
        if self.faulted() || tap.status().is_err() {
            return Err(LiveSubmitError::Faulted);
        }
        Ok(ack)
    }
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Acquire)
    }
    pub fn render_destroyed(&self) -> bool {
        self.render_done.load(Ordering::Acquire)
    }
    pub fn error_destroyed(&self) -> bool {
        self.error_done.load(Ordering::Acquire)
    }
    pub fn fault(&self, fault: LiveFaultKind, sequence: Option<u64>) {
        self.fault_at(fault, sequence, self.frames());
    }
    fn fault_at(&self, fault: LiveFaultKind, sequence: Option<u64>, frame: u64) {
        if self
            .fault_code
            .compare_exchange(0, u32::MAX, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.fault_frame.store(frame, Ordering::Relaxed);
            self.fault_sequence
                .store(sequence.unwrap_or(0), Ordering::Relaxed);
            self.fault_generation.store(
                self.applied_generation.load(Ordering::Acquire),
                Ordering::Relaxed,
            );
            self.fault_code.store(code(fault), Ordering::Release);
        }
    }
    pub fn first_fault(&self) -> Option<LiveFault> {
        let value = self.fault_code.load(Ordering::Acquire);
        self.fault_from_code(value)
    }
    fn fault_from_code(&self, value: u32) -> Option<LiveFault> {
        if value == 0 || value == u32::MAX {
            return None;
        }
        let sequence = self.fault_sequence.load(Ordering::Relaxed);
        Some(LiveFault {
            scope: LiveScope {
                panic_generation: self.fault_generation.load(Ordering::Relaxed),
                ..self.base
            },
            kind: kind(value),
            frame: self.fault_frame.load(Ordering::Relaxed),
            sequence: (sequence != 0).then_some(sequence),
        })
    }
    fn faulted(&self) -> bool {
        self.fault_code.load(Ordering::Acquire) != 0
    }
    fn renew(&self) -> Result<(), LiveSubmitError> {
        let Some(new) = self
            .frames()
            .checked_add(u64::from(self.rate))
            .filter(|n| *n < EXPIRED)
        else {
            self.fault(LiveFaultKind::CounterExhausted, None);
            return Err(LiveSubmitError::Faulted);
        };
        let mut old = self.deadline.load(Ordering::Acquire);
        for _ in 0..3 {
            if old & EXPIRED != 0 || self.faulted() {
                return Err(LiveSubmitError::Faulted);
            }
            if self.frames() >= old {
                if self
                    .deadline
                    .compare_exchange(old, old | EXPIRED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    self.fault_at(LiveFaultKind::LeaseExpired, None, old);
                    return Err(LiveSubmitError::Faulted);
                }
                old = self.deadline.load(Ordering::Acquire);
                continue;
            }
            match self.deadline.compare_exchange(
                old,
                old.max(new),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(value) => old = value,
            }
        }
        self.fault(LiveFaultKind::Clock, None);
        Err(LiveSubmitError::Faulted)
    }
    fn extend_finish(&self) -> Result<(), LiveSubmitError> {
        let Some(new) = self
            .frames()
            .checked_add((6 * LIVE_MAX_CALLBACK_FRAMES) as u64)
            .filter(|frame| *frame < EXPIRED)
        else {
            self.fault(LiveFaultKind::CounterExhausted, None);
            return Err(LiveSubmitError::Faulted);
        };
        let mut old = self.deadline.load(Ordering::Acquire);
        for _ in 0..3 {
            if old & EXPIRED != 0 || self.faulted() {
                return Err(LiveSubmitError::Faulted);
            }
            if self.frames() >= old {
                if self
                    .deadline
                    .compare_exchange(old, old | EXPIRED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    self.fault_at(LiveFaultKind::LeaseExpired, None, old);
                    return Err(LiveSubmitError::Faulted);
                }
                old = self.deadline.load(Ordering::Acquire);
                continue;
            }
            match self.deadline.compare_exchange(
                old,
                old.max(new),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return if self.faulted() {
                        Err(LiveSubmitError::Faulted)
                    } else {
                        Ok(())
                    };
                }
                Err(value) => old = value,
            }
        }
        self.fault(LiveFaultKind::Clock, None);
        Err(LiveSubmitError::Faulted)
    }
    fn audible_at(&self, frame: u64) -> bool {
        // Bounded CAS attempts. Pathological repeated contention fails closed.
        for _ in 0..4 {
            let deadline = self.deadline.load(Ordering::Acquire);
            if deadline & EXPIRED != 0 {
                return false;
            }
            if frame < deadline {
                return !self.faulted();
            }
            if self
                .deadline
                .compare_exchange(
                    deadline,
                    deadline | EXPIRED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                self.fault_at(LiveFaultKind::LeaseExpired, None, deadline);
                return false;
            }
        }
        self.fault(LiveFaultKind::Clock, None);
        false
    }
}
/// Constructed off callback. Its Drop is a native error-closure lifetime proof.
pub struct LiveErrorGuard {
    control: Arc<LiveControl>,
}
impl LiveErrorGuard {
    pub fn new(control: Arc<LiveControl>) -> Self {
        Self { control }
    }
    pub fn fault(&self) {
        self.control.fault(LiveFaultKind::Device, None);
    }
}
impl Drop for LiveErrorGuard {
    fn drop(&mut self) {
        self.control.error_done.store(true, Ordering::Release);
    }
}
struct Tables {
    sine: [f32; TABLE],
    increments: [f64; 128],
}
#[derive(Clone, Copy, Default)]
struct Source {
    note: u64,
    voice: u64,
    ticks: Option<u64>,
    sustain: bool,
}
#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    source: u8,
    id: u64,
    patch: LivePatch,
    phase: f64,
    increment: f64,
    amplitude: f32,
    envelope: f32,
    held: bool,
    sustained: bool,
    releasing: bool,
    release_step: f32,
    auto_release: Option<u64>,
    auto_release_velocity: u16,
    pending_release_velocity: u16,
    noise: u32,
}
const EMPTY: Voice = Voice {
    active: false,
    source: 0,
    id: 0,
    patch: LivePatch::Keys,
    phase: 0.0,
    increment: 0.0,
    amplitude: 0.0,
    envelope: 0.0,
    held: false,
    sustained: false,
    releasing: false,
    release_step: 0.0,
    auto_release: None,
    auto_release_velocity: 0,
    pending_release_velocity: 0,
    noise: 1,
};
fn release(voice: &mut Voice, velocity: u16, rate: u32) {
    voice.held = false;
    voice.sustained = false;
    voice.releasing = true;
    let seconds = if voice.patch == LivePatch::Pad {
        0.30
    } else {
        0.035 + 0.12 * (1.0 - f32::from(velocity) / 65535.0)
    };
    voice.release_step = voice.envelope.max(0.001) / (seconds * rate as f32).max(1.0);
}
/// Off-callback single producer; retains all backing owners past callback teardown.
pub struct LiveAdmission {
    scope: LiveScope,
    rate: u32,
    producer: Producer<LiveBatch>,
    control: Arc<LiveControl>,
    tables: Arc<Tables>,
    token: Option<LeaseCredit>,
    accepted: Option<u64>,
    source_ticks: [Option<u64>; LIVE_MAX_SOURCES],
    pending_panic: Option<LiveScope>,
    // Retains the recording ring off callback through renderer destruction.
    _recording_retention: Option<PerformanceTapRetention>,
    recording_control: Option<Arc<TapControl>>,
    finish: Option<LiveFinishFence>,
}
struct RecordingState {
    producer: PerformanceTapProducer,
    clears: [Option<TapClear>; 2],
    transitions: [Option<TapTransition>; 128],
    transition_len: u16,
    fault_cleared: bool,
    callback_index: u64,
}
impl RecordingState {
    fn reset_callback(&mut self) {
        self.clears = [None; 2];
        self.transitions = [None; 128];
        self.transition_len = 0;
    }
    fn transition(&mut self, transition: TapTransition) -> bool {
        let index = usize::from(self.transition_len);
        if index == self.transitions.len() {
            self.producer
                .fault(TapFaultKind::MalformedPacket, transition.frame);
            return false;
        }
        self.transitions[index] = Some(transition);
        self.transition_len += 1;
        true
    }
    fn clear(&mut self, clear: TapClear) -> bool {
        if let Some(slot) = self.clears.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(clear);
            true
        } else {
            self.producer
                .fault(TapFaultKind::MalformedPacket, clear.frame);
            false
        }
    }
}
/// Callback owner. Render operations allocate/deallocate nothing and never lock.
pub struct LiveRenderer {
    scope: LiveScope,
    rate: u32,
    channels: usize,
    consumer: Option<Consumer<LiveBatch>>,
    control: Arc<LiveControl>,
    tables: Arc<Tables>,
    sources: [Source; LIVE_MAX_SOURCES],
    voices: [Voice; LIVE_VOICES],
    frame: u64,
    late: u64,
    recording: Option<RecordingState>,
    finished: bool,
}
pub fn live_pair(
    scope: LiveScope,
    rate: u32,
    channels: u16,
) -> Result<(LiveAdmission, LiveRenderer), String> {
    if !scope.is_valid() || !(8000..=768000).contains(&rate) || !(1..=8).contains(&channels) {
        return Err("Invalid live renderer configuration".into());
    }
    let tables = Arc::new(Tables {
        sine: std::array::from_fn(|i| (std::f32::consts::TAU * i as f32 / TABLE as f32).sin()),
        increments: std::array::from_fn(|key| {
            440.0 * 2f64.powf((key as f64 - 69.0) / 12.0) / f64::from(rate)
        }),
    });
    let control = Arc::new(LiveControl {
        base: scope,
        rate,
        frames: AtomicU64::new(0),
        deadline: AtomicU64::new(u64::from(rate)),
        requested_generation: AtomicU64::new(scope.panic_generation),
        applied_generation: AtomicU64::new(scope.panic_generation),
        accepted: AtomicU64::new(0),
        applied: AtomicU64::new(0),
        active: AtomicU32::new(0),
        late: AtomicU64::new(0),
        rejected: AtomicU64::new(0),
        overflow: AtomicU64::new(0),
        peak: AtomicU32::new(0),
        fault_code: AtomicU32::new(0),
        fault_frame: AtomicU64::new(0),
        fault_sequence: AtomicU64::new(0),
        fault_generation: AtomicU64::new(scope.panic_generation),
        render_done: AtomicBool::new(false),
        error_done: AtomicBool::new(false),
        finish: FinishPublication::new(),
    });
    let (producer, consumer) = RingBuffer::new(LIVE_QUEUE_BATCHES);
    Ok((
        LiveAdmission {
            scope,
            rate,
            producer,
            control: control.clone(),
            tables: tables.clone(),
            token: Some(LeaseCredit {
                scope,
                token: LiveId::random(),
                issued_after_sequence: None,
            }),
            accepted: None,
            source_ticks: [None; LIVE_MAX_SOURCES],
            pending_panic: None,
            _recording_retention: None,
            recording_control: None,
            finish: None,
        },
        LiveRenderer {
            scope,
            rate,
            channels: usize::from(channels),
            consumer: Some(consumer),
            control,
            tables,
            sources: [Source::default(); LIVE_MAX_SOURCES],
            voices: [EMPTY; LIVE_VOICES],
            frame: 0,
            late: 0,
            recording: None,
            finished: false,
        },
    ))
}
/// Library-level recorded-live render bus. No device, journal, worker command,
/// take, or project mutation is created here. Keep admission until both native
/// callback closures are proven gone, exactly as for ordinary live ownership.
pub fn recording_live_pair(
    scope: LiveScope,
    rate: u32,
    channels: u16,
) -> Result<(LiveAdmission, LiveRenderer, PerformanceTapOwner), String> {
    let (mut admission, mut renderer) = live_pair(scope, rate, channels)?;
    let (producer, owner, retention) = performance_tap::tap_pair(TapInfo {
        scope,
        sample_rate: rate,
        channels,
    })
    .map_err(|error| format!("Cannot prepare performance transfer: {error:?}"))?;
    admission._recording_retention = Some(retention);
    admission.recording_control = Some(owner.control());
    renderer.recording = Some(RecordingState {
        producer,
        clears: [None; 2],
        transitions: [None; 128],
        transition_len: 0,
        fault_cleared: false,
        callback_index: 0,
    });
    Ok((admission, renderer, owner))
}
impl LiveAdmission {
    pub fn control(&self) -> Arc<LiveControl> {
        self.control.clone()
    }
    pub fn scope(&self) -> LiveScope {
        self.scope
    }
    pub fn lease_credit(&self) -> Option<LeaseCredit> {
        if self.control.faulted() || self.finish.is_some() {
            None
        } else {
            self.token
        }
    }
    pub fn progress(&self) -> LiveProgress {
        // Acquire the one-shot ACK BEFORE any mutable callback observations.
        // Its immutable cut/fence supplies the completed snapshot: observing
        // fresh ACK after previously sampled voice counts could mix two eras.
        let acknowledgment = self.control.finish.acknowledgment(self.control.base);
        let applied = self.control.applied.load(Ordering::Acquire);
        let voice_counts = self.control.active.load(Ordering::Acquire);
        let fault_code = self.control.fault_code.load(Ordering::Acquire);
        let fault = self.control.fault_from_code(fault_code);
        let scope = fault.map_or(self.scope, |f| f.scope);
        let same_scope = scope == self.scope;
        let cut = acknowledgment.filter(|ack| ack.fence.scope == scope);
        LiveProgress {
            scope,
            state: if fault_code == u32::MAX {
                LiveState::Panicking
            } else if fault_code != 0 {
                LiveState::Faulted
            } else if self.pending_panic.is_some() {
                LiveState::Panicking
            } else if cut.is_some() {
                LiveState::FinishedCut
            } else if self.finish.is_some() {
                LiveState::Finishing
            } else {
                LiveState::Running
            },
            worker_accepted_sequence: if same_scope { self.accepted } else { None },
            callback_applied_sequence: if same_scope {
                cut.map_or((applied != 0).then_some(applied), |ack| {
                    ack.fence.accepted_sequence
                })
            } else {
                None
            },
            active_voices: if cut.is_some() {
                0
            } else {
                (voice_counts & 0xffff) as u16
            },
            release_tail_voices: if cut.is_some() {
                0
            } else {
                (voice_counts >> 16) as u16
            },
            late_releases: self.control.late.load(Ordering::Acquire),
            rejected_batches: self.control.rejected.load(Ordering::Acquire),
            overflow_batches: self.control.overflow.load(Ordering::Acquire),
            frames: cut.map_or_else(|| self.control.frames(), |ack| ack.cut_frame),
            peak: f32::from_bits(self.control.peak.load(Ordering::Acquire)),
            lease_deadline_frame: self.control.deadline.load(Ordering::Acquire) & !EXPIRED,
            lease_credit: if fault_code == 0
                && self.pending_panic.is_none()
                && self.finish.is_none()
            {
                self.token
            } else {
                None
            },
            fault,
        }
    }
    fn reject(
        &self,
        error: LiveSubmitError,
        fault: LiveFaultKind,
        sequence: u64,
    ) -> LiveSubmitError {
        self.control.rejected.fetch_add(1, Ordering::Relaxed);
        if error == LiveSubmitError::Saturated {
            self.control.overflow.fetch_add(1, Ordering::Relaxed);
        }
        self.control.fault(fault, Some(sequence));
        error
    }
    pub fn admit(&mut self, batch: LiveBatch) -> Result<LiveProgress, LiveSubmitError> {
        if batch.scope != self.scope {
            return Err(LiveSubmitError::StaleScope);
        }
        if self.control.faulted() {
            return Err(LiveSubmitError::Faulted);
        }
        if self.finish.is_some() {
            return Err(LiveSubmitError::FinishPending);
        }
        if self.pending_panic.is_some() {
            return Err(LiveSubmitError::PanicPending);
        }
        batch
            .validate_shape(self.rate)
            .map_err(|e| self.reject(e, LiveFaultKind::InvalidBatch, batch.sequence))?;
        if self.accepted.map_or(Some(1), |n| n.checked_add(1)) != Some(batch.sequence) {
            return Err(self.reject(
                LiveSubmitError::Invalid,
                LiveFaultKind::Sequence,
                batch.sequence,
            ));
        }
        let mut ticks = self.source_ticks;
        for event in batch.events[..usize::from(batch.len)].iter().flatten() {
            if event.source > 4 || event.connection_generation != 1 {
                return Err(self.reject(
                    LiveSubmitError::Invalid,
                    LiveFaultKind::SourceOwnership,
                    batch.sequence,
                ));
            }
            let index = usize::from(event.source - 1);
            if ticks[index].is_some_and(|old| event.observed_ticks < old) {
                return Err(self.reject(
                    LiveSubmitError::Invalid,
                    LiveFaultKind::Clock,
                    batch.sequence,
                ));
            }
            ticks[index] = Some(event.observed_ticks);
        }
        if self.producer.is_full() {
            return Err(self.reject(
                LiveSubmitError::Saturated,
                LiveFaultKind::QueueOverflow,
                batch.sequence,
            ));
        }
        let consumes = batch
            .lease_token
            .is_some_and(|token| self.token.is_some_and(|credit| credit.token == token));
        if consumes {
            self.control.renew()?;
        }
        if self.producer.push(batch).is_err() {
            return Err(self.reject(
                LiveSubmitError::Saturated,
                LiveFaultKind::QueueOverflow,
                batch.sequence,
            ));
        }
        self.source_ticks = ticks;
        self.accepted = Some(batch.sequence);
        self.control
            .accepted
            .store(batch.sequence, Ordering::Release);
        if consumes {
            self.token = Some(LeaseCredit {
                scope: self.scope,
                token: LiveId::random(),
                issued_after_sequence: Some(batch.sequence),
            });
        }
        Ok(self.progress())
    }
    pub fn begin_panic(&mut self, scope: LiveScope) -> Result<LiveScope, LiveSubmitError> {
        if scope != self.scope {
            return Err(LiveSubmitError::StaleScope);
        }
        if self.control.faulted() {
            return Err(LiveSubmitError::Faulted);
        }
        if self.finish.is_some() {
            return Err(LiveSubmitError::FinishPending);
        }
        if let Some(next) = self.pending_panic {
            return Ok(next);
        }
        let generation = scope.panic_generation.checked_add(1).ok_or_else(|| {
            self.control.fault(LiveFaultKind::CounterExhausted, None);
            LiveSubmitError::Faulted
        })?;
        let next = LiveScope {
            panic_generation: generation,
            ..scope
        };
        self.pending_panic = Some(next);
        self.token = None;
        self.control
            .requested_generation
            .store(next.panic_generation, Ordering::Release);
        Ok(next)
    }
    pub fn finish_panic(&mut self) -> Result<Option<LiveProgress>, LiveSubmitError> {
        if self.control.faulted() {
            return Err(LiveSubmitError::Faulted);
        }
        if self.finish.is_some() {
            return Err(LiveSubmitError::FinishPending);
        }
        let Some(next) = self.pending_panic else {
            return Ok(None);
        };
        if self.control.applied_generation.load(Ordering::Acquire) != next.panic_generation {
            return Ok(None);
        }
        self.scope = next;
        self.pending_panic = None;
        self.accepted = None;
        self.control.accepted.store(0, Ordering::Release);
        self.token = Some(LeaseCredit {
            scope: next,
            token: LiveId::random(),
            issued_after_sequence: None,
        });
        Ok(Some(self.progress()))
    }
    fn finish_healthy(&self) -> Result<(), LiveSubmitError> {
        if self.control.faulted() {
            return Err(LiveSubmitError::Faulted);
        }
        let tap = self
            .recording_control
            .as_ref()
            .ok_or(LiveSubmitError::NotLive)?;
        if tap.status().is_err() {
            self.control.fault(LiveFaultKind::RecordingTap, None);
            return Err(LiveSubmitError::Faulted);
        }
        Ok(())
    }
    pub fn begin_finish(
        &mut self,
        scope: LiveScope,
        id: LiveId,
    ) -> Result<LiveFinishFence, LiveSubmitError> {
        if scope != self.scope {
            return Err(LiveSubmitError::StaleScope);
        }
        if !id.is_valid() {
            return Err(LiveSubmitError::Invalid);
        }
        self.finish_healthy()?;
        if let Some(fence) = self.finish {
            return if fence.scope == scope && fence.id == id {
                Ok(fence)
            } else {
                Err(LiveSubmitError::FinishPending)
            };
        }
        if self.pending_panic.is_some() {
            return Err(LiveSubmitError::PanicPending);
        }
        self.control.extend_finish()?;
        self.finish_healthy()?;
        let fence = LiveFinishFence {
            id,
            scope,
            accepted_sequence: self.accepted,
        };
        self.finish = Some(fence);
        self.token = None;
        self.control.finish.publish(fence);
        Ok(fence)
    }
    /// A retained callback ACK is not normal-seal authority. A later device or
    /// tap fault/cancellation vetoes this healthy result even after the cut.
    pub fn finish_ack(&self) -> Result<Option<TapFinish>, LiveSubmitError> {
        self.finish_healthy()?;
        let Some(fence) = self.finish else {
            return Ok(None);
        };
        let acknowledgment = self.control.finish.acknowledgment(self.control.base);
        if acknowledgment.is_some_and(|ack| ack.fence != fence) {
            self.control.fault(LiveFaultKind::Sequence, None);
            return Err(LiveSubmitError::Faulted);
        }
        self.finish_healthy()?;
        Ok(acknowledgment)
    }
    pub fn backing_owner_count(&self) -> usize {
        Arc::strong_count(&self.tables)
    }
}
impl LiveRenderer {
    fn capture_batch(&mut self, batch: LiveBatch, disposition: BatchDisposition) -> bool {
        if self.control.finish.request_ready.load(Ordering::Acquire)
            && disposition != BatchDisposition::Applied
        {
            self.control
                .fault(LiveFaultKind::Sequence, Some(batch.sequence));
        }
        if self
            .recording
            .as_mut()
            .is_some_and(|recording| recording.producer.push_batch(batch, disposition).is_err())
        {
            self.control.fault_at(
                LiveFaultKind::RecordingTap,
                Some(batch.sequence),
                self.frame,
            );
            self.clear_for_fault();
            return false;
        }
        true
    }
    fn capture_clear(&mut self, reason: TapClearReason) {
        if self.recording.as_mut().is_some_and(|recording| {
            !recording.clear(TapClear {
                scope: self.scope,
                frame: self.frame,
                reason,
            })
        }) {
            self.control
                .fault_at(LiveFaultKind::RecordingTap, None, self.frame);
        }
    }
    fn clear_for_fault(&mut self) {
        if let Some(recording) = &mut self.recording
            && !recording.fault_cleared
        {
            recording.fault_cleared = true;
            let reason = self
                .control
                .first_fault()
                .map_or(TapClearReason::FaultPending, |fault| {
                    TapClearReason::Fault { kind: fault.kind }
                });
            self.capture_clear(reason);
        }
        self.clear();
    }
    fn clear(&mut self) {
        self.voices = [EMPTY; LIVE_VOICES];
        self.control.active.store(0, Ordering::Release);
        self.control.peak.store(0, Ordering::Release);
        for s in &mut self.sources {
            s.sustain = false;
        }
    }
    fn apply(&mut self, batch: LiveBatch) -> Result<(), LiveFaultKind> {
        batch
            .validate_shape(self.rate)
            .map_err(|_| LiveFaultKind::InvalidBatch)?;
        // Entire fixed transaction is staged. No earlier event can sound if a later one fails.
        let mut sources = self.sources;
        let mut voices = self.voices;
        let mut late = self.late;
        for e in batch.events[..usize::from(batch.len)].iter().flatten() {
            let si = usize::from(e.source - 1);
            let source = &mut sources[si];
            if e.source > 4 || e.connection_generation != 1 {
                return Err(LiveFaultKind::SourceOwnership);
            }
            if source.ticks.is_some_and(|ticks| e.observed_ticks < ticks) {
                return Err(LiveFaultKind::Clock);
            }
            source.ticks = Some(e.observed_ticks);
            match e.event {
                LiveEventKind::NoteOn {
                    note,
                    voice,
                    key,
                    velocity16,
                    patch,
                }
                | LiveEventKind::Pulse {
                    note,
                    voice,
                    key,
                    velocity16,
                    patch,
                    ..
                } => {
                    if source.note.checked_add(1) != Some(note)
                        || source.voice.checked_add(1) != Some(voice)
                    {
                        return Err(LiveFaultKind::SourceOwnership);
                    }
                    let Some(slot) = voices.iter_mut().find(|v| !v.active) else {
                        return Err(LiveFaultKind::Polyphony);
                    };
                    let duration = match e.event {
                        LiveEventKind::Pulse {
                            duration_frames, ..
                        } => Some(u64::from(duration_frames)),
                        _ if patch == LivePatch::Drums => Some(u64::from(self.rate) / 5),
                        _ => None,
                    };
                    let auto_release = duration
                        .map(|n| {
                            self.frame
                                .checked_add(n)
                                .ok_or(LiveFaultKind::CounterExhausted)
                        })
                        .transpose()?;
                    *slot = Voice {
                        active: true,
                        source: e.source,
                        id: voice,
                        patch,
                        phase: 0.0,
                        increment: self.tables.increments[usize::from(key)],
                        amplitude: f32::from(velocity16) / 65535.0 * 0.16,
                        envelope: 0.0,
                        held: true,
                        sustained: false,
                        releasing: false,
                        release_step: 0.0,
                        auto_release,
                        auto_release_velocity: match e.event {
                            LiveEventKind::Pulse {
                                release_velocity16, ..
                            } => release_velocity16,
                            _ => 0,
                        },
                        pending_release_velocity: 0,
                        noise: (voice as u32)
                            .wrapping_mul(747796405)
                            .wrapping_add(u32::from(key) + 1),
                    };
                    source.note = note;
                    source.voice = voice;
                }
                LiveEventKind::NoteOff {
                    voice,
                    release_velocity16,
                } => {
                    if voice > source.voice {
                        return Err(LiveFaultKind::SourceOwnership);
                    }
                    if let Some(v) = voices
                        .iter_mut()
                        .find(|v| v.active && v.source == e.source && v.id == voice)
                    {
                        if v.held {
                            v.held = false;
                            if source.sustain && v.auto_release.is_none() {
                                v.sustained = true;
                                v.pending_release_velocity = release_velocity16;
                            } else {
                                release(v, release_velocity16, self.rate);
                            }
                        }
                    } else {
                        late = late.checked_add(1).ok_or(LiveFaultKind::CounterExhausted)?;
                    }
                }
                LiveEventKind::Sustain { down } => {
                    source.sustain = down;
                    if !down {
                        for v in voices
                            .iter_mut()
                            .filter(|v| v.active && v.source == e.source && v.sustained)
                        {
                            release(v, v.pending_release_velocity, self.rate);
                        }
                    }
                }
            }
        }
        self.sources = sources;
        self.voices = voices;
        self.late = late;
        Ok(())
    }
    fn boundary(&mut self) {
        let requested = self.control.requested_generation.load(Ordering::Acquire);
        if requested != self.scope.panic_generation {
            self.capture_clear(TapClearReason::Panic {
                next_generation: requested,
            });
            self.clear();
            for _ in 0..LIVE_CALLBACK_PACKET_BUDGET {
                let Ok(batch) = self.consumer.as_mut().expect("live consumer").pop() else {
                    break;
                };
                if !self.capture_batch(batch, BatchDisposition::DiscardedPanic) {
                    break;
                }
            }
            if !self.control.faulted() && self.consumer.as_ref().expect("live consumer").is_empty()
            {
                for source in &mut self.sources {
                    let ticks = source.ticks;
                    *source = Source {
                        ticks,
                        ..Source::default()
                    };
                }
                self.scope.panic_generation = requested;
                self.control.applied.store(0, Ordering::Release);
                self.control
                    .applied_generation
                    .store(requested, Ordering::Release);
            }
            return;
        }
        if self.control.faulted() {
            self.clear_for_fault();
            // Recover pending originals without applying them. This retains the
            // same four-packet total callback bound; an unpublished fault cause
            // never gets guessed and does not consume its pending batches.
            if self.recording.is_some()
                && let Some(fault) = self.control.first_fault()
            {
                for _ in 0..LIVE_CALLBACK_PACKET_BUDGET {
                    let Ok(batch) = self.consumer.as_mut().expect("live consumer").pop() else {
                        break;
                    };
                    if !self.capture_batch(batch, BatchDisposition::Rejected { fault: fault.kind })
                    {
                        break;
                    }
                }
            }
            return;
        }
        for _ in 0..LIVE_CALLBACK_PACKET_BUDGET {
            let Ok(batch) = self.consumer.as_mut().expect("live consumer").pop() else {
                break;
            };
            if batch.scope != self.scope {
                if !self.capture_batch(batch, BatchDisposition::DiscardedStale) {
                    break;
                }
                continue;
            }
            if let Err(fault) = self.apply(batch) {
                self.control.fault(fault, Some(batch.sequence));
                let _ = self.capture_batch(batch, BatchDisposition::Rejected { fault });
                self.clear_for_fault();
                break;
            }
            if !self.capture_batch(batch, BatchDisposition::Applied) {
                break;
            }
            self.control
                .applied
                .store(batch.sequence, Ordering::Release);
        }
    }
    fn sample(&mut self) -> f32 {
        if !self.control.audible_at(self.frame) {
            self.clear_for_fault();
            return 0.0;
        }
        let mut mix = 0.0;
        let mut capture_fault = false;
        let recording = &mut self.recording;
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            if voice.auto_release.is_some_and(|end| self.frame >= end) && !voice.releasing {
                // Pulse release velocity is retained in the packet; automatic release
                // must not rely on a future UI timer or sustain release.
                release(voice, voice.auto_release_velocity, self.rate);
                if recording.as_mut().is_some_and(|recording| {
                    !recording.transition(TapTransition {
                        scope: self.scope,
                        source: voice.source,
                        voice: voice.id,
                        frame: self.frame,
                        kind: TapTransitionKind::AutoRelease {
                            release_velocity16: voice.auto_release_velocity,
                        },
                    })
                }) {
                    capture_fault = true;
                    break;
                }
            }
            if voice.releasing {
                voice.envelope -= voice.release_step;
                if voice.envelope <= 0.0 {
                    voice.active = false;
                    if recording.as_mut().is_some_and(|recording| {
                        !recording.transition(TapTransition {
                            scope: self.scope,
                            source: voice.source,
                            voice: voice.id,
                            frame: self.frame,
                            kind: TapTransitionKind::VoiceEnded,
                        })
                    }) {
                        capture_fault = true;
                        break;
                    }
                    continue;
                }
            } else {
                let attack = if voice.patch == LivePatch::Pad {
                    0.04
                } else {
                    0.003
                };
                voice.envelope = (voice.envelope + 1.0 / (attack * self.rate as f32)).min(1.0);
            }
            let i = (voice.phase * TABLE as f64) as usize % TABLE;
            let sine = self.tables.sine[i];
            let wave = match voice.patch {
                LivePatch::Keys => sine * 0.8 + self.tables.sine[(i * 2) % TABLE] * 0.2,
                LivePatch::Bass => sine * 0.8 + (voice.phase as f32 * 2.0 - 1.0) * 0.2,
                LivePatch::Pad => sine * 0.65 + self.tables.sine[(i * 3) % TABLE] * 0.15,
                LivePatch::Drums => {
                    voice.noise ^= voice.noise << 13;
                    voice.noise ^= voice.noise >> 17;
                    voice.noise ^= voice.noise << 5;
                    sine * 0.5 + (voice.noise as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.5
                }
            };
            mix += wave * voice.envelope * voice.amplitude;
            voice.phase += voice.increment;
            if voice.phase >= 1.0 {
                voice.phase -= voice.phase.floor();
            }
        }
        if capture_fault {
            self.control
                .fault_at(LiveFaultKind::RecordingTap, None, self.frame);
            self.clear_for_fault();
            return 0.0;
        }
        mix.clamp(-1.0, 1.0)
    }
    fn render<T>(&mut self, out: &mut [T], convert: impl Fn(f32) -> T) {
        let frames = out.len() / self.channels;
        let end = if self.finished {
            // Silence after a cut has no new source-frame extent. Shape still
            // validates below, without overflowing a frozen high origin.
            Some(self.frame)
        } else {
            self.frame
                .checked_add(frames as u64)
                .filter(|end| *end < EXPIRED)
        };
        if out.is_empty()
            || !out.len().is_multiple_of(self.channels)
            || frames > LIVE_MAX_CALLBACK_FRAMES
            || end.is_none()
        {
            let fault = if end.is_none() {
                LiveFaultKind::CounterExhausted
            } else {
                LiveFaultKind::CallbackShape
            };
            self.control.fault(fault, None);
            if let Some(recording) = &self.recording {
                recording
                    .producer
                    .fault(TapFaultKind::MalformedPacket, self.frame);
            }
            self.clear();
            for sample in out {
                *sample = convert(0.0);
            }
            return;
        }
        let end = end.expect("validated whole callback extent");
        if self.finished {
            for sample in out {
                *sample = convert(0.0);
            }
            self.control.peak.store(0, Ordering::Release);
            return;
        }
        // Reserve evidence for the ENTIRE invocation before applying any queued
        // event or sounding a new voice. A broken tap permanently fails closed.
        if let Some(recording) = &mut self.recording {
            recording.reset_callback();
            match recording.producer.begin_callback(TapBegin {
                scope: self.scope,
                start_frame: self.frame,
                frames: frames as u16,
                sample_rate: self.rate,
                channels: self.channels as u16,
            }) {
                Ok(index) => recording.callback_index = index,
                Err(_) => {
                    self.control
                        .fault_at(LiveFaultKind::RecordingTap, None, self.frame);
                    self.clear();
                    for sample in out {
                        *sample = convert(0.0);
                    }
                    self.frame = end;
                    self.control.frames.store(end, Ordering::Release);
                    return;
                }
            }
        }
        // A later publication belongs to the next callback, never this earlier
        // boundary. All accepted originals still drain under the four limit.
        let finish_request = self.control.finish.request(self.control.base);
        self.boundary();
        let finish_candidate = finish_request.filter(|fence| {
            if fence.scope != self.scope {
                self.control.fault(LiveFaultKind::Sequence, None);
                return false;
            }
            if !self.consumer.as_ref().expect("live consumer").is_empty() {
                return false;
            }
            let applied = self.control.applied.load(Ordering::Acquire);
            if fence.accepted_sequence != (applied != 0).then_some(applied) {
                self.control.fault(LiveFaultKind::Sequence, None);
                return false;
            }
            !self.control.faulted()
        });
        let mut peak = 0.0f32;
        let mut interrupted = false;
        // Fixed256-frame logical-F32 packets precede native device conversion.
        // The single boundary above retains the four-batch WHOLE invocation cap.
        for block in out.chunks_mut(performance_tap::TAP_PCM_FRAMES * self.channels) {
            if interrupted {
                for sample in block {
                    *sample = convert(0.0);
                }
                continue;
            }
            let mut pcm = TapPcm {
                start_frame: self.frame,
                frames: (block.len() / self.channels) as u16,
                channels: self.channels as u16,
                samples: [0.0; performance_tap::TAP_PCM_FRAMES * performance_tap::TAP_CHANNELS],
            };
            for (index, frame) in block.chunks_mut(self.channels).enumerate() {
                let value = self.sample();
                peak = peak.max(value.abs());
                for (channel, dst) in frame.iter_mut().enumerate() {
                    *dst = convert(value);
                    if self.recording.is_some() {
                        pcm.samples[index * self.channels + channel] = value;
                    }
                }
                self.frame += 1; // Whole invocation checked before any event application.
                if !self.control.audible_at(self.frame) {
                    self.clear_for_fault();
                }
                self.control.frames.store(self.frame, Ordering::Release);
            }
            if self
                .recording
                .as_mut()
                .is_some_and(|recording| recording.producer.push_pcm(pcm).is_err())
            {
                self.control
                    .fault_at(LiveFaultKind::RecordingTap, None, self.frame);
                self.clear_for_fault();
                interrupted = true;
            }
        }
        if interrupted {
            // The supplied native buffer still advances, but this suffix has no
            // complete recording group. Never turn a partial prefix into End.
            self.frame = end;
            self.control.frames.store(end, Ordering::Release);
        }
        let active = self.voices.iter().filter(|v| v.active).count() as u32;
        let tails = self
            .voices
            .iter()
            .filter(|v| v.active && v.releasing)
            .count() as u32;
        let mut finish_packet = None;
        if !interrupted
            && !self.control.faulted()
            && let Some(fence) = finish_candidate
            && self
                .recording
                .as_ref()
                .is_some_and(|recording| recording.producer.status().is_ok())
        {
            let finish = TapFinish {
                fence,
                callback_index: self
                    .recording
                    .as_ref()
                    .expect("recorded finish")
                    .callback_index,
                cut_frame: self.frame,
                voices_cut: active as u16,
                release_tails_cut: tails as u16,
                policy: LiveFinishPolicy::CutAtCompleteCallbackEnd,
            };
            // This aggregate hard cut is not a human NoteOff or voice transition.
            self.clear();
            self.finished = true;
            if self
                .recording
                .as_mut()
                .expect("recorded finish")
                .producer
                .push_finish(finish)
                .is_ok()
            {
                finish_packet = Some(finish);
            } else {
                self.control
                    .fault_at(LiveFaultKind::RecordingTap, None, self.frame);
            }
        }
        if !interrupted && let Some(recording) = &mut self.recording {
            let applied = self.control.applied.load(Ordering::Acquire);
            let evidence = TapEnd {
                end_frame: self.frame,
                scope_after: self.scope,
                applied_sequence: (applied != 0).then_some(applied),
                active_voices: if self.finished { 0 } else { active as u16 },
                release_tail_voices: if self.finished { 0 } else { tails as u16 },
                clears: recording.clears,
                transitions: recording.transitions,
                transition_len: recording.transition_len,
                first_fault: self.control.first_fault(),
            };
            if recording.producer.end_callback(evidence).is_err() {
                self.control
                    .fault_at(LiveFaultKind::RecordingTap, None, self.frame);
                self.clear_for_fault();
            } else if let Some(finish) = finish_packet {
                // End is retained before the separate ACK publication. A fault
                // or receiver loss can still veto sealing after this proof.
                if !self.control.faulted() && recording.producer.status().is_ok() {
                    self.control.finish.acknowledge(finish);
                } else {
                    self.control
                        .fault_at(LiveFaultKind::RecordingTap, None, self.frame);
                }
            }
        }
        let active = self.voices.iter().filter(|v| v.active).count() as u32;
        let tails = self
            .voices
            .iter()
            .filter(|v| v.active && v.releasing)
            .count() as u32;
        self.control
            .active
            .store(active | (tails << 16), Ordering::Release);
        self.control.late.store(self.late, Ordering::Release);
        self.control.peak.store(
            if self.control.faulted() {
                0.0f32.to_bits()
            } else {
                peak.to_bits()
            },
            Ordering::Release,
        );
    }
    pub fn render_into(&mut self, out: &mut [f32]) {
        self.render(out, |v| v);
    }
    pub fn render_i16(&mut self, out: &mut [i16]) {
        self.render(out, |v| (v * 32767.0).round() as i16);
    }
    pub fn render_u16(&mut self, out: &mut [u16]) {
        self.render(out, |v| ((v * 32767.0) + 32768.0).round() as u16);
    }
}
impl Drop for LiveRenderer {
    fn drop(&mut self) {
        // Drop the WHOLE wrapper, including its Arc fields, before acknowledging
        // renderer destruction to the off-callback backing owner.
        drop(self.recording.take());
        drop(self.consumer.take());
        self.control.render_done.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (LiveAdmission, LiveRenderer) {
        live_pair(
            LiveScope {
                worker_generation: LiveId([1; 16]),
                session_id: LiveId([2; 16]),
                panic_generation: 1,
            },
            8000,
            1,
        )
        .unwrap()
    }
    fn note(scope: LiveScope, sequence: u64, voice: u64) -> LiveBatch {
        let mut b = LiveBatch::empty(scope, sequence, None);
        b.try_push(LiveEvent {
            source: 1,
            connection_generation: 1,
            observed_ticks: sequence,
            event: LiveEventKind::NoteOn {
                note: voice,
                voice,
                key: 69,
                velocity16: u16::MAX,
                patch: LivePatch::Keys,
            },
        })
        .unwrap();
        b
    }
    #[test]
    fn finish_publication_during_render_belongs_to_the_next_callback() {
        use crate::performance_tap::PerformancePacketKind;
        use std::cell::Cell;
        let scope = pair().0.scope();
        let (mut admission, mut renderer, mut owner) = recording_live_pair(scope, 8000, 1).unwrap();
        admission.admit(note(scope, 1, 1)).unwrap();
        // Hold only the final immutable request publication until sample one.
        // This deterministically models admission concurrent with a callback.
        let fence = LiveFinishFence {
            id: LiveId([3; 16]),
            scope,
            accepted_sequence: Some(1),
        };
        admission.control.extend_finish().unwrap();
        admission.finish = Some(fence);
        admission.token = None;
        let published = Cell::new(false);
        let control = admission.control.clone();
        renderer.render(&mut [0.0; 128], |value| {
            if !published.replace(true) {
                control.finish.publish(fence);
            }
            value
        });
        assert_eq!(admission.finish_ack().unwrap(), None);
        let progress = admission.progress();
        assert_eq!(progress.state, LiveState::Finishing);
        assert!(progress.is_valid());
        while let Some(packet) = owner.try_pop().unwrap() {
            assert!(!matches!(packet.kind, PerformancePacketKind::Finish(_)));
        }
        renderer.render_into(&mut [0.0; 1]);
        let ack = admission.finish_ack().unwrap().unwrap();
        assert_eq!((ack.callback_index, ack.cut_frame), (2, 129));
        assert_eq!(admission.progress().state, LiveState::FinishedCut);
        assert!(admission.progress().is_valid());
    }
    #[test]
    fn finish_cancellation_during_pcm_and_unpublished_fault_both_veto_ack() {
        use crate::performance_tap::PerformancePacketKind;
        use std::cell::Cell;
        for pending_fault in [false, true] {
            let scope = pair().0.scope();
            let (mut admission, mut renderer, mut owner) =
                recording_live_pair(scope, 8000, 1).unwrap();
            admission.admit(note(scope, 1, 1)).unwrap();
            admission.begin_finish(scope, LiveId([3; 16])).unwrap();
            if pending_fault {
                admission
                    .control
                    .fault_code
                    .store(u32::MAX, Ordering::Release);
            }
            let tap = owner.control();
            let samples = Cell::new(0);
            renderer.render(&mut [0.0; 128], |value| {
                samples.set(samples.get() + 1);
                if !pending_fault && samples.get() == 128 {
                    tap.cancel();
                }
                value
            });
            assert_eq!(admission.finish_ack(), Err(LiveSubmitError::Faulted));
            while let Some(packet) = owner.try_pop().unwrap() {
                assert!(!matches!(packet.kind, PerformancePacketKind::Finish(_)));
            }
            if pending_fault {
                assert_eq!(admission.progress().state, LiveState::Panicking);
                assert!(admission.progress().is_valid());
                admission.control.fault_frame.store(0, Ordering::Relaxed);
                admission
                    .control
                    .fault_generation
                    .store(scope.panic_generation, Ordering::Relaxed);
                admission
                    .control
                    .fault_code
                    .store(code(LiveFaultKind::Device), Ordering::Release);
                assert_eq!(
                    admission.progress().fault.unwrap().kind,
                    LiveFaultKind::Device
                );
                assert!(admission.progress().is_valid());
            }
        }
    }
    #[test]
    fn finished_progress_uses_immutable_cut_instead_of_earlier_callback_observations() {
        let scope = pair().0.scope();
        let (mut admission, mut renderer, _owner) = recording_live_pair(scope, 8000, 1).unwrap();
        admission.admit(note(scope, 1, 1)).unwrap();
        admission.begin_finish(scope, LiveId([3; 16])).unwrap();
        renderer.render_into(&mut [0.0; 128]);
        let ack = admission.finish_ack().unwrap().unwrap();
        // Deliberately supply an incompatible pre-cut observation tuple. The
        // acquired ACK, rather than these independently sampled fields, owns
        // every field required for a valid FinishedCut snapshot.
        admission
            .control
            .active
            .store(3 | (2 << 16), Ordering::Release);
        admission.control.applied.store(0, Ordering::Release);
        admission.control.frames.store(0, Ordering::Release);
        let progress = admission.progress();
        assert_eq!(progress.state, LiveState::FinishedCut);
        assert_eq!(progress.frames, ack.cut_frame);
        assert_eq!(
            progress.callback_applied_sequence,
            ack.fence.accepted_sequence
        );
        assert_eq!(
            (progress.active_voices, progress.release_tail_voices),
            (0, 0)
        );
        assert!(progress.is_valid());
        assert!(admission.lease_credit().is_none());
    }
    #[test]
    fn finish_preserves_longer_live_deadline_and_rejects_expiry_or_counter_exhaustion() {
        let scope = pair().0.scope();
        let (mut admission, _renderer, _owner) = recording_live_pair(scope, 768_000, 1).unwrap();
        admission.begin_finish(scope, LiveId([3; 16])).unwrap();
        assert_eq!(admission.progress().lease_deadline_frame, 768_000);
        for exhausted in [false, true] {
            let (mut admission, _renderer, _owner) = recording_live_pair(scope, 8000, 1).unwrap();
            admission.control.frames.store(
                if exhausted { EXPIRED - 1 } else { 8000 },
                Ordering::Release,
            );
            assert_eq!(
                admission.begin_finish(scope, LiveId([3; 16])),
                Err(LiveSubmitError::Faulted)
            );
            assert!(admission.finish.is_none());
            assert!(
                !admission
                    .control
                    .finish
                    .request_ready
                    .load(Ordering::Acquire)
            );
            assert_eq!(
                admission.control.first_fault().unwrap().kind,
                if exhausted {
                    LiveFaultKind::CounterExhausted
                } else {
                    LiveFaultKind::LeaseExpired
                }
            );
        }
    }
    #[test]
    fn recording_pending_fault_clear_retains_actual_frame_without_guessed_cause() {
        use crate::performance_tap::PerformancePacketKind;
        let (mut admission, mut renderer, mut owner) = recording_live_pair(
            LiveScope {
                worker_generation: LiveId([1; 16]),
                session_id: LiveId([2; 16]),
                panic_generation: 1,
            },
            8000,
            1,
        )
        .unwrap();
        admission.admit(note(admission.scope(), 1, 1)).unwrap();
        renderer.render_into(&mut [0.0; 1]);
        while owner.try_pop().unwrap().is_some() {}
        // Hold a deterministic publication window: the callback must silence
        // immediately without inventing a fault cause or consuming originals.
        admission
            .control
            .fault_code
            .store(u32::MAX, Ordering::Release);
        renderer.render_into(&mut [0.0; 2]);
        let mut ends = Vec::new();
        while let Some(packet) = owner.try_pop().unwrap() {
            if let PerformancePacketKind::End(end) = packet.kind {
                ends.push(end);
            }
        }
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0].first_fault, None);
        assert_eq!(
            ends[0].clears[0],
            Some(TapClear {
                scope: admission.scope(),
                frame: 1,
                reason: TapClearReason::FaultPending
            })
        );
        assert_eq!(ends[0].clears[1], None);
        admission.control.fault_frame.store(1, Ordering::Relaxed);
        admission
            .control
            .fault_generation
            .store(1, Ordering::Relaxed);
        admission
            .control
            .fault_code
            .store(code(LiveFaultKind::Device), Ordering::Release);
        renderer.render_into(&mut [0.0; 2]);
        while let Some(packet) = owner.try_pop().unwrap() {
            if let PerformancePacketKind::End(end) = packet.kind {
                assert_eq!(end.first_fault.unwrap().kind, LiveFaultKind::Device);
                assert_eq!(
                    end.clears, [None; 2],
                    "The earlier forced clear is not re-timed or duplicated"
                );
            }
        }
    }
    #[test]
    fn recording_malformed_fixed_original_is_rejected_before_indexing() {
        use crate::performance_tap::PerformancePacketKind;
        let (mut admission, mut renderer, mut owner) = recording_live_pair(
            LiveScope {
                worker_generation: LiveId([1; 16]),
                session_id: LiveId([2; 16]),
                panic_generation: 1,
            },
            8000,
            1,
        )
        .unwrap();
        let mut original = note(admission.scope(), 1, 1);
        original.len = u8::MAX;
        // Bypass public admission solely to challenge the callback defensive path.
        admission.producer.push(original).unwrap();
        let mut output = [1.0; 4];
        renderer.render_into(&mut output);
        assert_eq!(output, [0.0; 4]);
        let mut seen = false;
        while let Some(packet) = owner.try_pop().unwrap() {
            if let PerformancePacketKind::Batch { batch, disposition } = packet.kind {
                assert_eq!(batch, original);
                assert_eq!(
                    disposition,
                    BatchDisposition::Rejected {
                        fault: LiveFaultKind::InvalidBatch
                    }
                );
                seen = true;
            }
        }
        assert!(seen);
        assert_eq!(admission.progress().callback_applied_sequence, None);
    }
    #[test]
    fn held_voices_are_distinct_and_lease_silences_at_exact_frame() {
        let (mut a, mut r) = pair();
        a.admit(note(a.scope(), 1, 1)).unwrap();
        a.admit(note(a.scope(), 2, 2)).unwrap();
        let mut out = [0.0; 8001];
        r.render_into(&mut out);
        assert!(out[..8000].iter().any(|v| *v != 0.0));
        assert_eq!(out[8000], 0.0);
        assert_eq!(
            a.progress().fault.unwrap().kind,
            LiveFaultKind::LeaseExpired
        );
        assert_eq!(a.progress().active_voices, 0);
    }
    #[test]
    fn panic_drains_four_per_callback_before_new_scope() {
        let (mut a, mut r) = pair();
        for n in 1..=16 {
            a.admit(LiveBatch::empty(a.scope(), n, None)).unwrap();
        }
        let next = a.begin_panic(a.scope()).unwrap();
        for _ in 0..3 {
            r.render_into(&mut [0.0; 1]);
            assert!(a.finish_panic().unwrap().is_none());
        }
        r.render_into(&mut [0.0; 1]);
        assert_eq!(a.finish_panic().unwrap().unwrap().scope, next);
        assert_eq!(a.progress().next_sequence(), Some(1));
    }
    #[test]
    fn stale_does_not_fault_new_scope_and_bad_batch_is_atomic() {
        let (mut a, mut r) = pair();
        let mut b = note(a.scope(), 1, 1);
        b.scope.session_id = LiveId([3; 16]);
        assert_eq!(a.admit(b), Err(LiveSubmitError::StaleScope));
        assert!(a.progress().fault.is_none());
        let mut b = note(a.scope(), 1, 1);
        b.try_push(LiveEvent {
            source: 1,
            connection_generation: 1,
            observed_ticks: 2,
            event: LiveEventKind::NoteOff {
                voice: 999,
                release_velocity16: 0,
            },
        })
        .unwrap();
        a.admit(b).unwrap();
        let mut out = [1.0; 64];
        r.render_into(&mut out);
        assert!(out.iter().all(|v| *v == 0.0));
        assert_eq!(a.progress().active_voices, 0);
    }
    #[test]
    fn healthy_panic_resets_handles_but_not_same_epoch_clock() {
        let (mut admission, mut renderer) = pair();
        let mut batch = note(admission.scope(), 1, 1);
        batch.events[0].as_mut().unwrap().observed_ticks = 100;
        admission.admit(batch).unwrap();
        renderer.render_into(&mut [0.0; 1]);
        let next = admission.begin_panic(admission.scope()).unwrap();
        renderer.render_into(&mut [0.0; 1]);
        admission.finish_panic().unwrap().unwrap();
        let mut regressing = note(next, 1, 1);
        regressing.events[0].as_mut().unwrap().observed_ticks = 99;
        assert_eq!(admission.admit(regressing), Err(LiveSubmitError::Invalid));
        let fault = admission.progress().fault.unwrap();
        assert_eq!(fault.kind, LiveFaultKind::Clock);
        assert_eq!(fault.scope, next);
    }
    #[test]
    fn token_renews_at_admission_not_callback_drain() {
        let (mut a, mut r) = pair();
        r.render_into(&mut [0.0; 7999]);
        let credit = a.lease_credit().unwrap();
        let b = LiveBatch::empty(a.scope(), 1, Some(credit.token));
        a.admit(b).unwrap();
        assert_eq!(a.progress().lease_deadline_frame, 15999);
        assert_ne!(a.lease_credit().unwrap().token, credit.token);
        r.render_into(&mut [0.0; 8001]);
        assert_eq!(
            a.progress().fault.unwrap().kind,
            LiveFaultKind::LeaseExpired
        );
        assert_eq!(
            a.admit(LiveBatch::empty(
                a.scope(),
                2,
                Some(a.lease_credit().map_or(credit.token, |c| c.token))
            )),
            Err(LiveSubmitError::Faulted)
        );
    }
}
