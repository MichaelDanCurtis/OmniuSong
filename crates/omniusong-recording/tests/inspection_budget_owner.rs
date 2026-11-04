//! Device-free governor integration; no normal-seal or recovery authority.
use omniusong_core::contracts::{
    CaptureTime, CapturedEvent, EndpointId, Envelope, EventId, MessageId, MusicalEvent, RawPacket,
    SessionId, SourceAddress,
};
use omniusong_recording::*;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/inspection-budget-owner")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn governor(limits: InspectionLimits) -> InspectionBudget {
    InspectionBudget::new(limits, Arc::new(AtomicBool::new(false))).unwrap()
}
fn descriptor() -> TakeDescriptor {
    let clock = HostClock {
        domain: ClockDomainId("fixture-clock".into()),
        epoch: ClockEpochId("fixture-epoch".into()),
        ticks_per_second_numerator: 48000,
        ticks_per_second_denominator: 1,
    };
    let audio = StreamDescriptor {
        stream_id: "audio".into(),
        segment_id: "segment".into(),
        source_id: "fixture-local".into(),
        source_name: "Synthetic bounded fixture".into(),
        source_connection_generation: 1,
        host_clock: clock,
        kind: StreamKind::Audio {
            format: PcmFormat {
                sample_rate: 48000,
                channels: 1,
                encoding: PcmEncoding::S16Le,
                layout: PcmLayout::Interleaved,
            },
            audio_epoch: ClockEpochId("fixture-audio".into()),
        },
        initial_source_position: 0,
    };
    let mut raw = audio.clone();
    raw.stream_id = "raw".into();
    raw.kind = StreamKind::RawEvents {
        format: RawEventFormat::LengthPrefixedCaptureV1,
    };
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![audio, raw],
        provenance: BTreeMap::new(),
    }
}
fn header(take: &TakeDescriptor, index: usize) -> ChunkHeader {
    let stream = &take.streams[index];
    let stamp = HostTimestamp {
        domain: stream.host_clock.domain.clone(),
        epoch: stream.host_clock.epoch.clone(),
        ticks: 0,
    };
    let mut end = stamp.clone();
    end.ticks = 2;
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: take.take_id.clone(),
        stream_id: stream.stream_id.clone(),
        segment_id: stream.segment_id.clone(),
        sequence: 0,
        range: if index == 0 {
            SourceRange::AudioFrames { start: 0, count: 2 }
        } else {
            SourceRange::Events { start: 0, count: 2 }
        },
        interpretation: stream.kind.clone(),
        observed_start: stamp,
        observed_end: end,
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
fn event(index: u64) -> Vec<u8> {
    Envelope::new(
        SessionId("fixture-session".into()),
        MessageId(format!("message-{index}")),
        index,
        CapturedEvent {
            event_id: EventId(format!("event-{index}")),
            source: SourceAddress {
                endpoint: EndpointId("fixture-local".into()),
                connection_generation: 1,
                group: 0,
                channel: Some(0),
                extensions: Default::default(),
            },
            time: CaptureTime {
                observed: HostTimestamp {
                    domain: ClockDomainId("fixture-clock".into()),
                    epoch: ClockEpochId("fixture-epoch".into()),
                    ticks: index,
                },
                alignment: None,
                extensions: Default::default(),
            },
            raw: RawPacket::Midi1 {
                bytes: vec![0x90, 60, 64],
            },
            musical: MusicalEvent::Uninterpreted,
            extensions: Default::default(),
        },
    )
    .encode()
    .unwrap()
}

#[test]
fn raw_decode_refusal_escapes_the_damaged_tail_catcher_after_valid_audio() {
    let f = Fixture::new();
    let take = descriptor();
    let mut writer = Journal::create(f.path(), &take).unwrap();
    writer.append(&header(&take, 0), &[0, 0, 1, 0]).unwrap();
    let one = event(1);
    let two = event(2);
    let payload = encode_event_chunk(&[&one, &two]).unwrap();
    writer.append(&header(&take, 1), &payload).unwrap();
    drop(writer);
    let before = fs::read(f.path()).unwrap();
    let budget = governor(InspectionLimits {
        max_record_visits: 1,
        ..InspectionLimits::default()
    });
    assert!(matches!(
        CaptureReader::open_bounded(f.path(), &budget),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::RecordVisits,
            limit: 1,
            attempted: 2
        })
    ));
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert_eq!(
        CaptureReader::open(f.path())
            .unwrap()
            .inspection()
            .report
            .verified_chunks,
        2
    );
}

#[test]
fn preflight_includes_derived_rows_not_selected_by_generic_scan() {
    let f = Fixture::new();
    let take = descriptor();
    let mut writer = Journal::create(f.path(), &take).unwrap();
    writer.append(&header(&take, 0), &[0; 4]).unwrap();
    drop(writer);
    let probe = governor(InspectionLimits::default());
    drop(CaptureReader::open_bounded(f.path(), &probe).unwrap());
    let initial = probe.usage().physical_rows;
    let sql = rusqlite::Connection::open(f.path()).unwrap();
    for ordinal in 1..=3 {
        sql.execute(
            "INSERT INTO recovery_index(ordinal,sha256) VALUES(?1,?2)",
            rusqlite::params![ordinal, "a".repeat(64)],
        )
        .unwrap();
    }
    drop(sql);
    let before = fs::read(f.path()).unwrap();
    let budget = governor(InspectionLimits {
        max_physical_rows: initial + 2,
        ..InspectionLimits::default()
    });
    assert!(matches!(
        CaptureReader::open_bounded(f.path(), &budget),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::PhysicalRows,
            ..
        })
    ));
    assert_eq!(fs::read(f.path()).unwrap(), before);
}

#[test]
fn bounded_views_cannot_escape_to_unmetered_recovery_or_export() {
    let f = Fixture::new();
    let take = descriptor();
    let mut writer = Journal::create(f.path(), &take).unwrap();
    writer.append(&header(&take, 0), &[0; 4]).unwrap();
    drop(writer);
    let before = fs::read(f.path()).unwrap();
    let budget = governor(InspectionLimits::default());
    assert!(
        CaptureReader::open_bounded(f.path(), &budget)
            .unwrap()
            .recover("not-authorized")
            .is_err()
    );
    assert_eq!(fs::read(f.path()).unwrap(), before);
    let mut writer = Journal::open(f.path()).unwrap();
    writer
        .seal_recovered("fixture-only-generic-recovery")
        .unwrap();
    drop(writer);
    let before = fs::read(f.path()).unwrap();
    let budget = governor(InspectionLimits::default());
    let reader = SealedJournal::open_bounded(f.path(), &budget).unwrap();
    assert!(
        reader
            .export_backup(f.0.join("not-created.sqlite3"))
            .is_err()
    );
    assert!(!f.0.join("not-created.sqlite3").exists());
    assert_eq!(fs::read(f.path()).unwrap(), before);
}
