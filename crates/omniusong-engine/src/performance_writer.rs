//! Off-callback incremental performance originals and callback Finish evidence.
//! No device lifecycle or normal-seal authority is supplied here.
use crate::{live_types::*, performance_records::*, performance_tap::*};
use omniusong_core::contracts::{CapturedEvent, Envelope, HostTimestamp};
use omniusong_recording::{
    AppendChunk, CHUNK_VERSION, CaptureReader, ChunkHeader, ChunkKind, ChunkRecord, CompletePrefix,
    InspectionBudget, Journal, MAX_AUDIO_CHUNK_BYTES, MAX_EVENT_CHUNK_BYTES, RecordingError,
    RecoveryPrefix, SealedManifest, SourcePosition, SourceRange, StreamPrefix, TakeDescriptor,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, String>;
const MAX_CALLBACK_BYTES: usize = 2 * 1024 * 1024;
const MAX_GROUP_BYTES: usize = 8 * 1024 * 1024;
const MAX_GROUP_MEMBERS: usize = 63; // One final singleton checkpoint is reserved.
const MAX_GROUP_CALLBACKS: u64 = 64;
fn check(ok: bool, reason: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(reason.into()) }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn initial_prefixes(descriptor: &TakeDescriptor) -> Vec<StreamPrefix> {
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
fn advance(prefix: &mut StreamPrefix, header: &ChunkHeader) -> Result<()> {
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
struct OwnedChunk {
    header: ChunkHeader,
    payload: Vec<u8>,
}
impl OwnedChunk {
    fn identity(&self) -> Result<PerformanceChunkIdentity> {
        Ok(PerformanceChunkIdentity {
            stream_id: self.header.stream_id.clone(),
            sequence: self.header.sequence,
            sha256: self
                .header
                .sha256(&self.payload)
                .map_err(|e| e.to_string())?,
        })
    }
    fn bytes(&self) -> Result<usize> {
        Ok(self
            .header
            .canonical_bytes()
            .map_err(|e| e.to_string())?
            .len()
            + self.payload.len())
    }
}
#[derive(Clone)]
struct Context {
    scope: LiveScope,
    next_callback: u64,
    frame: u64,
    applied: Option<u64>,
    ticks: [Option<u64>; 4],
    capture_fault: Option<LiveFault>,
    capture_pending: bool,
    fault_cleared: bool,
    profile: PerformanceProfile,
    finish: Option<TapFinish>,
}
impl Context {
    fn new(info: TapInfo, profile: PerformanceProfile) -> Self {
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
struct Callback {
    index: u64,
    begin: TapBegin,
    next_ordinal: u16,
    batches: Vec<(u16, LiveBatch, BatchDisposition)>,
    pcm: Vec<u8>,
    frames: u64,
    finish: Option<TapFinish>,
}
impl Callback {
    fn new(
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
    fn push(&mut self, packet: PerformancePacket) -> Result<Option<TapEnd>> {
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
    fn checked_context(&self, end: &TapEnd, context: &Context) -> Result<Context> {
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WriterProgress {
    pub received_packets: u64,
    pub complete_callbacks: u64,
    pub complete_frames: u64,
    pub durable_callbacks: u64,
    pub durable_frames: u64,
    pub durable_prefixes: Vec<StreamPrefix>,
    pub checkpoint_sha256: Option<String>,
    pub first_error: Option<String>,
    pub capture_fault: Option<LiveFault>,
    pub capture_fault_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_cut: Option<TapFinish>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetainedPerformance {
    pub path: PathBuf,
    pub descriptor_sha256: String,
    pub progress: WriterProgress,
    pub unknown_tail: bool,
}
pub struct PerformanceWriter {
    journal: Journal,
    config: PerformanceConfig,
    profile: PerformanceProfile,
    descriptor: TakeDescriptor,
    context: Context,
    current: Option<Callback>,
    prefixes: Vec<StreamPrefix>,
    members: Vec<OwnedChunk>,
    first_group_callback: u64,
    progress: WriterProgress,
    group_sequence: u64,
    pending_request: Option<(Vec<OwnedChunk>, PerformanceCheckpoint, String)>,
    // Retained before the final transaction, including committed-but-unacked
    // failures. Never reconstruct from a shorter/recovered prefix on retry.
    pending_seal: Option<(String, CompletePrefix)>,
}
impl PerformanceWriter {
    pub fn create(path: &Path, config: PerformanceConfig) -> Result<Self> {
        Self::create_for(path, config, PerformanceProfile::OriginalV1)
    }
    /// Explicit v2 profile; only this profile can retain a callback Finish cut.
    /// This does not authorize a normal seal or prove native closure retirement.
    pub fn create_finishable(path: &Path, config: PerformanceConfig) -> Result<Self> {
        Self::create_for(path, config, PerformanceProfile::FinishV2)
    }
    fn create_for(
        path: &Path,
        config: PerformanceConfig,
        profile: PerformanceProfile,
    ) -> Result<Self> {
        let descriptor = create_descriptor_for(&config, profile)?;
        let prefixes = initial_prefixes(&descriptor);
        let journal = Journal::create(path, &descriptor).map_err(|e| e.to_string())?;
        Ok(Self {
            context: Context::new(config.info, profile),
            journal,
            config,
            profile,
            descriptor,
            current: None,
            prefixes: prefixes.clone(),
            members: Vec::new(),
            first_group_callback: 1,
            progress: WriterProgress {
                received_packets: 0,
                complete_callbacks: 0,
                complete_frames: 0,
                durable_callbacks: 0,
                durable_frames: 0,
                durable_prefixes: prefixes,
                checkpoint_sha256: None,
                first_error: None,
                capture_fault: None,
                capture_fault_pending: false,
                finish_cut: None,
            },
            group_sequence: 0,
            pending_request: None,
            pending_seal: None,
        })
    }
    pub fn path(&self) -> &Path {
        self.journal.path()
    }
    pub fn descriptor(&self) -> &TakeDescriptor {
        &self.descriptor
    }
    pub fn progress(&self) -> WriterProgress {
        self.progress.clone()
    }
    pub(crate) fn seal_native(
        &mut self,
        permit: &crate::performance_session::NativeSealPermit,
    ) -> Result<crate::CapturedTake> {
        let cut = permit.cut();
        let descriptor_hash = self.descriptor.sha256().map_err(|e| e.to_string())?;
        check(
            self.profile == PerformanceProfile::FinishV2
                && self.descriptor.take_id.0 == permit.take_id()
                && descriptor_hash == permit.descriptor_sha256()
                && self.context.finish == Some(cut)
                && self.progress.finish_cut == Some(cut)
                && self.context.scope == cut.fence.scope
                && self.context.applied == cut.fence.accepted_sequence
                && self.context.frame == cut.cut_frame
                && self.current.is_none()
                && self.pending_request.is_none()
                && self.members.is_empty()
                && self.progress.first_error.is_none()
                && self.progress.capture_fault.is_none()
                && !self.progress.capture_fault_pending
                && self.progress.complete_frames == cut.cut_frame
                && self.progress.durable_frames == cut.cut_frame
                && self.progress.complete_callbacks == cut.callback_index
                && self.progress.durable_callbacks == cut.callback_index
                && self.progress.checkpoint_sha256.is_some(),
            "Normal performance sealing requires the exact healthy durable Finish",
        )?;
        if self.pending_seal.is_none() {
            let inspection = {
                let view = self.journal.inspect_owned().map_err(|e| e.to_string())?;
                inspect_performance_snapshot(view.inspection(), |ordinal| {
                    view.read_ordinal(ordinal).map_err(|e| e.to_string())
                })?
            };
            let complete = self.journal.complete_prefix().map_err(|e| e.to_string())?;
            check(
                inspection.problem.is_none()
                    && inspection.descriptor_sha256 == descriptor_hash
                    && inspection.original_rows_sha256 == complete.original_rows_sha256
                    && inspection.selection.as_ref() == Some(&complete.prefix)
                    && inspection.finish_cut == Some(cut)
                    && inspection.checkpoint_sha256 == self.progress.checkpoint_sha256,
                "Normal performance full-prefix semantic review differs",
            )?;
            let mut digest = Sha256::new();
            digest.update(b"OmniuSong.native-performance-finish.v2\0");
            for field in [
                serde_json::to_vec(&permit.start_id()).map_err(|e| e.to_string())?,
                serde_json::to_vec(&cut).map_err(|e| e.to_string())?,
                serde_json::to_vec(&complete).map_err(|e| e.to_string())?,
                self.progress
                    .checkpoint_sha256
                    .as_ref()
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ] {
                digest.update((field.len() as u64).to_le_bytes());
                digest.update(field);
            }
            self.pending_seal = Some((format!("native-finish:{:x}", digest.finalize()), complete));
        }
        let (operation, complete) = self.pending_seal.as_ref().unwrap();
        // The concrete native owner minted this permit only after actual
        // closure retirement. The cancellation CAS is the final seal gate.
        permit.authorize_seal()?;
        let manifest = self
            .journal
            .seal_complete_prefix(operation, complete)
            .map_err(|e| e.to_string())?;
        check(
            manifest.body.interrupted.is_none()
                && manifest.body.descriptor_sha256 == descriptor_hash
                && manifest.body.chunk_count == complete.prefix.chunk_count
                && manifest.body.chunk_index_sha256 == complete.prefix.chunk_index_sha256,
            "Normal performance manifest differs from the retained request",
        )?;
        Ok(crate::CapturedTake {
            journal_path: self.path().to_owned(),
            take_id: self.descriptor.take_id.0.clone(),
            manifest_sha256: manifest.sha256,
            frames: cut.cut_frame,
            sample_rate: self.config.info.sample_rate,
            channels: self.config.info.channels,
            gap_frames: 0,
        })
    }
    fn latch<T>(&mut self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result
            && self.progress.first_error.is_none()
        {
            self.progress.first_error = Some(error.clone());
        }
        result
    }
    pub fn push_packet(&mut self, packet: PerformancePacket) -> Result<WriterProgress> {
        if let Some(error) = &self.progress.first_error {
            return Err(error.clone());
        }
        let result = self.push_inner(packet);
        self.latch(result)?;
        Ok(self.progress())
    }
    fn push_inner(&mut self, packet: PerformancePacket) -> Result<()> {
        self.progress.received_packets = self
            .progress
            .received_packets
            .checked_add(1)
            .ok_or("Packet counter overflow")?;
        if self.current.is_none() {
            self.current = Some(Callback::new(packet, &self.config, &self.context)?);
            return Ok(());
        }
        if matches!(packet.kind, PerformancePacketKind::Finish(_)) {
            check(
                self.profile == PerformanceProfile::FinishV2,
                "Version-1 originals cannot contain a Finish cut",
            )?;
        }
        let end = self
            .current
            .as_mut()
            .expect("current callback")
            .push(packet)?;
        let Some(end) = end else {
            return Ok(());
        };
        let callback = self.current.take().expect("completed callback");
        let next = callback.checked_context(&end, &self.context)?;
        let (mut chunks, mut prefixes) = self.encode_callback(&callback, Some(&end))?;
        let total = chunks
            .iter()
            .map(OwnedChunk::bytes)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .sum::<usize>();
        check(
            total <= MAX_CALLBACK_BYTES && chunks.len() <= MAX_GROUP_MEMBERS,
            "Encoded callback exceeds writer budget",
        )?;
        let prior = self
            .members
            .iter()
            .map(OwnedChunk::bytes)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .sum::<usize>();
        if !self.members.is_empty()
            && (self.members.len() + chunks.len() > MAX_GROUP_MEMBERS
                || prior + total > MAX_GROUP_BYTES - 64 * 1024)
        {
            self.flush_inner()?;
            // The prior group's checkpoint advances the control stream too.
            // Re-encode against that new exact prefix, including event IDs.
            (chunks, prefixes) = self.encode_callback(&callback, Some(&end))?;
        }
        if self.members.is_empty() {
            self.first_group_callback = callback.index;
        }
        self.members.extend(chunks);
        self.prefixes = prefixes;
        self.context = next;
        self.progress.complete_callbacks = callback.index;
        self.progress.complete_frames = end.end_frame;
        self.progress.capture_fault = self.context.capture_fault;
        self.progress.capture_fault_pending = self.context.capture_pending;
        self.progress.finish_cut = self.context.finish;
        if self.context.frame - self.progress.durable_frames
            >= u64::from(self.config.info.sample_rate).div_ceil(4)
            || callback.index - self.progress.durable_callbacks >= MAX_GROUP_CALLBACKS
            || self.context.finish.is_some()
        {
            self.flush_inner()?;
        }
        Ok(())
    }
    fn header(
        &self,
        index: usize,
        prefix: &StreamPrefix,
        count: u64,
        start_ticks: u64,
        end_ticks: u64,
    ) -> ChunkHeader {
        let stream = &self.descriptor.streams[index];
        ChunkHeader {
            version: CHUNK_VERSION,
            take_id: self.descriptor.take_id.clone(),
            stream_id: stream.stream_id.clone(),
            segment_id: stream.segment_id.clone(),
            sequence: prefix.next_sequence,
            range: if index == 0 {
                SourceRange::AudioFrames {
                    start: prefix.source_end.value(),
                    count,
                }
            } else {
                SourceRange::Events {
                    start: prefix.source_end.value(),
                    count,
                }
            },
            interpretation: stream.kind.clone(),
            observed_start: HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: start_ticks,
            },
            observed_end: HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: end_ticks,
            },
            clock_anchors: vec![],
            kind: ChunkKind::Content,
        }
    }
    fn encode_callback(
        &self,
        callback: &Callback,
        end: Option<&TapEnd>,
    ) -> Result<(Vec<OwnedChunk>, Vec<StreamPrefix>)> {
        let mut prefixes = self.prefixes.clone();
        let mut chunks = Vec::new();
        if !callback.pcm.is_empty() {
            check(
                callback.pcm.len() <= MAX_AUDIO_CHUNK_BYTES,
                "Callback PCM exceeds chunk ceiling",
            )?;
            let header = self.header(
                0,
                &prefixes[0],
                callback.frames,
                callback.begin.start_frame,
                callback.begin.start_frame + callback.frames,
            );
            advance(&mut prefixes[0], &header)?;
            chunks.push(OwnedChunk {
                header,
                payload: callback.pcm.clone(),
            });
        }
        let mut records: [Vec<(LocalRecord, u64)>; 6] = std::array::from_fn(|_| Vec::new());
        let record = |record| LocalRecord {
            version: self.profile.version(),
            record,
        };
        records[5].push((
            record(PerformanceRecord::Begin {
                callback_index: callback.index,
                begin: callback.begin,
            }),
            callback.begin.start_frame,
        ));
        for (ordinal, batch, disposition) in &callback.batches {
            records[5].push((
                record(PerformanceRecord::Batch {
                    callback_index: callback.index,
                    packet_ordinal: *ordinal,
                    batch: Box::new(*batch),
                    disposition: *disposition,
                }),
                callback.begin.start_frame,
            ));
            if *disposition == BatchDisposition::Applied {
                batch
                    .validate_shape(callback.begin.sample_rate)
                    .map_err(|e| e.to_string())?;
                for (event_index, original) in
                    batch.events[..usize::from(batch.len)].iter().enumerate()
                {
                    let original = original.ok_or("Applied event missing")?;
                    check(
                        (1..=4).contains(&original.source),
                        "Unprepared applied source",
                    )?;
                    records[usize::from(original.source)].push((
                        record(PerformanceRecord::AppliedEvent {
                            callback_index: callback.index,
                            batch_ordinal: *ordinal,
                            event_index: event_index as u8,
                            scope: batch.scope,
                            batch_sequence: batch.sequence,
                            applied_frame: callback.begin.start_frame,
                            original,
                        }),
                        original.observed_ticks,
                    ));
                }
            }
        }
        if let Some(end) = end {
            for clear in end.clears.into_iter().flatten() {
                records[5].push((
                    record(PerformanceRecord::Clear {
                        callback_index: callback.index,
                        clear,
                    }),
                    clear.frame,
                ));
            }
            for transition in end.transitions[..usize::from(end.transition_len)]
                .iter()
                .flatten()
            {
                records[5].push((
                    record(PerformanceRecord::Transition {
                        callback_index: callback.index,
                        transition: *transition,
                    }),
                    transition.frame,
                ));
            }
        }
        if let Some(finish) = callback.finish {
            records[5].push((
                record(PerformanceRecord::Finish {
                    callback_index: callback.index,
                    finish,
                }),
                finish.cut_frame,
            ));
        }
        if let Some(end) = end {
            records[5].push((
                record(PerformanceRecord::End {
                    callback_index: callback.index,
                    end_frame: end.end_frame,
                    scope_after: end.scope_after,
                    applied_sequence: end.applied_sequence,
                    active_voices: end.active_voices,
                    release_tail_voices: end.release_tail_voices,
                    clear_count: end.clears.iter().flatten().count() as u16,
                    transition_count: end.transition_len,
                    first_fault: end.first_fault,
                }),
                end.end_frame,
            ));
        }
        for index in 1..6 {
            let mut bytes = Vec::new();
            let mut count = 0;
            let mut low = u64::MAX;
            let mut high = 0;
            for (record, ticks) in &records[index] {
                let ordinal = prefixes[index].source_end.value() + count;
                let encoded = encode_record(&self.config, index, record, *ticks, ordinal)?;
                if bytes.len() + 4 + encoded.len() > MAX_EVENT_CHUNK_BYTES && !bytes.is_empty() {
                    let header = self.header(index, &prefixes[index], count, low, high);
                    advance(&mut prefixes[index], &header)?;
                    chunks.push(OwnedChunk {
                        header,
                        payload: std::mem::take(&mut bytes),
                    });
                    count = 0;
                    low = u64::MAX;
                    high = 0;
                }
                check(
                    4 + encoded.len() <= MAX_EVENT_CHUNK_BYTES,
                    "Local record exceeds raw chunk cap",
                )?;
                bytes.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&encoded);
                count += 1;
                low = low.min(*ticks);
                high = high.max(*ticks);
            }
            if !bytes.is_empty() {
                let header = self.header(index, &prefixes[index], count, low, high);
                advance(&mut prefixes[index], &header)?;
                chunks.push(OwnedChunk {
                    header,
                    payload: bytes,
                });
            }
        }
        Ok((chunks, prefixes))
    }
    pub fn flush(&mut self) -> Result<WriterProgress> {
        if let Some(error) = &self.progress.first_error {
            return Err(error.clone());
        }
        let result = self.flush_inner();
        self.latch(result)?;
        Ok(self.progress())
    }
    fn flush_inner(&mut self) -> Result<()> {
        if self.members.is_empty() {
            return Ok(());
        }
        let mut prefixes = self.prefixes.clone();
        let header = self.header(5, &prefixes[5], 1, self.context.frame, self.context.frame);
        advance(&mut prefixes[5], &header)?;
        let checkpoint = PerformanceCheckpoint {
            group_sequence: self
                .group_sequence
                .checked_add(1)
                .ok_or("Checkpoint counter overflow")?,
            previous_checkpoint_sha256: self.progress.checkpoint_sha256.clone(),
            first_callback_index: self.first_group_callback,
            last_callback_index: self.context.next_callback - 1,
            audio_end: self.context.frame,
            scope_after: self.context.scope,
            chunks: self
                .members
                .iter()
                .map(OwnedChunk::identity)
                .collect::<Result<_>>()?,
            prefixes,
        };
        let encoded = encode_record(
            &self.config,
            5,
            &LocalRecord {
                version: self.profile.version(),
                record: PerformanceRecord::Checkpoint {
                    checkpoint: checkpoint.clone(),
                },
            },
            self.context.frame,
            self.prefixes[5].source_end.value(),
        )?;
        let digest = checkpoint_sha256(&encoded)?;
        let mut payload = (encoded.len() as u32).to_le_bytes().to_vec();
        payload.extend_from_slice(&encoded);
        let mut request = self.members.clone();
        request.push(OwnedChunk { header, payload });
        check(
            request.len() <= 64
                && request
                    .iter()
                    .map(OwnedChunk::bytes)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .sum::<usize>()
                    <= MAX_GROUP_BYTES,
            "Append group exceeds journal budget",
        )?;
        self.pending_request = Some((request, checkpoint, digest));
        self.commit_pending()
    }
    fn commit_pending(&mut self) -> Result<()> {
        let (request, _, _) = self
            .pending_request
            .as_ref()
            .ok_or("No exact pending group")?;
        let borrowed: Vec<_> = request
            .iter()
            .map(|c| AppendChunk {
                header: &c.header,
                payload: &c.payload,
            })
            .collect();
        self.journal
            .append_group(&borrowed)
            .map_err(|e| e.to_string())?;
        let (_, checkpoint, digest) = self.pending_request.take().expect("pending exact request");
        self.group_sequence = checkpoint.group_sequence;
        self.prefixes = checkpoint.prefixes.clone();
        self.progress.durable_prefixes = checkpoint.prefixes;
        self.progress.durable_callbacks = checkpoint.last_callback_index;
        self.progress.durable_frames = checkpoint.audio_end;
        self.progress.checkpoint_sha256 = Some(digest);
        self.members.clear();
        Ok(())
    }
    /// Explicitly replay ONLY the retained immutable request after an ambiguous
    /// disk result. First failure remains sticky; capture can never resume here.
    pub fn retry_pending(&mut self) -> Result<WriterProgress> {
        let result = self.commit_pending();
        self.latch(result)?;
        Ok(self.progress())
    }
    pub fn close_interrupted(mut self) -> Result<RetainedPerformance> {
        if let Some(error) = &self.progress.first_error {
            return Err(error.clone());
        }
        self.flush_inner()?;
        if let Some(callback) = self.current.take() {
            let (chunks, _) = self.encode_callback(&callback, None)?;
            let borrowed: Vec<_> = chunks
                .iter()
                .map(|c| AppendChunk {
                    header: &c.header,
                    payload: &c.payload,
                })
                .collect();
            if !borrowed.is_empty() {
                self.journal
                    .append_group(&borrowed)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(RetainedPerformance {
            path: self.path().to_owned(),
            descriptor_sha256: self.descriptor.sha256().map_err(|e| e.to_string())?,
            progress: self.progress(),
            unknown_tail: true,
        })
    }
    #[cfg(feature = "test-hooks")]
    pub fn set_boundary_hook(
        &mut self,
        hook: impl FnMut(omniusong_recording::WriteBoundary) -> omniusong_recording::Result<()>
        + Send
        + 'static,
    ) {
        self.journal.set_boundary_hook(hook);
    }
    #[cfg(feature = "test-hooks")]
    pub fn clear_boundary_hook(&mut self) {
        self.journal.clear_boundary_hook();
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
enum InspectionFailure {
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
    fn into_recording(self) -> RecordingError {
        match self {
            Self::Semantic(message) => RecordingError::Invalid(message),
            Self::Stopped(error) => error,
        }
    }
}
type InspectionResult<T> = std::result::Result<T, InspectionFailure>;
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
fn contextual_records(
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
    result.map_err(InspectionFailure::into_recording)
}
fn inspect_performance_snapshot(
    inspection: &omniusong_recording::JournalInspection,
    mut read_ordinal: impl FnMut(u64) -> Result<ChunkRecord>,
) -> Result<PerformanceInspection> {
    inspect_performance_snapshot_bounded(inspection, None, |ordinal| {
        read_ordinal(ordinal).map_err(InspectionFailure::Semantic)
    })
    .map_err(|error| error.to_string())
}
fn inspect_performance_snapshot_bounded(
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
            if checkpoint.is_none() {
                check(
                    members.len() < MAX_GROUP_MEMBERS
                        && !decoded
                            .iter()
                            .any(|(_, r)| matches!(r.record, PerformanceRecord::Checkpoint { .. })),
                    "Checkpoint must be a final singleton chunk",
                )?;
                members.push(chunk.clone());
            }
            index_hash.update(ordinal.to_le_bytes());
            index_hash.update(chunk.sha256.as_bytes());
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

/// Explicit checked recovery. The stable identity binds ALL retained originals,
/// not only the selected audio; sealing can change physical bytes on exact replay.
pub fn recover_performance(
    reader: CaptureReader,
    expected: &PerformanceInspection,
    operation_id: &str,
) -> Result<(SealedManifest, bool)> {
    use omniusong_core::contracts::{OperationId, Validate};
    OperationId(operation_id.into())
        .validate()
        .map_err(|e| e.to_string())?;
    let current = inspect_performance(&reader)?;
    let stable = current.descriptor_sha256 == expected.descriptor_sha256
        && current.original_rows_sha256 == expected.original_rows_sha256
        && current.selection == expected.selection
        && current.checkpoint_sha256 == expected.checkpoint_sha256
        && current.checkpoint == expected.checkpoint
        && current.finish_cut == expected.finish_cut;
    check(
        stable && (reader.inspection().manifest.is_some() || current == *expected),
        "Performance originals/review changed; inspect again",
    )?;
    let selection = expected
        .selection
        .as_ref()
        .ok_or("No complete checkpoint can be recovered")?;
    let token = serde_json::to_vec(&(operation_id, expected)).map_err(|e| e.to_string())?;
    let derived = format!(
        "performance-recovery:{}",
        hash(
            &[
                b"OmniuSong.performance-recovery.v1\0".as_slice(),
                token.as_slice()
            ]
            .concat()
        )
    );
    reader
        .recover_prefix(&derived, selection)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod seal_cost_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::recording_live_pair;
    use omniusong_core::contracts::{ClockEpochId, EndpointId, TakeId};
    fn bounded_unit_source() -> (PathBuf, PerformanceConfig) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-bounded-unit-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let root = parent.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&root).unwrap();
        let path = root.join("take.sqlite3");
        let scope = LiveScope {
            worker_generation: LiveId([71; 16]),
            session_id: LiveId([72; 16]),
            panic_generation: 1,
        };
        let config = PerformanceConfig {
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            info: TapInfo {
                scope,
                sample_rate: 8000,
                channels: 1,
            },
            gui_clock: LiveClock {
                domain: "bounded-unit-gui".into(),
                epoch: "bounded-unit-epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            output_epoch: ClockEpochId("bounded-unit-render".into()),
            sources: std::array::from_fn(|i| PerformanceSource {
                binding: LiveSourceBinding {
                    kind: [
                        LiveSourceKind::ComputerKeys,
                        LiveSourceKind::PointerKeys,
                        LiveSourceKind::Pads,
                        LiveSourceKind::AccessiblePulse,
                    ][i],
                    handle: i as u8 + 1,
                    connection_generation: 1,
                },
                endpoint: EndpointId(format!("bounded-unit-source-{i}")),
            }),
        };
        let mut writer = PerformanceWriter::create(&path, config.clone()).unwrap();
        let (admission, mut renderer, mut tap) = recording_live_pair(scope, 8000, 1).unwrap();
        for _ in 0..2 {
            renderer.render_into(&mut [0.0; 16]);
            while let Some(packet) = tap.try_pop().unwrap() {
                writer.push_packet(packet).unwrap();
            }
            writer.flush().unwrap();
        }
        drop(renderer);
        drop(tap);
        drop(admission);
        writer.close_interrupted().unwrap();
        (path, config)
    }
    #[test]
    fn every_contextual_decode_is_charged_and_record_limit_refuses_before_next_owned_allocation() {
        use omniusong_recording::{InspectionLimits, InspectionResource};
        use std::sync::{Arc, atomic::AtomicBool};
        let (path, config) = bounded_unit_source();
        let reader = CaptureReader::open(&path).unwrap();
        let chunk = reader.read_ordinal(2).unwrap();
        assert_eq!(chunk.header.stream_id, "control");
        assert_eq!(chunk.header.range.count(), 2);
        let budget = InspectionBudget::new(
            InspectionLimits::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        for pass in 1..=2 {
            assert_eq!(
                contextual_records(
                    &config,
                    PerformanceProfile::OriginalV1,
                    5,
                    &chunk,
                    Some(&budget)
                )
                .unwrap()
                .len(),
                2
            );
            let usage = budget.usage();
            assert_eq!(usage.record_visits, pass * 2);
            assert_eq!(usage.work_bytes, pass * chunk.payload.len() as u64);
        }
        let limited = InspectionBudget::new(
            InspectionLimits {
                max_record_visits: 1,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let error = contextual_records(
            &config,
            PerformanceProfile::OriginalV1,
            5,
            &chunk,
            Some(&limited),
        )
        .unwrap_err()
        .into_recording();
        assert!(matches!(
            error,
            RecordingError::InspectionLimit {
                resource: InspectionResource::RecordVisits,
                limit: 1,
                attempted: 2
            }
        ));
        assert_eq!(limited.usage().record_visits, 1);
        let work = InspectionBudget::new(
            InspectionLimits {
                max_work_bytes: chunk.payload.len() as u64 - 1,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(matches!(
            contextual_records(
                &config,
                PerformanceProfile::OriginalV1,
                5,
                &chunk,
                Some(&work)
            )
            .unwrap_err()
            .into_recording(),
            RecordingError::InspectionLimit {
                resource: InspectionResource::WorkBytes,
                ..
            }
        ));
        assert_eq!(work.usage().record_visits, 0);
        drop(reader);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn cancellation_after_a_verified_checkpoint_overrides_a_simultaneous_semantic_tail_error() {
        use omniusong_recording::InspectionLimits;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let (path, _config) = bounded_unit_source();
        let reader = CaptureReader::open(&path).unwrap();
        let baseline = inspect_performance(&reader).unwrap();
        assert_eq!(baseline.checkpoint.as_ref().unwrap().group_sequence, 2);
        let cancelled = Arc::new(AtomicBool::new(false));
        let budget = InspectionBudget::new(InspectionLimits::default(), cancelled.clone()).unwrap();
        let mut seen = 0;
        let result =
            inspect_performance_snapshot_bounded(reader.inspection(), Some(&budget), |ordinal| {
                seen = ordinal;
                if ordinal == 4 {
                    cancelled.store(true, Ordering::Release);
                    return Err(InspectionFailure::Semantic(
                        "simultaneous invalid tail".into(),
                    ));
                }
                reader
                    .read_ordinal(ordinal)
                    .map_err(InspectionFailure::from)
            });
        assert_eq!(seen, 4);
        assert_eq!(
            budget.usage().groups,
            1,
            "a complete prefix existed before cancellation"
        );
        assert!(matches!(
            result.unwrap_err().into_recording(),
            RecordingError::InspectionCancelled
        ));
        cancelled.store(false, Ordering::Release);
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionCancelled)
        ));
        drop(reader);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn tiny_callbacks_force_chunk_budget_flush_and_rebase_control_ordinals() {
        let scope = LiveScope {
            worker_generation: LiveId([1; 16]),
            session_id: LiveId([2; 16]),
            panic_generation: 1,
        };
        let config = PerformanceConfig {
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            info: TapInfo {
                scope,
                sample_rate: 8000,
                channels: 1,
            },
            gui_clock: LiveClock {
                domain: "test-gui".into(),
                epoch: "test-epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            output_epoch: ClockEpochId("render-epoch".into()),
            sources: std::array::from_fn(|i| PerformanceSource {
                binding: LiveSourceBinding {
                    kind: [
                        LiveSourceKind::ComputerKeys,
                        LiveSourceKind::PointerKeys,
                        LiveSourceKind::Pads,
                        LiveSourceKind::AccessiblePulse,
                    ][i],
                    handle: (i + 1) as u8,
                    connection_generation: 1,
                },
                endpoint: EndpointId(format!("test-source-{i}")),
            }),
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/rust-performance-writer/unit")
            .join(uuid::Uuid::new_v4().to_string())
            .join("take.sqlite3");
        let mut writer = PerformanceWriter::create(&path, config).unwrap();
        let (mut admission, mut renderer, mut owner) = recording_live_pair(scope, 8000, 1).unwrap();
        let mut batch = LiveBatch::empty(scope, 1, None);
        batch
            .try_push(LiveEvent {
                source: 1,
                connection_generation: 1,
                observed_ticks: 9_007_199_254_740_993,
                event: LiveEventKind::NoteOn {
                    note: 1,
                    voice: 1,
                    key: 60,
                    velocity16: 32769,
                    patch: LivePatch::Keys,
                },
            })
            .unwrap();
        admission.admit(batch).unwrap();
        for _ in 0..80 {
            renderer.render_into(&mut [0.0]);
            while let Some(packet) = owner.try_pop().unwrap() {
                writer.push_packet(packet).unwrap();
            }
        }
        let progress = writer.flush().unwrap();
        assert_eq!(progress.durable_frames, 80);
        assert_eq!(progress.durable_callbacks, 80);
        drop(renderer);
        drop(owner);
        drop(admission);
        writer.close_interrupted().unwrap();
        let reader = CaptureReader::open(&path).unwrap();
        let review = inspect_performance(&reader).unwrap();
        assert!(review.problem.is_none(), "{review:?}");
        assert_eq!(review.checkpoint.as_ref().unwrap().audio_end, 80);
        let (manifest, replayed) =
            recover_performance(reader, &review, "tiny-callback-recover").unwrap();
        assert!(!replayed);
        assert_eq!(
            manifest.body.streams[0].source_end,
            SourcePosition::AudioFrames(80)
        );
    }
}
