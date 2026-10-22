//! Actual publisher/catalog interruption, separate from the transaction unit
//! tests and from source-reader SQL. All stores and originals are isolated
//! synthetic fixtures owned by the parent harness.
use super::*;

#[test]
fn final_catalog_vm_interrupt_rolls_back_publication_and_releases_the_old_governor() {
    const VM_LIMIT: u64 = 500_000;
    let limits = InspectionLimits {
        max_vm_steps: VM_LIMIT,
        ..InspectionLimits::default()
    };
    assert!(VM_LIMIT < InspectionLimits::default().max_vm_steps);

    // A separate, healthy original/store demonstrates that this same finite
    // ceiling admits the complete public operation, with ample SQL headroom.
    let healthy = Harness::new();
    let mut control = healthy.prepare();
    let control_staged = healthy.original.bytes();
    let control_owned = fs::read(&control.owned).unwrap();
    let (control_budget, _) = budget(limits);
    let control_receipt = control
        .store
        .publish_performance_projection(&control.intent, &control_budget)
        .unwrap();
    control_receipt.validate().unwrap();
    assert!(!control_receipt.replayed);
    assert_eq!(counts(&healthy.db()), (1, 1, 1));
    assert!(control_budget.usage().vm_steps > 0);
    assert!(
        control_budget.usage().vm_steps < VM_LIMIT / 4,
        "The healthy control must fit well below the attacked SQL ceiling: {:?}",
        control_budget.usage()
    );
    control_budget.checkpoint().unwrap();
    assert_source_bytes(&healthy, &control_staged, &control.owned, &control_owned);

    let attacked = Harness::new();
    let mut p = attacked.prepare();
    let db = attacked.db();
    // This trigger runs on the main catalog after the publisher has written
    // the candidate, its history edge and its operation row. Its first query
    // also asserts that those earlier writes are visible inside the scope.
    // A scalar subquery permits the recursive CTE inside SQLite trigger SQL.
    db.execute_batch(
        "CREATE TRIGGER exhaust_projection_catalog_vm AFTER INSERT ON operations
         WHEN NEW.operation_id LIKE 'os-perf-proj:%'
         BEGIN
           SELECT CASE WHEN
             (SELECT revision FROM projects WHERE id=NEW.project_id)=NEW.committed_revision
             AND (SELECT cursor FROM projects WHERE id=NEW.project_id)=1
             AND (SELECT count(*) FROM edits WHERE project_id=NEW.project_id)=1
             THEN 1 ELSE RAISE(ABORT,'projection writes were not visible') END;
           SELECT (WITH RECURSIVE work(n) AS (
             VALUES(1) UNION ALL SELECT n+1 FROM work WHERE n<10000000
           ) SELECT sum(n) FROM work);
         END;",
    )
    .unwrap();
    let before = catalog(&db);
    let staged = attacked.original.bytes();
    let owned = fs::read(&p.owned).unwrap();
    let (stopped, _) = budget(limits);
    let result = p.store.publish_performance_projection(&p.intent, &stopped);
    assert!(
        matches!(
            &result,
            Err(StoreError::Recording(RecordingError::InspectionLimit {
                resource: InspectionResource::VmSteps,
                limit: VM_LIMIT,
                attempted,
            })) if *attempted == VM_LIMIT + 1
        ),
        "The catalog trigger must surface the scope's typed VM stop: {result:?}"
    );
    assert_eq!(stopped.usage().vm_steps, VM_LIMIT);
    assert!(matches!(
        stopped.checkpoint(),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::VmSteps,
            limit: VM_LIMIT,
            attempted,
        }) if attempted == VM_LIMIT + 1
    ));
    let stopped_usage = stopped.usage();
    assert_eq!(
        catalog(&db),
        before,
        "The complete catalog, including all history and receipt cells, must roll back"
    );
    assert_eq!(counts(&db), (1, 0, 0));
    assert!(
        p.store
            .projection_publication_receipt(&p.intent)
            .unwrap()
            .is_none()
    );
    assert_eq!(p.store.load(p.project.id()).unwrap(), p.project);
    assert_source_bytes(&attacked, &staged, &p.owned, &owned);
    assert_eq!(stopped.usage(), stopped_usage);

    db.execute_batch("DROP TRIGGER exhaust_projection_catalog_vm")
        .unwrap();
    let (fresh, _) = budget(limits);
    let committed = p
        .store
        .publish_performance_projection(&p.intent, &fresh)
        .unwrap();
    committed.validate().unwrap();
    assert!(!committed.replayed);
    assert_eq!(
        (
            committed.previous_revision,
            committed.committed_revision,
            committed.lane_count,
            committed.note_count,
        ),
        (0, 1, 4, 4)
    );
    assert_eq!(counts(&db), (1, 1, 1));
    assert!(fresh.usage().vm_steps > 0 && fresh.usage().vm_steps < VM_LIMIT / 4);
    fresh.checkpoint().unwrap();
    assert_source_bytes(&attacked, &staged, &p.owned, &owned);
    assert_eq!(stopped.usage(), stopped_usage);

    // The compact commitment remains a fact when both input filenames have
    // gone away. Replay must use its independent bounded metadata scope,
    // without observing or charging an already-cancelled caller governor.
    let staged_original = fs::read(&attacked.original.path).unwrap();
    let parked_owned = attacked.root.join("catalog-replay-owned.sqlite3");
    let parked_staged = attacked.root.join("catalog-replay-staged.sqlite3");
    fs::rename(&p.owned, &parked_owned).unwrap();
    fs::rename(&attacked.original.path, &parked_staged).unwrap();
    let replay_before = catalog(&db);
    let (cancelled, token) = budget(limits);
    token.store(true, Ordering::Release);
    let caller_usage = cancelled.usage();
    let mut expected = committed;
    expected.replayed = true;
    assert_eq!(
        p.store
            .publish_performance_projection(&p.intent, &cancelled)
            .unwrap(),
        expected
    );
    assert_eq!(cancelled.usage(), caller_usage);
    assert!(matches!(
        cancelled.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_eq!(catalog(&db), replay_before);
    assert!(!p.owned.exists() && !attacked.original.path.exists());
    assert_eq!(fs::read(&parked_owned).unwrap(), owned);
    assert_eq!(fs::read(&parked_staged).unwrap(), staged_original);
    assert_eq!(stopped.usage(), stopped_usage);
}
