//! Independent checks of the trusted full-prefix storage primitive.
//! Synthetic PCM only: these tests supply no native retirement/seal authority.
use omniusong_recording::*;
use rusqlite::{Connection, OpenFlags, params, types::Value as SqlValue};
use serde_json::json;
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/complete-prefix-independent")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }
    fn path(&self) -> PathBuf {
        self.root.join("take.sqlite3")
    }
    fn sql(&self) -> Connection {
        Connection::open(self.path()).unwrap()
    }
    fn readonly(&self) -> Connection {
        Connection::open_with_flags(self.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
    }
    fn populated(&self) -> Journal {
        let take = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            streams: (0..2).map(|index| StreamDescriptor {
                stream_id: format!("pcm-{index}"),
                segment_id: format!("segment-{index}"),
                source_id: format!("software-fixture-{index}"),
                source_name: "Independent storage fixture; no native device".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("fixture-render".into()),
                    epoch: ClockEpochId("fixture-epoch".into()),
                    ticks_per_second_numerator: 48_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat { sample_rate: 48_000, channels: 1,
                        encoding: PcmEncoding::F32Le, layout: PcmLayout::Interleaved },
                    audio_epoch: ClockEpochId("fixture-audio-epoch".into()),
                },
                initial_source_position: 100,
            }).collect(),
            provenance: BTreeMap::from([
                ("omniusong.performance.v2".into(), json!({"version":2,"config":{"storage_fixture":true}})),
                ("opaque-original".into(), serde_json::from_str(r#"{"integer":123456789012345678901234567890,"decimal":0.123456789012345678901234567890,"null":null}"#).unwrap()),
            ]),
        };
        let mut journal = Journal::create(self.path(), &take).unwrap();
        for sequence in 0..2 {
            for stream in &take.streams {
                let stamp = HostTimestamp {
                    domain: stream.host_clock.domain.clone(),
                    epoch: stream.host_clock.epoch.clone(),
                    ticks: sequence * 2,
                };
                let header = ChunkHeader {
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
                };
                journal
                    .append(
                        &header,
                        &[0.25f32.to_le_bytes(), (-0.0f32).to_le_bytes()].concat(),
                    )
                    .unwrap();
            }
        }
        journal
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

type TableRows = Vec<(String, Vec<Vec<SqlValue>>)>;
fn table_rows(connection: &Connection) -> TableRows {
    [
        "take_meta",
        "stream_prefix",
        "chunks",
        "sqlite_sequence",
        "interruptions",
        "recovery_index",
        "sealed_manifests",
    ]
    .into_iter()
    .map(|table| {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|column| row.get::<_, SqlValue>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        (table.to_owned(), rows)
    })
    .collect()
}
fn assert_unpublished(journal: &Journal) {
    assert_eq!(journal.state().unwrap(), TakeState::Recording);
    assert!(journal.sealed_manifest().unwrap().is_none());
}

#[test]
fn final_transaction_rejects_new_recovery_metadata_or_state_reversal_from_its_own_writes() {
    let injections = [
        "CREATE TRIGGER injected_mode AFTER INSERT ON sealed_manifests BEGIN INSERT INTO recovery_index VALUES(1,printf('%064d',0)); END;",
        "CREATE TRIGGER injected_mode AFTER INSERT ON sealed_manifests BEGIN INSERT INTO interruptions VALUES(1,X'00',printf('%064d',0)); END;",
        "CREATE TRIGGER injected_mode AFTER UPDATE OF state ON take_meta WHEN NEW.state='sealed' BEGIN UPDATE take_meta SET state='recording' WHERE singleton=1; END;",
    ];
    for sql in injections {
        let fixture = Fixture::new();
        let mut journal = fixture.populated();
        fixture.sql().execute_batch(sql).unwrap();
        let receipt = journal.complete_prefix().unwrap();
        let before = table_rows(&fixture.readonly());
        let result = journal.seal_complete_prefix("injected-mode-must-roll-back", &receipt);
        assert!(
            result.is_err(),
            "Normal completion committed forbidden derived state: {result:?}"
        );
        assert_unpublished(&journal);
        assert_eq!(
            table_rows(&fixture.readonly()),
            before,
            "All finalization writes must roll back together"
        );
        assert_eq!(journal.complete_prefix().unwrap(), receipt);
    }
}

#[test]
fn rehashed_valid_pcm_replacement_cannot_reuse_the_old_original_receipt() {
    let fixture = Fixture::new();
    let mut journal = fixture.populated();
    let sql = fixture.sql();
    sql.execute_batch("DROP TRIGGER immutable_chunk_update;")
        .unwrap();
    let original = journal.complete_prefix().unwrap();
    let header_bytes: Vec<u8> = sql
        .query_row("SELECT header FROM chunks WHERE ordinal=4", [], |r| {
            r.get(0)
        })
        .unwrap();
    let header: ChunkHeader = serde_json::from_slice(&header_bytes).unwrap();
    let replacement = [(-0.75f32).to_le_bytes(), 0.5f32.to_le_bytes()].concat();
    let replacement_sha = header.sha256(&replacement).unwrap();
    sql.execute(
        "UPDATE chunks SET payload=?1,sha256=?2 WHERE ordinal=4",
        params![replacement, replacement_sha],
    )
    .unwrap();
    let changed = journal.complete_prefix().unwrap();
    assert_eq!(original.descriptor_sha256, changed.descriptor_sha256);
    assert_eq!(original.prefix.chunk_count, changed.prefix.chunk_count);
    assert_ne!(
        original.prefix.chunk_index_sha256,
        changed.prefix.chunk_index_sha256
    );
    assert_ne!(original.original_rows_sha256, changed.original_rows_sha256);
    let before_rejection = table_rows(&fixture.readonly());
    assert!(
        journal
            .seal_complete_prefix("same-finish-id", &original)
            .is_err()
    );
    let mixed = CompletePrefix {
        prefix: changed.prefix.clone(),
        ..original.clone()
    };
    assert!(
        journal
            .seal_complete_prefix("same-finish-id", &mixed)
            .is_err()
    );
    assert_unpublished(&journal);
    assert_eq!(table_rows(&fixture.readonly()), before_rejection);
    let manifest = journal
        .seal_complete_prefix("new-explicit-storage-review", &changed)
        .unwrap();
    assert_eq!(
        manifest.body.chunk_index_sha256,
        changed.prefix.chunk_index_sha256
    );
    assert!(manifest.body.interrupted.is_none());
}

#[test]
fn normalized_original_hash_distinguishes_sql_types_and_sequence_metadata_without_inventing_chunk_changes()
 {
    for target in 0..5 {
        let fixture = Fixture::new();
        let mut journal = fixture.populated();
        let sql = fixture.sql();
        sql.execute_batch("DROP TRIGGER immutable_chunk_update; ALTER TABLE chunks ADD COLUMN opaque_original; UPDATE chunks SET opaque_original=CAST('42' AS TEXT);").unwrap();
        let original = journal.complete_prefix().unwrap();
        match target {
            0 => {
                sql.execute(
                    "UPDATE chunks SET opaque_original=?1 WHERE ordinal=4",
                    [SqlValue::Blob(b"42".to_vec())],
                )
                .unwrap();
            }
            1 => {
                sql.execute(
                    "UPDATE chunks SET opaque_original=?1 WHERE ordinal=4",
                    [SqlValue::Integer(42)],
                )
                .unwrap();
            }
            2 => {
                sql.execute(
                    "UPDATE chunks SET opaque_original=?1 WHERE ordinal=4",
                    [SqlValue::Real(42.0)],
                )
                .unwrap();
            }
            3 => {
                sql.execute("UPDATE chunks SET opaque_original=NULL WHERE ordinal=4", [])
                    .unwrap();
            }
            _ => {
                sql.execute(
                    "UPDATE sqlite_sequence SET seq=seq+1 WHERE name='chunks'",
                    [],
                )
                .unwrap();
            }
        }
        let changed = journal.complete_prefix().unwrap();
        assert_eq!(original.descriptor_sha256, changed.descriptor_sha256);
        assert_eq!(
            original.prefix, changed.prefix,
            "Case {target} must isolate original-row authentication"
        );
        assert_ne!(original.original_rows_sha256, changed.original_rows_sha256);
        let before_rejection = table_rows(&fixture.readonly());
        assert!(
            journal
                .seal_complete_prefix("type-or-sequence-change", &original)
                .is_err()
        );
        assert_unpublished(&journal);
        assert_eq!(table_rows(&fixture.readonly()), before_rejection);
        journal
            .seal_complete_prefix("explicit-new-review", &changed)
            .unwrap();
    }
}

#[test]
fn extra_physical_rows_cannot_hide_outside_the_sequential_chunk_prefix() {
    for ordinal in [-1i64, 0, 99, i64::MAX] {
        let fixture = Fixture::new();
        let mut journal = fixture.populated();
        let receipt = journal.complete_prefix().unwrap();
        fixture.sql().execute(
            "INSERT INTO chunks(ordinal,stream_id,sequence,header,payload,sha256) SELECT ?1,'unselected-extra',0,header,payload,sha256 FROM chunks WHERE ordinal=1",
            [ordinal],
        ).unwrap();
        let retained = table_rows(&fixture.readonly());
        assert!(
            journal.complete_prefix().is_err(),
            "Physical ordinal {ordinal}"
        );
        assert!(
            journal
                .seal_complete_prefix("must-not-ignore-a-row", &receipt)
                .is_err()
        );
        assert_unpublished(&journal);
        assert_eq!(
            table_rows(&fixture.readonly()),
            retained,
            "A rejected original must not be repaired or truncated"
        );
    }
}

#[test]
fn zero_chunks_are_not_complete_and_normal_replay_never_promotes_an_interrupted_take() {
    let fixture = Fixture::new();
    let journal = fixture.populated();
    let receipt = journal.complete_prefix().unwrap();
    let original_bytes = fs::read(fixture.path()).unwrap();
    assert!(matches!(
        CaptureReader::open(fixture.path()),
        Err(RecordingError::Owned)
    ));
    assert_eq!(fs::read(fixture.path()).unwrap(), original_bytes);
    drop(journal);
    let reader = CaptureReader::open(fixture.path()).unwrap();
    let (recovered, replayed) = reader
        .recover_prefix("shared-operation-spelling", &receipt.prefix)
        .unwrap();
    assert!(!replayed);
    assert!(recovered.body.interrupted.is_some());
    let mut reopened = Journal::open(fixture.path()).unwrap();
    let before = table_rows(&fixture.readonly());
    assert!(
        reopened
            .seal_complete_prefix("shared-operation-spelling", &receipt)
            .is_err()
    );
    assert_eq!(reopened.sealed_manifest().unwrap(), Some(recovered));
    assert_eq!(table_rows(&fixture.readonly()), before);

    let empty_fixture = Fixture::new();
    let mut empty = Journal::create(empty_fixture.path(), reopened.descriptor()).unwrap();
    assert!(empty.complete_prefix().is_err());
    let forged = CompletePrefix {
        prefix: RecoveryPrefix {
            chunk_count: 0,
            ..receipt.prefix.clone()
        },
        ..receipt
    };
    assert!(
        empty
            .seal_complete_prefix("empty-is-not-finished", &forged)
            .is_err()
    );
    assert_unpublished(&empty);
}

#[test]
fn borrowed_inspection_keeps_the_live_owner_and_read_snapshot_until_drop() {
    let fixture = Fixture::new();
    let mut journal = fixture.populated();
    let complete = journal.complete_prefix().unwrap();
    let original_file = fs::read(fixture.path()).unwrap();
    let original_rows = table_rows(&fixture.readonly());
    let fourth = journal.read_chunk("pcm-1", 1).unwrap();
    {
        let view = journal.inspect_owned().unwrap();
        assert_eq!(view.inspection().descriptor, *journal.descriptor());
        assert_eq!(
            view.inspection().descriptor_sha256,
            complete.descriptor_sha256
        );
        assert_eq!(
            view.inspection().original_rows_sha256,
            complete.original_rows_sha256
        );
        assert_eq!(view.inspection().report.state, TakeState::Recording);
        assert_eq!(view.inspection().report.verified_chunks, 4);
        assert!(view.inspection().manifest.is_none());
        assert_eq!(view.read_ordinal(4).unwrap(), fourth);
        assert!(view.read_ordinal(0).is_err());
        assert!(view.read_ordinal(5).is_err());
        assert!(matches!(
            CaptureReader::open(fixture.path()),
            Err(RecordingError::Owned)
        ));
        assert_eq!(fs::read(fixture.path()).unwrap(), original_file);

        // A noncooperating SQL writer may stage writes but cannot commit across
        // the held read snapshot. The view keeps seeing its verified originals.
        let contender = fixture.sql();
        contender.busy_timeout(std::time::Duration::ZERO).unwrap();
        contender.execute_batch("BEGIN IMMEDIATE; UPDATE stream_prefix SET source_end=source_end+1,recorded_units=recorded_units+1 WHERE stream_id='pcm-0';").unwrap();
        assert!(contender.execute_batch("COMMIT;").is_err());
        assert_eq!(view.read_ordinal(4).unwrap(), fourth);
        contender.execute_batch("ROLLBACK;").unwrap();
    }
    assert_eq!(table_rows(&fixture.readonly()), original_rows);
    assert_eq!(
        journal.complete_prefix().unwrap(),
        complete,
        "Dropping the view must release its read transaction"
    );
    assert!(
        matches!(
            CaptureReader::open(fixture.path()),
            Err(RecordingError::Owned)
        ),
        "Dropping the view must not release the Journal owner"
    );
    let manifest = journal
        .seal_complete_prefix("after-borrowed-review", &complete)
        .unwrap();
    drop(journal);
    let reader = CaptureReader::open(fixture.path()).unwrap();
    assert_eq!(reader.inspection().manifest.as_ref(), Some(&manifest));
    assert_eq!(reader.read_ordinal(4).unwrap(), fourth);
}

#[test]
fn borrowed_inspection_rejects_a_rehashed_descriptor_changed_after_owner_acquisition() {
    let fixture = Fixture::new();
    let journal = fixture.populated();
    let mut changed = journal.descriptor().clone();
    changed.streams[0].source_name = "Different source under the same path".into();
    let sql = fixture.sql();
    sql.execute_batch("DROP TRIGGER immutable_descriptor;")
        .unwrap();
    sql.execute(
        "UPDATE take_meta SET descriptor=?1,descriptor_sha256=?2",
        params![
            serde_json::to_vec(&changed).unwrap(),
            changed.sha256().unwrap()
        ],
    )
    .unwrap();
    let retained = table_rows(&fixture.readonly());
    assert!(
        journal.inspect_owned().is_err(),
        "An inspection must not report cached identity with current foreign original rows"
    );
    assert!(journal.complete_prefix().is_err());
    assert_eq!(table_rows(&fixture.readonly()), retained);
}

#[cfg(feature = "test-hooks")]
#[test]
fn separate_reader_sees_atomic_normal_publication_and_lost_ack_replays_without_writes() {
    use std::sync::{Arc, Mutex};
    for boundary in [
        WriteBoundary::BeforeSealCommit,
        WriteBoundary::AfterSealCommit,
    ] {
        let fixture = Fixture::new();
        let mut journal = fixture.populated();
        let receipt = journal.complete_prefix().unwrap();
        let before = table_rows(&fixture.readonly());
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observed = observations.clone();
        let path = fixture.path();
        journal.set_boundary_hook(move |point| {
            if point == boundary {
                let connection =
                    Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                let state: String =
                    connection.query_row("SELECT state FROM take_meta", [], |row| row.get(0))?;
                let manifests: i64 =
                    connection.query_row("SELECT count(*) FROM sealed_manifests", [], |row| {
                        row.get(0)
                    })?;
                observed.lock().unwrap().push((state, manifests));
                return Err(RecordingError::Invalid(
                    "Injected lost completion response".into(),
                ));
            }
            Ok(())
        });
        assert!(
            journal
                .seal_complete_prefix("one-stable-normal-intent", &receipt)
                .is_err()
        );
        journal.clear_boundary_hook();
        if boundary == WriteBoundary::BeforeSealCommit {
            assert_eq!(*observations.lock().unwrap(), vec![("recording".into(), 0)]);
            assert_eq!(table_rows(&fixture.readonly()), before);
            assert_unpublished(&journal);
        } else {
            assert_eq!(*observations.lock().unwrap(), vec![("sealed".into(), 1)]);
            let committed = journal.sealed_manifest().unwrap().unwrap();
            assert!(committed.body.interrupted.is_none());
            let committed_rows = table_rows(&fixture.readonly());
            drop(journal);
            let mut reopened = Journal::open(fixture.path()).unwrap();
            assert_eq!(
                reopened
                    .seal_complete_prefix("one-stable-normal-intent", &receipt)
                    .unwrap(),
                committed
            );
            assert!(matches!(
                reopened.seal_complete_prefix("different-intent-after-lost-ack", &receipt),
                Err(RecordingError::Conflict(_))
            ));
            assert_eq!(table_rows(&fixture.readonly()), committed_rows);
        }
    }
}
