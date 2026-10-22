//! Pure performance event identities and bounded packet validation. No device authority.
use serde::{Deserialize, Serialize};

pub const LIVE_BATCH_EVENTS: usize = 32;
pub const LIVE_MAX_SOURCES: usize = 16;
pub const LIVE_VOICES: usize = 64;
pub const LIVE_MAX_CALLBACK_FRAMES: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveId(pub [u8; 16]);
impl LiveId {
    pub fn random() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
    pub fn is_valid(self) -> bool {
        self.0 != [0; 16]
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveScope {
    pub worker_generation: LiveId,
    pub session_id: LiveId,
    pub panic_generation: u64,
}
impl LiveScope {
    pub fn is_valid(self) -> bool {
        self.worker_generation.is_valid() && self.session_id.is_valid() && self.panic_generation > 0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveFinishFence {
    pub id: LiveId,
    pub scope: LiveScope,
    pub accepted_sequence: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveFinishPolicy {
    CutAtCompleteCallbackEnd,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveClock {
    pub domain: String,
    pub epoch: String,
    pub ticks_per_second_numerator: u64,
    pub ticks_per_second_denominator: u64,
}
impl LiveClock {
    pub fn is_valid(&self) -> bool {
        bounded_text(&self.domain, 96)
            && bounded_text(&self.epoch, 96)
            && (1..=1_000_000_000_000).contains(&self.ticks_per_second_numerator)
            && (1..=1_000_000_000_000).contains(&self.ticks_per_second_denominator)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSourceKind {
    ComputerKeys,
    PointerKeys,
    Pads,
    AccessiblePulse,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSourceBinding {
    pub kind: LiveSourceKind,
    pub handle: u8,
    pub connection_generation: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivePatch {
    Keys,
    Bass,
    Pad,
    Drums,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveEventKind {
    NoteOn {
        note: u64,
        voice: u64,
        key: u8,
        velocity16: u16,
        patch: LivePatch,
    },
    NoteOff {
        voice: u64,
        release_velocity16: u16,
    },
    Sustain {
        down: bool,
    },
    Pulse {
        note: u64,
        voice: u64,
        key: u8,
        velocity16: u16,
        release_velocity16: u16,
        patch: LivePatch,
        duration_frames: u32,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveEvent {
    pub source: u8,
    pub connection_generation: u64,
    pub observed_ticks: u64,
    pub event: LiveEventKind,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveBatch {
    pub scope: LiveScope,
    pub sequence: u64,
    /// Original GUI credit only. The supervisor never substitutes a token.
    pub lease_token: Option<LiveId>,
    pub len: u8,
    pub events: [Option<LiveEvent>; LIVE_BATCH_EVENTS],
}
impl LiveBatch {
    pub fn empty(scope: LiveScope, sequence: u64, lease_token: Option<LiveId>) -> Self {
        Self {
            scope,
            sequence,
            lease_token,
            len: 0,
            events: [None; LIVE_BATCH_EVENTS],
        }
    }
    pub fn try_push(&mut self, event: LiveEvent) -> Result<(), LiveSubmitError> {
        let index = usize::from(self.len);
        if index >= LIVE_BATCH_EVENTS {
            return Err(LiveSubmitError::Saturated);
        }
        self.events[index] = Some(event);
        self.len += 1;
        Ok(())
    }
    /// Shape/value checks only; runtime owns source, scope, sequence and voice checks.
    pub fn validate_shape(&self, sample_rate: u32) -> Result<(), LiveSubmitError> {
        if !self.scope.is_valid()
            || self.sequence == 0
            || usize::from(self.len) > LIVE_BATCH_EVENTS
            || self.lease_token.is_some_and(|token| !token.is_valid())
            || !(8_000..=768_000).contains(&sample_rate)
        {
            return Err(LiveSubmitError::Invalid);
        }
        for (index, slot) in self.events.iter().enumerate() {
            if index >= usize::from(self.len) {
                if slot.is_some() {
                    return Err(LiveSubmitError::Invalid);
                }
                continue;
            }
            let Some(event) = slot else {
                return Err(LiveSubmitError::Invalid);
            };
            if !(1..=LIVE_MAX_SOURCES as u8).contains(&event.source)
                || event.connection_generation == 0
            {
                return Err(LiveSubmitError::Invalid);
            }
            let valid = match event.event {
                LiveEventKind::NoteOn {
                    note, voice, key, ..
                } => note > 0 && voice > 0 && key <= 127,
                LiveEventKind::NoteOff { voice, .. } => voice > 0,
                LiveEventKind::Sustain { .. } => true,
                LiveEventKind::Pulse {
                    note,
                    voice,
                    key,
                    duration_frames,
                    ..
                } => {
                    note > 0
                        && voice > 0
                        && key <= 127
                        && (1..=sample_rate / 2).contains(&duration_frames)
                }
            };
            if !valid {
                return Err(LiveSubmitError::Invalid);
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSubmitError {
    NotLive,
    StaleScope,
    Invalid,
    PanicPending,
    FinishPending,
    Saturated,
    Faulted,
    Retired,
}
impl std::fmt::Display for LiveSubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotLive => "Live audition is not active",
            Self::StaleScope => "Live session or generation changed",
            Self::Invalid => "Invalid live event batch",
            Self::PanicPending => "Waiting for callback-confirmed all notes off",
            Self::FinishPending => "Recorded performance Finish permanently closed live admission",
            Self::Saturated => "Live event capacity exceeded",
            Self::Faulted => "Live audition faulted; stop before starting again",
            Self::Retired => "Audio worker is retired",
        })
    }
}
impl std::error::Error for LiveSubmitError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveFaultKind {
    InvalidBatch,
    Sequence,
    QueueOverflow,
    Polyphony,
    SourceOwnership,
    LeaseExpired,
    CallbackShape,
    Device,
    Clock,
    CounterExhausted,
    CallbackQuiescence,
    /// A recorded-live transfer failed; never resume or call it a complete take.
    RecordingTap,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveFault {
    pub scope: LiveScope,
    pub kind: LiveFaultKind,
    pub frame: u64,
    pub sequence: Option<u64>,
}
fn bounded_text(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.len() <= limit && !text.contains('\0')
}
