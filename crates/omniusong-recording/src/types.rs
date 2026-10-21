use omniusong_core::contracts::{CapturedEvent, Envelope, Validate};
pub use omniusong_core::contracts::{
    ClockAnchor, ClockDomainId, ClockEpochId, HostTimestamp, TakeId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, io,
};

pub const JOURNAL_VERSION: u32 = 1;
pub const CHUNK_VERSION: u32 = 1;
pub const MAX_AUDIO_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_EVENT_CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_METADATA_BYTES: usize = 256 * 1024;
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_STREAMS: usize = 64;
pub const MAX_APPEND_GROUP_CHUNKS: usize = 64;
pub const MAX_APPEND_GROUP_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum RecordingError {
    Invalid(String),
    Conflict(String),
    Owned,
    Interrupted,
    /// An export failed. Any private staging object or published orphan is retained.
    Backup {
        staging_path: Option<std::path::PathBuf>,
        destination_path: std::path::PathBuf,
        published: bool,
        message: String,
    },
    Database(rusqlite::Error),
    Io(io::Error),
}
impl fmt::Display for RecordingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Conflict(message) => f.write_str(message),
            Self::Owned => f.write_str("The take journal already has a live writer or recovery owner"),
            Self::Interrupted => f.write_str("Interrupted capture cannot resume; start a new take or explicitly seal its recovered prefix"),
            Self::Backup { staging_path, destination_path, published, message } => write!(
                f,
                "Recording backup failed: {message}; destination {} (published={published}); retained staging {:?}",
                destination_path.display(), staging_path,
            ),
            Self::Database(error) => error.fmt(f), Self::Io(error) => error.fmt(f),
        }
    }
}
impl Error for RecordingError {}
impl From<rusqlite::Error> for RecordingError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}
impl From<io::Error> for RecordingError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
pub type Result<T> = std::result::Result<T, RecordingError>;
pub(crate) fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(RecordingError::Invalid(message.into()))
    }
}
pub(crate) fn identifier(id: &str) -> Result<()> {
    require(
        !id.is_empty()
            && id.len() <= 96
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)),
        "Invalid recording identity",
    )
}
pub(crate) fn bounded_json<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| RecordingError::Invalid(error.to_string()))?;
    require(
        bytes.len() <= limit,
        "Recording metadata exceeds its byte limit",
    )?;
    Ok(bytes)
}
pub(crate) fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8], limit: usize) -> Result<T> {
    require(
        bytes.len() <= limit,
        "Stored recording metadata exceeds its byte limit",
    )?;
    serde_json::from_slice(bytes)
        .map_err(|error| RecordingError::Invalid(format!("Invalid recording metadata: {error}")))
}
pub(crate) fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostClock {
    pub domain: ClockDomainId,
    pub epoch: ClockEpochId,
    pub ticks_per_second_numerator: u64,
    pub ticks_per_second_denominator: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcmEncoding {
    S16Le,
    S24Le,
    F32Le,
}
impl PcmEncoding {
    pub fn bytes_per_sample(self) -> u64 {
        match self {
            Self::S16Le => 2,
            Self::S24Le => 3,
            Self::F32Le => 4,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcmLayout {
    Interleaved,
    Planar,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub encoding: PcmEncoding,
    pub layout: PcmLayout,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawEventFormat {
    LengthPrefixedCaptureV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StreamKind {
    Audio {
        format: PcmFormat,
        audio_epoch: ClockEpochId,
    },
    RawEvents {
        format: RawEventFormat,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamDescriptor {
    pub stream_id: String,
    pub segment_id: String,
    pub source_id: String,
    pub source_name: String,
    pub source_connection_generation: u64,
    pub host_clock: HostClock,
    pub kind: StreamKind,
    pub initial_source_position: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeDescriptor {
    pub version: u32,
    pub take_id: TakeId,
    pub streams: Vec<StreamDescriptor>,
    pub provenance: BTreeMap<String, Value>,
}
impl TakeDescriptor {
    pub fn validate(&self) -> Result<()> {
        require(
            self.version == JOURNAL_VERSION,
            "Unsupported recording descriptor version",
        )?;
        self.take_id
            .validate()
            .map_err(|error| RecordingError::Invalid(error.to_string()))?;
        require(
            !self.streams.is_empty() && self.streams.len() <= MAX_STREAMS,
            "Take needs 1–64 streams",
        )?;
        let mut ids = BTreeSet::new();
        for stream in &self.streams {
            identifier(&stream.stream_id)?;
            identifier(&stream.segment_id)?;
            identifier(&stream.source_id)?;
            require(ids.insert(&stream.stream_id), "Duplicate recording stream")?;
            require(
                stream.source_name.len() <= 1024 && stream.source_connection_generation > 0,
                "Invalid recording source",
            )?;
            require(
                stream.initial_source_position <= i64::MAX as u64,
                "Source position exceeds journal integer range",
            )?;
            stream
                .host_clock
                .domain
                .validate()
                .map_err(|error| RecordingError::Invalid(error.to_string()))?;
            stream
                .host_clock
                .epoch
                .validate()
                .map_err(|error| RecordingError::Invalid(error.to_string()))?;
            require(
                (1..=1_000_000_000_000).contains(&stream.host_clock.ticks_per_second_numerator)
                    && (1..=1_000_000_000)
                        .contains(&stream.host_clock.ticks_per_second_denominator),
                "Invalid recording clock timebase",
            )?;
            if let StreamKind::Audio {
                format,
                audio_epoch,
            } = &stream.kind
            {
                require(
                    (8_000..=768_000).contains(&format.sample_rate)
                        && (1..=16).contains(&format.channels),
                    "Unsupported recording PCM format",
                )?;
                audio_epoch
                    .validate()
                    .map_err(|error| RecordingError::Invalid(error.to_string()))?;
            }
        }
        bounded_json(self, MAX_METADATA_BYTES)?;
        Ok(())
    }
    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        Ok(digest(
            b"OmniuSong.take-descriptor.v1\0",
            &bounded_json(self, MAX_METADATA_BYTES)?,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceRange {
    AudioFrames { start: u64, count: u64 },
    Events { start: u64, count: u64 },
}
impl SourceRange {
    pub fn start(&self) -> u64 {
        match self {
            Self::AudioFrames { start, .. } | Self::Events { start, .. } => *start,
        }
    }
    pub fn count(&self) -> u64 {
        match self {
            Self::AudioFrames { count, .. } | Self::Events { count, .. } => *count,
        }
    }
    pub fn end(&self) -> Result<u64> {
        require(
            self.count() > 0,
            "Empty content or gap ranges are not meaningful",
        )?;
        self.start()
            .checked_add(self.count())
            .filter(|end| *end <= i64::MAX as u64)
            .ok_or_else(|| RecordingError::Invalid("Source range overflow".into()))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    TransferOverflow,
    DeviceLost,
    DeclaredLoss(String),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChunkKind {
    Content,
    Gap { reason: GapReason },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkHeader {
    pub version: u32,
    pub take_id: TakeId,
    pub stream_id: String,
    pub segment_id: String,
    pub sequence: u64,
    pub range: SourceRange,
    pub interpretation: StreamKind,
    pub observed_start: HostTimestamp,
    pub observed_end: HostTimestamp,
    pub clock_anchors: Vec<ClockAnchor>,
    pub kind: ChunkKind,
}
impl ChunkHeader {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        bounded_json(self, MAX_HEADER_BYTES)
    }
    pub fn sha256(&self, payload: &[u8]) -> Result<String> {
        let header = self.canonical_bytes()?;
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.recording-chunk.v1\0");
        hash.update((header.len() as u32).to_le_bytes());
        hash.update(header);
        hash.update(payload);
        Ok(format!("{:x}", hash.finalize()))
    }
    pub(crate) fn validate(&self, take: &TakeDescriptor, payload: &[u8]) -> Result<()> {
        require(
            self.version == CHUNK_VERSION,
            "Unsupported recording chunk version",
        )?;
        require(
            self.take_id == take.take_id && self.sequence < i64::MAX as u64,
            "Invalid chunk take or sequence",
        )?;
        let stream = take
            .streams
            .iter()
            .find(|stream| stream.stream_id == self.stream_id)
            .ok_or_else(|| RecordingError::Invalid("Unknown recording stream".into()))?;
        require(
            self.segment_id == stream.segment_id && self.interpretation == stream.kind,
            "Chunk interpretation/segment differs from declared stream",
        )?;
        self.range.end()?;
        require(
            matches!(
                (&self.range, &stream.kind),
                (SourceRange::AudioFrames { .. }, StreamKind::Audio { .. })
                    | (SourceRange::Events { .. }, StreamKind::RawEvents { .. })
            ),
            "Source range unit differs from stream",
        )?;
        for stamp in [&self.observed_start, &self.observed_end] {
            stamp
                .validate()
                .map_err(|error| RecordingError::Invalid(error.to_string()))?;
            require(
                stamp.domain == stream.host_clock.domain && stamp.epoch == stream.host_clock.epoch,
                "Chunk host clock epoch/domain mismatch",
            )?;
        }
        require(
            self.observed_start.ticks <= self.observed_end.ticks && self.clock_anchors.len() <= 16,
            "Invalid chunk timestamp bounds or anchor count",
        )?;
        let mut anchors = BTreeSet::new();
        for anchor in &self.clock_anchors {
            anchor
                .validate()
                .map_err(|error| RecordingError::Invalid(error.to_string()))?;
            require(
                anchors.insert(anchor.id.clone())
                    && anchor.host.domain == stream.host_clock.domain
                    && anchor.host.epoch == stream.host_clock.epoch
                    && anchor.ticks_per_second_numerator
                        == stream.host_clock.ticks_per_second_numerator
                    && anchor.ticks_per_second_denominator
                        == stream.host_clock.ticks_per_second_denominator,
                "Chunk clock anchor mismatch",
            )?;
            if let StreamKind::Audio {
                format,
                audio_epoch,
            } = &stream.kind
            {
                require(
                    anchor.audio.epoch == *audio_epoch
                        && anchor.audio.sample_rate == format.sample_rate,
                    "Audio anchor epoch/rate mismatch",
                )?;
            }
        }
        match &self.kind {
            ChunkKind::Gap { reason } => {
                require(
                    payload.is_empty(),
                    "A gap must not fabricate audio or raw event bytes",
                )?;
                if let GapReason::DeclaredLoss(detail) = reason {
                    require(
                        !detail.is_empty() && detail.len() <= 512,
                        "Invalid gap explanation",
                    )?;
                }
            }
            ChunkKind::Content => match &stream.kind {
                StreamKind::Audio { format, .. } => {
                    let expected = self
                        .range
                        .count()
                        .checked_mul(u64::from(format.channels))
                        .and_then(|samples| {
                            samples.checked_mul(format.encoding.bytes_per_sample())
                        });
                    require(
                        !payload.is_empty()
                            && payload.len() <= MAX_AUDIO_CHUNK_BYTES
                            && expected == Some(payload.len() as u64),
                        "Audio chunk size does not match its frame/channel/PCM interpretation",
                    )?;
                }
                StreamKind::RawEvents { .. } => {
                    validate_events(payload, self.range.count(), stream, self)?
                }
            },
        }
        self.canonical_bytes()?;
        Ok(())
    }
}

fn validate_events(
    payload: &[u8],
    expected: u64,
    stream: &StreamDescriptor,
    header: &ChunkHeader,
) -> Result<()> {
    require(
        !payload.is_empty() && payload.len() <= MAX_EVENT_CHUNK_BYTES,
        "Raw event chunk exceeds 64 KiB",
    )?;
    let mut cursor = 0;
    let mut count = 0_u64;
    while cursor < payload.len() {
        require(payload.len() - cursor >= 4, "Truncated raw event length")?;
        let len = u32::from_le_bytes(payload[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        require(
            len > 0 && len <= payload.len() - cursor,
            "Truncated raw event record",
        )?;
        let event = Envelope::<CapturedEvent>::decode(&payload[cursor..cursor + len])
            .map_err(|error| RecordingError::Invalid(error.to_string()))?;
        require(
            event.payload.source.endpoint.0 == stream.source_id
                && event.payload.source.connection_generation
                    == stream.source_connection_generation
                && event.payload.time.observed.domain == stream.host_clock.domain
                && event.payload.time.observed.epoch == stream.host_clock.epoch
                && (header.observed_start.ticks..=header.observed_end.ticks)
                    .contains(&event.payload.time.observed.ticks),
            "Raw event differs from declared source/clock/timestamp range",
        )?;
        cursor += len;
        count += 1;
    }
    require(
        count == expected,
        "Raw event count does not match its source range",
    )
}

/// Encode already validated capture envelopes, preserving the original JSON bytes.
pub fn encode_event_chunk(records: &[&[u8]]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for record in records {
        Envelope::<CapturedEvent>::decode(record)
            .map_err(|error| RecordingError::Invalid(error.to_string()))?;
        require(
            output
                .len()
                .checked_add(4 + record.len())
                .is_some_and(|len| len <= MAX_EVENT_CHUNK_BYTES),
            "Raw event chunk exceeds 64 KiB",
        )?;
        output.extend_from_slice(&(record.len() as u32).to_le_bytes());
        output.extend_from_slice(record);
    }
    require(!output.is_empty(), "Empty raw event chunk")?;
    Ok(output)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "unit",
    content = "end",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SourcePosition {
    AudioFrames(u64),
    Events(u64),
}
impl SourcePosition {
    pub fn value(&self) -> u64 {
        match self {
            Self::AudioFrames(end) | Self::Events(end) => *end,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamPrefix {
    pub stream_id: String,
    pub next_sequence: u64,
    pub source_end: SourcePosition,
    pub recorded_units: u64,
    pub gap_units: u64,
    pub gap_chunks: u64,
}
impl StreamPrefix {
    pub(crate) fn initial(stream: &StreamDescriptor) -> Self {
        Self {
            stream_id: stream.stream_id.clone(),
            next_sequence: 0,
            source_end: match stream.kind {
                StreamKind::Audio { .. } => {
                    SourcePosition::AudioFrames(stream.initial_source_position)
                }
                StreamKind::RawEvents { .. } => {
                    SourcePosition::Events(stream.initial_source_position)
                }
            },
            recorded_units: 0,
            gap_units: 0,
            gap_chunks: 0,
        }
    }
    pub(crate) fn advance(&mut self, header: &ChunkHeader) -> Result<()> {
        require(
            header.sequence == self.next_sequence
                && header.range.start() == self.source_end.value(),
            "Chunk sequence/source range is not the next contiguous range; declare a gap explicitly",
        )?;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| RecordingError::Invalid("Chunk sequence overflow".into()))?;
        let end = header.range.end()?;
        self.source_end = match header.range {
            SourceRange::AudioFrames { .. } => SourcePosition::AudioFrames(end),
            SourceRange::Events { .. } => SourcePosition::Events(end),
        };
        match header.kind {
            ChunkKind::Content => self.recorded_units += header.range.count(),
            ChunkKind::Gap { .. } => {
                self.gap_units += header.range.count();
                self.gap_chunks += 1;
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkReceipt {
    pub take_id: TakeId,
    pub stream_id: String,
    pub sequence: u64,
    pub sha256: String,
    pub durable_prefix: StreamPrefix,
    pub replayed: bool,
}
/// Borrowed originals for one bounded atomic append transaction.
#[derive(Debug, Clone, Copy)]
pub struct AppendChunk<'a> {
    pub header: &'a ChunkHeader,
    pub payload: &'a [u8],
}
/// The content hash identifies this ordered request, not historical proof that
/// these rows were first committed together. Replay may recognize rows that
/// were originally written by individual append calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppendGroupReceipt {
    pub group_sha256: String,
    pub first_ordinal: u64,
    pub last_ordinal: u64,
    pub chunks: Vec<ChunkReceipt>,
    pub replayed: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkRecord {
    pub header: ChunkHeader,
    pub payload: Vec<u8>,
    pub sha256: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakeState {
    Recording,
    Interrupted,
    Sealed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryProblem {
    pub stream_id: Option<String>,
    pub sequence: Option<u64>,
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReport {
    pub take_id: TakeId,
    pub state: TakeState,
    pub streams: Vec<StreamPrefix>,
    pub verified_chunks: u64,
    pub verified_payload_bytes: u64,
    pub chunk_index_sha256: String,
    pub problems: Vec<RecoveryProblem>,
    pub unknown_tail: bool,
}
/// Explicit low-level original prefix chosen by a caller's format-aware scan.
/// This binds bytes/order only; it does not validate engine checkpoint semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPrefix {
    pub chunk_count: u64,
    pub chunk_index_sha256: String,
}
impl RecoveryPrefix {
    pub(crate) fn validate(&self) -> Result<()> {
        require(
            self.chunk_count > 0
                && self.chunk_count <= i64::MAX as u64
                && self.chunk_index_sha256.len() == 64
                && self
                    .chunk_index_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "Selected recovery prefix must have a nonempty bounded count and canonical SHA-256",
        )
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterruptedReceipt {
    pub receipt_id: String,
    pub report: RecoveryReport,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBody {
    pub version: u32,
    pub manifest_id: String,
    pub operation_id: String,
    pub descriptor: TakeDescriptor,
    pub descriptor_sha256: String,
    pub streams: Vec<StreamPrefix>,
    pub chunk_count: u64,
    pub payload_bytes: u64,
    pub chunk_index_sha256: String,
    pub interrupted: Option<InterruptedReceipt>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedManifest {
    pub body: ManifestBody,
    pub sha256: String,
}
impl SealedManifest {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        bounded_json(self, MAX_METADATA_BYTES * 2)
    }
    pub fn verify_hash(&self) -> Result<()> {
        require(
            self.body.version == JOURNAL_VERSION,
            "Unsupported sealed manifest version",
        )?;
        self.body.descriptor.validate()?;
        require(
            self.body.descriptor_sha256 == self.body.descriptor.sha256()?
                && self.sha256
                    == digest(
                        b"OmniuSong.sealed-take.v1\0",
                        &bounded_json(&self.body, MAX_METADATA_BYTES * 2)?,
                    ),
            "Sealed manifest identity/hash mismatch",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBoundary {
    AfterChunkInsert,
    BeforeChunkCommit,
    AfterChunkCommit,
    AfterManifestInsert,
    BeforeSealCommit,
    AfterSealCommit,
}
