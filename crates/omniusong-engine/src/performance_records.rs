//! Off-callback performance evidence codec. No journal, files or device access.
//! Local GUI observations and render-frame evidence remain separate clocks.
use crate::live_types::{
    LIVE_BATCH_EVENTS, LIVE_MAX_CALLBACK_FRAMES, LIVE_VOICES, LiveBatch, LiveClock, LiveEvent,
    LiveFault, LiveScope, LiveSourceBinding, LiveSourceKind,
};
use crate::performance_tap::{
    BatchDisposition, TAP_CHANNELS, TAP_MAX_BATCHES, TAP_TRANSITIONS, TapBegin, TapClear,
    TapClearReason, TapInfo, TapTransition,
};
use omniusong_core::contracts::{
    CaptureTime, CapturedEvent, ClockDomainId, ClockEpochId, EndpointId, Envelope, EventId,
    HostTimestamp, MessageId, MusicalEvent, RawPacket, SessionId, TakeId, Validate,
};
use omniusong_recording::{
    HostClock, JOURNAL_VERSION, PcmEncoding, PcmFormat, PcmLayout, RawEventFormat, SourcePosition,
    StreamDescriptor, StreamKind, StreamPrefix, TakeDescriptor,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const PERFORMANCE_NAMESPACE: &str = "omniusong.performance.v1";
pub const STREAM_IDS: [&str; 6] = ["audio", "gui-1", "gui-2", "gui-3", "gui-4", "control"];
pub const RECORD_VERSION: u16 = 1;
const RENDER_DOMAIN: &str = "omniusong.render-frames";
const KINDS: [LiveSourceKind; 4] = [
    LiveSourceKind::ComputerKeys,
    LiveSourceKind::PointerKeys,
    LiveSourceKind::Pads,
    LiveSourceKind::AccessiblePulse,
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSource {
    pub binding: LiveSourceBinding,
    pub endpoint: EndpointId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceConfig {
    pub take_id: TakeId,
    pub info: TapInfo,
    pub gui_clock: LiveClock,
    pub output_epoch: ClockEpochId,
    pub sources: [PerformanceSource; 4],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    version: u16,
    config: PerformanceConfig,
}

fn require(valid: bool, message: &str) -> Result<(), String> {
    if valid { Ok(()) } else { Err(message.into()) }
}
fn digest(domain: &[u8], fields: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    for field in fields {
        hash.update((field.len() as u64).to_le_bytes());
        hash.update(field);
    }
    format!("{:x}", hash.finalize())
}
fn endpoint(config: &PerformanceConfig, role: &str) -> EndpointId {
    EndpointId(format!(
        "pe:{}",
        digest(
            b"OmniuSong.performance-endpoint.v1\0",
            &[config.take_id.0.as_bytes(), role.as_bytes()]
        )
    ))
}
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let rest = a % b;
        a = b;
        b = rest;
    }
    a
}
fn gui_clock(config: &PerformanceConfig) -> Result<HostClock, String> {
    require(config.gui_clock.is_valid(), "Invalid original GUI clock")?;
    let domain = ClockDomainId(config.gui_clock.domain.clone());
    let epoch = ClockEpochId(config.gui_clock.epoch.clone());
    domain.validate().map_err(|e| e.to_string())?;
    epoch.validate().map_err(|e| e.to_string())?;
    let divisor = gcd(
        config.gui_clock.ticks_per_second_numerator,
        config.gui_clock.ticks_per_second_denominator,
    );
    let numerator = config.gui_clock.ticks_per_second_numerator / divisor;
    let denominator = config.gui_clock.ticks_per_second_denominator / divisor;
    require(
        numerator <= 1_000_000_000_000 && denominator <= 1_000_000_000,
        "GUI clock ratio cannot be represented exactly by the recording clock",
    )?;
    Ok(HostClock {
        domain,
        epoch,
        ticks_per_second_numerator: numerator,
        ticks_per_second_denominator: u32::try_from(denominator)
            .map_err(|_| "GUI clock denominator overflow")?,
    })
}
fn render_clock(config: &PerformanceConfig) -> HostClock {
    HostClock {
        domain: ClockDomainId(RENDER_DOMAIN.into()),
        epoch: config.output_epoch.clone(),
        ticks_per_second_numerator: u64::from(config.info.sample_rate),
        ticks_per_second_denominator: 1,
    }
}
impl PerformanceConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.take_id.validate().map_err(|e| e.to_string())?;
        self.output_epoch.validate().map_err(|e| e.to_string())?;
        require(
            self.info.scope.is_valid()
                && (8_000..=768_000).contains(&self.info.sample_rate)
                && (1..=TAP_CHANNELS as u16).contains(&self.info.channels),
            "Invalid initial performance scope or output format",
        )?;
        gui_clock(self)?;
        require(
            self.gui_clock.domain != RENDER_DOMAIN || self.gui_clock.epoch != self.output_epoch.0,
            "GUI and render-frame clocks require distinct domain/epoch identities",
        )?;
        let mut endpoints =
            BTreeSet::from([endpoint(self, "audio").0, endpoint(self, "control").0]);
        require(endpoints.len() == 2, "Audio and control endpoints alias")?;
        for (index, source) in self.sources.iter().enumerate() {
            source.endpoint.validate().map_err(|e| e.to_string())?;
            require(
                source
                    .endpoint
                    .0
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)),
                "Endpoint is unsupported by recording stream identities",
            )?;
            require(
                source.binding.kind == KINDS[index]
                    && usize::from(source.binding.handle) == index + 1
                    && source.binding.connection_generation == 1,
                "Performance sources must bind handles 1–4 to the current immutable GUI profile",
            )?;
            require(
                endpoints.insert(source.endpoint.0.clone()),
                "Performance endpoints must not alias",
            )?;
        }
        Ok(())
    }
    pub fn from_descriptor(descriptor: &TakeDescriptor) -> Result<Self, String> {
        from_descriptor(descriptor)
    }
}

pub fn create_descriptor(config: &PerformanceConfig) -> Result<TakeDescriptor, String> {
    config.validate()?;
    let gui_clock = gui_clock(config)?;
    let render_clock = render_clock(config);
    let mut streams = Vec::with_capacity(6);
    streams.push(StreamDescriptor {
        stream_id: STREAM_IDS[0].into(),
        segment_id: "performance".into(),
        source_id: endpoint(config, "audio").0,
        source_name: "Procedural post-mix F32 bus before device conversion".into(),
        source_connection_generation: 1,
        host_clock: render_clock.clone(),
        kind: StreamKind::Audio {
            format: PcmFormat {
                sample_rate: config.info.sample_rate,
                channels: config.info.channels,
                encoding: PcmEncoding::F32Le,
                layout: PcmLayout::Interleaved,
            },
            audio_epoch: config.output_epoch.clone(),
        },
        initial_source_position: 0,
    });
    for (index, source) in config.sources.iter().enumerate() {
        streams.push(StreamDescriptor {
            stream_id: STREAM_IDS[index + 1].into(),
            segment_id: "performance".into(),
            source_id: source.endpoint.0.clone(),
            source_name: format!("Local performance {:?}", source.binding.kind),
            source_connection_generation: 1,
            host_clock: gui_clock.clone(),
            kind: StreamKind::RawEvents {
                format: RawEventFormat::LengthPrefixedCaptureV1,
            },
            initial_source_position: 0,
        });
    }
    streams.push(StreamDescriptor {
        stream_id: STREAM_IDS[5].into(),
        segment_id: "performance".into(),
        source_id: endpoint(config, "control").0,
        source_name: "Local renderer control evidence in output frames".into(),
        source_connection_generation: 1,
        host_clock: render_clock,
        kind: StreamKind::RawEvents {
            format: RawEventFormat::LengthPrefixedCaptureV1,
        },
        initial_source_position: 0,
    });
    let descriptor = TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: config.take_id.clone(),
        streams,
        provenance: BTreeMap::from([(
            PERFORMANCE_NAMESPACE.into(),
            serde_json::to_value(Provenance {
                version: RECORD_VERSION,
                config: config.clone(),
            })
            .map_err(|e| e.to_string())?,
        )]),
    };
    descriptor.validate().map_err(|e| e.to_string())?;
    Ok(descriptor)
}
pub fn from_descriptor(descriptor: &TakeDescriptor) -> Result<PerformanceConfig, String> {
    descriptor.validate().map_err(|e| e.to_string())?;
    let owned = descriptor
        .provenance
        .get(PERFORMANCE_NAMESPACE)
        .ok_or("Descriptor has no owned performance provenance")?;
    let provenance: Provenance = serde_json::from_value(owned.clone())
        .map_err(|e| format!("Invalid owned performance provenance: {e}"))?;
    require(
        provenance.version == RECORD_VERSION,
        "Unsupported performance provenance version",
    )?;
    require(
        create_descriptor(&provenance.config)? == *descriptor,
        "Performance descriptor differs from its exact declared configuration",
    )?;
    Ok(provenance.config)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceChunkIdentity {
    pub stream_id: String,
    pub sequence: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceCheckpoint {
    pub group_sequence: u64,
    pub previous_checkpoint_sha256: Option<String>,
    pub first_callback_index: u64,
    pub last_callback_index: u64,
    pub audio_end: u64,
    pub scope_after: LiveScope,
    pub chunks: Vec<PerformanceChunkIdentity>,
    pub prefixes: Vec<StreamPrefix>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerformanceRecord {
    Begin {
        callback_index: u64,
        begin: TapBegin,
    },
    Batch {
        callback_index: u64,
        packet_ordinal: u16,
        batch: Box<LiveBatch>,
        disposition: BatchDisposition,
    },
    Clear {
        callback_index: u64,
        clear: TapClear,
    },
    Transition {
        callback_index: u64,
        transition: TapTransition,
    },
    End {
        callback_index: u64,
        end_frame: u64,
        scope_after: LiveScope,
        applied_sequence: Option<u64>,
        active_voices: u16,
        release_tail_voices: u16,
        clear_count: u16,
        transition_count: u16,
        first_fault: Option<LiveFault>,
    },
    AppliedEvent {
        callback_index: u64,
        batch_ordinal: u16,
        event_index: u8,
        scope: LiveScope,
        batch_sequence: u64,
        applied_frame: u64,
        original: LiveEvent,
    },
    Checkpoint {
        checkpoint: PerformanceCheckpoint,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRecord {
    pub version: u16,
    pub record: PerformanceRecord,
}

fn sha_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn frame_valid(frame: u64) -> bool {
    frame <= i64::MAX as u64
}
fn fault_valid(fault: &LiveFault) -> bool {
    fault.scope.is_valid() && frame_valid(fault.frame) && fault.sequence != Some(0)
}
fn single_event_valid(
    scope: LiveScope,
    sequence: u64,
    original: LiveEvent,
    rate: u32,
) -> Result<(), String> {
    require(
        (1..=4).contains(&original.source) && original.connection_generation == 1,
        "Applied event has an undeclared source binding",
    )?;
    let mut batch = LiveBatch::empty(scope, sequence, None);
    batch.try_push(original).map_err(|e| e.to_string())?;
    batch.validate_shape(rate).map_err(|e| e.to_string())
}
impl LocalRecord {
    pub fn validate(&self) -> Result<(), String> {
        require(
            self.version == RECORD_VERSION,
            "Unsupported local performance record version",
        )?;
        match &self.record {
            PerformanceRecord::Begin {
                callback_index,
                begin,
            } => {
                require(
                    *callback_index > 0
                        && begin.scope.is_valid()
                        && begin.frames > 0
                        && usize::from(begin.frames) <= LIVE_MAX_CALLBACK_FRAMES
                        && (8_000..=768_000).contains(&begin.sample_rate)
                        && (1..=TAP_CHANNELS as u16).contains(&begin.channels)
                        && begin
                            .start_frame
                            .checked_add(u64::from(begin.frames))
                            .is_some_and(frame_valid),
                    "Invalid performance callback begin",
                )?;
            }
            PerformanceRecord::Batch {
                callback_index,
                packet_ordinal,
                batch,
                disposition,
            } => {
                require(
                    *callback_index > 0 && (1..=TAP_MAX_BATCHES as u16).contains(packet_ordinal),
                    "Invalid callback batch position",
                )?;
                // Discarded/rejected fixed packets can contain malformed source,
                // sequence or unused slots. Preserve them; never reinterpret them.
                if *disposition == BatchDisposition::Applied {
                    batch.validate_shape(768_000).map_err(|e| e.to_string())?;
                }
            }
            PerformanceRecord::Clear {
                callback_index,
                clear,
            } => {
                require(
                    *callback_index > 0 && clear.scope.is_valid() && frame_valid(clear.frame),
                    "Invalid performance clear",
                )?;
                if let TapClearReason::Panic { next_generation } = clear.reason {
                    require(
                        clear.scope.panic_generation.checked_add(1) == Some(next_generation),
                        "Invalid Panic generation transition",
                    )?;
                }
            }
            PerformanceRecord::Transition {
                callback_index,
                transition,
            } => {
                require(
                    *callback_index > 0
                        && transition.scope.is_valid()
                        && (1..=4).contains(&transition.source)
                        && transition.voice > 0
                        && frame_valid(transition.frame),
                    "Invalid automatic performance transition",
                )?;
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
                require(
                    *callback_index > 0
                        && frame_valid(*end_frame)
                        && scope_after.is_valid()
                        && *applied_sequence != Some(0)
                        && usize::from(*active_voices) <= LIVE_VOICES
                        && release_tail_voices <= active_voices
                        && *clear_count <= 2
                        && usize::from(*transition_count) <= TAP_TRANSITIONS
                        && first_fault
                            .as_ref()
                            .is_none_or(|fault| fault_valid(fault) && fault.frame <= *end_frame),
                    "Invalid performance callback end",
                )?;
            }
            PerformanceRecord::AppliedEvent {
                callback_index,
                batch_ordinal,
                event_index,
                scope,
                batch_sequence,
                applied_frame,
                original,
            } => {
                require(
                    *callback_index > 0
                        && (1..=TAP_MAX_BATCHES as u16).contains(batch_ordinal)
                        && usize::from(*event_index) < LIVE_BATCH_EVENTS
                        && frame_valid(*applied_frame),
                    "Invalid applied performance event position",
                )?;
                single_event_valid(*scope, *batch_sequence, *original, 768_000)?;
            }
            PerformanceRecord::Checkpoint { checkpoint } => validate_checkpoint(checkpoint)?,
        }
        Ok(())
    }
}
fn validate_checkpoint(checkpoint: &PerformanceCheckpoint) -> Result<(), String> {
    require(
        checkpoint.group_sequence > 0
            && checkpoint.first_callback_index > 0
            && checkpoint.last_callback_index >= checkpoint.first_callback_index
            && checkpoint.audio_end > 0
            && frame_valid(checkpoint.audio_end)
            && checkpoint.scope_after.is_valid(),
        "Invalid performance checkpoint extent",
    )?;
    require(
        match &checkpoint.previous_checkpoint_sha256 {
            None => checkpoint.group_sequence == 1,
            Some(sha) => checkpoint.group_sequence > 1 && sha_valid(sha),
        },
        "Invalid checkpoint chain identity",
    )?;
    require(
        checkpoint.chunks.len() <= 63 && checkpoint.prefixes.len() == STREAM_IDS.len(),
        "Performance checkpoint exceeds its fixed member bounds",
    )?;
    let mut identities = BTreeSet::new();
    for chunk in &checkpoint.chunks {
        require(
            STREAM_IDS.contains(&chunk.stream_id.as_str())
                && frame_valid(chunk.sequence)
                && sha_valid(&chunk.sha256)
                && identities.insert((&chunk.stream_id, chunk.sequence)),
            "Invalid or repeated performance chunk identity",
        )?;
    }
    for (index, prefix) in checkpoint.prefixes.iter().enumerate() {
        require(
            prefix.stream_id == STREAM_IDS[index]
                && frame_valid(prefix.next_sequence)
                && frame_valid(prefix.source_end.value())
                && prefix.recorded_units.checked_add(prefix.gap_units)
                    == Some(prefix.source_end.value())
                && prefix.gap_chunks <= prefix.next_sequence,
            "Invalid performance stream prefix",
        )?;
        require(
            matches!(
                (&prefix.source_end, index),
                (SourcePosition::AudioFrames(_), 0) | (SourcePosition::Events(_), 1..=5)
            ),
            "Performance prefix uses the wrong source units",
        )?;
    }
    require(
        checkpoint.prefixes[0].source_end == SourcePosition::AudioFrames(checkpoint.audio_end),
        "Checkpoint audio prefix differs from declared end",
    )?;
    Ok(())
}
fn hex_id(id: &[u8; 16]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns one bare capture envelope. The journal's existing length-prefixed
/// raw-chunk encoder supplies framing; this API does not add a second prefix.
pub fn encode_record(
    config: &PerformanceConfig,
    stream_index: usize,
    record: &LocalRecord,
    observed_ticks: u64,
    event_ordinal: u64,
) -> Result<Vec<u8>, String> {
    config.validate()?;
    record.validate()?;
    require(
        (1..STREAM_IDS.len()).contains(&stream_index) && event_ordinal < i64::MAX as u64,
        "Invalid performance event stream or source ordinal",
    )?;
    let (source, clock) = if let PerformanceRecord::AppliedEvent {
        original,
        scope,
        batch_sequence,
        ..
    } = &record.record
    {
        require(
            stream_index <= 4
                && usize::from(original.source) == stream_index
                && observed_ticks == original.observed_ticks,
            "Applied event does not match its original source/ticks",
        )?;
        single_event_valid(*scope, *batch_sequence, *original, config.info.sample_rate)?;
        (
            config.sources[stream_index - 1].endpoint.clone(),
            gui_clock(config)?,
        )
    } else {
        require(
            stream_index == 5 && frame_valid(observed_ticks),
            "Control evidence belongs to the render-frame stream",
        )?;
        let frame = match &record.record {
            PerformanceRecord::Begin { begin, .. } => Some(begin.start_frame),
            PerformanceRecord::Clear { clear, .. } => Some(clear.frame),
            PerformanceRecord::Transition { transition, .. } => Some(transition.frame),
            PerformanceRecord::End { end_frame, .. } => Some(*end_frame),
            PerformanceRecord::Checkpoint { checkpoint } => Some(checkpoint.audio_end),
            _ => None,
        };
        require(
            frame.is_none_or(|frame| frame == observed_ticks),
            "Control observation differs from its exact render frame",
        )?;
        if let PerformanceRecord::Begin { begin, .. } = &record.record {
            require(
                begin.sample_rate == config.info.sample_rate
                    && begin.channels == config.info.channels,
                "Begin format differs from performance descriptor",
            )?;
        }
        if let PerformanceRecord::Batch {
            batch,
            disposition: BatchDisposition::Applied,
            ..
        } = &record.record
        {
            batch
                .validate_shape(config.info.sample_rate)
                .map_err(|e| e.to_string())?;
        }
        (endpoint(config, "control"), render_clock(config))
    };
    let id = digest(
        b"OmniuSong.performance-event.v1\0",
        &[
            config.take_id.0.as_bytes(),
            STREAM_IDS[stream_index].as_bytes(),
            &event_ordinal.to_le_bytes(),
        ],
    );
    let event = CapturedEvent {
        event_id: EventId(format!("pe:{id}")),
        source: omniusong_core::contracts::SourceAddress {
            endpoint: source,
            connection_generation: 1,
            // Required structural placeholders for LOCAL evidence, not a MIDI
            // group/channel observation. No musical MIDI event is invented.
            group: 0,
            channel: None,
            extensions: BTreeMap::new(),
        },
        time: CaptureTime {
            observed: HostTimestamp {
                domain: clock.domain,
                epoch: clock.epoch,
                ticks: observed_ticks,
            },
            alignment: None,
            extensions: BTreeMap::new(),
        },
        raw: RawPacket::Local,
        musical: MusicalEvent::Uninterpreted,
        extensions: BTreeMap::from([(
            PERFORMANCE_NAMESPACE.into(),
            serde_json::to_value(record).map_err(|e| e.to_string())?,
        )]),
    };
    Envelope::new(
        SessionId(hex_id(&config.info.scope.session_id.0)),
        MessageId(format!("pm:{id}")),
        event_ordinal,
        event,
    )
    .encode()
    .map_err(|e| e.to_string())
}
pub fn decode_envelope(bytes: &[u8]) -> Result<(Envelope<CapturedEvent>, LocalRecord), String> {
    let envelope = Envelope::<CapturedEvent>::decode(bytes).map_err(|e| e.to_string())?;
    require(
        matches!(envelope.payload.raw, RawPacket::Local)
            && matches!(envelope.payload.musical, MusicalEvent::Uninterpreted)
            && envelope.payload.source.group == 0
            && envelope.payload.source.channel.is_none()
            && envelope.payload.time.alignment.is_none(),
        "Performance records require local uninterpreted unaligned evidence",
    )?;
    let owned = envelope
        .payload
        .extensions
        .get(PERFORMANCE_NAMESPACE)
        .ok_or("Capture envelope has no owned performance record")?;
    let record: LocalRecord = serde_json::from_value(owned.clone())
        .map_err(|e| format!("Invalid owned local performance record: {e}"))?;
    record.validate()?;
    Ok((envelope, record))
}
pub fn decode_record(bytes: &[u8]) -> Result<LocalRecord, String> {
    decode_envelope(bytes).map(|(_, record)| record)
}
pub fn checkpoint_sha256(encoded_envelope_bytes: &[u8]) -> Result<String, String> {
    let record = decode_record(encoded_envelope_bytes)?;
    require(
        matches!(record.record, PerformanceRecord::Checkpoint { .. }),
        "Checkpoint digest requires a validated checkpoint envelope",
    )?;
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.performance-checkpoint.v1\0");
    hash.update(encoded_envelope_bytes);
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_types::{LiveEventKind, LiveFaultKind, LiveId, LivePatch};
    use crate::performance_tap::TapTransitionKind;
    use omniusong_core::contracts::CAPTURE_MAX_BYTES;
    use serde_json::json;

    fn config() -> PerformanceConfig {
        PerformanceConfig {
            take_id: TakeId("synthetic-performance-take".into()),
            info: TapInfo {
                scope: LiveScope {
                    worker_generation: LiveId([1; 16]),
                    session_id: LiveId([2; 16]),
                    panic_generation: 1,
                },
                sample_rate: 48_000,
                channels: 2,
            },
            gui_clock: LiveClock {
                domain: "gui-clock".into(),
                epoch: "gui-epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            output_epoch: ClockEpochId("output-epoch".into()),
            sources: std::array::from_fn(|index| PerformanceSource {
                binding: LiveSourceBinding {
                    kind: KINDS[index],
                    handle: (index + 1) as u8,
                    connection_generation: 1,
                },
                endpoint: EndpointId(format!("local-source-{}", index + 1)),
            }),
        }
    }
    fn local(record: PerformanceRecord) -> LocalRecord {
        LocalRecord {
            version: RECORD_VERSION,
            record,
        }
    }
    fn applied(config: &PerformanceConfig, kind: LiveEventKind) -> LocalRecord {
        local(PerformanceRecord::AppliedEvent {
            callback_index: 1,
            batch_ordinal: 1,
            event_index: 0,
            scope: config.info.scope,
            batch_sequence: 1,
            applied_frame: 17,
            original: LiveEvent {
                source: 2,
                connection_generation: 1,
                observed_ticks: 9_007_199_254_740_993,
                event: kind,
            },
        })
    }
    fn begin(config: &PerformanceConfig) -> LocalRecord {
        local(PerformanceRecord::Begin {
            callback_index: 1,
            begin: TapBegin {
                scope: config.info.scope,
                start_frame: 0,
                frames: 512,
                sample_rate: config.info.sample_rate,
                channels: config.info.channels,
            },
        })
    }
    fn checkpoint(config: &PerformanceConfig) -> PerformanceCheckpoint {
        PerformanceCheckpoint {
            group_sequence: 1,
            previous_checkpoint_sha256: None,
            first_callback_index: 1,
            last_callback_index: 2,
            audio_end: 1024,
            scope_after: config.info.scope,
            chunks: vec![PerformanceChunkIdentity {
                stream_id: "audio".into(),
                sequence: 0,
                sha256: "a".repeat(64),
            }],
            prefixes: STREAM_IDS
                .iter()
                .enumerate()
                .map(|(index, id)| StreamPrefix {
                    stream_id: (*id).into(),
                    next_sequence: if index == 0 || index == 5 { 1 } else { 0 },
                    source_end: if index == 0 {
                        SourcePosition::AudioFrames(1024)
                    } else {
                        SourcePosition::Events(if index == 5 { 1 } else { 0 })
                    },
                    recorded_units: if index == 0 {
                        1024
                    } else if index == 5 {
                        1
                    } else {
                        0
                    },
                    gap_units: 0,
                    gap_chunks: 0,
                })
                .collect(),
        }
    }
    fn roundtrip_control(config: &PerformanceConfig, record: LocalRecord, ticks: u64) -> Vec<u8> {
        let bytes = encode_record(config, 5, &record, ticks, 0).unwrap();
        assert!(bytes.len() <= CAPTURE_MAX_BYTES);
        assert_eq!(decode_record(&bytes).unwrap(), record);
        bytes
    }

    #[test]
    fn descriptor_has_exact_six_streams_and_roundtrips_the_full_configuration() {
        let config = config();
        let descriptor = create_descriptor(&config).unwrap();
        assert_eq!(
            descriptor
                .streams
                .iter()
                .map(|stream| stream.stream_id.as_str())
                .collect::<Vec<_>>(),
            STREAM_IDS
        );
        assert!(
            descriptor
                .streams
                .iter()
                .all(|stream| stream.initial_source_position == 0)
        );
        assert_eq!(
            PerformanceConfig::from_descriptor(&descriptor).unwrap(),
            config
        );
        assert_eq!(descriptor.streams[0].host_clock, render_clock(&config));
        assert_eq!(descriptor.streams[5].host_clock, render_clock(&config));
        assert!(
            matches!(&descriptor.streams[0].kind,StreamKind::Audio { format, audio_epoch } if format.encoding == PcmEncoding::F32Le && format.layout == PcmLayout::Interleaved && audio_epoch == &config.output_epoch)
        );
        for (index, stream) in descriptor.streams[1..5].iter().enumerate() {
            assert_eq!(stream.source_id, config.sources[index].endpoint.0);
            assert_eq!(stream.host_clock, gui_clock(&config).unwrap());
        }
    }

    #[test]
    fn gcd_reduction_is_exact_and_never_rescales_original_observed_ticks() {
        let mut config = config();
        config.gui_clock.ticks_per_second_numerator = 1000;
        config.gui_clock.ticks_per_second_denominator = 1_000_000_000_000;
        let descriptor = create_descriptor(&config).unwrap();
        assert_eq!(
            descriptor.streams[1].host_clock.ticks_per_second_numerator,
            1
        );
        assert_eq!(
            descriptor.streams[1]
                .host_clock
                .ticks_per_second_denominator,
            1_000_000_000
        );
        assert_eq!(
            from_descriptor(&descriptor).unwrap().gui_clock,
            config.gui_clock
        );
        let mut record = applied(&config, LiveEventKind::Sustain { down: true });
        let PerformanceRecord::AppliedEvent { original, .. } = &mut record.record else {
            panic!()
        };
        original.observed_ticks = u64::MAX;
        let bytes = encode_record(&config, 2, &record, u64::MAX, 0).unwrap();
        let (envelope, decoded) = decode_envelope(&bytes).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(envelope.payload.time.observed.ticks, u64::MAX);
        assert_eq!(envelope.payload.time.observed.domain.0, "gui-clock");
        assert!(envelope.payload.time.alignment.is_none());
        config.gui_clock.ticks_per_second_numerator = 1;
        config.gui_clock.ticks_per_second_denominator = 1_000_000_001;
        assert!(
            create_descriptor(&config).is_err(),
            "Reject an unrepresentable ratio before any journal can be created"
        );
    }

    #[test]
    fn gui_clock_cannot_redeclare_the_render_frame_epoch() {
        for numerator in [48_000, 1_000_000_000] {
            let mut config = config();
            config.gui_clock.domain = RENDER_DOMAIN.into();
            config.gui_clock.epoch = config.output_epoch.0.clone();
            config.gui_clock.ticks_per_second_numerator = numerator;
            config.gui_clock.ticks_per_second_denominator = 1;
            assert!(create_descriptor(&config).is_err());
        }
    }
    #[test]
    fn invalid_ids_aliases_formats_and_non_profile_bindings_reject() {
        for invalid in 0..10 {
            let mut config = config();
            match invalid {
                0 => config.take_id.0 = "bad take".into(),
                1 => config.output_epoch.0.clear(),
                2 => config.gui_clock.domain = "bad clock".into(),
                3 => config.sources[1].endpoint = config.sources[0].endpoint.clone(),
                4 => config.sources[0].endpoint = endpoint(&config, "control"),
                5 => config.sources[0].binding.handle = 4,
                6 => config.sources[0].binding.kind = LiveSourceKind::Pads,
                7 => config.sources[0].binding.connection_generation = 2,
                8 => config.info.sample_rate = 7999,
                _ => config.info.channels = 9,
            }
            assert!(
                create_descriptor(&config).is_err(),
                "Invalid profile case {invalid}"
            );
        }
    }

    #[test]
    fn maximum_take_id_keeps_all_generated_identities_valid_and_deterministic() {
        let mut config = config();
        config.take_id.0 = "a".repeat(96);
        let descriptor = create_descriptor(&config).unwrap();
        for stream in &descriptor.streams {
            EndpointId(stream.source_id.clone()).validate().unwrap();
            assert!(stream.source_id.len() <= 96);
        }
        let record = begin(&config);
        let first = encode_record(&config, 5, &record, 0, 0).unwrap();
        assert_eq!(first, encode_record(&config, 5, &record, 0, 0).unwrap());
        let (envelope, _) = decode_envelope(&first).unwrap();
        assert!(envelope.message_id.0.len() <= 96);
        assert!(envelope.payload.event_id.0.len() <= 96);
        assert_ne!(
            envelope.payload.event_id,
            decode_envelope(&encode_record(&config, 5, &record, 0, 1).unwrap())
                .unwrap()
                .0
                .payload
                .event_id
        );
        config.take_id.0 = "b".repeat(96);
        assert_ne!(
            envelope.payload.event_id,
            decode_envelope(&encode_record(&config, 5, &record, 0, 0).unwrap())
                .unwrap()
                .0
                .payload
                .event_id
        );
    }

    #[test]
    fn descriptor_owned_provenance_and_interpretation_must_match_exactly() {
        for changed in 0..6 {
            let config = config();
            let mut descriptor = create_descriptor(&config).unwrap();
            match changed {
                0 => {
                    descriptor.provenance.remove(PERFORMANCE_NAMESPACE);
                }
                1 => {
                    descriptor
                        .provenance
                        .get_mut(PERFORMANCE_NAMESPACE)
                        .unwrap()["version"] = json!(2)
                }
                2 => {
                    descriptor
                        .provenance
                        .get_mut(PERFORMANCE_NAMESPACE)
                        .unwrap()["future"] = json!(true)
                }
                3 => descriptor.streams[0].source_name.push_str(" changed"),
                4 => descriptor.streams[1].host_clock.ticks_per_second_numerator += 1,
                _ => {
                    descriptor
                        .provenance
                        .insert("unowned-extra".into(), json!(1));
                }
            }
            assert!(
                from_descriptor(&descriptor).is_err(),
                "Changed descriptor case {changed}"
            );
        }
    }

    #[test]
    fn local_event_roundtrip_preserves_every_full_precision_value_without_midi_invention() {
        let config = config();
        for kind in [
            LiveEventKind::NoteOn {
                note: 1,
                voice: 1,
                key: 127,
                velocity16: 0,
                patch: LivePatch::Pad,
            },
            LiveEventKind::NoteOn {
                note: 2,
                voice: 2,
                key: 0,
                velocity16: u16::MAX,
                patch: LivePatch::Keys,
            },
            LiveEventKind::NoteOff {
                voice: 2,
                release_velocity16: u16::MAX,
            },
            LiveEventKind::Sustain { down: false },
            LiveEventKind::Pulse {
                note: 3,
                voice: 3,
                key: 36,
                velocity16: 32769,
                release_velocity16: 12345,
                patch: LivePatch::Drums,
                duration_frames: 24000,
            },
        ] {
            let record = applied(&config, kind);
            let bytes = encode_record(&config, 2, &record, 9_007_199_254_740_993, 7).unwrap();
            let (envelope, decoded) = decode_envelope(&bytes).unwrap();
            assert_eq!(decoded, record);
            assert!(matches!(envelope.payload.raw, RawPacket::Local));
            assert!(matches!(
                envelope.payload.musical,
                MusicalEvent::Uninterpreted
            ));
            assert_eq!(envelope.payload.source.endpoint, config.sources[1].endpoint);
            assert_eq!(envelope.payload.source.group, 0);
            assert_eq!(envelope.payload.source.channel, None);
            assert_eq!(envelope.sequence, 7);
        }
    }

    #[test]
    fn stream_and_observation_mismatches_reject_but_discarded_originals_stay_exact() {
        let config = config();
        let record = applied(&config, LiveEventKind::Sustain { down: true });
        assert!(encode_record(&config, 1, &record, 9_007_199_254_740_993, 0).is_err());
        assert!(encode_record(&config, 2, &record, 1, 0).is_err());
        assert!(encode_record(&config, 5, &record, 17, 0).is_err());
        assert!(encode_record(&config, 0, &begin(&config), 0, 0).is_err());
        assert!(encode_record(&config, 5, &begin(&config), 1, 0).is_err());
        let mut malformed = LiveBatch::empty(config.info.scope, 0, None);
        malformed.len = u8::MAX;
        malformed.scope.panic_generation = 0;
        malformed.events[31] = Some(LiveEvent {
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
            BatchDisposition::DiscardedPanic,
            BatchDisposition::DiscardedStale,
            BatchDisposition::Rejected {
                fault: LiveFaultKind::InvalidBatch,
            },
        ] {
            roundtrip_control(
                &config,
                local(PerformanceRecord::Batch {
                    callback_index: 1,
                    packet_ordinal: 1,
                    batch: Box::new(malformed),
                    disposition,
                }),
                0,
            );
        }
        assert!(
            encode_record(
                &config,
                5,
                &local(PerformanceRecord::Batch {
                    callback_index: 1,
                    packet_ordinal: 1,
                    batch: Box::new(malformed),
                    disposition: BatchDisposition::Applied
                }),
                0,
                0
            )
            .is_err()
        );
    }

    #[test]
    fn maximum_applied_batch_and_split_control_types_fit_existing_envelope_limits() {
        let config = config();
        let mut batch = LiveBatch::empty(config.info.scope, u64::MAX, None);
        for index in 0..LIVE_BATCH_EVENTS {
            batch
                .try_push(LiveEvent {
                    source: 1,
                    connection_generation: 1,
                    observed_ticks: u64::MAX,
                    event: LiveEventKind::Pulse {
                        note: u64::MAX - index as u64,
                        voice: u64::MAX - index as u64,
                        key: 127,
                        velocity16: u16::MAX,
                        release_velocity16: u16::MAX,
                        patch: LivePatch::Drums,
                        duration_frames: 24000,
                    },
                })
                .unwrap();
        }
        roundtrip_control(&config, begin(&config), 0);
        roundtrip_control(
            &config,
            local(PerformanceRecord::Batch {
                callback_index: 1,
                packet_ordinal: 4,
                batch: Box::new(batch),
                disposition: BatchDisposition::Applied,
            }),
            0,
        );
        for reason in [
            TapClearReason::Panic { next_generation: 2 },
            TapClearReason::Fault {
                kind: LiveFaultKind::Device,
            },
            TapClearReason::FaultPending,
        ] {
            roundtrip_control(
                &config,
                local(PerformanceRecord::Clear {
                    callback_index: 1,
                    clear: TapClear {
                        scope: config.info.scope,
                        frame: 512,
                        reason,
                    },
                }),
                512,
            );
        }
        for kind in [
            TapTransitionKind::AutoRelease {
                release_velocity16: u16::MAX,
            },
            TapTransitionKind::VoiceEnded,
        ] {
            roundtrip_control(
                &config,
                local(PerformanceRecord::Transition {
                    callback_index: 1,
                    transition: TapTransition {
                        scope: config.info.scope,
                        source: 4,
                        voice: u64::MAX,
                        frame: 512,
                        kind,
                    },
                }),
                512,
            );
        }
        let bytes = roundtrip_control(
            &config,
            local(PerformanceRecord::End {
                callback_index: 1,
                end_frame: 512,
                scope_after: config.info.scope,
                applied_sequence: Some(u64::MAX),
                active_voices: 64,
                release_tail_voices: 64,
                clear_count: 2,
                transition_count: 128,
                first_fault: Some(LiveFault {
                    scope: config.info.scope,
                    kind: LiveFaultKind::Device,
                    frame: 512,
                    sequence: Some(u64::MAX),
                }),
            }),
            512,
        );
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let end = &value["payload"][PERFORMANCE_NAMESPACE]["record"];
        assert!(
            end.get("clears").is_none() && end.get("transitions").is_none(),
            "End must not serialize the fixed128-slot callback arrays"
        );
    }

    #[test]
    fn maximum_checkpoint_fits_capture_cap_and_hashes_exact_validated_envelope_bytes() {
        let config = config();
        let mut checkpoint = checkpoint(&config);
        checkpoint.group_sequence = u64::MAX;
        checkpoint.previous_checkpoint_sha256 = Some("f".repeat(64));
        checkpoint.first_callback_index = u64::MAX - 2;
        checkpoint.last_callback_index = u64::MAX - 1;
        checkpoint.audio_end = i64::MAX as u64;
        checkpoint.chunks = (0..63)
            .map(|index| PerformanceChunkIdentity {
                stream_id: STREAM_IDS[index % 6].into(),
                sequence: i64::MAX as u64 - 63 + index as u64,
                sha256: "f".repeat(64),
            })
            .collect();
        for (index, prefix) in checkpoint.prefixes.iter_mut().enumerate() {
            prefix.next_sequence = i64::MAX as u64;
            prefix.source_end = if index == 0 {
                SourcePosition::AudioFrames(i64::MAX as u64)
            } else {
                SourcePosition::Events(i64::MAX as u64)
            };
            prefix.recorded_units = i64::MAX as u64;
        }
        let bytes = roundtrip_control(
            &config,
            local(PerformanceRecord::Checkpoint { checkpoint }),
            i64::MAX as u64,
        );
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.performance-checkpoint.v1\0");
        hash.update(&bytes);
        assert_eq!(
            checkpoint_sha256(&bytes).unwrap(),
            format!("{:x}", hash.finalize())
        );
        let mut spaced = vec![b' '];
        spaced.extend_from_slice(&bytes);
        assert_ne!(
            checkpoint_sha256(&bytes).unwrap(),
            checkpoint_sha256(&spaced).unwrap(),
            "Exact accepted envelope bytes, not a reserialization, bind the checkpoint"
        );
        assert!(
            checkpoint_sha256(&encode_record(&config, 5, &begin(&config), 0, 0).unwrap()).is_err()
        );
    }

    #[test]
    fn checkpoint_member_chain_units_and_prefix_bounds_reject() {
        let config = config();
        for invalid in 0..7 {
            let mut checkpoint = checkpoint(&config);
            match invalid {
                0 => checkpoint.chunks.resize(64, checkpoint.chunks[0].clone()),
                1 => checkpoint.previous_checkpoint_sha256 = Some("a".repeat(64)),
                2 => {
                    checkpoint.prefixes.pop();
                }
                3 => checkpoint.prefixes[0].source_end = SourcePosition::Events(1024),
                4 => checkpoint.prefixes[0].recorded_units = 1000,
                5 => checkpoint.chunks[0].sha256 = "A".repeat(64),
                _ => checkpoint.prefixes.swap(1, 2),
            }
            assert!(
                encode_record(
                    &config,
                    5,
                    &local(PerformanceRecord::Checkpoint { checkpoint }),
                    1024,
                    0
                )
                .is_err(),
                "Invalid checkpoint {invalid}"
            );
        }
    }

    #[test]
    fn unknown_versions_missing_namespace_and_extra_owned_fields_are_not_interpreted() {
        let config = config();
        let bytes = encode_record(&config, 5, &begin(&config), 0, 0).unwrap();
        let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for invalid in 0..6 {
            let mut value = original.clone();
            match invalid {
                0 => value["payload"][PERFORMANCE_NAMESPACE]["version"] = json!(2),
                1 => value["payload"][PERFORMANCE_NAMESPACE]["future"] = json!(true),
                2 => value["payload"][PERFORMANCE_NAMESPACE]["record"]["future"] = json!(true),
                3 => {
                    value["payload"]
                        .as_object_mut()
                        .unwrap()
                        .remove(PERFORMANCE_NAMESPACE);
                }
                4 => value["payload"][PERFORMANCE_NAMESPACE]["record"]["kind"] = json!("future"),
                _ => value["payload"]["source"]["channel"] = json!(0),
            }
            assert!(
                decode_record(&serde_json::to_vec(&value).unwrap()).is_err(),
                "Invalid owned record {invalid}"
            );
        }
        let (mut envelope, _) = decode_envelope(&bytes).unwrap();
        envelope.payload.raw = RawPacket::Midi1 { bytes: vec![0xf8] };
        assert!(decode_record(&envelope.encode().unwrap()).is_err());
        assert!(decode_record(&vec![b' '; CAPTURE_MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn nested_unit_and_struct_variants_refuse_extra_owned_fields() {
        for name in ["Applied", "DiscardedStale", "DiscardedPanic"] {
            assert!(
                serde_json::from_value::<BatchDisposition>(json!({(name):{"extra":1}})).is_err()
            );
        }
        assert!(
            serde_json::from_value::<TapClearReason>(json!({"FaultPending":{"extra":1}})).is_err()
        );
        assert!(
            serde_json::from_value::<TapTransitionKind>(json!({"VoiceEnded":{"extra":1}})).is_err()
        );
        assert!(
            serde_json::from_value::<BatchDisposition>(
                json!({"Rejected":{"fault":"device","extra":1}})
            )
            .is_err()
        );
        for value in [
            json!({"Panic":{"next_generation":2,"extra":1}}),
            json!({"Fault":{"kind":"device","extra":1}}),
        ] {
            assert!(serde_json::from_value::<TapClearReason>(value).is_err());
        }
        assert!(
            serde_json::from_value::<TapTransitionKind>(
                json!({"AutoRelease":{"release_velocity16":65535,"extra":1}})
            )
            .is_err()
        );
        for kind in [
            LiveEventKind::NoteOn {
                note: 1,
                voice: 1,
                key: 60,
                velocity16: 65535,
                patch: LivePatch::Keys,
            },
            LiveEventKind::NoteOff {
                voice: 1,
                release_velocity16: 65535,
            },
            LiveEventKind::Sustain { down: true },
            LiveEventKind::Pulse {
                note: 2,
                voice: 2,
                key: 36,
                velocity16: 1,
                release_velocity16: 2,
                patch: LivePatch::Drums,
                duration_frames: 4800,
            },
        ] {
            let mut value = serde_json::to_value(kind).unwrap();
            value["extra"] = json!(1);
            assert!(serde_json::from_value::<LiveEventKind>(value).is_err());
        }
    }
}
