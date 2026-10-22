//! Authenticated procedural note derivation. This never publishes, plays or
//! mutates a recording and grants no normal-seal or input-device authority.
use crate::{
    live_types::{LiveClock, LivePatch, LiveScope},
    performance_inspection::{
        PerformanceInspection, contextual_records, inspect_sealed_performance_bounded,
    },
    performance_records::{
        LocalRecord, PerformanceConfig, PerformanceProfile, PerformanceRecord, descriptor_profile,
        from_descriptor,
    },
    performance_tap::TapClearReason,
};
use omniusong_core::{
    NoteValues,
    contracts::{TakeId, Validate},
};
use omniusong_recording::{
    InspectionBudget, RecordingError, RecoveryPrefix, Result, SealedJournal,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;

mod arithmetic;
mod binding;
pub use binding::check_retained_binding_encoding_bounded;
mod lifecycle;
#[cfg(test)]
mod tests;

pub const MAX_PROJECTION_OBSERVATIONS: usize = 20_000;
pub const MAX_PROJECTION_REVIEW_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceProjectionSourcePin {
    pub take_id: String,
    pub manifest_sha256: String,
    pub descriptor_sha256: String,
    pub original_rows_sha256: String,
    pub journal_sha256_at_review: String,
    pub checkpoint_sha256: String,
    pub selection: RecoveryPrefix,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceFrameRegion {
    pub start: u64,
    pub end: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceProjectionRounding {
    NearestTiesUp,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceProjectionDuration {
    ReleaseGate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceProjectionPolicy {
    Reject,
    OmitWithDisclosure,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceProjectionRequest {
    pub version: u32,
    pub source: PerformanceProjectionSourcePin,
    pub region: PerformanceFrameRegion,
    pub sources: Vec<u8>,
    pub tempo_bits: u64,
    pub destination_start_tick: u64,
    pub rounding: PerformanceProjectionRounding,
    pub duration: PerformanceProjectionDuration,
    pub collapsed: PerformanceProjectionPolicy,
    pub incomplete: PerformanceProjectionPolicy,
    pub region_crossing: PerformanceProjectionPolicy,
}
impl PerformanceProjectionRequest {
    /// Request shape only. Source authority requires the held bounded reader.
    pub fn validate(&self) -> Result<()> {
        require(
            self.version == 1,
            "Unsupported performance projection version",
        )?;
        require(
            self.source.take_id.len() <= 128,
            "Projection take identity exceeds its bound",
        )?;
        TakeId(self.source.take_id.clone())
            .validate()
            .map_err(invalid)?;
        require(
            [
                &self.source.manifest_sha256,
                &self.source.descriptor_sha256,
                &self.source.original_rows_sha256,
                &self.source.journal_sha256_at_review,
                &self.source.checkpoint_sha256,
                &self.source.selection.chunk_index_sha256,
            ]
            .into_iter()
            .all(|value| sha(value)),
            "Projection needs exact lowercase SHA-256 source pins",
        )?;
        require(
            self.source.selection.chunk_count > 0
                && self.source.selection.chunk_count <= i64::MAX as u64,
            "Projection selection is outside the supported range",
        )?;
        require(
            self.region.start < self.region.end && self.region.end <= i64::MAX as u64,
            "Select a positive half-open source frame region",
        )?;
        require(
            !self.sources.is_empty()
                && self.sources.len() <= 4
                && self.sources.iter().all(|s| (1..=4).contains(s))
                && self.sources.windows(2).all(|pair| pair[0] < pair[1]),
            "Select sorted unique source handles from one through four",
        )?;
        require(
            self.destination_start_tick <= omniusong_core::note_edit_types::MAX_NOTE_START_TICK,
            "Projection placement exceeds the musical document range",
        )?;
        arithmetic::Tempo::new(self.tempo_bits).map(|_| ())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceVoiceIdentity {
    pub take_id: String,
    pub scope: LiveScope,
    pub source: u8,
    pub connection_generation: u64,
    pub note: u64,
    pub voice: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceRecordLocation {
    pub stream_id: String,
    pub stream_event_ordinal: u64,
    pub chunk_ordinal: u64,
    pub record_index: u32,
    pub callback_index: u64,
    pub batch_ordinal: Option<u16>,
    pub event_index: Option<u8>,
    /// Zero-based original GUI-stream ordinal, independent of chunk packing.
    pub applied_source_ordinal: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformancePulseShape {
    pub duration_frames: u32,
    pub release_velocity16: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformancePhysicalRelease {
    pub frame: u64,
    pub release_velocity16: u16,
    pub observed_ticks: u64,
    pub record: PerformanceRecordLocation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerformanceGateEndCause {
    NoteOff {},
    SustainUp {},
    AutoRelease {},
    Clear { reason: TapClearReason },
    Finish {},
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceGateEnd {
    pub cause: PerformanceGateEndCause,
    pub release_velocity16: Option<u16>,
    pub observed_ticks: Option<u64>,
    pub record: PerformanceRecordLocation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceObservedVoiceEnd {
    pub frame: u64,
    pub record: PerformanceRecordLocation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceHardCutObservation {
    pub frame: u64,
    pub cause: PerformanceGateEndCause,
    pub record: PerformanceRecordLocation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedPerformanceNote {
    pub identity: PerformanceVoiceIdentity,
    pub identity_sha256: String,
    pub values: NoteValues,
    pub onset_frame: u64,
    pub gate_end_frame: u64,
    pub onset_gui_ticks: u64,
    pub patch: LivePatch,
    pub pulse: Option<PerformancePulseShape>,
    pub onset_record: PerformanceRecordLocation,
    pub physical_key_up: Option<PerformancePhysicalRelease>,
    pub gate_end: PerformanceGateEnd,
    pub voice_ended: Option<PerformanceObservedVoiceEnd>,
    pub hard_cut: Option<PerformanceHardCutObservation>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceProjectionOmissionReason {
    ExcludedSource,
    OutsideRegion,
    RegionCrossing,
    Incomplete,
    CollapsedTicks,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceProjectionOmission {
    pub identity: PerformanceVoiceIdentity,
    pub identity_sha256: String,
    pub onset_frame: u64,
    pub gate_end_frame: Option<u64>,
    pub reason: PerformanceProjectionOmissionReason,
    pub onset_record: PerformanceRecordLocation,
    pub physical_key_up: Option<PerformancePhysicalRelease>,
    pub gate_end: Option<PerformanceGateEnd>,
    pub voice_ended: Option<PerformanceObservedVoiceEnd>,
    pub hard_cut: Option<PerformanceHardCutObservation>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceProjectionCounts {
    /// All authenticated note onsets, including sources/regions outside the view.
    pub observations: u64,
    pub notes: u32,
    pub excluded_source: u64,
    pub outside_region: u64,
    pub region_crossing: u32,
    pub incomplete: u32,
    pub collapsed: u32,
    pub applied_events: u64,
    pub sustain_events: u64,
    pub late_note_offs: u64,
    pub repeated_note_offs: u64,
    pub discarded_batches: u64,
    pub rejected_batches: u64,
    pub hard_cuts: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceProjectionDisclosure {
    ProceduralDerivationNotAi,
    NearestTiesUp960Ppq,
    ReleaseGateNotEnvelope,
    SustainFlattenedToGate,
    PatchAndPulseNotEncodedInNoteValues,
    GuiTicksAreUncalibratedObservations,
    SelectedSourcesAndRegionOnly,
    UnknownTailExcluded,
    ObservedHardCutsRetained,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceProjectionReview {
    pub version: u32,
    pub request: PerformanceProjectionRequest,
    pub profile_version: u16,
    pub sample_rate: u32,
    pub gui_clock: LiveClock,
    pub notes: Vec<ProjectedPerformanceNote>,
    pub omissions: Vec<PerformanceProjectionOmission>,
    /// Ordered, domain-separated digest of compact exclusion observations as
    /// each exclusion becomes known. Excluded/outside notes use exact counts
    /// and this digest, rather than consuming the selected-view row capacity.
    pub unselected_observations_sha256: String,
    pub counts: PerformanceProjectionCounts,
    pub disclosures: Vec<PerformanceProjectionDisclosure>,
    pub review_sha256: String,
}

fn invalid(message: impl ToString) -> RecordingError {
    RecordingError::Invalid(message.to_string())
}
fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(invalid(message)) }
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn charge(budget: &InspectionBudget, bytes: usize) -> Result<()> {
    budget.charge_work(u64::try_from(bytes).map_err(invalid)?)
}

struct HashWriter<'a> {
    budget: &'a InspectionBudget,
    hash: Sha256,
    used: usize,
    maximum: usize,
}
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.used) {
            return Err(std::io::Error::other(
                "Performance projection exceeds its canonical byte bound",
            ));
        }
        self.budget
            .charge_work((bytes.len() as u64).saturating_mul(2))
            .map_err(|_| std::io::Error::other("Performance projection governor stopped"))?;
        self.hash.update(bytes);
        self.used += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn digest<T: Serialize>(
    domain: &[u8],
    value: &T,
    maximum: usize,
    budget: &InspectionBudget,
) -> Result<String> {
    charge(budget, domain.len() + 128)?;
    let mut writer = HashWriter {
        budget,
        hash: Sha256::new(),
        used: 0,
        maximum,
    };
    writer.hash.update(domain);
    let encoded = serde_json::to_writer(&mut writer, value);
    budget.checkpoint()?;
    encoded.map_err(invalid)?;
    Ok(format!("{:x}", writer.hash.finalize()))
}

struct ControlChunk {
    sequence: u64,
    sha256: String,
    event_start: u64,
    records: Vec<LocalRecord>,
}

/// A borrowed, source-authenticated view under the reader's single cumulative
/// governor. It cannot be cloned, deserialized or constructed from a claimed
/// inspection. Each interpretation still rederives its actual control events.
pub struct AuthenticatedPerformance<'a> {
    reader: &'a SealedJournal,
    config: PerformanceConfig,
    profile: PerformanceProfile,
    inspected: PerformanceInspection,
    source: PerformanceProjectionSourcePin,
}
impl<'a> AuthenticatedPerformance<'a> {
    pub fn open(reader: &'a SealedJournal) -> Result<Self> {
        let budget = reader.inspection_budget().ok_or_else(|| {
            invalid("Performance authentication requires a held bounded sealed reader")
        })?;
        let result = (|| {
            budget.checkpoint()?;
            charge(budget, omniusong_recording::MAX_METADATA_BYTES * 8)?;
            let config = from_descriptor(reader.descriptor()).map_err(invalid)?;
            let profile = descriptor_profile(reader.descriptor()).map_err(invalid)?;
            let inspected = inspect_sealed_performance_bounded(reader)?;
            let selection = inspected
                .selection
                .as_ref()
                .ok_or_else(|| invalid("No authenticated selected performance prefix"))?;
            let checkpoint_sha256 = inspected.checkpoint_sha256.as_ref().ok_or_else(|| {
                invalid("No authenticated selected performance checkpoint digest")
            })?;
            require(
                inspected.checkpoint.is_some()
                    && config.take_id == reader.descriptor().take_id
                    && inspected.descriptor_sha256 == reader.manifest().body.descriptor_sha256
                    && selection.chunk_count == reader.manifest().body.chunk_count
                    && selection.chunk_index_sha256 == reader.manifest().body.chunk_index_sha256,
                "Authenticated performance checkpoint or prefix differs from the held sealed manifest",
            )?;
            require(
                [
                    &reader.manifest().sha256,
                    &inspected.descriptor_sha256,
                    &inspected.original_rows_sha256,
                    &inspected.journal_sha256_at_review,
                    checkpoint_sha256,
                    &selection.chunk_index_sha256,
                ]
                .into_iter()
                .all(|value| sha(value)),
                "Authenticated performance source pins are not canonical SHA-256 identities",
            )?;
            charge(budget, 4096)?;
            let source = PerformanceProjectionSourcePin {
                take_id: config.take_id.0.clone(),
                manifest_sha256: reader.manifest().sha256.clone(),
                descriptor_sha256: inspected.descriptor_sha256.clone(),
                original_rows_sha256: inspected.original_rows_sha256.clone(),
                journal_sha256_at_review: inspected.journal_sha256_at_review.clone(),
                checkpoint_sha256: checkpoint_sha256.clone(),
                selection: selection.clone(),
            };
            let held = Self {
                reader,
                config,
                profile,
                inspected,
                source,
            };
            held.verify_unchanged()?;
            Ok(held)
        })();
        budget.checkpoint()?;
        result
    }

    pub fn source_pin(&self) -> &PerformanceProjectionSourcePin {
        &self.source
    }

    fn budget(&self) -> &InspectionBudget {
        self.reader
            .inspection_budget()
            .expect("private authenticated context always retains its bounded reader")
    }

    pub fn verify_unchanged(&self) -> Result<()> {
        let budget = self.budget();
        budget.checkpoint()?;
        let unchanged = self.reader.verify_source_identity();
        budget.checkpoint()?;
        unchanged
    }

    pub fn project(
        &self,
        request: &PerformanceProjectionRequest,
    ) -> Result<PerformanceProjectionReview> {
        let budget = self.budget();
        let reader = self.reader;
        let config = &self.config;
        let profile = self.profile;
        let result = (|| {
            budget.checkpoint()?;
            charge(budget, 4096)?;
            request.validate()?;
            require(
                request.source == self.source,
                "Performance projection source pins differ from the held authenticated recording",
            )?;
            let checkpoint = self
                .inspected
                .checkpoint
                .as_ref()
                .expect("private authentication proves a complete checkpoint");
            require(
                request.region.end <= checkpoint.audio_end,
                "Projection region differs from the held sealed prefix",
            )?;
            self.verify_unchanged()?;
            let control = reader
                .manifest()
                .body
                .streams
                .iter()
                .find(|p| p.stream_id == "control")
                .ok_or_else(|| invalid("Performance manifest has no control prefix"))?;
            charge(budget, 64 * std::mem::size_of::<ControlChunk>())?;
            let mut pending = Vec::with_capacity(64);
            let mut group_base = 0u64;
            let mut group_bytes = 0usize;
            let mut lifecycle = lifecycle::Lifecycle::new(config, request, budget)?;
            for sequence in 0..control.next_sequence {
                budget.checkpoint()?;
                let chunk = reader.read_chunk("control", sequence)?;
                let decode_credit = chunk
                    .payload
                    .len()
                    .checked_mul(8)
                    .ok_or_else(|| invalid("Projection decode credit overflow"))?;
                charge(budget, decode_credit)?;
                let decoded = contextual_records(config, profile, 5, &chunk, Some(budget))
                    .map_err(|e| e.into_recording())?;
                if decoded.len() == 1
                    && matches!(&decoded[0].1.record, PerformanceRecord::Checkpoint { .. })
                {
                    let PerformanceRecord::Checkpoint { checkpoint: group } = &decoded[0].1.record
                    else {
                        unreachable!()
                    };
                    budget.visit_groups(1)?;
                    require(
                        group.chunks.len() <= 63
                            && pending.len()
                                == group
                                    .chunks
                                    .iter()
                                    .filter(|member| member.stream_id == "control")
                                    .count(),
                        "Control checkpoint membership differs from its decoded group",
                    )?;
                    for retained in pending.drain(..) {
                        let retained: ControlChunk = retained;
                        charge(budget, 63 * 128)?;
                        let index = group
                            .chunks
                            .iter()
                            .position(|member| {
                                member.stream_id == "control"
                                    && member.sequence == retained.sequence
                                    && member.sha256 == retained.sha256
                            })
                            .ok_or_else(|| {
                                invalid("Control chunk is absent from its authenticated checkpoint")
                            })?;
                        let ordinal = group_base
                            .checked_add(index as u64 + 1)
                            .ok_or_else(|| invalid("Control physical ordinal overflow"))?;
                        for (index, record) in retained.records.into_iter().enumerate() {
                            budget.visit_records(1)?;
                            charge(budget, 512)?;
                            let location = PerformanceRecordLocation {
                                stream_id: "control".into(),
                                stream_event_ordinal: retained
                                    .event_start
                                    .checked_add(index as u64)
                                    .ok_or_else(|| invalid("Control event ordinal overflow"))?,
                                chunk_ordinal: ordinal,
                                record_index: u32::try_from(index).map_err(invalid)?,
                                callback_index: 0,
                                batch_ordinal: None,
                                event_index: None,
                                applied_source_ordinal: None,
                            };
                            lifecycle.record(record.record, location, budget)?;
                        }
                    }
                    group_base = group_base
                        .checked_add(group.chunks.len() as u64 + 1)
                        .ok_or_else(|| invalid("Checkpoint physical ordinal overflow"))?;
                    lifecycle.checkpoint(group)?;
                    group_bytes = 0;
                } else {
                    require(
                        pending.len() < 63,
                        "Too many control chunks before an authenticated checkpoint",
                    )?;
                    group_bytes = group_bytes
                        .checked_add(chunk.payload.len())
                        .ok_or_else(|| invalid("Projection group byte count overflow"))?;
                    require(
                        group_bytes <= MAX_PROJECTION_REVIEW_BYTES,
                        "Projection control group exceeds 8 MiB",
                    )?;
                    charge(
                        budget,
                        decoded
                            .len()
                            .checked_mul(std::mem::size_of::<LocalRecord>())
                            .ok_or_else(|| invalid("Projection record allocation overflow"))?,
                    )?;
                    let records = decoded.into_iter().map(|(_, record)| record).collect();
                    pending.push(ControlChunk {
                        sequence,
                        sha256: chunk.sha256,
                        event_start: chunk.header.range.start(),
                        records,
                    });
                }
            }
            require(
                pending.is_empty() && group_base == request.source.selection.chunk_count,
                "Projection did not end at the authenticated selected checkpoint",
            )?;
            let review = lifecycle.finish(
                request,
                profile.version(),
                &config.gui_clock,
                checkpoint,
                reader.manifest().body.interrupted.is_some(),
                budget,
            )?;
            self.verify_unchanged()?;
            Ok(review)
        })();
        budget.checkpoint()?;
        result
    }
}

/// Compatibility entrypoint with early request-shape refusal. Authentication,
/// interpretation and output share the existing held reader's sticky governor.
pub fn project_sealed_performance_bounded(
    reader: &SealedJournal,
    request: &PerformanceProjectionRequest,
) -> Result<PerformanceProjectionReview> {
    let budget = reader
        .inspection_budget()
        .ok_or_else(|| invalid("Performance projection requires a held bounded sealed reader"))?;
    let result = (|| {
        budget.checkpoint()?;
        charge(budget, 4096)?;
        request.validate()?;
        AuthenticatedPerformance::open(reader)?.project(request)
    })();
    budget.checkpoint()?;
    result
}
