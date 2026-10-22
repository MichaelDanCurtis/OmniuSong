//! Actual closed synthetic SQLite files; no device or regular studio access.
use crate::{
    tests::{Fixture, descriptor, header},
    *,
};
use std::{
    fs::{self, FileTimes, OpenOptions},
    io::{Seek, SeekFrom, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn source() -> Fixture {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut journal = Journal::create(fixture.path(), &take).unwrap();
    for index in 0..3 {
        journal
            .append(&header(&take, index, index, 1), &[index as u8; 8])
            .unwrap();
    }
    journal.seal("identity-fixture-seal").unwrap();
    drop(journal);
    fixture
}

fn budget(cancelled: Arc<AtomicBool>) -> InspectionBudget {
    InspectionBudget::new(InspectionLimits::default(), cancelled).unwrap()
}

#[test]
fn unchanged_bounded_snapshot_chunk_and_visit_preserve_exact_bytes() {
    let fixture = source();
    let before = fs::read(fixture.path()).unwrap();
    let governor = budget(Arc::new(AtomicBool::new(false)));
    let reader = SealedJournal::open_bounded(fixture.path(), &governor).unwrap();
    reader.verify_source_identity().unwrap();
    assert_eq!(
        reader.inspect_snapshot().unwrap().manifest.as_ref(),
        Some(reader.manifest())
    );
    assert_eq!(reader.read_ordinal(2).unwrap().payload, [1; 8]);
    let mut seen = 0;
    reader
        .visit_chunks(|_, _| {
            seen += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, 3);
    reader.verify_source_identity().unwrap();
    drop(reader);
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
}

#[test]
fn same_sized_replacement_cannot_mix_a_new_path_hash_with_old_sqlite_rows() {
    let original = source();
    let other = source();
    let original_bytes = fs::read(original.path()).unwrap();
    let replacement_bytes = fs::read(other.path()).unwrap();
    assert_eq!(original_bytes.len(), replacement_bytes.len());
    assert_ne!(original_bytes, replacement_bytes);
    let governor = budget(Arc::new(AtomicBool::new(false)));
    let reader = SealedJournal::open_bounded(original.path(), &governor).unwrap();
    let manifest = reader.manifest().clone();
    let stream = reader.descriptor().streams[0].stream_id.clone();
    assert_eq!(reader.read_chunk(&stream, 0).unwrap().payload, [0; 8]);
    let parked = original.0.join("parked-original.sqlite3");
    fs::rename(original.path(), &parked).unwrap();
    fs::copy(other.path(), original.path()).unwrap();
    assert!(reader.verify_source_identity().is_err());
    assert!(reader.inspect_snapshot().is_err());
    assert!(reader.read_ordinal(1).is_err());
    assert!(reader.read_chunk(&stream, 0).is_err());
    let mut seen = 0;
    assert!(
        reader
            .visit_chunks(|_, _| {
                seen += 1;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(seen, 0);
    assert_eq!(reader.manifest(), &manifest);
    drop(reader);
    assert_eq!(fs::read(parked).unwrap(), original_bytes);
    assert_eq!(fs::read(original.path()).unwrap(), replacement_bytes);
}

#[test]
fn same_inode_write_with_restored_mtime_is_not_an_unchanged_source() {
    use std::os::unix::fs::MetadataExt;
    let fixture = source();
    let before = fs::read(fixture.path()).unwrap();
    let metadata = fs::metadata(fixture.path()).unwrap();
    let governor = budget(Arc::new(AtomicBool::new(false)));
    let reader = SealedJournal::open_bounded(fixture.path(), &governor).unwrap();
    // This test deliberately tampers with its synthetic file. Closing this test
    // descriptor is not being used as a SQLite exclusion mechanism.
    let mut file = OpenOptions::new().write(true).open(fixture.path()).unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(&[before[before.len() - 1] ^ 1]).unwrap();
    file.set_times(FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    file.sync_all().unwrap();
    drop(file);
    let after = fs::metadata(fixture.path()).unwrap();
    assert_eq!(metadata.ino(), after.ino());
    assert_eq!(metadata.len(), after.len());
    assert_eq!(metadata.modified().unwrap(), after.modified().unwrap());
    assert!(reader.verify_source_identity().is_err());
    assert!(reader.inspect_snapshot().is_err());
    drop(reader);
    let mut expected = before;
    let last = expected.len() - 1;
    expected[last] ^= 1;
    assert_eq!(fs::read(fixture.path()).unwrap(), expected);
}

#[test]
fn a_new_sidecar_is_refused_without_repair_or_removal() {
    let fixture = source();
    let before = fs::read(fixture.path()).unwrap();
    let governor = budget(Arc::new(AtomicBool::new(false)));
    let reader = SealedJournal::open_bounded(fixture.path(), &governor).unwrap();
    let mut sidecar = fixture.path().into_os_string();
    sidecar.push("-journal");
    fs::write(&sidecar, b"retained synthetic unexpected sidecar").unwrap();
    assert!(reader.verify_source_identity().is_err());
    assert!(reader.inspect_snapshot().is_err());
    drop(reader);
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    assert_eq!(
        fs::read(sidecar).unwrap(),
        b"retained synthetic unexpected sidecar"
    );
}

#[test]
fn replacement_in_last_visitor_cannot_receive_a_complete_success() {
    let fixture = source();
    let other = source();
    let before = fs::read(fixture.path()).unwrap();
    let replacement = fs::read(other.path()).unwrap();
    let parked = fixture.0.join("parked.sqlite3");
    let governor = budget(Arc::new(AtomicBool::new(false)));
    let reader = SealedJournal::open_bounded(fixture.path(), &governor).unwrap();
    let mut seen = 0;
    assert!(
        reader
            .visit_chunks(|ordinal, _| {
                seen += 1;
                if ordinal == 3 {
                    fs::rename(fixture.path(), &parked)?;
                    fs::copy(other.path(), fixture.path())?;
                }
                Ok(())
            })
            .is_err()
    );
    assert_eq!(seen, 3);
    drop(reader);
    assert_eq!(fs::read(parked).unwrap(), before);
    assert_eq!(fs::read(fixture.path()).unwrap(), replacement);
}

#[test]
fn sticky_cancellation_precedes_a_later_path_failure() {
    let fixture = source();
    let before = fs::read(fixture.path()).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let governor = budget(cancelled.clone());
    let reader = SealedJournal::open_bounded(fixture.path(), &governor).unwrap();
    let parked = fixture.0.join("parked.sqlite3");
    fs::rename(fixture.path(), &parked).unwrap();
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        reader.verify_source_identity(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert!(matches!(
        reader.inspect_snapshot(),
        Err(RecordingError::InspectionCancelled)
    ));
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        reader.verify_source_identity(),
        Err(RecordingError::InspectionCancelled)
    ));
    drop(reader);
    assert_eq!(fs::read(parked).unwrap(), before);
}
