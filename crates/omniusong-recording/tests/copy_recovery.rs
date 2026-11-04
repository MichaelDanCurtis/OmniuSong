//! Independent generic-journal copy tests. Synthetic files only; these tests
//! establish neither six-stream performance admission nor native seal authority.
#![cfg(unix)]
use omniusong_recording::*;
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const ACTION: &str = "11111111111111111111111111111111";

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    parent: PathBuf,
}
impl Fixture {
    fn new(bad_tail: bool, sealed: bool) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/copy-recovery-independent")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir(root.join("source-actions")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let source = root.join("source/take.sqlite3");
        let parent = root.join("source-actions");
        let descriptor = descriptor();
        let mut journal = Journal::create(&source, &descriptor).unwrap();
        for sequence in 0..3 {
            journal
                .append(
                    &header(&descriptor, sequence),
                    &[0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat(),
                )
                .unwrap();
        }
        if sealed {
            journal.seal("synthetic-normal-source").unwrap();
        }
        drop(journal);
        if bad_tail {
            let connection = Connection::open(&source).unwrap();
            let trigger: String = connection
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name='immutable_chunk_update'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            connection.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=3;").unwrap();
            // Restore the exact canonical program. A bad original row must not
            // accidentally turn this into the unsupported-schema test instead.
            connection.execute_batch(&trigger).unwrap();
        }
        Self {
            root,
            source,
            parent,
        }
    }
    fn bounded(&self, budget: &InspectionBudget) -> CaptureReader {
        CaptureReader::open_bounded(&self.source, budget).unwrap()
    }
    fn source_tree(&self) -> BTreeMap<PathBuf, Entry> {
        tree(self.source.parent().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Copy-recovery fixture retained at {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn descriptor() -> TakeDescriptor {
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(), segment_id: "synthetic-pcm".into(),
            source_id: "software-fixture".into(), source_name: "Synthetic copy; no device".into(),
            source_connection_generation: 1,
            host_clock: HostClock { domain: ClockDomainId("synthetic-frames".into()), epoch: ClockEpochId("original-epoch".into()), ticks_per_second_numerator: 48_000, ticks_per_second_denominator: 1 },
            kind: StreamKind::Audio { format: PcmFormat { sample_rate: 48_000, channels: 1, encoding: PcmEncoding::F32Le, layout: PcmLayout::Interleaved }, audio_epoch: ClockEpochId("original-audio".into()) },
            initial_source_position: 100,
        }],
        provenance: BTreeMap::from([("opaque-original".into(), serde_json::from_str(r#"{"large":123456789012345678901234567890,"fraction":0.12345678901234567890123456789,"text":"retained\u0000text"}"#).unwrap())]),
    }
}
fn header(descriptor: &TakeDescriptor, sequence: u64) -> ChunkHeader {
    let stream = &descriptor.streams[0];
    let stamp = HostTimestamp {
        domain: stream.host_clock.domain.clone(),
        epoch: stream.host_clock.epoch.clone(),
        ticks: sequence * 2,
    };
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: descriptor.take_id.clone(),
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
fn budget() -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap(),
        cancelled,
    )
}
fn failure<T>(result: std::result::Result<T, CopyRecoveryError>) -> CopyRecoveryError {
    match result {
        Ok(_) => panic!("copy operation unexpectedly succeeded"),
        Err(error) => error,
    }
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Directory,
    File(Vec<u8>),
    Link(PathBuf),
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
        for item in fs::read_dir(path).unwrap() {
            let item = item.unwrap();
            let path = item.path();
            let key = path.strip_prefix(root).unwrap().to_owned();
            let kind = item.file_type().unwrap();
            if kind.is_symlink() {
                entries.insert(key, Entry::Link(fs::read_link(&path).unwrap()));
            } else if kind.is_dir() {
                entries.insert(key, Entry::Directory);
                visit(root, &path, entries);
            } else {
                entries.insert(key, Entry::File(fs::read(path).unwrap()));
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}
#[derive(Debug, PartialEq, Eq)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
fn original_rows(path: &Path) -> BTreeMap<String, Vec<Vec<Cell>>> {
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !sidecar(path, suffix).exists(),
            "test oracle must not open sidecar-bearing SQLite"
        );
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    connection
        .execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED;")
        .unwrap();
    let mut result = BTreeMap::new();
    for (name, query) in [
        ("chunks", "SELECT rowid,* FROM chunks ORDER BY rowid"),
        (
            "stream_prefix",
            "SELECT rowid,* FROM stream_prefix ORDER BY rowid",
        ),
        (
            "descriptor",
            "SELECT singleton,descriptor,descriptor_sha256 FROM take_meta",
        ),
        (
            "sequence",
            "SELECT rowid,* FROM sqlite_sequence ORDER BY rowid",
        ),
        (
            "schema",
            "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name",
        ),
    ] {
        let mut statement = connection.prepare(query).unwrap();
        let count = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..count)
                    .map(|column| {
                        Ok(match row.get_ref(column)? {
                            ValueRef::Null => Cell::Null,
                            ValueRef::Integer(value) => Cell::Integer(value),
                            ValueRef::Real(value) => Cell::Real(value.to_bits()),
                            ValueRef::Text(value) => Cell::Text(value.to_vec()),
                            ValueRef::Blob(value) => Cell::Blob(value.to_vec()),
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        result.insert(name.to_owned(), rows);
    }
    result
}
fn selection(reader: &CaptureReader, count: u64) -> RecoveryPrefix {
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.recording-index.v1\0");
    hash.update(reader.inspection().descriptor_sha256.as_bytes());
    for ordinal in 1..=count {
        let row = reader.read_ordinal(ordinal).unwrap();
        hash.update(ordinal.to_le_bytes());
        hash.update(row.sha256.as_bytes());
    }
    RecoveryPrefix {
        chunk_count: count,
        chunk_index_sha256: format!("{:x}", hash.finalize()),
    }
}
fn state(path: &Path) -> String {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection.execute_batch("PRAGMA query_only=ON;").unwrap();
    connection
        .query_row("SELECT state FROM take_meta", [], |row| row.get(0))
        .unwrap()
}
fn owner_is_free(path: &Path) {
    let owner = OpenOptions::new()
        .read(true)
        .write(true)
        .open(sidecar(path, ".owner.lock"))
        .unwrap();
    owner
        .try_lock()
        .expect("copy operation leaked an owner after drop");
}

#[cfg(unix)]
#[test]
fn full_shorter_and_bad_tail_copies_preserve_all_bytes_and_only_publish_recovered_results() {
    for (bad_tail, selected) in [(false, 3), (false, 1), (true, 1)] {
        let fixture = Fixture::new(bad_tail, false);
        let source = fixture.source_tree();
        let bytes = fs::read(&fixture.source).unwrap();
        let rows = original_rows(&fixture.source);
        let (budget, _) = budget();
        let reader = fixture.bounded(&budget);
        let reviewed = reader.inspection().clone();
        let initial_usage = budget.usage();
        assert_eq!(
            reviewed.report.verified_chunks,
            if bad_tail { 2 } else { 3 }
        );
        let copy = RecoveryCopy::create(reader, &fixture.parent, ACTION).unwrap();
        let paths = copy.paths().clone();
        assert_eq!(paths.action, fixture.parent.join(ACTION));
        assert_eq!(paths.evidence, paths.action.join("evidence/take.sqlite3"));
        assert_eq!(paths.working, paths.action.join("recovery/take.sqlite3"));
        assert_eq!(fs::read(&paths.evidence).unwrap(), bytes);
        assert_eq!(fs::read(&paths.working).unwrap(), bytes);
        let chosen = selection(copy.reader(), selected);
        let receipt = copy.finalize(&chosen, "synthetic-recovered-copy").unwrap();
        assert_eq!(receipt.paths, paths);
        assert_eq!(
            receipt.manifest.body.operation_id,
            "synthetic-recovered-copy"
        );
        assert_eq!(receipt.manifest.body.chunk_count, selected);
        assert_eq!(
            receipt.manifest.body.chunk_index_sha256,
            chosen.chunk_index_sha256
        );
        assert_eq!(receipt.manifest.body.descriptor, reviewed.descriptor);
        let interruption = receipt
            .manifest
            .body
            .interrupted
            .as_ref()
            .expect("copy cannot publish normal seal");
        assert!(interruption.report.unknown_tail);
        assert_eq!(
            interruption.report.verified_chunks,
            reviewed.report.verified_chunks
        );
        assert_eq!(receipt.inspection.report.state, TakeState::Sealed);
        assert!(receipt.inspection.report.unknown_tail);
        assert_eq!(
            receipt.inspection.manifest.as_ref(),
            Some(&receipt.manifest)
        );
        assert_eq!(
            receipt.inspection.original_rows_sha256,
            reviewed.original_rows_sha256
        );
        assert_eq!(
            original_rows(&paths.working),
            rows,
            "good unselected and damaged raw rows must remain"
        );
        assert_eq!(fixture.source_tree(), source);
        assert_eq!(fs::read(&paths.evidence).unwrap(), bytes);
        assert_eq!(state(&fixture.source), "recording");
        assert_eq!(receipt.usage, budget.usage());
        assert!(
            receipt.usage.work_bytes >= initial_usage.work_bytes + 4 * bytes.len() as u64,
            "both complete copies must charge their reads and writes"
        );
        assert!(receipt.usage.file_bytes >= bytes.len() as u64);
        let recovered_bytes = fs::read(&paths.working).unwrap();
        let mut journal = Journal::open(&paths.working).unwrap();
        assert!(journal.seal("cannot-upgrade-recovered-to-normal").is_err());
        drop(journal);
        assert_eq!(fs::read(&paths.working).unwrap(), recovered_bytes);
        owner_is_free(&fixture.source);
        owner_is_free(&paths.working);
    }
}

#[test]
fn unbounded_or_sealed_readers_and_invalid_action_parents_or_ids_create_nothing() {
    for sealed in [false, true] {
        let fixture = Fixture::new(false, sealed);
        let before = tree(&fixture.root);
        let reader = if sealed {
            let (budget, _) = budget();
            fixture.bounded(&budget)
        } else {
            CaptureReader::open(&fixture.source).unwrap()
        };
        let error = failure(RecoveryCopy::create(reader, &fixture.parent, ACTION));
        assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
        assert_eq!(tree(&fixture.root), before);
    }
    let fixture = Fixture::new(false, false);
    let bad_ids = [
        String::new(),
        "0".repeat(32),
        "A".repeat(32),
        "1".repeat(31),
        "../outside-action".into(),
        fixture.root.join("absolute-action").display().to_string(),
    ];
    for bad in bad_ids {
        let before = tree(&fixture.root);
        let (budget, _) = budget();
        let error = failure(RecoveryCopy::create(
            fixture.bounded(&budget),
            &fixture.parent,
            &bad,
        ));
        assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
        assert_eq!(tree(&fixture.root), before);
    }
    fs::write(
        fixture.root.join("ordinary-file"),
        b"retained parent refusal",
    )
    .unwrap();
    let child_in_source = fixture.source.parent().unwrap().join("existing-child");
    fs::create_dir(&child_in_source).unwrap();
    for parent in [
        fixture.root.join("missing-parent"),
        fixture.root.join("ordinary-file"),
        fixture.parent.join("../source-actions"),
        fixture.source.parent().unwrap().to_owned(),
        child_in_source,
    ] {
        let before = tree(&fixture.root);
        let (budget, _) = budget();
        let error = failure(RecoveryCopy::create(
            fixture.bounded(&budget),
            parent,
            ACTION,
        ));
        assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
        assert_eq!(tree(&fixture.root), before);
    }
}

#[cfg(unix)]
#[test]
fn existing_action_directories_and_partial_artifacts_are_never_overwritten_or_removed() {
    for stage in [
        "directory",
        "evidence-partial",
        "working-partial",
        "completed-copy",
    ] {
        let fixture = Fixture::new(false, false);
        let action = fixture.parent.join(ACTION);
        fs::create_dir(&action).unwrap();
        if stage != "directory" {
            fs::create_dir(action.join("evidence")).unwrap();
            fs::write(
                action.join("evidence/take.sqlite3"),
                b"retained incomplete evidence\0\xff",
            )
            .unwrap();
        }
        if matches!(stage, "working-partial" | "completed-copy") {
            fs::create_dir(action.join("recovery")).unwrap();
            fs::write(
                action.join("recovery/take.sqlite3"),
                b"retained uncertain working result",
            )
            .unwrap();
        }
        if stage == "completed-copy" {
            fs::copy(&fixture.source, action.join("evidence/take.sqlite3")).unwrap();
            fs::copy(&fixture.source, action.join("recovery/take.sqlite3")).unwrap();
        }
        let before = tree(&fixture.root);
        let (budget, _) = budget();
        let error = failure(RecoveryCopy::create(
            fixture.bounded(&budget),
            &fixture.parent,
            ACTION,
        ));
        assert_eq!(
            error.boundary,
            CopyRecoveryBoundary::ExistingArtifacts,
            "{stage}"
        );
        assert_eq!(error.paths.action, action);
        assert_eq!(
            tree(&fixture.root),
            before,
            "existing {stage} artifacts changed"
        );
    }
}

#[cfg(unix)]
#[test]
fn cancellation_after_reader_or_copy_is_sticky_and_keeps_exact_source_and_artifacts() {
    for after_copy in [false, true] {
        let fixture = Fixture::new(false, false);
        let source = fixture.source_tree();
        let (budget, cancelled) = budget();
        let reader = fixture.bounded(&budget);
        if after_copy {
            let copy = RecoveryCopy::create(reader, &fixture.parent, ACTION).unwrap();
            let chosen = selection(copy.reader(), 1);
            let paths = copy.paths().clone();
            let before = tree(&fixture.root);
            cancelled.store(true, Ordering::Release);
            assert!(matches!(
                budget.checkpoint(),
                Err(RecordingError::InspectionCancelled)
            ));
            cancelled.store(false, Ordering::Release);
            let error = failure(copy.finalize(&chosen, "cancelled-copy"));
            assert!(matches!(error.cause, RecordingError::InspectionCancelled));
            assert_eq!(error.boundary, CopyRecoveryBoundary::WorkingVerified);
            assert_eq!(*error.paths, paths);
            assert_eq!(tree(&fixture.root), before);
        } else {
            let before = tree(&fixture.root);
            cancelled.store(true, Ordering::Release);
            assert!(matches!(
                budget.checkpoint(),
                Err(RecordingError::InspectionCancelled)
            ));
            cancelled.store(false, Ordering::Release);
            let error = failure(RecoveryCopy::create(reader, &fixture.parent, ACTION));
            assert!(matches!(error.cause, RecordingError::InspectionCancelled));
            assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
            assert_eq!(tree(&fixture.root), before);
        }
        assert!(matches!(
            budget.charge_work(u64::MAX),
            Err(RecordingError::InspectionCancelled)
        ));
        assert_eq!(fixture.source_tree(), source);
        owner_is_free(&fixture.source);
    }
}

#[cfg(unix)]
#[test]
fn work_refusal_after_reader_and_before_finalize_never_resets_the_cumulative_governor() {
    for after_copy in [false, true] {
        let fixture = Fixture::new(false, false);
        let source = fixture.source_tree();
        let (budget, cancelled) = budget();
        let reader = fixture.bounded(&budget);
        if after_copy {
            let copy = RecoveryCopy::create(reader, &fixture.parent, ACTION).unwrap();
            let chosen = selection(copy.reader(), 1);
            let before = tree(&fixture.root);
            let remaining = InspectionLimits::default().max_work_bytes - budget.usage().work_bytes;
            budget.charge_work(remaining - 1).unwrap();
            let error = failure(copy.finalize(&chosen, "work-limited-copy"));
            assert!(matches!(
                error.cause,
                RecordingError::InspectionLimit {
                    resource: InspectionResource::WorkBytes,
                    ..
                }
            ));
            assert_eq!(error.boundary, CopyRecoveryBoundary::WorkingVerified);
            assert_eq!(tree(&fixture.root), before);
        } else {
            let before = tree(&fixture.root);
            assert!(matches!(
                budget.charge_work(u64::MAX),
                Err(RecordingError::InspectionLimit {
                    resource: InspectionResource::WorkBytes,
                    ..
                })
            ));
            let error = failure(RecoveryCopy::create(reader, &fixture.parent, ACTION));
            assert!(matches!(
                error.cause,
                RecordingError::InspectionLimit {
                    resource: InspectionResource::WorkBytes,
                    ..
                }
            ));
            assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
            assert_eq!(tree(&fixture.root), before);
        }
        cancelled.store(true, Ordering::Release);
        assert!(
            matches!(
                budget.checkpoint(),
                Err(RecordingError::InspectionLimit {
                    resource: InspectionResource::WorkBytes,
                    ..
                })
            ),
            "a later cancellation erased the first refusal"
        );
        assert_eq!(fixture.source_tree(), source);
    }
}

#[cfg(unix)]
#[test]
fn source_and_working_sidecars_are_refused_before_sqlite_access_without_cleanup() {
    for suffix in ["-journal", "-wal", "-shm"] {
        for working_sidecar in [false, true] {
            let fixture = Fixture::new(false, false);
            let (budget, _) = budget();
            let reader = fixture.bounded(&budget);
            if working_sidecar {
                let copy = RecoveryCopy::create(reader, &fixture.parent, ACTION).unwrap();
                let chosen = selection(copy.reader(), 1);
                let paths = copy.paths().clone();
                fs::write(
                    sidecar(&paths.working, suffix),
                    b"retained ambiguous SQLite sidecar\0\xff",
                )
                .unwrap();
                let before = tree(&fixture.root);
                let error = failure(copy.finalize(&chosen, "sidecar-working"));
                assert_eq!(error.boundary, CopyRecoveryBoundary::WorkingVerified);
                assert_eq!(
                    tree(&fixture.root),
                    before,
                    "working {suffix} was changed/cleaned"
                );
            } else {
                fs::write(
                    sidecar(&fixture.source, suffix),
                    b"retained source sidecar\0\xff",
                )
                .unwrap();
                let before = tree(&fixture.root);
                let error = failure(RecoveryCopy::create(reader, &fixture.parent, ACTION));
                assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
                assert_eq!(
                    tree(&fixture.root),
                    before,
                    "source {suffix} was changed/cleaned"
                );
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn changed_source_bytes_after_review_or_copy_are_refused_without_changing_any_evidence() {
    for after_copy in [false, true] {
        let fixture = Fixture::new(false, false);
        let (budget, _) = budget();
        let reader = fixture.bounded(&budget);
        if after_copy {
            let copy = RecoveryCopy::create(reader, &fixture.parent, ACTION).unwrap();
            let chosen = selection(copy.reader(), 1);
            OpenOptions::new()
                .append(true)
                .open(&fixture.source)
                .unwrap()
                .write_all(b"synthetic physical tail change")
                .unwrap();
            let before = tree(&fixture.root);
            let error = failure(copy.finalize(&chosen, "source-changed-after-copy"));
            assert_eq!(error.boundary, CopyRecoveryBoundary::WorkingVerified);
            assert_eq!(tree(&fixture.root), before);
        } else {
            OpenOptions::new()
                .append(true)
                .open(&fixture.source)
                .unwrap()
                .write_all(b"synthetic physical tail change")
                .unwrap();
            let before = tree(&fixture.root);
            let error = failure(RecoveryCopy::create(reader, &fixture.parent, ACTION));
            assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
            assert_eq!(tree(&fixture.root), before);
        }
    }
}

#[cfg(unix)]
#[test]
fn byte_identical_replacement_and_source_or_owner_aliases_cannot_replace_the_open_identity() {
    use std::os::unix::fs::symlink;
    for target in ["data", "owner"] {
        for change in ["replace", "hardlink", "symlink"] {
            let fixture = Fixture::new(false, false);
            let (budget, _) = budget();
            let reader = fixture.bounded(&budget);
            let path = if target == "data" {
                fixture.source.clone()
            } else {
                sidecar(&fixture.source, ".owner.lock")
            };
            let bytes = fs::read(&path).unwrap();
            let retained = path.with_file_name(format!("retained-{target}-{change}"));
            match change {
                "replace" => {
                    fs::rename(&path, &retained).unwrap();
                    fs::write(&path, &bytes).unwrap();
                }
                "hardlink" => fs::hard_link(&path, &retained).unwrap(),
                "symlink" => {
                    fs::rename(&path, &retained).unwrap();
                    symlink(&retained, &path).unwrap();
                }
                _ => unreachable!(),
            }
            let before = tree(&fixture.root);
            let error = failure(RecoveryCopy::create(reader, &fixture.parent, ACTION));
            assert_eq!(
                error.boundary,
                CopyRecoveryBoundary::NoArtifacts,
                "{target} {change}"
            );
            assert_eq!(
                tree(&fixture.root),
                before,
                "refusal cleaned/replaced {target} {change}"
            );
        }
    }
    let fixture = Fixture::new(false, false);
    let redirected_parent = fixture.root.join("source-actions-alias");
    symlink(&fixture.parent, &redirected_parent).unwrap();
    let (budget, _) = budget();
    let before = tree(&fixture.root);
    let error = failure(RecoveryCopy::create(
        fixture.bounded(&budget),
        &redirected_parent,
        ACTION,
    ));
    assert_eq!(error.boundary, CopyRecoveryBoundary::NoArtifacts);
    assert_eq!(tree(&fixture.root), before);
}

#[cfg(unix)]
#[test]
fn replaced_evidence_working_or_owner_paths_after_creation_refuse_without_cleanup() {
    use std::os::unix::fs::symlink;
    for target in ["source", "evidence", "working", "working-owner"] {
        for change in ["replace", "hardlink", "symlink"] {
            let fixture = Fixture::new(false, false);
            let (budget, _) = budget();
            let copy =
                RecoveryCopy::create(fixture.bounded(&budget), &fixture.parent, ACTION).unwrap();
            let chosen = selection(copy.reader(), 1);
            let paths = copy.paths().clone();
            let path = match target {
                "source" => fixture.source.clone(),
                "evidence" => paths.evidence.clone(),
                "working" => paths.working.clone(),
                "working-owner" => sidecar(&paths.working, ".owner.lock"),
                _ => unreachable!(),
            };
            let bytes = fs::read(&path).unwrap();
            let retained = path.with_file_name(format!("retained-{target}-{change}"));
            match change {
                "replace" => {
                    fs::rename(&path, &retained).unwrap();
                    fs::write(&path, &bytes).unwrap();
                }
                "hardlink" => fs::hard_link(&path, &retained).unwrap(),
                "symlink" => {
                    fs::rename(&path, &retained).unwrap();
                    symlink(&retained, &path).unwrap();
                }
                _ => unreachable!(),
            }
            let before = tree(&fixture.root);
            let error = failure(copy.finalize(&chosen, "changed-artifact-identity"));
            assert_eq!(
                error.boundary,
                CopyRecoveryBoundary::WorkingVerified,
                "{target} {change}"
            );
            assert_eq!(
                tree(&fixture.root),
                before,
                "refusal rewrote retained {target} {change}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn opaque_column_or_extra_trigger_is_copied_exactly_but_refuses_mutation_before_writer_open() {
    for extension in [
        "ALTER TABLE chunks ADD COLUMN \"opaque \"\"original\" BLOB DEFAULT X'00ff817f';",
        "CREATE TRIGGER opaque_retained_program AFTER INSERT ON chunks BEGIN SELECT 1; END;",
    ] {
        let fixture = Fixture::new(false, false);
        Connection::open(&fixture.source)
            .unwrap()
            .execute_batch(extension)
            .unwrap();
        let bytes = fs::read(&fixture.source).unwrap();
        let rows = original_rows(&fixture.source);
        let source = fixture.source_tree();
        let (budget, _) = budget();
        let copy = RecoveryCopy::create(fixture.bounded(&budget), &fixture.parent, ACTION).unwrap();
        let chosen = selection(copy.reader(), 1);
        let paths = copy.paths().clone();
        assert_eq!(fs::read(&paths.evidence).unwrap(), bytes);
        assert_eq!(fs::read(&paths.working).unwrap(), bytes);
        assert_eq!(original_rows(&paths.working), rows);
        let before = tree(&fixture.root);
        let error = failure(copy.finalize(&chosen, "unsupported-schema-copy"));
        assert_eq!(error.boundary, CopyRecoveryBoundary::WorkingVerified);
        assert!(matches!(error.cause, RecordingError::Invalid(_)));
        assert_eq!(
            tree(&fixture.root),
            before,
            "unsupported schema was mutated or cleaned"
        );
        assert_eq!(fixture.source_tree(), source);
    }
}

#[cfg(unix)]
#[test]
fn source_and_working_owners_stay_held_until_drop_which_retains_both_unsealed_copies() {
    let fixture = Fixture::new(false, false);
    let source = fixture.source_tree();
    let (budget, _) = budget();
    let copy = RecoveryCopy::create(fixture.bounded(&budget), &fixture.parent, ACTION).unwrap();
    let paths = copy.paths().clone();
    let before = tree(&fixture.root);
    for path in [&fixture.source, &paths.working] {
        let (other_budget, _) = self::budget();
        assert!(matches!(
            CaptureReader::open_bounded(path, &other_budget),
            Err(RecordingError::Owned)
        ));
        assert!(matches!(Journal::open(path), Err(RecordingError::Owned)));
    }
    assert_eq!(tree(&fixture.root), before);
    drop(copy);
    assert_eq!(
        tree(&fixture.root),
        before,
        "dropping a prepared copy must retain its artifacts"
    );
    owner_is_free(&fixture.source);
    owner_is_free(&paths.working);
    assert_eq!(state(&paths.working), "recording");
    assert_eq!(fixture.source_tree(), source);
    let (new_budget, _) = self::budget();
    let error = failure(RecoveryCopy::create(
        fixture.bounded(&new_budget),
        &fixture.parent,
        ACTION,
    ));
    assert_eq!(error.boundary, CopyRecoveryBoundary::ExistingArtifacts);
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn old_bounded_recover_and_recover_prefix_guards_do_not_authorize_in_place_mutation() {
    let fixture = Fixture::new(false, false);
    let before = tree(&fixture.root);
    let (budget, _) = budget();
    let reader = fixture.bounded(&budget);
    let chosen = selection(&reader, 1);
    assert!(
        reader
            .recover_prefix("not-a-copy-capability", &chosen)
            .is_err()
    );
    assert_eq!(tree(&fixture.root), before);
    let (budget, _) = self::budget();
    assert!(
        fixture
            .bounded(&budget)
            .recover("not-a-copy-capability")
            .is_err()
    );
    assert_eq!(tree(&fixture.root), before);
    owner_is_free(&fixture.source);
}

#[cfg(unix)]
#[test]
fn bad_selected_counts_hashes_and_operations_never_change_source_or_evidence() {
    for invalid in [
        "empty",
        "too-many",
        "wrong-hash",
        "uppercase-hash",
        "bad-operation",
    ] {
        let fixture = Fixture::new(false, false);
        let source = fixture.source_tree();
        let (budget, _) = budget();
        let copy = RecoveryCopy::create(fixture.bounded(&budget), &fixture.parent, ACTION).unwrap();
        let paths = copy.paths().clone();
        let mut chosen = selection(copy.reader(), 1);
        match invalid {
            "empty" => chosen.chunk_count = 0,
            "too-many" => chosen.chunk_count = 4,
            "wrong-hash" => chosen.chunk_index_sha256 = "0".repeat(64),
            "uppercase-hash" => chosen.chunk_index_sha256 = "A".repeat(64),
            "bad-operation" => {}
            _ => unreachable!(),
        }
        let before = tree(&fixture.root);
        let operation = if invalid == "bad-operation" {
            ""
        } else {
            "bad-selection-copy"
        };
        let error = failure(copy.finalize(&chosen, operation));
        assert!(matches!(
            error.boundary,
            CopyRecoveryBoundary::WorkingVerified | CopyRecoveryBoundary::RecoveryMayHaveWritten
        ));
        assert_eq!(
            tree(&fixture.root),
            before,
            "invalid {invalid} must not modify even the working transaction"
        );
        assert_eq!(fixture.source_tree(), source);
        assert_eq!(state(&paths.working), "recording");
    }
}

#[test]
fn vm_limit_sweep_retains_post_commit_uncertainty_and_never_relabels_it_as_prewrite() {
    let baseline = Fixture::new(false, false);
    let (baseline_budget, _) = budget();
    let copy =
        RecoveryCopy::create(baseline.bounded(&baseline_budget), &baseline.parent, ACTION).unwrap();
    let chosen = selection(copy.reader(), 1);
    let before_finalize = baseline_budget.usage().vm_steps;
    let completed = copy.finalize(&chosen, "vm-threshold-copy").unwrap();
    let complete = completed.usage.vm_steps;
    assert!(complete > before_finalize);
    let mut committed_refusals = 0;
    let mut observed = Vec::new();
    // Fixed fractions approach the end of a successful finalization. At least
    // one must reach a durable recovered manifest and then fail its subsequent
    // bounded reinspection. No timing race or mutation hook is involved.
    for fraction in [500, 750, 875, 937, 968, 984, 992, 996] {
        let fixture = Fixture::new(false, false);
        let source = fixture.source_tree();
        let limit = before_finalize + (complete - before_finalize) * fraction / 1000;
        let cancelled = Arc::new(AtomicBool::new(false));
        let budget = InspectionBudget::new(
            InspectionLimits {
                max_vm_steps: limit,
                ..InspectionLimits::default()
            },
            cancelled,
        )
        .unwrap();
        let copy = RecoveryCopy::create(fixture.bounded(&budget), &fixture.parent, ACTION).unwrap();
        let chosen = selection(copy.reader(), 1);
        let paths = copy.paths().clone();
        let evidence = fs::read(&paths.evidence).unwrap();
        match copy.finalize(&chosen, "vm-threshold-copy") {
            Ok(receipt) => {
                assert!(receipt.manifest.body.interrupted.is_some());
                observed.push((limit, "success".to_owned()));
            }
            Err(error) => {
                assert!(
                    matches!(
                        error.cause,
                        RecordingError::InspectionLimit {
                            resource: InspectionResource::VmSteps,
                            ..
                        }
                    ),
                    "VM stop lost its typed cause: {error}"
                );
                assert_eq!(*error.paths, paths);
                let has_sidecar = ["-wal", "-shm", "-journal"]
                    .iter()
                    .any(|suffix| fs::symlink_metadata(sidecar(&paths.working, suffix)).is_ok());
                if !has_sidecar {
                    // Independent observer for this small synthetic artifact,
                    // after the operation released its owner. A sidecar-bearing
                    // failure is never opened to classify it.
                    let connection = Connection::open_with_flags(
                        &paths.working,
                        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                    )
                    .unwrap();
                    connection
                        .execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED;")
                        .unwrap();
                    let (state, manifests): (String, i64) = connection
                        .query_row(
                            "SELECT state,(SELECT count(*) FROM sealed_manifests) FROM take_meta",
                            [],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .unwrap();
                    if state == "sealed" && manifests == 1 {
                        committed_refusals += 1;
                        assert!(
                            matches!(
                                error.boundary,
                                CopyRecoveryBoundary::RecoveryMayHaveWritten
                                    | CopyRecoveryBoundary::RecoveredVerified
                            ),
                            "a committed recovered result was mislabeled {:?}",
                            error.boundary
                        );
                        let wire: Vec<u8> = connection
                            .query_row("SELECT body FROM sealed_manifests", [], |row| row.get(0))
                            .unwrap();
                        let manifest: SealedManifest = serde_json::from_slice(&wire).unwrap();
                        manifest.verify_hash().unwrap();
                        assert_eq!(manifest.body.operation_id, "vm-threshold-copy");
                        assert!(
                            manifest
                                .body
                                .interrupted
                                .as_ref()
                                .unwrap()
                                .report
                                .unknown_tail
                        );
                    }
                    observed.push((limit, format!("{:?}: {state}/{manifests}", error.boundary)));
                } else {
                    assert_eq!(error.boundary, CopyRecoveryBoundary::RecoveryMayHaveWritten);
                    observed.push((limit, format!("{:?}: retained sidecar", error.boundary)));
                }
            }
        }
        assert_eq!(fixture.source_tree(), source);
        assert_eq!(fs::read(&paths.evidence).unwrap(), evidence);
        assert!(
            paths.action.is_dir() && paths.working.is_file(),
            "a failed operation cleaned its artifacts"
        );
    }
    assert!(
        committed_refusals > 0,
        "VM sweep missed committed-but-unconfirmed coverage; baseline={before_finalize}..{complete}, observed={observed:?}"
    );
}
