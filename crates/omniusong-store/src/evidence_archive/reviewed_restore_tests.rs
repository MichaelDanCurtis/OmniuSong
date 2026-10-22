//! Content-review pin oracles use only current-test synthetic catalog/raw bytes.
//! No recording device, native session, live data root or installed app is used.
#![cfg(unix)]

use super::*;
use omniusong_core::ProjectDocument;
use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};
use rusqlite::{params, types::ValueRef};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const EXPORT_A: &str = "a1111111111111111111111111111111";
const EXPORT_B: &str = "b2222222222222222222222222222222";

fn governor(limits: InspectionLimits) -> InspectionBudget {
    InspectionBudget::new(limits, Arc::new(AtomicBool::new(false))).unwrap()
}

fn budget() -> InspectionBudget {
    governor(InspectionLimits::default())
}

fn raw_sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

fn rows(connection: &Connection, sql: &str) -> Vec<Vec<Cell>> {
    let mut statement = connection.prepare(sql).unwrap();
    let columns = statement.column_count();
    let mut result = statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| {
                    Ok(match row.get_ref(column)? {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(n) => Cell::Integer(n),
                        ValueRef::Real(n) => Cell::Real(n.to_bits()),
                        ValueRef::Text(bytes) => Cell::Text(bytes.to_vec()),
                        ValueRef::Blob(bytes) => Cell::Blob(bytes.to_vec()),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    result.sort();
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Catalog {
    schema: Vec<Vec<Cell>>,
    tables: BTreeMap<String, Vec<Vec<Cell>>>,
    version: i64,
    application: i64,
}

fn catalog(connection: &Connection) -> Catalog {
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    Catalog {
        schema: rows(
            connection,
            "SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema",
        ),
        tables: tables
            .into_iter()
            .map(|name| {
                let cells = rows(
                    connection,
                    &format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")),
                );
                (name, cells)
            })
            .collect(),
        version: connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap(),
        application: connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    Directory,
    File(Vec<u8>),
}

fn tree(root: &Path) -> BTreeMap<String, Item> {
    fn visit(root: &Path, directory: &Path, output: &mut BTreeMap<String, Item>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                output.insert(relative, Item::Directory);
                visit(root, &path, output);
            } else {
                assert!(metadata.is_file());
                output.insert(relative, Item::File(fs::read(path).unwrap()));
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

struct Fixture {
    root: PathBuf,
    store: NativeStore,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/reviewed-evidence-restore-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut store = NativeStore::open(root.join("synthetic-native")).unwrap();
        store
            .create(&ProjectDocument::new("Synthetic reviewed evidence source").unwrap())
            .unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TABLE reviewed_pin_oracle(n,i,r,t,b); PRAGMA application_id=48517;",
            )
            .unwrap();
        store
            .connection
            .execute(
                "INSERT INTO reviewed_pin_oracle VALUES(NULL,-19,1.25,?1,?2)",
                params!["opaque\0texté", b"\0original\xff".as_slice()],
            )
            .unwrap();
        // Raw retained files deliberately carry no recording ownership or
        // catalog linkage. Export must preserve these opaque originals exactly.
        for name in [
            "captures/stage/empty",
            "source-actions/opaque",
            "takes/empty",
        ] {
            fs::create_dir_all(store.root().join(name)).unwrap();
        }
        for (name, bytes) in [
            (
                "captures/stage/original.bin",
                b"synthetic original\0\xff".as_slice(),
            ),
            (
                "captures/stage/original.bin-journal",
                b"opaque raw sidecar".as_slice(),
            ),
            (
                "source-actions/opaque/retained.bin",
                b"retained action bytes".as_slice(),
            ),
            (
                "takes/orphan.partial",
                b"incomplete opaque object".as_slice(),
            ),
        ] {
            fs::write(store.root().join(name), bytes).unwrap();
        }
        Self { root, store }
    }

    fn export(&mut self, name: &str, export_id: &str) -> EvidenceArchiveReceipt {
        self.store
            .export_retained_evidence_full_root(self.root.join(name), export_id, &budget())
            .unwrap()
    }

    fn source(&self) -> (Catalog, BTreeMap<String, Item>) {
        (catalog(&self.store.connection), tree(self.store.root()))
    }

    fn unchanged(&self, before: &(Catalog, BTreeMap<String, Item>)) {
        assert_eq!(
            &self.source(),
            before,
            "live catalog, identities or original tree changed"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Reviewed restore fixture retained: {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct Parsed {
    manifest: EvidenceManifest,
    manifest_bytes: Vec<u8>,
    objects: BTreeMap<String, Vec<u8>>,
    bytes: Vec<u8>,
}

// This byte oracle does not call the production reader, hash helper or footer
// constants. It locates every object and authenticates both container digests.
fn parse(path: &Path) -> Parsed {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"OSRE\x01\r\n\0");
    let size = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let manifest_bytes = bytes[16..16 + size].to_vec();
    let manifest: EvidenceManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(serde_json::to_vec(&manifest).unwrap(), manifest_bytes);
    let mut offset = 16 + size;
    let mut objects = BTreeMap::new();
    for (index, entry) in manifest.entries.iter().enumerate() {
        if entry.kind == EvidenceEntryKind::File {
            let stop = offset + entry.byte_length as usize;
            let payload = bytes[offset..stop].to_vec();
            assert_eq!(Some(raw_sha(&payload)), entry.sha256);
            objects.insert(format!("{index:04}.bin"), payload);
            offset = stop;
        }
    }
    assert_eq!(bytes.len(), offset + 40);
    assert_eq!(
        &bytes[offset..offset + 32],
        Sha256::digest(&bytes[..offset]).as_slice()
    );
    assert_eq!(&bytes[offset + 32..], b"OSREEND1");
    Parsed {
        manifest,
        manifest_bytes,
        objects,
        bytes,
    }
}

fn names(path: &Path) -> BTreeSet<String> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

fn assert_inert(directory: &Path, parsed: &Parsed, complete: bool) {
    let mut top = BTreeSet::from(["manifest.json".into(), "objects".into()]);
    if complete {
        top.insert("complete.json".into());
    }
    assert_eq!(names(directory), top);
    assert_eq!(
        fs::read(directory.join("manifest.json")).unwrap(),
        parsed.manifest_bytes
    );
    assert_eq!(
        names(&directory.join("objects")),
        parsed.objects.keys().cloned().collect()
    );
    for (name, bytes) in &parsed.objects {
        assert_eq!(
            fs::read(directory.join("objects").join(name)).unwrap(),
            *bytes
        );
    }
    assert!(!directory.join("catalog.sqlite3").exists());
    for active in ["captures", "source-actions", "takes", "studio.sqlite3"] {
        assert!(
            !directory.join(active).exists(),
            "restored active identity {active}"
        );
    }
}

fn typed_stop(error: &StoreError, resource: Option<InspectionResource>) {
    match (error, resource) {
        (StoreError::Recording(RecordingError::InspectionCancelled), None) => (),
        (
            StoreError::Recording(RecordingError::InspectionLimit {
                resource: actual, ..
            }),
            Some(expected),
        ) => assert_eq!(*actual, expected),
        _ => panic!("expected sticky typed governor stop, got {error:?}"),
    }
}

#[test]
fn reviewed_valid_a_replaced_by_valid_b_refuses_completion_and_preserves_exact_sources() {
    let mut fixture = Fixture::new();
    let a = fixture.export("selected.osre", EXPORT_A);
    let review = inspect_evidence_archive(&a.path, &budget()).unwrap();
    let parsed_a = parse(&a.path);
    fixture
        .store
        .connection
        .execute(
            "INSERT INTO assets(id,payload) VALUES(?1,?2)",
            params![
                "later-synthetic-catalog",
                b"different valid B catalog".as_slice()
            ],
        )
        .unwrap();
    let b = fixture.export("replacement.osre", EXPORT_B);
    let parsed_b = parse(&b.path);
    let b_review = inspect_evidence_archive(&b.path, &budget()).unwrap();
    assert_ne!(review.manifest.export_id, b_review.manifest.export_id);
    assert_ne!(review.receipt.sha256, b_review.receipt.sha256);
    let a_catalog = parsed_a
        .manifest
        .entries
        .iter()
        .find(|entry| entry.path == "catalog.sqlite3")
        .unwrap();
    let b_catalog = parsed_b
        .manifest
        .entries
        .iter()
        .find(|entry| entry.path == "catalog.sqlite3")
        .unwrap();
    assert_ne!(a_catalog.sha256, b_catalog.sha256);
    fs::remove_file(&a.path).unwrap();
    fs::copy(&b.path, &a.path).unwrap();
    assert_eq!(fs::read(&a.path).unwrap(), parsed_b.bytes);
    let before = fixture.source();
    let directory = fixture.root.join("refused-reviewed-restore");
    let error = fixture
        .store
        .restore_retained_evidence_reviewed(&a.path, &directory, &review.receipt.sha256, &budget())
        .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)));
    assert_inert(&directory, &parsed_b, false);
    assert!(!directory.join("complete.json").exists());
    assert_eq!(fs::read(&a.path).unwrap(), parsed_b.bytes);
    assert_eq!(fs::read(&b.path).unwrap(), parsed_b.bytes);
    fixture.unchanged(&before);
}

#[test]
fn reviewed_same_bytes_at_another_ordinary_path_restore_with_whole_file_digest() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("reviewed.osre", EXPORT_A);
    let review = inspect_evidence_archive(&archive.path, &budget()).unwrap();
    let parsed = parse(&archive.path);
    assert_eq!(review.receipt.sha256, raw_sha(&parsed.bytes));
    assert_ne!(
        review.receipt.sha256,
        raw_sha(&parsed.bytes[..parsed.bytes.len() - 40])
    );
    let copied = fixture.root.join("ordinary-copy.osre");
    fs::copy(&archive.path, &copied).unwrap();
    let before = fixture.source();
    let directory = fixture.root.join("reviewed-copy-inert");
    let receipt = fixture
        .store
        .restore_retained_evidence_reviewed(&copied, &directory, &review.receipt.sha256, &budget())
        .unwrap();
    assert_eq!(receipt.archive.path, copied);
    assert_eq!(receipt.archive.sha256, raw_sha(&parsed.bytes));
    assert_eq!(receipt.archive.byte_length, parsed.bytes.len() as u64);
    let complete: EvidenceRestoreReceipt =
        serde_json::from_slice(&fs::read(directory.join("complete.json")).unwrap()).unwrap();
    assert_eq!(complete, receipt);
    assert_inert(&directory, &parsed, true);
    let index = parsed
        .manifest
        .entries
        .iter()
        .position(|entry| entry.path == "catalog.sqlite3")
        .unwrap();
    let archived_catalog = Connection::open_with_flags(
        directory.join("objects").join(format!("{index:04}.bin")),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(catalog(&archived_catalog), before.0);
    assert_eq!(fs::read(&archive.path).unwrap(), parsed.bytes);
    assert_eq!(fs::read(&copied).unwrap(), parsed.bytes);
    fixture.unchanged(&before);
}

#[test]
fn reviewed_malformed_expected_hashes_refuse_before_any_output() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("strict-digest.osre", EXPORT_A);
    let before = fixture.source();
    let whole_before = tree(&fixture.root);
    for (index, expected) in [
        String::new(),
        "A".repeat(64),
        "0".repeat(63),
        "0".repeat(65),
        "a".repeat(4097),
        format!("{}g", "0".repeat(63)),
        format!("{}\n", "0".repeat(63)),
    ]
    .iter()
    .enumerate()
    {
        let directory = fixture.root.join(format!("invalid-expected-{index}"));
        let error = fixture
            .store
            .restore_retained_evidence_reviewed(&archive.path, &directory, expected, &budget())
            .unwrap_err();
        assert!(matches!(error, StoreError::Invalid(_)), "{error:?}");
        assert!(!directory.exists());
        assert_eq!(tree(&fixture.root), whole_before);
    }
    fixture.unchanged(&before);
}

#[test]
fn reviewed_validation_cannot_replace_a_sticky_cancel_or_work_limit() {
    let fixture = Fixture::new();
    let before = fixture.source();
    let nonexistent = fixture.root.join("must-not-be-opened.osre");
    let cancelled = Arc::new(AtomicBool::new(true));
    let meter = InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
    let directory = fixture.root.join("cancelled-malformed");
    typed_stop(
        &fixture
            .store
            .restore_retained_evidence_reviewed(&nonexistent, &directory, "", &meter)
            .unwrap_err(),
        None,
    );
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        meter.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert!(!directory.exists());
    let meter = governor(InspectionLimits {
        max_work_bytes: 0,
        ..InspectionLimits::default()
    });
    assert!(meter.charge_work(1).is_err());
    let directory = fixture.root.join("limited-malformed");
    typed_stop(
        &fixture
            .store
            .restore_retained_evidence_reviewed(&nonexistent, &directory, "not-a-digest", &meter)
            .unwrap_err(),
        Some(InspectionResource::WorkBytes),
    );
    assert!(!directory.exists());
    fixture.unchanged(&before);
}

#[test]
fn reviewed_existing_destination_is_preserved_without_overwrite_or_cleanup() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("no-overwrite.osre", EXPORT_A);
    let review = inspect_evidence_archive(&archive.path, &budget()).unwrap();
    let directory = fixture.root.join("already-retained");
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("complete.json"),
        b"preexisting unrelated bytes",
    )
    .unwrap();
    fs::write(directory.join("sentinel.bin"), b"preserve without cleanup").unwrap();
    let before = fixture.source();
    let output_before = tree(&directory);
    let archive_before = fs::read(&archive.path).unwrap();
    let error = fixture
        .store
        .restore_retained_evidence_reviewed(
            &archive.path,
            &directory,
            &review.receipt.sha256,
            &budget(),
        )
        .unwrap_err();
    assert!(matches!(error, StoreError::Io(_)));
    assert_eq!(tree(&directory), output_before);
    assert_eq!(fs::read(&archive.path).unwrap(), archive_before);
    fixture.unchanged(&before);
}

#[test]
fn reviewed_cancel_after_directory_creation_wins_over_a_wrong_pin_and_keeps_partial() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("cancel-wrong-pin.osre", EXPORT_A);
    let before = fixture.source();
    let cancelled = Arc::new(AtomicBool::new(false));
    let meter = InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
    let directory = fixture.root.join("cancel-wrong-pin-inert");
    let mut reached = false;
    let error = fixture
        .store
        .restore_evidence_with_review_using(
            &archive.path,
            &directory,
            Some(&"0".repeat(64)),
            &meter,
            |boundary, _| {
                if boundary == EvidenceRestoreBoundary::AfterDirectoryCreated {
                    reached = true;
                    cancelled.store(true, Ordering::Release);
                }
                Ok(())
            },
        )
        .unwrap_err();
    assert!(reached);
    typed_stop(&error, None);
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        meter.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_eq!(names(&directory), BTreeSet::from(["objects".into()]));
    assert!(names(&directory.join("objects")).is_empty());
    assert!(!directory.join("complete.json").exists());
    fixture.unchanged(&before);
}

#[test]
fn reviewed_cancel_and_custom_limits_leave_no_false_completion() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("governed.osre", EXPORT_A);
    let review = inspect_evidence_archive(&archive.path, &budget()).unwrap();
    let parsed = parse(&archive.path);
    let before = fixture.source();
    for file_limit in [false, true] {
        let limits = if file_limit {
            InspectionLimits {
                max_file_bytes: 1,
                ..InspectionLimits::default()
            }
        } else {
            InspectionLimits {
                max_work_bytes: 8,
                ..InspectionLimits::default()
            }
        };
        let meter = governor(limits);
        let directory = fixture.root.join(format!("small-budget-{file_limit}"));
        typed_stop(
            &fixture
                .store
                .restore_retained_evidence_reviewed(
                    &archive.path,
                    &directory,
                    &review.receipt.sha256,
                    &meter,
                )
                .unwrap_err(),
            Some(if file_limit {
                InspectionResource::FileBytes
            } else {
                InspectionResource::WorkBytes
            }),
        );
        assert!(directory.join("objects").is_dir());
        assert!(!directory.join("complete.json").exists());
        assert!(meter.checkpoint().is_err());
    }
    for stop_by_limit in [false, true] {
        let cancelled = Arc::new(AtomicBool::new(false));
        let meter =
            InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
        let directory = fixture
            .root
            .join(format!("stop-before-completion-{stop_by_limit}"));
        let mut reached = false;
        let error = fixture
            .store
            .restore_evidence_with_review_using(
                &archive.path,
                &directory,
                Some(&review.receipt.sha256),
                &meter,
                |boundary, output| {
                    if boundary == EvidenceRestoreBoundary::BeforeCompletion {
                        reached = true;
                        assert_inert(output, &parsed, false);
                        if stop_by_limit {
                            assert!(
                                meter
                                    .charge_work(InspectionLimits::default().max_work_bytes)
                                    .is_err()
                            );
                        } else {
                            cancelled.store(true, Ordering::Release);
                        }
                        // A local error cannot obscure the cumulative governor's
                        // cancellation/limit origin at the public return boundary.
                        return Err(StoreError::Invalid("test boundary refusal".into()));
                    }
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(reached);
        typed_stop(
            &error,
            if stop_by_limit {
                Some(InspectionResource::WorkBytes)
            } else {
                None
            },
        );
        cancelled.store(false, Ordering::Release);
        assert!(meter.checkpoint().is_err());
        assert_inert(&directory, &parsed, false);
    }
    fixture.unchanged(&before);
}

#[test]
fn reviewed_mismatch_never_reaches_the_completion_hook() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("mismatch-boundary.osre", EXPORT_A);
    let parsed = parse(&archive.path);
    let before = fixture.source();
    let directory = fixture.root.join("mismatch-boundary-inert");
    let mut completion = false;
    let error = fixture
        .store
        .restore_evidence_with_review_using(
            &archive.path,
            &directory,
            Some(&"0".repeat(64)),
            &budget(),
            |boundary, _| {
                completion |= boundary == EvidenceRestoreBoundary::BeforeCompletion;
                Ok(())
            },
        )
        .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)));
    assert!(!completion);
    assert_inert(&directory, &parsed, false);
    fixture.unchanged(&before);
}

#[test]
fn reviewed_late_cancel_or_error_retains_complete_inert_evidence_without_active_adoption() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("late-error.osre", EXPORT_A);
    let review = inspect_evidence_archive(&archive.path, &budget()).unwrap();
    let parsed = parse(&archive.path);
    let before = fixture.source();
    for cancel in [false, true] {
        let cancelled = Arc::new(AtomicBool::new(false));
        let meter =
            InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
        let directory = fixture.root.join(format!("late-error-inert-{cancel}"));
        let mut reached = false;
        let error = fixture
            .store
            .restore_evidence_with_review_using(
                &archive.path,
                &directory,
                Some(&review.receipt.sha256),
                &meter,
                |boundary, output| {
                    if boundary == EvidenceRestoreBoundary::AfterCompletionWritten {
                        reached = true;
                        assert_inert(output, &parsed, true);
                        if cancel {
                            cancelled.store(true, Ordering::Release);
                        }
                        return Err(StoreError::Invalid(
                            "synthetic failure after marker write".into(),
                        ));
                    }
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(reached);
        if cancel {
            typed_stop(&error, None);
            cancelled.store(false, Ordering::Release);
            assert!(matches!(
                meter.checkpoint(),
                Err(RecordingError::InspectionCancelled)
            ));
        } else {
            assert!(matches!(error, StoreError::Invalid(_)));
            meter.checkpoint().unwrap();
        }
        // Late failure is uncertain to the caller: already-written evidence
        // remains intact. It is neither cleaned up nor installed as live state.
        assert_inert(&directory, &parsed, true);
        let retained: EvidenceRestoreReceipt =
            serde_json::from_slice(&fs::read(directory.join("complete.json")).unwrap()).unwrap();
        assert_eq!(retained.directory, directory);
        assert_eq!(retained.archive, review.receipt);
        let reinspected = inspect_evidence_archive(&archive.path, &budget()).unwrap();
        assert_eq!(reinspected, review);
        assert_eq!(fs::read(&archive.path).unwrap(), parsed.bytes);
        fixture.unchanged(&before);
    }
}

#[test]
fn reviewed_archive_and_output_anchors_are_rechecked_after_completion_hook() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("anchor-original.osre", EXPORT_A);
    let parsed = parse(&archive.path);
    let before = fixture.source();
    for case in 0..4 {
        let selected = fixture.root.join(format!("anchor-selected-{case}.osre"));
        fs::copy(&archive.path, &selected).unwrap();
        let parked = fixture.root.join(format!("anchor-parked-{case}"));
        let directory = fixture.root.join(format!("anchor-inert-{case}"));
        let boundary = if case == 0 {
            EvidenceRestoreBoundary::AfterDirectoryCreated
        } else {
            EvidenceRestoreBoundary::BeforeCompletion
        };
        let mut reached = false;
        let result = fixture.store.restore_evidence_with_review_using(
            &selected,
            &directory,
            Some(&raw_sha(&parsed.bytes)),
            &budget(),
            |point, output| {
                if point == boundary {
                    reached = true;
                    match case {
                        0 | 1 => {
                            fs::rename(&selected, &parked).unwrap();
                            fs::copy(&archive.path, &selected).unwrap();
                        }
                        2 => {
                            let mut bytes = fs::read(&selected).unwrap();
                            *bytes.last_mut().unwrap() ^= 1;
                            fs::write(&selected, bytes).unwrap();
                        }
                        _ => {
                            fs::rename(output, &parked).unwrap();
                            fs::create_dir(output).unwrap();
                        }
                    }
                }
                Ok(())
            },
        );
        assert!(
            reached && result.is_err(),
            "anchor substitution {case} accepted"
        );
        assert!(!directory.join("complete.json").exists());
        if case == 3 {
            assert_inert(&parked, &parsed, false);
        } else {
            assert_inert(&directory, &parsed, false);
        }
        assert_eq!(fs::read(&archive.path).unwrap(), parsed.bytes);
        fixture.unchanged(&before);
    }
}

#[test]
fn reviewed_pin_uses_the_same_read_budget_as_compatible_unreviewed_restore() {
    let mut fixture = Fixture::new();
    let archive = fixture.export("one-pass.osre", EXPORT_A);
    let parsed = parse(&archive.path);
    let before = fixture.source();
    let unreviewed = fixture.root.join("unreviewed");
    let reviewed = fixture.root.join("reviewed01");
    assert_eq!(unreviewed.as_os_str().len(), reviewed.as_os_str().len());
    let unreviewed_budget = budget();
    fixture
        .store
        .restore_retained_evidence(&archive.path, &unreviewed, &unreviewed_budget)
        .unwrap();
    let reviewed_budget = budget();
    fixture
        .store
        .restore_retained_evidence_reviewed(
            &archive.path,
            &reviewed,
            &raw_sha(&parsed.bytes),
            &reviewed_budget,
        )
        .unwrap();
    assert_eq!(
        reviewed_budget.usage(),
        unreviewed_budget.usage(),
        "review pin added another archive pass or reset the governor"
    );
    assert_inert(&unreviewed, &parsed, true);
    assert_inert(&reviewed, &parsed, true);
    fixture.unchanged(&before);
}
