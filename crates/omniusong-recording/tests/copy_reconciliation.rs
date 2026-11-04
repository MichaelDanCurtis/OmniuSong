//! Independent read-only reconciliation proofs over synthetic generic journals.
//! These do not establish six-stream admission or durable service authorization.
#![cfg(unix)]

use omniusong_recording::*;
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const ACTION: &str = "22222222222222222222222222222222";
const OPERATION: &str = "independent-existing-recovered-copy";

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    parent: PathBuf,
}
impl Fixture {
    fn new(bad_tail: bool) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/copy-reconciliation-independent")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir(root.join("actions")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let source = root.join("source/take.sqlite3");
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
        drop(journal);
        if bad_tail {
            update_chunk(
                &source,
                "UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=3;",
            );
        }
        Self {
            parent: root.join("actions"),
            root,
            source,
        }
    }
    fn lock(&self) -> PathBuf {
        self.parent.join(format!("{ACTION}.execution.lock"))
    }
    fn paths(&self) -> RecoveryCopyPaths {
        let action = self.parent.join(ACTION);
        RecoveryCopyPaths {
            evidence: action.join("evidence/take.sqlite3"),
            working: action.join("recovery/take.sqlite3"),
            action,
        }
    }
    fn recover(&self, selected: u64) -> (CopyRecoveryReceipt, String, u64) {
        // The low-level copy primitive has no durable action gateway. This
        // fixture deliberately acquires its independent action owner first.
        let owner = CopyActionOwner::create(&self.parent, ACTION).unwrap();
        let (budget, _) = budget();
        let reader = CaptureReader::open_bounded(&self.source, &budget).unwrap();
        let original = fs::read(&self.source).unwrap();
        let copy = RecoveryCopy::create(reader, &self.parent, ACTION).unwrap();
        assert_eq!(owner.paths(), copy.paths());
        let chosen = selection(copy.reader(), selected);
        let receipt = copy.finalize(&chosen, OPERATION).unwrap();
        owner.verify().unwrap();
        drop(owner);
        (receipt, digest(&original), original.len() as u64)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Reconciliation fixture retained at {}", self.root.display());
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
            stream_id: "audio".into(),
            segment_id: "synthetic-pcm".into(),
            source_id: "software-fixture".into(),
            source_name: "Synthetic reconciliation; no device".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("synthetic-frames".into()),
                epoch: ClockEpochId("original-clock".into()),
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
                audio_epoch: ClockEpochId("original-audio".into()),
            },
            initial_source_position: 10,
        }],
        provenance: BTreeMap::from([(
            "opaque-original".into(),
            serde_json::from_str(
                r#"{"number":123456789012345678901234567890,"text":"retained\u0000text"}"#,
            )
            .unwrap(),
        )]),
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
            start: 10 + sequence * 2,
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
fn failure<T>(result: Result<T>) -> RecordingError {
    match result {
        Ok(_) => panic!("operation unexpectedly succeeded"),
        Err(error) => error,
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}
fn writable(path: &Path) {
    let mode = fs::metadata(path).unwrap().permissions().mode();
    fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o200)).unwrap();
}
fn update_chunk(path: &Path, sql: &str) {
    writable(path);
    let connection = Connection::open(path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='immutable_chunk_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER immutable_chunk_update;")
        .unwrap();
    connection.execute_batch(sql).unwrap();
    connection.execute_batch(&trigger).unwrap();
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

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Directory {
        mode: u32,
        dev: u64,
        ino: u64,
    },
    File {
        bytes: Vec<u8>,
        mode: u32,
        dev: u64,
        ino: u64,
    },
    Link(PathBuf),
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
        for item in fs::read_dir(path).unwrap() {
            let item = item.unwrap();
            let path = item.path();
            let key = path.strip_prefix(root).unwrap().to_owned();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let entry = if metadata.file_type().is_symlink() {
                Entry::Link(fs::read_link(&path).unwrap())
            } else if metadata.is_dir() {
                visit(root, &path, entries);
                Entry::Directory {
                    mode: metadata.mode(),
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                }
            } else {
                Entry::File {
                    bytes: fs::read(&path).unwrap(),
                    mode: metadata.mode(),
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                }
            };
            entries.insert(key, entry);
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
fn readonly(path: &Path) -> Connection {
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !sidecar(path, suffix).exists(),
            "independent oracle must not touch sidecar-bearing SQLite"
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
    connection
}
fn original_rows(path: &Path) -> BTreeMap<String, Vec<Vec<Cell>>> {
    let connection = readonly(path);
    let mut result = BTreeMap::new();
    for (name, query) in [
        ("chunks", "SELECT rowid,* FROM chunks ORDER BY rowid"),
        ("prefix", "SELECT rowid,* FROM stream_prefix ORDER BY rowid"),
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
fn stored_manifest(path: &Path) -> Vec<u8> {
    readonly(path)
        .query_row(
            "SELECT body FROM sealed_manifests WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn completed_full_short_and_bad_tail_results_reopen_without_the_original_and_never_reseal() {
    for (bad_tail, selected) in [(false, 3), (false, 1), (true, 1)] {
        let fixture = Fixture::new(bad_tail);
        let original_bytes = fs::read(&fixture.source).unwrap();
        let rows = original_rows(&fixture.source);
        let (expected, hash, count) = fixture.recover(selected);
        assert_eq!(fs::read(&expected.paths.evidence).unwrap(), original_bytes);
        assert_eq!(original_rows(&expected.paths.working), rows);
        let manifest_bytes = stored_manifest(&expected.paths.working);
        fs::remove_dir_all(fixture.source.parent().unwrap()).unwrap();
        let before = tree(&fixture.root);
        for _ in 0..2 {
            let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
            let (budget, _) = budget();
            let view = RecoveredCopyView::open(&owner, &hash, count, &budget).unwrap();
            assert_eq!(
                view.evidence_inspection().report.state,
                TakeState::Recording
            );
            assert_eq!(view.evidence_inspection().journal_sha256, hash);
            assert_eq!(view.evidence_inspection().byte_length, count);
            assert_eq!(
                view.evidence_inspection().original_rows_sha256,
                expected.inspection.original_rows_sha256
            );
            assert_eq!(
                view.read_evidence_ordinal(1).unwrap().payload,
                [0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat()
            );
            assert_eq!(
                view.working_reader().read_ordinal(1).unwrap().payload,
                view.read_evidence_ordinal(1).unwrap().payload
            );
            assert!(view.working_reader().inspection_budget().is_some());
            assert_eq!(view.budget().usage(), budget.usage());
            view.verify_unchanged().unwrap();
            let replay = view.receipt().unwrap();
            assert_eq!(replay.paths, expected.paths);
            assert_eq!(replay.manifest, expected.manifest);
            assert_eq!(replay.inspection, expected.inspection);
            assert_eq!(replay.usage, budget.usage());
            assert!(
                replay
                    .manifest
                    .body
                    .interrupted
                    .as_ref()
                    .unwrap()
                    .report
                    .unknown_tail
            );
            assert_eq!(replay.manifest.body.operation_id, OPERATION);
            assert_eq!(replay.manifest.body.chunk_count, selected);
            assert_eq!(stored_manifest(&replay.paths.working), manifest_bytes);
            assert_eq!(
                tree(&fixture.root),
                before,
                "read-only replay changed retained artifacts"
            );
            drop(view);
            owner.verify().unwrap();
            drop(owner);
            assert_eq!(tree(&fixture.root), before);
        }
    }
}

#[test]
fn missing_lock_or_absent_partial_and_unsealed_actions_are_not_created_or_continued() {
    for shape in [
        "no-lock",
        "lock-only",
        "action-only",
        "evidence-only",
        "partial-working",
        "unsealed-working",
    ] {
        let fixture = Fixture::new(false);
        let source = fs::read(&fixture.source).unwrap();
        let paths = fixture.paths();
        if shape != "no-lock" {
            drop(CopyActionOwner::create(&fixture.parent, ACTION).unwrap());
        }
        if matches!(shape, "action-only" | "evidence-only" | "partial-working") {
            fs::create_dir(&paths.action).unwrap();
        }
        if matches!(shape, "evidence-only" | "partial-working") {
            fs::create_dir(paths.evidence.parent().unwrap()).unwrap();
            fs::write(&paths.evidence, &source).unwrap();
        }
        if shape == "partial-working" {
            fs::create_dir(paths.working.parent().unwrap()).unwrap();
            fs::write(&paths.working, b"retained interrupted copy\0\xff").unwrap();
        }
        if shape == "unsealed-working" {
            let _owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
            let (budget, _) = budget();
            let copy = RecoveryCopy::create(
                CaptureReader::open_bounded(&fixture.source, &budget).unwrap(),
                &fixture.parent,
                ACTION,
            )
            .unwrap();
            drop(copy);
        }
        let before = tree(&fixture.root);
        match CopyActionOwner::open_existing(&fixture.parent, ACTION) {
            Ok(owner) => {
                let (budget, _) = budget();
                failure(RecoveredCopyView::open(
                    &owner,
                    &digest(&source),
                    source.len() as u64,
                    &budget,
                ));
            }
            Err(_) => assert_eq!(shape, "no-lock"),
        }
        assert_eq!(
            tree(&fixture.root),
            before,
            "{shape} was created, recopied or cleaned"
        );
    }
    for missing in ["lease", "working-owner", "evidence", "working"] {
        let fixture = Fixture::new(false);
        let (receipt, hash, count) = fixture.recover(1);
        let path = match missing {
            "lease" => fixture.lock(),
            "working-owner" => sidecar(&receipt.paths.working, ".owner.lock"),
            "evidence" => receipt.paths.evidence.clone(),
            "working" => receipt.paths.working.clone(),
            _ => unreachable!(),
        };
        fs::remove_file(path).unwrap();
        let before = tree(&fixture.root);
        match CopyActionOwner::open_existing(&fixture.parent, ACTION) {
            Ok(owner) => {
                assert_ne!(missing, "lease");
                let (budget, _) = budget();
                failure(RecoveredCopyView::open(&owner, &hash, count, &budget));
            }
            Err(_) => assert_eq!(missing, "lease"),
        }
        assert_eq!(
            tree(&fixture.root),
            before,
            "missing {missing} was recreated"
        );
    }
}

#[test]
fn independent_file_owners_exclude_reconciliation_until_the_actual_lock_is_released() {
    let fixture = Fixture::new(false);
    let (_, hash, count) = fixture.recover(1);
    let before = tree(&fixture.root);
    let raw = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.lock())
        .unwrap();
    raw.try_lock().unwrap();
    assert!(matches!(
        failure(CopyActionOwner::open_existing(&fixture.parent, ACTION)),
        RecordingError::Owned
    ));
    drop(raw);
    let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
    let other = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.lock())
        .unwrap();
    assert!(
        other.try_lock().is_err(),
        "action guard did not own the OS file lock"
    );
    assert!(matches!(
        failure(CopyActionOwner::open_existing(&fixture.parent, ACTION)),
        RecordingError::Owned
    ));
    let (budget, _) = budget();
    let view = RecoveredCopyView::open(&owner, &hash, count, &budget).unwrap();
    assert!(matches!(
        CaptureReader::open(&fixture.paths().working),
        Err(RecordingError::Owned)
    ));
    execution_owner_child(&fixture.parent, true);
    assert_eq!(tree(&fixture.root), before);
    drop(view);
    drop(owner);
    other.try_lock().unwrap();
    execution_owner_child(&fixture.parent, true);
    drop(other);
    execution_owner_child(&fixture.parent, false);
    assert_eq!(tree(&fixture.root), before);
}

fn execution_owner_child(parent: &Path, expect_busy: bool) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "execution_owner_subprocess",
            "--ignored",
            "--nocapture",
        ])
        .env("OMNIUSONG_RECONCILIATION_TEST_PARENT", parent)
        .env(
            "OMNIUSONG_RECONCILIATION_TEST_BUSY",
            if expect_busy { "yes" } else { "no" },
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "action-lease child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "deterministic child invoked only by the parent action-owner test"]
fn execution_owner_subprocess() {
    let parent = PathBuf::from(
        std::env::var_os("OMNIUSONG_RECONCILIATION_TEST_PARENT")
            .expect("this helper requires its isolated parent's explicit path"),
    );
    let before = tree(&parent);
    match std::env::var("OMNIUSONG_RECONCILIATION_TEST_BUSY")
        .unwrap()
        .as_str()
    {
        "yes" => assert!(matches!(
            failure(CopyActionOwner::open_existing(&parent, ACTION)),
            RecordingError::Owned
        )),
        "no" => {
            let owner = CopyActionOwner::open_existing(&parent, ACTION).unwrap();
            owner.verify().unwrap();
        }
        other => panic!("invalid helper mode {other}"),
    }
    assert_eq!(
        tree(&parent),
        before,
        "a competing process changed retained artifacts"
    );
}

#[test]
fn invalid_owner_names_and_noncanonical_parent_aliases_make_no_files() {
    let fixture = Fixture::new(false);
    let alias = fixture.root.join("actions-alias");
    symlink(&fixture.parent, &alias).unwrap();
    for id in [
        "",
        "00000000000000000000000000000000",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "../escape",
        "2222222222222222222222222222222",
    ] {
        let before = tree(&fixture.root);
        failure(CopyActionOwner::create(&fixture.parent, id));
        failure(CopyActionOwner::open_existing(&fixture.parent, id));
        assert_eq!(tree(&fixture.root), before);
    }
    for parent in [
        alias,
        fixture.parent.join("../actions"),
        fixture.root.join("missing"),
    ] {
        let before = tree(&fixture.root);
        failure(CopyActionOwner::create(&parent, ACTION));
        failure(CopyActionOwner::open_existing(&parent, ACTION));
        assert_eq!(tree(&fixture.root), before);
    }
}

#[test]
fn lease_identity_replacement_links_and_nonempty_lock_files_are_refused() {
    for change in ["replace", "hardlink", "symlink", "content"] {
        let fixture = Fixture::new(false);
        let owner = CopyActionOwner::create(&fixture.parent, ACTION).unwrap();
        let lock = fixture.lock();
        let retained = fixture.parent.join("retained-execution-lock");
        match change {
            "replace" => {
                fs::rename(&lock, &retained).unwrap();
                fs::write(&lock, b"").unwrap();
            }
            "hardlink" => fs::hard_link(&lock, &retained).unwrap(),
            "symlink" => {
                fs::rename(&lock, &retained).unwrap();
                symlink(&retained, &lock).unwrap();
            }
            "content" => fs::write(&lock, b"foreign retained lock bytes").unwrap(),
            _ => unreachable!(),
        }
        let before = tree(&fixture.root);
        failure(owner.verify());
        assert_eq!(tree(&fixture.root), before);
        drop(owner);
        if change != "replace" {
            failure(CopyActionOwner::open_existing(&fixture.parent, ACTION));
        }
        assert_eq!(tree(&fixture.root), before);
    }
}

#[test]
fn evidence_and_working_sidecars_refuse_before_sqlite_and_remain_exact() {
    for target in ["evidence", "working"] {
        for suffix in ["-journal", "-wal", "-shm"] {
            for after_open in [false, true] {
                let fixture = Fixture::new(false);
                let (receipt, hash, count) = fixture.recover(1);
                let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
                let (budget, _) = budget();
                let view = if after_open {
                    Some(RecoveredCopyView::open(&owner, &hash, count, &budget).unwrap())
                } else {
                    None
                };
                let path = if target == "evidence" {
                    &receipt.paths.evidence
                } else {
                    &receipt.paths.working
                };
                fs::write(sidecar(path, suffix), b"retained ambiguous sidecar\0\xff").unwrap();
                let before = tree(&fixture.root);
                if let Some(view) = view {
                    failure(view.verify_unchanged());
                    failure(view.receipt());
                } else {
                    failure(RecoveredCopyView::open(&owner, &hash, count, &budget));
                }
                assert_eq!(tree(&fixture.root), before, "{target} {suffix} was changed");
            }
        }
    }
}

#[test]
fn exact_evidence_hash_length_and_whole_image_are_required() {
    for bad in ["hash", "uppercase", "short", "long", "trailing-bytes"] {
        let fixture = Fixture::new(false);
        let (receipt, mut hash, mut count) = fixture.recover(1);
        match bad {
            "hash" => hash = "0".repeat(64),
            "uppercase" => hash = hash.to_uppercase(),
            "short" => count -= 1,
            "long" => count += 1,
            "trailing-bytes" => {
                writable(&receipt.paths.evidence);
                OpenOptions::new()
                    .append(true)
                    .open(&receipt.paths.evidence)
                    .unwrap()
                    .write_all(b"unreviewed physical tail")
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = tree(&fixture.root);
        let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
        let (budget, _) = budget();
        failure(RecoveredCopyView::open(&owner, &hash, count, &budget));
        assert_eq!(
            tree(&fixture.root),
            before,
            "bad {bad} touched retained files"
        );
    }
}

#[test]
fn changed_unselected_original_working_rows_cannot_hide_behind_the_selected_manifest() {
    let fixture = Fixture::new(false);
    let (receipt, hash, count) = fixture.recover(1);
    let selected_manifest = stored_manifest(&receipt.paths.working);
    let evidence_rows = original_rows(&receipt.paths.evidence);
    update_chunk(
        &receipt.paths.working,
        "UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=3;",
    );
    assert_eq!(stored_manifest(&receipt.paths.working), selected_manifest);
    assert_ne!(original_rows(&receipt.paths.working), evidence_rows);
    let before = tree(&fixture.root);
    let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
    let (budget, _) = budget();
    failure(RecoveredCopyView::open(&owner, &hash, count, &budget));
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn normal_sealed_working_is_not_a_recovered_result_and_is_never_resealed() {
    let fixture = Fixture::new(false);
    let (receipt, hash, count) = fixture.recover(1);
    let normal_path = fixture.root.join("independent-normal.sqlite3");
    let descriptor = &receipt.manifest.body.descriptor;
    let mut normal_journal = Journal::create(&normal_path, descriptor).unwrap();
    for sequence in 0..3 {
        normal_journal
            .append(
                &header(descriptor, sequence),
                &[0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat(),
            )
            .unwrap();
    }
    let normal = normal_journal.seal("ordinary-normal-fixture").unwrap();
    assert!(normal.body.interrupted.is_none());
    drop(normal_journal);
    assert_eq!(
        original_rows(&normal_path),
        original_rows(&receipt.paths.evidence)
    );
    writable(&receipt.paths.working);
    fs::copy(&normal_path, &receipt.paths.working).unwrap();
    let before = tree(&fixture.root);
    let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
    let (budget, _) = budget();
    failure(RecoveredCopyView::open(&owner, &hash, count, &budget));
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn artifact_aliases_and_byte_identical_replacements_after_open_lose_identity() {
    for target in ["evidence", "working", "working-owner", "lease"] {
        for change in ["replace", "hardlink", "symlink"] {
            let fixture = Fixture::new(false);
            let (receipt, hash, count) = fixture.recover(1);
            let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
            let (budget, _) = budget();
            let view = RecoveredCopyView::open(&owner, &hash, count, &budget).unwrap();
            let path = match target {
                "evidence" => receipt.paths.evidence.clone(),
                "working" => receipt.paths.working.clone(),
                "working-owner" => sidecar(&receipt.paths.working, ".owner.lock"),
                "lease" => fixture.lock(),
                _ => unreachable!(),
            };
            let retained = path.with_file_name(format!("retained-{target}-{change}"));
            match change {
                "replace" => {
                    let bytes = fs::read(&path).unwrap();
                    fs::rename(&path, &retained).unwrap();
                    fs::write(&path, bytes).unwrap();
                }
                "hardlink" => fs::hard_link(&path, &retained).unwrap(),
                "symlink" => {
                    fs::rename(&path, &retained).unwrap();
                    symlink(&retained, &path).unwrap();
                }
                _ => unreachable!(),
            }
            let before = tree(&fixture.root);
            failure(view.verify_unchanged());
            failure(view.receipt());
            assert_eq!(
                tree(&fixture.root),
                before,
                "{target} {change} was changed or cleaned"
            );
        }
    }
}

#[test]
fn preexisting_artifact_symlinks_hardlinks_and_directory_aliases_are_not_followed() {
    for target in ["evidence", "working", "working-owner", "action-directory"] {
        for change in ["hardlink", "symlink"] {
            if target == "action-directory" && change == "hardlink" {
                continue;
            }
            let fixture = Fixture::new(false);
            let (receipt, hash, count) = fixture.recover(1);
            let path = match target {
                "evidence" => receipt.paths.evidence.clone(),
                "working" => receipt.paths.working.clone(),
                "working-owner" => sidecar(&receipt.paths.working, ".owner.lock"),
                "action-directory" => receipt.paths.action.clone(),
                _ => unreachable!(),
            };
            let retained = path.with_file_name(format!("retained-{target}"));
            if change == "hardlink" {
                fs::hard_link(&path, &retained).unwrap();
            } else {
                fs::rename(&path, &retained).unwrap();
                symlink(&retained, &path).unwrap();
            }
            let before = tree(&fixture.root);
            let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
            let (budget, _) = budget();
            failure(RecoveredCopyView::open(&owner, &hash, count, &budget));
            assert_eq!(tree(&fixture.root), before);
        }
    }
}

#[test]
fn changed_schema_cannot_be_accepted_as_the_exact_recovered_artifact() {
    for target in ["evidence", "working", "both"] {
        for sql in [
            "ALTER TABLE chunks ADD COLUMN \"opaque \"\"original\" BLOB DEFAULT X'00ff817f';",
            "CREATE TRIGGER opaque_retained_program AFTER INSERT ON chunks BEGIN SELECT 1; END;",
        ] {
            let fixture = Fixture::new(false);
            let (receipt, mut hash, mut count) = fixture.recover(1);
            for path in [&receipt.paths.evidence, &receipt.paths.working] {
                if target == "both"
                    || (target == "evidence" && path == &receipt.paths.evidence)
                    || (target == "working" && path == &receipt.paths.working)
                {
                    writable(path);
                    Connection::open(path).unwrap().execute_batch(sql).unwrap();
                }
            }
            if target != "working" {
                let bytes = fs::read(&receipt.paths.evidence).unwrap();
                hash = digest(&bytes);
                count = bytes.len() as u64;
            }
            let before = tree(&fixture.root);
            let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
            let (budget, _) = budget();
            failure(RecoveredCopyView::open(&owner, &hash, count, &budget));
            assert_eq!(
                tree(&fixture.root),
                before,
                "schema {target} {sql} was modified"
            );
        }
    }
}

#[test]
fn sticky_cancellation_and_work_limits_cover_open_reads_verification_and_receipt() {
    for after_open in [false, true] {
        for stop in ["cancel", "work"] {
            let fixture = Fixture::new(false);
            let (_, hash, count) = fixture.recover(1);
            let before = tree(&fixture.root);
            let owner = CopyActionOwner::open_existing(&fixture.parent, ACTION).unwrap();
            let (budget, cancelled) = budget();
            let view = if after_open {
                Some(RecoveredCopyView::open(&owner, &hash, count, &budget).unwrap())
            } else {
                None
            };
            if stop == "cancel" {
                cancelled.store(true, Ordering::Release);
                assert!(matches!(
                    budget.checkpoint(),
                    Err(RecordingError::InspectionCancelled)
                ));
                cancelled.store(false, Ordering::Release);
            } else {
                assert!(matches!(
                    budget.charge_work(u64::MAX),
                    Err(RecordingError::InspectionLimit {
                        resource: InspectionResource::WorkBytes,
                        ..
                    })
                ));
                cancelled.store(true, Ordering::Release);
            }
            let assert_stop = |error| match stop {
                "cancel" => assert!(matches!(error, RecordingError::InspectionCancelled)),
                "work" => assert!(matches!(
                    error,
                    RecordingError::InspectionLimit {
                        resource: InspectionResource::WorkBytes,
                        ..
                    }
                )),
                _ => unreachable!(),
            };
            if let Some(view) = view {
                assert_stop(failure(view.read_evidence_ordinal(1)));
                assert_stop(failure(view.working_reader().read_ordinal(1)));
                assert_stop(failure(view.verify_unchanged()));
                assert_stop(failure(view.receipt()));
            } else {
                assert_stop(failure(RecoveredCopyView::open(
                    &owner, &hash, count, &budget,
                )));
            }
            assert_eq!(tree(&fixture.root), before);
        }
    }
}
