use omniusong_core::ProjectDocument;
use omniusong_store::NativeStore;
use rusqlite::Connection;
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/rust-migration/existing-open-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Existing-open fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn existing_only_open_never_creates_a_missing_directory_or_database() {
    let fixture = Fixture::new();
    let missing = fixture.0.join("removed-volume").join("store");
    assert!(NativeStore::open_existing(&missing).is_err());
    assert!(!fixture.0.join("removed-volume").exists());
    assert!(NativeStore::open_existing(&fixture.0).is_err());
    assert!(!fixture.0.join("studio.sqlite3").exists());
    // An explicit first-run open still provisions storage.
    assert!(NativeStore::open(&missing).is_ok());
    assert!(missing.join("studio.sqlite3").exists());
}

#[test]
fn existing_only_rejects_empty_or_uninitialized_files_without_publishing_schema() {
    let fixture = Fixture::new();
    let database = fixture.0.join("studio.sqlite3");
    fs::write(&database, []).unwrap();
    assert!(NativeStore::open_existing(&fixture.0).is_err());
    assert!(fs::read(&database).unwrap().is_empty());
    let connection = Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TABLE foreign_content(value TEXT); INSERT INTO foreign_content VALUES('preserve');").unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();
    assert!(NativeStore::open_existing(&fixture.0).is_err());
    assert_eq!(fs::read(&database).unwrap(), before);
    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='projects'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn existing_catalog_reopens_with_exact_project_and_history() {
    let fixture = Fixture::new();
    let mut original = NativeStore::open(&fixture.0).unwrap();
    let mut document = original
        .create(&ProjectDocument::new("Existing music").unwrap())
        .unwrap();
    document.rename("Preserved saved name").unwrap();
    let saved = original.save(&document, 0).unwrap();
    drop(original);
    let mut reopened = NativeStore::open_existing(&fixture.0).unwrap();
    assert_eq!(reopened.load(saved.id()).unwrap(), saved);
    assert_eq!(
        reopened.undo(saved.id(), saved.revision()).unwrap().name(),
        "Existing music"
    );
}
