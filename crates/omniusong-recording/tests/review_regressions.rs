//! Independent journal regressions. Synthetic data only; no devices or legacy data.
use omniusong_core::contracts::{AnchorId, AudioFrame, CapturedEvent, Envelope};
use omniusong_recording::*;
use rusqlite::Connection;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

// A concurrent helper spawn can transiently inherit another test's owner file
// description before close-on-exec runs. Keep fixture lifetimes and the helper
// spawn in this binary disjoint; do not relax production try_lock or retry an
// unexpected Owned result. The child branch creates no Fixture and cannot wait
// on this process-local test scheduling lock.
static FIXTURE_SCHEDULE: Mutex<()> = Mutex::new(());
struct Fixture(PathBuf, #[allow(dead_code)] MutexGuard<'static, ()>);
impl Fixture {
    fn new() -> Self {
        let scheduling_guard = FIXTURE_SCHEDULE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/recording-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root, scheduling_guard)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn db(&self) -> Connection {
        Connection::open(self.path()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Recording review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn clock(ticks: u64) -> HostTimestamp {
    HostTimestamp {
        domain: ClockDomainId("mach-continuous".into()),
        epoch: ClockEpochId("host-epoch".into()),
        ticks,
    }
}
fn descriptor() -> TakeDescriptor {
    let host_clock = HostClock {
        domain: clock(0).domain,
        epoch: clock(0).epoch,
        ticks_per_second_numerator: 24_000_000,
        ticks_per_second_denominator: 1,
    };
    let stream = |stream_id: &str, kind| StreamDescriptor {
        stream_id: stream_id.into(),
        segment_id: format!("{stream_id}-segment"),
        source_id: format!("{stream_id}-source"),
        source_name: format!("Synthetic {stream_id}"),
        source_connection_generation: 17,
        host_clock: host_clock.clone(),
        kind,
        initial_source_position: 0,
    };
    let precision = serde_json::from_str(r#"{"integer":1234567890123456789012345678901234567890,"decimal":0.123456789012345678901234567890123456789,"vendor":{"assetId":"opaque-vendor-reference"}}"#).unwrap();
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId("review-take".into()),
        streams: vec![
            stream(
                "audio",
                StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 2,
                        encoding: PcmEncoding::S16Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("audio-epoch".into()),
                },
            ),
            stream(
                "events",
                StreamKind::RawEvents {
                    format: RawEventFormat::LengthPrefixedCaptureV1,
                },
            ),
        ],
        provenance: BTreeMap::from([("vendor".into(), precision)]),
    }
}
fn header(
    take: &TakeDescriptor,
    index: usize,
    sequence: u64,
    start: u64,
    count: u64,
) -> ChunkHeader {
    let stream = &take.streams[index];
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: take.take_id.clone(),
        stream_id: stream.stream_id.clone(),
        segment_id: stream.segment_id.clone(),
        sequence,
        range: if index == 0 {
            SourceRange::AudioFrames { start, count }
        } else {
            SourceRange::Events { start, count }
        },
        interpretation: stream.kind.clone(),
        observed_start: clock(1000),
        observed_end: clock(9000),
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
fn raw_record(sequence: u64) -> Vec<u8> {
    let value = json!({
        "protocol":"omniusong.capture", "version":1, "session_id":"capture-session", "message_id":format!("capture-{sequence}"),
        "sequence":sequence, "vendor":{"assetId":"opaque-not-native-asset"},
        "payload":{
            "event_id":format!("event-{sequence}"),
            "source":{"endpoint":"events-source","connection_generation":17,"group":0,"channel":0},
            "time":{"observed":{"domain":"mach-continuous","epoch":"host-epoch","ticks":2000},"alignment":null},
            "raw":{"format":"ump","data":{"words":[1083194368_u32,2147483649_u32]}},
            "musical":{"event":"uninterpreted"}
        }
    });
    let mut envelope: Envelope<CapturedEvent> = serde_json::from_value(value).unwrap();
    envelope.payload.extensions.insert("precision".into(), serde_json::from_str(r#"{"integer":123456789012345678901234567890,"decimal":0.12345678901234567890123456789}"#).unwrap());
    // Deliberately preserve pretty JSON and trailing whitespace byte-for-byte.
    let mut bytes = serde_json::to_vec_pretty(&envelope).unwrap();
    bytes.extend_from_slice(b"\n \t");
    Envelope::<CapturedEvent>::decode(&bytes).unwrap();
    bytes
}
fn chunk_count(db: &Connection) -> u64 {
    let count: i64 = db
        .query_row("SELECT count(*) FROM chunks", [], |row| row.get(0))
        .unwrap();
    u64::try_from(count).unwrap()
}

#[test]
fn immutable_original_bytes_headers_clocks_and_opaque_precision_survive_seal_reopen() {
    let fixture = Fixture::new();
    let take = descriptor();
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let mut audio = vec![0x12, 0x80, 0x34, 0x7f, 0x56, 0x00, 0x78, 0xff];
    let original_audio = audio.clone();
    let mut audio_header = header(&take, 0, 0, 0, 2);
    audio_header.clock_anchors.push(ClockAnchor {
        id: AnchorId("anchor-1".into()),
        host: clock(1000),
        audio: AudioFrame {
            epoch: ClockEpochId("audio-epoch".into()),
            sample_rate: 48_000,
            frame: -17,
        },
        ticks_per_second_numerator: 24_000_000,
        ticks_per_second_denominator: 1,
        uncertainty_frames: 2,
        extensions: BTreeMap::new(),
    });
    let receipt = journal.append(&audio_header, &audio).unwrap();
    audio.fill(0); // Caller storage must never remain an original's backing buffer.
    assert_eq!(
        journal.read_chunk("audio", 0).unwrap().payload,
        original_audio
    );
    let raw = raw_record(0);
    let payload = encode_event_chunk(&[&raw]).unwrap();
    let event_header = header(&take, 1, 0, 0, 1);
    journal.append(&event_header, &payload).unwrap();
    assert_eq!(&payload[4..], raw);
    let mut different_header = audio_header.clone();
    different_header.observed_end.ticks += 1;
    assert_ne!(
        different_header.sha256(&original_audio).unwrap(),
        receipt.sha256
    );
    assert!(matches!(
        journal.append(&different_header, &original_audio),
        Err(RecordingError::Conflict(_))
    ));
    let manifest = journal.seal("seal-original").unwrap();
    manifest.verify_hash().unwrap();
    assert_eq!(journal.seal("seal-original").unwrap(), manifest);
    assert!(matches!(
        journal.seal("conflicting-seal"),
        Err(RecordingError::Conflict(_))
    ));
    drop(journal);
    let journal = Journal::open(fixture.path()).unwrap();
    assert_eq!(journal.descriptor(), &take);
    assert_eq!(journal.read_chunk("audio", 0).unwrap().header, audio_header);
    assert_eq!(
        journal.read_chunk("audio", 0).unwrap().payload,
        original_audio
    );
    assert_eq!(journal.read_chunk("events", 0).unwrap().payload, payload);
    assert_eq!(journal.sealed_manifest().unwrap(), Some(manifest));
    assert!(!journal.recovery_report().unwrap().unknown_tail);
    let encoded = serde_json::to_string(journal.descriptor()).unwrap();
    assert!(encoded.contains("1234567890123456789012345678901234567890"));
    assert!(encoded.contains("0.123456789012345678901234567890123456789"));
}

#[test]
fn skipped_wrapped_conflicting_and_misinterpreted_chunks_publish_nothing() {
    let fixture = Fixture::new();
    let take = descriptor();
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let first = header(&take, 0, 0, 0, 1);
    let original = [1, 2, 3, 4];
    journal.append(&first, &original).unwrap();
    let mut bad_headers = vec![
        header(&take, 0, 2, 1, 1),
        header(&take, 0, u64::MAX, 1, 1),
        header(&take, 0, 1, 2, 1),
    ];
    let mut wrong_version = header(&take, 0, 1, 1, 1);
    wrong_version.version += 1;
    bad_headers.push(wrong_version);
    let mut wrong_clock = header(&take, 0, 1, 1, 1);
    wrong_clock.observed_end.epoch = ClockEpochId("other-epoch".into());
    bad_headers.push(wrong_clock);
    let mut wrong_format = header(&take, 0, 1, 1, 1);
    if let StreamKind::Audio { format, .. } = &mut wrong_format.interpretation {
        format.layout = PcmLayout::Planar;
    }
    bad_headers.push(wrong_format);
    for invalid in bad_headers {
        assert!(journal.append(&invalid, &original).is_err());
    }
    assert!(
        journal
            .append(&header(&take, 0, 1, 1, 2), &original)
            .is_err()
    );
    assert!(matches!(
        journal.append(&first, &[4, 3, 2, 1]),
        Err(RecordingError::Conflict(_))
    ));
    assert!(
        journal
            .append(
                &header(&take, 0, 1, 1, 1),
                &vec![0; MAX_AUDIO_CHUNK_BYTES + 1]
            )
            .is_err()
    );
    assert_eq!(chunk_count(&fixture.db()), 1);
    assert!(journal.append(&first, &original).unwrap().replayed);
    let mut gap = header(&take, 0, 1, 1, 7);
    gap.kind = ChunkKind::Gap {
        reason: GapReason::DeviceLost,
    };
    assert!(journal.append(&gap, &original).is_err());
    journal.append(&gap, &[]).unwrap();
    journal
        .append(&header(&take, 0, 2, 8, 1), &original)
        .unwrap();
    let manifest = journal.seal("explicit-gap").unwrap();
    assert_eq!(
        manifest.body.streams[0].source_end,
        SourcePosition::AudioFrames(9)
    );
    assert_eq!(manifest.body.streams[0].recorded_units, 2);
    assert_eq!(manifest.body.streams[0].gap_units, 7);
    assert_eq!(manifest.body.streams[0].gap_chunks, 1);
}

#[test]
fn raw_event_source_generation_clock_count_and_truncation_are_admission_guards() {
    let fixture = Fixture::new();
    let take = descriptor();
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let raw = raw_record(0);
    let original = encode_event_chunk(&[&raw]).unwrap();
    let expected = header(&take, 1, 0, 0, 1);
    for field in ["source", "generation", "clock"] {
        let mut record: Envelope<CapturedEvent> = Envelope::decode(&raw).unwrap();
        match field {
            "source" => record.payload.source.endpoint.0 = "different-source".into(),
            "generation" => record.payload.source.connection_generation += 1,
            _ => record.payload.time.observed.epoch = ClockEpochId("different-clock".into()),
        }
        let encoded = record.encode().unwrap();
        let payload = encode_event_chunk(&[&encoded]).unwrap();
        assert!(journal.append(&expected, &payload).is_err());
    }
    let mut truncated = original.clone();
    truncated.pop();
    assert!(journal.append(&expected, &truncated).is_err());
    assert!(
        journal
            .append(&header(&take, 1, 0, 0, 2), &original)
            .is_err()
    );
    assert_eq!(chunk_count(&fixture.db()), 0);
    journal.append(&expected, &original).unwrap();
    assert_eq!(journal.read_chunk("events", 0).unwrap().payload, original);
}

#[test]
fn ordinary_and_symlink_second_owners_cannot_interrupt_a_live_writer() {
    let fixture = Fixture::new();
    let take = descriptor();
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    assert!(matches!(
        Journal::open(fixture.path()),
        Err(RecordingError::Owned)
    ));
    #[cfg(unix)]
    {
        let alias = fixture.0.join("alias.sqlite3");
        std::os::unix::fs::symlink(fixture.path(), &alias).unwrap();
        assert!(matches!(Journal::open(alias), Err(RecordingError::Owned)));
    }
    assert_eq!(journal.state().unwrap(), TakeState::Recording);
    journal
        .append(&header(&take, 0, 0, 0, 1), &[1, 2, 3, 4])
        .unwrap();
    drop(journal);
    assert_eq!(
        Journal::open(fixture.path()).unwrap().state().unwrap(),
        TakeState::Interrupted
    );
}

#[cfg(unix)]
#[test]
fn hardlink_alias_is_rejected_without_interrupting_or_changing_original_data() {
    let fixture = Fixture::new();
    let take = descriptor();
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    journal
        .append(&header(&take, 0, 0, 0, 1), &[1, 2, 3, 4])
        .unwrap();
    let alias = fixture.0.join("hardlink.sqlite3");
    fs::hard_link(fixture.path(), &alias).unwrap();
    assert!(matches!(
        Journal::open(&alias),
        Err(RecordingError::Invalid(_))
    ));
    assert!(matches!(
        Journal::open(fixture.path()),
        Err(RecordingError::Invalid(_))
    ));
    assert_eq!(journal.state().unwrap(), TakeState::Recording);
    assert_eq!(
        journal.read_chunk("audio", 0).unwrap().payload,
        [1, 2, 3, 4]
    );
    journal
        .append(&header(&take, 0, 1, 1, 1), &[5, 6, 7, 8])
        .unwrap();
    fs::remove_file(alias).unwrap();
    journal.seal("single-link-again").unwrap();
    drop(journal);
    assert_eq!(
        Journal::open(fixture.path())
            .unwrap()
            .recovery_report()
            .unwrap()
            .verified_chunks,
        2
    );
}

#[test]
fn damaged_tail_recovery_seals_only_proven_prefix_and_retains_original_bad_rows() {
    let fixture = Fixture::new();
    let take = descriptor();
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let first = header(&take, 0, 0, 0, 1);
    let second = header(&take, 0, 1, 1, 1);
    journal.append(&first, &[1, 2, 3, 4]).unwrap();
    journal.append(&second, &[5, 6, 7, 8]).unwrap();
    drop(journal);
    let db = fixture.db();
    assert!(
        db.execute("UPDATE chunks SET payload=x'00000000' WHERE sequence=1", [])
            .is_err()
    );
    db.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'00000000' WHERE sequence=1;").unwrap();
    drop(db);
    let mut journal = Journal::open(fixture.path()).unwrap();
    let report = journal.recovery_report().unwrap();
    assert_eq!(report.state, TakeState::Interrupted);
    assert_eq!(report.verified_chunks, 1);
    assert!(report.unknown_tail);
    assert!(!report.problems.is_empty());
    assert!(journal.seal("wrong-mode").is_err());
    assert!(matches!(
        journal.append(&header(&take, 0, 2, 2, 1), &[9, 10, 11, 12]),
        Err(RecordingError::Interrupted)
    ));
    assert!(journal.append(&first, &[1, 2, 3, 4]).unwrap().replayed);
    assert!(journal.append(&second, &[5, 6, 7, 8]).is_err());
    let manifest = journal.seal_recovered("explicit-recovery").unwrap();
    assert_eq!(manifest.body.chunk_count, 1);
    let interrupted = manifest.body.interrupted.as_ref().unwrap();
    assert_eq!(interrupted.report, report);
    assert_eq!(chunk_count(&fixture.db()), 2);
    drop(journal);
    let journal = Journal::open(fixture.path()).unwrap();
    assert_eq!(journal.sealed_manifest().unwrap(), Some(manifest));
    assert!(journal.recovery_report().unwrap().unknown_tail);
}

#[test]
fn interruption_receipt_rejects_rehashed_replacement_in_full_and_shortened_prefixes() {
    for shortened in [false, true] {
        let fixture = Fixture::new();
        let take = descriptor();
        let mut journal = Journal::create(fixture.path(), &take).unwrap();
        for sequence in 0..3 {
            journal
                .append(&header(&take, 0, sequence, sequence, 1), &[1, 2, 3, 4])
                .unwrap();
        }
        drop(journal);
        let interrupted = Journal::open(fixture.path()).unwrap();
        assert_eq!(interrupted.recovery_report().unwrap().verified_chunks, 3);
        drop(interrupted);
        let db = fixture.db();
        let bytes: Vec<u8> = db
            .query_row("SELECT header FROM chunks WHERE sequence=0", [], |row| {
                row.get(0)
            })
            .unwrap();
        let original_header: ChunkHeader = serde_json::from_slice(&bytes).unwrap();
        let replacement = [9, 8, 7, 6];
        let replacement_hash = original_header.sha256(&replacement).unwrap();
        db.execute_batch("DROP TRIGGER immutable_chunk_update;")
            .unwrap();
        db.execute(
            "UPDATE chunks SET payload=?1,sha256=?2 WHERE sequence=0",
            rusqlite::params![replacement.as_slice(), replacement_hash],
        )
        .unwrap();
        if shortened {
            db.execute("UPDATE chunks SET payload=x'00000000' WHERE sequence=2", [])
                .unwrap();
        }
        drop(db);
        let mut journal = Journal::open(fixture.path()).unwrap();
        assert_eq!(
            journal.recovery_report().unwrap().verified_chunks,
            if shortened { 2 } else { 3 }
        );
        assert!(journal.seal_recovered("reject-rewritten-prefix").is_err());
        assert!(journal.sealed_manifest().unwrap().is_none());
        assert_eq!(journal.state().unwrap(), TakeState::Interrupted);
        assert_eq!(chunk_count(&fixture.db()), 3);
    }
}

#[test]
fn tampered_sealed_payload_header_and_manifest_are_rejected_on_fresh_open() {
    for target in ["payload", "header", "manifest"] {
        let fixture = Fixture::new();
        let take = descriptor();
        let mut journal = Journal::create(fixture.path(), &take).unwrap();
        journal
            .append(&header(&take, 0, 0, 0, 1), &[1, 2, 3, 4])
            .unwrap();
        journal.seal("seal").unwrap();
        drop(journal);
        let db = fixture.db();
        match target {
            "payload" => db
                .execute_batch(
                    "DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'04030201';",
                )
                .unwrap(),
            "header" => {
                let bytes: Vec<u8> = db
                    .query_row("SELECT header FROM chunks", [], |row| row.get(0))
                    .unwrap();
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                value["observed_end"]["ticks"] = json!(9001);
                db.execute_batch("DROP TRIGGER immutable_chunk_update;")
                    .unwrap();
                db.execute(
                    "UPDATE chunks SET header=?1",
                    [serde_json::to_vec(&value).unwrap()],
                )
                .unwrap();
            }
            _ => {
                let bytes: Vec<u8> = db
                    .query_row("SELECT body FROM sealed_manifests", [], |row| row.get(0))
                    .unwrap();
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                value["body"]["payload_bytes"] = json!(8);
                db.execute_batch("DROP TRIGGER immutable_manifest_update;")
                    .unwrap();
                db.execute(
                    "UPDATE sealed_manifests SET body=?1",
                    [serde_json::to_vec(&value).unwrap()],
                )
                .unwrap();
            }
        }
        drop(db);
        assert!(
            Journal::open(fixture.path()).is_err(),
            "accepted {target} tampering"
        );
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn before_commit_faults_roll_back_and_after_commit_lost_ack_retries_are_idempotent() {
    for boundary in [
        WriteBoundary::AfterChunkInsert,
        WriteBoundary::BeforeChunkCommit,
        WriteBoundary::AfterChunkCommit,
    ] {
        let fixture = Fixture::new();
        let take = descriptor();
        let mut journal = Journal::create(fixture.path(), &take).unwrap();
        journal.set_boundary_hook(move |hit| {
            if hit == boundary {
                Err(RecordingError::Invalid("isolated injected fault".into()))
            } else {
                Ok(())
            }
        });
        let first = header(&take, 0, 0, 0, 1);
        assert!(journal.append(&first, &[1, 2, 3, 4]).is_err());
        journal.clear_boundary_hook();
        let committed = boundary == WriteBoundary::AfterChunkCommit;
        assert_eq!(chunk_count(&fixture.db()), u64::from(committed));
        assert_eq!(
            journal.recovery_report().unwrap().streams[0].next_sequence,
            u64::from(committed)
        );
        assert_eq!(
            journal.append(&first, &[1, 2, 3, 4]).unwrap().replayed,
            committed
        );
    }
    for boundary in [
        WriteBoundary::AfterManifestInsert,
        WriteBoundary::BeforeSealCommit,
        WriteBoundary::AfterSealCommit,
    ] {
        let fixture = Fixture::new();
        let take = descriptor();
        let mut journal = Journal::create(fixture.path(), &take).unwrap();
        journal
            .append(&header(&take, 0, 0, 0, 1), &[1, 2, 3, 4])
            .unwrap();
        journal.set_boundary_hook(move |hit| {
            if hit == boundary {
                Err(RecordingError::Invalid("isolated injected fault".into()))
            } else {
                Ok(())
            }
        });
        assert!(journal.seal("seal").is_err());
        journal.clear_boundary_hook();
        let committed = boundary == WriteBoundary::AfterSealCommit;
        assert_eq!(journal.sealed_manifest().unwrap().is_some(), committed);
        assert_eq!(
            journal.state().unwrap(),
            if committed {
                TakeState::Sealed
            } else {
                TakeState::Recording
            }
        );
        let manifest = journal.seal("seal").unwrap();
        drop(journal);
        assert_eq!(
            Journal::open(fixture.path())
                .unwrap()
                .sealed_manifest()
                .unwrap(),
            Some(manifest)
        );
    }
}

#[cfg(all(unix, feature = "test-hooks"))]
#[test]
fn owned_helper_death_at_all_six_transaction_boundaries_recovers_only_committed_data() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    const CHILD_PATH: &str = "OMNIUSONG_REVIEW_CHILD_PATH";
    const CHILD_BOUNDARY: &str = "OMNIUSONG_REVIEW_CHILD_BOUNDARY";
    let boundaries = [
        WriteBoundary::AfterChunkInsert,
        WriteBoundary::BeforeChunkCommit,
        WriteBoundary::AfterChunkCommit,
        WriteBoundary::AfterManifestInsert,
        WriteBoundary::BeforeSealCommit,
        WriteBoundary::AfterSealCommit,
    ];
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let selected: usize = std::env::var(CHILD_BOUNDARY).unwrap().parse().unwrap();
        let boundary = boundaries[selected];
        let take = descriptor();
        let mut journal = Journal::create(path, &take).unwrap();
        journal
            .append(&header(&take, 0, 0, 0, 1), &[1, 2, 3, 4])
            .unwrap();
        journal.set_boundary_hook(move |hit| {
            if hit == boundary {
                println!("OMNIUSONG_REVIEW_BOUNDARY_READY");
                std::io::stdout().flush().unwrap();
                loop {
                    std::thread::park();
                }
            }
            Ok(())
        });
        if selected < 3 {
            journal
                .append(&header(&take, 0, 1, 1, 1), &[5, 6, 7, 8])
                .unwrap();
        } else {
            journal.seal("seal-child").unwrap();
        }
        panic!("Helper failed to reach owned test boundary");
    }
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
    for (selected, boundary) in boundaries.into_iter().enumerate() {
        let fixture = Fixture::new();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "owned_helper_death_at_all_six_transaction_boundaries_recovers_only_committed_data",
                "--nocapture",
            ])
            .env(CHILD_PATH, fixture.path())
            .env(CHILD_BOUNDARY, selected.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = OwnedChild(child);
        let stdout = child.0.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        let output = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                if line.contains("OMNIUSONG_REVIEW_BOUNDARY_READY") {
                    let _ = send.send(());
                    break;
                }
            }
        });
        if let Err(error) = receive.recv_timeout(Duration::from_secs(10)) {
            let status = child.0.try_wait().unwrap();
            panic!("Owned helper did not reach {boundary:?}: {error}; child state {status:?}");
        }
        assert!(child.0.try_wait().unwrap().is_none());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        output.join().unwrap();
        let journal = Journal::open(fixture.path()).unwrap();
        let expected_count = if boundary == WriteBoundary::AfterChunkCommit {
            2
        } else {
            1
        };
        let report = journal.recovery_report().unwrap();
        assert_eq!(report.verified_chunks, expected_count);
        assert_eq!(
            journal.read_chunk("audio", 0).unwrap().payload,
            [1, 2, 3, 4]
        );
        if boundary == WriteBoundary::AfterSealCommit {
            assert_eq!(journal.state().unwrap(), TakeState::Sealed);
            assert!(!report.unknown_tail);
            assert_eq!(
                journal
                    .sealed_manifest()
                    .unwrap()
                    .unwrap()
                    .body
                    .operation_id,
                "seal-child"
            );
        } else {
            assert_eq!(journal.state().unwrap(), TakeState::Interrupted);
            assert!(report.unknown_tail);
            assert!(journal.sealed_manifest().unwrap().is_none());
        }
    }
}
