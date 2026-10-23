//! Disposable, trigger-free catalog-shaped rows derived only from acquired
//! input and declared copy identities. This is not a NativeStore or authority.
use crate::{
    Result,
    archive::{self, Bundle},
    archive_boundary as boundary,
};
use rusqlite::{Connection, OpenFlags, params};
use std::{fs::OpenOptions, path::Path};

pub(crate) const REUSABLE_TABLES: &[&str] = &[
    "assets",
    "native_takes",
    "archive_copy_proofs",
    "archive_copy_proof_operations",
];

/// Fixed application-owned definitions; never execute a catalog's SQL text.
pub(crate) fn open_catalog(stage: &Path) -> Result<Connection> {
    open_catalog_in(stage, None)
}
pub(crate) fn open_catalog_bounded(
    stage: &Path,
    budget: &omniusong_recording::InspectionBudget,
) -> Result<Connection> {
    open_catalog_in(stage, Some(budget))
}
fn open_catalog_in(
    stage: &Path,
    budget: Option<&omniusong_recording::InspectionBudget>,
) -> Result<Connection> {
    let path = stage.join("planned-catalog.sqlite3");
    let reservation = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    drop(reservation);
    boundary::output((|| -> Result<_> {
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // This disposable database is never reopened as a completed plan after
        // failure. The main-file bound does not cover filesystem/temp overhead.
        connection.execute_batch(
            "PRAGMA page_size=4096; PRAGMA max_page_count=131072;
            PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-2048;
            PRAGMA temp_store=FILE; PRAGMA mmap_size=0; PRAGMA trusted_schema=OFF;",
        )?;
        let mut progress = budget
            .map(|budget| {
                crate::archive_governed::PrivateSql::new(
                    &connection,
                    budget,
                    crate::ArchiveSqlOrigin::PrivateOutput,
                )
            })
            .transpose()?;
        connection.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY NOT NULL,payload TEXT NOT NULL,revision INTEGER NOT NULL,cursor INTEGER NOT NULL);
            CREATE TABLE edits(project_id TEXT NOT NULL,sequence INTEGER NOT NULL,before_json TEXT NOT NULL,after_json TEXT NOT NULL,PRIMARY KEY(project_id,sequence));
            CREATE TABLE operations(operation_id TEXT PRIMARY KEY NOT NULL,project_id TEXT NOT NULL,request_json TEXT NOT NULL,request_sha256 TEXT NOT NULL,previous_revision INTEGER NOT NULL,committed_revision INTEGER NOT NULL,result_json TEXT NOT NULL,result_sha256 TEXT NOT NULL);
            CREATE TABLE imports(project_id TEXT PRIMARY KEY NOT NULL,source_sha256 TEXT NOT NULL,original_json BLOB NOT NULL);
            CREATE TABLE assets(id TEXT PRIMARY KEY NOT NULL,payload BLOB NOT NULL);
            CREATE TABLE native_takes(take_id TEXT PRIMARY KEY NOT NULL,manifest_sha256 TEXT NOT NULL,object_sha256 TEXT NOT NULL,byte_length INTEGER NOT NULL,manifest_json TEXT NOT NULL);
            CREATE TABLE recording_intents(id TEXT NOT NULL,revision INTEGER NOT NULL,payload BLOB NOT NULL,payload_sha256 TEXT NOT NULL,PRIMARY KEY(id,revision));
            CREATE TABLE performance_source_reviews(id TEXT PRIMARY KEY NOT NULL,payload BLOB NOT NULL,payload_sha256 TEXT NOT NULL,UNIQUE(id,payload_sha256));
            CREATE TABLE performance_source_actions(id TEXT NOT NULL,revision INTEGER NOT NULL,review_blob_sha256 TEXT NOT NULL,payload BLOB NOT NULL,payload_sha256 TEXT NOT NULL,PRIMARY KEY(id,revision));")?;
        for (_, sql) in crate::archive_copy::LINEAGE_TABLES {
            connection.execute_batch(sql)?;
        }
        if let Some(progress) = &mut progress {
            progress.detach()?;
        }
        drop(progress);
        Ok(connection)
    })())
}

/// Plan shared registrations/assets before any destination mutation. No owned
/// pathname is installed or authenticated by this metadata-only operation.
pub(crate) fn plan_objects(input: &Connection, bundle: &Bundle, plan: &Connection) -> Result<()> {
    for id in &bundle.assets {
        let bytes = boundary::input(crate::read_asset(input, id))?;
        boundary::output((|| -> Result<()> {
            plan.execute("INSERT INTO assets VALUES(?1,?2)", params![id, bytes])?;
            Ok(())
        })())?;
    }
    for (id, _) in archive::take_ids(bundle)? {
        let (object, json) = boundary::input(archive::take_metadata(input, &id))?;
        boundary::output(archive::register_take(plan, &object, &json))?;
    }
    Ok(())
}
