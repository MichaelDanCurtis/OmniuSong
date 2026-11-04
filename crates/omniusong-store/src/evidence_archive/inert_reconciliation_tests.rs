//! Independent current-test synthetic extraction and read-only/race oracles.
#![cfg(unix)]

use super::inert_reconciliation::{
    InertInspectionBoundary, inspect_restored_evidence, inspect_restored_evidence_using,
};
use super::*;
use omniusong_core::ProjectDocument;
use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn budget() -> InspectionBudget {
    governor(InspectionLimits::default())
}

fn governor(limits: InspectionLimits) -> InspectionBudget {
    InspectionBudget::new(limits, Arc::new(AtomicBool::new(false))).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn stamp(metadata: &fs::Metadata) -> Stamp {
    Stamp {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        bytes: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

type Tree = BTreeMap<PathBuf, (Stamp, Kind)>;

fn tree(root: &Path) -> Tree {
    fn visit(root: &Path, path: &Path, result: &mut Tree) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let kind = if metadata.file_type().is_symlink() {
            Kind::Symlink(fs::read_link(path).unwrap())
        } else if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), result);
            }
            Kind::Directory
        } else {
            assert!(metadata.is_file());
            Kind::File(fs::read(path).unwrap())
        };
        result.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            (stamp(&metadata), kind),
        );
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    archive: PathBuf,
    directory: PathBuf,
    receipt: EvidenceRestoreReceipt,
    manifest: EvidenceManifest,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/inert-reconciliation-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let source = root.join("synthetic-native");
        let mut store = NativeStore::open(&source).unwrap();
        store
            .create(&ProjectDocument::new("Synthetic inert reconciliation").unwrap())
            .unwrap();
        store.connection.execute_batch(
            "CREATE TABLE inert_oracle(n,b); INSERT INTO inert_oracle VALUES(-29,x'00ff17'); PRAGMA application_id=73129;",
        ).unwrap();
        for name in [
            "captures/stage/empty",
            "source-actions/opaque",
            "takes/empty",
        ] {
            fs::create_dir_all(source.join(name)).unwrap();
        }
        for (name, bytes) in [
            (
                "captures/stage/original.raw",
                b"uninterpreted original\0\xff".as_slice(),
            ),
            (
                "source-actions/opaque/future.bin",
                b"opaque retained action".as_slice(),
            ),
            (
                "takes/orphan.partial",
                b"retained incomplete object".as_slice(),
            ),
        ] {
            fs::write(source.join(name), bytes).unwrap();
        }
        let archive = root.join("synthetic.osre");
        let exported = store
            .export_retained_evidence_full_root(
                &archive,
                "9aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                &budget(),
            )
            .unwrap();
        let directory = root.join("inert");
        let receipt = store
            .restore_retained_evidence_reviewed(&archive, &directory, &exported.sha256, &budget())
            .unwrap();
        drop(store);
        let manifest =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        let fixture = Self {
            root,
            source,
            archive,
            directory,
            receipt,
            manifest,
        };
        assert_eq!(fixture.reconstructed(), fs::read(&fixture.archive).unwrap());
        fixture
    }

    fn object(&self, index: usize) -> PathBuf {
        self.directory
            .join("objects")
            .join(format!("{index:04}.bin"))
    }

    fn first_file(&self) -> usize {
        self.manifest
            .entries
            .iter()
            .position(|entry| entry.kind == EvidenceEntryKind::File)
            .unwrap()
    }

    // Deliberately independent framing/footer construction, raw object reads
    // and whole-file SHA. It does not call production reader/hash helpers.
    fn reconstructed(&self) -> Vec<u8> {
        let manifest = fs::read(self.directory.join("manifest.json")).unwrap();
        let mut bytes = b"OSRE\x01\r\n\0".to_vec();
        bytes.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&manifest);
        let parsed: EvidenceManifest = serde_json::from_slice(&manifest).unwrap();
        for (index, entry) in parsed.entries.iter().enumerate() {
            if entry.kind == EvidenceEntryKind::File {
                let object = fs::read(self.object(index)).unwrap();
                assert_eq!(object.len() as u64, entry.byte_length);
                assert_eq!(Some(sha(&object)), entry.sha256);
                bytes.extend_from_slice(&object);
            }
        }
        let content_sha = Sha256::digest(&bytes);
        bytes.extend_from_slice(&content_sha);
        bytes.extend_from_slice(b"OSREEND1");
        bytes
    }

    fn write_receipt(&self, receipt: &EvidenceRestoreReceipt) {
        fs::write(
            self.directory.join("complete.json"),
            serde_json::to_vec(receipt).unwrap(),
        )
        .unwrap();
    }

    fn inspect(&self, meter: &InspectionBudget) -> Result<EvidenceRestoreReceipt> {
        inspect_restored_evidence(
            &self.directory,
            &self.archive,
            &self.receipt.archive.sha256,
            meter,
        )
    }

    fn refused_unchanged(&self) -> StoreError {
        let before = tree(&self.root);
        let error = self.inspect(&budget()).unwrap_err();
        assert_eq!(
            tree(&self.root),
            before,
            "read-only refusal altered evidence/source files"
        );
        error
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Inert reconciliation fixture retained: {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
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
        _ => panic!("expected typed sticky stop, got {error:?}"),
    }
}

#[test]
fn inert_inspection_reconstructs_whole_archive_with_original_archive_and_source_absent() {
    let fixture = Fixture::new();
    let reconstructed = fixture.reconstructed();
    assert_eq!(sha(&reconstructed), fixture.receipt.archive.sha256);
    assert_eq!(
        reconstructed.len() as u64,
        fixture.receipt.archive.byte_length
    );
    assert_ne!(
        sha(&reconstructed[..reconstructed.len() - 40]),
        fixture.receipt.archive.sha256
    );
    fs::remove_file(&fixture.archive).unwrap();
    fs::remove_dir_all(&fixture.source).unwrap();
    let before = tree(&fixture.root);
    assert_eq!(fixture.inspect(&budget()).unwrap(), fixture.receipt);
    assert!(!fixture.archive.exists() && !fixture.source.exists());
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn inert_inspection_rejects_changed_missing_and_surplus_objects_without_writing() {
    for case in 0..4 {
        let fixture = Fixture::new();
        let object = fixture.object(fixture.first_file());
        match case {
            0 => {
                let mut bytes = fs::read(&object).unwrap();
                bytes[0] ^= 1;
                fs::write(&object, bytes).unwrap();
            }
            1 => fs::remove_file(&object).unwrap(),
            2 => fs::write(fixture.directory.join("objects/9999.bin"), b"surplus").unwrap(),
            _ => fs::create_dir(fixture.directory.join("objects/unlisted")).unwrap(),
        }
        fixture.refused_unchanged();
    }
}

#[test]
fn inert_inspection_rejects_top_membership_before_opening_metadata_files() {
    for missing in [false, true] {
        let fixture = Fixture::new();
        if missing {
            fs::remove_file(fixture.directory.join("complete.json")).unwrap();
        } else {
            fs::write(fixture.directory.join("surplus.bin"), b"unlisted").unwrap();
        }
        let held = File::open(fixture.directory.join("manifest.json")).unwrap();
        held.try_lock().unwrap();
        let error = fixture.refused_unchanged();
        assert!(
            matches!(error, StoreError::Invalid(_)),
            "top membership must refuse before locked metadata can be opened: {error:?}"
        );
    }
}

#[test]
fn inert_inspection_binds_every_receipt_field_and_exact_lexical_paths() {
    for case in 0..10 {
        let fixture = Fixture::new();
        let mut forged = fixture.receipt.clone();
        match case {
            0 => forged.directory = fixture.directory.join("."),
            1 => forged.archive.path = fixture.root.join(".").join("synthetic.osre"),
            2 => forged.archive.path = fixture.root.join("different.osre"),
            3 => forged.archive.export_id = "8bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            4 => forged.archive.sha256 = "0".repeat(64),
            5 => forged.archive.byte_length += 1,
            6 => forged.archive.file_count += 1,
            7 => forged.archive.directory_count += 1,
            8 => forged.archive.payload_bytes += 1,
            _ => forged.archive.path = PathBuf::from("relative.osre"),
        }
        fixture.write_receipt(&forged);
        fixture.refused_unchanged();
    }
}

#[test]
fn inert_inspection_requires_bounded_exact_canonical_metadata() {
    for case in 0..6 {
        let fixture = Fixture::new();
        let path = fixture.directory.join(if case % 2 == 0 {
            "complete.json"
        } else {
            "manifest.json"
        });
        if case < 2 {
            let mut bytes = fs::read(&path).unwrap();
            bytes.push(b'\n');
            fs::write(&path, bytes).unwrap();
        } else if case < 4 {
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            value["unknown"] = true.into();
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        } else {
            let size = if case % 2 == 0 {
                16_385
            } else {
                2 * 1024 * 1024 + 1
            };
            fs::write(&path, vec![b' '; size]).unwrap();
        }
        fixture.refused_unchanged();
    }
}

#[test]
fn inert_inspection_refuses_symlink_hardlink_and_nonprivate_namespaces() {
    for case in 0..8 {
        let fixture = Fixture::new();
        let path = match case {
            0 | 1 => fixture.directory.join("complete.json"),
            2 | 3 => fixture.directory.join("manifest.json"),
            4 | 5 => fixture.object(fixture.first_file()),
            _ => fixture.directory.join("objects"),
        };
        if case < 6 {
            let parked = fixture.root.join(format!("parked-{case}"));
            if case % 2 == 0 {
                fs::rename(&path, &parked).unwrap();
                symlink(&parked, &path).unwrap();
            } else {
                fs::hard_link(&path, &parked).unwrap();
            }
        } else if case == 6 {
            let parked = fixture.root.join("parked-objects");
            fs::rename(&path, &parked).unwrap();
            symlink(&parked, &path).unwrap();
        } else {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fixture.refused_unchanged();
    }
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.directory, fs::Permissions::from_mode(0o755)).unwrap();
    fixture.refused_unchanged();
    let fixture = Fixture::new();
    let parked = fixture.root.join("parked-inert");
    fs::rename(&fixture.directory, &parked).unwrap();
    symlink(&parked, &fixture.directory).unwrap();
    fixture.refused_unchanged();
}

#[test]
fn inert_inspection_refuses_wrong_objects_file_before_any_raw_descriptor() {
    let fixture = Fixture::new();
    let objects = fixture.directory.join("objects");
    fs::rename(&objects, fixture.root.join("parked-objects")).unwrap();
    let connection = Connection::open(&objects).unwrap();
    connection
        .execute_batch("CREATE TABLE untouched(n); INSERT INTO untouched VALUES(17);")
        .unwrap();
    drop(connection);
    let before = tree(&fixture.root);
    let connection = Connection::open(&objects).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    // Only the separate-process writer probe observes this active database.
    // Any extra same-process raw FD close could release its POSIX lock.
    super::tests::ownerless_sqlite_probe(&objects, true);
    let error = fixture.inspect(&budget()).unwrap_err();
    assert!(
        matches!(error, StoreError::Invalid(_)),
        "wrong objects type must fail before raw open: {error:?}"
    );
    super::tests::ownerless_sqlite_probe(&objects, true);
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
    super::tests::ownerless_sqlite_probe(&objects, false);
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn inert_inspection_authenticates_catalog_closure_after_all_hashes_are_recomputed() {
    let mut fixture = Fixture::new();
    let index = fixture
        .manifest
        .entries
        .iter()
        .position(|entry| entry.path == "catalog.sqlite3")
        .unwrap();
    let path = fixture.object(index);
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=5;").unwrap();
    drop(connection);
    let bytes = fs::read(&path).unwrap();
    fixture.manifest.entries[index].byte_length = bytes.len() as u64;
    fixture.manifest.entries[index].sha256 = Some(sha(&bytes));
    fs::write(
        fixture.directory.join("manifest.json"),
        serde_json::to_vec(&fixture.manifest).unwrap(),
    )
    .unwrap();
    let reconstructed = fixture.reconstructed();
    fixture.receipt.archive.sha256 = sha(&reconstructed);
    fixture.receipt.archive.byte_length = reconstructed.len() as u64;
    fixture.receipt.archive.payload_bytes = fixture
        .manifest
        .entries
        .iter()
        .map(|entry| entry.byte_length)
        .sum();
    fixture.write_receipt(&fixture.receipt);
    assert_ne!(
        fixture.receipt.archive.sha256,
        sha(&fs::read(&fixture.archive).unwrap())
    );
    let error = fixture.refused_unchanged();
    assert!(matches!(error, StoreError::Invalid(_)));
}

#[test]
fn inert_inspection_keeps_cancellation_and_custom_limits_typed_and_read_only() {
    let fixture = Fixture::new();
    let before = tree(&fixture.root);
    let cancelled = Arc::new(AtomicBool::new(true));
    let meter = InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
    typed_stop(
        &inspect_restored_evidence(&fixture.directory, &fixture.archive, "", &meter).unwrap_err(),
        None,
    );
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        meter.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_eq!(tree(&fixture.root), before);
    for resource in [
        InspectionResource::WorkBytes,
        InspectionResource::FileBytes,
        InspectionResource::RowVisits,
        InspectionResource::VmSteps,
    ] {
        let mut limits = InspectionLimits::default();
        match resource {
            InspectionResource::WorkBytes => limits.max_work_bytes = 0,
            InspectionResource::FileBytes => limits.max_file_bytes = 1,
            InspectionResource::RowVisits => limits.max_row_visits = 0,
            InspectionResource::VmSteps => limits.max_vm_steps = 0,
            _ => unreachable!(),
        }
        let meter = governor(limits);
        typed_stop(&fixture.inspect(&meter).unwrap_err(), Some(resource));
        assert!(meter.checkpoint().is_err());
        assert_eq!(tree(&fixture.root), before);
    }
    let largest_object = fixture
        .manifest
        .entries
        .iter()
        .map(|entry| entry.byte_length)
        .max()
        .unwrap();
    assert!(largest_object < fixture.receipt.archive.byte_length);
    let meter = governor(InspectionLimits {
        max_file_bytes: largest_object,
        ..InspectionLimits::default()
    });
    typed_stop(
        &fixture.inspect(&meter).unwrap_err(),
        Some(InspectionResource::FileBytes),
    );
    assert_eq!(tree(&fixture.root), before);
    for at_final in [false, true] {
        let cancelled = Arc::new(AtomicBool::new(false));
        let meter =
            InspectionBudget::new(InspectionLimits::default(), Arc::clone(&cancelled)).unwrap();
        let boundary = if at_final {
            InertInspectionBoundary::BeforeFinalAnchors
        } else {
            InertInspectionBoundary::AfterManifestRead
        };
        let mut reached = false;
        let error = inspect_restored_evidence_using(
            &fixture.directory,
            &fixture.archive,
            &fixture.receipt.archive.sha256,
            &meter,
            |point, _| {
                if point == boundary {
                    reached = true;
                    cancelled.store(true, Ordering::Release);
                    return Err(StoreError::Invalid(
                        "local synthetic error cannot mask cancellation".into(),
                    ));
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
        assert_eq!(tree(&fixture.root), before);
    }
}

#[test]
fn inert_inspection_final_stamps_refuse_identical_byte_substitution_and_directory_changes() {
    for case in 0..6 {
        let fixture = Fixture::new();
        let first = fixture.first_file();
        let boundary = if case == 0 {
            InertInspectionBoundary::AfterObjectRead(first)
        } else {
            InertInspectionBoundary::BeforeFinalAnchors
        };
        let mut after_fault = None;
        let result = inspect_restored_evidence_using(
            &fixture.directory,
            &fixture.archive,
            &fixture.receipt.archive.sha256,
            &budget(),
            |point, output| {
                if point == boundary {
                    if case < 3 {
                        let path = match case {
                            0 => fixture.object(first),
                            1 => output.join("manifest.json"),
                            _ => output.join("complete.json"),
                        };
                        let parked = fixture.root.join("same-byte-parked");
                        fs::rename(&path, &parked).unwrap();
                        fs::copy(&parked, &path).unwrap();
                    } else if case == 3 {
                        let objects = output.join("objects");
                        let transient = objects.join("transient");
                        fs::write(&transient, b"membership changed then removed").unwrap();
                        fs::remove_file(&transient).unwrap();
                        File::open(&objects)
                            .unwrap()
                            .set_times(
                                fs::FileTimes::new()
                                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
                            )
                            .unwrap();
                    } else {
                        fn copy_directory(from: &Path, to: &Path) {
                            fs::create_dir(to).unwrap();
                            fs::set_permissions(to, fs::Permissions::from_mode(0o700)).unwrap();
                            for entry in fs::read_dir(from).unwrap() {
                                let entry = entry.unwrap();
                                let target = to.join(entry.file_name());
                                if entry.file_type().unwrap().is_dir() {
                                    copy_directory(&entry.path(), &target);
                                } else {
                                    fs::copy(entry.path(), target).unwrap();
                                }
                            }
                        }
                        let path = if case == 4 {
                            output.join("objects")
                        } else {
                            output.to_owned()
                        };
                        let parked = fixture.root.join("replaced-directory");
                        fs::rename(&path, &parked).unwrap();
                        copy_directory(&parked, &path);
                    }
                    after_fault = Some(tree(&fixture.root));
                }
                Ok(())
            },
        );
        assert!(
            after_fault.is_some() && result.is_err(),
            "identity/timestamp substitution {case} accepted"
        );
        assert_eq!(
            tree(&fixture.root),
            after_fault.unwrap(),
            "verification repaired or cleaned changed evidence"
        );
    }
}

#[test]
fn inert_inspection_rejects_strict_pin_provenance_and_noncanonical_directory_inputs() {
    let fixture = Fixture::new();
    let before = tree(&fixture.root);
    for expected in [
        String::new(),
        "A".repeat(64),
        "0".repeat(63),
        "0".repeat(64),
        "a".repeat(65),
        "a".repeat(4097),
        format!("{}g", "0".repeat(63)),
    ] {
        assert!(
            inspect_restored_evidence(&fixture.directory, &fixture.archive, &expected, &budget())
                .is_err()
        );
    }
    for path in [
        PathBuf::from("relative.osre"),
        PathBuf::from(format!("/{}", "a".repeat(4096))),
        PathBuf::from("/archive\n.osre"),
    ] {
        assert!(
            inspect_restored_evidence(
                &fixture.directory,
                &path,
                &fixture.receipt.archive.sha256,
                &budget()
            )
            .is_err()
        );
    }
    let lexical = fixture.directory.join(".");
    assert!(
        inspect_restored_evidence(
            &lexical,
            &fixture.archive,
            &fixture.receipt.archive.sha256,
            &budget()
        )
        .is_err()
    );
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn inert_inspection_directory_lease_excludes_writers_through_the_final_read() {
    let fixture = Fixture::new();
    let before = tree(&fixture.root);
    let writer = crate::retained_file_coordination::writer(&fixture.directory).unwrap();
    assert!(matches!(
        fixture.inspect(&budget()),
        Err(StoreError::Recording(RecordingError::Owned))
    ));
    drop(writer);
    let mut reached = false;
    let receipt = inspect_restored_evidence_using(
        &fixture.directory,
        &fixture.archive,
        &fixture.receipt.archive.sha256,
        &budget(),
        |point, directory| {
            if point == InertInspectionBoundary::BeforeFinalAnchors {
                reached = true;
                assert!(matches!(
                    crate::retained_file_coordination::writer(directory),
                    Err(StoreError::Recording(RecordingError::Owned))
                ));
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(reached);
    assert_eq!(receipt, fixture.receipt);
    let _released = crate::retained_file_coordination::writer(&fixture.directory).unwrap();
    assert_eq!(tree(&fixture.root), before);
}
