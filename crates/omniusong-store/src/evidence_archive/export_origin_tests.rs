//! Real SQLite failures on isolated archive/catalog fixtures. Domain assignment
//! is observed through actual export paths, not inferred from error messages.
use super::*;
use crate::{EvidenceFailureOrigin, evidence_failure};

#[test]
fn rehashed_archived_sqlite_corruption_remains_local_and_live_store_stays_healthy() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("typed-origin-valid.osre");
    let mut parsed = parse_independent(&receipt.path);
    let before = catalog(&fixture.store.connection);
    let files = tree(fixture.store.root());
    // Preserve the valid SQLite header but corrupt the sqlite_schema btree.
    // Both OSRE checksums and the object hash are then recomputed independently.
    let bytes = parsed.payload.get_mut("catalog.sqlite3").unwrap();
    bytes[100] = 0xff;
    parsed
        .manifest
        .entries
        .iter_mut()
        .find(|e| e.path == "catalog.sqlite3")
        .unwrap()
        .sha256 = Some(sha(bytes));
    let corrupt = bundle(&parsed.manifest, &parsed.payload, None);
    let path = fixture.archive("typed-origin-corrupt.osre");
    fs::write(&path, &corrupt).unwrap();
    parse_independent(&path);
    let error = fixture
        .store
        .export_retained_evidence_full_root_typed(&path, EXPORT, &budget())
        .unwrap_err();
    assert_eq!(error.origin(), EvidenceFailureOrigin::Evidence);
    let StoreError::Database(sqlite) = error.operation_error() else {
        panic!("Expected actual archived SQLite error: {error:?}")
    };
    assert_eq!(sqlite.sqlite_error().unwrap().extended_code & 0xff, 11);
    assert_eq!(error.catalog_sqlite_error_code(), None);
    assert!(error.budget_stop().is_none());
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert_eq!(catalog(&fixture.store.connection), before);
    assert_eq!(tree(fixture.store.root()), files);
    // A subsequent valid replay and native query use the same healthy handle.
    assert_eq!(
        fixture
            .store
            .export_retained_evidence_full_root_typed(&receipt.path, EXPORT, &budget())
            .unwrap(),
        receipt
    );
    assert!(!fixture.store.list().unwrap().is_empty());
}

#[test]
fn real_fault_on_the_owned_connection_is_live_even_with_a_later_budget_stop() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("owned-connection-seed.osre");
    let parsed = parse_independent(&receipt.path);
    let before = catalog(&fixture.store.connection);
    let files = tree(fixture.store.root());
    let mut bytes = parsed.payload["catalog.sqlite3"].clone();
    bytes[100] = 0xff;
    // Test-only replacement of this isolated NativeStore's connection with an
    // actually corrupt SQLite image; the physical native catalog is untouched.
    let mut bad = Connection::open_in_memory().unwrap();
    bad.deserialize_read_exact(
        rusqlite::MAIN_DB,
        std::io::Cursor::new(&bytes),
        bytes.len(),
        true,
    )
    .unwrap();
    fixture.store.connection = bad;
    let governor = budget();
    let destination = fixture.archive("owned-connection-failed.osre");
    let error = fixture
        .store
        .export_retained_evidence_full_root_typed(&destination, EXPORT, &governor)
        .unwrap_err();
    assert_eq!(error.origin(), EvidenceFailureOrigin::LiveCatalog);
    assert_eq!(error.catalog_sqlite_error_code().unwrap() & 0xff, 11);
    // A later stop may be the displayed failure but cannot erase that typed
    // live error. This finalizer is the same one used by the public typed API.
    let cancelled =
        InspectionBudget::new(InspectionLimits::default(), Arc::new(AtomicBool::new(true)))
            .unwrap();
    let error = evidence_failure::finish::<()>(&cancelled, Err(error)).unwrap_err();
    assert!(matches!(
        error.budget_stop(),
        Some(RecordingError::InspectionCancelled)
    ));
    assert_eq!(error.catalog_sqlite_error_code().unwrap() & 0xff, 11);
    assert!(!destination.exists());
    fixture.store.connection =
        Connection::open(fixture.store.root().join("studio.sqlite3")).unwrap();
    assert_eq!(catalog(&fixture.store.connection), before);
    assert_eq!(tree(fixture.store.root()), files);
}

#[test]
fn source_write_transaction_backup_failure_has_mixed_origin_without_relabeling_the_catalog() {
    let mut fixture = Fixture::new();
    let before = catalog(&fixture.store.connection);
    let files = retained_tree(fixture.store.root());
    fixture
        .store
        .connection
        .execute_batch("BEGIN IMMEDIATE; UPDATE assets SET payload=payload;")
        .unwrap();
    let path = fixture.archive("mixed-backup-failure.osre");
    let error = fixture
        .store
        .export_retained_evidence_full_root_typed(&path, EXPORT, &budget())
        .unwrap_err();
    assert_eq!(error.origin(), EvidenceFailureOrigin::BackupTransfer);
    assert!(matches!(
        error.operation_error(),
        StoreError::Conflict | StoreError::Database(_)
    ));
    if let Some(code) = error.catalog_sqlite_error_code() {
        assert!(
            matches!(code & 0xff, 5 | 6),
            "Expected actual SQLite busy/locked: {error:?}"
        );
    }
    fixture.store.connection.execute_batch("ROLLBACK").unwrap();
    assert!(!path.exists());
    assert_live_unchanged(&fixture, &before, &files);
    assert!(
        fixture
            .store
            .export_retained_evidence_full_root_typed(&path, EXPORT, &budget())
            .is_ok()
    );
    assert_live_unchanged(&fixture, &before, &files);
}
