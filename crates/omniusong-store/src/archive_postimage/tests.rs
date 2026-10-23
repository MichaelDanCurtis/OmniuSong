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
fn repeated_substring_queries_prepay_full_cell_work_each_time() {
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
