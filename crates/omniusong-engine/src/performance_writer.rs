//! Off-callback incremental performance originals and callback Finish evidence.
//! No device lifecycle or normal-seal authority is supplied here.
use crate::{live_types::*, performance_records::*, performance_tap::*};
pub(crate) use omniusong_performance::performance_inspection::inspect_snapshot_with_budget;
use omniusong_performance::performance_inspection::{
    Callback, Context, MAX_GROUP_BYTES, MAX_GROUP_MEMBERS, advance, initial_prefixes,
    inspect_performance_snapshot,
};
#[cfg(test)]
use omniusong_performance::performance_inspection::{
    InspectionFailure, contextual_records, inspect_performance_snapshot_bounded,
    verify_audio_bus_chunk,
};
pub use omniusong_performance::performance_inspection::{
    PerformanceInspection, inspect_performance, inspect_performance_bounded,
    inspect_sealed_performance, inspect_sealed_performance_bounded,
};

use omniusong_core::contracts::HostTimestamp;
#[cfg(test)]
use omniusong_recording::SourcePosition;
use omniusong_recording::{
    AppendChunk, CHUNK_VERSION, CaptureReader, ChunkHeader, ChunkKind, CompletePrefix, Journal,
    MAX_AUDIO_CHUNK_BYTES, MAX_EVENT_CHUNK_BYTES, SealedManifest, SourceRange, StreamPrefix,
    TakeDescriptor,
};
#[cfg(test)]
use omniusong_recording::{InspectionBudget, RecordingError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, String>;
const MAX_CALLBACK_BYTES: usize = 2 * 1024 * 1024;
const MAX_GROUP_CALLBACKS: u64 = 64;
fn check(ok: bool, reason: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(reason.into()) }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
    fn every_pcm_validation_pass_is_charged_before_traversal_and_nan_still_refuses() {
        use omniusong_recording::{InspectionLimits, InspectionResource};
        use std::sync::{Arc, atomic::AtomicBool};
        let (path, _) = bounded_unit_source();
        let before = std::fs::read(&path).unwrap();
        let reader = CaptureReader::open(&path).unwrap();
        let chunk = reader.read_ordinal(1).unwrap();
        assert_eq!(chunk.header.stream_id, "audio");
        let bytes = chunk.payload.len() as u64;
        assert!(bytes >= 4);
        let budget = InspectionBudget::new(
            InspectionLimits::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        for pass in 1..=2 {
            verify_audio_bus_chunk(&chunk, Some(&budget)).unwrap();
            assert_eq!(budget.usage().work_bytes, pass * bytes);
        }
        let limited = InspectionBudget::new(
            InspectionLimits {
                max_work_bytes: bytes - 1,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let mut invalid = chunk.clone();
        invalid.payload[..4].copy_from_slice(&f32::NAN.to_le_bytes());
        // Even an invalid sample is not traversed before cumulative admission.
        assert!(
            matches!(verify_audio_bus_chunk(&invalid, Some(&limited)), Err(InspectionFailure::Stopped(RecordingError::InspectionLimit { resource: InspectionResource::WorkBytes, limit, attempted })) if limit == bytes - 1 && attempted == bytes)
        );
        assert_eq!(limited.usage().work_bytes, 0);
        assert!(matches!(
            limited.checkpoint(),
            Err(RecordingError::InspectionLimit { .. })
        ));
        let ample = InspectionBudget::new(
            InspectionLimits::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(
            matches!(verify_audio_bus_chunk(&invalid, Some(&ample)), Err(InspectionFailure::Semantic(message)) if message == "Audio bus clock/PCM mismatch")
        );
        assert_eq!(ample.usage().work_bytes, bytes);
        drop(reader);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
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
