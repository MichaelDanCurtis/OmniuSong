//! Shared read-only semantic verification of complete six-stream performance checkpoints.
//! Public hidden callback helpers support the engine writer; they confer no native seal authority.
use crate::{live_types::*, performance_records::*, performance_tap::*};
use omniusong_core::contracts::{CapturedEvent, Envelope};
use omniusong_recording::{
    CaptureReader, ChunkHeader, ChunkKind, ChunkRecord, InspectionBudget, RecordingError,
    RecoveryPrefix, SourcePosition, SourceRange, StreamPrefix, TakeDescriptor,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
type Result<T> = std::result::Result<T, String>;
#[doc(hidden)]
pub const MAX_GROUP_BYTES: usize = 8 * 1024 * 1024;
#[doc(hidden)]
pub const MAX_GROUP_MEMBERS: usize = 63;
fn check(ok: bool, reason: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(reason.into()) }
}
#[doc(hidden)]
pub fn initial_prefixes(descriptor: &TakeDescriptor) -> Vec<StreamPrefix> {
    descriptor
        .streams
        .iter()
        .map(|stream| StreamPrefix {
            stream_id: stream.stream_id.clone(),
            next_sequence: 0,
            source_end: if stream.stream_id == "audio" {
                SourcePosition::AudioFrames(0)
            } else {
                SourcePosition::Events(0)
            },
            recorded_units: 0,
            gap_units: 0,
            gap_chunks: 0,
        })
        .collect()
}
#[doc(hidden)]
pub fn advance(prefix: &mut StreamPrefix, header: &ChunkHeader) -> Result<()> {
    check(
        header.kind == ChunkKind::Content
            && prefix.stream_id == header.stream_id
            && prefix.next_sequence == header.sequence
            && prefix.source_end.value() == header.range.start(),
        "Performance chunk is not contiguous original content",
    )?;
    let end = header.range.end().map_err(|e| e.to_string())?;
    prefix.next_sequence = prefix
        .next_sequence
        .checked_add(1)
        .ok_or("Performance chunk sequence overflow")?;
    check(
        prefix.next_sequence <= i64::MAX as u64,
        "Performance sequence exceeds journal range",
    )?;
    prefix.source_end = match header.range {
        SourceRange::AudioFrames { .. } => SourcePosition::AudioFrames(end),
        SourceRange::Events { .. } => SourcePosition::Events(end),
    };
    prefix.recorded_units = prefix
        .recorded_units
        .checked_add(header.range.count())
        .ok_or("Performance prefix overflow")?;
    Ok(())
}
#[derive(Clone)]
#[doc(hidden)]
pub struct Context {
    pub scope: LiveScope,
    pub next_callback: u64,
    pub frame: u64,
    pub applied: Option<u64>,
    pub ticks: [Option<u64>; 4],
    pub capture_fault: Option<LiveFault>,
    pub capture_pending: bool,
    pub fault_cleared: bool,
    pub profile: PerformanceProfile,
    pub finish: Option<TapFinish>,
}
impl Context {
    pub fn new(info: TapInfo, profile: PerformanceProfile) -> Self {
        Self {
            scope: info.scope,
            next_callback: 1,
            frame: 0,
            applied: None,
            ticks: [None; 4],
            capture_fault: None,
            capture_pending: false,
            fault_cleared: false,
            profile,
            finish: None,
        }
    }
}
#[doc(hidden)]
pub struct Callback {
    pub index: u64,
    pub begin: TapBegin,
    pub next_ordinal: u16,
    pub batches: Vec<(u16, LiveBatch, BatchDisposition)>,
    pub pcm: Vec<u8>,
    pub frames: u64,
    pub finish: Option<TapFinish>,
}
impl Callback {
    pub fn new(
        packet: PerformancePacket,
        config: &PerformanceConfig,
        context: &Context,
    ) -> Result<Self> {
        let PerformancePacketKind::Begin(begin) = packet.kind else {
            return Err("Performance callback must start with Begin".into());
        };
        check(
            context.finish.is_none()
                && packet.callback_index == context.next_callback
                && packet.ordinal == 0
                && begin.scope == context.scope
                && begin.start_frame == context.frame
                && begin.sample_rate == config.info.sample_rate
                && begin.channels == config.info.channels
                && (1..=LIVE_MAX_CALLBACK_FRAMES).contains(&usize::from(begin.frames))
                && begin
                    .start_frame
                    .checked_add(u64::from(begin.frames))
                    .is_some_and(|e| e <= i64::MAX as u64),
            "Invalid performance Begin identity/format/extent",
        )?;
        Ok(Self {
            index: packet.callback_index,
            begin,
            next_ordinal: 1,
            batches: Vec::with_capacity(4),
            pcm: Vec::new(),
            frames: 0,
            finish: None,
        })
    }
    pub fn push(&mut self, packet: PerformancePacket) -> Result<Option<TapEnd>> {
        check(
            packet.callback_index == self.index
                && packet.ordinal == self.next_ordinal
                && usize::from(packet.ordinal) < TAP_CALLBACK_PACKETS,
            "Missing/reordered performance packet",
        )?;
        check(
            self.finish.is_none() || matches!(packet.kind, PerformancePacketKind::End(_)),
            "Only End may follow the callback Finish cut",
        )?;
        self.next_ordinal += 1;
        match packet.kind {
            PerformancePacketKind::Begin(_) => Err("Nested performance Begin".into()),
            PerformancePacketKind::Batch { batch, disposition } => {
                check(
                    self.frames == 0 && self.batches.len() < 4,
                    "Performance batch budget/order exceeded",
                )?;
                self.batches.push((packet.ordinal, batch, disposition));
                Ok(None)
            }
            PerformancePacketKind::Pcm(pcm) => {
                let count = usize::from(pcm.frames) * usize::from(pcm.channels);
                check(
                    (1..=TAP_PCM_FRAMES).contains(&usize::from(pcm.frames))
                        && pcm.channels == self.begin.channels
                        && pcm.start_frame == self.begin.start_frame + self.frames
                        && count <= pcm.samples.len()
                        && self.frames + u64::from(pcm.frames) <= u64::from(self.begin.frames)
                        && pcm.samples[..count].iter().all(|v| v.is_finite())
                        && pcm.samples[count..].iter().all(|v| v.to_bits() == 0),
                    "Invalid performance PCM coverage/format",
                )?;
                for value in &pcm.samples[..count] {
                    self.pcm.extend_from_slice(&value.to_le_bytes());
                }
                self.frames += u64::from(pcm.frames);
                Ok(None)
            }
            PerformancePacketKind::Finish(finish) => {
                check(
                    self.finish.is_none()
                        && self.frames == u64::from(self.begin.frames)
                        && finish.callback_index == self.index
                        && finish.cut_frame == self.begin.start_frame + self.frames,
                    "Finish must follow the complete callback PCM exactly once",
                )?;
                self.finish = Some(finish);
                Ok(None)
            }
            PerformancePacketKind::End(end) => {
                check(
                    self.frames == u64::from(self.begin.frames),
                    "Incomplete callback cannot receive End",
                )?;
                Ok(Some(end))
            }
        }
    }
    pub fn checked_context(&self, end: &TapEnd, context: &Context) -> Result<Context> {
        check(
            end.end_frame == self.begin.start_frame + u64::from(self.begin.frames)
                && end.scope_after.worker_generation == context.scope.worker_generation
                && end.scope_after.session_id == context.scope.session_id
                && (end.scope_after == context.scope
                    || context.scope.panic_generation.checked_add(1)
                        == Some(end.scope_after.panic_generation))
                && end.active_voices <= LIVE_VOICES as u16
                && end.release_tail_voices <= end.active_voices
                && usize::from(end.transition_len) <= TAP_TRANSITIONS,
            "Invalid callback End",
        )?;
        check(
            (context.capture_fault.is_none() && !context.capture_pending)
                || end.scope_after == context.scope,
            "A faulted performance cannot acknowledge a new Panic generation",
        )?;
        let mut next = context.clone();
        let mut gap = false;
        let mut panic = false;
        let mut fault_clear = false;
        for clear in end.clears {
            let Some(clear) = clear else {
                gap = true;
                continue;
            };
            check(
                !gap && clear.scope.worker_generation == context.scope.worker_generation
                    && clear.scope.session_id == context.scope.session_id
                    && (context.scope.panic_generation..=end.scope_after.panic_generation)
                        .contains(&clear.scope.panic_generation)
                    && (self.begin.start_frame..=end.end_frame).contains(&clear.frame),
                "Invalid callback clear",
            )?;
            if !matches!(clear.reason, TapClearReason::Panic { .. }) {
                check(
                    !context.fault_cleared && !fault_clear,
                    "Fault clear was duplicated or re-timed",
                )?;
                fault_clear = true;
                next.fault_cleared = true;
            }
            match clear.reason {
                TapClearReason::FaultPending => next.capture_pending = true,
                TapClearReason::Fault { kind } => check(
                    end.first_fault.is_some_and(|fault| fault.kind == kind),
                    "Resolved fault clear lacks its matching first cause",
                )?,
                TapClearReason::Panic { .. } => (),
            }
            if let TapClearReason::Panic { next_generation } = clear.reason {
                check(
                    clear.scope == context.scope
                        && clear.frame == self.begin.start_frame
                        && context.scope.panic_generation.checked_add(1) == Some(next_generation),
                    "Invalid Panic clear",
                )?;
                panic = true;
            }
        }
        let mut transition_frame = self.begin.start_frame;
        for (i, transition) in end.transitions.iter().enumerate() {
            if i >= usize::from(end.transition_len) {
                check(transition.is_none(), "Nonempty transition tail")?;
                continue;
            }
            let transition = transition.ok_or("Missing callback transition")?;
            check(
                transition.scope == end.scope_after
                    && (1..=4).contains(&transition.source)
                    && transition.voice > 0
                    && (self.begin.start_frame..end.end_frame).contains(&transition.frame)
                    && transition.frame >= transition_frame,
                "Invalid callback transition source/time",
            )?;
            transition_frame = transition.frame;
        }
        for (_, batch, disposition) in &self.batches {
            match disposition {
                BatchDisposition::Applied => {
                    check(
                        context.capture_fault.is_none() && !context.capture_pending,
                        "Faulted performance cannot resume event application",
                    )?;
                    batch
                        .validate_shape(self.begin.sample_rate)
                        .map_err(|e| e.to_string())?;
                    check(
                        batch.scope == context.scope
                            && next.applied.map_or(Some(1), |n| n.checked_add(1))
                                == Some(batch.sequence),
                        "Applied batch sequence/scope mismatch",
                    )?;
                    for event in batch.events[..usize::from(batch.len)].iter().flatten() {
                        check(
                            (1..=4).contains(&event.source) && event.connection_generation == 1,
                            "Applied event is not a prepared GUI source",
                        )?;
                        let ticks = &mut next.ticks[usize::from(event.source - 1)];
                        check(
                            ticks.is_none_or(|old| event.observed_ticks >= old),
                            "Applied source clock regressed",
                        )?;
                        *ticks = Some(event.observed_ticks);
                    }
                    next.applied = Some(batch.sequence);
                }
                BatchDisposition::DiscardedStale => check(
                    batch.scope != context.scope,
                    "Stale disposition has current scope",
                )?,
                BatchDisposition::DiscardedPanic => {
                    check(panic, "Panic-discarded batch has no Panic clear")?
                }
                BatchDisposition::Rejected { fault } => check(
                    end.first_fault.is_some_and(|f| f.kind == *fault),
                    "Rejected batch lacks matching first fault",
                )?,
            }
        }
        if end.scope_after != context.scope {
            check(panic, "Scope advanced without Panic")?;
            next.applied = None;
        }
        check(
            end.applied_sequence == next.applied,
            "End applied watermark differs from retained originals",
        )?;
        if let Some(fault) = end.first_fault {
            check(
                fault.scope.worker_generation == context.scope.worker_generation
                    && fault.scope.session_id == context.scope.session_id
                    && fault.scope.panic_generation <= end.scope_after.panic_generation
                    && fault.frame <= end.end_frame
                    && fault.sequence != Some(0)
                    && context.capture_fault.is_none_or(|old| old == fault),
                "Changed/invalid first capture fault",
            )?;
            next.capture_fault = Some(fault);
            next.capture_pending = false;
        } else {
            check(
                context.capture_fault.is_none(),
                "First capture fault disappeared",
            )?;
        }
        if let Some(finish) = self.finish {
            check(
                context.profile == PerformanceProfile::FinishV2
                    && context.finish.is_none()
                    && finish.fence.id.is_valid()
                    && finish.fence.scope == self.begin.scope
                    && finish.fence.scope == end.scope_after
                    && finish.fence.accepted_sequence == next.applied
                    && finish.fence.accepted_sequence != Some(0)
                    && finish.callback_index == self.index
                    && finish.cut_frame == end.end_frame
                    && finish.policy == LiveFinishPolicy::CutAtCompleteCallbackEnd
                    && finish.voices_cut <= LIVE_VOICES as u16
                    && finish.release_tails_cut <= finish.voices_cut
                    && end.active_voices == 0
                    && end.release_tail_voices == 0
                    && !panic
                    && end.clears.iter().all(Option::is_none)
                    && next.capture_fault.is_none()
                    && !next.capture_pending
                    && self
                        .batches
                        .iter()
                        .all(|(_, _, d)| *d == BatchDisposition::Applied),
                "Finish contradicts its profile, applied fence, healthy callback or hard cut",
            )?;
            next.finish = Some(finish);
        }
        next.scope = end.scope_after;
        next.frame = end.end_frame;
        next.next_callback = next
            .next_callback
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or("Callback counter overflow")?;
        Ok(next)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceInspection {
    pub descriptor_sha256: String,
    pub original_rows_sha256: String,
    pub journal_sha256_at_review: String,
    pub selection: Option<RecoveryPrefix>,
    pub checkpoint_sha256: Option<String>,
    pub checkpoint: Option<PerformanceCheckpoint>,
    pub received_generic_chunks: u64,
    pub unknown_tail: bool,
    pub problem: Option<String>,
    pub capture_fault: Option<LiveFault>,
    pub capture_fault_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_cut: Option<TapFinish>,
}

// Keep governor refusals typed through the corrupt-tail diagnostic boundary.
// Legacy callers retain their String API; only the explicit bounded entrypoints
// promise one cumulative reader/semantic governor and these outer stop errors.
#[derive(Debug)]
#[doc(hidden)]
pub enum InspectionFailure {
    Semantic(String),
    Stopped(RecordingError),
}
impl From<String> for InspectionFailure {
    fn from(value: String) -> Self {
        Self::Semantic(value)
    }
}
impl From<&str> for InspectionFailure {
    fn from(value: &str) -> Self {
        Self::Semantic(value.into())
    }
}
impl From<RecordingError> for InspectionFailure {
    fn from(value: RecordingError) -> Self {
        match value {
            RecordingError::InspectionCancelled | RecordingError::InspectionLimit { .. } => {
                Self::Stopped(value)
            }
            error => Self::Semantic(error.to_string()),
        }
    }
}
impl std::fmt::Display for InspectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Semantic(message) => f.write_str(message),
            Self::Stopped(error) => std::fmt::Display::fmt(error, f),
        }
    }
}
impl InspectionFailure {
    pub fn into_recording(self) -> RecordingError {
        match self {
            Self::Semantic(message) => RecordingError::Invalid(message),
            Self::Stopped(error) => error,
        }
    }
}
#[doc(hidden)]
pub type InspectionResult<T> = std::result::Result<T, InspectionFailure>;
fn inspection_checkpoint(budget: Option<&InspectionBudget>) -> InspectionResult<()> {
    if let Some(budget) = budget {
        budget.checkpoint()?;
    }
    Ok(())
}
fn charge_decode(budget: Option<&InspectionBudget>, bytes: usize) -> InspectionResult<()> {
    if let Some(budget) = budget {
        budget.charge_work(
            u64::try_from(bytes).map_err(|_| "Performance decode byte count overflow")?,
        )?;
        budget.visit_records(1)?;
    }
    Ok(())
}

type DecodedRecord = (Vec<u8>, Envelope<CapturedEvent>, LocalRecord);
fn records(
    payload: &[u8],
    budget: Option<&InspectionBudget>,
) -> InspectionResult<Vec<DecodedRecord>> {
    inspection_checkpoint(budget)?;
    if let Some(budget) = budget {
        budget.charge_work(
            u64::try_from(payload.len()).map_err(|_| "Performance payload byte count overflow")?,
        )?;
    }
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < payload.len() {
        check(
            payload.len() - offset >= 4,
            "Truncated performance record length",
        )?;
        let len =
            u32::from_le_bytes(payload[offset..offset + 4].try_into().expect("length")) as usize;
        offset += 4;
        check(
            len > 0
                && len <= omniusong_core::contracts::CAPTURE_MAX_BYTES
                && len <= payload.len() - offset,
            "Truncated/oversized performance record",
        )?;
        // Payload work was charged before parsing; charge each record before
        // its owned byte copy, envelope decode or owned-extension allocation.
        if let Some(budget) = budget {
            budget.visit_records(1)?;
        }
        let bytes = payload[offset..offset + len].to_vec();
        let (envelope, record) = decode_envelope(&bytes)?;
        out.push((bytes, envelope, record));
        offset += len;
    }
    Ok(out)
}
#[doc(hidden)]
pub fn contextual_records(
    config: &PerformanceConfig,
    profile: PerformanceProfile,
    index: usize,
    chunk: &ChunkRecord,
    budget: Option<&InspectionBudget>,
) -> InspectionResult<Vec<(Vec<u8>, LocalRecord)>> {
    check(
        chunk.header.clock_anchors.is_empty(),
        "Performance originals have no inferred clock anchors",
    )?;
    let mut result = Vec::new();
    for (ordinal, (bytes, envelope, record)) in
        records(&chunk.payload, budget)?.into_iter().enumerate()
    {
        inspection_checkpoint(budget)?;
        check(
            record.version == profile.version(),
            "Record profile differs from its declared original",
        )?;
        let expected = encode_record(
            config,
            index,
            &record,
            envelope.payload.time.observed.ticks,
            chunk.header.range.start() + ordinal as u64,
        )?;
        check(
            bytes == expected,
            "Owned performance envelope differs from exact declared source/clock/identity",
        )?;
        result.push((bytes, record));
    }
    check(
        result.len() as u64 == chunk.header.range.count(),
        "Raw performance range count differs",
    )?;
    Ok(result)
}
#[doc(hidden)]
pub fn verify_audio_bus_chunk(
    chunk: &ChunkRecord,
    budget: Option<&InspectionBudget>,
) -> InspectionResult<()> {
    // Loading authenticated the bytes and SHA; finite-f32 validation
    // is another full payload traversal under the same governor.
    if let Some(budget) = budget {
        budget.charge_work(
            u64::try_from(chunk.payload.len()).map_err(|_| "PCM decode byte count overflow")?,
        )?;
    }
    check(
        chunk.header.clock_anchors.is_empty()
            && chunk.header.observed_start.ticks == chunk.header.range.start()
            && chunk.header.observed_end.ticks
                == chunk.header.range.end().map_err(|e| e.to_string())?
            && chunk.payload.as_chunks::<4>().1.is_empty()
            && chunk
                .payload
                .as_chunks::<4>()
                .0
                .iter()
                .all(|b| f32::from_le_bytes(*b).is_finite()),
        "Audio bus clock/PCM mismatch",
    )?;
    Ok(())
}

fn verify_group(
    config: &PerformanceConfig,
    profile: PerformanceProfile,
    members: &[ChunkRecord],
    checkpoint: &PerformanceCheckpoint,
    before: &[StreamPrefix],
    context: &Context,
    budget: Option<&InspectionBudget>,
) -> InspectionResult<Context> {
    inspection_checkpoint(budget)?;
    let mut actual_gui: [Vec<Vec<u8>>; 4] = std::array::from_fn(|_| Vec::new());
    let mut expected_gui: [Vec<Vec<u8>>; 4] = std::array::from_fn(|_| Vec::new());
    let mut audio = Vec::new();
    let mut controls = Vec::new();
    for chunk in members {
        inspection_checkpoint(budget)?;
        let index = STREAM_IDS
            .iter()
            .position(|s| *s == chunk.header.stream_id)
            .ok_or("Unknown performance stream")?;
        if index == 0 {
            verify_audio_bus_chunk(chunk, budget)?;
            audio.push((chunk.header.range.start(), chunk.header.range.count()));
            continue;
        }
        check(
            chunk.header.clock_anchors.is_empty(),
            "Performance originals have no inferred clock anchors",
        )?;
        let decoded = contextual_records(config, profile, index, chunk, budget)?;
        if index < 5 {
            for (bytes, record) in decoded {
                check(
                    matches!(record.record, PerformanceRecord::AppliedEvent { .. }),
                    "GUI source contains a non-applied record",
                )?;
                actual_gui[index - 1].push(bytes);
            }
        } else {
            for (bytes, record) in decoded {
                charge_decode(budget, bytes.len())?;
                let observed = decode_envelope(&bytes)?.0.payload.time.observed.ticks;
                controls.push((record.record, observed));
            }
        }
    }
    let mut next = context.clone();
    let mut callback: Option<Callback> = None;
    let mut clears = Vec::new();
    let mut transitions = Vec::new();
    let mut expected_audio = Vec::new();
    for (record, observed) in controls {
        inspection_checkpoint(budget)?;
        match record {
            PerformanceRecord::Begin {
                callback_index,
                begin,
            } => {
                check(callback.is_none(), "Nested control callback")?;
                callback = Some(Callback::new(
                    PerformancePacket {
                        callback_index,
                        ordinal: 0,
                        kind: PerformancePacketKind::Begin(begin),
                    },
                    config,
                    &next,
                )?);
                clears.clear();
                transitions.clear();
            }
            PerformanceRecord::Batch {
                callback_index,
                packet_ordinal,
                batch,
                disposition,
            } => {
                let cb = callback.as_mut().ok_or("Batch outside callback")?;
                check(
                    observed == cb.begin.start_frame
                        && clears.is_empty()
                        && transitions.is_empty()
                        && cb.finish.is_none(),
                    "Batch observation differs from its exact callback boundary",
                )?;
                cb.push(PerformancePacket {
                    callback_index,
                    ordinal: packet_ordinal,
                    kind: PerformancePacketKind::Batch {
                        batch: *batch,
                        disposition,
                    },
                })?;
            }
            PerformanceRecord::Clear {
                callback_index,
                clear,
            } => {
                check(
                    callback
                        .as_ref()
                        .is_some_and(|c| c.index == callback_index && c.finish.is_none())
                        && clears.len() < 2,
                    "Unexpected clear record",
                )?;
                clears.push(clear);
            }
            PerformanceRecord::Transition {
                callback_index,
                transition,
            } => {
                check(
                    callback
                        .as_ref()
                        .is_some_and(|c| c.index == callback_index && c.finish.is_none())
                        && transitions.len() < TAP_TRANSITIONS,
                    "Unexpected transition record",
                )?;
                transitions.push(transition);
            }
            PerformanceRecord::Finish {
                callback_index,
                finish,
            } => {
                let cb = callback.as_mut().ok_or("Finish outside callback")?;
                check(
                    profile == PerformanceProfile::FinishV2
                        && cb.finish.is_none()
                        && cb.index == callback_index
                        && finish.callback_index == callback_index
                        && observed == finish.cut_frame
                        && finish.cut_frame == cb.begin.start_frame + u64::from(cb.begin.frames),
                    "Unexpected, duplicate or re-timed Finish record",
                )?;
                cb.finish = Some(finish);
            }
            PerformanceRecord::End {
                callback_index,
                end_frame,
                scope_after,
                applied_sequence,
                active_voices,
                release_tail_voices,
                clear_count,
                transition_count,
                first_fault,
            } => {
                let mut cb = callback.take().ok_or("End outside callback")?;
                check(
                    cb.index == callback_index
                        && usize::from(clear_count) == clears.len()
                        && usize::from(transition_count) == transitions.len(),
                    "End transition/clear count mismatch",
                )?;
                cb.frames = u64::from(cb.begin.frames);
                let mut end = TapEnd {
                    end_frame,
                    scope_after,
                    applied_sequence,
                    active_voices,
                    release_tail_voices,
                    clears: [None; 2],
                    transitions: [None; TAP_TRANSITIONS],
                    transition_len: transition_count,
                    first_fault,
                };
                for (dst, value) in end.clears.iter_mut().zip(&clears) {
                    *dst = Some(*value);
                }
                for (dst, value) in end.transitions.iter_mut().zip(&transitions) {
                    *dst = Some(*value);
                }
                next = cb.checked_context(&end, &next)?;
                expected_audio.push((cb.begin.start_frame, u64::from(cb.begin.frames)));
                for (batch_ordinal, batch, disposition) in &cb.batches {
                    if *disposition != BatchDisposition::Applied {
                        continue;
                    }
                    for (event_index, original) in
                        batch.events[..usize::from(batch.len)].iter().enumerate()
                    {
                        let original = original.ok_or("Missing applied source event")?;
                        let index = usize::from(original.source);
                        let record = LocalRecord {
                            version: profile.version(),
                            record: PerformanceRecord::AppliedEvent {
                                callback_index: cb.index,
                                batch_ordinal: *batch_ordinal,
                                event_index: event_index as u8,
                                scope: batch.scope,
                                batch_sequence: batch.sequence,
                                applied_frame: cb.begin.start_frame,
                                original,
                            },
                        };
                        let ordinal =
                            before[index].source_end.value() + expected_gui[index - 1].len() as u64;
                        expected_gui[index - 1].push(encode_record(
                            config,
                            index,
                            &record,
                            original.observed_ticks,
                            ordinal,
                        )?);
                    }
                }
            }
            _ => return Err("Unexpected control record inside checkpoint group".into()),
        }
    }
    check(
        callback.is_none() && !expected_audio.is_empty() && audio == expected_audio,
        "Checkpoint does not cover complete matching audio callbacks",
    )?;
    check(
        actual_gui == expected_gui,
        "Applied GUI originals differ from complete callback batch originals",
    )?;
    check(
        checkpoint.first_callback_index == context.next_callback
            && checkpoint.last_callback_index == next.next_callback - 1
            && checkpoint.audio_end == next.frame
            && checkpoint.scope_after == next.scope,
        "Checkpoint callback/frame/scope differs from complete originals",
    )?;
    Ok(next)
}

/// Read-only semantic review. A generic chunk prefix is NOT a performance
/// checkpoint; preserve all rows while selecting the last complete six-stream group.
pub fn inspect_performance(reader: &CaptureReader) -> Result<PerformanceInspection> {
    if reader.inspection_budget().is_some() {
        return inspect_performance_bounded(reader).map_err(|error| error.to_string());
    }
    inspect_performance_snapshot(reader.inspection(), |ordinal| {
        reader.read_ordinal(ordinal).map_err(|e| e.to_string())
    })
}
/// Format-aware read-only review of an already sealed/owned original. This
/// shares the exact checkpoint/record validator with recovery inspection but
/// cannot acquire native lifetime authority or mutate/recover the source.
pub fn inspect_sealed_performance(
    reader: &omniusong_recording::SealedJournal,
) -> Result<PerformanceInspection> {
    if reader.inspection_budget().is_some() {
        return inspect_sealed_performance_bounded(reader).map_err(|error| error.to_string());
    }
    let inspection = reader.inspect_snapshot().map_err(|e| e.to_string())?;
    inspect_performance_snapshot(&inspection, |ordinal| {
        reader.read_ordinal(ordinal).map_err(|e| e.to_string())
    })
}
/// Read-only semantic inspection governed by the exact budget used to open the
/// reader. Cancellation/limits are outer typed errors, never usable prefixes.
pub fn inspect_performance_bounded(
    reader: &CaptureReader,
) -> omniusong_recording::Result<PerformanceInspection> {
    let budget = reader.inspection_budget().ok_or_else(|| {
        RecordingError::Invalid(
            "Bounded performance inspection requires a bounded capture reader".into(),
        )
    })?;
    budget.checkpoint()?;
    let result =
        inspect_performance_snapshot_bounded(reader.inspection(), Some(budget), |ordinal| {
            reader
                .read_ordinal(ordinal)
                .map_err(InspectionFailure::from)
        });
    budget.checkpoint()?;
    result.map_err(InspectionFailure::into_recording)
}
/// Bounded format-aware inspection of a sealed source. Its snapshot, all
/// rereads and semantic decodes retain the same cumulative reader governor.
pub fn inspect_sealed_performance_bounded(
    reader: &omniusong_recording::SealedJournal,
) -> omniusong_recording::Result<PerformanceInspection> {
    let budget = reader.inspection_budget().ok_or_else(|| {
        RecordingError::Invalid(
            "Bounded performance inspection requires a bounded sealed reader".into(),
        )
    })?;
    budget.checkpoint()?;
    let inspection = reader.inspect_snapshot()?;
    let result = inspect_performance_snapshot_bounded(&inspection, Some(budget), |ordinal| {
        reader
            .read_ordinal(ordinal)
            .map_err(InspectionFailure::from)
    });
    budget.checkpoint()?;
    if result.is_ok() {
        reader.verify_source_identity()?;
    }
    result.map_err(InspectionFailure::into_recording)
}
/// Shared semantic validator for a held action-owned immutable image. Callers
/// must retain the exact reader/governor and its owner for the entire pass.
#[doc(hidden)]
pub fn inspect_snapshot_with_budget(
    inspection: &omniusong_recording::JournalInspection,
    budget: &InspectionBudget,
    mut read_ordinal: impl FnMut(u64) -> omniusong_recording::Result<ChunkRecord>,
) -> omniusong_recording::Result<PerformanceInspection> {
    budget.checkpoint()?;
    let result = inspect_performance_snapshot_bounded(inspection, Some(budget), |ordinal| {
        read_ordinal(ordinal).map_err(InspectionFailure::from)
    });
    budget.checkpoint()?;
    result.map_err(InspectionFailure::into_recording)
}
#[doc(hidden)]
pub fn inspect_performance_snapshot(
    inspection: &omniusong_recording::JournalInspection,
    mut read_ordinal: impl FnMut(u64) -> Result<ChunkRecord>,
) -> Result<PerformanceInspection> {
    inspect_performance_snapshot_bounded(inspection, None, |ordinal| {
        read_ordinal(ordinal).map_err(InspectionFailure::Semantic)
    })
    .map_err(|error| error.to_string())
}
#[doc(hidden)]
pub fn inspect_performance_snapshot_bounded(
    inspection: &omniusong_recording::JournalInspection,
    budget: Option<&InspectionBudget>,
    mut read_ordinal: impl FnMut(u64) -> InspectionResult<ChunkRecord>,
) -> InspectionResult<PerformanceInspection> {
    inspection_checkpoint(budget)?;
    let config = from_descriptor(&inspection.descriptor)?;
    let profile = descriptor_profile(&inspection.descriptor)?;
    let mut result = PerformanceInspection {
        descriptor_sha256: inspection.descriptor_sha256.clone(),
        original_rows_sha256: inspection.original_rows_sha256.clone(),
        journal_sha256_at_review: inspection.journal_sha256.clone(),
        selection: None,
        checkpoint_sha256: None,
        checkpoint: None,
        received_generic_chunks: inspection.report.verified_chunks,
        unknown_tail: true,
        problem: None,
        capture_fault: None,
        capture_fault_pending: false,
        finish_cut: None,
    };
    let mut context = Context::new(config.info, profile);
    let mut prefixes = initial_prefixes(&inspection.descriptor);
    let mut members = Vec::new();
    let mut group_bytes = 0usize;
    let mut next_group = 1u64;
    let mut index_hash = Sha256::new();
    index_hash.update(b"OmniuSong.recording-index.v1\0");
    index_hash.update(inspection.descriptor_sha256.as_bytes());
    for ordinal in 1..=inspection.report.verified_chunks {
        inspection_checkpoint(budget)?;
        let checked = (|| -> InspectionResult<()> {
            let chunk = read_ordinal(ordinal)?;
            let index = STREAM_IDS
                .iter()
                .position(|s| *s == chunk.header.stream_id)
                .ok_or("Unknown performance stream")?;
            group_bytes = group_bytes
                .checked_add(
                    chunk
                        .header
                        .canonical_bytes()
                        .map_err(|e| e.to_string())?
                        .len()
                        + chunk.payload.len(),
                )
                .ok_or("Group bytes overflow")?;
            check(
                group_bytes <= MAX_GROUP_BYTES,
                "Performance group exceeds8MiB",
            )?;
            let decoded = if index == 5 {
                contextual_records(&config, profile, index, &chunk, budget)?
            } else {
                Vec::new()
            };
            let checkpoint = if decoded.len() == 1 {
                if let PerformanceRecord::Checkpoint { checkpoint } = &decoded[0].1.record {
                    Some(checkpoint.clone())
                } else {
                    None
                }
            } else {
                None
            };
            index_hash.update(ordinal.to_le_bytes());
            index_hash.update(chunk.sha256.as_bytes());
            if checkpoint.is_none() {
                check(
                    members.len() < MAX_GROUP_MEMBERS
                        && !decoded
                            .iter()
                            .any(|(_, r)| matches!(r.record, PerformanceRecord::Checkpoint { .. })),
                    "Checkpoint must be a final singleton chunk",
                )?;
                // The authenticated record is already owned. Retain it until
                // its checkpoint without copying every PCM payload again.
                members.push(chunk);
                return Ok(());
            }
            if let Some(checkpoint) = checkpoint {
                if let Some(budget) = budget {
                    budget.visit_groups(1)?;
                }
                check(
                    checkpoint.group_sequence == next_group
                        && checkpoint.previous_checkpoint_sha256 == result.checkpoint_sha256,
                    "Checkpoint predecessor/sequence mismatch",
                )?;
                let expected: Vec<_> = members
                    .iter()
                    .map(|c| PerformanceChunkIdentity {
                        stream_id: c.header.stream_id.clone(),
                        sequence: c.header.sequence,
                        sha256: c.sha256.clone(),
                    })
                    .collect();
                check(
                    checkpoint.chunks == expected,
                    "Checkpoint member identities/hashes differ",
                )?;
                let mut predicted = prefixes.clone();
                for member in members.iter().chain(std::iter::once(&chunk)) {
                    let i = STREAM_IDS
                        .iter()
                        .position(|s| *s == member.header.stream_id)
                        .ok_or("Unknown member stream")?;
                    advance(&mut predicted[i], &member.header)?;
                }
                check(
                    checkpoint.prefixes == predicted,
                    "Checkpoint six-stream prefix differs",
                )?;
                check(
                    chunk.header.observed_start.ticks == checkpoint.audio_end
                        && chunk.header.observed_end.ticks == checkpoint.audio_end,
                    "Checkpoint clock differs from rendered frame axis",
                )?;
                context = verify_group(
                    &config,
                    profile,
                    &members,
                    &checkpoint,
                    &prefixes,
                    &context,
                    budget,
                )?;
                result.capture_fault = context.capture_fault;
                result.capture_fault_pending = context.capture_pending;
                result.finish_cut = context.finish;
                result.selection = Some(RecoveryPrefix {
                    chunk_count: ordinal,
                    chunk_index_sha256: format!("{:x}", index_hash.clone().finalize()),
                });
                // checkpoint_sha256 validates the exact envelope again, then
                // hashes its bytes; both passes count against cumulative work.
                charge_decode(budget, decoded[0].0.len())?;
                if let Some(budget) = budget {
                    budget.charge_work(
                        u64::try_from(decoded[0].0.len())
                            .map_err(|_| "Checkpoint byte count overflow")?,
                    )?;
                }
                result.checkpoint_sha256 = Some(checkpoint_sha256(&decoded[0].0)?);
                result.checkpoint = Some(checkpoint);
                prefixes = predicted;
                members.clear();
                group_bytes = 0;
                next_group = next_group
                    .checked_add(1)
                    .ok_or("Checkpoint sequence overflow")?;
            }
            Ok(())
        })();
        // A concurrent cancellation or sticky governor refusal wins even if an
        // inner semantic validator also found an ordinary corrupt tail.
        inspection_checkpoint(budget)?;
        if let Err(error) = checked {
            match error {
                InspectionFailure::Stopped(error) => return Err(InspectionFailure::Stopped(error)),
                InspectionFailure::Semantic(error) => {
                    result.problem = Some(error);
                    break;
                }
            }
        }
    }
    if result.problem.is_none() && !members.is_empty() {
        result.problem = Some(
            "Retained uncheckpointed tail is excluded from the complete performance prefix".into(),
        );
    }
    if result.problem.is_none() && !inspection.report.problems.is_empty() {
        result.problem = Some("Generic original has a damaged or discontinuous tail".into());
    }
    if result.selection.is_none() && result.problem.is_none() {
        result.problem = Some("No complete performance checkpoint is available".into());
    }
    inspection_checkpoint(budget)?;
    Ok(result)
}
