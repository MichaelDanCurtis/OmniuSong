use super::*;
use std::io;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/archive-outcome-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn store(&self, name: &str) -> NativeStore {
        NativeStore::open(self.0.join(name)).unwrap()
    }
    fn source(&self) -> (NativeStore, ProjectDocument) {
        let mut source = self.store("source");
        let project = source
            .create(&ProjectDocument::new("Exact archive outcome").unwrap())
            .unwrap();
        let project = source
            .apply_edit(&ProjectEditIntent {
                operation_id: omniusong_core::contracts::OperationId("outcome-rename".into()),
                project_id: omniusong_core::contracts::ProjectId(project.id().into()),
                expected_revision: project.revision(),
                scope: EditScope::Project,
                edit: EditOperation::Rename {
                    name: "Retained exact history".into(),
                },
                extensions: Default::default(),
            })
            .unwrap()
            .document;
        (source, project)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Archive outcome fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn injected() -> io::Error {
    io::Error::other("isolated archive boundary fault")
}
#[test]
fn actual_export_directory_cleanup_refuses_unexpected_private_entry_after_durable_completion() {
    let fixture = Fixture::new();
    let (source, project) = fixture.source();
    let path = fixture.0.join("unexpected-stage-entry.osar");
    let outcome = boundary::with_test_hook(
        Box::new(|event, context| {
            if event == boundary::TestEvent::ExportSync {
                fs::write(
                    context
                        .staging
                        .as_ref()
                        .unwrap()
                        .join("unexpected-retained-entry"),
                    b"retain this diagnostic",
                )?;
            }
            Ok(())
        }),
        || source.export_archive_outcome(project.id(), &path),
    )
    .unwrap();
    assert_eq!(outcome.completion, ArchiveCompletion::ExportDurable);
    assert_eq!(checksum(&fs::read(&path).unwrap()), outcome.value.sha256);
    assert_eq!(outcome.cleanup.len(), 1);
    assert!(outcome.cleanup[0].path.is_dir());
    assert_eq!(
        fs::read(outcome.cleanup[0].path.join("unexpected-retained-entry")).unwrap(),
        b"retain this diagnostic"
    );
    assert!(!outcome.cleanup[0].path.join("archive.sqlite3").exists());
}
#[test]
fn actual_export_and_identity_restore_report_completed_exact_results() {
    let fixture = Fixture::new();
    let (source, project) = fixture.source();
    let path = fixture.0.join("complete.osar");
    let exported = source.export_archive_outcome(project.id(), &path).unwrap();
    assert_eq!(exported.completion, ArchiveCompletion::ExportDurable);
    assert!(exported.cleanup.is_empty());
    assert!(exported.value.path.is_absolute());
    assert_eq!(exported.value.history_rows, 1);
    assert_eq!(exported.value.operation_rows, 1);
    assert_eq!(checksum(&fs::read(&path).unwrap()), exported.value.sha256);
    let mut destination = fixture.store("destination");
    let restored = destination.restore_archive_outcome(&path).unwrap();
    assert_eq!(restored.completion, ArchiveCompletion::CatalogCommitted);
    assert!(restored.cleanup.is_empty());
    assert_eq!(restored.value, project);
    assert_eq!(destination.load(project.id()).unwrap(), project);
    assert_eq!(count(&destination.connection, "edits").unwrap(), 1);
    assert_eq!(count(&destination.connection, "operations").unwrap(), 1);
    assert!(destination.connection.is_autocommit());
    assert_eq!(source.load(project.id()).unwrap(), project);
}
#[test]
fn attempted_link_and_confirmed_link_before_sync_have_distinct_failure_facts() {
    let fixture = Fixture::new();
    let (source, project) = fixture.source();
    for (event, phase, exists) in [
        (
            boundary::TestEvent::OutputLink,
            ArchivePhase::OutputLinkAttempted,
            false,
        ),
        (
            boundary::TestEvent::ExportSync,
            ArchivePhase::OutputLinked,
            true,
        ),
    ] {
        let path = fixture.0.join(if exists {
            "linked.osar"
        } else {
            "attempted.osar"
        });
        let result = boundary::with_test_hook(
            Box::new(move |actual, context| {
                if actual == event {
                    assert_eq!(context.phase, phase);
                    Err(injected())
                } else {
                    Ok(())
                }
            }),
            || source.export_archive_outcome(project.id(), &path),
        );
        let failure = result.unwrap_err();
        assert_eq!(failure.context().phase, phase);
        assert!(failure.publication_uncertain());
        assert_eq!(path.exists(), exists);
        assert_eq!(
            failure.context().destination,
            fs::canonicalize(&fixture.0)
                .unwrap()
                .join(path.file_name().unwrap())
        );
        let private = failure
            .context()
            .staging
            .as_ref()
            .unwrap()
            .join("archive.sqlite3");
        assert!(private.exists());
        assert_eq!(
            failure.context().archive_sha256.as_deref(),
            Some(checksum(&fs::read(&private).unwrap()).as_str())
        );
        if exists {
            assert_eq!(fs::read(&private).unwrap(), fs::read(&path).unwrap());
        }
        assert_eq!(source.load(project.id()).unwrap(), project);
    }
}
#[test]
fn cleanup_failures_preserve_durable_export_and_committed_restore() {
    let fixture = Fixture::new();
    let (source, project) = fixture.source();
    let path = fixture.0.join("cleanup.osar");
    let exported = boundary::with_test_hook(
        Box::new(|event, _| {
            if event == boundary::TestEvent::PrivateCleanup {
                Err(injected())
            } else {
                Ok(())
            }
        }),
        || source.export_archive_outcome(project.id(), &path),
    )
    .unwrap();
    assert_eq!(exported.completion, ArchiveCompletion::ExportDurable);
    assert_eq!(exported.cleanup.len(), 2);
    assert!(exported.cleanup.iter().all(|issue| issue.path.exists()));
    assert_eq!(checksum(&fs::read(&path).unwrap()), exported.value.sha256);
    let mut destination = fixture.store("destination");
    let restored = boundary::with_test_hook(
        Box::new(|event, _| {
            if event == boundary::TestEvent::PrivateCleanup {
                Err(injected())
            } else {
                Ok(())
            }
        }),
        || destination.restore_archive_outcome(&path),
    )
    .unwrap();
    assert_eq!(restored.completion, ArchiveCompletion::CatalogCommitted);
    assert_eq!(restored.cleanup.len(), 1);
    assert!(restored.cleanup[0].path.join("restore.json").exists());
    assert_eq!(restored.value, project);
    assert_eq!(destination.load(project.id()).unwrap(), project);
    assert!(destination.connection.is_autocommit());
}
#[test]
fn restore_commit_attempt_failure_rolls_back_exactly_and_preserves_usable_connection() {
    let fixture = Fixture::new();
    let (source, project) = fixture.source();
    let path = fixture.0.join("before-commit.osar");
    source.export_archive(project.id(), &path).unwrap();
    let mut destination = fixture.store("destination");
    let result = boundary::with_test_hook(
        Box::new(|event, context| {
            if event == boundary::TestEvent::RestoreCommit {
                assert_eq!(context.phase, ArchivePhase::CatalogCommitAttempted);
                Err(injected())
            } else {
                Ok(())
            }
        }),
        || destination.restore_archive_outcome(&path),
    );
    let failure = result.unwrap_err();
    assert_eq!(
        failure.context().phase,
        ArchivePhase::CatalogCommitAttempted
    );
    assert!(failure.publication_uncertain());
    assert!(
        failure
            .context()
            .staging
            .as_ref()
            .unwrap()
            .join("restore.json")
            .exists()
    );
    for table in [
        "projects",
        "edits",
        "operations",
        "imports",
        "assets",
        "native_takes",
        "archive_copies",
        "archive_project_lineage",
    ] {
        assert_eq!(count(&destination.connection, table).unwrap(), 0, "{table}");
    }
    assert!(destination.connection.is_autocommit());
    assert!(!failure.catalog_connection_unusable());
    destination
        .create(&ProjectDocument::new("Connection remains usable").unwrap())
        .unwrap();
}
#[test]
fn malformed_private_archive_keeps_input_sql_origin_and_legacy_wrapper_keeps_failure() {
    let fixture = Fixture::new();
    let path = fixture.0.join("malformed.osar");
    fs::write(&path, b"not a SQLite archive").unwrap();
    let mut destination = fixture.store("destination");
    let failure = destination.restore_archive_outcome(&path).unwrap_err();
    assert_eq!(failure.context().phase, ArchivePhase::NotStarted);
    assert!(!failure.publication_uncertain());
    let StoreError::ArchiveSql(sql) = failure.operation_error() else {
        panic!("Lost input SQL origin: {failure:?}");
    };
    assert_eq!(sql.origin(), ArchiveSqlOrigin::Input);
    assert_eq!(failure.catalog_sqlite_error_code(), None);
    assert!(!failure.catalog_connection_unusable());
    assert!(destination.connection.is_autocommit());
    let legacy = destination.restore_archive(&path).unwrap_err();
    assert!(matches!(legacy, StoreError::Archive(_)));
    assert_eq!(legacy.catalog_sqlite_error_code(), None);
}
#[test]
fn mixed_metadata_destinations_keep_their_sql_origin_through_opposite_outer_tags() {
    let fixture = Fixture::new();
    let (source, project) = fixture.source();
    let broken_output = Connection::open_in_memory().unwrap();
    let output = boundary::live(copy_metadata(
        &source.connection,
        Some(Destination::Archive(&broken_output)),
        project.id(),
    ))
    .err()
    .unwrap();
    let StoreError::ArchiveSql(sql) = output else {
        panic!("Lost private output tag");
    };
    assert_eq!(sql.origin(), ArchiveSqlOrigin::PrivateOutput);
    assert_eq!(sql.catalog_sqlite_error_code(), None);
    let path = fixture.0.join("mixed.osar");
    source.export_archive(project.id(), &path).unwrap();
    let input = readonly(&path).unwrap();
    let broken_live = Connection::open_in_memory().unwrap();
    let error = boundary::input(copy_metadata(
        &input,
        Some(Destination::Store(&broken_live)),
        project.id(),
    ))
    .err()
    .unwrap();
    let StoreError::ArchiveSql(sql) = error else {
        panic!("Lost live destination tag");
    };
    assert_eq!(sql.origin(), ArchiveSqlOrigin::LiveCatalog);
    assert!(sql.catalog_sqlite_error_code().is_some());
}
