//! Independent migration/archive boundaries for local recording intentions.
use omniusong_core::ProjectDocument;
use omniusong_store::{NativeReader, NativeStore, STORE_SCHEMA_VERSION};
use rusqlite::{Connection, types::Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-intent-independent")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn snapshot(path: &Path) -> BTreeMap<String, Vec<Vec<Value>>> {
    let db = Connection::open(path.join("studio.sqlite3")).unwrap();
    let names: Vec<String> = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name!='recording_intents' ORDER BY name").unwrap()
        .query_map([], |r| r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let sql = format!(
                "SELECT * FROM \"{}\" ORDER BY rowid",
                name.replace('"', "\"\"")
            );
            let mut statement = db.prepare(&sql).unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|i| row.get::<_, Value>(i))
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}
#[test]
fn version_four_upgrade_keeps_every_prior_table_and_reopens_read_only() {
    let f = Fixture::new();
    let path = f.0.join("studio");
    let mut store = NativeStore::open(&path).unwrap();
    let doc = store
        .create(&ProjectDocument::new("Prior musician work").unwrap())
        .unwrap();
    let mut changed = doc.clone();
    changed.rename("Retained edit").unwrap();
    changed.set_tempo(117.25).unwrap();
    let saved = store.save(&changed, doc.revision()).unwrap();
    let undone = store.undo(doc.id(), saved.revision()).unwrap();
    drop(store);
    let before = snapshot(&path);
    let db = Connection::open(path.join("studio.sqlite3")).unwrap();
    // This synthetic downgrade must remove every newer local ledger, otherwise
    // the fixture falsely collides with the schema-6 migration it is testing.
    db.execute_batch("DROP TABLE performance_source_actions; DROP TABLE performance_source_reviews; DROP TRIGGER immutable_recording_intent_update; DROP TRIGGER immutable_recording_intent_delete; DROP TABLE recording_intents; PRAGMA user_version=4;").unwrap();
    drop(db);
    assert!(
        NativeReader::open(&path).is_err(),
        "read-only paths must not migrate older catalogs"
    );
    let mut reopened = NativeStore::open_existing(&path).unwrap();
    assert_eq!(reopened.load(doc.id()).unwrap(), undone);
    assert_eq!(
        snapshot(&path),
        before,
        "migration must preserve all ten prior tables exactly"
    );
    let db = Connection::open(path.join("studio.sqlite3")).unwrap();
    assert_eq!(
        db.pragma_query_value::<u32, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        STORE_SCHEMA_VERSION
    );
    drop(db);
    assert!(NativeReader::open(&path).is_ok());
    assert_eq!(
        reopened.redo(doc.id(), undone.revision()).unwrap().name(),
        "Retained edit"
    );
    assert!(reopened.list_performance_intents().unwrap().is_empty());
}
#[test]
fn local_unassigned_intentions_are_retained_but_do_not_enter_song_archives() {
    let f = Fixture::new();
    let mut source = NativeStore::open(f.0.join("source")).unwrap();
    let doc = source
        .create(&ProjectDocument::new("Portable song").unwrap())
        .unwrap();
    let id = "0123456789abcdef0123456789abcdef";
    let intent = source
        .save_performance_intent(id, None, br#"{"version":1,"localOnly":"unassigned stage"}"#)
        .unwrap();
    let archive = f.0.join("song.osar");
    source.export_archive(doc.id(), &archive).unwrap();
    let mut restored = NativeStore::open(f.0.join("restored")).unwrap();
    assert_eq!(restored.restore_archive(&archive).unwrap(), doc);
    assert!(restored.list_performance_intents().unwrap().is_empty());
    assert_eq!(source.list_performance_intents().unwrap(), vec![intent]);
    assert_eq!(source.load(doc.id()).unwrap(), doc);
}
