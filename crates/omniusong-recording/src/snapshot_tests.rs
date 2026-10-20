use crate::{
    tests::{Fixture, descriptor, header},
    *,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn sealed(fixture: &Fixture) -> (TakeDescriptor, Journal, SealedManifest) {
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    for sequence in 0..3 {
        journal
            .append(&header(&take, sequence, sequence, 1), &[sequence as u8; 8])
            .unwrap();
    }
    let manifest = journal.seal("fixture-seal").unwrap();
    (take, journal, manifest)
}

fn file_hash(path: &Path) -> String {
    // Small fixture files only; production hashing has its own bounded buffer.
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

#[test]
fn sealed_readers_coexist_with_live_owner_and_never_change_original_bytes() {
    let fixture = Fixture::new();
    let (take, journal, manifest) = sealed(&fixture);
    let before = file_hash(&fixture.path());
    let reader = SealedJournal::open(fixture.path()).unwrap();
    let second = SealedJournal::open(fixture.path()).unwrap();
    assert_eq!(reader.descriptor(), &take);
    assert_eq!(reader.manifest(), &manifest);
    assert_eq!(second.read_ordinal(2).unwrap().payload, [1; 8]);
    assert!(matches!(
        Journal::open(fixture.path()),
        Err(RecordingError::Owned)
    ));
    assert_eq!(journal.state().unwrap(), TakeState::Sealed);
    assert_eq!(before, file_hash(&fixture.path()));
    drop((reader, second));
    assert_eq!(before, file_hash(&fixture.path()));
}

#[test]
fn read_only_open_of_active_or_interrupted_take_never_admits_recovery() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let journal = Journal::create(fixture.path(), &take).unwrap();
    let before = file_hash(&fixture.path());
    assert!(SealedJournal::open(fixture.path()).is_err());
    assert_eq!(journal.state().unwrap(), TakeState::Recording);
    assert_eq!(before, file_hash(&fixture.path()));
    drop(journal);
    let journal = Journal::open(fixture.path()).unwrap();
    let before = file_hash(&fixture.path());
    assert_eq!(journal.state().unwrap(), TakeState::Interrupted);
    assert!(SealedJournal::open(fixture.path()).is_err());
    assert_eq!(before, file_hash(&fixture.path()));
}

#[test]
fn ordinal_visitation_binds_ranges_gaps_payloads_and_the_sealed_index() {
    let fixture = Fixture::new();
    let mut take = descriptor(true);
    let mut second_stream = take.streams[0].clone();
    second_stream.stream_id = "source-2".into();
    second_stream.segment_id = "segment-2".into();
    take.streams.push(second_stream);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    journal.append(&header(&take, 0, 0, 1), &[5; 8]).unwrap();
    let mut gap = header(&take, 1, 1, 3);
    gap.kind = ChunkKind::Gap {
        reason: GapReason::TransferOverflow,
    };
    journal.append(&gap, &[]).unwrap();
    let mut other = header(&take, 0, 0, 2);
    other.stream_id = "source-2".into();
    other.segment_id = "segment-2".into();
    journal.append(&other, &[8; 16]).unwrap();
    let manifest = journal.seal("multistream-seal").unwrap();
    let reader = SealedJournal::open(fixture.path()).unwrap();
    let mut ordinals = Vec::new();
    let mut index = Sha256::new();
    index.update(b"OmniuSong.recording-index.v1\0");
    index.update(manifest.body.descriptor_sha256.as_bytes());
    reader
        .visit_chunks(|ordinal, chunk| {
            ordinals.push((
                ordinal,
                chunk.header.stream_id.clone(),
                chunk.header.sequence,
            ));
            index.update(ordinal.to_le_bytes());
            index.update(chunk.sha256.as_bytes());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        ordinals,
        [
            (1, "source-1".into(), 0),
            (2, "source-1".into(), 1),
            (3, "source-2".into(), 0)
        ]
    );
    assert_eq!(
        format!("{:x}", index.finalize()),
        manifest.body.chunk_index_sha256
    );
    assert_eq!(reader.read_ordinal(2).unwrap().header, gap);
    for ordinal in [0, 4, u64::MAX] {
        assert!(reader.read_ordinal(ordinal).is_err());
    }
    assert!(reader.read_chunk("source-2", 1).is_err());
    let mut visits = 0;
    assert!(
        reader
            .visit_chunks(|_, _| {
                visits += 1;
                Err(RecordingError::Invalid("Stop before publication".into()))
            })
            .is_err()
    );
    assert_eq!(visits, 1);
}

#[test]
fn sqlite_backup_receipt_verifies_file_bytes_while_original_stays_unchanged() {
    let fixture = Fixture::new();
    let (_, _owner, manifest) = sealed(&fixture);
    let before = file_hash(&fixture.path());
    let reader = SealedJournal::open(fixture.path()).unwrap();
    let destination = fixture.0.join("export.sqlite3");
    let receipt = reader.export_backup(&destination).unwrap();
    assert_eq!(receipt.path, destination);
    assert_eq!(receipt.byte_len, fs::metadata(&destination).unwrap().len());
    assert_eq!(receipt.sha256, file_hash(&destination));
    assert_eq!(receipt.manifest_sha256, manifest.sha256);
    assert_eq!(receipt.take_id, manifest.body.descriptor.take_id);
    let exported = SealedJournal::open(&destination).unwrap();
    assert_eq!(exported.manifest(), &manifest);
    exported
        .visit_chunks(|ordinal, chunk| {
            assert_eq!(*chunk, reader.read_ordinal(ordinal)?);
            Ok(())
        })
        .unwrap();
    assert_eq!(before, file_hash(&fixture.path()));
    assert!(!fs::read_dir(&fixture.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")
    }));
}

#[test]
fn backup_and_visitation_stream_more_than_thirty_mebibytes_in_bounded_chunks() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    let payload = vec![6; MAX_AUDIO_CHUNK_BYTES];
    let frames = payload.len() as u64 / 8;
    for sequence in 0..33 {
        journal
            .append(
                &header(&take, sequence, sequence * frames, frames),
                &payload,
            )
            .unwrap();
    }
    journal.seal("snapshot-long-audio").unwrap();
    let reader = SealedJournal::open(fixture.path()).unwrap();
    let receipt = reader
        .export_backup(fixture.0.join("large.sqlite3"))
        .unwrap();
    assert!(receipt.byte_len > 30 * 1024 * 1024);
    let copy = SealedJournal::open(receipt.path).unwrap();
    let mut count = 0;
    let mut bytes = 0;
    copy.visit_chunks(|ordinal, chunk| {
        count += 1;
        bytes += chunk.payload.len();
        assert_eq!(ordinal, count);
        assert_eq!(chunk.payload.len(), MAX_AUDIO_CHUNK_BYTES);
        assert_eq!(chunk.payload, payload);
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 33);
    assert_eq!(bytes, 33 * MAX_AUDIO_CHUNK_BYTES);
}

#[test]
fn recovered_export_retains_damaged_unselected_bytes_receipt_and_original_index() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    for sequence in 0..3 {
        journal
            .append(&header(&take, sequence, sequence, 1), &[sequence as u8; 8])
            .unwrap();
    }
    drop(journal);
    let database = Connection::open(fixture.path()).unwrap();
    database
        .execute_batch("DROP TRIGGER immutable_chunk_update;")
        .unwrap();
    let damaged = b"unselected original damaged tail";
    database
        .execute(
            "UPDATE chunks SET payload=?1 WHERE ordinal=3",
            [damaged.as_slice()],
        )
        .unwrap();
    drop(database);
    let mut journal = Journal::open(fixture.path()).unwrap();
    let manifest = journal.seal_recovered("retain-damaged-tail").unwrap();
    assert_eq!(manifest.body.chunk_count, 2);
    assert!(
        manifest
            .body
            .interrupted
            .as_ref()
            .unwrap()
            .report
            .unknown_tail
    );
    let before = file_hash(&fixture.path());
    let reader = SealedJournal::open(fixture.path()).unwrap();
    assert!(reader.read_ordinal(3).is_err());
    assert!(reader.read_chunk("source-1", 2).is_err());
    let destination = fixture.0.join("recovered.sqlite3");
    reader.export_backup(&destination).unwrap();
    let source = Connection::open(fixture.path()).unwrap();
    let copy = Connection::open(&destination).unwrap();
    let raw: Vec<u8> = copy
        .query_row("SELECT payload FROM chunks WHERE ordinal=3", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(raw, damaged);
    for query in [
        "SELECT receipt FROM interruptions",
        "SELECT body FROM sealed_manifests",
    ] {
        let original: Vec<u8> = source.query_row(query, [], |row| row.get(0)).unwrap();
        let retained: Vec<u8> = copy.query_row(query, [], |row| row.get(0)).unwrap();
        assert_eq!(original, retained);
    }
    let index = |database: &Connection| {
        database
            .prepare("SELECT ordinal,sha256 FROM recovery_index ORDER BY ordinal")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(index(&source), index(&copy));
    assert_eq!(before, file_hash(&fixture.path()));
}

#[test]
fn destination_conflicts_and_missing_parent_leave_source_and_existing_file_intact() {
    let fixture = Fixture::new();
    let (_, _owner, _) = sealed(&fixture);
    let reader = SealedJournal::open(fixture.path()).unwrap();
    let before = file_hash(&fixture.path());
    let destination = fixture.0.join("existing.sqlite3");
    fs::write(&destination, b"Keep this unrelated object").unwrap();
    assert!(matches!(
        reader.export_backup(&destination),
        Err(RecordingError::Backup {
            published: false,
            staging_path: None,
            ..
        })
    ));
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"Keep this unrelated object"
    );
    assert!(
        reader
            .export_backup(fixture.0.join("missing/export.sqlite3"))
            .is_err()
    );
    assert!(!fixture.0.join("missing").exists());
    assert_eq!(before, file_hash(&fixture.path()));
}

#[cfg(unix)]
#[test]
fn read_only_aliases_and_permissions_work_without_creating_owner_sidecars() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let (_, owner, manifest) = sealed(&fixture);
    drop(owner);
    let sidecar = fixture.0.join("take.sqlite3.owner.lock");
    fs::remove_file(&sidecar).unwrap();
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o444)).unwrap();
    let symlink_path = fixture.0.join("symbolic.sqlite3");
    let hardlink_path = fixture.0.join("hard.sqlite3");
    symlink(fixture.path(), &symlink_path).unwrap();
    fs::hard_link(fixture.path(), &hardlink_path).unwrap();
    let before = file_hash(&fixture.path());
    for path in [&fixture.path(), &symlink_path, &hardlink_path] {
        let reader = SealedJournal::open(path).unwrap();
        assert_eq!(reader.manifest(), &manifest);
    }
    assert!(!sidecar.exists());
    assert_eq!(before, file_hash(&fixture.path()));
    let dangling = fixture.0.join("dangling.sqlite3");
    symlink(fixture.0.join("absent"), &dangling).unwrap();
    let reader = SealedJournal::open(fixture.path()).unwrap();
    assert!(reader.export_backup(&dangling).is_err());
    assert!(
        fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!fixture.0.join("absent").exists());
}

#[cfg(unix)]
#[test]
fn held_source_snapshot_survives_path_replacement_and_exports_its_verified_identity() {
    let fixture = Fixture::new();
    let (_, owner, original) = sealed(&fixture);
    drop(owner);
    let reader = SealedJournal::open(fixture.path()).unwrap();
    let moved = fixture.0.join("original.sqlite3");
    fs::rename(fixture.path(), &moved).unwrap();
    let replacement_fixture = Fixture::new();
    let (_, replacement_owner, replacement) = sealed(&replacement_fixture);
    drop(replacement_owner);
    fs::copy(replacement_fixture.path(), fixture.path()).unwrap();
    assert_ne!(original.sha256, replacement.sha256);
    assert_eq!(reader.manifest(), &original);
    let destination = fixture.0.join("held-view.sqlite3");
    reader.export_backup(&destination).unwrap();
    assert_eq!(
        SealedJournal::open(&destination).unwrap().manifest(),
        &original
    );
    assert_eq!(
        SealedJournal::open(fixture.path()).unwrap().manifest(),
        &replacement
    );
}

#[test]
fn unsupported_versions_tampering_and_oversized_stored_values_are_rejected() {
    for mutation in [
        "PRAGMA user_version=2",
        "DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'0101010101010101' WHERE ordinal=1",
        "PRAGMA ignore_check_constraints=ON; DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(1048577) WHERE ordinal=1",
        "DROP TRIGGER immutable_descriptor; UPDATE take_meta SET descriptor=zeroblob(262145)",
        "DROP TRIGGER immutable_manifest_update; UPDATE sealed_manifests SET body=zeroblob(524289)",
    ] {
        let fixture = Fixture::new();
        let (_, owner, _) = sealed(&fixture);
        drop(owner);
        let database = Connection::open(fixture.path()).unwrap();
        database.execute_batch(mutation).unwrap();
        drop(database);
        let before = file_hash(&fixture.path());
        assert!(
            SealedJournal::open(fixture.path()).is_err(),
            "Mutation accepted: {mutation}"
        );
        assert_eq!(before, file_hash(&fixture.path()));
    }
}

#[test]
fn backup_preserves_unfamiliar_numeric_provenance_without_float_conversion() {
    let fixture = Fixture::new();
    let mut take = descriptor(true);
    let number: serde_json::Value =
        serde_json::from_str("18446744073709551616000000001.000000000000000001").unwrap();
    take.provenance
        .insert("future:exact".into(), number.clone());
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    journal.seal("empty-sealed-take").unwrap();
    let reader = SealedJournal::open(fixture.path()).unwrap();
    let destination = fixture.0.join("exact.sqlite3");
    reader.export_backup(&destination).unwrap();
    let copied = SealedJournal::open(&destination).unwrap();
    assert_eq!(copied.descriptor().provenance["future:exact"], number);
    let database = Connection::open(destination).unwrap();
    let chunks: i64 = database
        .query_row(
            "SELECT count(*) FROM chunks WHERE ordinal>?1",
            params![0_i64],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(chunks, 0);
}
