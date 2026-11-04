//! Independent fixed-view sealed snapshot and original-retention regressions.
use omniusong_recording::*;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/snapshot-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn source(&self) -> PathBuf {
        self.0.join("source.sqlite3")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Snapshot review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn descriptor(id: &str) -> TakeDescriptor {
    TakeDescriptor { version:JOURNAL_VERSION,take_id:TakeId(id.into()),streams:vec![StreamDescriptor {
        stream_id:"audio".into(),segment_id:"segment".into(),source_id:"input".into(),source_name:"Synthetic exact source".into(),source_connection_generation:1,
        host_clock:HostClock {domain:ClockDomainId("host".into()),epoch:ClockEpochId("host-epoch".into()),ticks_per_second_numerator:24_000_000,ticks_per_second_denominator:1},
        kind:StreamKind::Audio{format:PcmFormat{sample_rate:48_000,channels:2,encoding:PcmEncoding::S16Le,layout:PcmLayout::Interleaved},audio_epoch:ClockEpochId("audio-epoch".into())},
        initial_source_position:100,
    }],provenance:BTreeMap::from([("vendor".into(),serde_json::from_str(r#"{"integer":1234567890123456789012345678901234567890,"decimal":0.123456789012345678901234567890123456789,"takeId":"opaque"}"#).unwrap())]) }
}
fn header(take: &TakeDescriptor, sequence: u64) -> ChunkHeader {
    let observed = HostTimestamp {
        domain: take.streams[0].host_clock.domain.clone(),
        epoch: take.streams[0].host_clock.epoch.clone(),
        ticks: u64::MAX - 1,
    };
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: take.take_id.clone(),
        stream_id: "audio".into(),
        segment_id: "segment".into(),
        sequence,
        range: SourceRange::AudioFrames {
            start: 100 + sequence,
            count: 1,
        },
        interpretation: take.streams[0].kind.clone(),
        observed_start: observed.clone(),
        observed_end: observed,
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
fn create_sealed(path: &Path, id: &str, bytes: [u8; 4]) -> SealedManifest {
    let take = descriptor(id);
    let mut journal = Journal::create(path, &take).unwrap();
    journal.append(&header(&take, 0), &bytes).unwrap();
    journal.seal("seal").unwrap()
}
fn hash(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}
fn state(path: &Path) -> String {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT state FROM take_meta", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn read_only_rejection_does_not_interrupt_or_take_over_a_live_unsealed_owner() {
    let fixture = Fixture::new();
    let take = descriptor("unfinished");
    let mut owner = Journal::create(fixture.source(), &take).unwrap();
    owner.append(&header(&take, 0), &[1, 2, 3, 4]).unwrap();
    let before = fs::read(fixture.source()).unwrap();
    assert!(SealedJournal::open(fixture.source()).is_err());
    assert_eq!(fs::read(fixture.source()).unwrap(), before);
    assert_eq!(owner.state().unwrap(), TakeState::Recording);
    owner.append(&header(&take, 1), &[5, 6, 7, 8]).unwrap();
    owner.seal("seal").unwrap();
    let reader = SealedJournal::open(fixture.source()).unwrap();
    assert_eq!(reader.manifest().body.chunk_count, 2);
    assert_eq!(reader.read_ordinal(2).unwrap().payload, [5, 6, 7, 8]);
}

#[test]
fn backup_receipt_hashes_the_complete_owned_file_without_changing_source_bytes() {
    let fixture = Fixture::new();
    let manifest = create_sealed(&fixture.source(), "complete", [1, 2, 3, 4]);
    let before = fs::read(fixture.source()).unwrap();
    let reader = SealedJournal::open(fixture.source()).unwrap();
    let destination = fixture.0.join("backup.sqlite3");
    let receipt = reader.export_backup(&destination).unwrap();
    assert_eq!(receipt.path, fs::canonicalize(&destination).unwrap());
    assert_eq!(receipt.sha256, hash(&destination));
    assert_eq!(receipt.byte_len, fs::metadata(&destination).unwrap().len());
    assert_eq!(receipt.manifest_sha256, manifest.sha256);
    assert_eq!(receipt.take_id, manifest.body.descriptor.take_id);
    let copied = SealedJournal::open(&destination).unwrap();
    assert_eq!(copied.manifest(), &manifest);
    assert_eq!(copied.descriptor(), reader.descriptor());
    assert_eq!(copied.read_ordinal(1).unwrap().payload, [1, 2, 3, 4]);
    assert_eq!(fs::read(fixture.source()).unwrap(), before);
    assert!(!fixture.0.read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".omniusong-backup-")
    }));
}

#[test]
fn backup_never_overwrites_existing_files_or_destination_symlinks() {
    let fixture = Fixture::new();
    create_sealed(&fixture.source(), "destination", [1, 2, 3, 4]);
    let reader = SealedJournal::open(fixture.source()).unwrap();
    let existing = fixture.0.join("existing");
    fs::write(&existing, b"existing archive bytes").unwrap();
    assert!(matches!(
        reader.export_backup(&existing),
        Err(RecordingError::Backup {
            published: false,
            staging_path: None,
            ..
        })
    ));
    assert_eq!(fs::read(&existing).unwrap(), b"existing archive bytes");
    #[cfg(unix)]
    {
        for dangling in [false, true] {
            let link = fixture.0.join(if dangling { "dangling" } else { "link" });
            let target = if dangling {
                fixture.0.join("absent")
            } else {
                existing.clone()
            };
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(matches!(
                reader.export_backup(&link),
                Err(RecordingError::Backup {
                    published: false,
                    staging_path: None,
                    ..
                })
            ));
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            if dangling {
                assert!(!target.exists());
            } else {
                assert_eq!(fs::read(&target).unwrap(), b"existing archive bytes");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn actual_opened_read_view_survives_source_path_replacement_and_allows_source_aliases() {
    let fixture = Fixture::new();
    let old = create_sealed(&fixture.source(), "original-view", [1, 2, 3, 4]);
    let reader = SealedJournal::open(fixture.source()).unwrap();
    let retained = fixture.0.join("retained-original.sqlite3");
    fs::rename(fixture.source(), &retained).unwrap();
    let replacement = create_sealed(&fixture.source(), "replacement-view", [9, 8, 7, 6]);
    assert_ne!(old.sha256, replacement.sha256);
    assert_eq!(reader.manifest(), &old);
    assert_eq!(reader.read_chunk("audio", 0).unwrap().payload, [1, 2, 3, 4]);
    let destination = fixture.0.join("held-view.sqlite3");
    reader.export_backup(&destination).unwrap();
    assert_eq!(SealedJournal::open(&destination).unwrap().manifest(), &old);
    assert_eq!(
        SealedJournal::open(fixture.source()).unwrap().manifest(),
        &replacement
    );
    let symlink = fixture.0.join("source-link");
    std::os::unix::fs::symlink(&retained, &symlink).unwrap();
    let hardlink = fixture.0.join("source-hardlink");
    fs::hard_link(&retained, &hardlink).unwrap();
    for alias in [&symlink, &hardlink] {
        assert_eq!(SealedJournal::open(alias).unwrap().manifest(), &old);
    }
    assert_eq!(state(&retained), "sealed");
}

#[test]
fn recovered_snapshot_retains_bad_original_tail_but_exposes_only_selected_prefix() {
    let fixture = Fixture::new();
    let take = descriptor("recovered");
    let mut journal = Journal::create(fixture.source(), &take).unwrap();
    journal.append(&header(&take, 0), &[1, 2, 3, 4]).unwrap();
    journal.append(&header(&take, 1), &[5, 6, 7, 8]).unwrap();
    drop(journal);
    let db = Connection::open(fixture.source()).unwrap();
    db.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'00000000' WHERE sequence=1;").unwrap();
    drop(db);
    let mut recovered = Journal::open(fixture.source()).unwrap();
    let manifest = recovered.seal_recovered("recover").unwrap();
    drop(recovered);
    assert_eq!(manifest.body.chunk_count, 1);
    let reader = SealedJournal::open(fixture.source()).unwrap();
    assert!(reader.read_chunk("audio", 1).is_err());
    assert!(reader.read_ordinal(2).is_err());
    assert!(reader.read_ordinal(0).is_err());
    let mut ordinals = vec![];
    reader
        .visit_chunks(|ordinal, _| {
            ordinals.push(ordinal);
            Ok(())
        })
        .unwrap();
    assert_eq!(ordinals, [1]);
    let destination = fixture.0.join("recovered-backup.sqlite3");
    reader.export_backup(&destination).unwrap();
    let copied = SealedJournal::open(&destination).unwrap();
    assert_eq!(copied.manifest(), &manifest);
    let db = Connection::open(&destination).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
    let bad: Vec<u8> = db
        .query_row("SELECT payload FROM chunks WHERE sequence=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(bad, [0, 0, 0, 0]);
    let receipts: i64 = db
        .query_row("SELECT count(*) FROM interruptions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(receipts, 1);
    let indices: i64 = db
        .query_row("SELECT count(*) FROM recovery_index", [], |row| row.get(0))
        .unwrap();
    assert_eq!(indices, 1);
}

#[test]
fn visitor_failure_is_not_a_successful_complete_export_or_a_source_mutation() {
    let fixture = Fixture::new();
    create_sealed(&fixture.source(), "visitor", [1, 2, 3, 4]);
    let before = hash(&fixture.source());
    let reader = SealedJournal::open(fixture.source()).unwrap();
    let mut seen = 0;
    assert!(
        reader
            .visit_chunks(|_, _| {
                seen += 1;
                Err(RecordingError::Invalid("isolated visitor stop".into()))
            })
            .is_err()
    );
    assert_eq!(seen, 1);
    assert_eq!(hash(&fixture.source()), before);
    assert!(reader.read_chunk("missing", 0).is_err());
    assert!(reader.read_chunk("audio", u64::MAX).is_err());
    assert_eq!(reader.read_chunk("audio", 0).unwrap().payload, [1, 2, 3, 4]);
}
