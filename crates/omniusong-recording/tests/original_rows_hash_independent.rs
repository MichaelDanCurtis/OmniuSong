//! Independent v1 original-row encoding oracle for bounded synthetic fixtures.
//! The oracle reads whole SQL values via SELECT rowid,* and ValueRef, rather
//! than production's per-column length/type/piece queries. It is test-only and
//! never supplies native Finish, callback retirement or normal-seal authority.
use omniusong_recording::*;
use rusqlite::{Connection, OpenFlags, params, types::Value, types::ValueRef};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf};

const FRAMES_PER_CHUNK: u64 = 16_385;
const ROWS: u64 = 8;
const FIXTURE_BYTE_LIMIT: u64 = 10 * 1024 * 1024;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/original-rows-hash-independent")
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
    fn readonly(&self) -> Connection {
        Connection::open_with_flags(self.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
    }
    fn assert_small(&self) {
        assert!(fs::metadata(self.path()).unwrap().len() < FIXTURE_BYTE_LIMIT);
    }
    fn populated(&self) -> Journal {
        let descriptor = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "synthetic-segment".into(),
                source_id: "synthetic-value-oracle".into(),
                source_name: "Independent row-hash fixture; no native device".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("synthetic-frames".into()),
                    epoch: ClockEpochId("synthetic-host-epoch".into()),
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
                    audio_epoch: ClockEpochId("synthetic-audio-epoch".into()),
                },
                initial_source_position: 100,
            }],
            provenance: BTreeMap::from([(
                "omniusong.performance.v2".into(),
                json!({"version": 2, "config": {"independent_storage_fixture": true}}),
            )]),
        };
        let mut journal = Journal::create(self.path(), &descriptor).unwrap();
        let payload = (0..FRAMES_PER_CHUNK)
            .flat_map(|_| 0.125f32.to_le_bytes())
            .collect::<Vec<_>>();
        assert!(payload.len() > 64 * 1024);
        for sequence in 0..ROWS {
            let start = 100 + sequence * FRAMES_PER_CHUNK;
            let stamp = HostTimestamp {
                domain: descriptor.streams[0].host_clock.domain.clone(),
                epoch: descriptor.streams[0].host_clock.epoch.clone(),
                ticks: sequence * FRAMES_PER_CHUNK,
            };
            let header = ChunkHeader {
                version: CHUNK_VERSION,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "synthetic-segment".into(),
                sequence,
                range: SourceRange::AudioFrames {
                    start,
                    count: FRAMES_PER_CHUNK,
                },
                interpretation: descriptor.streams[0].kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            };
            journal.append(&header, &payload).unwrap();
        }
        let connection = self.sql();
        connection
            .execute_batch(
                "DROP TRIGGER immutable_chunk_update;
                 ALTER TABLE chunks ADD COLUMN \"opaque\"\"value\";
                 ALTER TABLE chunks ADD COLUMN \"select\";
                 ALTER TABLE chunks ADD COLUMN \"wide blob\";
                 ALTER TABLE chunks ADD COLUMN \"empty text\";
                 ALTER TABLE chunks ADD COLUMN \"empty blob\";",
            )
            .unwrap();
        let mixed = [
            Value::Null,
            Value::Integer(i64::MIN),
            Value::Integer(i64::MAX),
            Value::Real(-1234.125),
            Value::Text("text\0after-NUL 🦀".into()),
            Value::Blob(vec![0, 255, 1, 0, 128]),
            Value::Text(String::new()),
            Value::Blob(Vec::new()),
        ];
        let large_text = "é\0🦀".repeat(20_000);
        assert!(large_text.len() > 2 * 64 * 1024);
        let large_blob = (0..(2 * 64 * 1024 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        for (index, value) in mixed.into_iter().enumerate() {
            let text = match index {
                0 => Value::Text(large_text.clone()),
                1 => Value::Text(String::new()),
                2 => Value::Text("x".repeat(64 * 1024 + 1)),
                3 => Value::Null,
                4 => Value::Text("\0not a terminator\0".into()),
                5 => Value::Integer(42),
                6 => Value::Real(3.5),
                _ => Value::Blob(vec![0, 1, 2]),
            };
            let blob = match index {
                1 => Value::Blob(large_blob[..64 * 1024 + 1].to_vec()),
                2 => Value::Null,
                3 => Value::Blob(large_blob.clone()),
                4 => Value::Blob(vec![255, 0]),
                6 => Value::Text("blob-column text\0tail".into()),
                7 => Value::Integer(-42),
                _ => Value::Blob(Vec::new()),
            };
            connection
                .execute(
                    "UPDATE chunks SET \"opaque\"\"value\"=?1,\"select\"=?2,\"wide blob\"=?3,\"empty text\"=?4,\"empty blob\"=?5 WHERE ordinal=?6",
                    params![value, text, blob, "", Vec::<u8>::new(), index as i64 + 1],
                )
                .unwrap();
        }
        drop(connection);
        self.assert_small();
        journal
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn oracle_value(hash: &mut Sha256, value: ValueRef<'_>) {
    let kind: &[u8] = match value {
        ValueRef::Null => b"null",
        ValueRef::Integer(_) => b"integer",
        ValueRef::Real(_) => b"real",
        ValueRef::Text(_) => b"text",
        ValueRef::Blob(_) => b"blob",
    };
    hash.update((kind.len() as u64).to_le_bytes());
    hash.update(kind);
    match value {
        ValueRef::Null => {}
        ValueRef::Integer(number) => hash.update(number.to_le_bytes()),
        ValueRef::Real(number) => hash.update(number.to_bits().to_le_bytes()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
    }
}

fn oracle(connection: &Connection) -> String {
    let transaction = connection.unchecked_transaction().unwrap();
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.capture-original-rows.v1\0");
    for table in [
        "sqlite_schema",
        "take_meta",
        "stream_prefix",
        "chunks",
        "sqlite_sequence",
    ] {
        let mut statement = transaction
            .prepare(&format!("SELECT rowid,* FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement
            .column_names()
            .into_iter()
            .enumerate()
            .skip(1)
            .filter(|(_, name)| table != "take_meta" || !name.eq_ignore_ascii_case("state"))
            .map(|(index, name)| (index, name.to_owned()))
            .collect::<Vec<_>>();
        hash.update((table.len() as u64).to_le_bytes());
        hash.update(table.as_bytes());
        hash.update((columns.len() as u64).to_le_bytes());
        for (_, name) in &columns {
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
        }
        let mut rows = statement.query([]).unwrap();
        let mut count = 0u64;
        while let Some(row) = rows.next().unwrap() {
            hash.update(row.get::<_, i64>(0).unwrap().to_le_bytes());
            for (index, _) in &columns {
                oracle_value(&mut hash, row.get_ref(*index).unwrap());
            }
            count += 1;
        }
        hash.update(count.to_le_bytes());
    }
    transaction.commit().unwrap();
    format!("{:x}", hash.finalize())
}

#[test]
fn full_prefix_hash_matches_independent_sql_values_before_and_after_checked_seal() {
    let fixture = Fixture::new();
    let mut journal = fixture.populated();
    let before = fs::read(fixture.path()).unwrap();
    let expected = oracle(&fixture.readonly());
    let receipt = journal.complete_prefix().unwrap();
    assert_eq!(receipt.prefix.chunk_count, ROWS);
    assert_eq!(receipt.original_rows_sha256, expected);
    assert_eq!(journal.complete_prefix().unwrap(), receipt);
    {
        let view = journal.inspect_owned().unwrap();
        assert_eq!(view.inspection().original_rows_sha256, expected);
    }
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    let manifest = journal
        .seal_complete_prefix("independent-value-oracle", &receipt)
        .unwrap();
    assert!(manifest.body.interrupted.is_none());
    assert_eq!(oracle(&fixture.readonly()), expected);
    assert_eq!(journal.complete_prefix().unwrap(), receipt);
    assert_eq!(
        journal
            .seal_complete_prefix("independent-value-oracle", &receipt)
            .unwrap(),
        manifest
    );
    fixture.assert_small();
}

#[test]
fn changed_opaque_original_has_new_oracle_hash_and_cannot_reuse_prior_complete_receipt() {
    let fixture = Fixture::new();
    let mut journal = fixture.populated();
    let receipt = journal.complete_prefix().unwrap();
    assert_eq!(receipt.original_rows_sha256, oracle(&fixture.readonly()));
    fixture
        .sql()
        .execute(
            "UPDATE chunks SET \"opaque\"\"value\"=?1 WHERE ordinal=8",
            [Value::Blob(b"changed original\0tail".to_vec())],
        )
        .unwrap();
    let changed = journal.complete_prefix().unwrap();
    assert_eq!(changed.original_rows_sha256, oracle(&fixture.readonly()));
    assert_ne!(changed.original_rows_sha256, receipt.original_rows_sha256);
    assert_eq!(changed.prefix, receipt.prefix);
    let retained = fs::read(fixture.path()).unwrap();
    assert!(
        journal
            .seal_complete_prefix("must-reject-old-oracle-receipt", &receipt)
            .is_err()
    );
    assert_eq!(journal.state().unwrap(), TakeState::Recording);
    assert!(journal.sealed_manifest().unwrap().is_none());
    assert_eq!(fs::read(fixture.path()).unwrap(), retained);
    assert_eq!(journal.complete_prefix().unwrap(), changed);
}

#[test]
fn damaged_and_unselected_tail_rows_remain_in_the_independent_original_hash() {
    let fixture = Fixture::new();
    drop(fixture.populated());
    fixture
        .sql()
        .execute("UPDATE chunks SET payload=X'01' WHERE ordinal=7", [])
        .unwrap();
    let before = fs::read(fixture.path()).unwrap();
    let reader = CaptureReader::open(fixture.path()).unwrap();
    assert_eq!(reader.inspection().report.verified_chunks, 6);
    assert!(reader.inspection().report.unknown_tail);
    let first = reader.inspection().original_rows_sha256.clone();
    assert_eq!(first, oracle(&fixture.readonly()));
    drop(reader);
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    fixture
        .sql()
        .execute(
            "UPDATE chunks SET \"opaque\"\"value\"=?1 WHERE ordinal=8",
            [Value::Text(
                "unselected original changed\0after damage".into(),
            )],
        )
        .unwrap();
    let changed_bytes = fs::read(fixture.path()).unwrap();
    let reader = CaptureReader::open(fixture.path()).unwrap();
    assert_eq!(reader.inspection().report.verified_chunks, 6);
    assert_eq!(
        reader.inspection().original_rows_sha256,
        oracle(&fixture.readonly())
    );
    assert_ne!(reader.inspection().original_rows_sha256, first);
    assert_eq!(fs::read(fixture.path()).unwrap(), changed_bytes);
    fixture.assert_small();
}
