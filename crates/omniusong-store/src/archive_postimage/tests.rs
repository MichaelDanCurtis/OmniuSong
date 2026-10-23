//! Isolated SQL/storage tests. No musical or original-source authority claimed.
use super::*;
use rusqlite::types::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/archive-postimage-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn ledger(&self, layout: Layout) -> PostimageLedger<'static> {
        PostimageLedger::new(&self.0, layout, None).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Postimage fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn empty(layout: Layout) -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    for table in layout.tables() {
        let mut columns = table
            .columns
            .iter()
            .map(|column| format!("\"{}\" {}", column.name, column.kind))
            .collect::<Vec<_>>();
        columns.push(format!(
            "PRIMARY KEY({})",
            table
                .pk
                .iter()
                .map(|index| format!("\"{}\"", table.columns[*index].name))
                .collect::<Vec<_>>()
                .join(",")
        ));
        connection
            .execute_batch(&format!(
                "CREATE TABLE \"{}\"({})",
                table.name,
                columns.join(",")
            ))
            .unwrap();
    }
    connection
}
fn refs(values: &[Value]) -> Vec<ValueRef<'_>> {
    values.iter().map(ValueRef::from).collect()
}
fn asset(id: &str, payload: Value) -> Vec<Value> {
    vec![Value::Text(id.into()), payload]
}
fn put_asset(connection: &Connection, id: &str, payload: &Value) {
    connection
        .execute(
            "INSERT INTO assets(id,payload) VALUES(?1,?2)",
            params![id, payload],
        )
        .unwrap();
}
fn invalid_result<T>(result: Result<T>) {
    assert!(matches!(result, Err(StoreError::Invalid(_))));
}
fn governor(limits: omniusong_recording::InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let token = Arc::new(AtomicBool::new(false));
    (InspectionBudget::new(limits, token.clone()).unwrap(), token)
}

#[test]
fn late_stop_cannot_erase_actual_private_sqlite_full_or_semantic_failure() {
    use omniusong_recording::{InspectionLimits, RecordingError};
    let database = Connection::open_in_memory().unwrap();
    database
        .execute_batch("CREATE TABLE full_test(payload BLOB); PRAGMA max_page_count=4;")
        .unwrap();
    let full = database
        .execute("INSERT INTO full_test VALUES(zeroblob(1048576))", [])
        .unwrap_err();
    assert_eq!(
        full.sqlite_error().unwrap().extended_code,
        rusqlite::ffi::SQLITE_FULL
    );
    let (budget, token) = governor(InspectionLimits::default());
    token.store(true, Ordering::Release);
    let result: Result<()> = finish(
        Some(&budget),
        Err(boundary::sql_error(ArchiveSqlOrigin::PrivateOutput, full)),
    );
    let error = result.unwrap_err();
    assert!(
        matches!(&error, StoreError::ArchiveSql(value) if value.origin()==ArchiveSqlOrigin::PrivateOutput && matches!(value.operation_error(), StoreError::Database(sql) if sql.sqlite_error().unwrap().extended_code==rusqlite::ffi::SQLITE_FULL))
    );
    assert_eq!(error.catalog_sqlite_error_code(), None);
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    let semantic: Result<()> = finish(Some(&budget), Err(invalid("postimage mismatch")));
    assert!(matches!(semantic, Err(StoreError::Invalid(value)) if value=="postimage mismatch"));
    assert!(matches!(
        finish(Some(&budget), Ok(())),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
}

#[test]
fn multichunk_postimage_refuses_a_lower_cumulative_work_budget() {
    use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};
    let source = empty(Layout::Catalog);
    put_asset(&source, "bounded", &Value::Blob(vec![7; CHUNK * 3]));
    let (budget, _) = governor(InspectionLimits {
        max_work_bytes: 1_100_000,
        ..InspectionLimits::default()
    });
    let result = proof_row(&source, ASSET, 1, ArchiveSqlOrigin::Input, Some(&budget));
    assert!(matches!(
        result,
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        }))
    ));
    assert!(proof_row(&source, ASSET, 1, ArchiveSqlOrigin::Input, None).is_ok());
}

#[test]
fn typed_cell_hash_has_an_independent_framed_oracle_and_keeps_raw_real_bits() {
    let values = [
        ValueRef::Null,
        ValueRef::Integer(i64::MIN),
        ValueRef::Integer(i64::MAX),
        ValueRef::Real(f64::MAX),
        ValueRef::Real(f64::from_bits(1)),
        ValueRef::Real(-0.0),
        ValueRef::Text(b"a\0z"),
        ValueRef::Blob(b"\0\xff\0"),
    ];
    for value in values {
        let (_, actual) =
            proof_values(ASSET, &[ValueRef::Text(b"key\0tail"), value], None).unwrap();
        let mut bytes = b"OmniuSong.archive-postimage-row.v1\0".to_vec();
        bytes.extend_from_slice(&6u64.to_le_bytes());
        bytes.extend_from_slice(b"assets");
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.push(b't');
        bytes.extend_from_slice(&8u64.to_le_bytes());
        bytes.extend_from_slice(b"key\0tail");
        let (tag, raw) = match value {
            ValueRef::Null => (b'n', vec![]),
            ValueRef::Integer(value) => (b'i', value.to_le_bytes().to_vec()),
            ValueRef::Real(value) => (b'r', value.to_bits().to_le_bytes().to_vec()),
            ValueRef::Text(value) => (b't', value.to_vec()),
            ValueRef::Blob(value) => (b'b', value.to_vec()),
        };
        bytes.push(tag);
        bytes.extend_from_slice(&(raw.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&raw);
        assert_eq!(actual, <[u8; 32]>::from(Sha256::digest(bytes)));
    }
    assert_ne!(
        proof_values(ASSET, &[ValueRef::Text(b"x"), ValueRef::Real(0.0)], None)
            .unwrap()
            .1,
        proof_values(ASSET, &[ValueRef::Text(b"x"), ValueRef::Real(-0.0)], None)
            .unwrap()
            .1
    );
}

#[test]
fn text_nul_blob_integer_null_and_extreme_real_rows_match_frozen_expectations() {
    let fixture = Fixture::new();
    let connection = empty(Layout::Catalog);
    let ledger = fixture.ledger(Layout::Catalog);
    ledger
        .capture_schema(&connection, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    for (index, value) in [
        Value::Null,
        Value::Integer(i64::MIN),
        Value::Integer(i64::MAX),
        Value::Real(f64::MAX),
        Value::Real(f64::from_bits(1)),
        Value::Text("text\0after".into()),
        Value::Blob(vec![0, 255, 0]),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("key\0{index}");
        let values = asset(&id, value.clone());
        ledger
            .expect_insert("assets", &refs(&values), false)
            .unwrap();
        put_asset(&connection, &id, &value);
    }
    ledger
        .freeze()
        .unwrap()
        .verify(&connection, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
}

#[test]
fn missing_extra_valid_substitution_and_unrelated_row_swap_refuse() {
    for attack in 0..4 {
        let fixture = Fixture::new();
        let connection = empty(Layout::Catalog);
        put_asset(
            &connection,
            "unrelated-a",
            &Value::Blob(b"original-a".to_vec()),
        );
        put_asset(
            &connection,
            "unrelated-b",
            &Value::Blob(b"original-b".to_vec()),
        );
        let ledger = fixture.ledger(Layout::Catalog);
        ledger
            .capture_baseline(&connection, ArchiveSqlOrigin::LiveCatalog)
            .unwrap();
        let frozen = ledger.freeze().unwrap();
        match attack {
            0 => {
                connection
                    .execute("DELETE FROM assets WHERE id='unrelated-a'", [])
                    .unwrap();
            }
            1 => put_asset(&connection, "extra", &Value::Blob(b"extra".to_vec())),
            2 => {
                connection
                    .execute(
                        "UPDATE assets SET payload=x'71717171717171717171' WHERE id='unrelated-a'",
                        [],
                    )
                    .unwrap();
            }
            _ => {
                connection.execute("UPDATE assets SET payload=CASE id WHEN 'unrelated-a' THEN x'6f726967696e616c2d62' ELSE x'6f726967696e616c2d61' END",[]).unwrap();
            }
        }
        invalid_result(frozen.verify(&connection, ArchiveSqlOrigin::LiveCatalog));
    }
}

#[test]
fn exact_shared_baseline_reuse_is_explicit_and_duplicate_plans_poison_freeze() {
    for policy in 0..4 {
        let fixture = Fixture::new();
        let connection = empty(Layout::Catalog);
        let original = asset("shared", Value::Blob(b"same".to_vec()));
        put_asset(&connection, "shared", &original[1]);
        let ledger = fixture.ledger(Layout::Catalog);
        ledger
            .capture_baseline(&connection, ArchiveSqlOrigin::LiveCatalog)
            .unwrap();
        if policy == 0 {
            invalid_result(ledger.expect_insert("assets", &refs(&original), false));
            invalid_result(ledger.freeze());
        } else if policy == 1 {
            ledger
                .expect_insert("assets", &refs(&original), true)
                .unwrap();
            ledger
                .freeze()
                .unwrap()
                .verify(&connection, ArchiveSqlOrigin::LiveCatalog)
                .unwrap();
        } else if policy == 2 {
            let changed = asset("shared", Value::Blob(b"changed".to_vec()));
            invalid_result(ledger.expect_insert("assets", &refs(&changed), true));
            invalid_result(ledger.freeze());
        } else {
            ledger
                .expect_insert("assets", &refs(&original), true)
                .unwrap();
            invalid_result(ledger.expect_insert("assets", &refs(&original), true));
            invalid_result(ledger.freeze());
        }
    }
}

#[test]
fn independent_catalog_plan_is_streamed_before_output_rows_and_shared_policy_is_narrow() {
    let fixture = Fixture::new();
    let destination = empty(Layout::Catalog);
    let plan = empty(Layout::Catalog);
    put_asset(&destination, "shared", &Value::Blob(b"same".to_vec()));
    put_asset(&plan, "shared", &Value::Blob(b"same".to_vec()));
    put_asset(&plan, "new", &Value::Blob(vec![7; CHUNK * 2 + 17]));
    plan.execute(
        "INSERT INTO projects VALUES('new-project','bounded raw document',0,0)",
        [],
    )
    .unwrap();
    let ledger = fixture.ledger(Layout::Catalog);
    ledger
        .capture_baseline(&destination, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    ledger
        .expect_from_plan(&plan, ArchiveSqlOrigin::Input, &["assets"])
        .unwrap();
    let frozen = ledger.freeze().unwrap();
    destination
        .execute(
            "INSERT INTO assets SELECT 'new',?1",
            [vec![7u8; CHUNK * 2 + 17]],
        )
        .unwrap();
    destination
        .execute(
            "INSERT INTO projects VALUES('new-project','bounded raw document',0,0)",
            [],
        )
        .unwrap();
    frozen
        .verify(&destination, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    let bad = fixture.ledger(Layout::Catalog);
    bad.capture_baseline(&destination, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    invalid_result(bad.expect_from_plan(&plan, ArchiveSqlOrigin::Input, &["projects"]));
}

#[test]
fn every_catalog_table_including_local_actions_is_in_the_final_ledger() {
    let fixture = Fixture::new();
    let connection = empty(Layout::Catalog);
    connection
        .execute(
            "INSERT INTO performance_source_actions VALUES('local-action',1,?1,x'7b7d',?2)",
            params!["a".repeat(64), "b".repeat(64)],
        )
        .unwrap();
    let ledger = fixture.ledger(Layout::Catalog);
    ledger
        .capture_baseline(&connection, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    let frozen = ledger.freeze().unwrap();
    connection
        .execute("UPDATE performance_source_actions SET payload=x'5b5d'", [])
        .unwrap();
    invalid_result(frozen.verify(&connection, ArchiveSqlOrigin::LiveCatalog));
}

#[test]
fn schema_extra_view_generated_virtual_and_late_trigger_changes_refuse() {
    for attack in 0..5 {
        let fixture = Fixture::new();
        let connection = empty(Layout::Catalog);
        let ledger = fixture.ledger(Layout::Catalog);
        if attack == 4 {
            ledger
                .capture_schema(&connection, ArchiveSqlOrigin::LiveCatalog)
                .unwrap();
            let frozen = ledger.freeze().unwrap();
            connection
                .execute_batch(
                    "CREATE TRIGGER changed_schema AFTER INSERT ON assets BEGIN SELECT 1; END",
                )
                .unwrap();
            invalid_result(frozen.verify(&connection, ArchiveSqlOrigin::LiveCatalog));
            continue;
        }
        match attack {
            0=>connection.execute_batch("CREATE TABLE extra(id INTEGER PRIMARY KEY)").unwrap(),
            1=>connection.execute_batch("DROP TABLE assets;CREATE VIEW assets AS SELECT 'x' AS id,x'00' AS payload").unwrap(),
            2=>connection.execute_batch("DROP TABLE assets;CREATE TABLE assets(id TEXT PRIMARY KEY,payload BLOB GENERATED ALWAYS AS (zeroblob(1000000000)) VIRTUAL)").unwrap(),
            _=>connection.execute_batch("DROP TABLE assets;CREATE VIRTUAL TABLE assets USING fts5(id,payload)").unwrap(),
        }
        invalid_result(ledger.capture_baseline(&connection, ArchiveSqlOrigin::LiveCatalog));
    }
}

#[test]
fn field_primary_key_row_and_checked_raw_counter_bounds_refuse_before_freeze() {
    let fixture = Fixture::new();
    let connection = empty(Layout::Catalog);
    let ledger = fixture.ledger(Layout::Catalog);
    ledger
        .capture_schema(&connection, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    let payload = vec![0; 16_385];
    let id = "1".repeat(32);
    let hash = "a".repeat(64);
    invalid_result(ledger.expect_insert(
        "performance_source_actions",
        &[
            ValueRef::Text(id.as_bytes()),
            ValueRef::Integer(1),
            ValueRef::Text(hash.as_bytes()),
            ValueRef::Blob(&payload),
            ValueRef::Text(hash.as_bytes()),
        ],
        false,
    ));
    let ledger = fixture.ledger(Layout::Catalog);
    ledger
        .capture_schema(&connection, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    let long_a = "\0".repeat(64);
    let long_b = "\0".repeat(96);
    invalid_result(ledger.expect_insert(
        "archive_copy_proof_operations",
        &[
            ValueRef::Text(long_a.as_bytes()),
            ValueRef::Text(long_b.as_bytes()),
            ValueRef::Text(b"destination"),
            ValueRef::Text(hash.as_bytes()),
            ValueRef::Text(hash.as_bytes()),
            ValueRef::Text(hash.as_bytes()),
            ValueRef::Text(hash.as_bytes()),
        ],
        false,
    ));
    for raw_overflow in [false, true] {
        let ledger = fixture.ledger(Layout::Catalog);
        ledger
            .capture_schema(&connection, ArchiveSqlOrigin::LiveCatalog)
            .unwrap();
        if raw_overflow {
            ledger.raw_bytes.set(u64::MAX);
        } else {
            ledger.rows.set(MAX_ROWS);
        }
        invalid_result(ledger.expect_insert(
            "assets",
            &[ValueRef::Text(b"key"), ValueRef::Blob(b"one")],
            false,
        ));
        invalid_result(ledger.freeze());
    }
    connection
        .execute(
            "INSERT INTO performance_source_reviews VALUES('review',zeroblob(65537),'hash')",
            [],
        )
        .unwrap();
    let ledger = fixture.ledger(Layout::Catalog);
    invalid_result(ledger.capture_baseline(&connection, ArchiveSqlOrigin::LiveCatalog));
}

#[test]
fn disposable_main_file_page_cap_is_real_and_reports_private_output_sql() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger(Layout::Catalog);
    let configured: u32 = ledger
        .connection
        .pragma_query_value(None, "max_page_count", |row| row.get(0))
        .unwrap();
    assert_eq!(configured, PAGES);
    let used: u32 = ledger
        .connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .unwrap();
    ledger
        .connection
        .pragma_update(None, "max_page_count", used)
        .unwrap();
    let mut refusal = None;
    for index in 0..1000 {
        let id = format!("key-{index:08}");
        if let Err(error) = ledger.expect_insert(
            "assets",
            &[ValueRef::Text(id.as_bytes()), ValueRef::Blob(b"small")],
            false,
        ) {
            refusal = Some(error);
            break;
        }
    }
    let Some(StoreError::ArchiveSql(sql)) = refusal else {
        panic!("Expected finite lower-page-limit SQLITE_FULL")
    };
    assert_eq!(sql.origin(), ArchiveSqlOrigin::PrivateOutput);
    assert!(
        matches!(sql.operation_error(),StoreError::Database(error) if error.sqlite_error().is_some_and(|code|code.extended_code&0xff==rusqlite::ffi::SQLITE_FULL))
    );
}

#[test]
fn source_sql_origins_are_preserved_for_actual_malformed_private_fixture() {
    let fixture = Fixture::new();
    let path = fixture.0.join("notadb.sqlite3");
    fs::write(&path, [0xa5; 4096]).unwrap();
    let malformed = Connection::open(&path).unwrap();
    for origin in [ArchiveSqlOrigin::Input, ArchiveSqlOrigin::LiveCatalog] {
        let ledger = fixture.ledger(Layout::Catalog);
        let failure = ledger.capture_baseline(&malformed, origin).unwrap_err();
        let StoreError::ArchiveSql(sql) = &failure else {
            panic!("Expected actual SQLite failure")
        };
        assert_eq!(sql.origin(), origin);
        assert_eq!(
            failure.catalog_sqlite_error_code().is_some(),
            origin == ArchiveSqlOrigin::LiveCatalog
        );
    }
}

#[test]
fn borrowed_governor_cancellation_work_vm_and_later_sticky_stop_never_reset() {
    use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};
    let fixture = Fixture::new();
    let connection = empty(Layout::Catalog);
    for (limits, resource) in [
        (
            InspectionLimits {
                max_work_bytes: 1,
                ..Default::default()
            },
            InspectionResource::WorkBytes,
        ),
        (
            InspectionLimits {
                max_vm_steps: 1,
                ..Default::default()
            },
            InspectionResource::VmSteps,
        ),
    ] {
        let (budget, _) = governor(limits);
        assert!(
            matches!(PostimageLedger::new(&fixture.0,Layout::Catalog,Some(&budget)),Err(StoreError::Recording(RecordingError::InspectionLimit{resource:actual,..})) if actual==resource)
        );
        assert!(
            matches!(budget.checkpoint(),Err(RecordingError::InspectionLimit{resource:actual,..}) if actual==resource)
        );
    }
    let (budget, cancel) = governor(InspectionLimits::default());
    let ledger = PostimageLedger::new(&fixture.0, Layout::Catalog, Some(&budget)).unwrap();
    ledger
        .capture_schema(&connection, ArchiveSqlOrigin::Input)
        .unwrap();
    let before = budget.usage();
    cancel.store(true, Ordering::Release);
    assert!(matches!(
        ledger.expect_insert("assets", &[ValueRef::Text(b"key"), ValueRef::Null], false),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    assert_eq!(budget.usage(), before);
    assert!(matches!(
        PostimageLedger::new(&fixture.0, Layout::Catalog, Some(&budget)),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
}

#[test]
fn strict_v1_v2_layouts_keep_archive_payload_rows_in_the_expected_set() {
    for layout in [Layout::ArchiveV1, Layout::ArchiveV2] {
        let fixture = Fixture::new();
        let connection = empty(layout);
        let ledger = fixture.ledger(layout);
        ledger
            .capture_schema(&connection, ArchiveSqlOrigin::PrivateOutput)
            .unwrap();
        let payload = vec![9u8; CHUNK + 7];
        let object = "a".repeat(64);
        let digest = "b".repeat(64);
        ledger
            .expect_insert(
                "take_chunks",
                &[
                    ValueRef::Text(object.as_bytes()),
                    ValueRef::Integer(0),
                    ValueRef::Integer(payload.len() as i64),
                    ValueRef::Text(digest.as_bytes()),
                    ValueRef::Blob(&payload),
                ],
                false,
            )
            .unwrap();
        let frozen = ledger.freeze().unwrap();
        connection
            .execute(
                "INSERT INTO take_chunks VALUES(?1,0,?2,?3,?4)",
                params![object, payload.len() as i64, digest, payload],
            )
            .unwrap();
        frozen
            .verify(&connection, ArchiveSqlOrigin::PrivateOutput)
            .unwrap();
    }
}

#[test]
fn exact_baseline_replacement_and_remove_then_insert_preserve_every_other_row() {
    let fixture = Fixture::new();
    let db = empty(Layout::Catalog);
    let old = Value::Blob(vec![1, 2, 3]);
    let next = Value::Blob(vec![4, 5]);
    put_asset(&db, "change", &old);
    put_asset(&db, "keep", &old);
    let ledger = fixture.ledger(Layout::Catalog);
    ledger
        .capture_baseline(&db, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    ledger
        .expect_replace(
            "assets",
            &refs(&asset("change", old.clone())),
            &refs(&asset("change", next.clone())),
        )
        .unwrap();
    ledger
        .expect_remove("assets", &refs(&asset("keep", old.clone())))
        .unwrap();
    ledger
        .expect_insert("assets", &refs(&asset("keep", next.clone())), false)
        .unwrap();
    let expected = ledger.freeze().unwrap();
    db.execute("UPDATE assets SET payload=?1", [next]).unwrap();
    expected.verify(&db, ArchiveSqlOrigin::LiveCatalog).unwrap();
}

#[test]
fn baseline_cas_rejects_wrong_preimage_key_changes_and_duplicate_changes_and_poison_freeze() {
    for mode in 0..4 {
        let fixture = Fixture::new();
        let db = empty(Layout::Catalog);
        let old = Value::Blob(vec![1]);
        let next = Value::Blob(vec![2]);
        put_asset(&db, "a", &old);
        let ledger = fixture.ledger(Layout::Catalog);
        ledger
            .capture_baseline(&db, ArchiveSqlOrigin::LiveCatalog)
            .unwrap();
        let result = match mode {
            0 => ledger.expect_replace(
                "assets",
                &refs(&asset("a", next.clone())),
                &refs(&asset("a", old.clone())),
            ),
            1 => ledger.expect_replace(
                "assets",
                &refs(&asset("a", old.clone())),
                &refs(&asset("b", next.clone())),
            ),
            2 => {
                ledger
                    .expect_replace(
                        "assets",
                        &refs(&asset("a", old.clone())),
                        &refs(&asset("a", next.clone())),
                    )
                    .unwrap();
                ledger.expect_remove("assets", &refs(&asset("a", old.clone())))
            }
            _ => ledger.expect_remove("assets", &refs(&asset("a", next.clone()))),
        };
        invalid_result(result);
        invalid_result(ledger.freeze());
        let payload: Vec<u8> = db
            .query_row("SELECT payload FROM assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(payload, vec![1]);
    }
}

#[test]
fn frozen_local_overlay_rejects_same_count_replacement_of_an_unrelated_baseline_row() {
    let fixture = Fixture::new();
    let db = empty(Layout::Catalog);
    let old = Value::Blob(vec![1]);
    let next = Value::Blob(vec![2]);
    put_asset(&db, "change", &old);
    put_asset(&db, "unrelated", &old);
    let ledger = fixture.ledger(Layout::Catalog);
    ledger
        .capture_baseline(&db, ArchiveSqlOrigin::LiveCatalog)
        .unwrap();
    ledger
        .expect_replace(
            "assets",
            &refs(&asset("change", old.clone())),
            &refs(&asset("change", next.clone())),
        )
        .unwrap();
    let expected = ledger.freeze().unwrap();
    db.execute("UPDATE assets SET payload=?1", [next]).unwrap();
    invalid_result(expected.verify(&db, ArchiveSqlOrigin::LiveCatalog));
}

#[test]
fn private_sql_scope_maps_only_its_observed_interrupt_and_detaches_before_reuse() {
    use crate::archive_governed::PrivateSql;
    use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};
    let db = Connection::open_in_memory().unwrap();
    let (budget, _) = governor(InspectionLimits {
        max_vm_steps: 128,
        ..Default::default()
    });
    let mut scope = PrivateSql::new(&db, &budget, ArchiveSqlOrigin::PrivateOutput).unwrap();
    let error=db.query_row::<i64,_,_>("WITH RECURSIVE n(x) AS(VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000) SELECT sum(x) FROM n",[],|r|r.get(0)).unwrap_err();
    assert_eq!(
        error.sqlite_error().unwrap().extended_code & 0xff,
        rusqlite::ffi::SQLITE_INTERRUPT
    );
    let mapped: Result<()> = scope.finish(
        &budget,
        Err(boundary::sql_error(ArchiveSqlOrigin::PrivateOutput, error)),
    );
    assert!(matches!(
        mapped,
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::VmSteps,
            ..
        }))
    ));
    let other = boundary::sql_error(
        ArchiveSqlOrigin::PrivateOutput,
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL), None),
    );
    let kept: Result<()> = scope.finish(&budget, Err(other));
    assert!(
        matches!(kept,Err(StoreError::ArchiveSql(value)) if value.origin()==ArchiveSqlOrigin::PrivateOutput && matches!(value.operation_error(),StoreError::Database(error) if error.sqlite_error().unwrap().extended_code==rusqlite::ffi::SQLITE_FULL))
    );
    scope.detach().unwrap();
    drop(scope);
    assert_eq!(
        db.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

// Independent incremental-byte controls. All databases below are in memory;
// constructed close failures do not claim physical I/O fault injection.
mod incremental_byte_review {
    use super::*;
    use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};

    #[test]
    fn incremental_text_and_blob_bytes_match_sqlite_cast_in_all_three_database_encodings() {
        type Image = (i64, String, Vec<u8>);
        for encoding in ["UTF-8", "UTF-16le", "UTF-16be"] {
            let db = Connection::open_in_memory().unwrap();
            db.execute_batch(&format!(
                "PRAGMA encoding='{encoding}';CREATE TABLE cells(payload);"
            ))
            .unwrap();
            let actual_encoding: String = db
                .pragma_query_value(None, "encoding", |row| row.get(0))
                .unwrap();
            assert!(actual_encoding.eq_ignore_ascii_case(encoding));
            let text = "é𝄞\0z".repeat(100_000);
            let blob: Vec<u8> = (0..1_048_593).map(|index| (index % 251) as u8).collect();
            for value in [
                Value::Text(String::new()),
                Value::Blob(Vec::new()),
                Value::Text("a\0é𝄞after".into()),
                Value::Blob(vec![0, 255, 0, 1]),
                Value::Text(text),
                Value::Blob(blob),
            ] {
                db.execute("INSERT INTO cells VALUES(?1)", [value]).unwrap();
            }
            let before: Vec<Image> = db
                .prepare(
                    "SELECT rowid,typeof(payload),CAST(payload AS BLOB) FROM cells ORDER BY rowid",
                )
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            for (rowid, kind, expected) in &before {
                let (budget, _) = governor(InspectionLimits::default());
                let mut actual = Vec::new();
                let mut chunks = 0;
                read_cell_chunks(
                    &db,
                    CellRead {
                        table: "cells",
                        column: "payload",
                        rowid: *rowid,
                        length: expected.len(),
                        row_bytes: expected.len() as u64,
                    },
                    ArchiveSqlOrigin::Input,
                    Some(&budget),
                    |bytes| {
                        assert!(!bytes.is_empty() && bytes.len() <= 65_536);
                        chunks += 1;
                        actual.extend_from_slice(bytes);
                    },
                )
                .unwrap();
                assert_eq!(&actual, expected, "{encoding} {kind} row {rowid}");
                assert_eq!(chunks == 0, expected.is_empty());
                // A generous linear bound rejects repeated full-cell work
                // per chunk on these approximately 1 MiB values.
                assert!(budget.usage().work_bytes <= 6 * (expected.len() as u64) + 131_072);
                assert_eq!(budget.usage().file_bytes, 0);
            }
            let after: Vec<Image> = db
                .prepare(
                    "SELECT rowid,typeof(payload),CAST(payload AS BLOB) FROM cells ORDER BY rowid",
                )
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(after, before);
        }
    }

    #[test]
    fn first_chunk_work_and_cancel_stops_close_the_handle_and_allow_fresh_budget_reuse() {
        for cancel in [false, true] {
            let db = Connection::open_in_memory().unwrap();
            db.execute_batch("CREATE TABLE cells(payload BLOB)")
                .unwrap();
            let expected: Vec<u8> = (0..196_625).map(|index| (index % 253) as u8).collect();
            db.execute("INSERT INTO cells VALUES(?1)", [&expected])
                .unwrap();
            let limit = 8 * 1024 * 1024;
            let (budget, token) = governor(InspectionLimits {
                max_work_bytes: limit,
                ..Default::default()
            });
            let mut first = Vec::new();
            let mut calls = 0;
            let result = read_cell_chunks(
                &db,
                CellRead {
                    table: "cells",
                    column: "payload",
                    rowid: 1,
                    length: expected.len(),
                    row_bytes: expected.len() as u64,
                },
                ArchiveSqlOrigin::PrivateOutput,
                Some(&budget),
                |bytes| {
                    calls += 1;
                    first.extend_from_slice(bytes);
                    if cancel {
                        token.store(true, Ordering::Release);
                    } else {
                        // Consume remaining public work credit at the observation
                        // boundary, without duplicating the reader's cost formula.
                        budget
                            .charge_work(limit - budget.usage().work_bytes)
                            .unwrap();
                    }
                },
            );
            if cancel {
                assert!(matches!(
                    result,
                    Err(StoreError::Recording(RecordingError::InspectionCancelled))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(StoreError::Recording(RecordingError::InspectionLimit {
                        resource: InspectionResource::WorkBytes,
                        ..
                    }))
                ));
            }
            assert_eq!(calls, 1);
            assert!(!first.is_empty() && first.len() <= 65_536);
            assert_eq!(first, expected[..first.len()]);
            assert!(budget.checkpoint().is_err());
            // VACUUM rejects a connection retaining an unfinished incremental
            // handle. It changes no logical row in this isolated fixture.
            db.execute_batch("VACUUM").unwrap();
            let (fresh, _) = governor(InspectionLimits::default());
            let mut actual = Vec::new();
            read_cell_chunks(
                &db,
                CellRead {
                    table: "cells",
                    column: "payload",
                    rowid: 1,
                    length: expected.len(),
                    row_bytes: expected.len() as u64,
                },
                ArchiveSqlOrigin::Input,
                Some(&fresh),
                |bytes| actual.extend_from_slice(bytes),
            )
            .unwrap();
            assert_eq!(actual, expected);
            let stored: Vec<u8> = db
                .query_row("SELECT payload FROM cells", [], |row| row.get(0))
                .unwrap();
            assert_eq!(stored, expected);
            assert!(db.is_autocommit());
        }
    }

    #[test]
    fn incremental_sql_origins_and_constructed_close_errors_survive_both_own_interrupt_finalizers()
    {
        use crate::archive_governed::PrivateSql;
        fn code(value: i32) -> rusqlite::Error {
            rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(value), None)
        }
        fn dual(origin: ArchiveSqlOrigin, primary: i32, cleanup: i32) -> StoreError {
            boundary::sql_read_cleanup_error(
                origin,
                StoreError::Database(code(primary)),
                code(cleanup),
            )
        }
        fn assert_dual(
            error: &StoreError,
            origin: ArchiveSqlOrigin,
            primary: i32,
            cleanup: i32,
            reported: i32,
        ) {
            let StoreError::ArchiveSql(sql) = error else {
                panic!("explicit SQL origin lost")
            };
            assert_eq!(sql.origin(), origin);
            assert!(
                matches!(sql.operation_error(),StoreError::Database(error) if error.sqlite_error().unwrap().extended_code==primary)
            );
            assert_eq!(
                sql.cleanup_error()
                    .unwrap()
                    .sqlite_error()
                    .unwrap()
                    .extended_code,
                cleanup
            );
            assert_eq!(
                error.catalog_sqlite_error_code(),
                (origin == ArchiveSqlOrigin::LiveCatalog).then_some(reported)
            );
            assert_eq!(
                error.catalog_connection_unusable(),
                origin == ArchiveSqlOrigin::LiveCatalog
            );
        }
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE cells(payload BLOB);INSERT INTO cells VALUES(x'0001ff00')")
            .unwrap();
        for origin in [
            ArchiveSqlOrigin::Input,
            ArchiveSqlOrigin::PrivateOutput,
            ArchiveSqlOrigin::LiveCatalog,
        ] {
            let mut visited = false;
            let error = read_cell_chunks(
                &db,
                CellRead {
                    table: "cells",
                    column: "missing",
                    rowid: 1,
                    length: 4,
                    row_bytes: 4,
                },
                origin,
                None,
                |_| visited = true,
            )
            .unwrap_err();
            assert!(!visited);
            assert!(
                matches!(&error,StoreError::ArchiveSql(sql) if sql.origin()==origin && sql.cleanup_error().is_none() && matches!(sql.operation_error(),StoreError::Database(_)))
            );
            assert_eq!(
                error.catalog_sqlite_error_code().is_some(),
                origin == ArchiveSqlOrigin::LiveCatalog
            );
            for (primary, cleanup, reported) in [(9, 10, 10), (26, 10, 26), (8, 11, 11), (9, 5, 9)]
            {
                let (budget, token) = governor(InspectionLimits::default());
                token.store(true, Ordering::Release);
                let own = AtomicBool::new(true);
                let kept: Result<()> =
                    finish_spool(Some(&budget), &own, Err(dual(origin, primary, cleanup)));
                assert_dual(&kept.unwrap_err(), origin, primary, cleanup, reported);
            }
            let (budget, token) = governor(InspectionLimits::default());
            token.store(true, Ordering::Release);
            let own = AtomicBool::new(true);
            // CONSTRUCTED close-only INTERRUPT has one SQL error with an
            // explicit close marker, rather than two independent failures.
            let close = boundary::sql_read_close_error(origin, code(9));
            let kept: Result<()> = finish_spool(Some(&budget), &own, Err(close));
            assert_dual(&kept.unwrap_err(), origin, 9, 9, 9);
        }
        for private_scope in [false, true] {
            let database = Connection::open_in_memory().unwrap();
            let (budget, _) = governor(InspectionLimits {
                max_vm_steps: 128,
                ..Default::default()
            });
            let observed = Arc::new(AtomicBool::new(false));
            let mut scope = if private_scope {
                Some(PrivateSql::new(&database, &budget, ArchiveSqlOrigin::PrivateOutput).unwrap())
            } else {
                let mark = Arc::clone(&observed);
                let mut hook = budget.sqlite_vm_governor();
                database
                    .progress_handler(
                        1,
                        Some(move || {
                            let stopped = hook();
                            if stopped {
                                mark.store(true, Ordering::Release);
                            }
                            stopped
                        }),
                    )
                    .unwrap();
                None
            };
            let actual=database.query_row::<i64,_,_>("WITH RECURSIVE n(x) AS(VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000) SELECT sum(x) FROM n",[],|row|row.get(0)).unwrap_err();
            assert_eq!(actual.sqlite_error().unwrap().extended_code & 0xff, 9);
            assert!(
                scope
                    .as_ref()
                    .map_or_else(|| observed.load(Ordering::Acquire), PrivateSql::interrupted)
            );
            // Actual own INTERRUPT plus a CONSTRUCTED close CORRUPT must retain
            // both facts instead of becoming the hook's ordinary VM stop.
            let mixed = boundary::sql_read_cleanup_error(
                ArchiveSqlOrigin::PrivateOutput,
                StoreError::Database(actual),
                code(11),
            );
            let kept: Result<()> = if let Some(scope) = &scope {
                scope.finish(&budget, Err(mixed))
            } else {
                finish_spool(Some(&budget), &observed, Err(mixed))
            };
            assert_dual(
                &kept.unwrap_err(),
                ArchiveSqlOrigin::PrivateOutput,
                9,
                11,
                9,
            );
            let close = boundary::sql_read_close_error(ArchiveSqlOrigin::PrivateOutput, code(9));
            let kept: Result<()> = if let Some(scope) = &scope {
                scope.finish(&budget, Err(close))
            } else {
                finish_spool(Some(&budget), &observed, Err(close))
            };
            assert_dual(&kept.unwrap_err(), ArchiveSqlOrigin::PrivateOutput, 9, 9, 9);
            let plain = Err(boundary::sql_error(
                ArchiveSqlOrigin::PrivateOutput,
                code(9),
            ));
            let mapped: Result<()> = if let Some(scope) = &scope {
                scope.finish(&budget, plain)
            } else {
                finish_spool(Some(&budget), &observed, plain)
            };
            assert!(matches!(
                mapped,
                Err(StoreError::Recording(RecordingError::InspectionLimit {
                    resource: InspectionResource::VmSteps,
                    ..
                }))
            ));
            if let Some(scope) = &mut scope {
                scope.detach().unwrap();
            } else {
                database.progress_handler(0, None::<fn() -> bool>).unwrap();
            }
            drop(scope);
            assert_eq!(
                database
                    .query_row("SELECT 7", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                7
            );
        }
        assert_eq!(
            db.query_row("SELECT payload FROM cells", [], |row| row
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            vec![0, 1, 255, 0]
        );
    }

    #[test]
    fn wide_row_tiny_trailing_values_match_independent_typed_hash_and_require_prepaid_row_work() {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        use std::sync::atomic::AtomicUsize;
        const WIDE: Table = Table {
            name: "wide",
            columns: &[
                blob("leading", 2 * 1024 * 1024),
                integer("late_number"),
                text("tail", 96),
            ],
            pk: &[2],
        };
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE wide(leading BLOB,late_number INTEGER,tail TEXT)")
            .unwrap();
        let leading: Vec<u8> = (0..524_305).map(|index| (index % 241) as u8).collect();
        let number = i64::MIN + 31;
        let tail = "tiny\0é";
        db.execute(
            "INSERT INTO wide VALUES(?1,?2,?3)",
            params![leading, number, tail],
        )
        .unwrap();
        let (key, actual, raw) = proof_row(&db, WIDE, 1, ArchiveSqlOrigin::Input, None).unwrap();
        let mut oracle = Sha256::new();
        oracle.update(b"OmniuSong.archive-postimage-row.v1\0");
        oracle.update(4_u64.to_le_bytes());
        oracle.update(b"wide");
        oracle.update(3_u64.to_le_bytes());
        let number_bytes = number.to_le_bytes();
        for (tag, bytes) in [
            (b'b', leading.as_slice()),
            (b'i', number_bytes.as_slice()),
            (b't', tail.as_bytes()),
        ] {
            oracle.update([tag]);
            oracle.update((bytes.len() as u64).to_le_bytes());
            oracle.update(bytes);
        }
        assert_eq!(actual, <[u8; 32]>::from(oracle.finalize()));
        assert_eq!(raw, (leading.len() + 8 + tail.len()) as u64);
        assert_eq!(
            key,
            vec![0x20, b't', b'i', b'n', b'y', 0, 255, 0xc3, 0xa9, 0, 0]
        );
        let reads = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&reads);
        db.authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "wide",
                    column_name: "late_number",
                    ..
                }
            ) && observed.fetch_add(1, Ordering::SeqCst) >= 3
            {
                return Authorization::Deny;
            }
            Authorization::Allow
        }))
        .unwrap();
        // Enough work for metadata plus the first large value, fewer than two
        // complete row-value passes. The late scalar SELECT must not prepare.
        let (budget, _) = governor(InspectionLimits {
            max_work_bytes: 5 * raw + 65_536,
            ..Default::default()
        });
        let stopped = proof_row(&db, WIDE, 1, ArchiveSqlOrigin::Input, Some(&budget));
        assert!(matches!(
            stopped,
            Err(StoreError::Recording(RecordingError::InspectionLimit {
                resource: InspectionResource::WorkBytes,
                ..
            }))
        ));
        assert_eq!(
            reads.load(Ordering::SeqCst),
            3,
            "late scalar value was requested before whole-row credit"
        );
        db.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let stored: (Vec<u8>, i64, String) = db
            .query_row("SELECT leading,late_number,tail FROM wide", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(stored, (leading, number, tail.into()));
        let (fresh, _) = governor(InspectionLimits::default());
        assert_eq!(
            proof_row(&db, WIDE, 1, ArchiveSqlOrigin::Input, Some(&fresh)).unwrap(),
            (key, actual, raw)
        );
    }
}
