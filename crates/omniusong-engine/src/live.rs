//! Device-free bounded live synthesis. Only `LiveRenderer` mutates voices.
//! GUI ticks are retained observations; events apply at the next callback boundary.
use crate::live_types::*;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
const EXPIRED: u64 = 1 << 63;
const TABLE: usize = 2048;

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
}
impl LiveControl {
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
        },
    ))
}
impl LiveAdmission {
    pub fn control(&self) -> Arc<LiveControl> {
        self.control.clone()
    }
    pub fn scope(&self) -> LiveScope {
        self.scope
    }
    pub fn lease_credit(&self) -> Option<LeaseCredit> {
        if self.control.faulted() {
            None
        } else {
            self.token
        }
    }
    pub fn progress(&self) -> LiveProgress {
        let applied = self.control.applied.load(Ordering::Acquire);
        let voice_counts = self.control.active.load(Ordering::Acquire);
        let fault_code = self.control.fault_code.load(Ordering::Acquire);
        let fault = self.control.fault_from_code(fault_code);
        let scope = fault.map_or(self.scope, |f| f.scope);
        let same_scope = scope == self.scope;
        LiveProgress {
            scope,
            state: if fault_code == u32::MAX {
                LiveState::Panicking
            } else if fault_code != 0 {
                LiveState::Faulted
            } else if self.pending_panic.is_some() {
                LiveState::Panicking
            } else {
                LiveState::Running
            },
            worker_accepted_sequence: if same_scope { self.accepted } else { None },
            callback_applied_sequence: if same_scope {
                (applied != 0).then_some(applied)
            } else {
                None
            },
            active_voices: (voice_counts & 0xffff) as u16,
            release_tail_voices: (voice_counts >> 16) as u16,
            late_releases: self.control.late.load(Ordering::Acquire),
            rejected_batches: self.control.rejected.load(Ordering::Acquire),
            overflow_batches: self.control.overflow.load(Ordering::Acquire),
            frames: self.control.frames(),
            peak: f32::from_bits(self.control.peak.load(Ordering::Acquire)),
            lease_deadline_frame: self.control.deadline.load(Ordering::Acquire) & !EXPIRED,
            lease_credit: if fault_code == 0 && self.pending_panic.is_none() {
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
    pub fn backing_owner_count(&self) -> usize {
        Arc::strong_count(&self.tables)
    }
}
impl LiveRenderer {
    fn clear(&mut self) {
        self.voices = [EMPTY; LIVE_VOICES];
        self.control.active.store(0, Ordering::Release);
        self.control.peak.store(0, Ordering::Release);
        for s in &mut self.sources {
            s.sustain = false;
        }
    }
    fn apply(&mut self, batch: LiveBatch) -> Result<(), LiveFaultKind> {
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
            self.clear();
            for _ in 0..LIVE_CALLBACK_PACKET_BUDGET {
                if self
                    .consumer
                    .as_mut()
                    .expect("live consumer")
                    .pop()
                    .is_err()
                {
                    break;
                }
            }
            if self.consumer.as_ref().expect("live consumer").is_empty() {
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
            self.clear();
            return;
        }
        for _ in 0..LIVE_CALLBACK_PACKET_BUDGET {
            let Ok(batch) = self.consumer.as_mut().expect("live consumer").pop() else {
                break;
            };
            if batch.scope != self.scope {
                continue;
            }
            if let Err(fault) = self.apply(batch) {
                self.control.fault(fault, Some(batch.sequence));
                self.clear();
                break;
            }
            self.control
                .applied
                .store(batch.sequence, Ordering::Release);
        }
    }
    fn sample(&mut self) -> f32 {
        if !self.control.audible_at(self.frame) {
            self.clear();
            return 0.0;
        }
        let mut mix = 0.0;
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            if voice.auto_release.is_some_and(|end| self.frame >= end) && !voice.releasing {
                // Pulse release velocity is retained in the packet; automatic release
                // must not rely on a future UI timer or sustain release.
                release(voice, voice.auto_release_velocity, self.rate);
            }
            if voice.releasing {
                voice.envelope -= voice.release_step;
                if voice.envelope <= 0.0 {
                    voice.active = false;
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
        mix.clamp(-1.0, 1.0)
    }
    fn render<T>(&mut self, out: &mut [T], convert: impl Fn(f32) -> T) {
        if out.is_empty()
            || !out.len().is_multiple_of(self.channels)
            || out.len() / self.channels > LIVE_MAX_CALLBACK_FRAMES
        {
            self.control.fault(LiveFaultKind::CallbackShape, None);
            self.clear();
            for sample in out {
                *sample = convert(0.0);
            }
            return;
        }
        self.boundary();
        let mut peak = 0.0f32;
        // At most four packets above for the WHOLE native invocation; frame work is bounded.
        for block in out.chunks_mut(2048 * self.channels) {
            for frame in block.chunks_mut(self.channels) {
                let value = self.sample();
                peak = peak.max(value.abs());
                for dst in frame {
                    *dst = convert(value);
                }
                let Some(next) = self.frame.checked_add(1).filter(|n| *n < EXPIRED) else {
                    self.control.fault(LiveFaultKind::CounterExhausted, None);
                    break;
                };
                self.frame = next;
                if !self.control.audible_at(next) {
                    self.clear();
                }
                self.control.frames.store(next, Ordering::Release);
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
        self.control.peak.store(peak.to_bits(), Ordering::Release);
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
