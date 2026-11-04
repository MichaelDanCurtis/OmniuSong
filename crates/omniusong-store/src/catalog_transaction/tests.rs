//! Real isolated SQLite operations exercise hook ownership and transaction
//! cleanup. No application catalog, source file or device is opened here.
use super::*;
use omniusong_recording::{InspectionBudget, InspectionLimits, InspectionResource, RecordingError};
use rusqlite::{Connection, hooks::Action};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn connection() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE retained(id INTEGER PRIMARY KEY,value TEXT NOT NULL); INSERT INTO retained VALUES(1,'before'); CREATE TABLE earlier(id INTEGER PRIMARY KEY);").unwrap();
    db
}

#[test]
fn legacy_archive_scope_commits_and_explicitly_rolls_back_without_a_governor() {
    let mut db = connection();
    CatalogTransaction::legacy_immediate(&mut db)
        .unwrap()
        .run(|tx| {
            tx.execute(
                "INSERT INTO retained VALUES(2,'committed legacy archive')",
                [],
            )?;
            tx.commit()
        })
        .unwrap();
    let error: crate::Result<()> =
        CatalogTransaction::legacy_immediate(&mut db)
            .unwrap()
            .run(|tx| {
                tx.execute("INSERT INTO retained VALUES(3,'must roll back')", [])?;
                Err(StoreError::Invalid("original legacy body refusal".into()))
            });
    assert!(
        matches!(error,Err(StoreError::Invalid(message)) if message=="original legacy body refusal")
    );
    CatalogTransaction::legacy_deferred(&db)
        .unwrap()
        .run(|tx| {
            assert_eq!(
                tx.query_row("SELECT count(*) FROM retained", [], |row| row
                    .get::<_, i64>(0))?,
                2
            );
            tx.rollback()
        })
        .unwrap();
    assert!(db.is_autocommit());
    assert_eq!(
        rows(&db),
        vec![(1, "before".into()), (2, "committed legacy archive".into())]
    );
}

#[test]
fn legacy_archive_denied_commit_retains_sql_and_rolls_back_real_mutation() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
    let mut db = connection();
    db.authorizer(Some(|context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::Transaction {
                // This pinned rusqlite maps COMMIT to Unknown; its public
                // TransactionOperation has Begin/Release/Rollback only.
                operation: TransactionOperation::Unknown
            }
        ) {
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }))
    .unwrap();
    let error = CatalogTransaction::legacy_immediate(&mut db)
        .unwrap()
        .run(|tx| {
            tx.execute("INSERT INTO retained VALUES(2,'refused commit')", [])?;
            tx.commit()
        })
        .unwrap_err();
    assert_eq!(
        error.catalog_sqlite_error_code(),
        Some(rusqlite::ffi::SQLITE_AUTH)
    );
    assert!(!error.catalog_connection_unusable());
    assert!(db.is_autocommit());
    db.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn nested_archive_private_failure_cannot_hide_real_live_rollback_failure() {
    use crate::{
        ArchiveAction, ArchiveContext, ArchiveFailure, ArchiveSqlOrigin, archive_boundary,
    };
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
    let mut db = connection();
    db.authorizer(Some(|context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::Transaction {
                operation: TransactionOperation::Rollback
            }
        ) {
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }))
    .unwrap();
    let error: crate::Result<()> =
        CatalogTransaction::legacy_immediate(&mut db)
            .unwrap()
            .run(|tx| {
                tx.execute(
                    "INSERT INTO retained VALUES(2,'actual pending mutation')",
                    [],
                )?;
                Err(archive_boundary::sql_error(
                    ArchiveSqlOrigin::Input,
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(26),
                        Some("explicit private error constructor".into()),
                    ),
                ))
            });
    let error = error.unwrap_err();
    let StoreError::Catalog(failure) = &error else {
        panic!("lost actual cleanup: {error:?}")
    };
    assert!(
        matches!(failure.operation_error(),StoreError::ArchiveSql(private) if private.origin()==ArchiveSqlOrigin::Input)
    );
    assert_eq!(failure.cleanup_errors().len(), 1);
    assert!(!db.is_autocommit());
    let error = StoreError::Archive(Box::new(ArchiveFailure::new(
        ArchiveContext::new(ArchiveAction::Restore, std::path::Path::new("isolated")),
        error,
    )));
    let error = archive_boundary::input::<()>(Err(error)).unwrap_err();
    assert_eq!(
        error.catalog_sqlite_error_code(),
        Some(rusqlite::ffi::SQLITE_AUTH)
    );
    assert!(error.catalog_connection_unusable());
    db.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}
fn governor(steps: u64) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(
            InspectionLimits {
                max_vm_steps: steps,
                ..Default::default()
            },
            cancelled.clone(),
        )
        .unwrap(),
        cancelled,
    )
}
fn rows(db: &Connection) -> Vec<(i64, String)> {
    db.prepare("SELECT id,value FROM retained ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn vm_limit<T: std::fmt::Debug>(result: crate::Result<T>) {
    assert!(
        matches!(
            &result,
            Err(StoreError::Recording(RecordingError::InspectionLimit {
                resource: InspectionResource::VmSteps,
                ..
            }))
        ),
        "Expected a real scope-requested SQLite interruption to retain its typed VM stop: {result:?}"
    );
}
fn assert_detached(db: &Connection, budget: &InspectionBudget) {
    assert!(db.is_autocommit(), "Cleanup left an active transaction");
    let before = budget.usage();
    assert_eq!(
        db.query_row("SELECT 40+2", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        42
    );
    assert_eq!(
        budget.usage(),
        before,
        "SQL after scope completion charged a detached governor"
    );
}

#[test]
fn explicitly_tagged_live_interrupt_maps_only_this_scopes_governor_stop() {
    let mut db = connection();
    let (budget, _) = governor(128);
    let result = CatalogTransaction::immediate(&mut db, &budget).unwrap().run(|tx| {
        tx.execute("INSERT INTO retained VALUES(2,'rollback live tagged')", [])?;
        crate::archive_boundary::live(tx.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) SELECT sum(x) FROM n", [], |row| row.get::<_, i64>(0)).map_err(StoreError::Database))?;
        tx.commit()
    });
    vm_limit(result);
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn private_interrupt_is_not_relabelled_after_actual_live_hook_stop() {
    let mut db = connection();
    let (budget, _) = governor(128);
    let result: crate::Result<()> = CatalogTransaction::immediate(&mut db, &budget).unwrap().run(|tx| {
        let actual = tx.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) SELECT sum(x) FROM n", [], |row| row.get::<_, i64>(0)).unwrap_err();
        assert_eq!(actual.sqlite_error().unwrap().extended_code, 9);
        // Explicit origin injection: this is not the failing live query.
        Err(crate::archive_boundary::sql_error(crate::ArchiveSqlOrigin::Input, rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(9),Some("private origin control".into()))))
    });
    let error = result.unwrap_err();
    assert!(
        matches!(&error, StoreError::ArchiveSql(value) if value.origin()==crate::ArchiveSqlOrigin::Input)
    );
    assert_eq!(error.catalog_sqlite_error_code(), None);
    assert_detached(&db, &budget);
}

#[test]
fn own_interrupt_mapping_cannot_erase_nested_connection_unusable_failure() {
    let mut db = connection();
    let (budget, _) = governor(128);
    let result: crate::Result<()> = CatalogTransaction::immediate(&mut db, &budget).unwrap().run(|tx| {
        let actual = tx.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) SELECT sum(x) FROM n", [], |row| row.get::<_, i64>(0)).unwrap_err();
        // Construct a nested prior cleanup failure; outer scope cleanup must
        // preserve its unusable fact even though its own connection recovers.
        Err(StoreError::Catalog(Box::new(CatalogFailure {
            operation: Box::new(crate::archive_boundary::sql_error(crate::ArchiveSqlOrigin::LiveCatalog, actual)),
            cleanup: vec![], transaction_active: true,
        })))
    });
    let error = result.unwrap_err();
    assert!(error.catalog_connection_unusable());
    assert_eq!(error.catalog_sqlite_error_code(), Some(9));
    assert_detached(&db, &budget);
}

#[test]
fn preparation_failure_explicitly_rolls_back_and_detaches_before_return() {
    let mut db = connection();
    let (budget, _) = governor(1024);
    {
        let mut scope = CatalogTransaction::immediate(&mut db, &budget).unwrap();
        let error = scope
            .prepare::<()>(|tx| {
                tx.execute("INSERT INTO retained VALUES(2,'preparation rollback')", [])?;
                tx.execute("INSERT INTO absent_table VALUES(1)", [])?;
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.catalog_sqlite_error_code(), Some(1));
        assert!(scope.transaction.is_none());
        assert!(scope.is_autocommit());
    }
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn real_recursive_read_quota_rolls_back_prior_writes_and_detaches_for_connection_reuse() {
    let mut db = connection();
    let before = rows(&db);
    let (budget, _) = governor(128);
    let result = CatalogTransaction::immediate(&mut db,&budget).unwrap().run(|tx| {
        tx.execute("INSERT INTO retained VALUES(2,'uncommitted')",[])?;
        let _:i64 = tx.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) SELECT sum(x) FROM n",[],|row|row.get(0))?;
        tx.commit()?; Ok(())
    });
    vm_limit(result);
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), before);
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::VmSteps,
            ..
        })
    ));
    let stopped = budget.usage();
    let (fresh, _) = governor(1024);
    CatalogTransaction::immediate(&mut db, &fresh)
        .unwrap()
        .run(|tx| {
            tx.execute("INSERT INTO retained VALUES(2,'fresh scope')", [])?;
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    assert_eq!(budget.usage(), stopped);
    assert!(fresh.usage().vm_steps > 0);
    assert_eq!(
        rows(&db),
        vec![(1, "before".into()), (2, "fresh scope".into())]
    );
}

#[test]
fn mutating_recursive_trigger_quota_observes_sqlites_automatic_transaction_rollback() {
    let mut db = connection();
    db.execute_batch("PRAGMA recursive_triggers=ON; CREATE TABLE cascade(id INTEGER PRIMARY KEY); CREATE TRIGGER cascade_insert AFTER INSERT ON cascade WHEN NEW.id<10000 BEGIN INSERT INTO cascade VALUES(NEW.id+1); END;").unwrap();
    let (budget, _) = governor(256);
    let automatic = Arc::new(AtomicBool::new(false));
    let observed = automatic.clone();
    let result = CatalogTransaction::immediate(&mut db, &budget)
        .unwrap()
        .run(|tx| {
            tx.execute("INSERT INTO earlier VALUES(1)", [])?;
            let inserted = tx.execute("INSERT INTO cascade VALUES(1)", []);
            observed.store(tx.is_autocommit(), Ordering::Release);
            inserted?;
            tx.commit()?;
            Ok(())
        });
    vm_limit(result);
    assert!(
        automatic.load(Ordering::Acquire),
        "Interrupted mutating DML should automatically roll back the whole transaction"
    );
    assert_detached(&db, &budget);
    for table in ["earlier", "cascade"] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn deterministic_update_hook_cancellation_keeps_automatic_rollback_and_reuse_clean() {
    let mut db = connection();
    let (budget, cancelled) = governor(100_000);
    let updates = Arc::new(AtomicUsize::new(0));
    let count = updates.clone();
    let token = cancelled.clone();
    db.update_hook(Some(
        move |action: Action, _database: &str, table: &str, _row: i64| {
            if action == Action::SQLITE_INSERT && table == "retained" {
                count.fetch_add(1, Ordering::AcqRel);
                token.store(true, Ordering::Release);
            }
        },
    ))
    .unwrap();
    let automatic = Arc::new(AtomicBool::new(false));
    let observed = automatic.clone();
    let result = CatalogTransaction::immediate(&mut db,&budget).unwrap().run(|tx| {
        tx.execute("INSERT INTO earlier VALUES(1)",[])?;
        let inserted = tx.execute("WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO retained SELECT x,'cancelled mutation' FROM n",[]);
        observed.store(tx.is_autocommit(),Ordering::Release);
        inserted?; tx.commit()?; Ok(())
    });
    assert!(matches!(
        result,
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    assert_eq!(
        updates.load(Ordering::Acquire),
        1,
        "Cancellation must be driven by the first actual row update"
    );
    assert!(automatic.load(Ordering::Acquire));
    db.update_hook(None::<fn(Action, &str, &str, i64)>).unwrap();
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
    assert_eq!(
        db.query_row("SELECT count(*) FROM earlier", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn explicit_body_error_rolls_back_without_erasing_the_original_operation_error() {
    let mut db = connection();
    let (budget, _) = governor(1024);
    let result: crate::Result<()> = CatalogTransaction::immediate(&mut db, &budget)
        .unwrap()
        .run(|tx| {
            tx.execute("INSERT INTO retained VALUES(2,'not committed')", [])?;
            Err(StoreError::Invalid("explicit body refusal".into()))
        });
    assert!(
        matches!(&result,Err(StoreError::Invalid(message)) if message=="explicit body refusal")
    );
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn panic_unwinding_uses_drop_only_as_a_detach_and_rollback_backstop() {
    let mut db = connection();
    let (budget, _) = governor(1024);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: crate::Result<()> = CatalogTransaction::immediate(&mut db, &budget)
            .unwrap()
            .run(|tx| {
                tx.execute("INSERT INTO retained VALUES(2,'panic rollback')", [])?;
                panic!("intentional catalog transaction test panic");
            });
    }));
    assert!(panic.is_err());
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn unfinalized_drop_and_explicit_rollback_leave_no_live_hooks_or_pending_writes() {
    let mut db = connection();
    let (budget, cancelled) = governor(1024);
    {
        let tx = CatalogTransaction::immediate(&mut db, &budget).unwrap();
        tx.execute("INSERT INTO retained VALUES(2,'drop rollback')", [])
            .unwrap();
    }
    cancelled.store(true, Ordering::Release);
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
    let (fresh, _) = governor(1024);
    let answer = CatalogTransaction::immediate(&mut db, &fresh)
        .unwrap()
        .run(|tx| {
            tx.execute("INSERT INTO retained VALUES(2,'explicit rollback')", [])?;
            tx.rollback()?;
            Ok(17)
        })
        .unwrap();
    assert_eq!(answer, 17);
    assert_detached(&db, &fresh);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn many_short_statements_share_cumulative_vm_credit_instead_of_resetting_per_statement() {
    let db = connection();
    let (budget, _) = governor(128);
    let mut completed = 0;
    let result = CatalogTransaction::deferred(&db, &budget)
        .unwrap()
        .run(|tx| {
            for _ in 0..500 {
                assert_eq!(tx.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?, 1);
                completed += 1;
            }
            tx.rollback()?;
            Ok(())
        });
    vm_limit(result);
    assert!(completed > 0 && completed < 500);
    assert_eq!(budget.usage().vm_steps, 128);
    assert_detached(&db, &budget);
}

#[test]
fn late_commit_hook_cancellation_does_not_erase_a_successful_commit() {
    let mut db = connection();
    let (budget, cancelled) = governor(1024);
    let invoked = Arc::new(AtomicUsize::new(0));
    let calls = invoked.clone();
    let token = cancelled.clone();
    db.commit_hook(Some(move || {
        calls.fetch_add(1, Ordering::AcqRel);
        token.store(true, Ordering::Release);
        false
    }))
    .unwrap();
    let receipt = CatalogTransaction::immediate(&mut db, &budget)
        .unwrap()
        .run(|tx| {
            tx.execute("INSERT INTO retained VALUES(2,'durable result')", [])?;
            tx.commit()?;
            Ok("committed receipt")
        })
        .unwrap();
    assert_eq!(receipt, "committed receipt");
    assert_eq!(invoked.load(Ordering::Acquire), 1);
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    db.commit_hook(None::<fn() -> bool>).unwrap();
    assert_detached(&db, &budget);
    assert_eq!(
        rows(&db),
        vec![(1, "before".into()), (2, "durable result".into())]
    );
}

#[test]
fn rejected_commit_remains_the_actual_database_error_even_when_commit_hook_also_cancels() {
    let mut db = connection();
    let (budget, cancelled) = governor(1024);
    let token = cancelled.clone();
    db.commit_hook(Some(move || {
        token.store(true, Ordering::Release);
        true
    }))
    .unwrap();
    let result = CatalogTransaction::immediate(&mut db, &budget)
        .unwrap()
        .run(|tx| {
            tx.execute("INSERT INTO retained VALUES(2,'commit refused')", [])?;
            tx.commit()?;
            Ok(())
        });
    assert!(
        matches!(&result,Err(StoreError::Database(rusqlite::Error::SqliteFailure(error,_))) if error.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_COMMITHOOK),
        "A real rejected COMMIT must not become a cancellation or a fictitious cleanup failure: {result:?}"
    );
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    db.commit_hook(None::<fn() -> bool>).unwrap();
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn nested_scope_refuses_before_replacing_the_outer_governor_or_charging_the_inner_one() {
    let mut db = connection();
    let (outer, _) = governor(128);
    let (inner, _) = governor(0);
    let result = CatalogTransaction::immediate(&mut db,&outer).unwrap().run(|tx| {
        match CatalogTransaction::deferred(tx,&inner) {
            Err(StoreError::Invalid(message)) => assert_eq!(message,"Catalog scope requires an idle connection"),
            Err(error) => panic!("Unexpected nested-scope refusal: {error}"),
            Ok(_) => panic!("A nested scope replaced its parent's hook owner"),
        }
        assert_eq!(inner.usage().vm_steps,0);
        let _:i64 = tx.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n",[],|row|row.get(0))?;
        tx.commit()?; Ok(())
    });
    vm_limit(result);
    assert_eq!(inner.usage().vm_steps, 0);
    assert_eq!(outer.usage().vm_steps, 128);
    assert_detached(&db, &outer);
}

#[test]
fn explicit_governor_switch_replaces_only_the_hook_and_preserves_both_cumulative_accounts() {
    let mut db = connection();
    let (first, cancelled) = governor(1024);
    let (second, _) = governor(1024);
    let mut first_at_switch = None;
    CatalogTransaction::immediate(&mut db, &first)
        .unwrap()
        .run(|tx| {
            assert_eq!(
                tx.query_row("SELECT 11", [], |row| row.get::<_, i64>(0))?,
                11
            );
            first_at_switch = Some(first.usage());
            tx.switch_governor(&second)?;
            cancelled.store(true, Ordering::Release);
            tx.execute("INSERT INTO retained VALUES(2,'second governor')", [])?;
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    assert_eq!(first.usage(), first_at_switch.unwrap());
    assert!(second.usage().vm_steps > 0);
    assert!(matches!(
        first.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_detached(&db, &first);
    assert_detached(&db, &second);
    assert_eq!(
        rows(&db),
        vec![(1, "before".into()), (2, "second governor".into())]
    );
}

#[test]
fn explicit_legacy_release_does_not_leave_the_older_governor_on_ordinary_sql() {
    let mut db = connection();
    let (budget, cancelled) = governor(128);
    let mut before_release = None;
    CatalogTransaction::immediate(&mut db,&budget).unwrap().run(|tx| {
        assert_eq!(tx.query_row("SELECT 7",[],|row|row.get::<_,i64>(0))?,7);
        before_release=Some(budget.usage());
        tx.release_governor_for_legacy()?;
        cancelled.store(true,Ordering::Release);
        // This deliberately makes no VM-bound claim for the legacy branch.
        let sum:i64 = tx.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n",[],|row|row.get(0))?;
        assert_eq!(sum,50_005_000);
        tx.execute("INSERT INTO retained VALUES(2,'ordinary legacy branch')",[])?;
        tx.commit()?; Ok(())
    }).unwrap();
    assert_eq!(budget.usage(), before_release.unwrap());
    assert_detached(&db, &budget);
    assert_eq!(
        rows(&db),
        vec![(1, "before".into()), (2, "ordinary legacy branch".into())]
    );
}

#[test]
fn a_body_that_forgets_to_finalize_is_refused_and_rolled_back_instead_of_returning_success() {
    let mut db = connection();
    let (budget, _) = governor(1024);
    let result = CatalogTransaction::immediate(&mut db, &budget)
        .unwrap()
        .run(|tx| {
            tx.execute("INSERT INTO retained VALUES(2,'unfinished')", [])?;
            Ok("not a committed receipt")
        });
    assert!(
        matches!(&result,Err(StoreError::Invalid(message)) if message=="Catalog operation did not finish its transaction")
    );
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn denied_rollback_preserves_primary_and_real_cleanup_error_and_marks_the_connection_unusable() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
    let mut db = connection();
    let (budget, cancelled) = governor(1024);
    db.authorizer(Some(|context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::Transaction {
                operation: TransactionOperation::Rollback
            }
        ) {
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }))
    .unwrap();
    let result: crate::Result<()> = CatalogTransaction::immediate(&mut db, &budget)
        .unwrap()
        .run(|tx| {
            tx.execute(
                "INSERT INTO retained VALUES(2,'cleanup must be explicit')",
                [],
            )?;
            Err(StoreError::Invalid("original body failure retained".into()))
        });
    let error = result.unwrap_err();
    let StoreError::Catalog(failure) = &error else {
        panic!("Rollback failure was discarded: {error:?}")
    };
    assert!(
        matches!(failure.operation_error(),StoreError::Invalid(message) if message=="original body failure retained")
    );
    assert_eq!(failure.cleanup_errors().len(), 1);
    assert!(
        matches!(&failure.cleanup_errors()[0],rusqlite::Error::SqliteFailure(code,_) if code.extended_code & 0xff==rusqlite::ffi::SQLITE_AUTH)
    );
    assert!(failure.connection_unusable());
    assert!(error.catalog_connection_unusable());
    assert_eq!(
        failure.catalog_sqlite_error_code().unwrap() & 0xff,
        rusqlite::ffi::SQLITE_AUTH
    );
    assert!(
        !db.is_autocommit(),
        "A denied ROLLBACK must not be reported as successful cleanup"
    );
    // Only this test fixture is manually recovered. A native caller must retire
    // the unusable connection, preserving both reported failures.
    db.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    cancelled.store(true, Ordering::Release);
    let stopped = budget.usage();
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(budget.usage(), stopped);
    assert_detached(&db, &budget);
    assert_eq!(rows(&db), vec![(1, "before".into())]);
}

#[test]
fn injected_database_primary_codes_survive_late_cancellation_without_faking_physical_io_failure() {
    // Explicit error-constructor coverage, not an I/O/corruption simulation.
    // The INSERT, rollback and subsequent connection reuse are real SQLite.
    for code in [9, 10, 11, 26] {
        let mut db = connection();
        let (budget, cancelled) = governor(1024);
        let result: crate::Result<()> = CatalogTransaction::immediate(&mut db, &budget)
            .unwrap()
            .run(|tx| {
                tx.execute("INSERT INTO retained VALUES(2,'rolled back')", [])?;
                cancelled.store(true, Ordering::Release);
                Err(StoreError::Database(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(code),
                    Some("injected primary-code oracle".into()),
                )))
            });
        let error = result.unwrap_err();
        assert!(
            matches!(&error, StoreError::Database(rusqlite::Error::SqliteFailure(primary, _)) if primary.extended_code == code)
        );
        assert_eq!(error.catalog_sqlite_error_code(), Some(code));
        assert_eq!(error.catalog_connection_unusable(), code != 9);
        assert_detached(&db, &budget);
        assert_eq!(rows(&db), vec![(1, "before".into())]);
    }
}
