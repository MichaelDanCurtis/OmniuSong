use crate::*;
use omniusong_core::contracts::{
    CaptureTime, CapturedEvent, EndpointId, Envelope, EventId, MessageId, MusicalEvent, RawPacket,
    SessionId, SourceAddress,
};
use rusqlite::{Connection, params};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Fixture(pub(super) PathBuf);
impl Fixture {
    pub(super) fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".runtime/rust-migration/recording-tests");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub(super) fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn descriptor(audio: bool) -> TakeDescriptor {
    let kind = if audio {
        StreamKind::Audio {
            format: PcmFormat {
                sample_rate: 48_000,
                channels: 2,
                encoding: PcmEncoding::F32Le,
                layout: PcmLayout::Interleaved,
            },
            audio_epoch: ClockEpochId("audio-1".into()),
        }
    } else {
        StreamKind::RawEvents {
            format: RawEventFormat::LengthPrefixedCaptureV1,
        }
    };
    TakeDescriptor {
        version: 1,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![StreamDescriptor {
            stream_id: "source-1".into(),
            segment_id: "segment-1".into(),
            source_id: "device-1".into(),
            source_name: "Fixture source".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("host".into()),
                epoch: ClockEpochId("boot".into()),
                ticks_per_second_numerator: 48_000,
                ticks_per_second_denominator: 1,
            },
            kind,
            initial_source_position: 0,
        }],
        provenance: BTreeMap::new(),
    }
}
pub(super) fn header(take: &TakeDescriptor, sequence: u64, start: u64, count: u64) -> ChunkHeader {
    let stream = &take.streams[0];
    ChunkHeader {
        version: 1,
        take_id: take.take_id.clone(),
        stream_id: stream.stream_id.clone(),
        segment_id: stream.segment_id.clone(),
        sequence,
        range: match stream.kind {
            StreamKind::Audio { .. } => SourceRange::AudioFrames { start, count },
            StreamKind::RawEvents { .. } => SourceRange::Events { start, count },
        },
        interpretation: stream.kind.clone(),
        observed_start: HostTimestamp {
            domain: stream.host_clock.domain.clone(),
            epoch: stream.host_clock.epoch.clone(),
            ticks: start,
        },
        observed_end: HostTimestamp {
            domain: stream.host_clock.domain.clone(),
            epoch: stream.host_clock.epoch.clone(),
            ticks: start + count,
        },
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
fn event(index: u64) -> Vec<u8> {
    Envelope::new(
        SessionId("capture-session".into()),
        MessageId(format!("message-{index}")),
        index,
        CapturedEvent {
            event_id: EventId(format!("event-{index}")),
            source: SourceAddress {
                endpoint: EndpointId("device-1".into()),
                connection_generation: 1,
                group: 0,
                channel: Some(0),
                extensions: Default::default(),
            },
            time: CaptureTime {
                observed: HostTimestamp {
                    domain: ClockDomainId("host".into()),
                    epoch: ClockEpochId("boot".into()),
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
fn injected_error() -> RecordingError {
    RecordingError::Invalid("Injected writer boundary failure".into())
}

#[test]
fn audio_larger_than_legacy_cap_is_streamed_and_sealed_without_whole_take_buffer() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let payload = vec![7_u8; MAX_AUDIO_CHUNK_BYTES];
    let frames = payload.len() as u64 / 8;
    for sequence in 0..33 {
        let receipt = journal
            .append(
                &header(&take, sequence, sequence * frames, frames),
                &payload,
            )
            .unwrap();
        assert!(!receipt.replayed);
        assert_eq!(
            receipt.durable_prefix.source_end,
            SourcePosition::AudioFrames((sequence + 1) * frames)
        );
    }
    let manifest = journal.seal("seal-long-audio").unwrap();
    assert_eq!(
        manifest.body.payload_bytes,
        33 * MAX_AUDIO_CHUNK_BYTES as u64
    );
    assert_eq!(manifest.body.chunk_count, 33);
    assert!(manifest.body.payload_bytes > 30 * 1024 * 1024);
    assert!(manifest.body.interrupted.is_none());
    manifest.verify_hash().unwrap();
    drop(journal);
    let reopened = Journal::open(fixture.path()).unwrap();
    assert_eq!(reopened.sealed_manifest().unwrap(), Some(manifest));
    assert_eq!(
        reopened.read_chunk("source-1", 32).unwrap().payload,
        payload
    );
}

#[test]
fn midi_only_more_than_embedded_event_cap_retains_exact_records_without_fake_frames() {
    let fixture = Fixture::new();
    let mut take = descriptor(false);
    take.provenance.insert(
        "future:counter".into(),
        serde_json::from_str("18446744073709551616001").unwrap(),
    );
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    for sequence in 0..80 {
        let start = sequence * 64;
        let records: Vec<Vec<u8>> = (start..start + 64).map(event).collect();
        let refs: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
        let bytes = encode_event_chunk(&refs).unwrap();
        journal
            .append(&header(&take, sequence, start, 64), &bytes)
            .unwrap();
    }
    let manifest = journal.seal("seal-midi-only").unwrap();
    assert_eq!(
        manifest.body.streams[0].source_end,
        SourcePosition::Events(5120)
    );
    assert_eq!(
        manifest.body.descriptor.provenance["future:counter"].to_string(),
        "18446744073709551616001"
    );
    let record = journal.read_chunk("source-1", 0).unwrap();
    let length = u32::from_le_bytes(record.payload[..4].try_into().unwrap()) as usize;
    assert_eq!(&record.payload[4..4 + length], event(0));
}

#[test]
fn live_owner_rejected_then_restart_preserves_interruption_receipt_and_forbids_resume() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut writer = Journal::create(fixture.path(), &take).unwrap();
    let first = header(&take, 0, 0, 4);
    let bytes = vec![8; 32];
    writer.append(&first, &bytes).unwrap();
    assert!(matches!(
        Journal::open(fixture.path()),
        Err(RecordingError::Owned)
    ));
    assert_eq!(writer.state().unwrap(), TakeState::Recording);
    drop(writer);
    let mut recovery = Journal::open(fixture.path()).unwrap();
    let report = recovery.recovery_report().unwrap();
    assert_eq!(report.state, TakeState::Interrupted);
    assert!(report.unknown_tail);
    assert_eq!(report.verified_chunks, 1);
    assert!(recovery.append(&first, &bytes).unwrap().replayed);
    assert!(matches!(
        recovery.append(&header(&take, 1, 4, 4), &bytes),
        Err(RecordingError::Interrupted)
    ));
    let manifest = recovery.seal_recovered("user-recover-1").unwrap();
    let interrupted = manifest.body.interrupted.as_ref().unwrap();
    assert!(interrupted.report.unknown_tail);
    assert_eq!(interrupted.report.state, TakeState::Interrupted);
    drop(recovery);
    let mut reopened = Journal::open(fixture.path()).unwrap();
    assert_eq!(reopened.seal_recovered("user-recover-1").unwrap(), manifest);
    assert!(reopened.seal("different-finish-mode").is_err());
}

#[test]
fn explicit_gaps_are_hashed_and_discontinuities_or_empty_gaps_rejected() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    journal.append(&header(&take, 0, 0, 4), &[1; 32]).unwrap();
    assert!(journal.append(&header(&take, 1, 8, 4), &[1; 32]).is_err());
    let mut gap = header(&take, 1, 4, 4);
    gap.kind = ChunkKind::Gap {
        reason: GapReason::TransferOverflow,
    };
    journal.append(&gap, &[]).unwrap();
    let mut empty = header(&take, 2, 8, 0);
    empty.kind = gap.kind.clone();
    assert!(journal.append(&empty, &[]).is_err());
    journal.append(&header(&take, 2, 8, 4), &[2; 32]).unwrap();
    let manifest = journal.seal("seal-gap").unwrap();
    assert_eq!(manifest.body.streams[0].recorded_units, 8);
    assert_eq!(manifest.body.streams[0].gap_units, 4);
    assert_eq!(manifest.body.streams[0].gap_chunks, 1);
}

#[test]
fn metadata_format_clock_ranges_and_versions_are_authenticated() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let original = header(&take, 0, 0, 4);
    let hash = original.sha256(&[5; 32]).unwrap();
    let mut changed = original.clone();
    changed.observed_end.ticks += 1;
    assert_ne!(changed.sha256(&[5; 32]).unwrap(), hash);
    changed = original.clone();
    changed.version = 2;
    assert!(journal.append(&changed, &[5; 32]).is_err());
    changed = original.clone();
    changed.observed_start.epoch = ClockEpochId("different-boot".into());
    assert!(journal.append(&changed, &[5; 32]).is_err());
    changed = original.clone();
    if let StreamKind::Audio { format, .. } = &mut changed.interpretation {
        format.sample_rate = 44_100;
    }
    assert!(journal.append(&changed, &[5; 32]).is_err());
    assert!(journal.append(&original, &[5; 31]).is_err());
    let oversized = vec![5; MAX_AUDIO_CHUNK_BYTES + 8];
    assert!(
        journal
            .append(&header(&take, 0, 0, oversized.len() as u64 / 8), &oversized)
            .is_err()
    );
    journal.append(&original, &[5; 32]).unwrap();
    let mut conflict = original.clone();
    conflict.observed_end.ticks += 1;
    assert!(matches!(
        journal.append(&conflict, &[5; 32]),
        Err(RecordingError::Conflict(_))
    ));
}

#[test]
fn before_commit_faults_roll_back_chunk_and_prefix_together() {
    for boundary in [
        WriteBoundary::AfterChunkInsert,
        WriteBoundary::BeforeChunkCommit,
    ] {
        let fixture = Fixture::new();
        let take = descriptor(true);
        let mut journal = Journal::create(fixture.path(), &take).unwrap();
        journal.set_boundary_hook(move |point| {
            if point == boundary {
                Err(injected_error())
            } else {
                Ok(())
            }
        });
        assert!(journal.append(&header(&take, 0, 0, 4), &[4; 32]).is_err());
        journal.clear_boundary_hook();
        assert_eq!(journal.recovery_report().unwrap().verified_chunks, 0);
        let receipt = journal.append(&header(&take, 0, 0, 4), &[4; 32]).unwrap();
        assert!(!receipt.replayed);
        assert_eq!(receipt.durable_prefix.next_sequence, 1);
    }
}

#[test]
fn committed_chunk_lost_ack_is_replayed_without_duplicate_original() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    journal.set_boundary_hook(|point| {
        if point == WriteBoundary::AfterChunkCommit {
            Err(injected_error())
        } else {
            Ok(())
        }
    });
    let chunk = header(&take, 0, 0, 4);
    assert!(journal.append(&chunk, &[3; 32]).is_err());
    journal.clear_boundary_hook();
    assert!(journal.append(&chunk, &[3; 32]).unwrap().replayed);
    assert_eq!(journal.recovery_report().unwrap().verified_chunks, 1);
}

#[test]
fn seal_transaction_boundaries_and_committed_lost_ack_are_idempotent() {
    for boundary in [
        WriteBoundary::AfterManifestInsert,
        WriteBoundary::BeforeSealCommit,
        WriteBoundary::AfterSealCommit,
    ] {
        let fixture = Fixture::new();
        let take = descriptor(true);
        let mut journal = Journal::create(fixture.path(), &take).unwrap();
        journal.append(&header(&take, 0, 0, 4), &[2; 32]).unwrap();
        journal.set_boundary_hook(move |point| {
            if point == boundary {
                Err(injected_error())
            } else {
                Ok(())
            }
        });
        assert!(journal.seal("stop-1").is_err());
        journal.clear_boundary_hook();
        if boundary == WriteBoundary::AfterSealCommit {
            assert_eq!(journal.state().unwrap(), TakeState::Sealed);
            assert!(journal.sealed_manifest().unwrap().is_some());
        } else {
            assert_eq!(journal.state().unwrap(), TakeState::Recording);
            assert!(journal.sealed_manifest().unwrap().is_none());
        }
        let sealed = journal.seal("stop-1").unwrap();
        assert_eq!(journal.seal("stop-1").unwrap(), sealed);
        assert!(journal.seal("conflicting-stop").is_err());
    }
}

#[test]
fn corruption_keeps_proven_prefix_and_recovery_seal_retains_bad_original_tail() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    for sequence in 0..3 {
        journal
            .append(&header(&take, sequence, sequence * 4, 4), &[7; 32])
            .unwrap();
    }
    drop(journal);
    let db = Connection::open(fixture.path()).unwrap();
    db.execute_batch("DROP TRIGGER immutable_chunk_update;")
        .unwrap();
    db.execute(
        "UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=2",
        [],
    )
    .unwrap();
    drop(db);
    let mut recovery = Journal::open(fixture.path()).unwrap();
    let report = recovery.recovery_report().unwrap();
    assert_eq!(report.verified_chunks, 1);
    assert!(!report.problems.is_empty());
    assert!(report.unknown_tail);
    let sealed = recovery.seal_recovered("accept-proven-prefix").unwrap();
    assert_eq!(sealed.body.chunk_count, 1);
    assert!(
        !sealed
            .body
            .interrupted
            .as_ref()
            .unwrap()
            .report
            .problems
            .is_empty()
    );
    drop(recovery);
    let db = Connection::open(fixture.path()).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 3);
    drop(db);
    let reopened = Journal::open(fixture.path()).unwrap();
    assert_eq!(reopened.sealed_manifest().unwrap(), Some(sealed));
}

#[test]
fn truncated_payload_and_unknown_schema_are_rejected_without_erasing_originals() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    journal.append(&header(&take, 0, 0, 4), &[6; 32]).unwrap();
    drop(journal);
    let db = Connection::open(fixture.path()).unwrap();
    db.execute_batch(
        "DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=substr(payload,1,31);",
    )
    .unwrap();
    drop(db);
    let recovered = Journal::open(fixture.path()).unwrap();
    assert_eq!(recovered.recovery_report().unwrap().verified_chunks, 0);
    assert!(recovered.read_chunk("source-1", 0).is_err());
    drop(recovered);
    let db = Connection::open(fixture.path()).unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);
    assert!(Journal::open(fixture.path()).is_err());
}

#[test]
fn stored_oversize_blob_and_modified_sealed_manifest_fail_closed() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    journal.append(&header(&take, 0, 0, 4), &[6; 32]).unwrap();
    journal.seal("seal-1").unwrap();
    drop(journal);
    let db = Connection::open(fixture.path()).unwrap();
    db.execute_batch("DROP TRIGGER immutable_manifest_update;")
        .unwrap();
    db.execute(
        "UPDATE sealed_manifests SET body=zeroblob(?1)",
        [i64::try_from(MAX_METADATA_BYTES * 2 + 1).unwrap()],
    )
    .unwrap();
    drop(db);
    assert!(Journal::open(fixture.path()).is_err());
}

#[test]
fn unrelated_main_database_write_transaction_does_not_block_recording_commit() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let other = Connection::open(fixture.0.join("main-store-fixture.sqlite3")).unwrap();
    other.execute_batch("CREATE TABLE edits(value TEXT);BEGIN IMMEDIATE;INSERT INTO edits VALUES('uncommitted project edit');").unwrap();
    let receipt = journal.append(&header(&take, 0, 0, 4), &[1; 32]).unwrap();
    assert_eq!(receipt.durable_prefix.next_sequence, 1);
    other.execute_batch("ROLLBACK;").unwrap();
}

#[test]
fn sql_guards_reject_in_place_mutation_and_deletion_of_original_chunks() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    journal.append(&header(&take, 0, 0, 4), &[6; 32]).unwrap();
    let db = Connection::open(fixture.path()).unwrap();
    assert!(db.execute("DELETE FROM chunks", []).is_err());
    assert!(
        db.execute("UPDATE chunks SET payload=?1", params![vec![0_u8; 32]])
            .is_err()
    );
    assert_eq!(
        journal.read_chunk("source-1", 0).unwrap().payload,
        vec![6; 32]
    );
}

#[test]
fn host_ticks_keep_full_u64_while_sql_counters_reject_signed_overflow() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let mut chunk = header(&take, 0, 0, 4);
    chunk.observed_start.ticks = u64::MAX;
    chunk.observed_end.ticks = u64::MAX;
    journal.append(&chunk, &[1; 32]).unwrap();
    assert_eq!(
        journal
            .read_chunk("source-1", 0)
            .unwrap()
            .header
            .observed_start
            .ticks,
        u64::MAX
    );
    assert!(journal.read_chunk("source-1", u64::MAX).is_err());
    let mut invalid = take.clone();
    invalid.streams[0].initial_source_position = u64::MAX;
    assert!(invalid.validate().is_err());
}

#[test]
fn negative_stored_prefix_counter_is_not_reinterpreted_as_unsigned() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let journal = Journal::create(fixture.path(), &take).unwrap();
    drop(journal);
    let db = Connection::open(fixture.path()).unwrap();
    db.execute_batch(
        "PRAGMA ignore_check_constraints=ON; UPDATE stream_prefix SET next_sequence=-1;",
    )
    .unwrap();
    drop(db);
    let recovery = Journal::open(fixture.path()).unwrap();
    let report = recovery.recovery_report().unwrap();
    assert_eq!(report.verified_chunks, 0);
    assert!(!report.problems.is_empty());
}

#[test]
fn recovery_rejects_rehashed_replacement_bytes_after_interruption_receipt() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut writer = Journal::create(fixture.path(), &take).unwrap();
    let chunk = header(&take, 0, 0, 4);
    writer.append(&chunk, &[1; 32]).unwrap();
    drop(writer);
    let mut recovery = Journal::open(fixture.path()).unwrap();
    let db = Connection::open(fixture.path()).unwrap();
    db.execute_batch("DROP TRIGGER immutable_chunk_update;")
        .unwrap();
    db.execute(
        "UPDATE chunks SET payload=?1,sha256=?2 WHERE ordinal=1",
        params![vec![2_u8; 32], chunk.sha256(&[2; 32]).unwrap()],
    )
    .unwrap();
    drop(db);
    assert!(recovery.seal_recovered("reject-replacement").is_err());
    assert!(recovery.sealed_manifest().unwrap().is_none());
}
