//! Independent opaque-ledger tests. Fixtures are isolated synthetic catalogs;
//! capacity rows do not claim legal typed Prepared transitions or recovery.
use super::{NativeStore, PerformanceSourceActionHistory};
use crate::{NativeReader, STORE_SCHEMA_VERSION, StoreError};
use rusqlite::{Connection, params, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
};

const IDENTITIES: u64 = 32;
const REVISIONS: u64 = 16;
const REVIEW_BYTES: usize = 65_536;
const ACTION_BYTES: usize = 16_384;
const REVIEW: &[u8] =
    b" {\n \"usage\":{\"row_visits\":2,\"vm_steps\":7},\"\\u006eame\":\"\\u00e9\"\n} ";
const SAME_REVIEW: &[u8] = b"{\"name\":\"\xc3\xa9\",\"usage\":{\"vm_steps\":7,\"row_visits\":2}}";
const ACTION: &[u8] = b" {\"phase\":\"Prepared\", \"choice\":\"copy\"}\n";
const SAME_ACTION: &[u8] = b"{\"choice\":\"copy\",\"phase\":\"Prepared\"}";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-source-action-independent")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn store(&self) -> NativeStore {
        NativeStore::open(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Source-action fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn id(index: u64) -> String {
    format!("{index:032x}")
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, PartialEq, Eq)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
fn raw_rows(connection: &Connection, sql: &str) -> Vec<Vec<Cell>> {
    let mut statement = connection.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| {
                    Ok(match row.get_ref(column)? {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(value) => Cell::Integer(value),
                        ValueRef::Real(value) => Cell::Real(value.to_bits()),
                        ValueRef::Text(value) => Cell::Text(value.to_vec()),
                        ValueRef::Blob(value) => Cell::Blob(value.to_vec()),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    version: i64,
    schema: Vec<Vec<Cell>>,
    tables: BTreeMap<String, Vec<Vec<Cell>>>,
}
fn snapshot(connection: &Connection, include_new: bool) -> Snapshot {
    let exclusion = if include_new {
        ""
    } else {
        " AND tbl_name NOT IN ('performance_source_reviews','performance_source_actions')"
    };
    let schema = raw_rows(
        connection,
        &format!(
            "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'{exclusion} ORDER BY type,name"
        ),
    );
    let names: Vec<String> = connection.prepare(&format!("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'{exclusion} ORDER BY name")).unwrap()
        .query_map([], |row| row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    let tables = names
        .into_iter()
        .map(|name| {
            let sql = format!(
                "SELECT * FROM \"{}\" ORDER BY rowid",
                name.replace('"', "\"\"")
            );
            let rows = raw_rows(connection, &sql);
            (name, rows)
        })
        .collect();
    Snapshot {
        version: connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap(),
        schema,
        tables,
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), result);
            } else {
                result.insert(
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
fn loosen_fixture(connection: &Connection) {
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;
        DROP TRIGGER immutable_performance_source_review_update;
        DROP TRIGGER immutable_performance_source_review_delete;
        DROP TRIGGER immutable_performance_source_action_update;
        DROP TRIGGER immutable_performance_source_action_delete;",
        )
        .unwrap();
}
fn loose_table(connection: &Connection, table: &str) {
    assert!(matches!(
        table,
        "performance_source_reviews" | "performance_source_actions"
    ));
    connection.execute_batch(&format!("CREATE TABLE loose_fixture_rows AS SELECT * FROM {table}; DROP TABLE {table}; ALTER TABLE loose_fixture_rows RENAME TO {table};")).unwrap();
}
fn rehash_fixture_payloads(connection: &Connection, table: &str) {
    assert!(matches!(
        table,
        "performance_source_reviews" | "performance_source_actions"
    ));
    let mut statement = connection
        .prepare(&format!("SELECT rowid,payload FROM {table}"))
        .unwrap();
    let rows: Vec<(i64, Vec<u8>)> = statement
        .query_map([], |row| {
            let bytes = match row.get_ref(1)? {
                ValueRef::Blob(bytes) | ValueRef::Text(bytes) => bytes.to_vec(),
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok((row.get(0)?, bytes))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    drop(statement);
    for (rowid, payload) in rows {
        connection
            .execute(
                &format!("UPDATE {table} SET payload_sha256=?1 WHERE rowid=?2"),
                params![sha(&payload), rowid],
            )
            .unwrap();
    }
}
fn refuse_unchanged(store: &mut NativeStore) {
    let before = snapshot(&store.connection, true);
    assert!(store.list_performance_source_actions().is_err());
    assert!(
        store
            .save_performance_source_action(&id(31), None, b"{}", b"{}")
            .is_err()
    );
    assert_eq!(snapshot(&store.connection, true), before);
}
fn exact_object(bytes: usize, tag: &str) -> Vec<u8> {
    let prefix = format!(" {{\"sentinel\":\"BEGIN-{tag}\",\"data\":\"");
    let suffix = format!("|END-{tag}\"}} \n");
    let mut result = prefix.into_bytes();
    result.resize(bytes - suffix.len(), b'x');
    let middle = format!("MID-{tag}");
    let offset = result.len() / 2;
    result[offset..offset + middle.len()].copy_from_slice(middle.as_bytes());
    result.extend_from_slice(suffix.as_bytes());
    assert_eq!(result.len(), bytes);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&result)
            .unwrap()
            .is_object()
    );
    result
}
fn seed(connection: &mut Connection, identities: u64, revisions: u64, maximum: bool) {
    let tx = connection.transaction().unwrap();
    for identity in 1..=identities {
        let key = id(identity);
        let review = if maximum {
            exact_object(REVIEW_BYTES, &format!("review-{identity}"))
        } else {
            REVIEW.to_vec()
        };
        let review_sha = sha(&review);
        tx.execute(
            "INSERT INTO performance_source_reviews(id,payload,payload_sha256) VALUES(?1,?2,?3)",
            params![key, review, review_sha],
        )
        .unwrap();
        for revision in 1..=revisions {
            let payload = if maximum {
                exact_object(
                    ACTION_BYTES,
                    &format!("action-{identity}-revision-{revision}"),
                )
            } else {
                format!("{{\"raw_revision\":{revision}}}").into_bytes()
            };
            tx.execute("INSERT INTO performance_source_actions(id,revision,review_blob_sha256,payload,payload_sha256) VALUES(?1,?2,?3,?4,?5)", params![key, revision as i64, review_sha, payload, sha(&payload)]).unwrap();
        }
    }
    tx.commit().unwrap();
}

#[test]
fn exact_noncanonical_review_and_action_bytes_survive_receipts_sql_and_reopen() {
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(REVIEW).unwrap(),
        serde_json::from_slice::<serde_json::Value>(SAME_REVIEW).unwrap()
    );
    assert_ne!(REVIEW, SAME_REVIEW);
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let receipt = store
        .save_performance_source_action(&id(1), None, REVIEW, ACTION)
        .unwrap();
    assert_eq!(receipt.id, id(1));
    assert_eq!(receipt.revision, 1);
    assert_eq!(receipt.payload, ACTION);
    assert_eq!(receipt.payload_sha256, sha(ACTION));
    assert_eq!(receipt.review_blob_sha256, sha(REVIEW));
    assert_eq!(
        raw_rows(
            &store.connection,
            "SELECT typeof(payload),payload FROM performance_source_reviews"
        ),
        vec![vec![
            Cell::Text(b"blob".to_vec()),
            Cell::Blob(REVIEW.to_vec())
        ]]
    );
    assert_eq!(
        raw_rows(
            &store.connection,
            "SELECT typeof(payload),payload FROM performance_source_actions"
        ),
        vec![vec![
            Cell::Text(b"blob".to_vec()),
            Cell::Blob(ACTION.to_vec())
        ]]
    );
    drop(store);
    let store = NativeStore::open_existing(&fixture.0).unwrap();
    let history: PerformanceSourceActionHistory = store.list_performance_source_actions().unwrap();
    assert_eq!(history.reviews.len(), 1);
    assert_eq!(history.revisions.len(), 1);
    assert_eq!(history.reviews[0].payload, REVIEW);
    assert_eq!(history.reviews[0].payload_sha256, sha(REVIEW));
    assert_eq!(history.revisions[0].payload, ACTION);
    assert_eq!(history.revisions[0].payload_sha256, sha(ACTION));
    assert_eq!(
        files(&fixture.0).keys().cloned().collect::<Vec<_>>(),
        vec![PathBuf::from("studio.sqlite3")],
        "ledger operations must not create source-action artifacts or capture stages"
    );
}

#[test]
fn exact_current_replay_precedes_cas_but_never_canonicalizes_or_rebinds_a_review() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let first = store
        .save_performance_source_action(&id(1), None, REVIEW, ACTION)
        .unwrap();
    for expected in [None, Some(0), Some(999), Some(u64::MAX)] {
        let before = snapshot(&store.connection, true);
        let replay = store
            .save_performance_source_action(&id(1), expected, REVIEW, ACTION)
            .unwrap();
        assert_eq!(replay.revision, first.revision);
        assert_eq!(replay.payload, ACTION);
        assert_eq!(snapshot(&store.connection, true), before);
        assert!(
            store
                .save_performance_source_action(&id(1), expected, SAME_REVIEW, ACTION)
                .is_err()
        );
        assert_eq!(snapshot(&store.connection, true), before);
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(ACTION).unwrap(),
        serde_json::from_slice::<serde_json::Value>(SAME_ACTION).unwrap()
    );
    assert!(matches!(
        store.save_performance_source_action(&id(1), None, REVIEW, SAME_ACTION),
        Err(StoreError::Conflict)
    ));
    let second = store
        .save_performance_source_action(&id(1), Some(1), REVIEW, SAME_ACTION)
        .unwrap();
    assert_eq!(
        second.revision, 2,
        "opaque storage compares exact bytes, not JSON value equality"
    );
    assert!(matches!(
        store.save_performance_source_action(&id(1), Some(1), REVIEW, ACTION),
        Err(StoreError::Conflict)
    ));
    let replay = store
        .save_performance_source_action(&id(1), None, REVIEW, SAME_ACTION)
        .unwrap();
    assert_eq!(replay.revision, 2);
    assert_eq!(
        store
            .list_performance_source_actions()
            .unwrap()
            .revisions
            .len(),
        2
    );
}

#[test]
fn input_byte_shape_and_identity_refusals_never_publish_an_orphan_review() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    for expectation in [Some(0), Some(1), Some(u64::MAX)] {
        let before = snapshot(&store.connection, true);
        assert!(matches!(
            store.save_performance_source_action(&id(1), expectation, REVIEW, ACTION),
            Err(StoreError::Conflict)
        ));
        assert_eq!(
            snapshot(&store.connection, true),
            before,
            "failed initial CAS left a review without an action"
        );
    }
    let bad_ids = [
        String::new(),
        "0".repeat(32),
        "A".repeat(32),
        "1".repeat(31),
        "1".repeat(33),
        "../escape".into(),
        "é".repeat(32),
    ];
    for bad in bad_ids {
        let before = snapshot(&store.connection, true);
        assert!(
            store
                .save_performance_source_action(&bad, None, REVIEW, ACTION)
                .is_err()
        );
        assert_eq!(snapshot(&store.connection, true), before);
    }
    let bad_json: &[&[u8]] = &[
        b"",
        b"[]",
        b"null",
        b"false",
        b"1",
        b"\"text\"",
        b"{",
        &[0xff],
        b"{\"x\":\"\0\"}",
    ];
    for bad in bad_json {
        for review_is_bad in [false, true] {
            let before = snapshot(&store.connection, true);
            let (review, action) = if review_is_bad {
                (*bad, ACTION)
            } else {
                (REVIEW, *bad)
            };
            assert!(
                store
                    .save_performance_source_action(&id(1), None, review, action)
                    .is_err()
            );
            assert_eq!(snapshot(&store.connection, true), before);
        }
    }
    for (review, action) in [
        (
            exact_object(REVIEW_BYTES + 1, "review-plus-one"),
            ACTION.to_vec(),
        ),
        (
            REVIEW.to_vec(),
            exact_object(ACTION_BYTES + 1, "action-plus-one"),
        ),
    ] {
        let before = snapshot(&store.connection, true);
        assert!(
            store
                .save_performance_source_action(&id(1), None, &review, &action)
                .is_err()
        );
        assert_eq!(snapshot(&store.connection, true), before);
    }
}

#[test]
fn review_and_action_update_delete_guards_preserve_every_exact_row() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    store
        .save_performance_source_action(&id(1), None, REVIEW, ACTION)
        .unwrap();
    for sql in [
        "UPDATE performance_source_reviews SET payload=payload",
        "DELETE FROM performance_source_reviews",
        "UPDATE performance_source_actions SET payload=payload",
        "DELETE FROM performance_source_actions",
    ] {
        let before = snapshot(&store.connection, true);
        assert!(
            store.connection.execute(sql, []).is_err(),
            "missing immutable guard: {sql}"
        );
        assert_eq!(snapshot(&store.connection, true), before);
    }
}

#[test]
fn failed_initial_inserts_and_trigger_postimages_roll_back_both_tables() {
    for trigger in [
        "CREATE TRIGGER review_refusal BEFORE INSERT ON performance_source_reviews BEGIN SELECT RAISE(ABORT,'synthetic review refusal'); END;",
        "CREATE TRIGGER action_refusal BEFORE INSERT ON performance_source_actions BEGIN SELECT RAISE(ABORT,'synthetic action refusal after review insertion'); END;",
        "CREATE TRIGGER surplus_review AFTER INSERT ON performance_source_actions BEGIN INSERT INTO performance_source_reviews(id,payload,payload_sha256) SELECT '0000000000000000000000000000000003',payload,payload_sha256 FROM performance_source_reviews WHERE id=NEW.id; END;",
        "CREATE TRIGGER surplus_revision AFTER INSERT ON performance_source_actions BEGIN INSERT INTO performance_source_actions(id,revision,review_blob_sha256,payload,payload_sha256) VALUES(NEW.id,NEW.revision+1,NEW.review_blob_sha256,NEW.payload,NEW.payload_sha256); END;",
        "DROP TRIGGER immutable_performance_source_action_update; CREATE TRIGGER changed_prior_row AFTER INSERT ON performance_source_actions BEGIN UPDATE performance_source_actions SET payload=NEW.payload,payload_sha256=NEW.payload_sha256 WHERE id='00000000000000000000000000000001'; END;",
    ] {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store
            .save_performance_source_action(&id(1), None, REVIEW, ACTION)
            .unwrap();
        store.connection.execute_batch(trigger).unwrap();
        let before = snapshot(&store.connection, true);
        let physical = files(&fixture.0);
        assert!(
            store
                .save_performance_source_action(&id(2), None, SAME_REVIEW, SAME_ACTION)
                .is_err(),
            "unexpected commit with {trigger}"
        );
        assert_eq!(snapshot(&store.connection, true), before);
        assert_eq!(
            files(&fixture.0),
            physical,
            "failed publication changed retained catalog bytes"
        );
        let history = store.list_performance_source_actions().unwrap();
        assert_eq!(history.reviews.len(), 1);
        assert_eq!(history.revisions.len(), 1);
        assert_eq!(history.reviews[0].id, id(1));
    }
}

#[test]
fn malformed_review_types_hashes_and_byte_bounds_block_listing_and_unrelated_save() {
    for mutation in [
        "UPDATE performance_source_reviews SET id=CAST(id AS BLOB)",
        "UPDATE performance_source_reviews SET id='00000000000000000000000000000000'",
        "UPDATE performance_source_reviews SET id='AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'",
        "UPDATE performance_source_reviews SET payload=CAST(payload AS TEXT)",
        "UPDATE performance_source_reviews SET payload=X'ff'",
        "UPDATE performance_source_reviews SET payload=X'5b5d'",
        "UPDATE performance_source_reviews SET payload=zeroblob(0)",
        "UPDATE performance_source_reviews SET payload=zeroblob(65537)",
        "UPDATE performance_source_reviews SET payload_sha256=CAST(payload_sha256 AS BLOB)",
        "UPDATE performance_source_reviews SET payload_sha256=upper(payload_sha256)",
        "UPDATE performance_source_reviews SET payload_sha256=substr(payload_sha256,1,63)",
        "UPDATE performance_source_reviews SET payload_sha256=printf('%064d',0)",
        "UPDATE performance_source_reviews SET payload_sha256=payload_sha256||'x'",
        "UPDATE performance_source_reviews SET id=NULL",
        "UPDATE performance_source_reviews SET payload=NULL",
        "UPDATE performance_source_reviews SET payload_sha256=NULL",
    ] {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store
            .save_performance_source_action(&id(1), None, REVIEW, ACTION)
            .unwrap();
        loosen_fixture(&store.connection);
        if mutation.contains("=NULL") {
            loose_table(&store.connection, "performance_source_reviews");
        }
        store.connection.execute_batch(mutation).unwrap();
        if mutation.contains("SET payload=") && !mutation.contains("=NULL") {
            // Isolate shape/type/size validation from a coincidentally stale
            // hash: malformed historical payloads have correct raw hashes.
            rehash_fixture_payloads(&store.connection, "performance_source_reviews");
        }
        refuse_unchanged(&mut store);
        let before = snapshot(&store.connection, true);
        assert!(
            store
                .save_performance_source_action(&id(1), None, REVIEW, ACTION)
                .is_err(),
            "current replay hid malformed review: {mutation}"
        );
        assert_eq!(snapshot(&store.connection, true), before);
    }
}

#[test]
fn malformed_action_types_hashes_references_and_revision_gaps_fail_closed() {
    for mutation in [
        "UPDATE performance_source_actions SET id=CAST(id AS BLOB) WHERE revision=1",
        "UPDATE performance_source_actions SET revision=0 WHERE revision=1",
        "UPDATE performance_source_actions SET revision=-1 WHERE revision=1",
        "UPDATE performance_source_actions SET revision=17 WHERE revision=1",
        "UPDATE performance_source_actions SET revision=1.5 WHERE revision=1",
        "UPDATE performance_source_actions SET revision='not-an-integer' WHERE revision=1",
        "UPDATE performance_source_actions SET revision=X'31' WHERE revision=1",
        "DELETE FROM performance_source_actions WHERE revision=1",
        "UPDATE performance_source_actions SET payload=CAST(payload AS TEXT) WHERE revision=1",
        "UPDATE performance_source_actions SET payload=X'ff' WHERE revision=1",
        "UPDATE performance_source_actions SET payload=X'6e756c6c' WHERE revision=1",
        "UPDATE performance_source_actions SET payload=zeroblob(16385) WHERE revision=1",
        "UPDATE performance_source_actions SET payload_sha256=CAST(payload_sha256 AS BLOB) WHERE revision=1",
        "UPDATE performance_source_actions SET payload_sha256=upper(payload_sha256) WHERE revision=1",
        "UPDATE performance_source_actions SET payload_sha256=printf('%064d',0) WHERE revision=1",
        "UPDATE performance_source_actions SET review_blob_sha256=CAST(review_blob_sha256 AS BLOB) WHERE revision=1",
        "UPDATE performance_source_actions SET review_blob_sha256=printf('%064d',0) WHERE revision=1",
        "UPDATE performance_source_actions SET id=NULL WHERE revision=1",
        "UPDATE performance_source_actions SET revision=NULL WHERE revision=1",
        "UPDATE performance_source_actions SET payload=NULL WHERE revision=1",
        "UPDATE performance_source_actions SET payload_sha256=NULL WHERE revision=1",
        "UPDATE performance_source_actions SET review_blob_sha256=NULL WHERE revision=1",
    ] {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store
            .save_performance_source_action(&id(1), None, REVIEW, ACTION)
            .unwrap();
        store
            .save_performance_source_action(&id(1), Some(1), REVIEW, SAME_ACTION)
            .unwrap();
        loosen_fixture(&store.connection);
        if mutation.contains("=NULL") {
            loose_table(&store.connection, "performance_source_actions");
        }
        store.connection.execute_batch(mutation).unwrap();
        if mutation.contains("SET payload=") && !mutation.contains("=NULL") {
            rehash_fixture_payloads(&store.connection, "performance_source_actions");
        }
        refuse_unchanged(&mut store);
        let before = snapshot(&store.connection, true);
        assert!(
            store
                .save_performance_source_action(&id(1), None, REVIEW, SAME_ACTION)
                .is_err(),
            "latest replay hid damaged historical action: {mutation}"
        );
        assert_eq!(snapshot(&store.connection, true), before);
    }
}

#[test]
fn missing_or_orphan_reviews_cross_identity_rebinding_and_surplus_rows_are_rejected() {
    for mutation in [
        "DELETE FROM performance_source_reviews WHERE id='00000000000000000000000000000001'",
        "DELETE FROM performance_source_actions WHERE id='00000000000000000000000000000001'",
        "UPDATE performance_source_actions SET id='00000000000000000000000000000002'",
        "INSERT INTO performance_source_reviews(id,payload,payload_sha256) SELECT '00000000000000000000000000000002',payload,payload_sha256 FROM performance_source_reviews",
    ] {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store
            .save_performance_source_action(&id(1), None, REVIEW, ACTION)
            .unwrap();
        loosen_fixture(&store.connection);
        store.connection.execute_batch(mutation).unwrap();
        refuse_unchanged(&mut store);
    }
    // The sentinel read must reject identity 33 even when all raw rows and
    // hashes are individually valid, rather than return a truncated prefix.
    let fixture = Fixture::new();
    let mut store = fixture.store();
    seed(&mut store.connection, IDENTITIES + 1, 1, false);
    refuse_unchanged(&mut store);
    // Isolate the 513th-action sentinel from the independent 33-review bound.
    // A deliberately loose synthetic table permits a duplicate 16th revision;
    // legitimate schema constraints still prevent this during normal writes.
    let fixture = Fixture::new();
    let mut store = fixture.store();
    seed(&mut store.connection, IDENTITIES, REVISIONS, false);
    loosen_fixture(&store.connection);
    store.connection.execute_batch("CREATE TABLE loose_actions AS SELECT * FROM performance_source_actions;
        DROP TABLE performance_source_actions;
        ALTER TABLE loose_actions RENAME TO performance_source_actions;
        INSERT INTO performance_source_actions SELECT * FROM performance_source_actions WHERE id='00000000000000000000000000000020' AND revision=16;").unwrap();
    refuse_unchanged(&mut store);
    let fixture = Fixture::new();
    let mut store = fixture.store();
    store
        .save_performance_source_action(&id(1), None, REVIEW, ACTION)
        .unwrap();
    loosen_fixture(&store.connection);
    loose_table(&store.connection, "performance_source_reviews");
    store
        .connection
        .execute_batch(
            "INSERT INTO performance_source_reviews SELECT * FROM performance_source_reviews",
        )
        .unwrap();
    refuse_unchanged(&mut store);
}

#[test]
fn full_opaque_capacity_preserves_ten_mib_exact_bytes_and_replays_without_eviction() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    // Raw-store capacity only: these 512 JSON objects are not a claim that the
    // Prepared-only session protocol currently permits 16 typed transitions.
    seed(&mut store.connection, IDENTITIES, REVISIONS, true);
    drop(store);
    let mut store = NativeStore::open_existing(&fixture.0).unwrap();
    let history = store.list_performance_source_actions().unwrap();
    assert_eq!(
        history.reviews.len(),
        IDENTITIES as usize,
        "each review must be returned once"
    );
    assert_eq!(history.revisions.len(), (IDENTITIES * REVISIONS) as usize);
    let total = history
        .reviews
        .iter()
        .map(|row| row.payload.len())
        .sum::<usize>()
        + history
            .revisions
            .iter()
            .map(|row| row.payload.len())
            .sum::<usize>();
    assert_eq!(total, 10_485_760);
    for (index, review) in history.reviews.iter().enumerate() {
        let identity = index as u64 + 1;
        let expected = exact_object(REVIEW_BYTES, &format!("review-{identity}"));
        assert_eq!(review.id, id(identity));
        assert_eq!(review.payload, expected);
        assert_eq!(review.payload_sha256, sha(&expected));
    }
    for (index, row) in history.revisions.iter().enumerate() {
        let identity = index as u64 / REVISIONS + 1;
        let revision = index as u64 % REVISIONS + 1;
        let expected = exact_object(
            ACTION_BYTES,
            &format!("action-{identity}-revision-{revision}"),
        );
        assert_eq!((&row.id, row.revision), (&id(identity), revision));
        assert_eq!(row.payload, expected);
        assert_eq!(row.payload_sha256, sha(&expected));
        assert_eq!(
            row.review_blob_sha256,
            history.reviews[(identity - 1) as usize].payload_sha256
        );
    }
    let review = exact_object(REVIEW_BYTES, "review-32");
    let payload = exact_object(ACTION_BYTES, "action-32-revision-16");
    let before = snapshot(&store.connection, true);
    for expectation in [None, Some(1), Some(u64::MAX)] {
        let replay = store
            .save_performance_source_action(&id(32), expectation, &review, &payload)
            .unwrap();
        assert_eq!(replay.revision, REVISIONS);
        assert_eq!(replay.payload, payload);
    }
    assert!(
        store
            .save_performance_source_action(&id(33), None, REVIEW, ACTION)
            .is_err()
    );
    assert!(
        store
            .save_performance_source_action(&id(32), Some(16), &review, ACTION)
            .is_err()
    );
    assert!(
        store
            .save_performance_source_action(&id(32), Some(15), &review, ACTION)
            .is_err()
    );
    assert_eq!(snapshot(&store.connection, true), before);
}

#[test]
fn two_writer_initial_replay_cas_and_last_slot_races_preserve_one_exact_result() {
    for scenario in [
        "exact-initial",
        "conflicting-review",
        "same-cas",
        "last-slot",
    ] {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        if scenario == "same-cas" {
            store
                .save_performance_source_action(&id(1), None, REVIEW, ACTION)
                .unwrap();
        } else if scenario == "last-slot" {
            seed(&mut store.connection, IDENTITIES - 1, 1, false);
        }
        drop(store);
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|choice| {
                let root = fixture.0.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut store = NativeStore::open_existing(root).unwrap();
                    let identity = if scenario == "last-slot" {
                        id(32 + choice)
                    } else {
                        id(1)
                    };
                    let expected = (scenario == "same-cas").then_some(1);
                    let review = if scenario == "conflicting-review" && choice == 1 {
                        SAME_REVIEW
                    } else {
                        REVIEW
                    };
                    let payload = if scenario == "same-cas" {
                        format!("{{\"opaque_choice\":{choice}}}").into_bytes()
                    } else {
                        ACTION.to_vec()
                    };
                    barrier.wait();
                    store.save_performance_source_action(&identity, expected, review, &payload)
                })
            })
            .collect();
        let outcomes: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        let successes = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
        assert_eq!(
            successes,
            if scenario == "exact-initial" { 2 } else { 1 },
            "{scenario}: {outcomes:?}"
        );
        if scenario == "same-cas" {
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|outcome| matches!(outcome, Err(StoreError::Conflict)))
                    .count(),
                1
            );
        }
        let store = NativeStore::open_existing(&fixture.0).unwrap();
        let history = store.list_performance_source_actions().unwrap();
        let expected_reviews = if scenario == "last-slot" {
            IDENTITIES as usize
        } else {
            1
        };
        assert_eq!(history.reviews.len(), expected_reviews);
        assert_eq!(
            history.revisions.len(),
            if scenario == "same-cas" {
                2
            } else {
                expected_reviews
            }
        );
        for outcome in outcomes.into_iter().filter_map(std::result::Result::ok) {
            let stored = history
                .revisions
                .iter()
                .find(|row| row.id == outcome.id && row.revision == outcome.revision)
                .unwrap();
            assert_eq!(stored, &outcome);
            let review = history
                .reviews
                .iter()
                .find(|row| row.id == outcome.id)
                .unwrap();
            assert_eq!(review.payload_sha256, outcome.review_blob_sha256);
        }
    }
}

#[test]
fn validated_history_fence_precedes_writes_and_replay_and_serializes_different_ids() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let empty = store.list_performance_source_actions().unwrap();
    let first = store
        .save_performance_source_action_if_history_matches(&id(1), None, REVIEW, ACTION, &empty)
        .unwrap();
    let after_first = snapshot(&store.connection, true);
    // Even CURRENT exact-byte replay cannot bypass a stale validated history.
    for (identity, payload) in [(id(1), ACTION), (id(2), SAME_ACTION)] {
        assert!(matches!(
            store.save_performance_source_action_if_history_matches(
                &identity, None, REVIEW, payload, &empty
            ),
            Err(StoreError::Conflict)
        ));
        assert_eq!(snapshot(&store.connection, true), after_first);
    }
    let validated = store.list_performance_source_actions().unwrap();
    let replay = store
        .save_performance_source_action_if_history_matches(
            &id(1),
            Some(u64::MAX),
            REVIEW,
            ACTION,
            &validated,
        )
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(snapshot(&store.connection, true), after_first);
    // A write under another identity invalidates the typed caller's observed
    // namespace, even though the chosen action's own CAS/review are unchanged.
    let mut other = NativeStore::open_existing(&fixture.0).unwrap();
    other
        .save_performance_source_action(&id(3), None, SAME_REVIEW, SAME_ACTION)
        .unwrap();
    drop(other);
    let after_unrelated = snapshot(&store.connection, true);
    for identity in [id(1), id(2)] {
        assert!(matches!(
            store.save_performance_source_action_if_history_matches(
                &identity, None, REVIEW, ACTION, &validated
            ),
            Err(StoreError::Conflict)
        ));
        assert_eq!(snapshot(&store.connection, true), after_unrelated);
    }
    let fresh = store.list_performance_source_actions().unwrap();
    let admitted = store
        .save_performance_source_action_if_history_matches(&id(2), None, REVIEW, ACTION, &fresh)
        .unwrap();
    assert_eq!(admitted.revision, 1);
    assert_eq!(admitted.payload, ACTION);
    assert_eq!(
        store
            .list_performance_source_actions()
            .unwrap()
            .reviews
            .len(),
        3
    );

    let fixture = Fixture::new();
    let mut store = fixture.store();
    store
        .save_performance_source_action(&id(1), None, REVIEW, ACTION)
        .unwrap();
    let base = store.list_performance_source_actions().unwrap();
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (2..=3)
        .map(|identity| {
            let root = fixture.0.clone();
            let barrier = barrier.clone();
            let expected = base.clone();
            std::thread::spawn(move || {
                let mut store = NativeStore::open_existing(root).unwrap();
                // The store does not interpret recovery_op. This raw fixture
                // checks the serialization fence used by the separate typed
                // uniqueness validator; it grants no recovery authority.
                let payload =
                    b"{\"phase\":\"Prepared\",\"recovery_op\":\"same-synthetic-operation\"}";
                barrier.wait();
                store.save_performance_source_action_if_history_matches(
                    &id(identity),
                    None,
                    REVIEW,
                    payload,
                    &expected,
                )
            })
        })
        .collect();
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Err(StoreError::Conflict)))
            .count(),
        1
    );
    let store = NativeStore::open_existing(&fixture.0).unwrap();
    let history = store.list_performance_source_actions().unwrap();
    assert_eq!(
        history.reviews.len(),
        2,
        "losing initial save left an orphan review"
    );
    assert_eq!(history.revisions.len(), 2);
    assert_eq!(
        history.reviews.iter().find(|row| row.id == id(1)).unwrap(),
        &base.reviews[0]
    );
    assert_eq!(
        history
            .revisions
            .iter()
            .find(|row| row.id == id(1))
            .unwrap(),
        &base.revisions[0]
    );
    let winner = outcomes
        .into_iter()
        .find_map(std::result::Result::ok)
        .unwrap();
    assert_eq!(
        history
            .revisions
            .iter()
            .find(|row| row.id == winner.id)
            .unwrap(),
        &winner
    );
    assert_eq!(
        history
            .reviews
            .iter()
            .find(|row| row.id == winner.id)
            .unwrap()
            .payload,
        REVIEW
    );
}

fn version_five(fixture: &Fixture) -> (String, String) {
    let mut store = fixture.store();
    let first = store
        .create(&omniusong_core::ProjectDocument::new("Synthetic prior musician work").unwrap())
        .unwrap();
    let renamed = store
        .apply_edit(&omniusong_core::contracts::ProjectEditIntent {
            operation_id: omniusong_core::contracts::OperationId("synthetic-prior-rename".into()),
            project_id: omniusong_core::contracts::ProjectId(first.id().into()),
            expected_revision: first.revision(),
            scope: omniusong_core::contracts::EditScope::Project,
            edit: omniusong_core::contracts::EditOperation::Rename {
                name: "Retained prior edit".into(),
            },
            extensions: omniusong_core::contracts::Extensions::new(),
        })
        .unwrap()
        .document;
    let mut edit = renamed.clone();
    edit.set_tempo(117.25).unwrap();
    let second = store.save(&edit, renamed.revision()).unwrap();
    let undone = store.undo(first.id(), second.revision()).unwrap();
    let project = first.id().to_owned();
    let name = undone.name().to_owned();
    store.save_performance_intent(&id(1), None, REVIEW).unwrap();
    store
        .save_performance_intent(&id(1), Some(1), SAME_REVIEW)
        .unwrap();
    let imported = first.to_json().unwrap();
    store
        .connection
        .execute(
            "INSERT INTO imports(project_id,source_sha256,original_json) VALUES(?1,?2,?3)",
            params![project, sha(&imported), imported],
        )
        .unwrap();
    store.connection.execute_batch("CREATE TABLE opaque_prior_metadata(key TEXT, value);
        INSERT INTO opaque_prior_metadata VALUES('null',NULL),('integer',9223372036854775807),('real',1.25),('text','retained'||char(0)||'text'),('blob',X'00ff817f');").unwrap();
    let prior_asset = b"synthetic immutable prior asset";
    store
        .connection
        .execute(
            "INSERT INTO assets(id,payload) VALUES(?1,?2)",
            params![sha(prior_asset), prior_asset.as_slice()],
        )
        .unwrap();
    // Convert only this freshly created fixture to schema5, preserving every
    // schema5 row, trigger, index and opaque column/type before migration.
    store
        .connection
        .execute_batch(
            "DROP TRIGGER immutable_performance_source_action_update;
        DROP TRIGGER immutable_performance_source_action_delete;
        DROP TRIGGER immutable_performance_source_review_update;
        DROP TRIGGER immutable_performance_source_review_delete;
        DROP TABLE performance_source_actions;
        DROP TABLE performance_source_reviews;
        PRAGMA user_version=5;",
        )
        .unwrap();
    drop(store);
    (project, name)
}

#[test]
fn populated_version_five_migration_preserves_exact_old_rows_schema_and_history() {
    let fixture = Fixture::new();
    let (project, name) = version_five(&fixture);
    fs::write(
        fixture.0.join("unrelated-prototype.bytes"),
        b"exact unrelated synthetic bytes\0\xff",
    )
    .unwrap();
    let db = Connection::open(fixture.0.join("studio.sqlite3")).unwrap();
    let before = snapshot(&db, false);
    drop(db);
    let physical = files(&fixture.0);
    assert!(
        NativeReader::open(&fixture.0).is_err(),
        "a reader must not migrate schema5"
    );
    assert_eq!(files(&fixture.0), physical);
    let mut store = NativeStore::open_existing(&fixture.0).unwrap();
    assert_eq!(STORE_SCHEMA_VERSION, 6);
    let mut after = snapshot(&store.connection, false);
    assert_eq!(after.version, 6);
    after.version = before.version;
    assert_eq!(after, before);
    assert_eq!(store.load(&project).unwrap().name(), name);
    let history = store.list_performance_intents().unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].payload, REVIEW);
    assert_eq!(history[1].payload, SAME_REVIEW);
    assert!(
        store
            .list_performance_source_actions()
            .unwrap()
            .reviews
            .is_empty()
    );
    assert_eq!(
        fs::read(fixture.0.join("unrelated-prototype.bytes")).unwrap(),
        b"exact unrelated synthetic bytes\0\xff"
    );
    let current = store.load(&project).unwrap();
    assert_eq!(
        store.redo(&project, current.revision()).unwrap().name(),
        "Retained prior edit"
    );
    drop(store);
    assert!(NativeReader::open(&fixture.0).is_ok());
    assert!(NativeStore::open_existing(&fixture.0).is_ok());
}

#[test]
fn migration_collisions_roll_back_every_statement_and_future_versions_refuse_unchanged() {
    for collision in [
        "CREATE TABLE performance_source_actions(marker BLOB); INSERT INTO performance_source_actions VALUES(X'00ff');",
        "CREATE TRIGGER immutable_performance_source_action_delete BEFORE DELETE ON opaque_prior_metadata BEGIN SELECT RAISE(ABORT,'preexisting schema object'); END;",
    ] {
        let fixture = Fixture::new();
        version_five(&fixture);
        let db = Connection::open(fixture.0.join("studio.sqlite3")).unwrap();
        db.execute_batch(collision).unwrap();
        let before = snapshot(&db, true);
        drop(db);
        let physical = files(&fixture.0);
        assert!(
            NativeStore::open_existing(&fixture.0).is_err(),
            "collision must refuse: {collision}"
        );
        assert_eq!(
            files(&fixture.0),
            physical,
            "failed schema6 migration changed exact catalog bytes"
        );
        let db = Connection::open(fixture.0.join("studio.sqlite3")).unwrap();
        assert_eq!(snapshot(&db, true), before);
        assert_eq!(before.version, 5);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='performance_source_reviews'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "earlier CREATE TABLE must roll back"
        );
    }
    let fixture = Fixture::new();
    let store = fixture.store();
    store
        .connection
        .pragma_update(None, "user_version", STORE_SCHEMA_VERSION + 1)
        .unwrap();
    let before = snapshot(&store.connection, true);
    drop(store);
    let physical = files(&fixture.0);
    assert!(NativeStore::open_existing(&fixture.0).is_err());
    assert!(NativeReader::open(&fixture.0).is_err());
    assert_eq!(files(&fixture.0), physical);
    let db = Connection::open(fixture.0.join("studio.sqlite3")).unwrap();
    assert_eq!(snapshot(&db, true), before);
}
