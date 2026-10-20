//! Version 1 control-plane contracts. Decode/validate off the realtime thread.
//!
//! JSON envelopes own variable-sized data and are never callback payloads.
//! `RealtimeBatch` is the separate fixed-capacity, allocation-free admission
//! target. Transport, queues, workers, and durable recording are not implemented
//! by these definitions; see docs/contracts/NATIVE-PROTOCOLS.md.

use crate::{DocumentNoteAddress, NoteTimelinePolicy, NoteValues};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt};

pub const PROTOCOL_VERSION: u16 = 1;
pub const CONTROL_MAX_BYTES: usize = 4_096;
pub const WORKER_MAX_BYTES: usize = 65_536;
pub const CAPTURE_MAX_BYTES: usize = 16_384;
pub const REALTIME_EVENT_CAPACITY: usize = 256;
pub const MAX_BLOCK_FRAMES: u32 = 2_048;
pub const MAX_BLOB_BYTES: u64 = 1 << 40;
pub type Extensions = BTreeMap<String, Value>;
type Result<T> = std::result::Result<T, ContractError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError(pub String);
impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl Error for ContractError {}

fn require(ok: bool, reason: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(ContractError(reason.into()))
    }
}
fn text(value: &str, max: usize) -> Result<()> {
    require(
        !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control),
        "Empty, oversized, or control-bearing text",
    )
}
fn unique_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        require(seen.insert(id), "Duplicate identity in a selection")?;
    }
    Ok(())
}

pub trait Validate {
    fn validate(&self) -> Result<()>;
}
pub trait WirePayload: Validate + Serialize + DeserializeOwned {
    const PROTOCOL: &'static str;
    const MAX_BYTES: usize;
}

macro_rules! identities {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl Validate for $name {
            fn validate(&self) -> Result<()> {
                require(!self.0.is_empty() && self.0.len() <= 96
                    && self.0.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_:.@".contains(&b)),
                    concat!("Invalid ", stringify!($name)))
            }
        }
    )+};
}
identities!(
    SessionId,
    MessageId,
    ProjectId,
    TrackId,
    NoteId,
    VoiceId,
    EventId,
    EndpointId,
    ClockDomainId,
    ClockEpochId,
    AnchorId,
    OperationId,
    JobId,
    CandidateId,
    WorkerId,
    ResourceId,
    TakeId
);

fn json_shape(value: &Value, depth: usize, visited: &mut usize) -> Result<()> {
    *visited += 1;
    require(
        depth <= 16 && *visited <= 2_048,
        "Extension metadata is too complex",
    )?;
    match value {
        Value::Array(items) => {
            for item in items {
                json_shape(item, depth + 1, visited)?;
            }
        }
        Value::Object(items) => {
            for (key, item) in items {
                text(key, 128)?;
                json_shape(item, depth + 1, visited)?;
            }
        }
        Value::String(s) => require(s.len() <= 8_192, "Extension string is too large")?,
        _ => {}
    }
    Ok(())
}
fn extensions(ext: &Extensions, reserved: &[&str]) -> Result<()> {
    require(ext.len() <= 64, "Too many extension fields")?;
    let mut visited = 0;
    for (key, value) in ext {
        text(key, 128)?;
        require(
            !reserved.contains(&key.as_str()),
            "Extension shadows a protocol field",
        )?;
        json_shape(value, 0, &mut visited)?;
    }
    Ok(())
}

/// Use `decode`/`encode`, not unchecked serde deserialization, at process edges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol: String,
    pub version: u16,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub sequence: u64,
    pub payload: T,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl<T: WirePayload> Envelope<T> {
    pub fn new(session_id: SessionId, message_id: MessageId, sequence: u64, payload: T) -> Self {
        Self {
            protocol: T::PROTOCOL.into(),
            version: PROTOCOL_VERSION,
            session_id,
            message_id,
            sequence,
            payload,
            extensions: Extensions::new(),
        }
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        require(
            bytes.len() <= T::MAX_BYTES,
            "Protocol frame exceeds byte limit",
        )?;
        let message: Self = serde_json::from_slice(bytes)
            .map_err(|e| ContractError(format!("Invalid protocol JSON: {e}")))?;
        message.validate()?;
        Ok(message)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| ContractError(e.to_string()))?;
        require(
            bytes.len() <= T::MAX_BYTES,
            "Protocol frame exceeds byte limit",
        )?;
        Ok(bytes)
    }
}
impl<T: WirePayload> Validate for Envelope<T> {
    fn validate(&self) -> Result<()> {
        require(
            self.version == PROTOCOL_VERSION,
            "Unsupported protocol version",
        )?;
        require(
            self.protocol == T::PROTOCOL,
            "Wrong protocol for this channel",
        )?;
        self.session_id.validate()?;
        self.message_id.validate()?;
        self.payload.validate()?;
        extensions(
            &self.extensions,
            &[
                "protocol",
                "version",
                "session_id",
                "message_id",
                "sequence",
                "payload",
            ],
        )
    }
}
macro_rules! wire {
    ($ty:ty, $name:literal, $limit:expr) => {
        impl WirePayload for $ty {
            const PROTOCOL: &'static str = $name;
            const MAX_BYTES: usize = $limit;
        }
    };
}

/// Immutable object capability; not a path or permission to read arbitrary files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobRef {
    pub sha256: String,
    pub byte_len: u64,
    pub media_type: String,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for BlobRef {
    fn validate(&self) -> Result<()> {
        require(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid SHA-256",
        )?;
        require(
            self.byte_len > 0 && self.byte_len <= MAX_BLOB_BYTES,
            "Invalid object size",
        )?;
        text(&self.media_type, 128)?;
        extensions(&self.extensions, &["sha256", "byte_len", "media_type"])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioFrame {
    pub epoch: ClockEpochId,
    pub sample_rate: u32,
    pub frame: i64,
}
impl Validate for AudioFrame {
    fn validate(&self) -> Result<()> {
        self.epoch.validate()?;
        require(
            (8_000..=768_000).contains(&self.sample_rate),
            "Unsupported sample rate",
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostTimestamp {
    pub domain: ClockDomainId,
    pub epoch: ClockEpochId,
    pub ticks: u64,
}
impl Validate for HostTimestamp {
    fn validate(&self) -> Result<()> {
        self.domain.validate()?;
        self.epoch.validate()
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockAnchor {
    pub id: AnchorId,
    pub host: HostTimestamp,
    pub audio: AudioFrame,
    /// Host ticks per second = numerator / denominator, never implicit ns.
    pub ticks_per_second_numerator: u64,
    pub ticks_per_second_denominator: u32,
    pub uncertainty_frames: u32,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for ClockAnchor {
    fn validate(&self) -> Result<()> {
        self.id.validate()?;
        self.host.validate()?;
        self.audio.validate()?;
        require(
            (1..=1_000_000_000_000).contains(&self.ticks_per_second_numerator)
                && (1..=1_000_000_000).contains(&self.ticks_per_second_denominator),
            "Invalid host clock timebase",
        )?;
        require(
            self.uncertainty_frames <= self.audio.sample_rate,
            "Invalid clock uncertainty",
        )?;
        extensions(
            &self.extensions,
            &[
                "id",
                "host",
                "audio",
                "ticks_per_second_numerator",
                "ticks_per_second_denominator",
                "uncertainty_frames",
            ],
        )
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureAlignment {
    pub anchor: ClockAnchor,
    pub audio: AudioFrame,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureTime {
    pub observed: HostTimestamp,
    pub alignment: Option<CaptureAlignment>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for CaptureTime {
    fn validate(&self) -> Result<()> {
        self.observed.validate()?;
        if let Some(alignment) = &self.alignment {
            let anchor = &alignment.anchor;
            anchor.validate()?;
            alignment.audio.validate()?;
            require(
                self.observed.domain == anchor.host.domain
                    && self.observed.epoch == anchor.host.epoch,
                "Capture and anchor have different host clock domains or epochs",
            )?;
            require(
                alignment.audio.epoch == anchor.audio.epoch
                    && alignment.audio.sample_rate == anchor.audio.sample_rate,
                "Aligned audio has a different epoch or sample rate",
            )?;
            // i128 bounds cover every permitted u64 tick and timebase product.
            let delta = i128::from(self.observed.ticks) - i128::from(anchor.host.ticks);
            let frames = delta
                * i128::from(anchor.audio.sample_rate)
                * i128::from(anchor.ticks_per_second_denominator)
                / i128::from(anchor.ticks_per_second_numerator);
            let predicted = i128::from(anchor.audio.frame) + frames;
            require(
                predicted >= i128::from(i64::MIN) && predicted <= i128::from(i64::MAX),
                "Clock conversion exceeds timeline range",
            )?;
            require(
                (i128::from(alignment.audio.frame) - predicted).abs()
                    <= i128::from(anchor.uncertainty_frames) + 1,
                "Aligned frame disagrees with its retained clock anchor",
            )?;
        }
        extensions(&self.extensions, &["observed", "alignment"])
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceAddress {
    pub endpoint: EndpointId,
    pub connection_generation: u64,
    pub group: u8,
    /// None for system/group-scoped messages; zero-based when present.
    pub channel: Option<u8>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for SourceAddress {
    fn validate(&self) -> Result<()> {
        self.endpoint.validate()?;
        require(
            self.connection_generation > 0
                && self.group < 16
                && self.channel.is_none_or(|v| v < 16),
            "Invalid MIDI source address",
        )?;
        extensions(
            &self.extensions,
            &["endpoint", "connection_generation", "group", "channel"],
        )
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "format",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RawPacket {
    Ump {
        words: Vec<u32>,
    },
    Midi1 {
        bytes: Vec<u8>,
    },
    Local,
    /// Malformed/unsupported source bytes remain in the immutable raw journal.
    Quarantined {
        original: BlobRef,
        reason: String,
    },
}
impl RawPacket {
    fn validate_source(&self, source: &SourceAddress) -> Result<()> {
        match self {
            Self::Ump { words } => {
                require(
                    !words.is_empty() && words.len() <= 4,
                    "Invalid UMP word count",
                )?;
                let mt = (words[0] >> 28) as usize;
                let sizes = [1, 1, 1, 2, 2, 4, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4];
                require(words.len() == sizes[mt], "Truncated UMP")?;
                // Stream/utility packets do not use this field as a MIDI group.
                if matches!(mt, 1..=5 | 13) {
                    require(
                        ((words[0] >> 24) & 15) as u8 == source.group,
                        "UMP group mismatch",
                    )?;
                }
                if matches!(mt, 2 | 4) {
                    require(
                        source.channel == Some(((words[0] >> 16) & 15) as u8),
                        "UMP channel mismatch",
                    )?;
                }
            }
            Self::Midi1 { bytes } => {
                require(
                    !bytes.is_empty() && bytes.len() <= 1_024,
                    "Invalid MIDI 1 capture size",
                )?;
                require(source.group == 0, "MIDI 1 byte transport has no UMP group")?;
                let status = bytes[0];
                if (0x80..0xf0).contains(&status) {
                    require(
                        source.channel == Some(status & 15),
                        "MIDI 1 channel mismatch",
                    )?;
                }
            }
            Self::Local => {}
            Self::Quarantined { original, reason } => {
                original.validate()?;
                text(reason, 512)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "unit",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExpressionValue {
    PitchCents(f64),
    Normalized(f64),
    ProtocolInteger { value: u32, bits: u8 },
}
impl Validate for ExpressionValue {
    fn validate(&self) -> Result<()> {
        match self {
            Self::PitchCents(v) => {
                require(v.is_finite() && v.abs() <= 19_200.0, "Invalid pitch offset")
            }
            Self::Normalized(v) => require(
                v.is_finite() && (0.0..=1.0).contains(v),
                "Invalid normalized expression",
            ),
            Self::ProtocolInteger { value, bits } => {
                require(
                    matches!(bits, 7 | 14 | 16 | 32),
                    "Unsupported protocol precision",
                )?;
                require(
                    u64::from(*value) < (1_u64 << bits),
                    "Expression exceeds declared precision",
                )
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "scope",
    content = "address",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExpressionTarget {
    Note { note_id: NoteId, voice_id: VoiceId },
    Channel,
    MpeZone { manager_channel: u8 },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionDimension {
    Pitch,
    Pressure,
    Timbre,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "origin",
    content = "reference",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExpressionOrigin {
    Measured(EventId),
    Edited(OperationId),
    Estimated(JobId),
}
impl Validate for ExpressionOrigin {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Measured(id) => id.validate(),
            Self::Edited(id) => id.validate(),
            Self::Estimated(id) => id.validate(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MusicalEvent {
    NoteOn {
        note_id: NoteId,
        voice_id: VoiceId,
        key: u8,
        velocity16: u16,
    },
    NoteOff {
        note_id: NoteId,
        voice_id: VoiceId,
        key: u8,
        release_velocity16: u16,
    },
    Expression {
        target: ExpressionTarget,
        dimension: ExpressionDimension,
        value: ExpressionValue,
        origin: ExpressionOrigin,
    },
    Controller {
        controller: u8,
        value: ExpressionValue,
    },
    Uninterpreted,
}
impl MusicalEvent {
    fn validate_source(&self, source: &SourceAddress) -> Result<()> {
        match self {
            Self::NoteOn {
                note_id,
                voice_id,
                key,
                ..
            }
            | Self::NoteOff {
                note_id,
                voice_id,
                key,
                ..
            } => {
                note_id.validate()?;
                voice_id.validate()?;
                require(
                    *key <= 127 && source.channel.is_some(),
                    "Note requires MIDI key and channel",
                )?;
            }
            Self::Expression {
                target,
                dimension,
                value,
                origin,
            } => {
                value.validate()?;
                origin.validate()?;
                require(
                    source.channel.is_some(),
                    "Expressive source requires a channel",
                )?;
                require(
                    !matches!(value, ExpressionValue::PitchCents(_))
                        || *dimension == ExpressionDimension::Pitch,
                    "Pitch units used on non-pitch lane",
                )?;
                match target {
                    ExpressionTarget::Note { note_id, voice_id } => {
                        note_id.validate()?;
                        voice_id.validate()?;
                    }
                    ExpressionTarget::MpeZone { manager_channel } => {
                        require(
                            matches!(manager_channel, 0 | 15)
                                && source.channel == Some(*manager_channel),
                            "Invalid MPE manager address",
                        )?;
                    }
                    ExpressionTarget::Channel => {}
                }
            }
            Self::Controller { controller, value } => {
                require(
                    *controller <= 127 && source.channel.is_some(),
                    "Invalid controller address",
                )?;
                value.validate()?;
                require(
                    !matches!(value, ExpressionValue::PitchCents(_)),
                    "Controller has pitch units",
                )?;
            }
            Self::Uninterpreted => {}
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedEvent {
    pub event_id: EventId,
    pub source: SourceAddress,
    pub time: CaptureTime,
    pub raw: RawPacket,
    pub musical: MusicalEvent,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for CapturedEvent {
    fn validate(&self) -> Result<()> {
        self.event_id.validate()?;
        self.source.validate()?;
        self.time.validate()?;
        self.raw.validate_source(&self.source)?;
        self.musical.validate_source(&self.source)?;
        if matches!(self.raw, RawPacket::Quarantined { .. }) {
            require(
                matches!(self.musical, MusicalEvent::Uninterpreted),
                "Quarantined packet must not drive a musical event",
            )?;
        }
        if let MusicalEvent::Expression { origin, .. } = &self.musical {
            require(
                matches!(origin, ExpressionOrigin::Measured(id) if id == &self.event_id),
                "Captured expression must refer to its original event",
            )?;
        }
        extensions(
            &self.extensions,
            &["event_id", "source", "time", "raw", "musical"],
        )
    }
}
wire!(CapturedEvent, "omniusong.capture", CAPTURE_MAX_BYTES);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "command",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AudioAction {
    PreparePlan {
        plan_id: ResourceId,
        render_revision: u64,
        object: BlobRef,
    },
    ActivatePlan {
        plan_id: ResourceId,
        render_revision: u64,
        at: AudioFrame,
    },
    Start {
        at: AudioFrame,
    },
    Stop,
    Seek {
        to: AudioFrame,
    },
    Schedule {
        events: BlobRef,
        at: AudioFrame,
    },
    SetParameter {
        node: u32,
        parameter: u32,
        normalized: f64,
        at: AudioFrame,
    },
    StartRecording {
        take_id: TakeId,
        at: AudioFrame,
    },
    StopRecording {
        take_id: TakeId,
    },
    Panic,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioCommand {
    pub project_id: ProjectId,
    pub action: AudioAction,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for AudioCommand {
    fn validate(&self) -> Result<()> {
        self.project_id.validate()?;
        match &self.action {
            AudioAction::PreparePlan {
                plan_id, object, ..
            } => {
                plan_id.validate()?;
                object.validate()?;
            }
            AudioAction::ActivatePlan { plan_id, at, .. } => {
                plan_id.validate()?;
                at.validate()?;
            }
            AudioAction::Start { at } => at.validate()?,
            AudioAction::Seek { to } => to.validate()?,
            AudioAction::Schedule { events, at } => {
                events.validate()?;
                at.validate()?;
            }
            AudioAction::SetParameter { normalized, at, .. } => {
                require(
                    normalized.is_finite() && (0.0..=1.0).contains(normalized),
                    "Invalid parameter value",
                )?;
                at.validate()?;
            }
            AudioAction::StartRecording { take_id, at } => {
                take_id.validate()?;
                at.validate()?;
            }
            AudioAction::StopRecording { take_id } => take_id.validate()?,
            AudioAction::Stop | AudioAction::Panic => {}
        }
        extensions(&self.extensions, &["project_id", "action"])
    }
}
wire!(AudioCommand, "omniusong.audio-command", CONTROL_MAX_BYTES);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
impl Validate for Failure {
    fn validate(&self) -> Result<()> {
        text(&self.code, 64)?;
        text(&self.message, 512)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AudioNotice {
    Queued,
    Prepared {
        plan_id: ResourceId,
    },
    Applied {
        render_revision: u64,
        at: AudioFrame,
    },
    Rejected(Failure),
    ClockChanged(ClockAnchor),
    PluginDeadlineMissed {
        instance: u32,
        at: AudioFrame,
        consecutive: u32,
    },
    CaptureGap {
        take_id: TakeId,
        first_source_frame: u64,
        frames: u64,
    },
    ChunkCommitted {
        take_id: TakeId,
        stream_id: ResourceId,
        sequence: u64,
        durable_source_frames: Option<u64>,
        durable_events: Option<u64>,
        object: BlobRef,
    },
    RecordingFinalized {
        take_id: TakeId,
        manifest: BlobRef,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioEvent {
    pub in_reply_to: Option<MessageId>,
    pub notice: AudioNotice,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for AudioEvent {
    fn validate(&self) -> Result<()> {
        if let Some(id) = &self.in_reply_to {
            id.validate()?;
        }
        match &self.notice {
            AudioNotice::Queued => {}
            AudioNotice::Prepared { plan_id } => plan_id.validate()?,
            AudioNotice::Applied { at, .. } => at.validate()?,
            AudioNotice::Rejected(failure) => failure.validate()?,
            AudioNotice::ClockChanged(anchor) => anchor.validate()?,
            AudioNotice::PluginDeadlineMissed {
                at, consecutive, ..
            } => {
                at.validate()?;
                require(*consecutive > 0, "Deadline miss count is zero")?;
            }
            AudioNotice::CaptureGap {
                take_id,
                first_source_frame,
                frames,
            } => {
                take_id.validate()?;
                require(
                    *frames > 0 && first_source_frame.checked_add(*frames).is_some(),
                    "Invalid capture gap",
                )?;
            }
            AudioNotice::ChunkCommitted {
                take_id,
                stream_id,
                durable_source_frames,
                durable_events,
                object,
                ..
            } => {
                take_id.validate()?;
                stream_id.validate()?;
                object.validate()?;
                require(
                    durable_source_frames.is_some() || durable_events.is_some(),
                    "Committed chunk has no durable prefix",
                )?;
                require(
                    durable_source_frames.is_none_or(|n| n > 0)
                        && durable_events.is_none_or(|n| n > 0),
                    "Empty committed prefix",
                )?;
            }
            AudioNotice::RecordingFinalized { take_id, manifest } => {
                take_id.validate()?;
                manifest.validate()?;
            }
        }
        extensions(&self.extensions, &["in_reply_to", "notice"])
    }
}
wire!(AudioEvent, "omniusong.audio-event", CONTROL_MAX_BYTES);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TickRange {
    pub start: u64,
    pub end: u64,
}
impl Validate for TickRange {
    fn validate(&self) -> Result<()> {
        require(self.end > self.start, "Empty or reversed musical scope")
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteAddress {
    pub track_id: TrackId,
    pub note_id: NoteId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "scope",
    content = "selection",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EditScope {
    Project,
    /// Exact legacy document keys; not transport or filesystem identities.
    DocumentNote {
        address: DocumentNoteAddress,
    },
    Tracks {
        track_ids: Vec<TrackId>,
        range: Option<TickRange>,
    },
    Notes {
        notes: Vec<NoteAddress>,
    },
    Lyrics {
        line_ids: Vec<String>,
    },
}
impl Validate for EditScope {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Project => {}
            Self::DocumentNote { address } => address.validate()?,
            Self::Tracks { track_ids, range } => {
                require(
                    !track_ids.is_empty() && track_ids.len() <= 64,
                    "Invalid track selection size",
                )?;
                for id in track_ids {
                    id.validate()?;
                }
                unique_ids(track_ids.iter().map(|id| id.0.as_str()))?;
                if let Some(range) = range {
                    range.validate()?;
                }
            }
            Self::Notes { notes } => {
                require(
                    !notes.is_empty() && notes.len() <= 256,
                    "Invalid note selection size",
                )?;
                let mut seen = std::collections::BTreeSet::new();
                for note in notes {
                    note.track_id.validate()?;
                    note.note_id.validate()?;
                    require(
                        seen.insert((&note.track_id, &note.note_id)),
                        "Duplicate note selection",
                    )?;
                }
            }
            Self::Lyrics { line_ids } => {
                require(
                    !line_ids.is_empty() && line_ids.len() <= 256,
                    "Invalid lyric selection size",
                )?;
                for id in line_ids {
                    text(id, 96)?;
                }
                unique_ids(line_ids.iter().map(String::as_str))?;
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EditOperation {
    Rename {
        name: String,
    },
    SetTempo {
        bpm: f64,
    },
    /// One user-facing draft save; name and tempo share a revision and Undo step.
    UpdateSession {
        name: String,
        tempo: f64,
    },
    AddNote {
        values: NoteValues,
        timeline: NoteTimelinePolicy,
    },
    UpdateNote {
        values: NoteValues,
        timeline: NoteTimelinePolicy,
    },
    DeleteNote,
    RestoreNote {
        timeline: NoteTimelinePolicy,
    },
    /// Versioned semantic edit object, not an unrestricted JSON merge patch.
    ApplyPreparedEdit {
        edit: BlobRef,
    },
    AcceptCandidate {
        candidate_id: CandidateId,
    },
    Undo,
    Redo,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectEditIntent {
    pub operation_id: OperationId,
    pub project_id: ProjectId,
    pub expected_revision: u64,
    pub scope: EditScope,
    pub edit: EditOperation,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for ProjectEditIntent {
    fn validate(&self) -> Result<()> {
        self.operation_id.validate()?;
        self.project_id.validate()?;
        self.scope.validate()?;
        match &self.edit {
            EditOperation::Rename { name } | EditOperation::UpdateSession { name, .. } => {
                text(name, 640)?;
                require(
                    name.chars().count() <= 160 && !name.trim().is_empty(),
                    "Invalid project name",
                )?;
                require(
                    matches!(self.scope, EditScope::Project),
                    "Session metadata edits require project scope",
                )?;
                if let EditOperation::UpdateSession { tempo, .. } = &self.edit {
                    require(
                        tempo.is_finite() && (30.0..=300.0).contains(tempo),
                        "Invalid tempo",
                    )?;
                }
            }
            EditOperation::SetTempo { bpm } => {
                require(
                    bpm.is_finite() && (30.0..=300.0).contains(bpm),
                    "Invalid tempo",
                )?;
                require(
                    matches!(self.scope, EditScope::Project),
                    "Tempo requires project scope",
                )?;
            }
            EditOperation::ApplyPreparedEdit { edit } => edit.validate()?,
            EditOperation::AcceptCandidate { candidate_id } => candidate_id.validate()?,
            EditOperation::AddNote { values, .. } | EditOperation::UpdateNote { values, .. } => {
                values.validate()?;
                let EditScope::DocumentNote { address } = &self.scope else {
                    return Err(ContractError(
                        "Note edits require one exact document-note scope".into(),
                    ));
                };
                if matches!(self.edit, EditOperation::AddNote { .. }) {
                    NoteId(address.note_id.clone()).validate()?;
                }
            }
            EditOperation::DeleteNote | EditOperation::RestoreNote { .. } => require(
                matches!(self.scope, EditScope::DocumentNote { .. }),
                "Note edits require one exact document-note scope",
            )?,
            EditOperation::Undo | EditOperation::Redo => require(
                matches!(self.scope, EditScope::Project),
                "History navigation requires project scope",
            )?,
        }
        extensions(
            &self.extensions,
            &[
                "operation_id",
                "project_id",
                "expected_revision",
                "scope",
                "edit",
            ],
        )
    }
}
wire!(
    ProjectEditIntent,
    "omniusong.project-edit",
    WORKER_MAX_BYTES
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Lyrics,
    Transcription,
    SymbolicComposition,
    FullSong,
    StemSeparation,
    InstrumentCreation,
    SingingSynthesis,
    InteractiveAccompaniment,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub provider: String,
    pub model: String,
    pub revision: String,
    /// Model/config/tokenizer manifest; pins the full dependency set by hash.
    pub artifact_manifest: BlobRef,
    pub runtime: String,
    pub runtime_version: String,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for ModelIdentity {
    fn validate(&self) -> Result<()> {
        for v in [
            &self.provider,
            &self.model,
            &self.revision,
            &self.runtime,
            &self.runtime_version,
        ] {
            text(v, 256)?;
        }
        self.artifact_manifest.validate()?;
        extensions(
            &self.extensions,
            &[
                "provider",
                "model",
                "revision",
                "artifact_manifest",
                "runtime",
                "runtime_version",
            ],
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobBudget {
    pub wall_time_ms: u64,
    pub memory_bytes: u64,
    pub cpu_threads: u16,
    pub output_bytes: u64,
}
impl Validate for JobBudget {
    fn validate(&self) -> Result<()> {
        require(
            (1..=86_400_000).contains(&self.wall_time_ms)
                && (1..=MAX_BLOB_BYTES).contains(&self.memory_bytes)
                && (1..=256).contains(&self.cpu_threads)
                && (1..=MAX_BLOB_BYTES).contains(&self.output_bytes),
            "Invalid job budget",
        )
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRequest {
    pub job_id: JobId,
    pub operation_id: OperationId,
    pub project_id: ProjectId,
    pub base_revision: u64,
    pub scope: EditScope,
    pub role: ModelRole,
    pub model: ModelIdentity,
    pub inputs: Vec<BlobRef>,
    pub parameters: Value,
    pub budget: JobBudget,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for JobRequest {
    fn validate(&self) -> Result<()> {
        self.job_id.validate()?;
        self.operation_id.validate()?;
        self.project_id.validate()?;
        self.scope.validate()?;
        self.model.validate()?;
        self.budget.validate()?;
        require(
            self.inputs.len() <= 64,
            "Too many job inputs; use a manifest",
        )?;
        for input in &self.inputs {
            input.validate()?;
        }
        require(
            self.parameters.is_object(),
            "Job parameters must be an object",
        )?;
        json_shape(&self.parameters, 0, &mut 0)?;
        extensions(
            &self.extensions,
            &[
                "job_id",
                "operation_id",
                "project_id",
                "base_revision",
                "scope",
                "role",
                "model",
                "inputs",
                "parameters",
                "budget",
            ],
        )
    }
}
wire!(JobRequest, "omniusong.job-request", WORKER_MAX_BYTES);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobCancel {
    pub job_id: JobId,
    pub operation_id: OperationId,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for JobCancel {
    fn validate(&self) -> Result<()> {
        self.job_id.validate()?;
        self.operation_id.validate()?;
        extensions(&self.extensions, &["job_id", "operation_id"])
    }
}
wire!(JobCancel, "omniusong.job-cancel", CONTROL_MAX_BYTES);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Loading,
    Running,
    Cancelling,
    Cancelled,
    Failed,
    Succeeded,
    Interrupted,
}
impl JobState {
    pub fn allows(self, next: Self) -> bool {
        use JobState::*;
        matches!(
            (self, next),
            (Queued, Loading | Cancelling | Failed | Interrupted)
                | (Loading, Running | Cancelling | Failed | Interrupted)
                | (
                    Running,
                    Running | Cancelling | Succeeded | Failed | Interrupted
                )
                | (Cancelling, Cancelled | Failed | Interrupted)
        )
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobUpdate {
    pub job_id: JobId,
    pub state: JobState,
    pub progress: Option<f64>,
    pub candidate_id: Option<CandidateId>,
    pub failure: Option<Failure>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for JobUpdate {
    fn validate(&self) -> Result<()> {
        self.job_id.validate()?;
        if let Some(v) = self.progress {
            require(
                v.is_finite() && (0.0..=1.0).contains(&v),
                "Invalid progress",
            )?;
        }
        if let Some(id) = &self.candidate_id {
            id.validate()?;
        }
        if let Some(failure) = &self.failure {
            failure.validate()?;
        }
        require(
            self.candidate_id.is_some() == (self.state == JobState::Succeeded),
            "Only succeeded jobs name a validated candidate",
        )?;
        require(
            self.failure.is_some()
                == matches!(self.state, JobState::Failed | JobState::Interrupted),
            "Failed/interrupted job requires a failure receipt",
        )?;
        extensions(
            &self.extensions,
            &["job_id", "state", "progress", "candidate_id", "failure"],
        )
    }
}
wire!(JobUpdate, "omniusong.job-update", WORKER_MAX_BYTES);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    pub job_id: JobId,
    pub project_id: ProjectId,
    pub base_revision: u64,
    pub candidate_id: CandidateId,
    pub scope: EditScope,
    pub model: ModelIdentity,
    pub outputs: Vec<BlobRef>,
    pub proposal: BlobRef,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for JobResult {
    fn validate(&self) -> Result<()> {
        self.job_id.validate()?;
        self.project_id.validate()?;
        self.candidate_id.validate()?;
        self.scope.validate()?;
        self.model.validate()?;
        self.proposal.validate()?;
        require(
            !self.outputs.is_empty() && self.outputs.len() <= 64,
            "Invalid result artifact count",
        )?;
        for output in &self.outputs {
            output.validate()?;
        }
        extensions(
            &self.extensions,
            &[
                "job_id",
                "project_id",
                "base_revision",
                "candidate_id",
                "scope",
                "model",
                "outputs",
                "proposal",
            ],
        )
    }
}
wire!(JobResult, "omniusong.job-result", WORKER_MAX_BYTES);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Ready,
    Auditioning,
    Accepted,
    Discarded,
    Invalidated,
}
impl CandidateState {
    pub fn allows(self, next: Self) -> bool {
        use CandidateState::*;
        matches!(
            (self, next),
            (Ready, Auditioning | Accepted | Discarded | Invalidated)
                | (Auditioning, Ready | Accepted | Discarded | Invalidated)
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDecisionKind {
    Audition,
    StopAudition,
    Accept,
    Discard,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateDecision {
    pub candidate_id: CandidateId,
    pub operation_id: OperationId,
    pub project_id: ProjectId,
    pub expected_revision: u64,
    pub decision: CandidateDecisionKind,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for CandidateDecision {
    fn validate(&self) -> Result<()> {
        self.candidate_id.validate()?;
        self.operation_id.validate()?;
        self.project_id.validate()?;
        extensions(
            &self.extensions,
            &[
                "candidate_id",
                "operation_id",
                "project_id",
                "expected_revision",
                "decision",
            ],
        )
    }
}
wire!(
    CandidateDecision,
    "omniusong.candidate-decision",
    CONTROL_MAX_BYTES
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    AudioUnit,
    Vst3,
    Clap,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "role",
    content = "identity",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorkerRole {
    Engine,
    PluginScanner {
        format: PluginFormat,
    },
    PluginInstance {
        format: PluginFormat,
        plugin_id: String,
    },
    Model {
        provider: String,
    },
}
impl Validate for WorkerRole {
    fn validate(&self) -> Result<()> {
        match self {
            Self::PluginInstance { plugin_id, .. } => text(plugin_id, 512),
            Self::Model { provider } => text(provider, 256),
            _ => Ok(()),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Windows,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    Aarch64,
    X86_64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerLimits {
    pub control_bytes: u32,
    pub worker_bytes: u32,
    pub realtime_events: u16,
    pub block_frames: u32,
}
impl Validate for WorkerLimits {
    fn validate(&self) -> Result<()> {
        require(
            self.control_bytes > 0
                && self.control_bytes <= CONTROL_MAX_BYTES as u32
                && self.worker_bytes > 0
                && self.worker_bytes <= WORKER_MAX_BYTES as u32
                && self.realtime_events > 0
                && self.realtime_events as usize <= REALTIME_EVENT_CAPACITY
                && self.block_frames > 0
                && self.block_frames <= MAX_BLOCK_FRAMES,
            "Invalid negotiated worker limits",
        )
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerHello {
    pub worker_id: WorkerId,
    /// Supervisor-issued per-launch nonce; supervisor must compare, not trust it.
    pub launch_nonce: String,
    pub role: WorkerRole,
    pub platform: Platform,
    pub architecture: Architecture,
    pub build_id: String,
    pub min_protocol: u16,
    pub max_protocol: u16,
    pub limits: WorkerLimits,
    pub capabilities: Vec<String>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl fmt::Debug for WorkerHello {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerHello")
            .field("worker_id", &self.worker_id)
            .field("launch_nonce", &"[redacted]")
            .field("role", &self.role)
            .field("platform", &self.platform)
            .field("architecture", &self.architecture)
            .field("build_id", &self.build_id)
            .field("min_protocol", &self.min_protocol)
            .field("max_protocol", &self.max_protocol)
            .field("limits", &self.limits)
            .field("capabilities_count", &self.capabilities.len())
            .field("extensions_count", &self.extensions.len())
            .finish()
    }
}
impl Validate for WorkerHello {
    fn validate(&self) -> Result<()> {
        self.worker_id.validate()?;
        self.role.validate()?;
        self.limits.validate()?;
        if matches!(
            self.role,
            WorkerRole::PluginScanner {
                format: PluginFormat::AudioUnit
            } | WorkerRole::PluginInstance {
                format: PluginFormat::AudioUnit,
                ..
            }
        ) {
            require(
                self.platform == Platform::Macos,
                "Audio Units require a Mac worker",
            )?;
        }
        require(
            self.launch_nonce.len() == 64
                && self.launch_nonce.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid worker launch nonce",
        )?;
        text(&self.build_id, 256)?;
        require(
            self.min_protocol > 0
                && self.min_protocol <= PROTOCOL_VERSION
                && self.max_protocol >= PROTOCOL_VERSION,
            "No supported worker protocol overlap",
        )?;
        require(
            self.capabilities.len() <= 64,
            "Too many worker capabilities",
        )?;
        for capability in &self.capabilities {
            text(capability, 128)?;
        }
        unique_ids(self.capabilities.iter().map(String::as_str))?;
        extensions(
            &self.extensions,
            &[
                "worker_id",
                "launch_nonce",
                "role",
                "platform",
                "architecture",
                "build_id",
                "min_protocol",
                "max_protocol",
                "limits",
                "capabilities",
            ],
        )
    }
}
wire!(WorkerHello, "omniusong.worker-hello", WORKER_MAX_BYTES);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerWelcome {
    pub worker_id: WorkerId,
    pub negotiated_protocol: u16,
    pub limits: WorkerLimits,
    pub grants: Vec<BlobRef>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}
impl Validate for WorkerWelcome {
    fn validate(&self) -> Result<()> {
        self.worker_id.validate()?;
        self.limits.validate()?;
        require(
            self.negotiated_protocol == PROTOCOL_VERSION,
            "Unsupported negotiated protocol",
        )?;
        require(self.grants.len() <= 64, "Too many initial object grants")?;
        for grant in &self.grants {
            grant.validate()?;
        }
        extensions(
            &self.extensions,
            &["worker_id", "negotiated_protocol", "limits", "grants"],
        )
    }
}
wire!(WorkerWelcome, "omniusong.worker-welcome", WORKER_MAX_BYTES);

/// Process-local prepared handles. Never serialize Rust layout across processes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RealtimeEventKind {
    NoteOn {
        note: u64,
        voice: u64,
        key: u8,
        velocity16: u16,
    },
    NoteOff {
        voice: u64,
        release_velocity16: u16,
    },
    Expression {
        voice: u64,
        dimension: RealtimeDimension,
        value: f64,
    },
    Parameter {
        node: u32,
        parameter: u32,
        normalized: f64,
    },
    Ump {
        port: u16,
        words: [u32; 4],
        word_count: u8,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeDimension {
    PitchCents,
    Pressure,
    Timbre,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RealtimeEvent {
    pub frame_offset: u32,
    pub kind: RealtimeEventKind,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeReject {
    Capacity,
    FrameRange,
    Order,
    Value,
    Epoch,
    BlockSize,
}

/// Prepared process-local handles only; applied at a block boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeCommand {
    ActivatePlan {
        plan_handle: u64,
        render_revision: u64,
    },
    Start {
        timeline_frame: i64,
    },
    Stop,
    Seek {
        timeline_frame: i64,
    },
    StartRecording {
        take_handle: u64,
    },
    StopRecording {
        take_handle: u64,
    },
    Panic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeControl {
    pub epoch_handle: u64,
    pub command: RealtimeCommand,
}
impl RealtimeControl {
    pub fn validate(self) -> std::result::Result<(), RealtimeReject> {
        if self.epoch_handle == 0 {
            return Err(RealtimeReject::Epoch);
        }
        let valid = match self.command {
            RealtimeCommand::ActivatePlan { plan_handle, .. } => plan_handle > 0,
            RealtimeCommand::StartRecording { take_handle }
            | RealtimeCommand::StopRecording { take_handle } => take_handle > 0,
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(RealtimeReject::Value)
        }
    }
}
impl RealtimeEvent {
    fn valid(self) -> bool {
        match self.kind {
            RealtimeEventKind::NoteOn {
                note, voice, key, ..
            } => note > 0 && voice > 0 && key <= 127,
            RealtimeEventKind::NoteOff { voice, .. } => voice > 0,
            RealtimeEventKind::Expression {
                voice,
                dimension,
                value,
            } => {
                voice > 0
                    && value.is_finite()
                    && match dimension {
                        RealtimeDimension::PitchCents => value.abs() <= 19_200.0,
                        _ => (0.0..=1.0).contains(&value),
                    }
            }
            RealtimeEventKind::Parameter { normalized, .. } => {
                normalized.is_finite() && (0.0..=1.0).contains(&normalized)
            }
            RealtimeEventKind::Ump {
                words, word_count, ..
            } => {
                let sizes = [1, 1, 1, 2, 2, 4, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4];
                word_count == sizes[(words[0] >> 28) as usize]
            }
        }
    }
}
const EMPTY_RT_EVENT: RealtimeEvent = RealtimeEvent {
    frame_offset: 0,
    kind: RealtimeEventKind::NoteOff {
        voice: 1,
        release_velocity16: 0,
    },
};

/// Fixed storage with ordered sample offsets; no heap, refcounts or destructors.
/// This is an in-process type, not a stable shared-memory or third-party ABI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RealtimeBatch {
    epoch_handle: u64,
    start_frame: i64,
    frames: u32,
    len: u16,
    events: [RealtimeEvent; REALTIME_EVENT_CAPACITY],
}
impl RealtimeBatch {
    pub fn new(
        epoch_handle: u64,
        start_frame: i64,
        frames: u32,
    ) -> std::result::Result<Self, RealtimeReject> {
        if epoch_handle == 0 {
            return Err(RealtimeReject::Epoch);
        }
        if frames == 0
            || frames > MAX_BLOCK_FRAMES
            || start_frame.checked_add(i64::from(frames)).is_none()
        {
            return Err(RealtimeReject::BlockSize);
        }
        Ok(Self {
            epoch_handle,
            start_frame,
            frames,
            len: 0,
            events: [EMPTY_RT_EVENT; REALTIME_EVENT_CAPACITY],
        })
    }
    pub fn try_push(&mut self, event: RealtimeEvent) -> std::result::Result<(), RealtimeReject> {
        if self.len as usize == REALTIME_EVENT_CAPACITY {
            return Err(RealtimeReject::Capacity);
        }
        if event.frame_offset >= self.frames {
            return Err(RealtimeReject::FrameRange);
        }
        if self.len > 0 && event.frame_offset < self.events[self.len as usize - 1].frame_offset {
            return Err(RealtimeReject::Order);
        }
        if !event.valid() {
            return Err(RealtimeReject::Value);
        }
        self.events[self.len as usize] = event;
        self.len += 1;
        Ok(())
    }
    pub fn events(&self) -> &[RealtimeEvent] {
        &self.events[..self.len as usize]
    }
    pub fn epoch_handle(&self) -> u64 {
        self.epoch_handle
    }
    pub fn start_frame(&self) -> i64 {
        self.start_frame
    }
    pub fn frames(&self) -> u32 {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn blob() -> BlobRef {
        BlobRef {
            sha256: "a".repeat(64),
            byte_len: 123,
            media_type: "application/x-omniusong-manifest+json".into(),
            extensions: Extensions::new(),
        }
    }
    fn captured() -> CapturedEvent {
        let host = HostTimestamp {
            domain: ClockDomainId("mach-host-time".into()),
            epoch: ClockEpochId("host-boot-1".into()),
            ticks: 10_000,
        };
        let audio = AudioFrame {
            epoch: ClockEpochId("device-run-2".into()),
            sample_rate: 48_000,
            frame: -480,
        };
        CapturedEvent {
            event_id: EventId("event-1".into()),
            source: SourceAddress {
                endpoint: EndpointId("coremidi:42".into()),
                connection_generation: 1,
                group: 0,
                channel: Some(0),
                extensions: Extensions::new(),
            },
            time: CaptureTime {
                observed: HostTimestamp {
                    ticks: 20_000,
                    ..host.clone()
                },
                alignment: Some(CaptureAlignment {
                    anchor: ClockAnchor {
                        id: AnchorId("anchor-1".into()),
                        host,
                        audio: audio.clone(),
                        ticks_per_second_numerator: 1_000_000,
                        ticks_per_second_denominator: 1,
                        uncertainty_frames: 0,
                        extensions: Extensions::new(),
                    },
                    audio: AudioFrame { frame: 0, ..audio },
                }),
                extensions: Extensions::new(),
            },
            raw: RawPacket::Ump {
                words: vec![0x4090_3c00, 0xffff_0000],
            },
            musical: MusicalEvent::NoteOn {
                note_id: NoteId("note-1".into()),
                voice_id: VoiceId("capture-voice-1".into()),
                key: 60,
                velocity16: u16::MAX,
            },
            extensions: Extensions::new(),
        }
    }
    fn envelope<T: WirePayload>(payload: T) -> Envelope<T> {
        Envelope::new(
            SessionId("session-1".into()),
            MessageId("message-1".into()),
            7,
            payload,
        )
    }

    #[test]
    fn captured_roundtrip_preserves_raw_precision_extensions_and_signed_clock() {
        let mut event = captured();
        event.extensions.insert(
            "vendor:exact".into(),
            serde_json::from_str(
                r#"{"decimal":0.12345678901234567890123456789,"counter":18446744073709551616000}"#,
            )
            .unwrap(),
        );
        let mut original = envelope(event);
        original.extensions.insert(
            "future:receipt".into(),
            serde_json::json!({"assetId":"remote-not-a-file"}),
        );
        original.extensions.insert(
            "future:counter".into(),
            serde_json::from_str("18446744073709551616001").unwrap(),
        );
        original.payload.extensions.insert(
            "vendor:decimal".into(),
            serde_json::from_str("0.1234567890123456789012345678901").unwrap(),
        );
        let bytes = original.encode().unwrap();
        let decoded = Envelope::<CapturedEvent>::decode(&bytes).unwrap();
        assert_eq!(original, decoded);
        // Inspect literal wire shape as well as Value equality: flattened
        // arbitrary-precision numbers must not become private-marker objects.
        let wire: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(wire["future:counter"].is_number());
        assert!(wire["payload"]["vendor:decimal"].is_number());
        assert!(!String::from_utf8(bytes).unwrap().contains("$serde_json"));
        assert_eq!(
            decoded.extensions["future:counter"].to_string(),
            "18446744073709551616001"
        );
        assert_eq!(
            decoded.payload.extensions["vendor:decimal"].to_string(),
            "0.1234567890123456789012345678901"
        );
        assert_eq!(
            decoded.payload.extensions["vendor:exact"]["counter"].to_string(),
            "18446744073709551616000"
        );
        assert_eq!(
            decoded.payload.extensions["vendor:exact"]["decimal"].to_string(),
            "0.12345678901234567890123456789"
        );
        assert_eq!(
            decoded.payload.time.alignment.unwrap().anchor.audio.frame,
            -480
        );
    }

    #[test]
    fn version_protocol_size_and_reserved_extension_are_rejected() {
        let mut message = envelope(AudioCommand {
            project_id: ProjectId("project-1".into()),
            action: AudioAction::Panic,
            extensions: Extensions::new(),
        });
        let good = message.encode().unwrap();
        let mut raw: Value = serde_json::from_slice(&good).unwrap();
        raw["version"] = 2.into();
        assert!(Envelope::<AudioCommand>::decode(&serde_json::to_vec(&raw).unwrap()).is_err());
        assert!(Envelope::<JobUpdate>::decode(&good).is_err());
        assert!(Envelope::<AudioCommand>::decode(&vec![b' '; CONTROL_MAX_BYTES + 1]).is_err());
        message.extensions.insert("version".into(), 99.into());
        assert!(message.encode().is_err());
        message.extensions.clear();
        message.extensions.insert(
            "vendor:oversized".into(),
            Value::String("x".repeat(CONTROL_MAX_BYTES)),
        );
        assert!(message.encode().is_err());
    }

    #[test]
    fn clock_integrity_rejects_cross_epoch_cross_domain_rate_and_false_alignment() {
        for mutate in 0..5 {
            let mut event = captured();
            match mutate {
                0 => event.time.observed.domain = ClockDomainId("qpc".into()),
                1 => event.time.observed.epoch = ClockEpochId("host-boot-2".into()),
                2 => event.time.alignment.as_mut().unwrap().audio.sample_rate = 44_100,
                3 => event.time.alignment.as_mut().unwrap().audio.frame = 100,
                _ => {
                    event
                        .time
                        .alignment
                        .as_mut()
                        .unwrap()
                        .anchor
                        .ticks_per_second_numerator = 0
                }
            }
            assert!(envelope(event).encode().is_err());
        }
        let mut earlier = captured();
        earlier.time.observed.ticks = 0;
        earlier.time.alignment.as_mut().unwrap().audio.frame = -960;
        assert!(envelope(earlier).encode().is_ok());
    }

    #[test]
    fn address_and_resolution_are_checked_without_inventing_wire_note_ids() {
        let mut event = captured();
        event.source.channel = Some(2);
        assert!(event.validate().is_err());
        event = captured();
        event.raw = RawPacket::Ump {
            words: vec![0x4090_3c00],
        };
        assert!(event.validate().is_err());
        assert!(
            ExpressionValue::ProtocolInteger {
                value: 128,
                bits: 7
            }
            .validate()
            .is_err()
        );
        assert!(
            ExpressionValue::ProtocolInteger {
                value: u32::MAX,
                bits: 32
            }
            .validate()
            .is_ok()
        );
        event = captured();
        event.musical = MusicalEvent::Expression {
            target: ExpressionTarget::MpeZone { manager_channel: 5 },
            dimension: ExpressionDimension::Pitch,
            value: ExpressionValue::PitchCents(0.0),
            origin: ExpressionOrigin::Measured(event.event_id.clone()),
        };
        assert!(event.validate().is_err());
    }

    #[test]
    fn realtime_batch_rejects_overflow_disorder_and_bad_offsets_without_mutation() {
        let mut batch = RealtimeBatch::new(1, -64, 64).unwrap();
        let event = RealtimeEvent {
            frame_offset: 1,
            kind: RealtimeEventKind::NoteOn {
                note: 2,
                voice: 3,
                key: 60,
                velocity16: 65_535,
            },
        };
        batch.try_push(event).unwrap();
        let before = batch;
        assert_eq!(
            batch.try_push(RealtimeEvent {
                frame_offset: 64,
                ..event
            }),
            Err(RealtimeReject::FrameRange)
        );
        assert_eq!(
            batch.try_push(RealtimeEvent {
                frame_offset: 0,
                ..event
            }),
            Err(RealtimeReject::Order)
        );
        assert_eq!(batch, before);
        for _ in 1..REALTIME_EVENT_CAPACITY {
            batch.try_push(event).unwrap();
        }
        assert_eq!(batch.try_push(event), Err(RealtimeReject::Capacity));
        assert_eq!(batch.events().len(), REALTIME_EVENT_CAPACITY);
        assert!(RealtimeBatch::new(1, i64::MAX, 64).is_err());
        assert!(!std::mem::needs_drop::<RealtimeBatch>());
        assert!(!std::mem::needs_drop::<RealtimeControl>());
        assert_eq!(
            (RealtimeControl {
                epoch_handle: 1,
                command: RealtimeCommand::StartRecording { take_handle: 0 }
            })
            .validate(),
            Err(RealtimeReject::Value)
        );
    }

    #[test]
    fn worker_handshake_and_model_candidate_contract_roundtrip() {
        let hello = WorkerHello {
            worker_id: WorkerId("worker-1".into()),
            launch_nonce: "b".repeat(64),
            role: WorkerRole::PluginInstance {
                format: PluginFormat::Clap,
                plugin_id: "org.example.synth".into(),
            },
            platform: Platform::Macos,
            architecture: Architecture::Aarch64,
            build_id: "test-build".into(),
            min_protocol: 1,
            max_protocol: 1,
            limits: WorkerLimits {
                control_bytes: CONTROL_MAX_BYTES as u32,
                worker_bytes: WORKER_MAX_BYTES as u32,
                realtime_events: REALTIME_EVENT_CAPACITY as u16,
                block_frames: MAX_BLOCK_FRAMES,
            },
            capabilities: vec!["clap.note-expression".into()],
            extensions: Extensions::new(),
        };
        assert!(!format!("{hello:?}").contains(&hello.launch_nonce));
        assert!(format!("{hello:?}").contains("[redacted]"));
        let wire = envelope(hello);
        assert!(!format!("{wire:?}").contains(&wire.payload.launch_nonce));
        assert_eq!(
            Envelope::<WorkerHello>::decode(&wire.encode().unwrap()).unwrap(),
            wire
        );
        let mut bad = wire.clone();
        bad.payload.min_protocol = 2;
        bad.payload.max_protocol = 3;
        assert!(bad.encode().is_err());
        let result = envelope(JobResult {
            job_id: JobId("job-1".into()),
            project_id: ProjectId("project-1".into()),
            base_revision: 9,
            candidate_id: CandidateId("candidate-1".into()),
            scope: EditScope::Project,
            model: ModelIdentity {
                provider: "native".into(),
                model: "fixture".into(),
                revision: "r1".into(),
                artifact_manifest: blob(),
                runtime: "test".into(),
                runtime_version: "1".into(),
                extensions: Extensions::new(),
            },
            outputs: vec![blob()],
            proposal: blob(),
            extensions: Extensions::new(),
        });
        assert_eq!(
            Envelope::<JobResult>::decode(&result.encode().unwrap()).unwrap(),
            result
        );
    }

    #[test]
    fn terminal_lifecycles_and_edit_scopes_are_explicit() {
        assert!(JobState::Running.allows(JobState::Cancelling));
        assert!(JobState::Cancelling.allows(JobState::Cancelled));
        assert!(!JobState::Cancelling.allows(JobState::Succeeded));
        assert!(!JobState::Interrupted.allows(JobState::Running));
        assert!(!CandidateState::Accepted.allows(CandidateState::Ready));
        assert!(CandidateState::Auditioning.allows(CandidateState::Accepted));
        let edit = envelope(ProjectEditIntent {
            operation_id: OperationId("edit-1".into()),
            project_id: ProjectId("project-1".into()),
            expected_revision: 7,
            scope: EditScope::Notes {
                notes: vec![NoteAddress {
                    track_id: TrackId("track-1".into()),
                    note_id: NoteId("note-1".into()),
                }],
            },
            edit: EditOperation::SetTempo { bpm: 120.0 },
            extensions: Extensions::new(),
        });
        assert!(edit.encode().is_err());
        let invalid = JobUpdate {
            job_id: JobId("job-1".into()),
            state: JobState::Succeeded,
            progress: Some(1.0),
            candidate_id: None,
            failure: None,
            extensions: Extensions::new(),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn session_draft_save_has_one_checked_project_scope_and_retains_extensions() {
        let mut request = ProjectEditIntent {
            operation_id: OperationId("session-save".into()),
            project_id: ProjectId("session".into()),
            expected_revision: 9,
            scope: EditScope::Project,
            edit: EditOperation::UpdateSession {
                name: "Human performance".into(),
                tempo: 117.25,
            },
            extensions: Extensions::from([(
                "vendor".into(),
                serde_json::from_str("18446744073709551616001").unwrap(),
            )]),
        };
        let wire = envelope(request.clone());
        assert_eq!(
            Envelope::<ProjectEditIntent>::decode(&wire.encode().unwrap()).unwrap(),
            wire
        );
        request.scope = EditScope::Tracks {
            track_ids: vec![TrackId("track".into())],
            range: None,
        };
        assert!(request.validate().is_err());
        request.scope = EditScope::Project;
        for tempo in [f64::NAN, f64::INFINITY, 29.999, 300.001] {
            request.edit = EditOperation::UpdateSession {
                name: "Human performance".into(),
                tempo,
            };
            assert!(request.validate().is_err());
        }
        for name in [" ".into(), "a".repeat(161), "line\nbreak".into()] {
            request.edit = EditOperation::UpdateSession {
                name,
                tempo: 117.25,
            };
            assert!(request.validate().is_err());
        }
    }

    #[test]
    fn midi_only_commits_and_cancellation_have_concrete_wire_records() {
        let committed = envelope(AudioEvent {
            in_reply_to: None,
            notice: AudioNotice::ChunkCommitted {
                take_id: TakeId("take-1".into()),
                stream_id: ResourceId("midi-stream-1".into()),
                sequence: 3,
                durable_source_frames: None,
                durable_events: Some(100_000),
                object: blob(),
            },
            extensions: Extensions::new(),
        });
        assert_eq!(
            Envelope::<AudioEvent>::decode(&committed.encode().unwrap()).unwrap(),
            committed
        );
        let cancel = envelope(JobCancel {
            job_id: JobId("job-1".into()),
            operation_id: OperationId("cancel-1".into()),
            extensions: Extensions::new(),
        });
        assert_eq!(
            Envelope::<JobCancel>::decode(&cancel.encode().unwrap()).unwrap(),
            cancel
        );
    }
}
