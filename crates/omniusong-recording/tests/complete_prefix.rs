//! Trusted storage primitive tests, not native Finish/lifetime/seal authority.
//! Every original is tiny synthetic PCM; no input/output device is opened.
use omniusong_recording::*;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/complete-prefix-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn sql(&self) -> Connection {
        Connection::open(self.path()).unwrap()
    }
    fn originals(&self) -> Vec<(i64, Vec<u8>, Vec<u8>, String)> {
        let connection =
            Connection::open_with_flags(self.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut statement = connection
            .prepare("SELECT ordinal,header,payload,sha256 FROM chunks ORDER BY ordinal")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
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
fn descriptor(provenance: BTreeMap<String, Value>) -> TakeDescriptor {
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: (0..2)
            .map(|index| StreamDescriptor {
                stream_id: format!("audio-{index}"),
                segment_id: format!("segment-{index}"),
                source_id: format!("synthetic-{index}"),
                source_name: "Synthetic trusted storage fixture; not a native performance".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("software-frames".into()),
                    epoch: ClockEpochId("synthetic-epoch".into()),
                    ticks_per_second_numerator: 48_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 1,
                        encoding: PcmEncoding::F32Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("synthetic-audio".into()),
                },
                initial_source_position: 100,
            })
            .collect(),
        provenance,
    }
}
fn v2() -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("omniusong.performance.v2".into(), json!({"version":2,"config":{"synthetic_storage_fixture":true}})),
        ("opaque".into(), serde_json::from_str(r#"{"integer":123456789012345678901234567890,"decimal":0.12345678901234567890123456789}"#).unwrap()),
    ])
}
fn header(take: &TakeDescriptor, stream: usize, sequence: u64) -> ChunkHeader {
    let stream = &take.streams[stream];
    let stamp = HostTimestamp {
        domain: stream.host_clock.domain.clone(),
        epoch: stream.host_clock.epoch.clone(),
        ticks: sequence * 2,
    };
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: take.take_id.clone(),
        stream_id: stream.stream_id.clone(),
        segment_id: stream.segment_id.clone(),
        sequence,
        range: SourceRange::AudioFrames {
            start: 100 + sequence * 2,
            count: 2,
        },
        interpretation: stream.kind.clone(),
        observed_start: stamp.clone(),
        observed_end: stamp,
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
fn populated(fixture: &Fixture, provenance: BTreeMap<String, Value>) -> Journal {
    let take = descriptor(provenance);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    for sequence in 0..2 {
        for stream in 0..2 {
            journal
                .append(
                    &header(&take, stream, sequence),
                    &[0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat(),
                )
                .unwrap();
        }
    }
    journal
}
fn no_manifest(fixture: &Fixture, journal: &Journal) {
    assert_eq!(journal.state().unwrap(), TakeState::Recording);
    assert!(journal.sealed_manifest().unwrap().is_none());
    let count: i64 = fixture
        .sql()
        .query_row("SELECT count(*) FROM sealed_manifests", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn full_normal_prefix_preserves_exact_originals_and_replays_after_fresh_reopen() {
    let fixture = Fixture::new();
    let mut journal = populated(&fixture, v2());
    let original_rows = fixture.originals();
    let original_file = format!("{:x}", Sha256::digest(fs::read(fixture.path()).unwrap()));
    let complete = journal.complete_prefix().unwrap();
    assert_eq!(complete.prefix.chunk_count, 4);
    assert_eq!(
        complete.descriptor_sha256,
        journal.descriptor().sha256().unwrap()
    );
    assert_eq!(
        original_file,
        format!("{:x}", Sha256::digest(fs::read(fixture.path()).unwrap()))
    );
    assert!(journal.seal("generic-forbidden").is_err());
    no_manifest(&fixture, &journal);
    let manifest = journal
        .seal_complete_prefix("normal-full-prefix", &complete)
        .unwrap();
    assert_eq!(manifest.body.chunk_count, 4);
    assert!(manifest.body.interrupted.is_none());
    assert_eq!(manifest.body.descriptor, *journal.descriptor());
    assert!(
        manifest
            .body
            .streams
            .iter()
            .all(|stream| stream.recorded_units == 4
                && stream.gap_units == 0
                && stream.gap_chunks == 0)
    );
    assert_eq!(journal.complete_prefix().unwrap(), complete);
    assert_eq!(
        journal
            .seal_complete_prefix("normal-full-prefix", &complete)
            .unwrap(),
        manifest
    );
    assert!(matches!(
        journal.seal_complete_prefix("different-operation", &complete),
        Err(RecordingError::Conflict(_))
    ));
    assert_eq!(fixture.originals(), original_rows);
    drop(journal);
    let mut reopened = Journal::open(fixture.path()).unwrap();
    assert_eq!(reopened.state().unwrap(), TakeState::Sealed);
    assert_eq!(reopened.complete_prefix().unwrap(), complete);
    assert_eq!(
        reopened
            .seal_complete_prefix("normal-full-prefix", &complete)
            .unwrap(),
        manifest
    );
    assert_eq!(fixture.originals(), original_rows);
}

#[test]
fn strict_complete_receipt_rejects_mutation_empty_and_shorter_prefix_without_publication() {
    let fixture = Fixture::new();
    let mut journal = populated(&fixture, v2());
    let complete = journal.complete_prefix().unwrap();
    let original_rows = fixture.originals();
    let mut serialized = serde_json::to_value(&complete).unwrap();
    serialized
        .as_object_mut()
        .unwrap()
        .insert("native_authority".into(), Value::Bool(true));
    assert!(serde_json::from_value::<CompletePrefix>(serialized).is_err());
    for case in 0..7 {
        let mut changed = complete.clone();
        match case {
            0 => changed.descriptor_sha256 = "0".repeat(64),
            1 => changed.original_rows_sha256 = "0".repeat(64),
            2 => changed.prefix.chunk_index_sha256 = "0".repeat(64),
            3 => changed.prefix.chunk_count = 0,
            4 => {
                // A correctly hashed shorter original prefix still cannot be
                // selected through the normal-completion API.
                changed.prefix.chunk_count = 3;
                let mut hash = Sha256::new();
                hash.update(b"OmniuSong.recording-index.v1\0");
                hash.update(complete.descriptor_sha256.as_bytes());
                for (ordinal, _, _, sha256) in original_rows.iter().take(3) {
                    hash.update((*ordinal as u64).to_le_bytes());
                    hash.update(sha256.as_bytes());
                }
                changed.prefix.chunk_index_sha256 = format!("{:x}", hash.finalize());
            }
            5 => changed.prefix.chunk_count = 5,
            _ => changed.original_rows_sha256 = "A".repeat(64),
        }
        assert!(
            journal
                .seal_complete_prefix("changed-receipt", &changed)
                .is_err(),
            "case {case}"
        );
        no_manifest(&fixture, &journal);
        assert_eq!(journal.complete_prefix().unwrap(), complete);
        assert_eq!(fixture.originals(), original_rows);
    }
    // The same full receipt also remains bound after normal publication.
    journal
        .seal_complete_prefix("unchanged", &complete)
        .unwrap();
    let mut changed = complete.clone();
    changed.original_rows_sha256 = "0".repeat(64);
    assert!(journal.seal_complete_prefix("unchanged", &changed).is_err());
}

#[test]
fn new_chunks_or_opaque_original_column_changes_invalidate_an_admitted_receipt() {
    for opaque in [false, true] {
        let fixture = Fixture::new();
        let mut journal = populated(&fixture, v2());
        if opaque {
            fixture.sql().execute_batch("DROP TRIGGER immutable_chunk_update; ALTER TABLE chunks ADD COLUMN opaque_original TEXT; UPDATE chunks SET opaque_original='original';").unwrap();
        }
        let complete = journal.complete_prefix().unwrap();
        if opaque {
            fixture
                .sql()
                .execute(
                    "UPDATE chunks SET opaque_original='changed' WHERE ordinal=4",
                    [],
                )
                .unwrap();
        } else {
            let take = journal.descriptor().clone();
            journal
                .append(
                    &header(&take, 0, 2),
                    &[0.5f32.to_le_bytes(), 0.75f32.to_le_bytes()].concat(),
                )
                .unwrap();
        }
        let fresh = journal.complete_prefix().unwrap();
        assert_ne!(fresh.original_rows_sha256, complete.original_rows_sha256);
        if opaque {
            assert_eq!(fresh.prefix, complete.prefix);
        }
        assert!(journal.seal_complete_prefix("stale", &complete).is_err());
        no_manifest(&fixture, &journal);
        journal.seal_complete_prefix("fresh", &fresh).unwrap();
        if opaque {
            fixture
                .sql()
                .execute(
                    "UPDATE chunks SET opaque_original='changed after seal' WHERE ordinal=4",
                    [],
                )
                .unwrap();
            assert!(
                journal.seal_complete_prefix("fresh", &fresh).is_err(),
                "Exact operation replay must still authenticate every original column"
            );
        }
    }
}

#[test]
fn gaps_bad_tail_and_extra_or_inconsistent_stream_prefixes_cannot_complete() {
    for case in 0..5 {
        let fixture = Fixture::new();
        let mut journal = populated(&fixture, v2());
        let complete = journal.complete_prefix().unwrap();
        match case {
            0 => {
                let mut gap = header(journal.descriptor(), 0, 2);
                gap.kind = ChunkKind::Gap {
                    reason: GapReason::TransferOverflow,
                };
                journal.append(&gap, &[]).unwrap();
            }
            1 => {
                fixture.sql().execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=4;").unwrap();
            }
            2 => {
                fixture
                    .sql()
                    .execute_batch("INSERT INTO stream_prefix VALUES('unexpected',0,0,0,0,0);")
                    .unwrap();
            }
            3 => {
                fixture
                    .sql()
                    .execute_batch(
                        "UPDATE stream_prefix SET recorded_units=3 WHERE stream_id='audio-0';",
                    )
                    .unwrap();
            }
            _ => {
                fixture
                    .sql()
                    .execute_batch("DELETE FROM stream_prefix WHERE stream_id='audio-0';")
                    .unwrap();
            }
        }
        assert!(journal.complete_prefix().is_err(), "case {case}");
        assert!(
            journal.seal_complete_prefix("damaged", &complete).is_err(),
            "case {case}"
        );
        no_manifest(&fixture, &journal);
    }
}

#[test]
fn rehashed_descriptor_replacement_cannot_rebind_a_live_journal_or_receipt() {
    let fixture = Fixture::new();
    let mut journal = populated(&fixture, v2());
    let complete = journal.complete_prefix().unwrap();
    let mut changed = journal.descriptor().clone();
    changed.streams[0].source_name = "A different synthetic source".into();
    let connection = fixture.sql();
    connection
        .execute_batch("DROP TRIGGER immutable_descriptor;")
        .unwrap();
    connection
        .execute(
            "UPDATE take_meta SET descriptor=?1,descriptor_sha256=?2",
            params![
                serde_json::to_vec(&changed).unwrap(),
                changed.sha256().unwrap()
            ],
        )
        .unwrap();
    assert!(journal.complete_prefix().is_err());
    assert!(journal.seal_complete_prefix("rebound", &complete).is_err());
    no_manifest(&fixture, &journal);
}

#[test]
fn only_supported_owned_v2_configuration_admits_checked_normal_completion() {
    let invalid = [
        BTreeMap::new(),
        BTreeMap::from([("omniusong.performance.v1".into(), Value::Null)]),
        BTreeMap::from([("omniusong.performance.v2".into(), Value::Null)]),
        BTreeMap::from([(
            "omniusong.performance.v2".into(),
            json!({"version":1,"config":{}}),
        )]),
        BTreeMap::from([(
            "omniusong.performance.v2".into(),
            json!({"version":2,"config":null}),
        )]),
        BTreeMap::from([(
            "omniusong.performance.v2".into(),
            json!({"version":2,"config":{},"extra":true}),
        )]),
        BTreeMap::from([
            (
                "omniusong.performance.v2".into(),
                json!({"version":2,"config":{}}),
            ),
            ("omniusong.performance.v1".into(), Value::Null),
        ]),
        BTreeMap::from([
            (
                "omniusong.performance.v2".into(),
                json!({"version":2,"config":{}}),
            ),
            ("omniusong.performance.v3".into(), Value::Null),
        ]),
        BTreeMap::from([(
            "omniusong.performance.v3".into(),
            json!({"version":3,"config":{}}),
        )]),
    ];
    for provenance in invalid {
        let fixture = Fixture::new();
        let mut journal = populated(&fixture, provenance);
        assert!(journal.complete_prefix().is_err());
        let forged = CompletePrefix {
            descriptor_sha256: journal.descriptor().sha256().unwrap(),
            original_rows_sha256: "0".repeat(64),
            prefix: RecoveryPrefix {
                chunk_count: 4,
                chunk_index_sha256: journal.recovery_report().unwrap().chunk_index_sha256,
            },
        };
        assert!(
            journal
                .seal_complete_prefix("unsupported", &forged)
                .is_err()
        );
        no_manifest(&fixture, &journal);
    }
    // Existing generic nonperformance storage is still usable through its API.
    let fixture = Fixture::new();
    let mut journal = populated(&fixture, BTreeMap::new());
    assert!(
        journal
            .seal("generic-still-valid")
            .unwrap()
            .body
            .interrupted
            .is_none()
    );
}

#[test]
fn interrupted_or_selected_recovered_original_never_becomes_normal() {
    for recovered in [false, true] {
        let fixture = Fixture::new();
        let journal = populated(&fixture, v2());
        let complete = journal.complete_prefix().unwrap();
        let originals = fixture.originals();
        drop(journal);
        if recovered {
            let reader = CaptureReader::open(fixture.path()).unwrap();
            let (manifest, replayed) = reader
                .recover_prefix("selected-recovered", &complete.prefix)
                .unwrap();
            assert!(!replayed);
            assert!(manifest.body.interrupted.is_some());
        }
        let mut journal = Journal::open(fixture.path()).unwrap();
        let before = journal.sealed_manifest().unwrap();
        assert!(journal.complete_prefix().is_err());
        assert!(
            journal
                .seal_complete_prefix("no-promotion", &complete)
                .is_err()
        );
        assert_eq!(journal.sealed_manifest().unwrap(), before);
        assert_eq!(fixture.originals(), originals);
    }
}

#[test]
fn original_mutation_by_a_seal_trigger_is_detected_and_the_whole_transaction_rolls_back() {
    let fixture = Fixture::new();
    let mut journal = populated(&fixture, v2());
    fixture.sql().execute_batch("CREATE TRIGGER injected_seal_mutation AFTER INSERT ON sealed_manifests BEGIN UPDATE stream_prefix SET recorded_units=recorded_units+1 WHERE stream_id='audio-0'; END;").unwrap();
    let complete = journal.complete_prefix().unwrap();
    let originals = fixture.originals();
    assert!(
        journal
            .seal_complete_prefix("trigger-rollback", &complete)
            .is_err()
    );
    no_manifest(&fixture, &journal);
    assert_eq!(journal.complete_prefix().unwrap(), complete);
    assert_eq!(fixture.originals(), originals);
}

#[cfg(feature = "test-hooks")]
#[test]
fn before_commit_errors_roll_back_and_after_commit_lost_ack_replays_exact_normal_manifest() {
    for boundary in [
        WriteBoundary::AfterManifestInsert,
        WriteBoundary::BeforeSealCommit,
        WriteBoundary::AfterSealCommit,
    ] {
        let fixture = Fixture::new();
        let mut journal = populated(&fixture, v2());
        let complete = journal.complete_prefix().unwrap();
        let originals = fixture.originals();
        journal.set_boundary_hook(move |point| {
            if point == boundary {
                Err(RecordingError::Invalid(
                    "Injected normal completion boundary".into(),
                ))
            } else {
                Ok(())
            }
        });
        assert!(
            journal
                .seal_complete_prefix("stable-normal-intent", &complete)
                .is_err()
        );
        journal.clear_boundary_hook();
        assert_eq!(journal.complete_prefix().unwrap(), complete);
        let retained = journal.sealed_manifest().unwrap();
        if boundary == WriteBoundary::AfterSealCommit {
            assert!(retained.is_some());
        } else {
            no_manifest(&fixture, &journal);
        }
        let manifest = journal
            .seal_complete_prefix("stable-normal-intent", &complete)
            .unwrap();
        if let Some(retained) = retained {
            assert_eq!(manifest, retained);
        }
        assert!(manifest.body.interrupted.is_none());
        assert_eq!(
            journal
                .seal_complete_prefix("stable-normal-intent", &complete)
                .unwrap(),
            manifest
        );
        assert_eq!(fixture.originals(), originals);
        drop(journal);
        let mut journal = Journal::open(fixture.path()).unwrap();
        assert_eq!(
            journal
                .seal_complete_prefix("stable-normal-intent", &complete)
                .unwrap(),
            manifest
        );
    }
}
