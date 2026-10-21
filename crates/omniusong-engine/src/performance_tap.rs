//! Fixed callback evidence for a future retained performance writer. This is
//! process-local Copy storage, not a wire format, journal codec or heard-audio claim.
use crate::live_types::{
    LIVE_MAX_CALLBACK_FRAMES, LIVE_MAX_SOURCES, LIVE_VOICES, LiveBatch, LiveFault, LiveFaultKind,
    LiveScope,
};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{
        Arc, Mutex, TryLockError,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

pub const TAP_SLOTS: usize = 256;
pub const TAP_PCM_FRAMES: usize = 256;
pub const TAP_CHANNELS: usize = 8;
pub const TAP_MAX_BATCHES: usize = 4;
pub const TAP_TRANSITIONS: usize = 128;
pub const TAP_CALLBACK_PACKETS: usize = 70;
const WRITING: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TapInfo {
    pub scope: LiveScope,
    pub sample_rate: u32,
    pub channels: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TapBegin {
    pub scope: LiveScope,
    pub start_frame: u64,
    pub frames: u16,
    pub sample_rate: u32,
    pub channels: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum BatchDisposition {
    Applied,
    DiscardedStale,
    DiscardedPanic,
    Rejected { fault: LiveFaultKind },
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapPcm {
    pub start_frame: u64,
    pub frames: u16,
    pub channels: u16,
    pub samples: [f32; TAP_PCM_FRAMES * TAP_CHANNELS],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TapClearReason {
    Panic { next_generation: u64 },
    Fault { kind: LiveFaultKind },
    FaultPending,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TapClear {
    pub scope: LiveScope,
    pub frame: u64,
    pub reason: TapClearReason,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TapTransitionKind {
    AutoRelease { release_velocity16: u16 },
    VoiceEnded,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TapTransition {
    pub scope: LiveScope,
    pub source: u8,
    pub voice: u64,
    pub frame: u64,
    pub kind: TapTransitionKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TapEnd {
    pub end_frame: u64,
    pub scope_after: LiveScope,
    pub applied_sequence: Option<u64>,
    pub active_voices: u16,
    pub release_tail_voices: u16,
    pub clears: [Option<TapClear>; 2],
    pub transitions: [Option<TapTransition>; TAP_TRANSITIONS],
    pub transition_len: u16,
    pub first_fault: Option<LiveFault>,
}
// Fixed Copy union is deliberate: callback transfer must never allocate or drop
// heap owners. Both large variants fit the compile-checked16KiB packet budget.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PerformancePacketKind {
    Begin(TapBegin),
    Batch {
        batch: LiveBatch,
        disposition: BatchDisposition,
    },
    Pcm(TapPcm),
    End(TapEnd),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerformancePacket {
    pub callback_index: u64,
    pub ordinal: u16,
    pub kind: PerformancePacketKind,
}

const _: () = assert!(std::mem::size_of::<PerformancePacket>() <= 16 * 1024);
const _: () = assert!(std::mem::size_of::<PerformancePacket>() * TAP_SLOTS <= 4 * 1024 * 1024);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapFaultKind {
    Overflow,
    ReceiverAbandoned,
    MalformedPacket,
    CounterExhausted,
    Aborted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TapFault {
    pub kind: TapFaultKind,
    pub frame: u64,
    pub callback_index: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapError {
    Invalid,
    Faulted(TapFault),
    Busy,
}
impl fmt::Display for TapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => f.write_str("Invalid performance tap configuration"),
            Self::Busy => f.write_str("Performance tap snapshot or reader is busy"),
            Self::Faulted(fault) => write!(
                f,
                "Performance tap {:?} at frame {} callback {:?}",
                fault.kind, fault.frame, fault.callback_index
            ),
        }
    }
}
impl std::error::Error for TapError {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TapSnapshot {
    pub completed_callback_index: u64,
    pub completed_frames: u64,
    pub producer_destroyed: bool,
    pub cancelled: bool,
    pub fault: Option<TapFault>,
}

pub struct TapControl {
    completed_version: AtomicU64,
    completed_callback: AtomicU64,
    completed_frames: AtomicU64,
    producer_destroyed: AtomicBool,
    cancelled: AtomicBool,
    fault_code: AtomicU32,
    fault_frame: AtomicU64,
    fault_callback: AtomicU64,
}
fn fault_code(kind: TapFaultKind) -> u32 {
    match kind {
        TapFaultKind::Overflow => 1,
        TapFaultKind::ReceiverAbandoned => 2,
        TapFaultKind::MalformedPacket => 3,
        TapFaultKind::CounterExhausted => 4,
        TapFaultKind::Aborted => 5,
    }
}
fn fault_kind(code: u32) -> TapFaultKind {
    match code {
        1 => TapFaultKind::Overflow,
        2 => TapFaultKind::ReceiverAbandoned,
        3 => TapFaultKind::MalformedPacket,
        4 => TapFaultKind::CounterExhausted,
        _ => TapFaultKind::Aborted,
    }
}
impl TapControl {
    fn new() -> Self {
        Self {
            completed_version: AtomicU64::new(0),
            completed_callback: AtomicU64::new(0),
            completed_frames: AtomicU64::new(0),
            producer_destroyed: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            fault_code: AtomicU32::new(0),
            fault_frame: AtomicU64::new(0),
            fault_callback: AtomicU64::new(0),
        }
    }
    fn latch(&self, kind: TapFaultKind, frame: u64, callback: Option<u64>) {
        if self
            .fault_code
            .compare_exchange(0, WRITING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.fault_frame.store(frame, Ordering::Relaxed);
            self.fault_callback
                .store(callback.unwrap_or(0), Ordering::Relaxed);
            self.fault_code.store(fault_code(kind), Ordering::Release);
        }
    }
    fn first_fault(&self) -> Result<Option<TapFault>, TapError> {
        let code = self.fault_code.load(Ordering::Acquire);
        if code == WRITING {
            return Err(TapError::Busy);
        }
        if code == 0 {
            return Ok(None);
        }
        let index = self.fault_callback.load(Ordering::Relaxed);
        Ok(Some(TapFault {
            kind: fault_kind(code),
            frame: self.fault_frame.load(Ordering::Relaxed),
            callback_index: (index != 0).then_some(index),
        }))
    }
    fn status(&self) -> Result<(), TapError> {
        if let Some(fault) = self.first_fault()? {
            return Err(TapError::Faulted(fault));
        }
        if self.cancelled.load(Ordering::Acquire) {
            return Err(TapError::Busy);
        }
        Ok(())
    }
    fn completed_prefix(&self) -> Result<(u64, u64), TapError> {
        for _ in 0..2 {
            let version = self.completed_version.load(Ordering::SeqCst);
            if version & 1 != 0 {
                continue;
            }
            let callback = self.completed_callback.load(Ordering::SeqCst);
            let frames = self.completed_frames.load(Ordering::SeqCst);
            if version == self.completed_version.load(Ordering::SeqCst) {
                return Ok((callback, frames));
            }
        }
        Err(TapError::Busy)
    }
    fn observed_fault_context(&self) -> (u64, Option<u64>) {
        match self.completed_prefix() {
            Ok((callback, frames)) => (frames, (callback != 0).then_some(callback)),
            // Publication is busy: this is only an off-callback frame sample.
            // Omitting the index prevents associating a newer frame with an
            // older callback, or claiming an exact callback failure boundary.
            Err(_) => (self.completed_frames.load(Ordering::SeqCst), None),
        }
    }
    /// Bounded coherent read. Busy never substitutes a made-up first cause or
    /// a mixed old callback/new frame count during concurrent publication.
    pub fn snapshot(&self) -> Result<TapSnapshot, TapError> {
        for _ in 0..2 {
            let destroyed = self.producer_destroyed.load(Ordering::Acquire);
            let version = self.completed_version.load(Ordering::SeqCst);
            if version & 1 != 0 {
                continue;
            }
            let callback = self.completed_callback.load(Ordering::SeqCst);
            let frames = self.completed_frames.load(Ordering::SeqCst);
            let fault = self.first_fault()?;
            let cancelled = self.cancelled.load(Ordering::Acquire);
            if version == self.completed_version.load(Ordering::SeqCst) {
                return Ok(TapSnapshot {
                    completed_callback_index: callback,
                    completed_frames: frames,
                    producer_destroyed: destroyed,
                    cancelled,
                    fault,
                });
            }
        }
        Err(TapError::Busy)
    }
    /// Off-callback observation of the last completed prefix, not an exact
    /// failure-frame measurement. Cancellation never overwrites an earlier cause.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let (frame, callback) = self.observed_fault_context();
        self.latch(TapFaultKind::Aborted, frame, callback);
    }
    fn abandoned(&self) {
        let (frame, callback) = self.observed_fault_context();
        self.latch(TapFaultKind::ReceiverAbandoned, frame, callback);
    }
}

struct Backing {
    consumer: Mutex<Consumer<PerformancePacket>>,
    control: Arc<TapControl>,
}
/// Unique off-callback drain owner. Drop signals abandonment while the producer
/// exists; queued originals remain available through try_pop after a tap fault.
pub struct PerformanceTapOwner {
    info: TapInfo,
    backing: Arc<Backing>,
}
impl PerformanceTapOwner {
    pub fn info(&self) -> TapInfo {
        self.info
    }
    pub fn control(&self) -> Arc<TapControl> {
        self.backing.control.clone()
    }
    pub fn try_pop(&mut self) -> Result<Option<PerformancePacket>, TapError> {
        let mut consumer = match self.backing.consumer.try_lock() {
            Ok(consumer) => consumer,
            Err(TryLockError::WouldBlock) => return Err(TapError::Busy),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };
        Ok(consumer.pop().ok())
    }
}
impl Drop for PerformanceTapOwner {
    fn drop(&mut self) {
        if !self
            .backing
            .control
            .producer_destroyed
            .load(Ordering::Acquire)
        {
            self.backing.control.abandoned();
        }
    }
}
/// Admission retains this until the ENTIRE renderer destructor has completed.
/// The producer flag follows rtrb Producer destruction; it is not permission to
/// race destruction of the remaining renderer/producer wrapper fields.
pub(crate) struct PerformanceTapRetention {
    backing: Option<Arc<Backing>>,
}
impl Drop for PerformanceTapRetention {
    fn drop(&mut self) {
        if let Some(backing) = self.backing.take()
            && !backing.control.producer_destroyed.load(Ordering::Acquire)
        {
            backing.control.abandoned();
            // One bounded allocation retained until process exit on broken
            // ownership. Never let the callback become the queue's final owner.
            std::mem::forget(backing);
        }
    }
}

#[derive(Clone, Copy)]
struct ActiveCallback {
    begin: TapBegin,
    index: u64,
    ordinal: u16,
    batches: u8,
    pcm_frames: u64,
    reservation: usize,
}
pub(crate) struct PerformanceTapProducer {
    info: TapInfo,
    producer: Option<Producer<PerformancePacket>>,
    control: Arc<TapControl>,
    active: Option<ActiveCallback>,
    next_callback: u64,
    next_frame: u64,
    next_scope: LiveScope,
}
pub(crate) fn tap_pair(
    info: TapInfo,
) -> Result<
    (
        PerformanceTapProducer,
        PerformanceTapOwner,
        PerformanceTapRetention,
    ),
    TapError,
> {
    if !info.scope.is_valid()
        || !(8_000..=768_000).contains(&info.sample_rate)
        || !(1..=TAP_CHANNELS as u16).contains(&info.channels)
    {
        return Err(TapError::Invalid);
    }
    let (producer, consumer) = RingBuffer::new(TAP_SLOTS);
    let control = Arc::new(TapControl::new());
    let backing = Arc::new(Backing {
        consumer: Mutex::new(consumer),
        control: control.clone(),
    });
    Ok((
        PerformanceTapProducer {
            info,
            producer: Some(producer),
            control,
            active: None,
            next_callback: 1,
            next_frame: 0,
            next_scope: info.scope,
        },
        PerformanceTapOwner {
            info,
            backing: backing.clone(),
        },
        PerformanceTapRetention {
            backing: Some(backing),
        },
    ))
}

fn same_session(left: LiveScope, right: LiveScope) -> bool {
    left.is_valid()
        && right.is_valid()
        && left.worker_generation == right.worker_generation
        && left.session_id == right.session_id
}
impl PerformanceTapProducer {
    pub fn fault(&self, kind: TapFaultKind, frame: u64) {
        self.control
            .latch(kind, frame, self.active.map(|active| active.index));
    }
    fn fail(&self, kind: TapFaultKind, frame: u64, callback: Option<u64>) -> TapError {
        self.control.latch(kind, frame, callback);
        match self.control.first_fault() {
            Ok(Some(fault)) => TapError::Faulted(fault),
            _ => TapError::Busy,
        }
    }
    fn malformed(&self) -> TapError {
        self.fail(
            TapFaultKind::MalformedPacket,
            self.active
                .map_or(self.next_frame, |a| a.begin.start_frame + a.pcm_frames),
            self.active.map(|a| a.index),
        )
    }
    fn emit(&mut self, kind: PerformancePacketKind) -> Result<(), TapError> {
        self.control.status()?;
        let active = self.active.ok_or_else(|| self.malformed())?;
        if usize::from(active.ordinal) >= active.reservation
            || usize::from(active.ordinal) >= TAP_CALLBACK_PACKETS
        {
            return Err(self.malformed());
        }
        let packet = PerformancePacket {
            callback_index: active.index,
            ordinal: active.ordinal,
            kind,
        };
        if self
            .producer
            .as_mut()
            .expect("tap producer exists")
            .push(packet)
            .is_err()
        {
            return Err(self.fail(
                TapFaultKind::Overflow,
                active.begin.start_frame + active.pcm_frames,
                Some(active.index),
            ));
        }
        self.active.as_mut().expect("active callback").ordinal += 1;
        Ok(())
    }
    /// Reserve the whole maximum callback transaction BEFORE renderer changes.
    pub fn begin_callback(&mut self, begin: TapBegin) -> Result<u64, TapError> {
        self.control.status()?;
        if self.active.is_some() {
            return Err(self.malformed());
        }
        let frames = usize::from(begin.frames);
        let end = begin.start_frame.checked_add(u64::from(begin.frames));
        if begin.scope != self.next_scope
            || begin.start_frame != self.next_frame
            || begin.sample_rate != self.info.sample_rate
            || begin.channels != self.info.channels
            || frames == 0
            || frames > LIVE_MAX_CALLBACK_FRAMES
            || end.is_none_or(|end| end > i64::MAX as u64)
        {
            return Err(self.fail(
                TapFaultKind::MalformedPacket,
                begin.start_frame,
                Some(self.next_callback),
            ));
        }
        if self.next_callback == u64::MAX {
            return Err(self.fail(
                TapFaultKind::CounterExhausted,
                begin.start_frame,
                Some(self.next_callback),
            ));
        }
        let reservation = 2 + TAP_MAX_BATCHES + frames.div_ceil(TAP_PCM_FRAMES);
        if reservation > TAP_CALLBACK_PACKETS
            || self.producer.as_ref().expect("tap producer exists").slots() < reservation
        {
            return Err(self.fail(
                TapFaultKind::Overflow,
                begin.start_frame,
                Some(self.next_callback),
            ));
        }
        let index = self.next_callback;
        self.active = Some(ActiveCallback {
            begin,
            index,
            ordinal: 0,
            batches: 0,
            pcm_frames: 0,
            reservation,
        });
        self.emit(PerformancePacketKind::Begin(begin))?;
        Ok(index)
    }
    pub fn push_batch(
        &mut self,
        batch: LiveBatch,
        disposition: BatchDisposition,
    ) -> Result<(), TapError> {
        self.control.status()?;
        let active = self.active.ok_or_else(|| self.malformed())?;
        if usize::from(active.batches) >= TAP_MAX_BATCHES || active.pcm_frames != 0 {
            return Err(self.malformed());
        }
        if disposition == BatchDisposition::Applied
            && (batch.scope != active.begin.scope
                || batch.validate_shape(active.begin.sample_rate).is_err())
        {
            return Err(self.malformed());
        }
        // Rejected/discarded packets are bounded Copy originals. Do not slice by
        // their possibly invalid len or reinterpret foreign source/clock data.
        self.emit(PerformancePacketKind::Batch { batch, disposition })?;
        self.active.as_mut().expect("active callback").batches += 1;
        Ok(())
    }
    pub fn push_pcm(&mut self, pcm: TapPcm) -> Result<(), TapError> {
        self.control.status()?;
        let active = self.active.ok_or_else(|| self.malformed())?;
        let frames = usize::from(pcm.frames);
        let samples = frames.checked_mul(usize::from(pcm.channels));
        let next = active.pcm_frames.checked_add(u64::from(pcm.frames));
        if frames == 0
            || frames > TAP_PCM_FRAMES
            || pcm.channels != self.info.channels
            || pcm.start_frame != active.begin.start_frame + active.pcm_frames
            || next.is_none_or(|next| next > u64::from(active.begin.frames))
            || samples.is_none_or(|samples| samples > pcm.samples.len())
        {
            return Err(self.malformed());
        }
        let samples = samples.expect("checked PCM extent");
        if pcm.samples[..samples].iter().any(|v| !v.is_finite())
            || pcm.samples[samples..].iter().any(|v| v.to_bits() != 0)
        {
            return Err(self.malformed());
        }
        self.emit(PerformancePacketKind::Pcm(pcm))?;
        self.active.as_mut().expect("active callback").pcm_frames = next.expect("checked frames");
        Ok(())
    }
    pub fn end_callback(&mut self, end: TapEnd) -> Result<(), TapError> {
        self.control.status()?;
        let active = self.active.ok_or_else(|| self.malformed())?;
        let scope = active.begin.scope;
        let after = end.scope_after;
        let valid_scope = |candidate: LiveScope| {
            same_session(scope, candidate)
                && candidate.panic_generation >= scope.panic_generation
                && candidate.panic_generation <= after.panic_generation
        };
        if active.pcm_frames != u64::from(active.begin.frames)
            || end.end_frame != active.begin.start_frame + u64::from(active.begin.frames)
            || !same_session(scope, after)
            || !(after.panic_generation == scope.panic_generation
                || scope.panic_generation.checked_add(1) == Some(after.panic_generation))
            || end.applied_sequence == Some(0)
            || usize::from(end.active_voices) > LIVE_VOICES
            || end.release_tail_voices > end.active_voices
            || usize::from(end.transition_len) > TAP_TRANSITIONS
        {
            return Err(self.malformed());
        }
        let mut clear_gap = false;
        for clear in end.clears {
            let Some(clear) = clear else {
                clear_gap = true;
                continue;
            };
            if clear_gap
                || !valid_scope(clear.scope)
                || !(active.begin.start_frame..=end.end_frame).contains(&clear.frame)
            {
                return Err(self.malformed());
            }
            if let TapClearReason::Panic { next_generation } = clear.reason
                && clear.scope.panic_generation.checked_add(1) != Some(next_generation)
            {
                return Err(self.malformed());
            }
        }
        for (index, transition) in end.transitions.iter().enumerate() {
            if index >= usize::from(end.transition_len) {
                if transition.is_some() {
                    return Err(self.malformed());
                }
                continue;
            }
            let Some(transition) = transition else {
                return Err(self.malformed());
            };
            if !valid_scope(transition.scope)
                || !(1..=LIVE_MAX_SOURCES as u8).contains(&transition.source)
                || transition.voice == 0
                || !(active.begin.start_frame..=end.end_frame).contains(&transition.frame)
            {
                return Err(self.malformed());
            }
        }
        if end.first_fault.is_some_and(|fault| {
            !same_session(scope, fault.scope)
                || fault.scope.panic_generation > after.panic_generation
                || fault.frame > end.end_frame
                || fault.sequence == Some(0)
        }) {
            return Err(self.malformed());
        }
        let version = self.control.completed_version.load(Ordering::SeqCst);
        let Some(next_version) = version.checked_add(2) else {
            return Err(self.fail(
                TapFaultKind::CounterExhausted,
                end.end_frame,
                Some(active.index),
            ));
        };
        self.emit(PerformancePacketKind::End(end))?;
        // One SC publication per completed callback, never per sample. Keeping
        // the small prefix tuple in one SC order avoids mixed index/frame reads
        // or a reader accepting fields reordered around its version checks.
        self.control
            .completed_version
            .store(version + 1, Ordering::SeqCst);
        self.control
            .completed_frames
            .store(end.end_frame, Ordering::SeqCst);
        self.control
            .completed_callback
            .store(active.index, Ordering::SeqCst);
        self.control
            .completed_version
            .store(next_version, Ordering::SeqCst);
        self.next_frame = end.end_frame;
        self.next_scope = after;
        self.next_callback = active.index + 1;
        self.active = None;
        Ok(())
    }
}
impl Drop for PerformanceTapProducer {
    fn drop(&mut self) {
        if let Some(active) = self.active {
            self.control.latch(
                TapFaultKind::Aborted,
                active.begin.start_frame + active.pcm_frames,
                Some(active.index),
            );
        }
        drop(self.producer.take());
        self.control
            .producer_destroyed
            .store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_types::{LiveEvent, LiveEventKind, LiveId, LivePatch};
    fn info() -> TapInfo {
        TapInfo {
            scope: LiveScope {
                worker_generation: LiveId([1; 16]),
                session_id: LiveId([2; 16]),
                panic_generation: 1,
            },
            sample_rate: 48_000,
            channels: 2,
        }
    }
    fn begin(info: TapInfo, start: u64, frames: u16) -> TapBegin {
        TapBegin {
            scope: info.scope,
            start_frame: start,
            frames,
            sample_rate: info.sample_rate,
            channels: info.channels,
        }
    }
    fn pcm(info: TapInfo, start: u64, frames: u16) -> TapPcm {
        let mut pcm = TapPcm {
            start_frame: start,
            frames,
            channels: info.channels,
            samples: [0.0; 2048],
        };
        pcm.samples[..usize::from(frames) * usize::from(info.channels)].fill(0.25);
        pcm
    }
    fn end(info: TapInfo, frame: u64) -> TapEnd {
        TapEnd {
            end_frame: frame,
            scope_after: info.scope,
            applied_sequence: None,
            active_voices: 0,
            release_tail_voices: 0,
            clears: [None; 2],
            transitions: [None; 128],
            transition_len: 0,
            first_fault: None,
        }
    }
    fn complete(producer: &mut PerformanceTapProducer, info: TapInfo, start: u64, frames: u16) {
        producer.begin_callback(begin(info, start, frames)).unwrap();
        let mut at = start;
        while at < start + u64::from(frames) {
            let count = (start + u64::from(frames) - at).min(256) as u16;
            producer.push_pcm(pcm(info, at, count)).unwrap();
            at += u64::from(count);
        }
        producer.end_callback(end(info, at)).unwrap();
    }
    #[test]
    fn packet_storage_is_copy_and_bounded() {
        fn copy<T: Copy>() {}
        copy::<PerformancePacket>();
        assert!(std::mem::size_of::<PerformancePacket>() <= 16 * 1024);
        assert!(std::mem::size_of::<PerformancePacket>() * TAP_SLOTS <= 4 * 1024 * 1024);
        for invalid in [
            TapInfo {
                channels: 0,
                ..info()
            },
            TapInfo {
                sample_rate: 7_999,
                ..info()
            },
            TapInfo {
                scope: LiveScope {
                    panic_generation: 0,
                    ..info().scope
                },
                ..info()
            },
        ] {
            assert!(matches!(tap_pair(invalid), Err(TapError::Invalid)));
        }
    }
    #[test]
    fn complete_callbacks_preserve_originals_ordinals_and_exact_frame_prefix() {
        let i = info();
        let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
        assert_eq!(owner.info(), i);
        assert_eq!(p.begin_callback(begin(i, 0, 257)).unwrap(), 1);
        let mut batch = LiveBatch::empty(i.scope, 1, None);
        batch
            .try_push(LiveEvent {
                source: 1,
                connection_generation: 1,
                observed_ticks: u64::MAX,
                event: LiveEventKind::NoteOn {
                    note: 1,
                    voice: 1,
                    key: 60,
                    velocity16: 0,
                    patch: LivePatch::Keys,
                },
            })
            .unwrap();
        p.push_batch(batch, BatchDisposition::Applied).unwrap();
        p.push_pcm(pcm(i, 0, 256)).unwrap();
        p.push_pcm(pcm(i, 256, 1)).unwrap();
        let mut final_end = end(i, 257);
        final_end.applied_sequence = Some(1);
        final_end.active_voices = 1;
        p.end_callback(final_end).unwrap();
        let mut packets = Vec::new();
        while let Some(packet) = owner.try_pop().unwrap() {
            packets.push(packet);
        }
        assert_eq!(packets.len(), 5);
        for (ordinal, packet) in packets.iter().enumerate() {
            assert_eq!(packet.callback_index, 1);
            assert_eq!(usize::from(packet.ordinal), ordinal);
        }
        assert_eq!(
            packets[1].kind,
            PerformancePacketKind::Batch {
                batch,
                disposition: BatchDisposition::Applied
            }
        );
        assert_eq!(packets[4].kind, PerformancePacketKind::End(final_end));
        assert_eq!(owner.control().snapshot().unwrap().completed_frames, 257);
        complete(&mut p, i, 257, 1);
        assert_eq!(
            owner.control().snapshot().unwrap().completed_callback_index,
            2
        );
        drop(p);
        assert!(owner.control().snapshot().unwrap().producer_destroyed);
    }
    #[test]
    fn maximum_callback_reserves_and_emits_no_more_than_seventy_packets() {
        let i = info();
        let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
        p.begin_callback(begin(i, 0, 16_384)).unwrap();
        for sequence in 1..=4 {
            p.push_batch(
                LiveBatch::empty(i.scope, sequence, None),
                BatchDisposition::Applied,
            )
            .unwrap();
        }
        for at in (0..16_384).step_by(256) {
            p.push_pcm(pcm(i, at, 256)).unwrap();
        }
        let mut e = end(i, 16_384);
        e.applied_sequence = Some(4);
        e.transition_len = 128;
        for (index, slot) in e.transitions.iter_mut().enumerate() {
            *slot = Some(TapTransition {
                scope: i.scope,
                source: 1,
                voice: (index / 2 + 1) as u64,
                frame: 16_383,
                kind: if index % 2 == 0 {
                    TapTransitionKind::AutoRelease {
                        release_velocity16: u16::MAX,
                    }
                } else {
                    TapTransitionKind::VoiceEnded
                },
            });
        }
        p.end_callback(e).unwrap();
        let mut count = 0;
        while let Some(packet) = owner.try_pop().unwrap() {
            assert_eq!(usize::from(packet.ordinal), count);
            count += 1;
        }
        assert_eq!(count, 70);
        drop(p);
    }
    #[test]
    fn insufficient_reservation_faults_before_any_begin_and_remains_drainable() {
        let i = info();
        let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
        for frame in 0..84 {
            complete(&mut p, i, frame, 1);
        }
        let error = p.begin_callback(begin(i, 84, 1)).unwrap_err();
        assert!(matches!(
            error,
            TapError::Faulted(TapFault {
                kind: TapFaultKind::Overflow,
                frame: 84,
                callback_index: Some(85)
            })
        ));
        let snapshot = owner.control().snapshot().unwrap();
        assert_eq!(snapshot.completed_frames, 84);
        assert_eq!(snapshot.completed_callback_index, 84);
        let mut count = 0;
        while owner.try_pop().unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 252);
        assert_eq!(p.begin_callback(begin(i, 84, 1)).unwrap_err(), error);
        drop(p);
    }
    #[test]
    fn rejected_and_discarded_originals_are_not_reinterpreted_as_valid_events() {
        let i = info();
        let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
        p.begin_callback(begin(i, 0, 1)).unwrap();
        let mut original = LiveBatch::empty(i.scope, 1, None);
        original.len = u8::MAX;
        original.sequence = 0;
        original.scope.panic_generation = 0;
        original.events[31] = Some(LiveEvent {
            source: 255,
            connection_generation: 0,
            observed_ticks: u64::MAX,
            event: LiveEventKind::NoteOn {
                note: 0,
                voice: 0,
                key: 255,
                velocity16: u16::MAX,
                patch: LivePatch::Pad,
            },
        });
        for disposition in [
            BatchDisposition::DiscardedStale,
            BatchDisposition::DiscardedPanic,
            BatchDisposition::Rejected {
                fault: LiveFaultKind::InvalidBatch,
            },
        ] {
            p.push_batch(original, disposition).unwrap();
        }
        p.push_pcm(pcm(i, 0, 1)).unwrap();
        p.end_callback(end(i, 1)).unwrap();
        owner.try_pop().unwrap();
        for disposition in [
            BatchDisposition::DiscardedStale,
            BatchDisposition::DiscardedPanic,
            BatchDisposition::Rejected {
                fault: LiveFaultKind::InvalidBatch,
            },
        ] {
            assert_eq!(
                owner.try_pop().unwrap().unwrap().kind,
                PerformancePacketKind::Batch {
                    batch: original,
                    disposition
                }
            );
        }
        drop(p);
    }
    #[test]
    fn missing_pcm_or_invalid_packet_never_publishes_completed_end() {
        let i = info();
        for variant in 0..6 {
            let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
            p.begin_callback(begin(i, 0, 2)).unwrap();
            let result = match variant {
                0 => p.end_callback(end(i, 2)),
                1 => p.push_pcm(pcm(i, 1, 1)),
                2 => {
                    let mut packet = pcm(i, 0, 2);
                    packet.samples[1] = f32::NAN;
                    p.push_pcm(packet)
                }
                3 => {
                    let mut packet = pcm(i, 0, 2);
                    packet.samples[100] = 1.0;
                    p.push_pcm(packet)
                }
                4 => {
                    let mut packet = pcm(i, 0, 2);
                    packet.channels = 1;
                    p.push_pcm(packet)
                }
                _ => {
                    p.push_pcm(pcm(i, 0, 2)).unwrap();
                    let mut e = end(i, 2);
                    e.transitions[1] = Some(TapTransition {
                        scope: i.scope,
                        source: 1,
                        voice: 1,
                        frame: 1,
                        kind: TapTransitionKind::VoiceEnded,
                    });
                    e.transition_len = 2;
                    p.end_callback(e)
                }
            };
            assert!(matches!(
                result,
                Err(TapError::Faulted(TapFault {
                    kind: TapFaultKind::MalformedPacket,
                    ..
                }))
            ));
            assert_eq!(owner.control().snapshot().unwrap().completed_frames, 0);
            while let Some(packet) = owner.try_pop().unwrap() {
                assert!(!matches!(packet.kind, PerformancePacketKind::End(_)));
            }
            drop(p);
        }
    }
    #[test]
    fn scope_and_global_frames_continue_across_panic_without_reset() {
        let i = info();
        let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
        p.begin_callback(begin(i, 0, 1)).unwrap();
        p.push_pcm(pcm(i, 0, 1)).unwrap();
        let next = TapInfo {
            scope: LiveScope {
                panic_generation: 2,
                ..i.scope
            },
            ..i
        };
        let mut e = end(next, 1);
        e.clears[0] = Some(TapClear {
            scope: i.scope,
            frame: 0,
            reason: TapClearReason::Panic { next_generation: 2 },
        });
        p.end_callback(e).unwrap();
        complete(&mut p, next, 1, 1);
        assert_eq!(
            owner.control().snapshot().unwrap().completed_callback_index,
            2
        );
        while owner.try_pop().unwrap().is_some() {}
        assert!(p.begin_callback(begin(i, 2, 1)).is_err());
        drop(p);
    }
    #[test]
    fn owner_loss_and_cancel_are_sticky_and_keep_backing_off_the_callback() {
        let i = info();
        let (mut p, owner, retention) = tap_pair(i).unwrap();
        let control = owner.control();
        let weak = Arc::downgrade(&owner.backing);
        drop(owner);
        assert_eq!(
            control.snapshot().unwrap().fault.unwrap().kind,
            TapFaultKind::ReceiverAbandoned
        );
        control.cancel();
        assert_eq!(
            control.snapshot().unwrap().fault.unwrap().kind,
            TapFaultKind::ReceiverAbandoned
        );
        assert!(p.begin_callback(begin(i, 0, 1)).is_err());
        assert!(weak.upgrade().is_some());
        drop(p);
        assert!(control.snapshot().unwrap().producer_destroyed);
        drop(retention);
        assert!(weak.upgrade().is_none());
        let (p, owner, retention) = tap_pair(i).unwrap();
        let control = owner.control();
        control.cancel();
        assert_eq!(
            control.snapshot().unwrap().fault.unwrap().kind,
            TapFaultKind::Aborted
        );
        drop(p);
        drop(owner);
        drop(retention);
    }
    #[test]
    fn producer_drop_retains_partial_callback_without_false_completion() {
        let i = info();
        let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
        p.begin_callback(begin(i, 0, 2)).unwrap();
        p.push_pcm(pcm(i, 0, 1)).unwrap();
        drop(p);
        let snapshot = owner.control().snapshot().unwrap();
        assert!(snapshot.producer_destroyed);
        assert_eq!(snapshot.completed_callback_index, 0);
        assert_eq!(
            snapshot.fault.unwrap(),
            TapFault {
                kind: TapFaultKind::Aborted,
                frame: 1,
                callback_index: Some(1)
            }
        );
        assert!(matches!(
            owner.try_pop().unwrap().unwrap().kind,
            PerformancePacketKind::Begin(_)
        ));
        assert!(matches!(
            owner.try_pop().unwrap().unwrap().kind,
            PerformancePacketKind::Pcm(_)
        ));
        assert!(owner.try_pop().unwrap().is_none());
    }
    #[test]
    fn early_retention_drop_preserves_bounded_backing_instead_of_rt_final_free() {
        let i = info();
        let (p, owner, retention) = tap_pair(i).unwrap();
        let weak = Arc::downgrade(&owner.backing);
        let control = owner.control();
        drop(retention);
        drop(owner);
        assert!(weak.upgrade().is_some());
        drop(p);
        assert!(control.snapshot().unwrap().producer_destroyed);
        assert!(weak.upgrade().is_some());
    }
    #[test]
    fn snapshot_busy_does_not_publish_partial_fault_or_mixed_prefix() {
        let c = TapControl::new();
        c.completed_version.store(1, Ordering::Relaxed);
        assert_eq!(c.snapshot(), Err(TapError::Busy));
        c.completed_version.store(2, Ordering::Relaxed);
        c.fault_code.store(WRITING, Ordering::Relaxed);
        assert_eq!(c.snapshot(), Err(TapError::Busy));
    }
    #[test]
    fn exhausted_counters_fail_before_false_begin_or_end_publication() {
        let i = info();
        let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
        p.next_callback = u64::MAX;
        assert!(matches!(
            p.begin_callback(begin(i, 0, 1)),
            Err(TapError::Faulted(TapFault {
                kind: TapFaultKind::CounterExhausted,
                ..
            }))
        ));
        assert!(owner.try_pop().unwrap().is_none());
        drop(p);
        let (mut p, mut owner, _retention) = tap_pair(i).unwrap();
        p.begin_callback(begin(i, 0, 1)).unwrap();
        p.push_pcm(pcm(i, 0, 1)).unwrap();
        p.control
            .completed_version
            .store(u64::MAX - 1, Ordering::SeqCst);
        assert!(matches!(
            p.end_callback(end(i, 1)),
            Err(TapError::Faulted(TapFault {
                kind: TapFaultKind::CounterExhausted,
                ..
            }))
        ));
        while let Some(packet) = owner.try_pop().unwrap() {
            assert!(!matches!(packet.kind, PerformancePacketKind::End(_)));
        }
        drop(p);
    }
    #[test]
    fn off_callback_faults_correlate_only_a_coherent_completed_prefix() {
        let stable = TapControl::new();
        stable.completed_version.store(14, Ordering::SeqCst);
        stable.completed_callback.store(7, Ordering::SeqCst);
        stable.completed_frames.store(100, Ordering::SeqCst);
        stable.cancel();
        assert_eq!(
            stable.first_fault().unwrap(),
            Some(TapFault {
                kind: TapFaultKind::Aborted,
                frame: 100,
                callback_index: Some(7)
            })
        );
        for abandoned in [false, true] {
            let busy = TapControl::new();
            busy.completed_version.store(15, Ordering::SeqCst);
            busy.completed_callback.store(7, Ordering::SeqCst);
            busy.completed_frames.store(200, Ordering::SeqCst);
            assert_eq!(busy.completed_prefix(), Err(TapError::Busy));
            if abandoned {
                busy.abandoned();
            } else {
                busy.cancel();
            }
            assert_eq!(
                busy.first_fault().unwrap(),
                Some(TapFault {
                    kind: if abandoned {
                        TapFaultKind::ReceiverAbandoned
                    } else {
                        TapFaultKind::Aborted
                    },
                    frame: 200,
                    callback_index: None
                })
            );
            // A later exact observation cannot overwrite the first cause.
            busy.latch(TapFaultKind::Overflow, 201, Some(8));
            assert_eq!(busy.first_fault().unwrap().unwrap().callback_index, None);
        }
    }
}
