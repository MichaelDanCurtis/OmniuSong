//! Bounded synthetic evidence for atomic multi-stream appends; no devices.
use omniusong_core::contracts::{
    CaptureTime, CapturedEvent, EndpointId, Envelope, EventId, Extensions, MessageId, MusicalEvent,
    RawPacket, SessionId, SourceAddress,
};
use omniusong_recording::*;
use rusqlite::Connection;
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/append-group-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn rows(&self) -> u64 {
        let c =
            Connection::open_with_flags(self.path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let count: i64 = c
            .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
            .unwrap();
        u64::try_from(count).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn descriptor() -> TakeDescriptor {
    let clock = HostClock {
        domain: ClockDomainId("synthetic-clock".into()),
        epoch: ClockEpochId("synthetic-epoch".into()),
        ticks_per_second_numerator: 48_000,
        ticks_per_second_denominator: 1,
    };
    let audio = StreamDescriptor {
        stream_id: "audio".into(),
        segment_id: "audio-segment".into(),
        source_id: "rendered-bus".into(),
        source_name: "Synthetic software bus; no device".into(),
        source_connection_generation: 1,
        host_clock: clock.clone(),
        kind: StreamKind::Audio {
            format: PcmFormat {
                sample_rate: 48_000,
                channels: 1,
                encoding: PcmEncoding::F32Le,
                layout: PcmLayout::Interleaved,
            },
            audio_epoch: ClockEpochId("render-epoch".into()),
        },
        initial_source_position: 100,
    };
    let events = StreamDescriptor {
        stream_id: "local".into(),
        segment_id: "local-segment".into(),
        source_id: "local-keys".into(),
        source_name: "Synthetic local packet fixture".into(),
        source_connection_generation: 3,
        host_clock: clock,
        kind: StreamKind::RawEvents {
            format: RawEventFormat::LengthPrefixedCaptureV1,
        },
        initial_source_position: 50,
    };
    TakeDescriptor {version:JOURNAL_VERSION,take_id:TakeId(uuid::Uuid::new_v4().to_string()),streams:vec![audio,events],provenance:BTreeMap::from([("fixture".into(),serde_json::from_str(r#"{"synthetic":true,"human":false,"deviceOpened":false,"integer":123456789012345678901234567890,"decimal":0.12345678901234567890123456789}"#).unwrap())])}
}
fn header(
    t: &TakeDescriptor,
    stream_index: usize,
    sequence: u64,
    start: u64,
    count: u64,
) -> ChunkHeader {
    let stream = &t.streams[stream_index];
    let stamp = HostTimestamp {
        domain: stream.host_clock.domain.clone(),
        epoch: stream.host_clock.epoch.clone(),
        ticks: 10 + sequence,
    };
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: t.take_id.clone(),
        stream_id: stream.stream_id.clone(),
        segment_id: stream.segment_id.clone(),
        sequence,
        range: if stream_index == 0 {
            SourceRange::AudioFrames { start, count }
        } else {
            SourceRange::Events { start, count }
        },
        interpretation: stream.kind.clone(),
        observed_start: stamp.clone(),
        observed_end: stamp,
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
fn event(t: &TakeDescriptor, sequence: u64) -> Vec<u8> {
    let stream = &t.streams[1];
    let event = CapturedEvent {
        event_id: EventId(format!("local-event-{sequence}")),
        source: SourceAddress {
            endpoint: EndpointId(stream.source_id.clone()),
            connection_generation: 3,
            group: 0,
            channel: None,
            extensions: Extensions::new(),
        },
        time: CaptureTime {
            observed: HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: 10 + sequence,
            },
            alignment: None,
            extensions: Extensions::new(),
        },
        raw: RawPacket::Local,
        musical: MusicalEvent::Uninterpreted,
        extensions: BTreeMap::from([(
            "local-fixture".into(),
            serde_json::json!({"velocity16":65535,"releaseVelocity16":0,"observed":18446744073709551615u64}),
        )]),
    };
    let bytes = Envelope::new(
        SessionId("synthetic-session".into()),
        MessageId(format!("message-{sequence}")),
        sequence,
        event,
    )
    .encode()
    .unwrap();
    encode_event_chunk(&[&bytes]).unwrap()
}
struct Item {
    header: ChunkHeader,
    payload: Vec<u8>,
}
fn items(t: &TakeDescriptor) -> Vec<Item> {
    vec![
        Item {
            header: header(t, 0, 0, 100, 2),
            payload: [0.25f32.to_le_bytes(), (-0.25f32).to_le_bytes()].concat(),
        },
        Item {
            header: header(t, 1, 0, 50, 1),
            payload: event(t, 0),
        },
        Item {
            header: header(t, 0, 1, 102, 2),
            payload: [0.5f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat(),
        },
        Item {
            header: header(t, 1, 1, 51, 1),
            payload: event(t, 1),
        },
    ]
}
fn refs(items: &[Item]) -> Vec<AppendChunk<'_>> {
    items
        .iter()
        .map(|item| AppendChunk {
            header: &item.header,
            payload: &item.payload,
        })
        .collect()
}

#[test]
fn atomic_multistream_group_has_ordered_originals_final_prefixes_and_stable_content_hash() {
    let f = Fixture::new();
    let t = descriptor();
    let original = t.clone();
    let mut j = Journal::create(f.path(), &t).unwrap();
    let chunks = items(&t);
    let receipt = j.append_group(&refs(&chunks)).unwrap();
    assert!(!receipt.replayed);
    assert_eq!((receipt.first_ordinal, receipt.last_ordinal), (1, 4));
    assert_eq!(receipt.chunks.len(), 4);
    for (entry, item) in receipt.chunks.iter().zip(&chunks) {
        assert_eq!(entry.sha256, item.header.sha256(&item.payload).unwrap());
        assert_eq!(entry.durable_prefix.next_sequence, 2);
        assert!(!entry.replayed);
        assert_eq!(
            j.read_chunk(&item.header.stream_id, item.header.sequence)
                .unwrap()
                .payload,
            item.payload
        );
    }
    assert_eq!(
        receipt.chunks[0].durable_prefix.source_end,
        SourcePosition::AudioFrames(104)
    );
    assert_eq!(
        receipt.chunks[1].durable_prefix.source_end,
        SourcePosition::Events(52)
    );
    assert_eq!(j.descriptor(), &original);
    let replay = j.append_group(&refs(&chunks)).unwrap();
    assert!(replay.replayed);
    assert!(replay.chunks.iter().all(|r| r.replayed));
    assert_eq!(receipt.group_sha256, replay.group_sha256);
    assert_eq!(f.rows(), 4);
    let mut hash = sha2::Sha256::new();
    use sha2::Digest;
    hash.update(b"OmniuSong.recording-append-group.v1\0");
    hash.update(t.sha256().unwrap().as_bytes());
    for item in &chunks {
        hash.update(item.header.sha256(&item.payload).unwrap().as_bytes());
    }
    assert_eq!(receipt.group_sha256, format!("{:x}", hash.finalize()));
}

#[test]
fn content_replay_does_not_claim_historical_group_atomicity_and_append_stays_compatible() {
    let f = Fixture::new();
    let t = descriptor();
    let mut j = Journal::create(f.path(), &t).unwrap();
    let chunks = items(&t);
    for item in &chunks {
        j.append(&item.header, &item.payload).unwrap();
    }
    let replay = j.append_group(&refs(&chunks)).unwrap();
    assert!(replay.replayed);
    assert_eq!((replay.first_ordinal, replay.last_ordinal), (1, 4));
    let next = header(&t, 0, 2, 104, 1);
    assert!(!j.append(&next, &1f32.to_le_bytes()).unwrap().replayed);
    assert!(j.append(&next, &1f32.to_le_bytes()).unwrap().replayed);
    let later = j.append_group(&refs(&chunks)).unwrap();
    assert_eq!(later.group_sha256, replay.group_sha256);
    assert_eq!(later.chunks[0].durable_prefix.next_sequence, 3);
    assert_eq!(f.rows(), 5);
}

#[test]
fn mixed_conflicting_duplicate_reordered_and_nonconsecutive_retries_do_not_mutate() {
    let f = Fixture::new();
    let t = descriptor();
    let mut j = Journal::create(f.path(), &t).unwrap();
    let chunks = items(&t);
    j.append_group(&refs(&chunks[..2])).unwrap();
    assert!(matches!(
        j.append_group(&refs(&chunks)),
        Err(RecordingError::Conflict(_))
    ));
    assert_eq!(f.rows(), 2);
    j.append_group(&refs(&chunks[2..])).unwrap();
    let reversed = [
        AppendChunk {
            header: &chunks[1].header,
            payload: &chunks[1].payload,
        },
        AppendChunk {
            header: &chunks[0].header,
            payload: &chunks[0].payload,
        },
    ];
    assert!(matches!(
        j.append_group(&reversed),
        Err(RecordingError::Conflict(_))
    ));
    let skipped = [
        AppendChunk {
            header: &chunks[0].header,
            payload: &chunks[0].payload,
        },
        AppendChunk {
            header: &chunks[2].header,
            payload: &chunks[2].payload,
        },
    ];
    assert!(matches!(
        j.append_group(&skipped),
        Err(RecordingError::Conflict(_))
    ));
    let duplicate = [AppendChunk {
        header: &chunks[0].header,
        payload: &chunks[0].payload,
    }; 2];
    assert!(matches!(
        j.append_group(&duplicate),
        Err(RecordingError::Conflict(_))
    ));
    let mut changed = chunks[0].payload.clone();
    changed[0] ^= 1;
    assert!(matches!(
        j.append_group(&[AppendChunk {
            header: &chunks[0].header,
            payload: &changed
        }]),
        Err(RecordingError::Conflict(_))
    ));
    assert_eq!(f.rows(), 4);
    assert_eq!(j.recovery_report().unwrap().verified_chunks, 4);
}

#[test]
fn full_prevalidation_and_prefix_simulation_precede_any_insert() {
    let f = Fixture::new();
    let t = descriptor();
    let mut j = Journal::create(f.path(), &t).unwrap();
    let mut chunks = items(&t);
    chunks[3].payload.pop();
    assert!(j.append_group(&refs(&chunks)).is_err());
    assert_eq!(f.rows(), 0);
    let mut chunks = items(&t);
    chunks[2].header.range = SourceRange::AudioFrames {
        start: 103,
        count: 2,
    };
    assert!(j.append_group(&refs(&chunks)).is_err());
    assert_eq!(f.rows(), 0);
    let chunks = items(&t);
    assert_eq!(j.append_group(&refs(&chunks)).unwrap().last_ordinal, 4);
}

#[test]
fn group_count_and_canonical_header_plus_payload_budget_are_bounded() {
    let f = Fixture::new();
    let t = descriptor();
    let mut j = Journal::create(f.path(), &t).unwrap();
    assert!(j.append_group(&[]).is_err());
    let headers: Vec<_> = (0..65)
        .map(|sequence| {
            let mut h = header(&t, 0, sequence, 100 + sequence, 1);
            h.kind = ChunkKind::Gap {
                reason: GapReason::DeclaredLoss("Synthetic gap bound fixture".into()),
            };
            h
        })
        .collect();
    let requests: Vec<_> = headers
        .iter()
        .map(|header| AppendChunk {
            header,
            payload: &[],
        })
        .collect();
    assert!(j.append_group(&requests).is_err());
    assert_eq!(f.rows(), 0);
    let payload = vec![0u8; MAX_AUDIO_CHUNK_BYTES];
    let large: Vec<_> = (0..8)
        .map(|sequence| header(&t, 0, sequence, 100 + sequence * 262_144, 262_144))
        .collect();
    let requests: Vec<_> = large
        .iter()
        .map(|header| AppendChunk {
            header,
            payload: &payload,
        })
        .collect();
    assert!(j.append_group(&requests).is_err());
    assert_eq!(f.rows(), 0);
    let requests: Vec<_> = headers[..64]
        .iter()
        .map(|header| AppendChunk {
            header,
            payload: &[],
        })
        .collect();
    assert_eq!(j.append_group(&requests).unwrap().chunks.len(), 64);
    assert_eq!(f.rows(), 64);
}

#[test]
fn exact_old_groups_replay_sealed_and_interrupted_but_new_chunks_do_not() {
    for sealed in [false, true] {
        let f = Fixture::new();
        let t = descriptor();
        let chunks = items(&t);
        let mut j = Journal::create(f.path(), &t).unwrap();
        j.append_group(&refs(&chunks)).unwrap();
        if sealed {
            j.seal("explicit-test-seal").unwrap();
        }
        drop(j);
        let mut j = Journal::open(f.path()).unwrap();
        assert_eq!(
            j.state().unwrap(),
            if sealed {
                TakeState::Sealed
            } else {
                TakeState::Interrupted
            }
        );
        let before = fs::read(f.path()).unwrap();
        assert!(j.append_group(&refs(&chunks)).unwrap().replayed);
        assert_eq!(fs::read(f.path()).unwrap(), before);
        let next = header(&t, 0, 2, 104, 1);
        let result = j.append_group(&[AppendChunk {
            header: &next,
            payload: &1f32.to_le_bytes(),
        }]);
        if !sealed {
            assert!(matches!(result, Err(RecordingError::Interrupted)));
        } else {
            assert!(result.is_err());
        }
        assert_eq!(f.rows(), 4);
    }
}

#[test]
fn damaged_tail_replay_is_confined_to_authenticated_recovered_prefix() {
    let f = Fixture::new();
    let t = descriptor();
    let chunks = items(&t);
    let mut j = Journal::create(f.path(), &t).unwrap();
    j.append_group(&refs(&chunks)).unwrap();
    drop(j);
    let c = Connection::open(f.path()).unwrap();
    c.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=4;").unwrap();
    drop(c);
    let mut j = Journal::open(f.path()).unwrap();
    assert_eq!(j.recovery_report().unwrap().verified_chunks, 3);
    assert!(j.append_group(&refs(&chunks[..3])).unwrap().replayed);
    assert!(j.append_group(&refs(&chunks)).is_err());
    assert_eq!(f.rows(), 4);
    j.seal_recovered("explicit-prefix-only").unwrap();
    assert!(j.append_group(&refs(&chunks[..3])).unwrap().replayed);
    assert!(j.append_group(&refs(&chunks)).is_err());
    assert_eq!(f.rows(), 4);
}

#[cfg(feature = "test-hooks")]
#[test]
fn failure_after_each_insert_or_before_commit_rolls_back_every_stream_and_ordinal() {
    for stop_at in 1..=5 {
        let f = Fixture::new();
        let t = descriptor();
        let chunks = items(&t);
        let mut j = Journal::create(f.path(), &t).unwrap();
        let mut inserts = 0;
        j.set_boundary_hook(move |boundary| {
            if boundary == WriteBoundary::AfterChunkInsert {
                inserts += 1;
            }
            if (boundary == WriteBoundary::AfterChunkInsert && inserts == stop_at)
                || (boundary == WriteBoundary::BeforeChunkCommit && stop_at == 5)
            {
                return Err(RecordingError::Invalid(
                    "Injected whole-group rollback".into(),
                ));
            }
            Ok(())
        });
        assert!(j.append_group(&refs(&chunks)).is_err());
        j.clear_boundary_hook();
        assert_eq!(f.rows(), 0);
        let report = j.recovery_report().unwrap();
        assert_eq!(report.verified_chunks, 0);
        assert!(report.streams.iter().all(|s| s.next_sequence == 0));
        let receipt = j.append_group(&refs(&chunks)).unwrap();
        assert_eq!((receipt.first_ordinal, receipt.last_ordinal), (1, 4));
        assert!(!receipt.replayed);
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn lost_ack_after_group_commit_reopens_and_replays_without_duplicate_rows() {
    let f = Fixture::new();
    let t = descriptor();
    let chunks = items(&t);
    let mut j = Journal::create(f.path(), &t).unwrap();
    j.set_boundary_hook(|boundary| {
        if boundary == WriteBoundary::AfterChunkCommit {
            Err(RecordingError::Invalid(
                "Injected lost group acknowledgement".into(),
            ))
        } else {
            Ok(())
        }
    });
    assert!(j.append_group(&refs(&chunks)).is_err());
    assert_eq!(f.rows(), 4);
    drop(j);
    let mut j = Journal::open(f.path()).unwrap();
    let receipt = j.append_group(&refs(&chunks)).unwrap();
    assert!(receipt.replayed);
    assert_eq!((receipt.first_ordinal, receipt.last_ordinal), (1, 4));
    assert_eq!(f.rows(), 4);
    assert_eq!(j.recovery_report().unwrap().verified_chunks, 4);
}
