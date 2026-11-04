use super::*;

fn injected_code(code: i32) -> StoreError {
    // Error-constructor classification only, not physical corruption testing.
    StoreError::Database(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(code),
        Some("explicit classification test".into()),
    ))
}

#[test]
fn input_and_private_output_sql_codes_never_impersonate_actual_catalog_faults() {
    for code in [10, 11, 26, 10 | (1 << 8), 5, 19] {
        for origin in [
            ArchiveSqlOrigin::Input,
            ArchiveSqlOrigin::PrivateOutput,
            ArchiveSqlOrigin::LiveCatalog,
        ] {
            let error = tag::<()>(Err(injected_code(code)), origin).unwrap_err();
            assert_eq!(
                error.catalog_sqlite_error_code(),
                (origin == ArchiveSqlOrigin::LiveCatalog).then_some(code)
            );
            assert_eq!(
                error.catalog_connection_unusable(),
                origin == ArchiveSqlOrigin::LiveCatalog && matches!(code & 0xff, 10 | 11 | 26)
            );
        }
    }
}

#[test]
fn mixed_private_boundary_never_relabels_nested_live_error_or_archive_context() {
    let actual = live::<()>(Err(injected_code(26))).unwrap_err();
    let preserved = input::<()>(Err(actual)).unwrap_err();
    assert!(
        matches!(&preserved,StoreError::ArchiveSql(error) if error.origin()==ArchiveSqlOrigin::LiveCatalog)
    );
    let mut context = ArchiveContext::new(ArchiveAction::Restore, Path::new("destination"));
    context.phase = ArchivePhase::CatalogCommitAttempted;
    let preserved = output::<()>(Err(StoreError::Archive(Box::new(ArchiveFailure::new(
        context.clone(),
        preserved,
    )))))
    .unwrap_err();
    assert!(preserved.archive_publication_uncertain());
    assert!(preserved.catalog_connection_unusable());
    assert_eq!(preserved.catalog_sqlite_error_code(), Some(26));
    assert!(matches!(preserved,StoreError::Archive(error) if error.context()==&context));
}

#[test]
fn cleanup_error_keeps_completed_value_and_exact_private_path() {
    let path = Path::new("isolated-private-cleanup");
    let outcome = with_test_hook(
        Box::new(|event, _| {
            assert_eq!(event, TestEvent::PrivateCleanup);
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "explicit test cleanup denial",
            ))
        }),
        || {
            let mut outcome = ArchiveOutcome::new(
                "already committed receipt",
                ArchiveCompletion::CatalogCommitted,
            );
            outcome.cleanup_tree(path);
            outcome
        },
    );
    assert_eq!(outcome.value, "already committed receipt");
    assert_eq!(outcome.completion, ArchiveCompletion::CatalogCommitted);
    assert_eq!(outcome.cleanup.len(), 1);
    assert_eq!(outcome.cleanup[0].path, path);
    assert_eq!(
        outcome.cleanup[0].error.kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn test_hook_scope_restores_outer_callback_even_on_panic() {
    with_test_hook(
        Box::new(|_, _| Err(io::Error::other("outer callback"))),
        || {
            let panic = std::panic::catch_unwind(|| {
                with_test_hook(Box::new(|_, _| Ok(())), || {
                    panic!("explicit hook scope test")
                })
            });
            assert!(panic.is_err());
            let error = checkpoint(
                TestEvent::CopyCommit,
                &ArchiveContext::new(ArchiveAction::Copy, Path::new("isolated")),
            )
            .unwrap_err();
            assert!(error.to_string().contains("outer callback"));
        },
    );
    checkpoint(
        TestEvent::CopyCommit,
        &ArchiveContext::new(ArchiveAction::Copy, Path::new("isolated")),
    )
    .unwrap();
}
