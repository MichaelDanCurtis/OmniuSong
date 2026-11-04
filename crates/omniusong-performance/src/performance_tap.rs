//! Pure fixed callback evidence packets. No ring, device or sealing authority.
use crate::live_types::{
    LiveBatch, LiveFault, LiveFaultKind, LiveFinishFence, LiveFinishPolicy, LiveScope,
};
use serde::{Deserialize, Serialize};

pub const TAP_PCM_FRAMES: usize = 256;
pub const TAP_CHANNELS: usize = 8;
pub const TAP_MAX_BATCHES: usize = 4;
pub const TAP_TRANSITIONS: usize = 128;
pub const TAP_CALLBACK_PACKETS: usize = 71;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TapFinish {
    pub fence: LiveFinishFence,
    pub callback_index: u64,
    pub cut_frame: u64,
    pub voices_cut: u16,
    pub release_tails_cut: u16,
    pub policy: LiveFinishPolicy,
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
    Finish(TapFinish),
    End(TapEnd),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerformancePacket {
    pub callback_index: u64,
    pub ordinal: u16,
    pub kind: PerformancePacketKind,
}

const _: () = assert!(std::mem::size_of::<PerformancePacket>() <= 16 * 1024);
