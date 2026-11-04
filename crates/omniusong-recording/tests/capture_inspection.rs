//! Owner qualification of read-only stage guards and normalized original proofs.
use omniusong_recording::*;
use rusqlite::Connection;
use std::{collections::BTreeMap, fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/capture-inspection-owner")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn create(&self) -> TakeDescriptor {
        let take = descriptor();
        let mut j = Journal::create(self.path(), &take).unwrap();
        for sequence in 0..2 {
            j.append(&chunk(&take, sequence), &[1; 8]).unwrap();
        }
        drop(j);
        take
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
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "segment".into(),
            source_id: "synthetic".into(),
            source_name: "No device".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("host".into()),
                epoch: ClockEpochId("epoch".into()),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            kind: StreamKind::Audio {
                format: PcmFormat {
                    sample_rate: 48000,
                    channels: 1,
                    encoding: PcmEncoding::F32Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: ClockEpochId("audio".into()),
            },
            initial_source_position: 100,
        }],
        provenance: BTreeMap::from([(
            "vendor".into(),
            serde_json::from_str(r#"{"huge":123456789012345678901234567890,"takeId":"opaque"}"#)
                .unwrap(),
        )]),
    }
}
fn chunk(t: &TakeDescriptor, sequence: u64) -> ChunkHeader {
    let stamp = HostTimestamp {
        domain: t.streams[0].host_clock.domain.clone(),
        epoch: t.streams[0].host_clock.epoch.clone(),
        ticks: sequence * 1000,
    };
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: t.take_id.clone(),
        stream_id: "audio".into(),
        segment_id: "segment".into(),
        sequence,
        range: SourceRange::AudioFrames {
            start: 100 + sequence * 2,
            count: 2,
        },
        interpretation: t.streams[0].kind.clone(),
        observed_start: stamp.clone(),
        observed_end: stamp,
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
#[test]
fn missing_owner_guard_is_not_created_by_readonly_inspection() {
    let f = Fixture::new();
    f.create();
    let mut owner = f.path().as_os_str().to_owned();
    owner.push(".owner.lock");
    let owner = PathBuf::from(owner);
    fs::remove_file(&owner).unwrap();
    let before = fs::read(f.path()).unwrap();
    assert!(CaptureReader::open(f.path()).is_err());
    assert!(!owner.exists());
    assert_eq!(fs::read(f.path()).unwrap(), before);
}
#[test]
fn wal_mode_is_rejected_before_sqlite_can_create_sidecars() {
    let f = Fixture::new();
    f.create();
    let mut bytes = fs::read(f.path()).unwrap();
    bytes[18] = 2;
    bytes[19] = 2;
    fs::write(f.path(), &bytes).unwrap();
    assert!(CaptureReader::open(f.path()).is_err());
    assert_eq!(fs::read(f.path()).unwrap(), bytes);
    for suffix in ["-wal", "-shm"] {
        let mut path = f.path().as_os_str().to_owned();
        path.push(suffix);
        assert!(!PathBuf::from(path).exists());
    }
}
#[test]
fn intentional_finalization_changes_physical_hash_but_not_any_original_rows() {
    let f = Fixture::new();
    let take = f.create();
    let c = Connection::open(f.path()).unwrap();
    c.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(8) WHERE sequence=1;").unwrap();
    drop(c);
    let reader = CaptureReader::open(f.path()).unwrap();
    let before = reader.inspection().clone();
    assert_eq!(before.report.verified_chunks, 1);
    let (manifest, replayed) = reader.recover("owner-reviewed-recovery").unwrap();
    assert!(!replayed);
    let reader = CaptureReader::open(f.path()).unwrap();
    let after = reader.inspection();
    assert_ne!(after.journal_sha256, before.journal_sha256);
    assert_eq!(after.original_rows_sha256, before.original_rows_sha256);
    assert_eq!(after.descriptor, take);
    assert_eq!(after.manifest, Some(manifest));
    assert!(after.report.unknown_tail);
    drop(reader);
    let c = Connection::open(f.path()).unwrap();
    let count: i64 = c
        .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    let bad: Vec<u8> = c
        .query_row("SELECT payload FROM chunks WHERE sequence=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(bad, vec![0; 8]);
}
#[test]
fn unexpected_oversize_schema_identity_is_rejected_without_source_mutation() {
    let f = Fixture::new();
    f.create();
    let c = Connection::open(f.path()).unwrap();
    let name = "x".repeat(1024);
    c.execute_batch(&format!("CREATE TABLE \"{name}\"(payload BLOB);"))
        .unwrap();
    drop(c);
    let before = fs::read(f.path()).unwrap();
    let error = match CaptureReader::open(f.path()) {
        Ok(_) => panic!("unknown schema was admitted"),
        Err(e) => e,
    };
    assert!(error.to_string().contains("schema table identity"));
    assert_eq!(fs::read(f.path()).unwrap(), before);
}
