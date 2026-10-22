//! Exclusive, consistent, read-only review of a retained original. No takeover.
use crate::{
    InspectionBudget,
    journal::{self, Journal},
    types::*,
};
use rusqlite::{Connection, OpenFlags, Statement, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
mod copy_recovery;
pub use copy_recovery::{
    COPY_RECOVERY_MAX_ARTIFACT_BYTES, COPY_RECOVERY_MAX_WORKING_BYTES, CopyRecoveryBoundary,
    CopyRecoveryError, CopyRecoveryReceipt, RecoveryCopy, RecoveryCopyPaths,
};

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalInspection {
    pub path: PathBuf,
    pub descriptor: TakeDescriptor,
    pub descriptor_sha256: String,
    pub journal_sha256: String,
    pub original_rows_sha256: String,
    pub byte_length: u64,
    pub report: RecoveryReport,
    pub manifest: Option<SealedManifest>,
}
/// The read transaction and existing owner guard remain held until drop or
/// checked conversion to the recovery writer. This type never resumes capture.
pub struct CaptureReader {
    connection: Connection,
    owner: File,
    inspection: JournalInspection,
    budget: Option<InspectionBudget>,
    anchors: Option<copy_recovery::SourceAnchors>,
}
/// Consistent read-only access under an existing journal owner. This cannot be
/// converted to a recovery writer and never releases/reacquires the owner lock.
pub struct OwnedJournalView<'a> {
    transaction: rusqlite::Transaction<'a>,
    inspection: JournalInspection,
}
impl<'a> OwnedJournalView<'a> {
    pub(crate) fn new(
        connection: &'a Connection,
        path: &Path,
        descriptor: &TakeDescriptor,
    ) -> Result<Self> {
        let transaction = connection.unchecked_transaction()?;
        require(
            journal::load_descriptor(&transaction)? == *descriptor,
            "Original descriptor changed after journal acquisition",
        )?;
        let inspection = inspect_connection(&transaction, path, descriptor)?;
        Ok(Self {
            transaction,
            inspection,
        })
    }
    pub fn inspection(&self) -> &JournalInspection {
        &self.inspection
    }
    pub fn read_ordinal(&self, ordinal: u64) -> Result<ChunkRecord> {
        read_ordinal(&self.transaction, &self.inspection, ordinal)
    }
}
fn read_ordinal(
    connection: &Connection,
    inspection: &JournalInspection,
    ordinal: u64,
) -> Result<ChunkRecord> {
    read_ordinal_bounded(connection, inspection, ordinal, None)
}
fn read_ordinal_bounded(
    connection: &Connection,
    inspection: &JournalInspection,
    ordinal: u64,
    budget: Option<&InspectionBudget>,
) -> Result<ChunkRecord> {
    crate::budget::checkpoint(budget)?;
    require(
        ordinal > 0 && ordinal <= inspection.report.verified_chunks,
        "Capture ordinal lies outside the held verified prefix",
    )?;
    crate::budget::rows(budget, 3)?;
    crate::budget::work(budget, 105)?;
    let (stream, sequence): (String, u64) = connection.query_row(
        "SELECT substr(stream_id,1,97),sequence FROM chunks WHERE ordinal=?1",
        [journal::sql_integer(ordinal)?],
        |row| Ok((row.get(0)?, journal::read_unsigned(row, 1)?)),
    )?;
    require(
        stream.len() <= 96,
        "Stored capture stream identity exceeds its bound",
    )?;
    let result = journal::load_chunk_bounded(
        connection,
        &inspection.descriptor,
        &stream,
        sequence,
        budget,
    );
    crate::budget::finish(budget, result)
}
fn ordinary(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    require(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Capture inspection requires an ordinary non-symlink file",
    )?;
    journal::reject_hardlinks(path)
}
fn existing_owner(path: &Path) -> Result<File> {
    let mut name = path.as_os_str().to_owned();
    name.push(".owner.lock");
    let path = PathBuf::from(name);
    ordinary(&path)?;
    let file = OpenOptions::new().read(true).write(true).open(&path)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => RecordingError::Owned,
        std::fs::TryLockError::Error(e) => RecordingError::Io(e),
    })?;
    ordinary(&path)?;
    Ok(file)
}
/// Bounded-path policy is intentionally stricter than legacy sealed aliases.
pub(crate) fn preflight_path(path: &Path, budget: &InspectionBudget) -> Result<PathBuf> {
    budget.checkpoint()?;
    ordinary(path)?;
    budget.file(fs::metadata(path)?.len())?;
    let path = fs::canonicalize(path)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        budget.checkpoint()?;
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        require(
            fs::symlink_metadata(PathBuf::from(name))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "Bounded inspection refuses SQLite sidecars; no repair was attempted",
        )?;
    }
    budget.charge_work(100)?;
    let mut header = [0u8; 100];
    File::open(&path)?.read_exact(&mut header)?;
    require(
        &header[..16] == b"SQLite format 3\0" && header[18] == 1 && header[19] == 1,
        "Bounded inspection requires rollback-mode SQLite",
    )?;
    budget.checkpoint()?;
    Ok(path)
}
const TABLES: [&str; 8] = [
    "sqlite_schema",
    "take_meta",
    "stream_prefix",
    "chunks",
    "interruptions",
    "recovery_index",
    "sealed_manifests",
    "sqlite_sequence",
];
pub(crate) fn preflight_connection(
    connection: &Connection,
    budget: &InspectionBudget,
) -> Result<()> {
    // Read bounded identifiers before using any application table or payload.
    let mut statement=connection.prepare("SELECT substr(type,1,17),length(CAST(type AS BLOB)),substr(name,1,257),length(CAST(name AS BLOB)),length(CAST(sql AS BLOB)) FROM sqlite_schema ORDER BY rowid")?;
    let mut rows = statement.query([])?;
    let mut tables = std::collections::BTreeSet::new();
    while let Some(row) = rows.next()? {
        budget.physical_row()?;
        budget.visit_rows(5)?;
        budget.charge_work(306)?;
        let kind: String = row.get(0)?;
        let kind_len: i64 = row.get(1)?;
        let name: String = row.get(2)?;
        let name_len: i64 = row.get(3)?;
        let sql_len: Option<i64> = row.get(4)?;
        require(
            (1..=16).contains(&kind_len) && (1..=256).contains(&name_len),
            "Inspection schema identity is oversized",
        )?;
        require(
            matches!(kind.as_str(), "table" | "index" | "trigger"),
            "Views or unknown schema objects cannot be inspected",
        )?;
        require(
            sql_len.is_none_or(|len| (0..=MAX_METADATA_BYTES as i64).contains(&len)),
            "Inspection schema SQL is oversized",
        )?;
        if kind == "table" {
            require(
                TABLES.contains(&name.as_str()) && name != "sqlite_schema",
                "Unexpected source table",
            )?;
            tables.insert(name);
        }
    }
    drop(rows);
    drop(statement);
    require(
        TABLES[1..].iter().all(|table| tables.contains(*table)),
        "Missing expected physical source table",
    )?;
    for table in TABLES {
        budget.visit_rows(3)?;
        budget.charge_work(24)?;
        let (kind, without_rowid): (String, i64) = connection.query_row(
            "SELECT type,wr FROM pragma_table_list WHERE schema='main' AND name=?1",
            [table],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        require(
            kind == "table" && without_rowid == 0,
            "Virtual, shadow or non-rowid source table is unsupported",
        )?;
        let mut columns=connection.prepare("SELECT substr(name,1,257),length(CAST(name AS BLOB)),hidden FROM pragma_table_xinfo(?1) ORDER BY cid")?;
        let mut fields = columns.query([table])?;
        let mut names = std::collections::BTreeSet::new();
        while let Some(row) = fields.next()? {
            budget.visit_rows(4)?;
            budget.charge_work(273)?;
            let length: i64 = row.get(1)?;
            let hidden: i64 = row.get(2)?;
            require(
                (1..=256).contains(&length) && names.len() < 128 && hidden == 0,
                "Generated, hidden or oversized source columns are unsupported",
            )?;
            let name: String = row.get(0)?;
            require(
                !["rowid", "_rowid_", "oid"]
                    .iter()
                    .any(|reserved| name.eq_ignore_ascii_case(reserved)),
                "Source column shadows SQLite row identity",
            )?;
            names.insert(name.to_ascii_lowercase());
        }
        let required: &[&str] = match table {
            "sqlite_schema" => &["type", "name", "tbl_name", "rootpage", "sql"],
            "take_meta" => &["singleton", "descriptor", "descriptor_sha256", "state"],
            "stream_prefix" => &[
                "stream_id",
                "next_sequence",
                "source_end",
                "recorded_units",
                "gap_units",
                "gap_chunks",
            ],
            "chunks" => &[
                "ordinal",
                "stream_id",
                "sequence",
                "header",
                "payload",
                "sha256",
            ],
            "interruptions" => &["singleton", "receipt", "sha256"],
            "recovery_index" => &["ordinal", "sha256"],
            "sealed_manifests" => &["singleton", "operation_id", "body", "sha256"],
            "sqlite_sequence" => &["name", "seq"],
            _ => unreachable!(),
        };
        require(
            required.iter().all(|name| names.contains(*name)),
            "Required source columns are missing",
        )?;
        drop(fields);
        drop(columns);
        if table == "sqlite_schema" {
            continue;
        }
        // No count(*) or payload decoding before every physical row is bounded.
        let mut physical =
            connection.prepare(&format!("SELECT rowid FROM {table} ORDER BY rowid"))?;
        let mut rows = physical.query([])?;
        while let Some(row) = rows.next()? {
            budget.physical_row()?;
            budget.visit_rows(2)?;
            budget.charge_work(8)?;
            let _: i64 = row.get(0)?;
        }
    }
    budget.checkpoint()
}
fn physical_hash_bounded(path: &Path, budget: Option<&InspectionBudget>) -> Result<(u64, String)> {
    crate::budget::checkpoint(budget)?;
    ordinary(path)?;
    let mut file = File::open(path)?;
    let before = file.metadata()?.len();
    if let Some(b) = budget {
        b.file(before)?;
    }
    let mut count = 0u64;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        crate::budget::checkpoint(budget)?;
        let capacity = if budget.is_some() {
            (before.saturating_sub(count)).min(buffer.len() as u64) as usize
        } else {
            buffer.len()
        };
        if capacity == 0 {
            break;
        }
        if budget.is_some() {
            crate::budget::work(budget, (capacity as u64).saturating_mul(2))?;
        }
        let n = file.read(&mut buffer[..capacity])?;
        if n == 0 {
            break;
        }
        count = count
            .checked_add(n as u64)
            .ok_or_else(|| RecordingError::Invalid("Capture byte length overflow".into()))?;
        hash.update(&buffer[..n]);
    }
    require(
        count == before && file.metadata()?.len() == before,
        "Capture file changed while hashing",
    )?;
    Ok((count, format!("{:x}", hash.finalize())))
}
struct OriginalFieldQueries<'connection> {
    metadata: Statement<'connection>,
    scalar: Statement<'connection>,
    bytes: Statement<'connection>,
}
impl<'connection> OriginalFieldQueries<'connection> {
    fn new(
        connection: &'connection Connection,
        table: &str,
        column: &str,
        budget: Option<&InspectionBudget>,
    ) -> Result<Self> {
        // Table names are compile-time fixed; column names are bounded metadata
        // identifiers, quoted once per hash rather than once per original row.
        let column = format!("\"{}\"", column.replace('"', "\"\""));
        crate::budget::checkpoint(budget)?;
        let metadata = connection.prepare(&format!(
            "SELECT typeof({column}),length(CAST({column} AS BLOB)) FROM {table} WHERE rowid=?1"
        ))?;
        crate::budget::checkpoint(budget)?;
        let scalar = connection.prepare(&format!("SELECT {column} FROM {table} WHERE rowid=?1"))?;
        crate::budget::checkpoint(budget)?;
        let bytes = connection.prepare(&format!(
            "SELECT substr(CAST({column} AS BLOB),?2,?3) FROM {table} WHERE rowid=?1"
        ))?;
        Ok(Self {
            metadata,
            scalar,
            bytes,
        })
    }
}
fn hash_field(
    queries: &mut OriginalFieldQueries<'_>,
    rowid: i64,
    hash: &mut Sha256,
    budget: Option<&InspectionBudget>,
) -> Result<()> {
    crate::budget::rows(budget, 3)?;
    crate::budget::work(budget, 32)?;
    let (kind, len): (String, Option<i64>) = queries
        .metadata
        .query_row([rowid], |r| Ok((r.get(0)?, r.get(1)?)))?;
    hash.update((kind.len() as u64).to_le_bytes());
    hash.update(kind.as_bytes());
    match kind.as_str() {
        "null" => {}
        "integer" => {
            crate::budget::rows(budget, 2)?;
            crate::budget::work(budget, 16)?;
            let n: i64 = queries.scalar.query_row([rowid], |r| r.get(0))?;
            hash.update(n.to_le_bytes());
        }
        "real" => {
            crate::budget::rows(budget, 2)?;
            crate::budget::work(budget, 16)?;
            let n: f64 = queries.scalar.query_row([rowid], |r| r.get(0))?;
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
                let bytes: Vec<u8> = queries.bytes.query_row(
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
pub(crate) fn original_rows_hash(connection: &Connection) -> Result<String> {
    original_rows_hash_bounded(connection, None)
}
pub(crate) fn original_rows_hash_bounded(
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
        // At most three statements per bounded included column, scoped to this
        // table and hash pass. Every original field is still read and hashed.
        let mut field_queries = columns
            .iter()
            .map(|column| OriginalFieldQueries::new(connection, table, column, budget))
            .collect::<Result<Vec<_>>>()?;
        let mut statement =
            connection.prepare(&format!("SELECT rowid FROM {table} ORDER BY rowid"))?;
        let mut rows = statement.query([])?;
        let mut count = 0u64;
        while let Some(row) = rows.next()? {
            crate::budget::rows(budget, 2)?;
            crate::budget::work(budget, 16)?;
            let rowid: i64 = row.get(0)?;
            hash.update(rowid.to_le_bytes());
            for queries in &mut field_queries {
                hash_field(queries, rowid, &mut hash, budget)?;
            }
            count += 1;
        }
        hash.update(count.to_le_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
#[path = "inspection/hash_tests.rs"]
mod hash_tests;
pub(crate) fn inspect_connection(
    connection: &Connection,
    path: &Path,
    descriptor: &TakeDescriptor,
) -> Result<JournalInspection> {
    inspect_connection_bounded(connection, path, descriptor, None)
}
pub(crate) fn inspect_connection_bounded(
    connection: &Connection,
    path: &Path,
    descriptor: &TakeDescriptor,
    budget: Option<&InspectionBudget>,
) -> Result<JournalInspection> {
    let result = inspect_inner(connection, path, descriptor, budget);
    crate::budget::finish(budget, result)
}
fn inspect_inner(
    connection: &Connection,
    path: &Path,
    descriptor: &TakeDescriptor,
    budget: Option<&InspectionBudget>,
) -> Result<JournalInspection> {
    let state = journal::state_bounded(connection, budget)?;
    if state == TakeState::Interrupted {
        journal::load_interrupted_bounded(connection, budget)?;
    }
    let manifest = if state == TakeState::Sealed {
        Some(journal::verify_sealed_bounded(
            connection, descriptor, budget,
        )?)
    } else {
        None
    };
    let mut report = journal::scan_bounded(
        connection,
        descriptor,
        manifest.as_ref().map(|m| m.body.chunk_count),
        budget,
    )?;
    report.unknown_tail = manifest
        .as_ref()
        .is_none_or(|m| m.body.interrupted.is_some());
    let original_rows_sha256 = original_rows_hash_bounded(connection, budget)?;
    let (byte_length, journal_sha256) = physical_hash_bounded(path, budget)?;
    Ok(JournalInspection {
        path: path.to_owned(),
        descriptor: descriptor.clone(),
        descriptor_sha256: descriptor.sha256()?,
        journal_sha256,
        original_rows_sha256,
        byte_length,
        report,
        manifest,
    })
}
impl CaptureReader {
    pub fn open_bounded(path: impl AsRef<Path>, budget: &InspectionBudget) -> Result<Self> {
        let result = (|| -> Result<Self> {
            let path = preflight_path(path.as_ref(), budget)?;
            let owner = existing_owner(&path)?;
            Self::open_bounded_owned(&path, budget, owner)
        })();
        budget.finish(result)
    }
    fn open_bounded_owned(path: &Path, budget: &InspectionBudget, owner: File) -> Result<Self> {
        let result = (|| -> Result<Self> {
            // Recheck all sidecars after owner acquisition. The opened file and
            // lock identities remain anchored for any later explicit copy.
            let path = preflight_path(path, budget)?;
            let anchors = copy_recovery::SourceAnchors::capture(&path, &owner)?;
            ordinary(&path)?;
            let connection = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            budget.install(&connection)?;
            connection.busy_timeout(Duration::from_secs(2))?;
            connection.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; BEGIN DEFERRED;")?;
            preflight_connection(&connection, budget)?;
            budget.visit_rows(2)?;
            budget.charge_work(8)?;
            let version: u32 =
                connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
            require(
                version == JOURNAL_VERSION,
                "Unsupported retained capture version",
            )?;
            let descriptor = journal::load_descriptor_bounded(&connection, Some(budget))?;
            let inspection =
                inspect_connection_bounded(&connection, &path, &descriptor, Some(budget))?;
            if let Some(anchors) = &anchors {
                anchors.check(&path, &owner)?;
            }
            Ok(Self {
                connection,
                owner,
                inspection,
                budget: Some(budget.share()),
                anchors,
            })
        })();
        budget.finish(result)
    }
    pub fn inspection_budget(&self) -> Option<&InspectionBudget> {
        self.budget.as_ref()
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let requested = path.as_ref();
        ordinary(requested)?;
        let path = fs::canonicalize(requested)?;
        let owner = existing_owner(&path)?;
        ordinary(&path)?;
        // WAL readers can create shared-memory sidecars even with a read-only
        // database handle. Reject that mode before SQLite sees the connection.
        let mut header = [0u8; 100];
        let mut file = File::open(&path)?;
        file.read_exact(&mut header)?;
        file.seek(SeekFrom::Start(0))?;
        require(
            &header[..16] == b"SQLite format 3\0" && header[18] == 1 && header[19] == 1,
            "Capture inspection requires a rollback-mode SQLite original",
        )?;
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            require(
                fs::symlink_metadata(PathBuf::from(sidecar))
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
                "Capture has an ambiguous WAL/shared-memory sidecar",
            )?;
        }
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA query_only=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; BEGIN DEFERRED;",
        )?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        require(
            version == JOURNAL_VERSION,
            "Unsupported retained capture version",
        )?;
        let descriptor = journal::load_descriptor(&connection)?;
        let inspection = inspect_connection(&connection, &path, &descriptor)?;
        Ok(Self {
            connection,
            owner,
            inspection,
            budget: None,
            anchors: None,
        })
    }
    pub fn inspection(&self) -> &JournalInspection {
        &self.inspection
    }
    /// Read only a chunk already authenticated in this reader's held snapshot.
    pub fn read_ordinal(&self, ordinal: u64) -> Result<ChunkRecord> {
        let result = read_ordinal_bounded(
            &self.connection,
            &self.inspection,
            ordinal,
            self.budget.as_ref(),
        );
        crate::budget::finish(self.budget.as_ref(), result)
    }
    /// Low-level seal operation. A caller replaying a prior review must also
    /// authenticate its retained original-row digest; NativeStore supplies that
    /// admission check and derives the operation from the complete review.
    pub fn recover(self, operation_id: &str) -> Result<(SealedManifest, bool)> {
        require(
            self.budget.is_none(),
            "Bounded read-only inspection does not authorize recovery",
        )?;
        identifier(operation_id)?;
        require(
            !self.inspection.descriptor.provenance.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "omniusong.performance.v1" | "omniusong.performance.v2"
                )
            }),
            "Performance originals require an explicit format-aware selected recovery prefix",
        )?;
        if let Some(manifest) = &self.inspection.manifest {
            require(
                manifest.body.operation_id == operation_id && manifest.body.interrupted.is_some(),
                "Conflicting retry of reviewed prefix recovery",
            )?;
            return Ok((manifest.clone(), true));
        }
        require(
            self.inspection
                .report
                .streams
                .iter()
                .zip(&self.inspection.descriptor.streams)
                .any(|(prefix, stream)| {
                    matches!(stream.kind, StreamKind::Audio { .. }) && prefix.recorded_units > 0
                }),
            "An empty verified audio prefix cannot become a playable recovered take",
        )?;
        let Self {
            connection,
            owner,
            inspection,
            budget: _,
            anchors: _,
        } = self;
        drop(connection);
        let mut journal = Journal::open_owned(inspection.path.clone(), owner, Some(&inspection))?;
        let mut expected = inspection.report.clone();
        expected.state = TakeState::Interrupted;
        expected.unknown_tail = true;
        require(
            journal.descriptor() == &inspection.descriptor
                && journal.recovery_report()? == expected,
            "Reviewed prefix changed during recovery acquisition",
        )?;
        let manifest =
            journal.seal_recovered_checked(operation_id, &inspection.original_rows_sha256)?;
        require(
            manifest.body.descriptor_sha256 == inspection.descriptor_sha256
                && manifest.body.chunk_count == inspection.report.verified_chunks
                && manifest.body.payload_bytes == inspection.report.verified_payload_bytes
                && manifest.body.chunk_index_sha256 == inspection.report.chunk_index_sha256
                && manifest.body.streams == inspection.report.streams,
            "Recovered manifest differs from the exact reviewed prefix",
        )?;
        Ok((manifest, false))
    }
    /// Explicit byte-prefix recovery. The caller, rather than this recording
    /// crate, must establish that its chosen engine checkpoint is complete.
    /// All later original rows remain in the journal and its backups.
    pub fn recover_prefix(
        self,
        operation_id: &str,
        selection: &RecoveryPrefix,
    ) -> Result<(SealedManifest, bool)> {
        require(
            self.budget.is_none(),
            "Bounded read-only inspection does not authorize recovery",
        )?;
        identifier(operation_id)?;
        selection.validate()?;
        require(
            selection.chunk_count <= self.inspection.report.verified_chunks,
            "Selected recovery prefix exceeds the held retained verified prefix",
        )?;
        let report = journal::scan(
            &self.connection,
            &self.inspection.descriptor,
            Some(selection.chunk_count),
        )?;
        require(
            report.problems.is_empty()
                && report.verified_chunks == selection.chunk_count
                && report.chunk_index_sha256 == selection.chunk_index_sha256,
            "Selected recovery prefix hash/count differs from the held original",
        )?;
        require(
            report
                .streams
                .iter()
                .zip(&self.inspection.descriptor.streams)
                .any(|(prefix, stream)| {
                    matches!(stream.kind, StreamKind::Audio { .. }) && prefix.recorded_units > 0
                }),
            "Selected recovery prefix must contain recorded audio",
        )?;
        let replayed = self.inspection.manifest.is_some();
        if let Some(manifest) = &self.inspection.manifest {
            require(
                manifest.body.operation_id == operation_id
                    && manifest.body.interrupted.is_some()
                    && manifest.body.chunk_count == selection.chunk_count
                    && manifest.body.chunk_index_sha256 == selection.chunk_index_sha256,
                "Conflicting retry of selected prefix recovery",
            )?;
        }
        let Self {
            connection,
            owner,
            inspection,
            budget: _,
            anchors: _,
        } = self;
        drop(connection);
        let mut journal = Journal::open_owned(inspection.path.clone(), owner, Some(&inspection))?;
        let mut expected = inspection.report.clone();
        if !replayed {
            expected.state = TakeState::Interrupted;
            expected.unknown_tail = true;
        }
        require(
            journal.descriptor() == &inspection.descriptor
                && journal.recovery_report()? == expected,
            "Reviewed original changed during selected recovery acquisition",
        )?;
        let manifest = journal.seal_recovered_prefix_checked(
            operation_id,
            &inspection.original_rows_sha256,
            selection,
        )?;
        require(
            manifest.body.descriptor_sha256 == inspection.descriptor_sha256
                && manifest.body.chunk_count == selection.chunk_count
                && manifest.body.chunk_index_sha256 == selection.chunk_index_sha256
                && manifest.body.streams == report.streams
                && manifest.body.payload_bytes == report.verified_payload_bytes,
            "Recovered manifest differs from the exact selected original prefix",
        )?;
        Ok((manifest, replayed))
    }
}
