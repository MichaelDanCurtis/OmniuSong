//! Low-level selected byte-prefix recovery, not engine checkpoint acceptance.
use omniusong_core::contracts::{
    CaptureTime, CapturedEvent, EndpointId, Envelope, EventId, Extensions, MessageId, MusicalEvent,
    RawPacket, SessionId, SourceAddress,
};
use omniusong_recording::*;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/selected-prefix-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn rows(&self) -> Vec<(i64, Vec<u8>, Vec<u8>, String)> {
        let c =
            Connection::open_with_flags(self.path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let mut q = c
            .prepare("SELECT ordinal,header,payload,sha256 FROM chunks ORDER BY ordinal")
            .unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
    fn state(&self) -> String {
        Connection::open_with_flags(self.path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("SELECT state FROM take_meta", [], |r| r.get(0))
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn descriptor(marker: Option<serde_json::Value>) -> TakeDescriptor {
    let clock = HostClock {
        domain: ClockDomainId("synthetic".into()),
        epoch: ClockEpochId("epoch".into()),
        ticks_per_second_numerator: 48_000,
        ticks_per_second_denominator: 1,
    };
    let audio = StreamDescriptor {
        stream_id: "audio".into(),
        segment_id: "pcm".into(),
        source_id: "bus".into(),
        source_name: "Synthetic software fixture; no device".into(),
        source_connection_generation: 1,
        host_clock: clock.clone(),
        kind: StreamKind::Audio {
            format: PcmFormat {
                sample_rate: 48_000,
                channels: 1,
                encoding: PcmEncoding::F32Le,
                layout: PcmLayout::Interleaved,
            },
            audio_epoch: ClockEpochId("audio-epoch".into()),
        },
        initial_source_position: 100,
    };
    let raw = StreamDescriptor {
        stream_id: "control".into(),
        segment_id: "events".into(),
        source_id: "local".into(),
        source_name: "Synthetic local events".into(),
        source_connection_generation: 1,
        host_clock: clock,
        kind: StreamKind::RawEvents {
            format: RawEventFormat::LengthPrefixedCaptureV1,
        },
        initial_source_position: 0,
    };
    let mut provenance=BTreeMap::from([("opaque".into(),serde_json::from_str(r#"{"integer":123456789012345678901234567890,"decimal":0.12345678901234567890123456789}"#).unwrap())]);
    if let Some(value) = marker {
        provenance.insert("omniusong.performance.v1".into(), value);
    }
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![audio, raw],
        provenance,
    }
}
fn header(t: &TakeDescriptor, index: usize, sequence: u64) -> ChunkHeader {
    let s = &t.streams[index];
    let stamp = HostTimestamp {
        domain: s.host_clock.domain.clone(),
        epoch: s.host_clock.epoch.clone(),
        ticks: sequence,
    };
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: t.take_id.clone(),
        stream_id: s.stream_id.clone(),
        segment_id: s.segment_id.clone(),
        sequence,
        range: if index == 0 {
            SourceRange::AudioFrames {
                start: 100 + sequence * 2,
                count: 2,
            }
        } else {
            SourceRange::Events {
                start: sequence,
                count: 1,
            }
        },
        interpretation: s.kind.clone(),
        observed_start: stamp.clone(),
        observed_end: stamp,
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
fn populate(f: &Fixture, marker: Option<serde_json::Value>, bad_tail: bool) -> TakeDescriptor {
    let t = descriptor(marker);
    let mut j = Journal::create(f.path(), &t).unwrap();
    let s = &t.streams[1];
    let e = CapturedEvent {
        event_id: EventId("local-fixture".into()),
        source: SourceAddress {
            endpoint: EndpointId("local".into()),
            connection_generation: 1,
            group: 0,
            channel: None,
            extensions: Extensions::new(),
        },
        time: CaptureTime {
            observed: HostTimestamp {
                domain: s.host_clock.domain.clone(),
                epoch: s.host_clock.epoch.clone(),
                ticks: 0,
            },
            alignment: None,
            extensions: Extensions::new(),
        },
        raw: RawPacket::Local,
        musical: MusicalEvent::Uninterpreted,
        extensions: Extensions::new(),
    };
    let wire = Envelope::new(
        SessionId("synthetic".into()),
        MessageId("event".into()),
        0,
        e,
    )
    .encode()
    .unwrap();
    let payload = encode_event_chunk(&[&wire]).unwrap();
    j.append(&header(&t, 1, 0), &payload).unwrap();
    for sequence in 0..3 {
        j.append(
            &header(&t, 0, sequence),
            &[0.25f32.to_le_bytes(), 0.5f32.to_le_bytes()].concat(),
        )
        .unwrap();
    }
    drop(j);
    if bad_tail {
        let c = Connection::open(f.path()).unwrap();
        c.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=4;").unwrap();
    }
    t
}
fn select(reader: &CaptureReader, count: u64) -> RecoveryPrefix {
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.recording-index.v1\0");
    hash.update(reader.inspection().descriptor_sha256.as_bytes());
    for ordinal in 1..=count {
        let chunk = reader.read_ordinal(ordinal).unwrap();
        hash.update(ordinal.to_le_bytes());
        hash.update(chunk.sha256.as_bytes());
    }
    RecoveryPrefix {
        chunk_count: count,
        chunk_index_sha256: format!("{:x}", hash.finalize()),
    }
}

#[test]
fn selected_shorter_prefix_preserves_later_good_bad_rows_and_original_descriptor() {
    let f = Fixture::new();
    let t = populate(
        &f,
        Some(serde_json::json!({"version":1,"synthetic":true})),
        true,
    );
    let original_rows = f.rows();
    let original_bytes = fs::read(f.path()).unwrap();
    let reader = CaptureReader::open(f.path()).unwrap();
    let inspection = reader.inspection().clone();
    assert_eq!(inspection.report.verified_chunks, 3);
    assert!(reader.read_ordinal(0).is_err());
    assert!(reader.read_ordinal(4).is_err());
    assert_eq!(reader.read_ordinal(3).unwrap().header.sequence, 1);
    assert_eq!(fs::read(f.path()).unwrap(), original_bytes);
    let selection = select(&reader, 2);
    let (manifest, replayed) = reader.recover_prefix("checked-prefix", &selection).unwrap();
    assert!(!replayed);
    assert_eq!(manifest.body.chunk_count, 2);
    assert_eq!(manifest.body.descriptor, t);
    assert_eq!(
        manifest
            .body
            .interrupted
            .as_ref()
            .unwrap()
            .report
            .verified_chunks,
        3
    );
    assert_eq!(
        manifest.body.streams[0].source_end,
        SourcePosition::AudioFrames(102)
    );
    assert_eq!(manifest.body.streams[0].recorded_units, 2);
    assert_eq!(f.rows(), original_rows);
    let reader = CaptureReader::open(f.path()).unwrap();
    assert_eq!(
        reader.inspection().original_rows_sha256,
        inspection.original_rows_sha256
    );
    assert_ne!(
        reader.inspection().journal_sha256,
        inspection.journal_sha256
    );
    assert!(reader.read_ordinal(3).is_err());
    assert_eq!(reader.inspection().report.verified_chunks, 2);
    drop(reader);
    let sealed = SealedJournal::open(f.path()).unwrap();
    let backup = f.0.join("complete-original-backup.sqlite3");
    sealed.export_backup(&backup).unwrap();
    let c =
        Connection::open_with_flags(backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let count: i64 = c
        .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 4);
}

#[test]
fn selection_validation_rejects_before_takeover_and_writes() {
    let f = Fixture::new();
    populate(&f, None, false);
    let original = fs::read(f.path()).unwrap();
    let reader = CaptureReader::open(f.path()).unwrap();
    let good = select(&reader, 2);
    let only_events = select(&reader, 1);
    drop(reader);
    for bad in [
        RecoveryPrefix {
            chunk_count: 0,
            ..good.clone()
        },
        RecoveryPrefix {
            chunk_count: 5,
            ..good.clone()
        },
        RecoveryPrefix {
            chunk_index_sha256: "0".repeat(64),
            ..good.clone()
        },
        RecoveryPrefix {
            chunk_index_sha256: "A".repeat(64),
            ..good.clone()
        },
        only_events,
    ] {
        assert!(
            CaptureReader::open(f.path())
                .unwrap()
                .recover_prefix("bad-selection", &bad)
                .is_err()
        );
        assert_eq!(f.state(), "recording");
        assert_eq!(fs::read(f.path()).unwrap(), original);
    }
    assert!(serde_json::from_value::<RecoveryPrefix>(serde_json::json!({"chunk_count":2,"chunk_index_sha256":good.chunk_index_sha256,"extra":1})).is_err());
}

#[test]
fn replay_requires_same_operation_recovered_mode_selected_count_and_hash() {
    let f = Fixture::new();
    populate(&f, None, false);
    let reader = CaptureReader::open(f.path()).unwrap();
    let selection = select(&reader, 3);
    let other = select(&reader, 2);
    let (first, replayed) = reader
        .recover_prefix("selected-operation", &selection)
        .unwrap();
    assert!(!replayed);
    let before = fs::read(f.path()).unwrap();
    let (replay, replayed) = CaptureReader::open(f.path())
        .unwrap()
        .recover_prefix("selected-operation", &selection)
        .unwrap();
    assert!(replayed);
    assert_eq!(replay, first);
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert!(
        CaptureReader::open(f.path())
            .unwrap()
            .recover_prefix("changed-operation", &selection)
            .is_err()
    );
    assert!(
        CaptureReader::open(f.path())
            .unwrap()
            .recover_prefix("selected-operation", &other)
            .is_err()
    );
    let wrong = RecoveryPrefix {
        chunk_index_sha256: "0".repeat(64),
        ..selection
    };
    assert!(
        CaptureReader::open(f.path())
            .unwrap()
            .recover_prefix("selected-operation", &wrong)
            .is_err()
    );
    assert_eq!(fs::read(f.path()).unwrap(), before);
    let f = Fixture::new();
    populate(&f, None, false);
    let mut journal = Journal::open(f.path()).unwrap();
    journal.seal_recovered("generic-whole-prefix").unwrap();
    drop(journal);
    let reader = CaptureReader::open(f.path()).unwrap();
    let whole = select(&reader, 4);
    assert!(reader.recover_prefix("different-id", &whole).is_err());
}

#[test]
fn any_performance_marker_blocks_generic_paths_but_explicit_selection_is_available() {
    for marker in [
        serde_json::Value::Null,
        serde_json::json!(false),
        serde_json::json!({"unsupportedVersion":999}),
    ] {
        let f = Fixture::new();
        let t = descriptor(Some(marker));
        let mut j = Journal::create(f.path(), &t).unwrap();
        j.append(&header(&t, 0, 0), &[1; 8]).unwrap();
        let before = fs::read(f.path()).unwrap();
        assert!(j.seal("generic").is_err());
        assert!(j.seal_recovered("generic").is_err());
        assert_eq!(j.state().unwrap(), TakeState::Recording);
        assert_eq!(fs::read(f.path()).unwrap(), before);
        drop(j);
        assert!(
            CaptureReader::open(f.path())
                .unwrap()
                .recover("generic")
                .is_err()
        );
        assert_eq!(fs::read(f.path()).unwrap(), before);
        let reader = CaptureReader::open(f.path()).unwrap();
        let selected = select(&reader, 1);
        let (_, replayed) = reader
            .recover_prefix("explicit-lowlevel-selection", &selected)
            .unwrap();
        assert!(!replayed);
        let mut j = Journal::open(f.path()).unwrap();
        assert!(j.seal_recovered("explicit-lowlevel-selection").is_err());
    }
}

#[test]
fn ordinary_generic_recovery_remains_full_prefix_and_normal_manifest_is_not_recovered_replay() {
    let f = Fixture::new();
    populate(&f, None, false);
    let (manifest, replayed) = CaptureReader::open(f.path())
        .unwrap()
        .recover("ordinary-generic")
        .unwrap();
    assert!(!replayed);
    assert_eq!(manifest.body.chunk_count, 4);
    let f = Fixture::new();
    let t = descriptor(None);
    let mut j = Journal::create(f.path(), &t).unwrap();
    j.append(&header(&t, 0, 0), &[0; 8]).unwrap();
    j.seal("normal-original").unwrap();
    drop(j);
    let reader = CaptureReader::open(f.path()).unwrap();
    let selection = select(&reader, 1);
    assert!(
        reader
            .recover_prefix("normal-original", &selection)
            .is_err()
    );
}
