//! Independent native project storage. The prototype's files are read-only input.
//! Project publication, original import receipt and content assets commit together.

mod archive;
mod archive_copy;
mod captures;
mod evidence_archive;
mod evidence_failure;
mod operations;
mod performance_attachment_check;
mod performance_intents;
mod performance_source_actions;
mod projection_boundary;
mod reader;
mod recovered_takes;
mod retained_file_coordination;
mod source_attachment;
mod takes;
pub use reader::NativeReader;
#[cfg(test)]
mod reader_tests;
pub use archive::{
    ARCHIVE_APPLICATION_ID, ARCHIVE_CHUNK_BYTES, ARCHIVE_METADATA_BYTES, ARCHIVE_PAYLOAD_BYTES,
    ARCHIVE_VERSION, ArchiveReceipt,
};
pub use archive_copy::{
    ArchiveInspection, MAX_LINEAGE_MAPPINGS, MAX_LINEAGE_PROOFS, RestoreCopyIntent,
    RestoreCopyReceipt,
};
pub use captures::{
    CaptureAudio, CaptureInspection, CaptureRecoveryReceipt, CaptureStage, CaptureStageScan,
    CaptureStageState, MAX_CAPTURE_STAGES,
};
pub use evidence_archive::{
    EVIDENCE_SCOPE, EvidenceAbsence, EvidenceArchiveInspection, EvidenceArchiveReceipt,
    EvidenceEntry, EvidenceEntryKind, EvidenceManifest, EvidenceRestoreReceipt, EvidenceRoot,
    MAX_EVIDENCE_ENTRIES, MAX_EVIDENCE_MANIFEST_BYTES, inspect_evidence_archive,
    inspect_restored_evidence, reconcile_evidence_export,
};
pub use evidence_failure::{EvidenceFailure, EvidenceFailureOrigin};
pub use operations::EditReceipt;
pub use performance_intents::StoredPerformanceIntent;
pub use performance_source_actions::{
    MAX_PERFORMANCE_SOURCE_ACTION_BYTES, MAX_PERFORMANCE_SOURCE_ACTION_REVISIONS,
    MAX_PERFORMANCE_SOURCE_ACTIONS, MAX_PERFORMANCE_SOURCE_REVIEW_BYTES,
    PerformanceSourceActionHistory, StoredPerformanceSourceAction, StoredPerformanceSourceReview,
};
pub use source_attachment::{SOURCE_ATTACHMENT_OPERATION_PREFIX, SourceAttachmentCommit};
pub use takes::{TakeObject, TakePublication};

use omniusong_core::native::MAX_NATIVE_PROJECT_BYTES;
use omniusong_core::{DocumentError, MAX_PROJECT_BYTES, ProjectDocument, valid_asset_id};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fmt, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_ASSET_BYTES: usize = 30 * 1024 * 1024;
const MAX_TOTAL_ASSET_BYTES: usize = 48 * 1024 * 1024;
pub const STORE_SCHEMA_VERSION: u32 = 6;

#[derive(Debug)]
pub enum StoreError {
    Document(DocumentError),
    /// A pure musical mutation, or an explicitly requested audio window outside
    /// an authenticated current source prefix, rejected before any edit writes.
    /// Never use this for damaged reads, replay verification or commit errors.
    EditRejected(DocumentError),
    Database(rusqlite::Error),
    Io(io::Error),
    Recording(omniusong_recording::RecordingError),
    Conflict,
    Invalid(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Document(e) | Self::EditRejected(e) => e.fmt(f),
            Self::Database(e) => e.fmt(f),
            Self::Io(e) => e.fmt(f),
            Self::Recording(e) => e.fmt(f),
            Self::Conflict => f.write_str("Project changed. Reopen it before saving."),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}
impl Error for StoreError {}
impl From<DocumentError> for StoreError {
    fn from(e: DocumentError) -> Self {
        Self::Document(e)
    }
}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}
impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<omniusong_recording::RecordingError> for StoreError {
    fn from(error: omniusong_recording::RecordingError) -> Self {
        Self::Recording(error)
    }
}
type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub track_count: usize,
    pub note_count: usize,
    pub problem: Option<String>,
}

pub struct NativeStore {
    connection: Connection,
    root: PathBuf,
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(StoreError::Invalid(format!(
            "{} exceeds the import size limit",
            path.display()
        )));
    }
    Ok(bytes)
}

fn check_asset(id: &str, bytes: &[u8]) -> Result<()> {
    if !valid_asset_id(id)
        || bytes.is_empty()
        || bytes.len() > MAX_ASSET_BYTES
        || checksum(bytes) != id[..64]
    {
        return Err(StoreError::Invalid(format!(
            "Audio asset is missing, damaged or invalid: {id}"
        )));
    }
    Ok(())
}

fn load_document(connection: &Connection, id: &str) -> Result<ProjectDocument> {
    let (payload, revision): (Option<String>, i64) = connection.query_row(
        "SELECT CASE WHEN octet_length(payload)<=?2 THEN payload END,revision FROM projects WHERE id=?1",
        params![id, MAX_NATIVE_PROJECT_BYTES as i64],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let payload = payload.ok_or_else(|| {
        StoreError::Invalid("Stored project exceeds its document size limit".into())
    })?;
    let document = ProjectDocument::from_json(payload.as_bytes())?;
    if document.id() != id || Some(document.revision()) != u64::try_from(revision).ok() {
        return Err(StoreError::Invalid(
            "Stored project identity/revision is inconsistent".into(),
        ));
    }
    projection_boundary::reject_unverified_projection(&document)?;
    Ok(document)
}

fn check_references(connection: &Connection, project: &ProjectDocument) -> Result<()> {
    projection_boundary::reject_unverified_projection(project)?;
    for id in project.referenced_assets()? {
        read_asset(connection, &id)?;
    }
    takes::check_take_references(connection, project)?;
    Ok(())
}

fn read_asset(connection: &Connection, id: &str) -> Result<Vec<u8>> {
    // octet_length uses SQLite's stored byte length; an oversized BLOB is not
    // materialized into a Rust Vec merely to discover that it is too large.
    let row: Option<Option<Vec<u8>>> = connection
        .query_row(
            "SELECT CASE WHEN octet_length(payload)<=?2 THEN payload END FROM assets WHERE id=?1",
            params![id, MAX_ASSET_BYTES as i64],
            |row| row.get(0),
        )
        .optional()?;
    let bytes = row.flatten().ok_or_else(|| {
        StoreError::Invalid(format!(
            "Project audio is missing or exceeds the schema-1 size limit: {id}"
        ))
    })?;
    check_asset(id, &bytes)?;
    Ok(bytes)
}

fn insert_document(connection: &Connection, document: &ProjectDocument) -> Result<()> {
    if document.revision() != 0 {
        return Err(StoreError::Invalid(
            "New native projects must start at revision zero".into(),
        ));
    }
    check_references(connection, document)?;
    let payload = String::from_utf8(document.to_json()?).expect("JSON is UTF-8");
    connection.execute(
        "INSERT INTO projects(id, payload, revision, cursor) VALUES(?1,?2,0,0)",
        params![document.id(), payload],
    )?;
    Ok(())
}

// These helpers share the caller's transaction with an operation receipt. They
// deliberately do not commit independently or acknowledge work before commit.
fn save_in_transaction(
    connection: &Connection,
    root: &Path,
    document: &ProjectDocument,
    expected_revision: u64,
) -> Result<ProjectDocument> {
    save_in_transaction_with_budget(connection, root, document, expected_revision, None)
}

fn save_in_transaction_with_budget(
    connection: &Connection,
    root: &Path,
    document: &ProjectDocument,
    expected_revision: u64,
    budget: Option<&omniusong_recording::InspectionBudget>,
) -> Result<ProjectDocument> {
    if document.revision() != expected_revision {
        return Err(StoreError::Conflict);
    }
    let current = load_document(connection, document.id())?;
    if current.revision() != expected_revision {
        return Err(StoreError::Conflict);
    }
    projection_boundary::reject_opaque_promotion(&current, document)?;
    check_references(connection, document)?;
    if let Some(budget) = budget {
        takes::verify_document_files_with_budget(root, connection, document, budget)?;
    } else {
        takes::verify_document_files(root, connection, document)?;
    }
    let cursor: i64 = connection.query_row(
        "SELECT cursor FROM projects WHERE id=?1",
        [document.id()],
        |row| row.get(0),
    )?;
    let next_cursor = cursor
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid("Stored edit cursor is out of range".into()))?;
    let next = document.with_revision(
        expected_revision
            .checked_add(1)
            .ok_or_else(|| StoreError::Invalid("Project revision is out of range".into()))?,
    )?;
    let before = String::from_utf8(current.to_json()?).expect("JSON is UTF-8");
    let after = String::from_utf8(next.to_json()?).expect("JSON is UTF-8");
    connection.execute(
        "DELETE FROM edits WHERE project_id=?1 AND sequence>?2",
        params![document.id(), cursor],
    )?;
    connection.execute(
        "INSERT INTO edits(project_id,sequence,before_json,after_json) VALUES(?1,?2,?3,?4)",
        params![document.id(), next_cursor, before, after],
    )?;
    connection.execute(
        "UPDATE projects SET payload=?1,revision=?2,cursor=?3 WHERE id=?4",
        params![after, next.revision() as i64, next_cursor, document.id()],
    )?;
    Ok(next)
}

fn navigate_in_transaction(
    connection: &Connection,
    root: &Path,
    id: &str,
    expected_revision: u64,
    redo: bool,
) -> Result<ProjectDocument> {
    let current = load_document(connection, id)?;
    if current.revision() != expected_revision {
        return Err(StoreError::Conflict);
    }
    let cursor: i64 =
        connection.query_row("SELECT cursor FROM projects WHERE id=?1", [id], |row| {
            row.get(0)
        })?;
    let target = if redo {
        cursor
            .checked_add(1)
            .ok_or_else(|| StoreError::Invalid("Stored edit cursor is out of range".into()))?
    } else {
        cursor
    };
    let snapshot: Option<(Option<String>, Option<String>)> = connection.query_row(
        "SELECT CASE WHEN octet_length(before_json)<=?3 THEN before_json END,CASE WHEN octet_length(after_json)<=?3 THEN after_json END FROM edits WHERE project_id=?1 AND sequence=?2",
        params![id,target,MAX_NATIVE_PROJECT_BYTES as i64], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let snapshot = snapshot.ok_or_else(|| {
        StoreError::Invalid(
            if redo {
                "Nothing to redo"
            } else {
                "Nothing to undo"
            }
            .into(),
        )
    })?;
    let (Some(before), Some(after)) = snapshot else {
        return Err(StoreError::Invalid(
            "Stored history exceeds its document size limit".into(),
        ));
    };
    let before = ProjectDocument::from_json(before.as_bytes())?;
    let after = ProjectDocument::from_json(after.as_bytes())?;
    // Validate the recorded forward operation, even during reverse navigation.
    // Checking current -> target would incorrectly reject Undo of a legitimate
    // downgrade that preserved an older schema's opaque field.
    projection_boundary::reject_unverified_projection(&before)?;
    projection_boundary::reject_unverified_projection(&after)?;
    projection_boundary::reject_opaque_promotion(&before, &after)?;
    if before.id() != id
        || after.id() != id
        || before.revision().checked_add(1) != Some(after.revision())
    {
        return Err(StoreError::Invalid(
            "Stored edit history identity/revision pair is inconsistent".into(),
        ));
    }
    let next = if redo { after } else { before }.with_revision(
        expected_revision
            .checked_add(1)
            .ok_or_else(|| StoreError::Invalid("Project revision is out of range".into()))?,
    )?;
    if next.id() != id {
        return Err(StoreError::Invalid(
            "Stored edit history belongs to a different project".into(),
        ));
    }
    check_references(connection, &next)?;
    takes::verify_document_files(root, connection, &next)?;
    let next_cursor = if redo {
        target
    } else {
        cursor
            .checked_sub(1)
            .filter(|n| *n >= 0)
            .ok_or_else(|| StoreError::Invalid("Stored edit cursor is out of range".into()))?
    };
    connection.execute(
        "UPDATE projects SET payload=?1,revision=?2,cursor=?3 WHERE id=?4",
        params![
            String::from_utf8(next.to_json()?).expect("JSON is UTF-8"),
            next.revision() as i64,
            next_cursor,
            id
        ],
    )?;
    Ok(next)
}

impl NativeStore {
    /// Canonical owned namespace captured when this catalog was opened.
    /// Callers must not infer that a recording stage already exists from it.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Existence only. A true result must be followed by authenticated open;
    /// callers must never treat a damaged registered original as absent.
    pub fn has_registered_take(&self, take_id: &omniusong_core::contracts::TakeId) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_takes WHERE take_id=?1)",
                [&take_id.0],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_mode(root.as_ref(), true)
    }

    /// Reopen known storage without creating directories or a replacement catalog.
    /// READ_WRITE without CREATE also protects the check/open race on a missing file.
    pub fn open_existing(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_mode(root.as_ref(), false)
    }

    fn open_mode(root: &Path, create: bool) -> Result<Self> {
        if create {
            fs::create_dir_all(root)?;
        }
        let canonical_root = fs::canonicalize(root)?;
        let _writer = crate::retained_file_coordination::writer(&canonical_root)?;
        let root = canonical_root.as_path();
        let mut connection = if create {
            Connection::open(root.join("studio.sqlite3"))?
        } else {
            Connection::open_with_flags(
                root.join("studio.sqlite3"),
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?
        };
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if !create && version == 0 {
            return Err(StoreError::Invalid(
                "Expected an initialized native store; no replacement was created".into(),
            ));
        }
        if version > STORE_SCHEMA_VERSION {
            return Err(StoreError::Invalid(
                "This native store was created by a newer version; it was not migrated".into(),
            ));
        }
        if version < STORE_SCHEMA_VERSION {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Another process may have initialized the store while this writer
            // waited. Re-read the schema under the same migration lock.
            let mut locked_version: u32 =
                tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if !create && locked_version == 0 {
                return Err(StoreError::Invalid(
                    "Expected an initialized native store; no replacement was created".into(),
                ));
            }
            if locked_version == 0 {
                tx.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY NOT NULL, payload TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>=0), cursor INTEGER NOT NULL CHECK(cursor>=0));
                CREATE TABLE edits(project_id TEXT NOT NULL REFERENCES projects(id), sequence INTEGER NOT NULL, before_json TEXT NOT NULL, after_json TEXT NOT NULL, PRIMARY KEY(project_id,sequence));
                CREATE TABLE assets(id TEXT PRIMARY KEY NOT NULL, payload BLOB NOT NULL);
                CREATE TABLE imports(project_id TEXT PRIMARY KEY NOT NULL REFERENCES projects(id), source_sha256 TEXT NOT NULL, original_json BLOB NOT NULL);
                PRAGMA user_version=1;")?;
                locked_version = 1;
            }
            if locked_version == 1 {
                tx.execute_batch(
                    "CREATE TABLE operations(
                    operation_id TEXT PRIMARY KEY NOT NULL,
                    project_id TEXT NOT NULL REFERENCES projects(id),
                    request_json TEXT NOT NULL,
                    request_sha256 TEXT NOT NULL,
                    previous_revision INTEGER NOT NULL CHECK(previous_revision>=0),
                    committed_revision INTEGER NOT NULL CHECK(committed_revision>previous_revision),
                    result_json TEXT NOT NULL,
                    result_sha256 TEXT NOT NULL
                ); PRAGMA user_version=2;",
                )?;
                locked_version = 2;
            }
            if locked_version == 2 {
                tx.execute_batch("CREATE TABLE native_takes(
                    take_id TEXT PRIMARY KEY NOT NULL,
                    manifest_sha256 TEXT NOT NULL,
                    object_sha256 TEXT NOT NULL,
                    byte_length INTEGER NOT NULL CHECK(byte_length>0),
                    manifest_json TEXT NOT NULL
                );
                CREATE TRIGGER immutable_take_registration_update BEFORE UPDATE ON native_takes BEGIN SELECT RAISE(ABORT,'Registered originals are immutable'); END;
                CREATE TRIGGER immutable_take_registration_delete BEFORE DELETE ON native_takes BEGIN SELECT RAISE(ABORT,'Registered originals are retained'); END;
                PRAGMA user_version=3;")?;
                locked_version = 3;
            }
            if locked_version == 3 {
                // The same explicit table definitions are used by OSAR v2.
                // Ancestral proofs are historical content, while archive_copies
                // admits only live request IDs into the global operation namespace.
                for (_, sql) in archive_copy::LINEAGE_TABLES {
                    tx.execute_batch(sql)?;
                }
                for table in [
                    "archive_copy_proofs",
                    "archive_copy_proof_operations",
                    "archive_project_lineage",
                    "archive_copies",
                ] {
                    for action in ["UPDATE", "DELETE"] {
                        tx.execute_batch(&format!("CREATE TRIGGER immutable_{table}_{} BEFORE {action} ON {table} BEGIN SELECT RAISE(ABORT,'Archive copy lineage is retained and immutable'); END;", action.to_lowercase()))?;
                    }
                }
                tx.execute_batch("CREATE TRIGGER operation_id_not_copy_insert BEFORE INSERT ON operations WHEN EXISTS(SELECT 1 FROM archive_copies WHERE operation_id=NEW.operation_id) BEGIN SELECT RAISE(ABORT,'Operation identity already belongs to an archive copy'); END;
                    CREATE TRIGGER operation_id_not_copy_update BEFORE UPDATE OF operation_id ON operations WHEN EXISTS(SELECT 1 FROM archive_copies WHERE operation_id=NEW.operation_id) BEGIN SELECT RAISE(ABORT,'Operation identity already belongs to an archive copy'); END;
                    CREATE TRIGGER copy_id_not_operation_insert BEFORE INSERT ON archive_copies WHEN EXISTS(SELECT 1 FROM operations WHERE operation_id=NEW.operation_id) BEGIN SELECT RAISE(ABORT,'Operation identity already belongs to a project edit'); END;
                    CREATE TRIGGER copy_id_not_operation_update BEFORE UPDATE OF operation_id ON archive_copies WHEN EXISTS(SELECT 1 FROM operations WHERE operation_id=NEW.operation_id) BEGIN SELECT RAISE(ABORT,'Operation identity already belongs to a project edit'); END;
                    PRAGMA user_version=4;")?;
                locked_version = 4;
            }
            if locked_version == 4 {
                // Local recording intentions are retained separately from immutable
                // take originals and portable song/archive content. Every revision
                // is appended; no previous request can be rewritten or removed.
                tx.execute_batch("CREATE TABLE recording_intents(
                    id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 16),
                    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 16384),
                    payload_sha256 TEXT NOT NULL,
                    PRIMARY KEY(id,revision)
                );
                CREATE TRIGGER immutable_recording_intent_update BEFORE UPDATE ON recording_intents BEGIN SELECT RAISE(ABORT,'Recording intentions are immutable'); END;
                CREATE TRIGGER immutable_recording_intent_delete BEFORE DELETE ON recording_intents BEGIN SELECT RAISE(ABORT,'Recording intentions are retained'); END;
                PRAGMA user_version=5;")?;
                locked_version = 5;
            }
            if locked_version == 5 {
                // Local source choices retain exact review bytes separately from
                // compact revisions. No original or portable song table changes.
                tx.execute_batch("CREATE TABLE performance_source_reviews(
                    id TEXT PRIMARY KEY NOT NULL,
                    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 65536),
                    payload_sha256 TEXT NOT NULL,
                    UNIQUE(id,payload_sha256)
                );
                CREATE TABLE performance_source_actions(
                    id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 16),
                    review_blob_sha256 TEXT NOT NULL,
                    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 16384),
                    payload_sha256 TEXT NOT NULL,
                    PRIMARY KEY(id,revision),
                    FOREIGN KEY(id,review_blob_sha256) REFERENCES performance_source_reviews(id,payload_sha256)
                );
                CREATE TRIGGER immutable_performance_source_review_update BEFORE UPDATE ON performance_source_reviews BEGIN SELECT RAISE(ABORT,'Performance source reviews are immutable'); END;
                CREATE TRIGGER immutable_performance_source_review_delete BEFORE DELETE ON performance_source_reviews BEGIN SELECT RAISE(ABORT,'Performance source reviews are retained'); END;
                CREATE TRIGGER immutable_performance_source_action_update BEFORE UPDATE ON performance_source_actions BEGIN SELECT RAISE(ABORT,'Performance source actions are immutable'); END;
                CREATE TRIGGER immutable_performance_source_action_delete BEFORE DELETE ON performance_source_actions BEGIN SELECT RAISE(ABORT,'Performance source actions are retained'); END;
                PRAGMA user_version=6;")?;
                locked_version = 6;
            }
            if locked_version != STORE_SCHEMA_VERSION {
                return Err(StoreError::Invalid(
                    "Native store schema changed during initialization".into(),
                ));
            }
            tx.commit()?;
        }
        Ok(Self {
            connection,
            root: fs::canonicalize(root)?,
        })
    }

    pub fn list(&self) -> Result<Vec<ProjectSummary>> {
        let mut statement = self
            .connection
            .prepare("SELECT id,CASE WHEN octet_length(payload)<=?1 THEN payload END,revision FROM projects ORDER BY rowid DESC")?;
        let rows = statement.query_map([MAX_NATIVE_PROJECT_BYTES as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, payload, revision) = row?;
            let parsed = payload
                .ok_or_else(|| {
                    DocumentError("Stored project exceeds its document size limit".into())
                })
                .and_then(|payload| ProjectDocument::from_json(payload.as_bytes()))
                .and_then(|p| {
                    if p.id() == id && Some(p.revision()) == u64::try_from(revision).ok() {
                        projection_boundary::reject_unverified_projection(&p)
                            .map_err(|error| DocumentError(error.to_string()))?;
                        Ok(p)
                    } else {
                        Err(DocumentError(
                            "Stored project identity/revision is inconsistent".into(),
                        ))
                    }
                });
            Ok(match parsed {
                Ok(project) => ProjectSummary {
                    id,
                    name: project.name().into(),
                    revision: project.revision(),
                    track_count: project.tracks().len(),
                    note_count: project
                        .tracks()
                        .iter()
                        .filter(|track| track["kind"] != "recording")
                        .map(|t| t["notes"].as_array().map_or(0, Vec::len))
                        .sum(),
                    problem: None,
                },
                Err(error) => ProjectSummary {
                    id,
                    name: "Damaged session · recovery needed".into(),
                    revision: u64::try_from(revision).unwrap_or(0),
                    track_count: 0,
                    note_count: 0,
                    problem: Some(error.to_string()),
                },
            })
        })
        .collect()
    }

    /// Metadata capability admission only. This opens no take file and grants
    /// no source-authentication, revision, replay or publication authority.
    /// Mutation paths must repeat their complete checks at commit.
    pub fn check_project_metadata_admission(&self, id: &str) -> Result<()> {
        load_document(&self.connection, id).map(|_| ())
    }

    pub fn load(&self, id: &str) -> Result<ProjectDocument> {
        let document = load_document(&self.connection, id)?;
        check_references(&self.connection, &document)?;
        takes::verify_document_files(&self.root, &self.connection, &document)?;
        Ok(document)
    }

    pub fn asset(&self, id: &str) -> Result<Vec<u8>> {
        if !valid_asset_id(id) {
            return Err(StoreError::Invalid("Invalid asset identity".into()));
        }
        read_asset(&self.connection, id)
    }

    pub fn create(&mut self, document: &ProjectDocument) -> Result<ProjectDocument> {
        projection_boundary::reject_unverified_projection(document)?;
        takes::verify_document_files(&self.root, &self.connection, document)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_document(&tx, document)?;
        tx.commit()?;
        Ok(document.clone())
    }

    pub fn import_legacy(
        &mut self,
        project_path: &Path,
        assets_dir: &Path,
    ) -> Result<ProjectDocument> {
        let raw = read_bounded(project_path, MAX_PROJECT_BYTES)?;
        let source = ProjectDocument::from_json(&raw)?;
        if source.schema_version() != 1 {
            return Err(StoreError::Invalid(
                "Native projects require a complete native archive, not legacy JSON import".into(),
            ));
        }
        let mut total = 0;
        let mut assets = Vec::new();
        for id in source.referenced_assets()? {
            let bytes = read_bounded(&assets_dir.join(&id), MAX_ASSET_BYTES)?;
            check_asset(&id, &bytes)?;
            total += bytes.len();
            if total > MAX_TOTAL_ASSET_BYTES {
                return Err(StoreError::Invalid(
                    "Project exceeds schema-1 total audio size".into(),
                ));
            }
            assets.push((id, bytes));
        }
        let source_hash = checksum(&raw);
        let imported = source.imported_copy(&source_hash)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (id, bytes) in assets {
            tx.execute("INSERT INTO assets(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![id,bytes])?;
        }
        insert_document(&tx, &imported)?;
        tx.execute(
            "INSERT INTO imports(project_id,source_sha256,original_json) VALUES(?1,?2,?3)",
            params![imported.id(), source_hash, raw],
        )?;
        tx.commit()?;
        Ok(imported)
    }

    /// Reads the exact original JSON bytes retained at migration, not a re-export.
    pub fn imported_original(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let result: Option<(Option<String>, Option<Vec<u8>>)> = self
            .connection
            .query_row(
                "SELECT CASE WHEN octet_length(source_sha256)=64 THEN source_sha256 END,CASE WHEN octet_length(original_json)<=?2 THEN original_json END FROM imports WHERE project_id=?1",
                params![id,MAX_PROJECT_BYTES as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match result {
            Some((Some(expected), Some(bytes))) if checksum(&bytes) == expected => Ok(Some(bytes)),
            Some(_) => Err(StoreError::Invalid(
                "Original migration receipt is damaged".into(),
            )),
            None => Ok(None),
        }
    }

    pub fn save(
        &mut self,
        document: &ProjectDocument,
        expected_revision: u64,
    ) -> Result<ProjectDocument> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next = save_in_transaction(&tx, &self.root, document, expected_revision)?;
        tx.commit()?;
        Ok(next)
    }

    pub fn undo(&mut self, id: &str, expected_revision: u64) -> Result<ProjectDocument> {
        self.navigate_history(id, expected_revision, false)
    }
    pub fn redo(&mut self, id: &str, expected_revision: u64) -> Result<ProjectDocument> {
        self.navigate_history(id, expected_revision, true)
    }

    fn navigate_history(
        &mut self,
        id: &str,
        expected_revision: u64,
        redo: bool,
    ) -> Result<ProjectDocument> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next = navigate_in_transaction(&tx, &self.root, id, expected_revision, redo)?;
        tx.commit()?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    struct TestRoot(PathBuf);
    impl TestRoot {
        fn new() -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/native-tests")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn native_edits_undo_redo_survive_reopen_and_reject_stale_writes() {
        let root = TestRoot::new();
        let mut store = NativeStore::open(&root.0).unwrap();
        let project = store
            .create(&ProjectDocument::new("Human original").unwrap())
            .unwrap();
        let mut edited = project.clone();
        edited.rename("Native revision").unwrap();
        edited.set_tempo(133.5).unwrap();
        let saved = store.save(&edited, 0).unwrap();
        assert!(matches!(store.save(&project, 0), Err(StoreError::Conflict)));
        drop(store);
        let mut store = NativeStore::open(&root.0).unwrap();
        assert_eq!(store.load(saved.id()).unwrap(), saved);
        let undone = store.undo(saved.id(), 1).unwrap();
        assert_eq!(undone.name(), project.name());
        assert_eq!(undone.tracks(), project.tracks());
        assert_eq!(undone.revision(), 2);
        drop(store);
        let mut store = NativeStore::open(&root.0).unwrap();
        let redone = store.redo(saved.id(), 2).unwrap();
        assert_eq!(redone.name(), saved.name());
        assert_eq!(redone.tempo(), 133.5);
        assert_eq!(redone.revision(), 3);
        assert!(store.redo(saved.id(), 3).is_err());
        store.undo(saved.id(), 3).unwrap();
        let mut replacement = store.load(saved.id()).unwrap();
        replacement.rename("New branch").unwrap();
        let replacement = store.save(&replacement, 4).unwrap();
        assert!(store.redo(saved.id(), replacement.revision()).is_err());
    }

    #[test]
    fn two_connections_cannot_publish_the_same_expected_revision() {
        let root = TestRoot::new();
        let mut first = NativeStore::open(&root.0).unwrap();
        let mut second = NativeStore::open(&root.0).unwrap();
        let project = first
            .create(&ProjectDocument::new("Shared").unwrap())
            .unwrap();
        let mut a = first.load(project.id()).unwrap();
        let mut b = second.load(project.id()).unwrap();
        a.rename("First writer").unwrap();
        b.rename("Stale writer").unwrap();
        first.save(&a, 0).unwrap();
        assert!(matches!(second.save(&b, 0), Err(StoreError::Conflict)));
        assert_eq!(second.load(project.id()).unwrap().name(), "First writer");
    }

    #[test]
    fn legacy_import_is_complete_and_retains_exact_original_bytes() {
        let root = TestRoot::new();
        let assets = root.0.join("source-assets");
        fs::create_dir_all(&assets).unwrap();
        let source_bytes = b"original immutable source audio";
        let asset_id = format!("{}.wav", checksum(source_bytes));
        fs::write(assets.join(&asset_id), source_bytes).unwrap();
        let mut value = ProjectDocument::new("Legacy song").unwrap().value().clone();
        value["tracks"][0]["sourceAssetId"] = asset_id.clone().into();
        value["custom:unknown"] =
            json!({"nativeClock":"18446744073709551615","resolution":4294967295u32});
        let original = ProjectDocument::from_value(value).unwrap();
        let raw = original.to_json().unwrap();
        let source = root.0.join("original.json");
        fs::write(&source, &raw).unwrap();
        let mut store = NativeStore::open(root.0.join("native")).unwrap();
        let imported = store.import_legacy(&source, &assets).unwrap();
        assert_ne!(imported.id(), original.id());
        assert_eq!(imported.tracks(), original.tracks());
        assert_eq!(
            imported.value()["custom:unknown"],
            original.value()["custom:unknown"]
        );
        assert_eq!(
            store.imported_original(imported.id()).unwrap().unwrap(),
            raw
        );
        assert_eq!(fs::read(&source).unwrap(), raw);
        assert_eq!(fs::read(assets.join(&asset_id)).unwrap(), source_bytes);
        fs::remove_dir_all(&assets).unwrap();
        assert_eq!(store.load(imported.id()).unwrap(), imported);
    }

    #[test]
    fn damaged_or_missing_audio_never_publishes_a_partial_import() {
        let root = TestRoot::new();
        let assets = root.0.join("assets");
        fs::create_dir_all(&assets).unwrap();
        let asset_id = format!("{}.wav", checksum(b"correct"));
        let mut value = ProjectDocument::new("Missing source")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["sourceAssetId"] = asset_id.clone().into();
        let source = root.0.join("source.json");
        fs::write(&source, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut store = NativeStore::open(root.0.join("native")).unwrap();
        assert!(store.import_legacy(&source, &assets).is_err());
        fs::write(assets.join(&asset_id), b"damaged").unwrap();
        assert!(store.import_legacy(&source, &assets).is_err());
        assert!(store.list().unwrap().is_empty());
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM assets", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM imports", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn aborted_transaction_retains_last_committed_project_after_reopen() {
        let root = TestRoot::new();
        let mut store = NativeStore::open(&root.0).unwrap();
        let original = store
            .create(&ProjectDocument::new("Committed").unwrap())
            .unwrap();
        {
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            tx.execute(
                "UPDATE projects SET payload='damaged uncommitted write' WHERE id=?1",
                [original.id()],
            )
            .unwrap();
        }
        drop(store);
        let store = NativeStore::open(&root.0).unwrap();
        assert_eq!(store.load(original.id()).unwrap(), original);
    }

    #[test]
    fn damaged_history_cannot_replace_the_target_project_identity() {
        let root = TestRoot::new();
        let mut store = NativeStore::open(&root.0).unwrap();
        let original = store
            .create(&ProjectDocument::new("Original").unwrap())
            .unwrap();
        let mut edited = original.clone();
        edited.rename("Committed edit").unwrap();
        let saved = store.save(&edited, 0).unwrap();
        let unrelated = ProjectDocument::new("Unrelated history").unwrap();
        store
            .connection
            .execute(
                "UPDATE edits SET before_json=?1 WHERE project_id=?2",
                params![
                    String::from_utf8(unrelated.to_json().unwrap()).unwrap(),
                    original.id()
                ],
            )
            .unwrap();
        assert!(store.undo(saved.id(), 1).is_err());
        assert_eq!(store.load(saved.id()).unwrap(), saved);
    }

    #[test]
    fn one_damaged_session_does_not_hide_healthy_sessions() {
        let root = TestRoot::new();
        let mut store = NativeStore::open(&root.0).unwrap();
        let good = store
            .create(&ProjectDocument::new("Healthy song").unwrap())
            .unwrap();
        let bad = store
            .create(&ProjectDocument::new("Damaged song").unwrap())
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE projects SET payload='damaged' WHERE id=?1",
                [bad.id()],
            )
            .unwrap();
        let summaries = store.list().unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(
            summaries
                .iter()
                .find(|p| p.id == good.id())
                .unwrap()
                .problem
                .is_none()
        );
        assert!(
            summaries
                .iter()
                .find(|p| p.id == bad.id())
                .unwrap()
                .problem
                .is_some()
        );
        assert_eq!(store.load(good.id()).unwrap(), good);
        assert!(store.load(bad.id()).is_err());
    }

    #[test]
    fn simultaneous_first_open_does_not_race_schema_creation() {
        let root = TestRoot::new();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|index| {
                let path = root.0.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut store = NativeStore::open(path).unwrap();
                    store
                        .create(&ProjectDocument::new(&format!("Writer {index}")).unwrap())
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(NativeStore::open(&root.0).unwrap().list().unwrap().len(), 4);
    }
}
