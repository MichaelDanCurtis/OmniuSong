//! Isolated metadata only: requests never execute any artifact operation.
#![cfg(unix)]
use super::*;
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn budget() -> InspectionBudget {
    InspectionBudget::new(Default::default(), Arc::new(AtomicBool::new(false))).unwrap()
}
struct Fixture {
    root: PathBuf,
    origin: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/evidence-job-journal-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let origin = root.join("synthetic-native-origin");
        fs::create_dir(&origin).unwrap();
        Self { root, origin }
    }
    fn open(&self) -> EvidenceJournal {
        EvidenceJournal::open(&self.origin, true, &budget())
            .unwrap()
            .unwrap()
    }
    fn request(&self, n: usize) -> EvidenceJobRequest {
        EvidenceJobRequest {
            version: 1,
            job_id: format!("{n:032x}"),
            origin_root: self.origin.to_str().unwrap().into(),
            operation: EvidenceOperation::Export {
                scope: omniusong_store::EVIDENCE_SCOPE.into(),
                export_id: format!("{:032x}", n + 1000),
                destination: self.root.join(format!("artifact-{n}.osre")),
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Evidence journal fixture retained: {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn fresh(
    journal: &EvidenceJournal,
    request: &EvidenceJobRequest,
    meter: &InspectionBudget,
) -> JobEntry {
    match journal.admit(request, meter).unwrap() {
        Admission::Fresh(entry) => entry,
        Admission::Existing(_) => panic!("expected a fresh immutable request"),
    }
}
fn stopped() -> EvidenceOutcome {
    EvidenceOutcome::Stopped {
        problem: EvidenceProblem {
            kind: EvidenceProblemKind::Cancelled,
            origin: EvidenceResourceOrigin::Evidence,
            detail: EvidenceMessage::new("synthetic stop; no artifact work"),
            sqlite_code: None,
            budget_stop: Some(EvidenceBudgetStop::Cancelled),
        },
    }
}
fn exported(request: &EvidenceJobRequest) -> EvidenceOutcome {
    let EvidenceOperation::Export {
        export_id,
        destination,
        ..
    } = &request.operation
    else {
        panic!("expected export fixture")
    };
    EvidenceOutcome::Exported {
        receipt: EvidenceArchiveReceipt {
            path: destination.clone(),
            export_id: export_id.clone(),
            sha256: "c".repeat(64),
            byte_length: 1000,
            file_count: 1,
            directory_count: 0,
            payload_bytes: 800,
        },
    }
}
fn error<T>(result: Result<T, ServiceError>) -> ServiceError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected refusal"),
    }
}
fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
#[derive(Debug, PartialEq, Eq)]
enum Item {
    Directory(u64, u32),
    File(u64, u32, u64, Vec<u8>),
    Link(PathBuf),
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Item> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Item>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let item = if metadata.file_type().is_symlink() {
            Item::Link(fs::read_link(path).unwrap())
        } else if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), out);
            }
            Item::Directory(metadata.ino(), metadata.mode())
        } else {
            Item::File(
                metadata.ino(),
                metadata.mode(),
                metadata.nlink(),
                fs::read(path).unwrap(),
            )
        };
        out.insert(path.strip_prefix(root).unwrap().to_owned(), item);
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
fn independent_history(path: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.EvidenceJobHistory.v1\0");
    let mut ordered = vec![
        "request.json".to_owned(),
        "started.json".into(),
        "terminal.json".into(),
    ];
    ordered.extend((1..=8).map(|n| format!("observation-{n:02}.json")));
    for name in ordered {
        let file = path.join(&name);
        if file.exists() {
            let bytes = fs::read(file).unwrap();
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
    }
    format!("{:x}", hash.finalize())
}
fn fault(journal: &EvidenceJournal, boundary: Boundary, slot: &'static str) {
    *journal.observer.borrow_mut() = Some(Box::new(move |point, path| {
        if point == boundary && path.file_name().is_some_and(|name| name == slot) {
            return Err(ServiceError::backend(
                "injected metadata boundary failure",
                true,
            ));
        }
        Ok(())
    }));
}

#[test]
fn read_only_open_is_absent_without_creation_and_survives_missing_origin() {
    let fixture = Fixture::new();
    let before = tree(&fixture.root);
    assert!(
        EvidenceJournal::open(&fixture.origin, false, &budget())
            .unwrap()
            .is_none()
    );
    assert_eq!(tree(&fixture.root), before);
    let journal = fixture.open();
    let request = fixture.request(1);
    let entry = fresh(&journal, &request, &budget());
    let history = entry.view.history_sha256.clone();
    drop(entry);
    drop(journal);
    fs::remove_dir(&fixture.origin).unwrap();
    let before = tree(&fixture.root);
    let journal = EvidenceJournal::open(&fixture.origin, false, &budget())
        .unwrap()
        .unwrap();
    let list = journal.list(&budget()).unwrap();
    assert_eq!(list.jobs.len(), 1);
    assert_eq!(list.jobs[0].phase, EvidenceJobPhase::Unresolved);
    let existing = journal
        .read_if_present(&request.job_id, &budget())
        .unwrap()
        .unwrap();
    assert_eq!(existing.view.history_sha256, history);
    assert!(
        journal
            .read_if_present(&format!("{:032x}", 2), &budget())
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        journal.admit(&request, &budget()).unwrap(),
        Admission::Existing(_)
    ));
    assert!(!request.destination().exists());
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn request_envelope_and_full_raw_history_have_independent_hash_oracles() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    let request = fixture.request(1);
    let entry = fresh(&journal, &request, &budget());
    let raw = fs::read(entry.view.directory.join("request.json")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let body = serde_json::to_vec(&request).unwrap();
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"OmniuSong.EvidenceJobRequest.v1\0");
    fingerprint.update(&body);
    assert_eq!(
        value["request_sha256"].as_str().unwrap(),
        format!("{:x}", fingerprint.finalize())
    );
    let mut body_hash = Sha256::new();
    body_hash.update(b"OmniuSong.EvidenceJobRecordBody.v1\0");
    body_hash.update(&body);
    assert_eq!(
        value["body_sha256"].as_str().unwrap(),
        format!("{:x}", body_hash.finalize())
    );
    assert_eq!(
        entry.view.history_sha256,
        independent_history(&entry.view.directory)
    );
    let entry = journal.mark_started(entry, &budget()).unwrap();
    let permit = journal.reserve_finalization(&budget()).unwrap();
    let entry = journal
        .complete(entry, &exported(&request), permit)
        .unwrap();
    assert_eq!(
        entry.view.history_sha256,
        independent_history(&entry.view.directory)
    );
    assert!(
        !request.destination().exists(),
        "historical metadata never executes/checks the claimed artifact"
    );
    drop(entry);
    drop(journal);
    let journal = fixture.open();
    let list = journal.list(&budget()).unwrap();
    assert_eq!(list.jobs[0].phase, EvidenceJobPhase::HistoricalComplete);
    assert!(!request.destination().exists());
}

#[test]
fn exact_existing_request_never_reexecutes_and_lexical_aliases_refuse() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    let request = fixture.request(1);
    let entry = fresh(&journal, &request, &budget());
    drop(entry);
    let before = tree(&fixture.root);
    assert!(matches!(
        journal.admit(&request, &budget()).unwrap(),
        Admission::Existing(_)
    ));
    let mut alias = request.clone();
    if let EvidenceOperation::Export { destination, .. } = &mut alias.operation {
        *destination = fixture.root.join(".").join("artifact-1.osre");
    }
    assert_eq!(alias.destination(), request.destination());
    assert_ne!(
        alias.destination().as_os_str(),
        request.destination().as_os_str()
    );
    error(journal.admit(&alias, &budget()));
    assert_eq!(tree(&fixture.root), before);
    assert!(!request.destination().exists());
}

#[test]
fn immutable_requests_reserve_destinations_and_refuse_journal_inputs_or_self_outputs() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    let first = fixture.request(1);
    drop(fresh(&journal, &first, &budget()));
    let mut second = fixture.request(2);
    if let EvidenceOperation::Export { destination, .. } = &mut second.operation {
        *destination = first.destination().to_owned();
    }
    error(journal.admit(&second, &budget()));
    for destination in [
        fixture.origin.join("inside.osre"),
        journal.path.join("self.osre"),
        fixture.root.join(".").join("alias.osre"),
        PathBuf::from(format!("{}//alias.osre", fixture.root.display())),
        fixture.origin.join("..").join("alias.osre"),
        PathBuf::from(format!("{}/alias.osre/", fixture.root.display())),
    ] {
        let mut request = fixture.request(3);
        if let EvidenceOperation::Export {
            destination: selected,
            ..
        } = &mut request.operation
        {
            *selected = destination;
        }
        let before = tree(&fixture.root);
        error(journal.admit(&request, &budget()));
        assert_eq!(tree(&fixture.root), before);
    }
    let request = EvidenceJobRequest {
        operation: EvidenceOperation::Restore {
            archive_path: journal.path.join("namespace.json"),
            directory: fixture.root.join("new-inert"),
            expected_archive_sha256: "d".repeat(64),
        },
        ..fixture.request(4)
    };
    let before = tree(&fixture.root);
    error(journal.admit(&request, &budget()));
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn initialization_faults_retain_partial_namespaces_without_repair() {
    for target in ["namespace", ".owner.lock", "namespace.json"] {
        let fixture = Fixture::new();
        error(EvidenceJournal::open_using(
            &fixture.origin,
            true,
            &budget(),
            |point, path| {
                let selected = if target == "namespace" {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(PREFIX)
                } else {
                    path.file_name().is_some_and(|name| name == target)
                };
                if selected && point == Boundary::Created {
                    Err(ServiceError::backend(
                        "synthetic initialization failure",
                        true,
                    ))
                } else {
                    Ok(())
                }
            },
        ));
        let before = tree(&fixture.root);
        error(EvidenceJournal::open(&fixture.origin, false, &budget()));
        assert_eq!(tree(&fixture.root), before);
        error(EvidenceJournal::open(&fixture.origin, true, &budget()));
        assert_eq!(tree(&fixture.root), before);
    }
    let fixture = Fixture::new();
    let parked = fixture.root.join("parked-created-namespace");
    error(EvidenceJournal::open_using(
        &fixture.origin,
        true,
        &budget(),
        |point, path| {
            if point == Boundary::Created
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(PREFIX)
            {
                fs::rename(path, &parked).unwrap();
                create_directory(path).unwrap();
            }
            Ok(())
        },
    ));
    assert!(fs::read_dir(&parked).unwrap().next().is_none());
}

#[test]
fn every_request_and_started_write_sync_boundary_remains_an_existing_uncertain_job() {
    for started in [false, true] {
        for point in [
            Boundary::Created,
            Boundary::Written,
            Boundary::FileSynced,
            Boundary::DirectorySynced,
        ] {
            let fixture = Fixture::new();
            let journal = fixture.open();
            let request = fixture.request(1);
            if started {
                let entry = fresh(&journal, &request, &budget());
                fault(&journal, point, "started.json");
                error(journal.mark_started(entry, &budget()));
            } else {
                fault(&journal, point, "request.json");
                error(journal.admit(&request, &budget()));
            }
            drop(journal);
            let journal = fixture.open();
            let entry = journal
                .read_if_present(&request.job_id, &budget())
                .unwrap()
                .unwrap();
            assert!(!request.destination().exists());
            assert_eq!(
                entry.view.history_sha256,
                independent_history(&entry.view.directory)
            );
            let before = tree(&fixture.root);
            let result = journal.admit(&request, &budget());
            if entry.view.request.is_some() {
                assert!(matches!(result.unwrap(), Admission::Existing(_)));
            } else {
                error(result);
            }
            assert_eq!(tree(&fixture.root), before);
        }
    }
}

#[test]
fn prepaid_finalization_records_a_sticky_stop_without_charging_or_resetting_the_governor() {
    for limit in [false, true] {
        let fixture = Fixture::new();
        let journal = fixture.open();
        let request = fixture.request(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let meter = InspectionBudget::new(Default::default(), Arc::clone(&cancelled)).unwrap();
        let entry = fresh(&journal, &request, &meter);
        let permit = journal.reserve_finalization(&meter).unwrap();
        let entry = journal.mark_started(entry, &meter).unwrap();
        if limit {
            assert!(
                meter
                    .charge_work(omniusong_recording::InspectionLimits::default().max_work_bytes)
                    .is_err()
            );
        } else {
            cancelled.store(true, Ordering::Release);
        }
        let usage = meter.usage();
        let entry = journal.complete(entry, &stopped(), permit).unwrap();
        assert_eq!(meter.usage(), usage);
        assert!(meter.checkpoint().is_err());
        assert!(entry.view.terminal.is_some());
        assert_eq!(
            entry.view.history_sha256,
            independent_history(&entry.view.directory)
        );
        cancelled.store(false, Ordering::Release);
        assert!(meter.checkpoint().is_err());
        drop(entry);
        drop(journal);
        let journal = fixture.open();
        assert_eq!(
            journal
                .read(&request.job_id, &budget())
                .unwrap()
                .view
                .terminal,
            Some(stopped())
        );
        assert!(!request.destination().exists());
    }
}

#[test]
fn prepaid_finalization_handles_quote_heavy_near_bound_restore_receipts_after_cancel() {
    let fixture = Fixture::new();
    let archive_path = fixture.root.join(format!("{}.osre", "\"".repeat(3000)));
    let directory = fixture.root.join(format!("{}-inert", "\"".repeat(3000)));
    let sha = "e".repeat(64);
    let request = EvidenceJobRequest {
        operation: EvidenceOperation::Restore {
            archive_path: archive_path.clone(),
            directory: directory.clone(),
            expected_archive_sha256: sha.clone(),
        },
        ..fixture.request(1)
    };
    let outcome = EvidenceOutcome::Restored {
        receipt: EvidenceRestoreReceipt {
            directory,
            archive: EvidenceArchiveReceipt {
                path: archive_path,
                export_id: "7".repeat(32),
                sha256: sha,
                byte_length: 1000,
                file_count: 1,
                directory_count: 0,
                payload_bytes: 800,
            },
        },
    };
    assert!(serde_json::to_vec(&outcome).unwrap().len() > 12_000);
    // Seed a canonical historical request image directly. Its bounded lexical
    // path fields stress escaping without depending on this host's PATH_MAX,
    // creating long artifact paths, or executing any artifact operation.
    let journal = fixture.open();
    let path = journal.path.join(&request.job_id);
    drop(journal);
    create_directory(&path).unwrap();
    write_private(
        &path.join("request.json"),
        &envelope(
            &request,
            &request.fingerprint().unwrap(),
            Kind::Request,
            0,
            MAX_EVIDENCE_JOB_REQUEST_BYTES,
        )
        .unwrap(),
    );
    let journal = fixture.open();
    let cancelled = Arc::new(AtomicBool::new(false));
    let meter = InspectionBudget::new(Default::default(), Arc::clone(&cancelled)).unwrap();
    let entry = journal.read(&request.job_id, &meter).unwrap();
    let permit = journal.reserve_finalization(&meter).unwrap();
    let entry = journal.mark_started(entry, &meter).unwrap();
    cancelled.store(true, Ordering::Release);
    let usage = meter.usage();
    let entry = journal.complete(entry, &outcome, permit).unwrap();
    assert_eq!(entry.view.terminal, Some(outcome));
    assert_eq!(meter.usage(), usage);
    assert!(meter.checkpoint().is_err());
    assert_eq!(
        entry.view.history_sha256,
        independent_history(&entry.view.directory)
    );
    assert!(!request.destination().exists());
}

#[test]
fn finalization_allowance_is_owner_bound_and_occupied_terminals_never_overwrite() {
    let a = Fixture::new();
    let ja = a.open();
    let permit = ja.reserve_finalization(&budget()).unwrap();
    let b = Fixture::new();
    let jb = b.open();
    let request = b.request(1);
    let entry = fresh(&jb, &request, &budget());
    let entry = jb.mark_started(entry, &budget()).unwrap();
    error(jb.complete(entry, &stopped(), permit));
    assert!(!jb.path.join(&request.job_id).join("terminal.json").exists());
    let entry = jb.read(&request.job_id, &budget()).unwrap();
    let permit = jb.reserve_finalization(&budget()).unwrap();
    let entry = jb.complete(entry, &stopped(), permit).unwrap();
    drop(entry);
    let before = tree(&b.root);
    let entry = jb.read(&request.job_id, &budget()).unwrap();
    let permit = jb.reserve_finalization(&budget()).unwrap();
    error(jb.complete(entry, &exported(&request), permit));
    assert_eq!(tree(&b.root), before);
}

#[test]
fn terminal_and_observation_faults_retain_partial_or_complete_slots_without_cleanup() {
    for observation in [false, true] {
        for point in [
            Boundary::Created,
            Boundary::Written,
            Boundary::FileSynced,
            Boundary::DirectorySynced,
        ] {
            let fixture = Fixture::new();
            let journal = fixture.open();
            let request = fixture.request(1);
            let entry = fresh(&journal, &request, &budget());
            let entry = journal.mark_started(entry, &budget()).unwrap();
            let permit = journal.reserve_finalization(&budget()).unwrap();
            let name = if observation {
                "observation-01.json"
            } else {
                "terminal.json"
            };
            fault(&journal, point, name);
            if observation {
                error(journal.observe(entry, &stopped(), permit));
            } else {
                error(journal.complete(entry, &stopped(), permit));
            }
            drop(journal);
            let journal = fixture.open();
            let entry = journal.read(&request.job_id, &budget()).unwrap();
            assert!(entry.view.directory.join(name).exists());
            assert_eq!(
                entry.view.history_sha256,
                independent_history(&entry.view.directory)
            );
            if observation {
                assert_eq!(entry.view.occupied_observations, 1);
                let previous = entry.view.history_sha256.clone();
                let permit = journal.reserve_finalization(&budget()).unwrap();
                let entry = journal.observe(entry, &stopped(), permit).unwrap();
                assert_eq!(entry.view.observations.last().unwrap().sequence, 2);
                assert_eq!(
                    entry
                        .view
                        .observations
                        .last()
                        .unwrap()
                        .previous_history_sha256,
                    previous
                );
            } else {
                let before = tree(&fixture.root);
                let permit = journal.reserve_finalization(&budget()).unwrap();
                error(journal.complete(entry, &stopped(), permit));
                assert_eq!(tree(&fixture.root), before);
            }
            assert!(!request.destination().exists());
        }
    }
}

#[test]
fn sync_boundary_replacements_in_place_rewrites_and_surplus_slots_never_acknowledge_history() {
    for observation in [false, true] {
        for point in [Boundary::FileSynced, Boundary::DirectorySynced] {
            for mutation in 0..3 {
                let fixture = Fixture::new();
                let journal = fixture.open();
                let request = fixture.request(1);
                let entry = fresh(&journal, &request, &budget());
                let entry = journal.mark_started(entry, &budget()).unwrap();
                // The synthetic artifact represents an independently obtained primary
                // outcome. Metadata refusal must neither remove nor rewrite it.
                fs::write(request.destination(), b"synthetic completed artifact").unwrap();
                let artifact = fs::read(request.destination()).unwrap();
                let old_history = entry.view.history_sha256.clone();
                let target = if observation {
                    "observation-01.json"
                } else {
                    "terminal.json"
                };
                let saved = fixture.root.join("parked-written-record");
                *journal.observer.borrow_mut() = Some(Box::new(move |boundary, path| {
                    if boundary == point && path.file_name().is_some_and(|name| name == target) {
                        if mutation == 0 {
                            fs::rename(path, &saved).unwrap();
                            write_private(path, b"different replacement bytes");
                        } else if mutation == 1 {
                            write_private(
                                &path.parent().unwrap().join("surplus.bin"),
                                b"unexpected occupied slot",
                            );
                        } else {
                            let mut bytes = fs::read(path).unwrap();
                            bytes[0] ^= 1;
                            fs::write(path, bytes).unwrap();
                            File::open(path)
                                .unwrap()
                                .set_times(fs::FileTimes::new().set_modified(
                                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(1),
                                ))
                                .unwrap();
                        }
                    }
                    Ok(())
                }));
                let permit = journal.reserve_finalization(&budget()).unwrap();
                let primary = exported(&request);
                if observation {
                    error(journal.observe(entry, &primary, permit));
                } else {
                    error(journal.complete(entry, &primary, permit));
                }
                assert_eq!(fs::read(request.destination()).unwrap(), artifact);
                assert!(journal.path.join(&request.job_id).join(target).exists());
                assert_ne!(
                    independent_history(&journal.path.join(&request.job_id)),
                    old_history
                );
                let before = tree(&fixture.root);
                drop(journal);
                let journal = fixture.open();
                if mutation == 1 {
                    error(journal.read(&request.job_id, &budget()));
                } else {
                    let entry = journal.read(&request.job_id, &budget()).unwrap();
                    assert!(entry.view.terminal.is_none());
                    assert!(!entry.view.metadata_problems.is_empty());
                }
                assert_eq!(tree(&fixture.root), before);
            }
        }
    }
}

#[test]
fn request_and_started_writes_refuse_same_byte_inode_substitution_or_unexpected_members() {
    for started in [false, true] {
        for surplus in [false, true] {
            let fixture = Fixture::new();
            let journal = fixture.open();
            let request = fixture.request(1);
            let entry = if started {
                Some(fresh(&journal, &request, &budget()))
            } else {
                None
            };
            let target = if started {
                "started.json"
            } else {
                "request.json"
            };
            let parked = fixture.root.join("parked-write");
            *journal.observer.borrow_mut() = Some(Box::new(move |point, path| {
                if point == Boundary::DirectorySynced
                    && path.file_name().is_some_and(|name| name == target)
                {
                    if surplus {
                        write_private(&path.parent().unwrap().join("surplus.bin"), b"untracked");
                    } else {
                        fs::rename(path, &parked).unwrap();
                        fs::copy(&parked, path).unwrap();
                    }
                }
                Ok(())
            }));
            if let Some(entry) = entry {
                error(journal.mark_started(entry, &budget()));
            } else {
                error(journal.admit(&request, &budget()));
            }
            assert!(!request.destination().exists());
        }
    }
}

#[test]
fn malformed_later_slots_are_hashed_and_allow_only_explicit_bounded_observation() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    let request = fixture.request(1);
    let entry = fresh(&journal, &request, &budget());
    let entry = journal.mark_started(entry, &budget()).unwrap();
    let path = entry.view.directory.clone();
    drop(entry);
    drop(journal);
    write_private(&path.join("terminal.json"), b"\0malformed terminal\xff");
    write_private(&path.join("observation-03.json"), b"partial");
    let journal = fixture.open();
    let entry = journal.read(&request.job_id, &budget()).unwrap();
    assert!(entry.view.request.is_some() && entry.view.terminal.is_none());
    assert_eq!(entry.view.occupied_observations, 3);
    assert!(entry.view.metadata_problems.len() >= 2);
    assert_eq!(entry.view.history_sha256, independent_history(&path));
    let previous = entry.view.history_sha256.clone();
    let permit = journal.reserve_finalization(&budget()).unwrap();
    let entry = journal.observe(entry, &stopped(), permit).unwrap();
    assert_eq!(entry.view.observations.last().unwrap().sequence, 4);
    assert_eq!(
        entry
            .view
            .observations
            .last()
            .unwrap()
            .previous_history_sha256,
        previous
    );
    assert!(!path.join("observation-01.json").exists());
    assert_eq!(
        fs::read(path.join("terminal.json")).unwrap(),
        b"\0malformed terminal\xff"
    );
}

#[test]
fn invalid_prior_requests_block_fresh_reservations_and_empty_records_are_not_absent() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    let path = journal.path.join(format!("{:032x}", 1));
    drop(journal);
    create_directory(&path).unwrap();
    let journal = fixture.open();
    let missing = journal.read(&format!("{:032x}", 1), &budget()).unwrap();
    let old_history = missing.view.history_sha256.clone();
    drop(missing);
    drop(journal);
    write_private(&path.join("request.json"), b"");
    let journal = fixture.open();
    let entry = journal
        .read_if_present(&format!("{:032x}", 1), &budget())
        .unwrap()
        .unwrap();
    assert!(entry.view.request.is_none());
    assert_ne!(entry.view.history_sha256, old_history);
    let before = tree(&fixture.root);
    error(journal.admit(&fixture.request(2), &budget()));
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn eight_observations_and_128_occupied_job_directories_are_hard_caps_without_eviction() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    let request = fixture.request(1);
    let mut entry = fresh(&journal, &request, &budget());
    for sequence in 1..=8 {
        let previous = entry.view.history_sha256.clone();
        let permit = journal.reserve_finalization(&budget()).unwrap();
        entry = journal.observe(entry, &stopped(), permit).unwrap();
        assert_eq!(entry.view.occupied_observations, sequence);
        assert_eq!(
            entry
                .view
                .observations
                .last()
                .unwrap()
                .previous_history_sha256,
            previous
        );
    }
    let before = tree(&fixture.root);
    let permit = journal.reserve_finalization(&budget()).unwrap();
    error(journal.observe(entry, &stopped(), permit));
    assert_eq!(tree(&fixture.root), before);
    drop(journal);
    let fixture = Fixture::new();
    let journal = fixture.open();
    let root = journal.path.clone();
    drop(journal);
    for n in 1..=128 {
        create_directory(&root.join(format!("{n:032x}"))).unwrap();
    }
    let journal = fixture.open();
    assert_eq!(journal.list(&budget()).unwrap().jobs.len(), 128);
    let before = tree(&fixture.root);
    error(journal.admit(&fixture.request(129), &budget()));
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn unknown_linked_oversized_or_substituted_metadata_refuses_without_repair() {
    for case in 0..6 {
        let fixture = Fixture::new();
        let journal = fixture.open();
        let request = fixture.request(1);
        let entry = fresh(&journal, &request, &budget());
        let path = entry.view.directory.clone();
        drop(entry);
        let namespace = journal.path.clone();
        drop(journal);
        match case {
            0 => write_private(&path.join("unknown.bin"), b"unknown"),
            1 => write_private(&path.join("request.json"), &vec![b' '; 16_385]),
            2 => {
                fs::hard_link(path.join("request.json"), fixture.root.join("alias.json")).unwrap();
            }
            3 => {
                let saved = fixture.root.join("saved-request");
                fs::rename(path.join("request.json"), &saved).unwrap();
                std::os::unix::fs::symlink(saved, path.join("request.json")).unwrap();
            }
            4 => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            _ => {
                let saved = fixture.root.join("saved-owner");
                fs::rename(namespace.join(".owner.lock"), &saved).unwrap();
                std::os::unix::fs::symlink(saved, namespace.join(".owner.lock")).unwrap();
            }
        }
        let before = tree(&fixture.root);
        match EvidenceJournal::open(&fixture.origin, false, &budget()) {
            Ok(Some(journal)) => {
                error(journal.read(&request.job_id, &budget()));
            }
            Err(_) => (),
            Ok(None) => panic!("invalid namespace looked absent"),
        }
        assert_eq!(tree(&fixture.root), before);
    }
    let fixture = Fixture::new();
    let journal = fixture.open();
    let request = fixture.request(1);
    let entry = fresh(&journal, &request, &budget());
    let raw = entry.view.directory.join("request.json");
    let saved = fixture.root.join("parked-request");
    fs::rename(&raw, &saved).unwrap();
    fs::copy(saved, &raw).unwrap();
    let before = tree(&fixture.root);
    error(journal.verify_entry(&entry, &budget()));
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn process_and_os_owner_guards_are_nonblocking_and_namespaces_are_independent() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    assert_eq!(
        error(EvidenceJournal::open(&fixture.origin, false, &budget())).kind,
        FailureKind::SourceBusy
    );
    fn probe(origin: &Path, expected_busy: bool) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "evidence_jobs::journal::tests::owner_probe_subprocess",
                "--ignored",
                "--nocapture",
            ])
            .env("OMNIUSONG_JOURNAL_TEST_ORIGIN", origin)
            .env(
                "OMNIUSONG_JOURNAL_TEST_BUSY",
                if expected_busy { "yes" } else { "no" },
            )
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .contains("JOURNAL_OWNER_PROBE_COMPLETED"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    probe(&fixture.origin, true);
    let other = Fixture::new();
    let _other_journal = other.open();
    drop(journal);
    probe(&fixture.origin, false);
}

#[test]
#[ignore = "isolated subprocess owner-lock probe"]
fn owner_probe_subprocess() {
    let origin = PathBuf::from(
        std::env::var_os("OMNIUSONG_JOURNAL_TEST_ORIGIN")
            .expect("explicit synthetic journal origin required"),
    );
    let expected_busy = std::env::var("OMNIUSONG_JOURNAL_TEST_BUSY").unwrap() == "yes";
    let result = EvidenceJournal::open(&origin, false, &budget());
    if expected_busy {
        assert_eq!(error(result).kind, FailureKind::SourceBusy);
    } else {
        assert!(result.unwrap().is_some());
    }
    println!("JOURNAL_OWNER_PROBE_COMPLETED");
}
