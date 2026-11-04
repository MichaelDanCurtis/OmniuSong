//! Existing-artifact-only export reconciliation, with independent raw oracles.
use super::*;

#[test]
fn exact_export_reconciles_with_original_root_absent_without_creating_artifacts() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("existing-export.osre");
    let origin = fixture.store.root().to_str().unwrap().to_owned();
    let parsed = parse_independent(&receipt.path);
    assert_eq!(receipt.sha256, sha(&fs::read(&receipt.path).unwrap()));
    // The original recorded root no longer exists; its bytes remain retained
    // under a different test-only path. Reconciliation must not consult it.
    fs::rename(fixture.store.root(), fixture.root.join("retained-away")).unwrap();
    assert!(!Path::new(&origin).exists());
    let before = tree(&fixture.root);
    let observed = reconcile_evidence_export(&receipt.path, EXPORT, &origin, &budget()).unwrap();
    assert_eq!(observed.receipt, receipt);
    assert_eq!(observed.manifest, parsed.manifest);
    assert_eq!(tree(&fixture.root), before);
    assert!(!Path::new(&origin).exists());
}

#[test]
fn missing_partial_or_wrong_identity_export_refuses_without_fresh_export_fallback() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("existing-identity.osre");
    let origin = fixture.store.root().to_str().unwrap().to_owned();
    let missing = fixture.archive("missing-destination.osre");
    let partial = fixture.archive("partial-destination.osre");
    let bytes = fs::read(&receipt.path).unwrap();
    fs::write(&partial, &bytes[..bytes.len() - 1]).unwrap();
    let before = tree(&fixture.root);
    for (path, id, root) in [
        (&missing, EXPORT, origin.as_str()),
        (&partial, EXPORT, origin.as_str()),
        (&receipt.path, ACTION, origin.as_str()),
        (&receipt.path, EXPORT, "/different-recorded-origin"),
    ] {
        assert!(reconcile_evidence_export(path, id, root, &budget()).is_err());
        assert_eq!(tree(&fixture.root), before);
    }
    assert!(!missing.exists());
    assert_eq!(fs::read(&partial).unwrap(), bytes[..bytes.len() - 1]);
    assert_eq!(
        reconcile_evidence_export(&receipt.path, EXPORT, &origin, &budget())
            .unwrap()
            .receipt,
        receipt
    );
}

#[test]
fn export_reconciliation_validates_recorded_fields_and_sticky_budget_first() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("bounded-recorded-fields.osre");
    let origin = fixture.store.root().to_str().unwrap().to_owned();
    let before = tree(&fixture.root);
    for id in [
        "",
        "00000000000000000000000000000000",
        "ABCDEF0123456789ABCDEF0123456789AB",
    ] {
        assert!(reconcile_evidence_export(&receipt.path, id, &origin, &budget()).is_err());
    }
    for root in [
        "relative",
        "/invalid\nroot",
        &format!("/{}", "x".repeat(4096)),
    ] {
        assert!(reconcile_evidence_export(&receipt.path, EXPORT, root, &budget()).is_err());
    }
    assert!(reconcile_evidence_export("relative.osre", EXPORT, &origin, &budget()).is_err());
    for path in [
        format!("/{}/archive.osre", "x".repeat(4096)),
        "/invalid\narchive.osre".into(),
    ] {
        assert!(matches!(
            reconcile_evidence_export(path, EXPORT, &origin, &budget()),
            Err(StoreError::Invalid(_))
        ));
    }
    let non_utf8 = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 255]));
    assert!(matches!(
        reconcile_evidence_export(non_utf8, EXPORT, &origin, &budget()),
        Err(StoreError::Invalid(_))
    ));
    let cancelled =
        InspectionBudget::new(InspectionLimits::default(), Arc::new(AtomicBool::new(true)))
            .unwrap();
    assert!(matches!(
        reconcile_evidence_export("relative", "bad-id", "bad-root", &cancelled),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    let exhausted = governor(InspectionLimits {
        max_work_bytes: 0,
        ..Default::default()
    });
    assert!(exhausted.charge_work(1).is_err());
    assert!(matches!(
        reconcile_evidence_export("relative", "bad-id", "bad-root", &exhausted),
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        }))
    ));
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn export_reconciliation_rejects_aliases_owned_names_and_live_catalog_without_touching_them() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("alias-source.osre");
    let origin = fixture.store.root().to_str().unwrap().to_owned();
    let symlink = fixture.archive("redirected.osre");
    std::os::unix::fs::symlink(&receipt.path, &symlink).unwrap();
    assert!(reconcile_evidence_export(&symlink, EXPORT, &origin, &budget()).is_err());
    fs::remove_file(&symlink).unwrap();
    let hardlink = fixture.archive("aliased.osre");
    fs::hard_link(&receipt.path, &hardlink).unwrap();
    assert!(reconcile_evidence_export(&hardlink, EXPORT, &origin, &budget()).is_err());
    fs::remove_file(&hardlink).unwrap();
    let before = catalog(&fixture.store.connection);
    let main = fs::read(fixture.store.root().join("studio.sqlite3")).unwrap();
    fixture
        .store
        .connection
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    let native = fixture.store.root().join("studio.sqlite3");
    ownerless_sqlite_probe(&native, true);
    assert!(reconcile_evidence_export(&native, EXPORT, &origin, &budget()).is_err());
    ownerless_sqlite_probe(&native, true);
    fixture.store.connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(fs::read(native).unwrap(), main);
    assert_eq!(catalog(&fixture.store.connection), before);
    let mut owner = receipt.path.as_os_str().to_os_string();
    owner.push(".owner.lock");
    let owner = PathBuf::from(owner);
    fs::write(&owner, b"retained owner bytes").unwrap();
    let files = tree(&fixture.root);
    assert!(reconcile_evidence_export(&receipt.path, EXPORT, &origin, &budget()).is_err());
    assert_eq!(tree(&fixture.root), files);
    assert_eq!(fs::read(owner).unwrap(), b"retained owner bytes");
}

#[test]
fn rehashed_corrupt_archived_catalog_reconciliation_is_local_and_live_queries_stay_healthy() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("healthy-for-reconcile.osre");
    let origin = fixture.store.root().to_str().unwrap().to_owned();
    let mut parsed = parse_independent(&receipt.path);
    let catalog_bytes = parsed.payload.get_mut("catalog.sqlite3").unwrap();
    catalog_bytes[100] = 0xff;
    parsed
        .manifest
        .entries
        .iter_mut()
        .find(|e| e.path == "catalog.sqlite3")
        .unwrap()
        .sha256 = Some(sha(catalog_bytes));
    let path = fixture.archive("real-corrupt-reconcile.osre");
    let bytes = bundle(&parsed.manifest, &parsed.payload, None);
    fs::write(&path, &bytes).unwrap();
    parse_independent(&path);
    let before = catalog(&fixture.store.connection);
    let files = tree(&fixture.root);
    let error = reconcile_evidence_export(&path, EXPORT, &origin, &budget()).unwrap_err();
    let StoreError::Database(sqlite) = error else {
        panic!("Expected real archived-catalog failure: {error:?}")
    };
    assert_eq!(sqlite.sqlite_error().unwrap().extended_code & 0xff, 11);
    assert_eq!(tree(&fixture.root), files);
    assert_eq!(catalog(&fixture.store.connection), before);
    assert!(!fixture.store.list().unwrap().is_empty());
    assert_eq!(
        reconcile_evidence_export(&receipt.path, EXPORT, &origin, &budget())
            .unwrap()
            .receipt,
        receipt
    );
}
