//! Synthetic regressions for reusing an authenticated sealed-prefix report.
//! No native performance authority or production source is involved.
use crate::{
    tests::{Fixture, descriptor, header},
    *,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn governor(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
        cancelled,
    )
}
fn files(fixture: &Fixture) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(&fixture.0)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
fn refusal<T>(result: Result<T>) -> RecordingError {
    match result {
        Ok(_) => panic!("Expected a retained-source refusal"),
        Err(error) => error,
    }
}
fn normal(chunks: u64, payload_bytes: usize) -> (Fixture, TakeDescriptor, SealedManifest) {
    let fixture = Fixture::new();
    let mut take = descriptor(true);
    let mut empty = take.streams[0].clone();
    empty.stream_id = "empty-second-stream".into();
    empty.segment_id = "empty-second-segment".into();
    take.streams.push(empty);
    let mut writer = Journal::create(fixture.path(), &take).unwrap();
    let payload = vec![0x31; payload_bytes];
    let frames = payload_bytes as u64 / 8;
    for sequence in 0..chunks {
        writer
            .append(
                &header(&take, sequence, sequence * frames, frames),
                &payload,
            )
            .unwrap();
    }
    let manifest = writer.seal("report-reuse-normal-fixture").unwrap();
    drop(writer);
    (fixture, take, manifest)
}

// The old inspection path independently scanned the selected prefix after its
// manifest verifier, whose normal path additionally scanned the entire table.
// Retain those direct scan oracles without calling the new reuse helper.
fn scan_oracle(
    fixture: &Fixture,
    take: &TakeDescriptor,
    manifest: &SealedManifest,
) -> RecoveryReport {
    let connection =
        Connection::open_with_flags(fixture.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection.execute_batch("BEGIN DEFERRED").unwrap();
    let mut report =
        crate::journal::scan(&connection, take, Some(manifest.body.chunk_count)).unwrap();
    assert!(report.problems.is_empty());
    assert_eq!(report.streams, manifest.body.streams);
    assert_eq!(report.verified_payload_bytes, manifest.body.payload_bytes);
    assert_eq!(report.chunk_index_sha256, manifest.body.chunk_index_sha256);
    if manifest.body.interrupted.is_none() {
        let complete = crate::journal::scan(&connection, take, None).unwrap();
        assert!(complete.problems.is_empty());
        assert_eq!(complete, report);
    }
    report.unknown_tail = manifest.body.interrupted.is_some();
    report
}

#[test]
fn normal_inspection_reuses_one_authentication_pass_under_a_conservative_work_cap() {
    let count = 8;
    let (fixture, take, manifest) = normal(count, MAX_AUDIO_CHUNK_BYTES);
    let expected = scan_oracle(&fixture, &take, &manifest);
    let before = files(&fixture);
    let physical = &before["take.sqlite3"];
    let physical_sha = format!("{:x}", Sha256::digest(physical));
    let original_sha = {
        let connection =
            Connection::open_with_flags(fixture.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        crate::inspection::original_rows_hash(&connection).unwrap()
    };
    let payload_bytes = count * MAX_AUDIO_CHUNK_BYTES as u64;
    // Includes one read/hash of every chunk, the full original-row digest and
    // complete physical-file read/hash, with ample metadata credit. The former
    // three chunk-authentication passes require at least 10x these PCM bytes.
    let (budget, _) = governor(InspectionLimits {
        max_work_bytes: payload_bytes * 7 + 1024 * 1024,
        ..Default::default()
    });
    let reader = CaptureReader::open_bounded(fixture.path(), &budget).unwrap();
    assert_eq!(reader.inspection().report, expected);
    assert_eq!(reader.inspection().manifest.as_ref(), Some(&manifest));
    assert_eq!(reader.inspection().original_rows_sha256, original_sha);
    assert_eq!(reader.inspection().journal_sha256, physical_sha);
    assert_eq!(reader.inspection().byte_length, physical.len() as u64);
    assert_eq!(budget.usage().chunk_visits, count);
    assert!(budget.usage().work_bytes >= payload_bytes * 6);
    drop(reader);
    assert_eq!(files(&fixture), before);

    let (budget, _) = governor(InspectionLimits {
        max_work_bytes: payload_bytes * 3,
        ..Default::default()
    });
    let sealed = SealedJournal::open_bounded(fixture.path(), &budget).unwrap();
    assert_eq!(sealed.manifest(), &manifest);
    assert_eq!(budget.usage().chunk_visits, count);
    drop(sealed);
    assert_eq!(files(&fixture), before);
}

#[test]
fn empty_normal_journal_keeps_the_exact_report_and_manifest() {
    let (fixture, take, manifest) = normal(0, 8);
    let expected = scan_oracle(&fixture, &take, &manifest);
    let before = files(&fixture);
    let (budget, _) = governor(InspectionLimits::default());
    let reader = CaptureReader::open_bounded(fixture.path(), &budget).unwrap();
    assert_eq!(reader.inspection().report, expected);
    assert_eq!(reader.inspection().manifest.as_ref(), Some(&manifest));
    assert_eq!(budget.usage().chunk_visits, 0);
    drop(reader);
    assert_eq!(files(&fixture), before);
}

#[test]
fn every_declared_durable_prefix_including_an_empty_nonfirst_stream_remains_checked() {
    for stream in ["source-1", "empty-second-stream"] {
        for field in [
            "next_sequence",
            "source_end",
            "recorded_units",
            "gap_units",
            "gap_chunks",
            "missing",
        ] {
            let (fixture, _, _) = normal(2, 8);
            let connection = Connection::open(fixture.path()).unwrap();
            if field == "missing" {
                connection
                    .execute("DELETE FROM stream_prefix WHERE stream_id=?1", [stream])
                    .unwrap();
            } else {
                // Both SQL identifiers are a fixed test-only allowlist.
                connection
                    .execute(
                        &format!("UPDATE stream_prefix SET {field}={field}+1 WHERE stream_id=?1"),
                        [stream],
                    )
                    .unwrap();
            }
            drop(connection);
            let before = files(&fixture);
            let (budget, _) = governor(InspectionLimits::default());
            assert!(
                matches!(
                    refusal(CaptureReader::open_bounded(fixture.path(), &budget)),
                    RecordingError::Invalid(_)
                ),
                "{stream}/{field}"
            );
            assert_eq!(files(&fixture), before);
        }
    }
}

#[test]
fn extra_missing_reordered_and_corrupt_normal_originals_still_refuse() {
    for mutation in [
        "DROP TRIGGER chunk_only_recording; INSERT INTO chunks(ordinal,stream_id,sequence,header,payload,sha256) SELECT 3,stream_id,2,header,zeroblob(8),sha256 FROM chunks WHERE ordinal=1",
        "DROP TRIGGER chunk_only_recording; INSERT INTO chunks(ordinal,stream_id,sequence,header,payload,sha256) SELECT 0,stream_id,2,header,zeroblob(8),sha256 FROM chunks WHERE ordinal=1",
        "DROP TRIGGER immutable_chunk_delete; DELETE FROM chunks WHERE ordinal=2",
        "DROP TRIGGER immutable_chunk_update; UPDATE chunks SET ordinal=3 WHERE ordinal=2",
        "DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(8) WHERE ordinal=1",
    ] {
        let (fixture, _, _) = normal(2, 8);
        Connection::open(fixture.path())
            .unwrap()
            .execute_batch(mutation)
            .unwrap();
        let before = files(&fixture);
        let (budget, _) = governor(InspectionLimits::default());
        assert!(
            matches!(
                refusal(CaptureReader::open_bounded(fixture.path(), &budget)),
                RecordingError::Invalid(_)
            ),
            "{mutation}"
        );
        assert_eq!(files(&fixture), before);
    }
}

#[test]
fn recovered_selection_preserves_and_binds_bad_unselected_bytes_and_unknown_columns() {
    let fixture = Fixture::new();
    let take = descriptor(true);
    let mut writer = Journal::create(fixture.path(), &take).unwrap();
    for sequence in 0..3 {
        writer
            .append(
                &header(&take, sequence, sequence, 1),
                &[sequence as u8 + 1; 8],
            )
            .unwrap();
    }
    drop(writer);
    Connection::open(fixture.path()).unwrap().execute_batch(
        "DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(8) WHERE ordinal=3; ALTER TABLE chunks ADD COLUMN opaque_vendor BLOB; UPDATE chunks SET opaque_vendor=x'00ff1234' WHERE ordinal=3"
    ).unwrap();
    let reader = CaptureReader::open(fixture.path()).unwrap();
    assert_eq!(reader.inspection().report.verified_chunks, 2);
    let original_sha = reader.inspection().original_rows_sha256.clone();
    let first = reader.read_ordinal(1).unwrap();
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.recording-index.v1\0");
    hash.update(take.sha256().unwrap().as_bytes());
    hash.update(1_u64.to_le_bytes());
    hash.update(first.sha256.as_bytes());
    let selection = RecoveryPrefix {
        chunk_count: 1,
        chunk_index_sha256: format!("{:x}", hash.finalize()),
    };
    let (manifest, replayed) = reader
        .recover_prefix("report-reuse-selected", &selection)
        .unwrap();
    assert!(!replayed);
    let expected = scan_oracle(&fixture, &take, &manifest);
    let before = files(&fixture);
    let (budget, _) = governor(InspectionLimits::default());
    let reader = CaptureReader::open_bounded(fixture.path(), &budget).unwrap();
    assert_eq!(reader.inspection().report, expected);
    assert!(reader.inspection().report.unknown_tail);
    assert_eq!(reader.inspection().manifest.as_ref(), Some(&manifest));
    assert_eq!(reader.inspection().original_rows_sha256, original_sha);
    assert_eq!(budget.usage().chunk_visits, 1);
    let first_inspection = reader.inspection().clone();
    drop(reader);
    assert_eq!(files(&fixture), before);

    Connection::open(fixture.path()).unwrap().execute_batch(
        "UPDATE chunks SET payload=x'0101010101010101',opaque_vendor=x'00ff1235' WHERE ordinal=3"
    ).unwrap();
    let changed_bytes = files(&fixture);
    let (budget, _) = governor(InspectionLimits::default());
    let changed = CaptureReader::open_bounded(fixture.path(), &budget).unwrap();
    assert_eq!(changed.inspection().report, first_inspection.report);
    assert_eq!(changed.inspection().manifest, first_inspection.manifest);
    assert_ne!(
        changed.inspection().original_rows_sha256,
        first_inspection.original_rows_sha256
    );
    assert_ne!(
        changed.inspection().journal_sha256,
        first_inspection.journal_sha256
    );
    drop(changed);
    assert_eq!(files(&fixture), changed_bytes);

    Connection::open(fixture.path()).unwrap().execute_batch(
        "DROP TRIGGER immutable_recovery_index_update; UPDATE recovery_index SET sha256=printf('%064d',0) WHERE ordinal=1"
    ).unwrap();
    let before = files(&fixture);
    let (budget, _) = governor(InspectionLimits::default());
    assert!(matches!(
        refusal(CaptureReader::open_bounded(fixture.path(), &budget)),
        RecordingError::Invalid(_)
    ));
    assert_eq!(files(&fixture), before);
}

#[test]
fn report_reuse_preserves_sticky_work_row_vm_and_cancellation_refusals() {
    let (fixture, _, _) = normal(3, 32 * 1024);
    let before = files(&fixture);
    let (probe, _) = governor(InspectionLimits::default());
    drop(CaptureReader::open_bounded(fixture.path(), &probe).unwrap());
    for resource in [
        InspectionResource::WorkBytes,
        InspectionResource::RowVisits,
        InspectionResource::VmSteps,
    ] {
        let mut limits = InspectionLimits::default();
        match resource {
            InspectionResource::WorkBytes => limits.max_work_bytes = probe.usage().work_bytes - 1,
            InspectionResource::RowVisits => limits.max_row_visits = probe.usage().row_visits - 1,
            InspectionResource::VmSteps => limits.max_vm_steps = 0,
            _ => unreachable!(),
        }
        let (budget, cancelled) = governor(limits);
        assert!(
            matches!(refusal(CaptureReader::open_bounded(fixture.path(), &budget)), RecordingError::InspectionLimit { resource: actual, .. } if actual == resource)
        );
        cancelled.store(true, Ordering::Release);
        assert!(
            matches!(budget.checkpoint(), Err(RecordingError::InspectionLimit { resource: actual, .. }) if actual == resource)
        );
        assert_eq!(files(&fixture), before);
    }
    let (budget, cancelled) = governor(InspectionLimits::default());
    let reader = CaptureReader::open_bounded(fixture.path(), &budget).unwrap();
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        reader.read_ordinal(1),
        Err(RecordingError::InspectionCancelled)
    ));
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    drop(reader);
    assert_eq!(files(&fixture), before);
}
