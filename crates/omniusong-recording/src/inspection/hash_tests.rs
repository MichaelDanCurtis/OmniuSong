//! Compatibility oracle pinned to e95d5e9 before statement reuse.
//! Synthetic in-memory rows only; no native capture or seal authority.
use super::*;
use crate::{InspectionLimits, InspectionResource};
use rusqlite::types::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const HASH_SCHEMA: &str = r#"
CREATE TABLE take_meta(singleton INTEGER PRIMARY KEY,descriptor BLOB,descriptor_sha256 TEXT,state TEXT);
CREATE TABLE stream_prefix(stream_id TEXT,next_sequence INTEGER,source_end INTEGER,recorded_units INTEGER,gap_units INTEGER,gap_chunks INTEGER);
CREATE TABLE chunks(ordinal INTEGER PRIMARY KEY AUTOINCREMENT,stream_id TEXT,sequence INTEGER,header BLOB,payload BLOB,sha256 TEXT,"opaque""field λ");
CREATE TABLE interruptions(singleton INTEGER PRIMARY KEY,receipt BLOB,sha256 TEXT);
CREATE TABLE recovery_index(ordinal INTEGER PRIMARY KEY,sha256 TEXT);
CREATE TABLE sealed_manifests(singleton INTEGER PRIMARY KEY,operation_id TEXT,body BLOB,sha256 TEXT);
"#;

fn empty_fixture() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(HASH_SCHEMA).unwrap();
    connection
}
fn fixture() -> Connection {
    let connection = empty_fixture();
    connection.execute_batch(
        "INSERT INTO take_meta VALUES(1,x'0001ff','descriptor-sha','recording');
         INSERT INTO stream_prefix(rowid,stream_id,next_sequence,source_end,recorded_units,gap_units,gap_chunks)
         VALUES(-9,'synthetic',3,12,12,0,0);",
    ).unwrap();
    connection
}
fn insert_row(connection: &Connection, ordinal: i64, value: Value, payload: &[u8]) {
    connection.execute(
        r#"INSERT INTO chunks(ordinal,stream_id,sequence,header,payload,sha256,"opaque""field λ")
           VALUES(?1,'synthetic',?1,x'00ff',?2,'chunk-sha',?3)"#,
        params![ordinal, payload, value],
    ).unwrap();
}
fn governor(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
        cancelled,
    )
}
type HashFunction = fn(&Connection, Option<&InspectionBudget>) -> Result<String>;

fn outer_hash(
    connection: &Connection,
    budget: &InspectionBudget,
    hasher: HashFunction,
) -> Result<String> {
    let result = hasher(connection, Some(budget));
    crate::budget::finish(Some(budget), result)
}
fn assert_compatible(connection: &Connection) -> String {
    let tx = connection.unchecked_transaction().unwrap();
    let expected = legacy_original_rows_hash(&tx).unwrap();
    assert_eq!(original_rows_hash(&tx).unwrap(), expected);
    let (old_budget, _) = governor(InspectionLimits::default());
    let (new_budget, _) = governor(InspectionLimits::default());
    assert_eq!(
        outer_hash(&tx, &old_budget, legacy_original_rows_hash_bounded).unwrap(),
        expected
    );
    assert_eq!(
        outer_hash(&tx, &new_budget, original_rows_hash_bounded).unwrap(),
        expected
    );
    // No SQL hooks are installed for this comparison: logical charges must be
    // identical, while actual VM work is covered separately below.
    assert_eq!(new_budget.usage(), old_budget.usage());
    expected
}
fn refused(result: Result<String>, resource: InspectionResource) -> RecordingError {
    let error = result.unwrap_err();
    assert!(
        matches!(&error, RecordingError::InspectionLimit { resource: actual, limit, attempted }
            if *actual == resource && attempted > limit),
        "Expected {resource:?} refusal, got {error:?}"
    );
    error
}

#[test]
fn reused_queries_match_frozen_v1_for_mixed_types_and_byte_boundaries() {
    let connection = fixture();
    let values = [
        Value::Null,
        Value::Integer(i64::MIN),
        Value::Integer(i64::MAX),
        Value::Real(-17.125),
        Value::Real(f64::INFINITY),
        Value::Real(f64::NEG_INFINITY),
        Value::Text(String::new()),
        Value::Blob(Vec::new()),
        Value::Text("before\0after λ🎹".into()),
        Value::Blob(vec![0, 255, 128, 1]),
        Value::Text("é\0🎹".repeat(20_000)),
    ];
    let lengths = [0, 1, 65_535, 65_536, 65_537, 131_089];
    for (index, value) in values.into_iter().enumerate() {
        let length = lengths[index % lengths.len()];
        let payload: Vec<u8> = (0..length).map(|offset| (offset % 251) as u8).collect();
        let ordinal = if index == 0 { -7 } else { index as i64 * 11 };
        insert_row(&connection, ordinal, value, &payload);
    }
    // Text bytes must not be converted through Rust UTF-8 or character substr.
    insert_row(&connection, 121, Value::Null, b"invalid-text-bytes");
    connection
        .execute(
            r#"UPDATE chunks SET "opaque""field λ"=CAST(x'ff00c32880' AS TEXT) WHERE ordinal=121"#,
            [],
        )
        .unwrap();
    assert_compatible(&connection);
}

#[test]
fn original_type_tail_sequence_and_state_rules_stay_exactly_v1() {
    let connection = fixture();
    insert_row(&connection, 42, Value::Text("42".into()), b"tail");
    insert_row(&connection, -7, Value::Null, b"");
    insert_row(&connection, 3, Value::Integer(7), b"prefix");
    let baseline = assert_compatible(&connection);
    connection
        .execute("UPDATE take_meta SET state='sealed'", [])
        .unwrap();
    assert_eq!(assert_compatible(&connection), baseline);
    for value in [
        Value::Blob(b"42".to_vec()),
        Value::Integer(42),
        Value::Real(42.0),
        Value::Null,
    ] {
        connection
            .execute(
                r#"UPDATE chunks SET "opaque""field λ"=?1 WHERE ordinal=42"#,
                [value],
            )
            .unwrap();
        assert_ne!(assert_compatible(&connection), baseline);
    }
    connection
        .execute(
            r#"UPDATE chunks SET "opaque""field λ"='42' WHERE ordinal=42"#,
            [],
        )
        .unwrap();
    assert_eq!(assert_compatible(&connection), baseline);
    connection
        .execute(
            "UPDATE sqlite_sequence SET seq=seq+1 WHERE name='chunks'",
            [],
        )
        .unwrap();
    assert_ne!(assert_compatible(&connection), baseline);
    let before_tail = assert_compatible(&connection);
    connection
        .execute("UPDATE chunks SET payload=x'7461696d' WHERE ordinal=42", [])
        .unwrap();
    assert_ne!(assert_compatible(&connection), before_tail);
}

#[test]
fn empty_tables_and_128_included_columns_match_frozen_v1() {
    let connection = empty_fixture();
    assert_compatible(&connection);
    // Seven initial included chunk columns plus 121 ordinary extensions.
    // This exceeds default statement-cache capacity without depending on it.
    for index in 0..121 {
        connection
            .execute_batch(&format!("ALTER TABLE chunks ADD COLUMN \"vendor_{index}\""))
            .unwrap();
    }
    insert_row(&connection, 1, Value::Integer(-1), b"");
    connection
        .execute("UPDATE chunks SET vendor_120=x'00ff'", [])
        .unwrap();
    assert_compatible(&connection);
}

#[test]
fn cumulative_logical_charges_and_mid_hash_limits_match_frozen_v1() {
    let connection = fixture();
    insert_row(&connection, 1, Value::Null, b"");
    // Created inside SQLite: neither hash may load this complete field in Rust.
    connection
        .execute(
            r#"UPDATE chunks SET "opaque""field λ"=zeroblob(2097169)"#,
            [],
        )
        .unwrap();
    let identity = assert_compatible(&connection);
    let (probe, _) = governor(InspectionLimits::default());
    outer_hash(&connection, &probe, legacy_original_rows_hash_bounded).unwrap();
    let once = probe.usage();
    for hasher in [
        legacy_original_rows_hash_bounded as HashFunction,
        original_rows_hash_bounded as HashFunction,
    ] {
        let (budget, _) = governor(InspectionLimits::default());
        outer_hash(&connection, &budget, hasher).unwrap();
        outer_hash(&connection, &budget, hasher).unwrap();
        assert_eq!(budget.usage().work_bytes, once.work_bytes * 2);
        assert_eq!(budget.usage().row_visits, once.row_visits * 2);
    }
    for resource in [InspectionResource::WorkBytes, InspectionResource::RowVisits] {
        let mut limits = InspectionLimits::default();
        match resource {
            InspectionResource::WorkBytes => limits.max_work_bytes = once.work_bytes - 65_536,
            InspectionResource::RowVisits => limits.max_row_visits = once.row_visits - 1,
            _ => unreachable!(),
        }
        let (old_budget, _) = governor(limits);
        let expected = refused(
            outer_hash(&connection, &old_budget, legacy_original_rows_hash_bounded),
            resource,
        );
        let (new_budget, cancelled) = governor(limits);
        let actual = refused(
            outer_hash(&connection, &new_budget, original_rows_hash_bounded),
            resource,
        );
        assert_eq!(actual.to_string(), expected.to_string());
        assert_eq!(new_budget.usage(), old_budget.usage());
        cancelled.store(true, Ordering::Release);
        let later = new_budget.finish::<()>(Err(RecordingError::Invalid("later row".into())));
        assert_eq!(later.unwrap_err().to_string(), actual.to_string());
    }
    assert_eq!(original_rows_hash(&connection).unwrap(), identity);
}

#[test]
fn reused_field_queries_observe_sticky_cancellation_after_an_accepted_row() {
    let connection = fixture();
    insert_row(&connection, 1, Value::Blob(vec![0; 65_537]), b"first");
    insert_row(&connection, 2, Value::Integer(42), b"second");
    let (budget, cancelled) = governor(InspectionLimits::default());
    let mut queries =
        OriginalFieldQueries::new(&connection, "chunks", "opaque\"field λ", Some(&budget)).unwrap();
    let mut hash = Sha256::new();
    hash_field(&mut queries, 1, &mut hash, Some(&budget)).unwrap();
    assert!(budget.usage().work_bytes > 131_072);
    let accepted = budget.usage();
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        budget.finish(hash_field(&mut queries, 2, &mut hash, Some(&budget))),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_eq!(budget.usage(), accepted);
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        outer_hash(&connection, &budget, original_rows_hash_bounded),
        Err(RecordingError::InspectionCancelled)
    ));
    assert!(matches!(
        budget.finish::<()>(Err(RecordingError::Invalid("later row".into()))),
        Err(RecordingError::InspectionCancelled)
    ));
}

#[test]
fn prepared_reuse_retains_actual_vm_hook_and_outer_typed_refusal() {
    let connection = fixture();
    for ordinal in 1..=12 {
        insert_row(&connection, ordinal, Value::Integer(ordinal), b"synthetic");
    }
    let mut queries =
        OriginalFieldQueries::new(&connection, "chunks", "opaque\"field λ", None).unwrap();
    let (budget, cancelled) = governor(InspectionLimits {
        max_vm_steps: 80,
        ..InspectionLimits::default()
    });
    budget.install(&connection).unwrap();
    let mut accepted = 0;
    let mut hash = Sha256::new();
    for ordinal in 1..=12 {
        let result = hash_field(&mut queries, ordinal, &mut hash, Some(&budget));
        match budget.finish(result) {
            Ok(()) => accepted += 1,
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::VmSteps,
                ..
            }) => break,
            other => panic!("Unexpected reused-query VM result: {other:?}"),
        }
    }
    assert!(accepted > 0 && accepted < 12);
    assert_eq!(budget.usage().vm_steps, 80);
    let error = refused(
        outer_hash(&connection, &budget, original_rows_hash_bounded),
        InspectionResource::VmSteps,
    );
    cancelled.store(true, Ordering::Release);
    assert_eq!(
        budget
            .finish::<()>(Err(RecordingError::Invalid("later row".into())))
            .unwrap_err()
            .to_string(),
        error.to_string()
    );
}

// Frozen pre-optimization implementation. Keep its SQL, transcript and charges
// independent from the reusable-query implementation above.
fn legacy_hash_field(
    connection: &Connection,
    table: &str,
    column: &str,
    rowid: i64,
    hash: &mut Sha256,
    budget: Option<&InspectionBudget>,
) -> Result<()> {
    crate::budget::rows(budget, 3)?;
    crate::budget::work(budget, 32)?;
    // Table names are compile-time fixed; column names are bounded metadata
    // identifiers quoted by the caller, never executable path/user SQL.
    let sql = format!(
        "SELECT typeof({column}),length(CAST({column} AS BLOB)) FROM {table} WHERE rowid=?1"
    );
    let (kind, len): (String, Option<i64>) =
        connection.query_row(&sql, [rowid], |r| Ok((r.get(0)?, r.get(1)?)))?;
    hash.update((kind.len() as u64).to_le_bytes());
    hash.update(kind.as_bytes());
    match kind.as_str() {
        "null" => {}
        "integer" => {
            crate::budget::rows(budget, 2)?;
            crate::budget::work(budget, 16)?;
            let n: i64 = connection.query_row(
                &format!("SELECT {column} FROM {table} WHERE rowid=?1"),
                [rowid],
                |r| r.get(0),
            )?;
            hash.update(n.to_le_bytes());
        }
        "real" => {
            crate::budget::rows(budget, 2)?;
            crate::budget::work(budget, 16)?;
            let n: f64 = connection.query_row(
                &format!("SELECT {column} FROM {table} WHERE rowid=?1"),
                [rowid],
                |r| r.get(0),
            )?;
            hash.update(n.to_bits().to_le_bytes());
        }
        "text" | "blob" => {
            let len = len.and_then(|n| u64::try_from(n).ok()).ok_or_else(|| {
                RecordingError::Invalid("Original capture field has invalid byte length".into())
            })?;
            hash.update(len.to_le_bytes());
            let mut offset = 0u64;
            while offset < len {
                let count = (len - offset).min(64 * 1024);
                crate::budget::rows(budget, 2)?;
                crate::budget::work(budget, count.saturating_mul(2))?;
                let bytes: Vec<u8> = connection.query_row(
                    &format!(
                        "SELECT substr(CAST({column} AS BLOB),?2,?3) FROM {table} WHERE rowid=?1"
                    ),
                    params![
                        rowid,
                        journal::sql_integer(offset + 1)?,
                        journal::sql_integer(count)?
                    ],
                    |r| r.get(0),
                )?;
                require(
                    bytes.len() == count as usize,
                    "Original capture field changed while hashing",
                )?;
                hash.update(bytes);
                offset += count;
            }
        }
        _ => {
            return Err(RecordingError::Invalid(
                "Unknown SQLite original field type".into(),
            ));
        }
    }
    Ok(())
}
pub(crate) fn legacy_original_rows_hash(connection: &Connection) -> Result<String> {
    legacy_original_rows_hash_bounded(connection, None)
}
pub(crate) fn legacy_original_rows_hash_bounded(
    connection: &Connection,
    budget: Option<&InspectionBudget>,
) -> Result<String> {
    crate::budget::checkpoint(budget)?;
    // Recovery's three derived tables and take state change intentionally.
    // Every retained original row, including invalid/unselected tails, is hashed
    // in bounded pieces without decoding or discarding its bytes.
    let mut tables =
        connection.prepare("SELECT typeof(name),length(CAST(name AS BLOB)),substr(name,1,65) FROM sqlite_schema WHERE type='table' ORDER BY name")?;
    let mut rows = tables.query([])?;
    while let Some(row) = rows.next()? {
        crate::budget::rows(budget, 4)?;
        crate::budget::work(budget, 88)?;
        let kind: String = row.get(0)?;
        let length: i64 = row.get(1)?;
        require(
            kind == "text" && (0..=64).contains(&length),
            "Capture schema table identity exceeds its bounded byte limit",
        )?;
        let name: String = row.get(2)?;
        require(
            matches!(
                name.as_str(),
                "take_meta"
                    | "stream_prefix"
                    | "chunks"
                    | "interruptions"
                    | "recovery_index"
                    | "sealed_manifests"
                    | "sqlite_sequence"
            ),
            "Unknown capture table cannot be authenticated for recovery",
        )?;
    }
    drop(rows);
    drop(tables);
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.capture-original-rows.v1\0");
    for table in [
        "sqlite_schema",
        "take_meta",
        "stream_prefix",
        "chunks",
        "sqlite_sequence",
    ] {
        let mut metadata=connection.prepare("SELECT substr(name,1,257),length(CAST(name AS BLOB)) FROM pragma_table_xinfo(?1) ORDER BY cid")?;
        let mut fields = metadata.query([table])?;
        let mut columns = Vec::new();
        while let Some(row) = fields.next()? {
            crate::budget::rows(budget, 3)?;
            crate::budget::work(budget, 528)?;
            let length: i64 = row.get(1)?;
            require(
                (1..=256).contains(&length) && columns.len() < 128,
                "Original capture column metadata exceeds its bounded limit",
            )?;
            let name: String = row.get(0)?;
            require(
                !["rowid", "_rowid_", "oid"]
                    .iter()
                    .any(|reserved| name.eq_ignore_ascii_case(reserved)),
                "An original column shadows SQLite row identity and cannot be authenticated",
            )?;
            if table != "take_meta" || !name.eq_ignore_ascii_case("state") {
                columns.push(name);
            }
        }
        drop(fields);
        drop(metadata);
        require(!columns.is_empty(), "Missing capture source table columns")?;
        hash.update((table.len() as u64).to_le_bytes());
        hash.update(table.as_bytes());
        hash.update((columns.len() as u64).to_le_bytes());
        for name in &columns {
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
        }
        let mut statement =
            connection.prepare(&format!("SELECT rowid FROM {table} ORDER BY rowid"))?;
        let mut rows = statement.query([])?;
        let mut count = 0u64;
        while let Some(row) = rows.next()? {
            crate::budget::rows(budget, 2)?;
            crate::budget::work(budget, 16)?;
            let rowid: i64 = row.get(0)?;
            hash.update(rowid.to_le_bytes());
            for column in &columns {
                let quoted = format!("\"{}\"", column.replace('"', "\"\""));
                legacy_hash_field(connection, table, &quoted, rowid, &mut hash, budget)?;
            }
            count += 1;
        }
        hash.update(count.to_le_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}
