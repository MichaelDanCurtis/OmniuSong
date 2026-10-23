//! Independent bounded SQL postimages. These hashes prove exact rows/schema,
//! never musical semantics or source authority. The disposable spool's main
//! file is capped at 512 MiB; SQLite temporary-file overhead is separate.
//! `None` preserves legacy SQL behavior and grants no VM/cumulative/time bound.
//! With `Some`, the caller keeps its source/destination SQL progress ownership
//! under that same governor; only this disposable spool installs its own hook.
use crate::{
    Result, StoreError,
    archive_boundary::{self as boundary, ArchiveSqlOrigin},
};
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

const MAX_ROWS: u64 = 4_194_304;
const MAX_KEY: usize = 256;
const CHUNK: usize = 65_536;
const MAX_SCHEMA_ROWS: usize = 256;
const PAGES: u32 = 131_072;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    Catalog,
    ArchiveV1,
    ArchiveV2,
}
#[derive(Clone, Copy)]
struct Column {
    name: &'static str,
    kind: &'static str,
    maximum: usize,
}
#[derive(Clone, Copy)]
struct Table {
    name: &'static str,
    columns: &'static [Column],
    pk: &'static [usize],
}
const fn text(name: &'static str, maximum: usize) -> Column {
    Column {
        name,
        kind: "TEXT",
        maximum,
    }
}
const fn integer(name: &'static str) -> Column {
    Column {
        name,
        kind: "INTEGER",
        maximum: 8,
    }
}
const fn blob(name: &'static str, maximum: usize) -> Column {
    Column {
        name,
        kind: "BLOB",
        maximum,
    }
}
const DOC: usize = omniusong_core::MAX_NATIVE_PROJECT_BYTES;
const PROJECT: Table = Table {
    name: "projects",
    columns: &[
        text("id", 96),
        text("payload", DOC),
        integer("revision"),
        integer("cursor"),
    ],
    pk: &[0],
};
const ARCHIVE_PROJECT: Table = Table {
    name: "projects",
    columns: &[
        text("id", 96),
        text("payload", DOC),
        integer("revision"),
        integer("cursor"),
        text("payload_sha256", 64),
    ],
    pk: &[0],
};
const EDIT: Table = Table {
    name: "edits",
    columns: &[
        text("project_id", 96),
        integer("sequence"),
        text("before_json", DOC),
        text("after_json", DOC),
    ],
    pk: &[0, 1],
};
const ARCHIVE_EDIT: Table = Table {
    name: "edits",
    columns: &[
        text("project_id", 96),
        integer("sequence"),
        text("before_json", DOC),
        text("after_json", DOC),
        text("before_sha256", 64),
        text("after_sha256", 64),
    ],
    pk: &[0, 1],
};
const ASSET: Table = Table {
    name: "assets",
    columns: &[text("id", 96), blob("payload", 30 * 1024 * 1024)],
    pk: &[0],
};
const IMPORT: Table = Table {
    name: "imports",
    columns: &[
        text("project_id", 96),
        text("source_sha256", 64),
        blob("original_json", omniusong_core::MAX_PROJECT_BYTES),
    ],
    pk: &[0],
};
const OPERATION: Table = Table {
    name: "operations",
    columns: &[
        text("operation_id", 96),
        text("project_id", 96),
        text("request_json", omniusong_core::contracts::WORKER_MAX_BYTES),
        text("request_sha256", 64),
        integer("previous_revision"),
        integer("committed_revision"),
        text("result_json", DOC),
        text("result_sha256", 64),
    ],
    pk: &[0],
};
const TAKE: Table = Table {
    name: "native_takes",
    columns: &[
        text("take_id", 96),
        text("manifest_sha256", 64),
        text("object_sha256", 64),
        integer("byte_length"),
        text("manifest_json", 512 * 1024),
    ],
    pk: &[0],
};
const PROOF: Table = Table {
    name: "archive_copy_proofs",
    columns: &[
        text("proof_sha256", 64),
        text("receipt_json", DOC + 32 * 1024),
    ],
    pk: &[0],
};
const MAPPING: Table = Table {
    name: "archive_copy_proof_operations",
    columns: &[
        text("proof_sha256", 64),
        text("source_operation_id", 96),
        text("destination_operation_id", 96),
        text("source_request_sha256", 64),
        text("source_result_sha256", 64),
        text("destination_request_sha256", 64),
        text("destination_result_sha256", 64),
    ],
    pk: &[0, 1],
};
const LINEAGE: Table = Table {
    name: "archive_project_lineage",
    columns: &[text("project_id", 96), text("proof_sha256", 64)],
    pk: &[0, 1],
};
const COPY: Table = Table {
    name: "archive_copies",
    columns: &[
        text("operation_id", 96),
        text("project_id", 96),
        text("proof_sha256", 64),
    ],
    pk: &[0],
};
const INTENT: Table = Table {
    name: "recording_intents",
    columns: &[
        text("id", 32),
        integer("revision"),
        blob("payload", 16_384),
        text("payload_sha256", 64),
    ],
    pk: &[0, 1],
};
const REVIEW: Table = Table {
    name: "performance_source_reviews",
    columns: &[
        text("id", 32),
        blob("payload", 65_536),
        text("payload_sha256", 64),
    ],
    pk: &[0],
};
const ACTION: Table = Table {
    name: "performance_source_actions",
    columns: &[
        text("id", 32),
        integer("revision"),
        text("review_blob_sha256", 64),
        blob("payload", 16_384),
        text("payload_sha256", 64),
    ],
    pk: &[0, 1],
};
const META: Table = Table {
    name: "archive_meta",
    columns: &[
        integer("id"),
        text("project_id", 96),
        integer("metadata_bytes"),
        integer("asset_bytes"),
        integer("take_bytes"),
        integer("edit_count"),
        integer("operation_count"),
        integer("import_count"),
        integer("asset_count"),
        integer("take_count"),
        integer("chunk_count"),
        text("index_sha256", 64),
    ],
    pk: &[0],
};
const CHUNKS: Table = Table {
    name: "take_chunks",
    columns: &[
        text("object_sha256", 64),
        integer("sequence"),
        integer("byte_length"),
        text("sha256", 64),
        blob("payload", crate::ARCHIVE_CHUNK_BYTES),
    ],
    pk: &[0, 1],
};
const CATALOG: &[Table] = &[
    PROJECT, EDIT, ASSET, IMPORT, OPERATION, TAKE, PROOF, MAPPING, LINEAGE, COPY, INTENT, REVIEW,
    ACTION,
];
const ARCHIVE_V1: &[Table] = &[
    META,
    ARCHIVE_PROJECT,
    ARCHIVE_EDIT,
    OPERATION,
    IMPORT,
    ASSET,
    TAKE,
    CHUNKS,
];
const ARCHIVE_V2: &[Table] = &[
    META,
    ARCHIVE_PROJECT,
    ARCHIVE_EDIT,
    OPERATION,
    IMPORT,
    ASSET,
    TAKE,
    CHUNKS,
    PROOF,
    MAPPING,
    LINEAGE,
    COPY,
];
impl Layout {
    pub(crate) fn archive_version(version: u32) -> Result<Self> {
        match version {
            1 => Ok(Self::ArchiveV1),
            2 => Ok(Self::ArchiveV2),
            _ => Err(invalid("Unsupported postimage archive version")),
        }
    }
    fn tables(self) -> &'static [Table] {
        match self {
            Self::Catalog => CATALOG,
            Self::ArchiveV1 => ARCHIVE_V1,
            Self::ArchiveV2 => ARCHIVE_V2,
        }
    }
}
fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}
fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(invalid(message)) }
}
fn sql<T>(origin: ArchiveSqlOrigin, result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|error| boundary::sql_error(origin, error))
}
fn checkpoint(budget: Option<&InspectionBudget>) -> Result<()> {
    budget.map_or(Ok(()), |budget| {
        budget.checkpoint().map_err(StoreError::Recording)
    })
}
fn credit(budget: Option<&InspectionBudget>, bytes: u64) -> Result<()> {
    if let Some(budget) = budget {
        budget.checkpoint()?;
        budget.charge_work(bytes)?;
    }
    Ok(())
}
fn row_credit(budget: Option<&InspectionBudget>) -> Result<()> {
    if let Some(budget) = budget {
        budget.visit_rows(1)?;
    }
    credit(budget, 1024)
}
fn framed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
fn finish<T>(budget: Option<&InspectionBudget>, result: Result<T>) -> Result<T> {
    if result.as_ref().err().is_some_and(|error| {
        error.catalog_connection_unusable()
            || error
                .catalog_sqlite_error_code()
                .is_some_and(|code| matches!(code & 0xff, 10 | 11 | 26))
    }) {
        return result;
    }
    checkpoint(budget)?;
    result
}
fn row_hash(table: Table) -> Sha256 {
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.archive-postimage-row.v1\0");
    framed(&mut hash, table.name.as_bytes());
    hash.update((table.columns.len() as u64).to_le_bytes());
    hash
}
fn cell_header(hash: &mut Sha256, tag: u8, length: usize) {
    hash.update([tag]);
    hash.update((length as u64).to_le_bytes());
}
fn key_cell(key: &mut Vec<u8>, value: ValueRef<'_>, column: Column) -> Result<()> {
    match value {
        ValueRef::Integer(value) if column.kind == "INTEGER" => {
            key.push(0x10);
            key.extend_from_slice(&((value as u64) ^ (1 << 63)).to_be_bytes());
        }
        ValueRef::Text(bytes) if column.kind == "TEXT" => {
            require(
                bytes.len() <= column.maximum,
                "Postimage primary key exceeds its field bound",
            )?;
            require(
                std::str::from_utf8(bytes).is_ok(),
                "Postimage primary key is not UTF-8",
            )?;
            key.push(0x20);
            for byte in bytes {
                if *byte == 0 {
                    key.extend_from_slice(&[0, 255]);
                } else {
                    key.push(*byte);
                }
                require(
                    key.len() <= MAX_KEY,
                    "Postimage primary key exceeds 256 encoded bytes",
                )?;
            }
            key.extend_from_slice(&[0, 0]);
        }
        _ => {
            return Err(invalid(
                "Postimage primary key has an unsupported SQLite type",
            ));
        }
    }
    require(
        key.len() <= MAX_KEY,
        "Postimage primary key exceeds 256 encoded bytes",
    )
}
fn proof_values(
    table: Table,
    values: &[ValueRef<'_>],
    budget: Option<&InspectionBudget>,
) -> Result<(Vec<u8>, [u8; 32])> {
    require(
        values.len() == table.columns.len() && values.len() <= 16,
        "Postimage row column count differs from its static layout",
    )?;
    let mut hash = row_hash(table);
    for (value, column) in values.iter().zip(table.columns) {
        let (tag, length) = match value {
            ValueRef::Null => (b'n', 0),
            ValueRef::Integer(_) => (b'i', 8),
            ValueRef::Real(_) => (b'r', 8),
            ValueRef::Text(v) => (b't', v.len()),
            ValueRef::Blob(v) => (b'b', v.len()),
        };
        require(
            length <= column.maximum,
            "Postimage field exceeds its compiled byte ceiling",
        )?;
        cell_header(&mut hash, tag, length);
        match value {
            ValueRef::Null => {}
            ValueRef::Integer(value) => {
                credit(budget, 16)?;
                hash.update(value.to_le_bytes());
            }
            ValueRef::Real(value) => {
                credit(budget, 16)?;
                hash.update(value.to_bits().to_le_bytes());
            }
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                for chunk in bytes.chunks(CHUNK) {
                    credit(
                        budget,
                        (chunk.len() as u64)
                            .checked_mul(2)
                            .ok_or_else(|| invalid("Postimage work overflow"))?,
                    )?;
                    hash.update(chunk);
                }
            }
        }
    }
    credit(budget, MAX_KEY as u64)?;
    let mut key = Vec::new();
    for index in table.pk {
        key_cell(&mut key, values[*index], table.columns[*index])?;
    }
    Ok((key, hash.finalize().into()))
}

#[derive(Clone, PartialEq, Eq)]
struct Schema([u8; 32]);
fn schema(
    connection: &Connection,
    layout: Layout,
    origin: ArchiveSqlOrigin,
    budget: Option<&InspectionBudget>,
) -> Result<Schema> {
    let mut digest = Sha256::new();
    digest.update(b"OmniuSong.archive-postimage-schema.v1\0");
    let mut found = std::collections::BTreeSet::new();
    credit(budget, (4 * CHUNK) as u64)?;
    let mut statement=sql(origin,connection.prepare("SELECT CASE WHEN typeof(type)='text' AND octet_length(type)<=16 THEN type END,CASE WHEN typeof(name)='text' AND octet_length(name)<=256 THEN name END,CASE WHEN typeof(tbl_name)='text' AND octet_length(tbl_name)<=256 THEN tbl_name END,CASE WHEN sql IS NULL THEN NULL WHEN typeof(sql)='text' AND octet_length(sql)<=65536 THEN sql ELSE 1 END FROM main.sqlite_schema ORDER BY type,name LIMIT 257"))?;
    let mut rows = sql(origin, statement.query([]))?;
    let mut count = 0;
    loop {
        row_credit(budget)?;
        credit(budget, (2 * CHUNK) as u64)?;
        let Some(row) = sql(origin, rows.next())? else {
            break;
        };
        count += 1;
        require(
            count <= MAX_SCHEMA_ROWS,
            "Postimage schema exceeds its object ceiling",
        )?;
        let kind: String = sql(origin, row.get(0))?;
        let name: String = sql(origin, row.get(1))?;
        let owner: String = sql(origin, row.get(2))?;
        let ddl: Option<String> = sql(origin, row.get(3))?;
        if kind == "table" {
            require(
                layout.tables().iter().any(|table| table.name == name),
                "Postimage layout contains an unknown persistent table",
            )?;
            let definition = ddl
                .as_deref()
                .ok_or_else(|| invalid("Postimage table has no ordinary SQL definition"))?;
            let upper = definition.trim_start().to_ascii_uppercase();
            require(
                upper.starts_with("CREATE TABLE ") && !upper.contains("WITHOUT ROWID"),
                "Postimage virtual or WITHOUT ROWID table is unsupported",
            )?;
            found.insert(name.clone());
        } else {
            require(
                (kind == "index" || kind == "trigger")
                    && layout.tables().iter().any(|table| table.name == owner),
                "Postimage schema contains an unknown persistent object",
            )?;
        }
        framed(&mut digest, kind.as_bytes());
        framed(&mut digest, name.as_bytes());
        framed(&mut digest, owner.as_bytes());
        match ddl {
            Some(ddl) => {
                digest.update([1]);
                framed(&mut digest, ddl.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    require(
        found.len() == layout.tables().len(),
        "Postimage layout is missing a required persistent table",
    )?;
    drop(rows);
    drop(statement);
    for table in layout.tables() {
        credit(budget, (17 * CHUNK) as u64)?;
        let mut statement = sql(
            origin,
            connection.prepare(&format!("PRAGMA main.table_xinfo('{}')", table.name)),
        )?;
        let mut rows = sql(origin, statement.query([]))?;
        let mut index = 0;
        while let Some(row) = sql(origin, rows.next())? {
            row_credit(budget)?;
            require(
                index < table.columns.len() && index < 16,
                "Postimage table has surplus or generated columns",
            )?;
            let cid: i64 = sql(origin, row.get(0))?;
            let name: String = sql(origin, row.get(1))?;
            let kind: String = sql(origin, row.get(2))?;
            let default: Option<String> = sql(origin, row.get(4))?;
            let pk: i64 = sql(origin, row.get(5))?;
            let hidden: i64 = sql(origin, row.get(6))?;
            let expected_pk = table
                .pk
                .iter()
                .position(|column| *column == index)
                .map_or(0, |position| position as i64 + 1);
            require(
                cid == index as i64
                    && name == table.columns[index].name
                    && kind.eq_ignore_ascii_case(table.columns[index].kind)
                    && default.is_none()
                    && pk == expected_pk
                    && hidden == 0,
                "Postimage table columns/primary key are generated, hidden or unsupported",
            )?;
            index += 1;
        }
        require(
            index == table.columns.len(),
            "Postimage table is missing static columns",
        )?;
    }
    let application: i64 = sql(
        origin,
        connection.pragma_query_value(Some("main"), "application_id", |row| row.get(0)),
    )?;
    let version: i64 = sql(
        origin,
        connection.pragma_query_value(Some("main"), "user_version", |row| row.get(0)),
    )?;
    digest.update(application.to_le_bytes());
    digest.update(version.to_le_bytes());
    Ok(Schema(digest.finalize().into()))
}

fn proof_row(
    connection: &Connection,
    table: Table,
    rowid: i64,
    origin: ArchiveSqlOrigin,
    budget: Option<&InspectionBudget>,
) -> Result<(Vec<u8>, [u8; 32], u64)> {
    let mut hash = row_hash(table);
    let mut key_parts = Vec::new();
    let mut raw = 0u64;
    for (index, column) in table.columns.iter().enumerate() {
        row_credit(budget)?;
        let query = format!(
            "SELECT typeof(\"{0}\"),CASE WHEN typeof(\"{0}\") IN ('text','blob') THEN octet_length(\"{0}\") ELSE 0 END,CASE WHEN typeof(\"{0}\") IN ('integer','real') THEN \"{0}\" END FROM main.\"{1}\" WHERE rowid=?1",
            column.name, table.name
        );
        let (kind, length, scalar): (String, i64, rusqlite::types::Value) = sql(
            origin,
            connection.query_row(&query, [rowid], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            }),
        )?;
        let raw_length = if kind == "integer" || kind == "real" {
            8
        } else {
            u64::try_from(length).map_err(|_| invalid("Postimage field length is negative"))?
        };
        require(
            raw_length <= column.maximum as u64,
            "Postimage field exceeds its compiled byte ceiling",
        )?;
        raw = raw
            .checked_add(raw_length)
            .ok_or_else(|| invalid("Postimage raw counter overflow"))?;
        let mut primary = None;
        match kind.as_str() {
            "null" => cell_header(&mut hash, b'n', 0),
            "integer" => {
                let rusqlite::types::Value::Integer(value) = scalar else {
                    return Err(invalid("Postimage integer metadata changed"));
                };
                cell_header(&mut hash, b'i', 8);
                hash.update(value.to_le_bytes());
                primary = Some(rusqlite::types::Value::Integer(value));
            }
            "real" => {
                let rusqlite::types::Value::Real(value) = scalar else {
                    return Err(invalid("Postimage real metadata changed"));
                };
                cell_header(&mut hash, b'r', 8);
                hash.update(value.to_bits().to_le_bytes());
                primary = Some(rusqlite::types::Value::Real(value));
            }
            "text" | "blob" => {
                let length = usize::try_from(length)
                    .map_err(|_| invalid("Postimage field length is negative"))?;
                require(
                    length <= column.maximum,
                    "Postimage field exceeds its compiled byte ceiling",
                )?;
                cell_header(&mut hash, if kind == "text" { b't' } else { b'b' }, length);
                let is_key = table.pk.contains(&index);
                let mut key_bytes = Vec::new();
                if is_key {
                    require(
                        length <= MAX_KEY,
                        "Postimage primary key exceeds its byte ceiling",
                    )?;
                    credit(budget, length as u64)?;
                }
                let query = format!(
                    "SELECT substr(CAST(\"{}\" AS BLOB),?2,?3) FROM main.\"{}\" WHERE rowid=?1",
                    column.name, table.name
                );
                let mut offset = 0;
                while offset < length {
                    let size = (length - offset).min(CHUNK);
                    row_credit(budget)?;
                    // A separate SQLite substr/CAST query may interpret the
                    // complete cell again. Prepay that full-cell work for
                    // every query, in addition to its bounded Rust result.
                    credit(
                        budget,
                        (length as u64)
                            .checked_mul(2)
                            .ok_or_else(|| invalid("Postimage work overflow"))?,
                    )?;
                    credit(budget, (size as u64) * 2)?;
                    let bytes: Vec<u8> = sql(
                        origin,
                        connection.query_row(
                            &query,
                            params![rowid, (offset + 1) as i64, size as i64],
                            |row| row.get(0),
                        ),
                    )?;
                    require(
                        bytes.len() == size,
                        "Postimage field changed during its bounded read",
                    )?;
                    hash.update(&bytes);
                    if is_key {
                        key_bytes.extend_from_slice(&bytes);
                    }
                    offset += size;
                }
                if is_key {
                    require(
                        kind == "text",
                        "Postimage primary key has an unsupported SQLite type",
                    )?;
                    primary = Some(rusqlite::types::Value::Text(
                        String::from_utf8(key_bytes)
                            .map_err(|_| invalid("Postimage primary key is not UTF-8"))?,
                    ));
                }
            }
            _ => return Err(invalid("Unsupported postimage SQLite storage class")),
        }
        if table.pk.contains(&index) {
            key_parts.push((
                index,
                primary.ok_or_else(|| invalid("Postimage primary key is NULL"))?,
            ));
        }
    }
    let mut key = Vec::new();
    credit(budget, MAX_KEY as u64)?;
    for index in table.pk {
        let value = &key_parts
            .iter()
            .find(|(column, _)| column == index)
            .ok_or_else(|| invalid("Postimage primary key is missing"))?
            .1;
        key_cell(&mut key, ValueRef::from(value), table.columns[*index])?;
    }
    Ok((key, hash.finalize().into(), raw))
}

pub(crate) struct PostimageLedger<'b> {
    connection: Connection,
    file: File,
    path: PathBuf,
    layout: Layout,
    budget: Option<&'b InspectionBudget>,
    schema: RefCell<Option<Schema>>,
    baseline: Cell<bool>,
    planned: Cell<bool>,
    frozen: Cell<bool>,
    poisoned: Cell<bool>,
    rows: Cell<u64>,
    raw_bytes: Cell<u64>,
}
#[derive(PartialEq, Eq)]
struct TableDigest {
    rows: u64,
    sha256: [u8; 32],
}
pub(crate) struct ExpectedPostimage<'b> {
    ledger: PostimageLedger<'b>,
    digests: Vec<TableDigest>,
}
impl<'b> PostimageLedger<'b> {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn new(
        stage: &Path,
        layout: Layout,
        budget: Option<&'b InspectionBudget>,
    ) -> Result<Self> {
        checkpoint(budget)?;
        credit(budget, 4096)?;
        let metadata = fs::symlink_metadata(stage)?;
        require(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && fs::canonicalize(stage)? == stage,
            "Postimage spool requires its canonical ordinary staging directory",
        )?;
        let path = stage.join(format!(
            "expected-postimage-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)?;
        let connection = sql(
            ArchiveSqlOrigin::PrivateOutput,
            Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            ),
        )?;
        sql(ArchiveSqlOrigin::PrivateOutput,connection.execute_batch("PRAGMA page_size=4096;PRAGMA max_page_count=131072;PRAGMA journal_mode=OFF;PRAGMA synchronous=OFF;PRAGMA temp_store=FILE;PRAGMA cache_size=-512;PRAGMA mmap_size=0;PRAGMA trusted_schema=OFF;"))?;
        if let Some(budget) = budget {
            budget.install_sqlite_progress(&connection)?;
        }
        finish(budget,sql(ArchiveSqlOrigin::PrivateOutput,connection.execute_batch("CREATE TABLE expected(t INTEGER NOT NULL,k BLOB NOT NULL,h BLOB NOT NULL,planned INTEGER NOT NULL,PRIMARY KEY(t,k)) WITHOUT ROWID;CREATE TABLE observed(t INTEGER NOT NULL,k BLOB NOT NULL,h BLOB NOT NULL,PRIMARY KEY(t,k)) WITHOUT ROWID;")))?;
        let ledger = Self {
            connection,
            file,
            path,
            layout,
            budget,
            schema: RefCell::new(None),
            baseline: Cell::new(false),
            planned: Cell::new(false),
            frozen: Cell::new(false),
            poisoned: Cell::new(false),
            rows: Cell::new(0),
            raw_bytes: Cell::new(0),
        };
        ledger.spool_identity()?;
        Ok(ledger)
    }
    fn mutable(&self) -> Result<()> {
        checkpoint(self.budget)?;
        require(
            !self.frozen.get() && !self.poisoned.get(),
            "Postimage plan is frozen or incomplete",
        )
    }
    fn checked<T>(&self, result: Result<T>) -> Result<T> {
        let result = finish(self.budget, result);
        if result.is_err() {
            self.poisoned.set(true);
        }
        result
    }
    fn observe(&self) -> Result<()> {
        row_credit(self.budget)?;
        let rows = self
            .rows
            .get()
            .checked_add(1)
            .ok_or_else(|| invalid("Postimage row counter overflow"))?;
        require(
            rows <= MAX_ROWS,
            "Postimage observations/plans exceed the row ceiling",
        )?;
        self.rows.set(rows);
        Ok(())
    }
    fn add_raw(&self, bytes: u64) -> Result<()> {
        let total = self
            .raw_bytes
            .get()
            .checked_add(bytes)
            .ok_or_else(|| invalid("Postimage raw counter overflow"))?;
        self.raw_bytes.set(total);
        Ok(())
    }
    fn spool_identity(&self) -> Result<()> {
        let held = self.file.metadata()?;
        let path = fs::symlink_metadata(&self.path)?;
        require(
            path.is_file()
                && !path.file_type().is_symlink()
                && held.len() <= u64::from(PAGES) * 4096,
            "Postimage spool is not its bounded ordinary file",
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            require(
                held.dev() == path.dev()
                    && held.ino() == path.ino()
                    && held.nlink() == 1
                    && path.nlink() == 1,
                "Postimage spool identity changed",
            )?;
        }
        Ok(())
    }
    fn table(&self, name: &str) -> Result<(usize, Table)> {
        self.layout
            .tables()
            .iter()
            .copied()
            .enumerate()
            .find(|(_, table)| table.name == name)
            .ok_or_else(|| invalid("Unsupported postimage table"))
    }
    fn insert_proof(
        &self,
        index: usize,
        key: &[u8],
        hash: &[u8; 32],
        planned: bool,
        reuse: bool,
    ) -> Result<()> {
        self.observe()?;
        credit(self.budget, (key.len() + hash.len() + 1024) as u64)?;
        self.spool_identity()?;
        let current: Option<(Vec<u8>, i64)> = sql(
            ArchiveSqlOrigin::PrivateOutput,
            self.connection
                .query_row(
                    "SELECT h,planned FROM expected WHERE t=?1 AND k=?2",
                    params![index as i64, key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional(),
        )?;
        if let Some((existing, was_planned)) = current {
            require(
                planned && reuse && was_planned == 0 && existing.as_slice() == hash,
                "Postimage expected key collision or duplicate planned row",
            )?;
            sql(
                ArchiveSqlOrigin::PrivateOutput,
                self.connection.execute(
                    "UPDATE expected SET planned=1 WHERE t=?1 AND k=?2",
                    params![index as i64, key],
                ),
            )?;
        } else {
            sql(
                ArchiveSqlOrigin::PrivateOutput,
                self.connection.execute(
                    "INSERT INTO expected VALUES(?1,?2,?3,?4)",
                    params![index as i64, key, hash.as_slice(), i64::from(planned)],
                ),
            )?;
        }
        Ok(())
    }
    pub(crate) fn capture_schema(
        &self,
        connection: &Connection,
        origin: ArchiveSqlOrigin,
    ) -> Result<()> {
        self.checked((|| {
            self.mutable()?;
            require(
                self.schema.borrow().is_none() && !self.planned.get(),
                "Postimage schema must be captured once before planning",
            )?;
            let schema = schema(connection, self.layout, origin, self.budget)?;
            *self.schema.borrow_mut() = Some(schema);
            Ok(())
        })())
    }
    pub(crate) fn capture_baseline(
        &self,
        connection: &Connection,
        origin: ArchiveSqlOrigin,
    ) -> Result<()> {
        self.checked((|| {
            self.mutable()?;
            require(
                !self.baseline.get() && !self.planned.get() && self.rows.get() == 0,
                "Postimage baseline must precede every planned insert",
            )?;
            let observed = schema(connection, self.layout, origin, self.budget)?;
            if let Some(schema) = self.schema.borrow().as_ref() {
                require(schema == &observed, "Postimage baseline schema changed")?;
            }
            *self.schema.borrow_mut() = Some(observed);
            self.scan(connection, origin, |index, key, hash| {
                self.insert_proof(index, key, hash, false, false)
            })?;
            self.baseline.set(true);
            Ok(())
        })())
    }
    pub(crate) fn expect_insert(
        &self,
        table: &str,
        values: &[ValueRef<'_>],
        reuse_exact: bool,
    ) -> Result<()> {
        self.checked((|| {
            self.mutable()?;
            let (index, table) = self.table(table)?;
            let (key, hash) = proof_values(table, values, self.budget)?;
            let mut raw = 0u64;
            for value in values {
                let bytes = match value {
                    ValueRef::Null => 0,
                    ValueRef::Integer(_) | ValueRef::Real(_) => 8,
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len() as u64,
                };
                raw = raw
                    .checked_add(bytes)
                    .ok_or_else(|| invalid("Postimage raw counter overflow"))?;
            }
            self.add_raw(raw)?;
            self.insert_proof(index, &key, &hash, true, reuse_exact)?;
            self.planned.set(true);
            Ok(())
        })())
    }
    pub(crate) fn expect_from_plan(
        &self,
        source: &Connection,
        origin: ArchiveSqlOrigin,
        reusable_tables: &[&str],
    ) -> Result<()> {
        self.checked((|| {
            self.mutable()?;
            require(
                self.layout == Layout::Catalog,
                "Catalog row plans require the Catalog layout",
            )?;
            for name in reusable_tables {
                require(
                    matches!(
                        *name,
                        "assets"
                            | "native_takes"
                            | "archive_copy_proofs"
                            | "archive_copy_proof_operations"
                    ),
                    "Unsupported reusable postimage table",
                )?;
            }
            schema(source, Layout::Catalog, origin, self.budget)?;
            self.scan(source, origin, |index, key, hash| {
                self.insert_proof(
                    index,
                    key,
                    hash,
                    true,
                    reusable_tables.contains(&self.layout.tables()[index].name),
                )
            })?;
            self.planned.set(true);
            Ok(())
        })())
    }
    fn scan(
        &self,
        connection: &Connection,
        origin: ArchiveSqlOrigin,
        mut visit: impl FnMut(usize, &[u8], &[u8; 32]) -> Result<()>,
    ) -> Result<()> {
        for (index, table) in self.layout.tables().iter().copied().enumerate() {
            let mut statement = sql(
                origin,
                connection.prepare(&format!(
                    "SELECT rowid FROM main.\"{}\" NOT INDEXED ORDER BY rowid",
                    table.name
                )),
            )?;
            let mut rows = sql(origin, statement.query([]))?;
            loop {
                row_credit(self.budget)?;
                let Some(row) = sql(origin, rows.next())? else {
                    break;
                };
                self.observe()?;
                let rowid: i64 = sql(origin, row.get(0))?;
                let (key, hash, raw) = proof_row(connection, table, rowid, origin, self.budget)?;
                self.add_raw(raw)?;
                visit(index, &key, &hash)?;
            }
        }
        Ok(())
    }
    fn digests(&self, table: &str) -> Result<Vec<TableDigest>> {
        require(
            table == "expected" || table == "observed",
            "Unsupported postimage spool index",
        )?;
        let mut output = Vec::new();
        for index in 0..self.layout.tables().len() {
            let mut statement = sql(
                ArchiveSqlOrigin::PrivateOutput,
                self.connection
                    .prepare(&format!("SELECT k,h FROM {table} WHERE t=?1 ORDER BY k")),
            )?;
            let mut rows = sql(
                ArchiveSqlOrigin::PrivateOutput,
                statement.query([index as i64]),
            )?;
            let mut count = 0u64;
            let mut digest = Sha256::new();
            digest.update(b"OmniuSong.archive-postimage-table.v1\0");
            framed(&mut digest, self.layout.tables()[index].name.as_bytes());
            loop {
                row_credit(self.budget)?;
                credit(self.budget, (MAX_KEY + 32) as u64)?;
                let Some(row) = sql(ArchiveSqlOrigin::PrivateOutput, rows.next())? else {
                    break;
                };
                let key: Vec<u8> = sql(ArchiveSqlOrigin::PrivateOutput, row.get(0))?;
                let hash: Vec<u8> = sql(ArchiveSqlOrigin::PrivateOutput, row.get(1))?;
                require(
                    key.len() <= MAX_KEY && hash.len() == 32,
                    "Postimage spool row exceeds its static encoding",
                )?;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| invalid("Postimage digest row counter overflow"))?;
                require(
                    count <= MAX_ROWS,
                    "Postimage digest row count exceeds its ceiling",
                )?;
                framed(&mut digest, &key);
                framed(&mut digest, &hash);
            }
            output.push(TableDigest {
                rows: count,
                sha256: digest.finalize().into(),
            });
        }
        Ok(output)
    }
    pub(crate) fn freeze(self) -> Result<ExpectedPostimage<'b>> {
        let result = (|| {
            self.mutable()?;
            require(
                self.schema.borrow().is_some(),
                "Postimage schema must be captured before freeze",
            )?;
            self.spool_identity()?;
            let digests = self.digests("expected")?;
            self.frozen.set(true);
            Ok(digests)
        })();
        let digests = self.checked(result)?;
        Ok(ExpectedPostimage {
            ledger: self,
            digests,
        })
    }
}
impl ExpectedPostimage<'_> {
    pub(crate) fn verify(self, connection: &Connection, origin: ArchiveSqlOrigin) -> Result<()> {
        let ledger = &self.ledger;
        ledger.checked((|| {
            checkpoint(ledger.budget)?;
            ledger.spool_identity()?;
            let actual = schema(connection, ledger.layout, origin, ledger.budget)?;
            require(
                ledger.schema.borrow().as_ref() == Some(&actual),
                "Postimage schema changed after planning",
            )?;
            ledger.scan(connection, origin, |index, key, hash| {
                ledger.observe()?;
                credit(ledger.budget, (key.len() + 64) as u64)?;
                let duplicate: bool = sql(
                    ArchiveSqlOrigin::PrivateOutput,
                    ledger.connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM observed WHERE t=?1 AND k=?2)",
                        params![index as i64, key],
                        |row| row.get(0),
                    ),
                )?;
                require(!duplicate, "Actual postimage repeats a typed primary key")?;
                sql(
                    ArchiveSqlOrigin::PrivateOutput,
                    ledger.connection.execute(
                        "INSERT INTO observed VALUES(?1,?2,?3)",
                        params![index as i64, key, hash.as_slice()],
                    ),
                )?;
                Ok(())
            })?;
            require(
                ledger.digests("observed")? == self.digests,
                "Final SQL rows differ from the frozen expected postimage",
            )?;
            ledger.spool_identity()?;
            Ok(())
        })())
    }
}

#[cfg(test)]
mod tests;
