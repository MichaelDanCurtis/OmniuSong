use crate::types::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

type BoundaryHook = Option<Box<dyn FnMut(WriteBoundary) -> Result<()> + Send>>;

pub(crate) fn sql_integer(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        RecordingError::Invalid("Recording integer exceeds SQLite signed range".into())
    })
}

pub(crate) fn read_unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

/// One writer/recovery owner per canonical journal path. Moving to a writer thread
/// is supported; sharing this object with a callback is deliberately unnecessary.
pub struct Journal {
    connection: Connection,
    descriptor: TakeDescriptor,
    path: PathBuf,
    hook: BoundaryHook,
    _owner: File,
}

const SCHEMA: &str = "
CREATE TABLE take_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),descriptor BLOB NOT NULL,descriptor_sha256 TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('recording','interrupted','sealed')));
CREATE TABLE stream_prefix(stream_id TEXT PRIMARY KEY,next_sequence INTEGER NOT NULL CHECK(next_sequence>=0),source_end INTEGER NOT NULL CHECK(source_end>=0),recorded_units INTEGER NOT NULL CHECK(recorded_units>=0),gap_units INTEGER NOT NULL CHECK(gap_units>=0),gap_chunks INTEGER NOT NULL CHECK(gap_chunks>=0));
CREATE TABLE chunks(ordinal INTEGER PRIMARY KEY AUTOINCREMENT,stream_id TEXT NOT NULL,sequence INTEGER NOT NULL CHECK(sequence>=0),header BLOB NOT NULL CHECK(length(header)<=65536),payload BLOB NOT NULL CHECK(length(payload)<=1048576),sha256 TEXT NOT NULL CHECK(length(sha256)=64),UNIQUE(stream_id,sequence));
CREATE TABLE interruptions(singleton INTEGER PRIMARY KEY CHECK(singleton=1),receipt BLOB NOT NULL,sha256 TEXT NOT NULL);
CREATE TABLE recovery_index(ordinal INTEGER PRIMARY KEY CHECK(ordinal>0),sha256 TEXT NOT NULL CHECK(length(sha256)=64));
CREATE TABLE sealed_manifests(singleton INTEGER PRIMARY KEY CHECK(singleton=1),operation_id TEXT NOT NULL,body BLOB NOT NULL,sha256 TEXT NOT NULL);
CREATE TRIGGER immutable_chunk_update BEFORE UPDATE ON chunks BEGIN SELECT RAISE(ABORT,'Original chunks are immutable'); END;
CREATE TRIGGER immutable_chunk_delete BEFORE DELETE ON chunks BEGIN SELECT RAISE(ABORT,'Original chunks are immutable'); END;
CREATE TRIGGER chunk_only_recording BEFORE INSERT ON chunks WHEN (SELECT state FROM take_meta WHERE singleton=1)!='recording' BEGIN SELECT RAISE(ABORT,'Take is not recording'); END;
CREATE TRIGGER immutable_descriptor BEFORE UPDATE OF descriptor,descriptor_sha256 ON take_meta BEGIN SELECT RAISE(ABORT,'Original descriptor is immutable'); END;
CREATE TRIGGER immutable_take_delete BEFORE DELETE ON take_meta BEGIN SELECT RAISE(ABORT,'Original take is immutable'); END;
CREATE TRIGGER immutable_interruption_update BEFORE UPDATE ON interruptions BEGIN SELECT RAISE(ABORT,'Interruption receipt is immutable'); END;
CREATE TRIGGER immutable_interruption_delete BEFORE DELETE ON interruptions BEGIN SELECT RAISE(ABORT,'Interruption receipt is immutable'); END;
CREATE TRIGGER immutable_recovery_index_update BEFORE UPDATE ON recovery_index BEGIN SELECT RAISE(ABORT,'Interrupted index is immutable'); END;
CREATE TRIGGER immutable_recovery_index_delete BEFORE DELETE ON recovery_index BEGIN SELECT RAISE(ABORT,'Interrupted index is immutable'); END;
CREATE TRIGGER immutable_manifest_update BEFORE UPDATE ON sealed_manifests BEGIN SELECT RAISE(ABORT,'Sealed manifest is immutable'); END;
CREATE TRIGGER immutable_manifest_delete BEFORE DELETE ON sealed_manifests BEGIN SELECT RAISE(ABORT,'Sealed manifest is immutable'); END;
PRAGMA user_version=1;";

fn configure(connection: &Connection) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;")?;
    let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    let synchronous: u32 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    require(
        mode.eq_ignore_ascii_case("delete") && synchronous == 2,
        "Recording requires rollback journal and synchronous FULL",
    )
}

fn owner(path: &Path) -> Result<File> {
    let mut name = path.as_os_str().to_owned();
    name.push(".owner.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(PathBuf::from(name))?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => RecordingError::Owned,
        std::fs::TryLockError::Error(error) => RecordingError::Io(error),
    })?;
    Ok(file)
}

pub(crate) fn reject_hardlinks(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            fs::metadata(path)?.nlink() == 1,
            "Hard-linked take journals are unsupported; live ownership was not changed",
        )?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(crate) fn state(connection: &Connection) -> Result<TakeState> {
    let value: String = connection.query_row(
        "SELECT substr(state,1,12) FROM take_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    match value.as_str() {
        "recording" => Ok(TakeState::Recording),
        "interrupted" => Ok(TakeState::Interrupted),
        "sealed" => Ok(TakeState::Sealed),
        _ => Err(RecordingError::Invalid("Invalid stored take state".into())),
    }
}

fn prefix(connection: &Connection, stream: &StreamDescriptor) -> Result<StreamPrefix> {
    let mut value = StreamPrefix::initial(stream);
    let (sequence,end,recorded,gaps,gap_chunks):(u64,u64,u64,u64,u64)=connection.query_row(
        "SELECT next_sequence,source_end,recorded_units,gap_units,gap_chunks FROM stream_prefix WHERE stream_id=?1",[&stream.stream_id],|row|Ok((read_unsigned(row,0)?,read_unsigned(row,1)?,read_unsigned(row,2)?,read_unsigned(row,3)?,read_unsigned(row,4)?)))?;
    value.next_sequence = sequence;
    value.source_end = match stream.kind {
        StreamKind::Audio { .. } => SourcePosition::AudioFrames(end),
        StreamKind::RawEvents { .. } => SourcePosition::Events(end),
    };
    value.recorded_units = recorded;
    value.gap_units = gaps;
    value.gap_chunks = gap_chunks;
    require(
        end.checked_sub(stream.initial_source_position) == recorded.checked_add(gaps)
            && gap_chunks <= sequence,
        "Stored durable prefix is inconsistent",
    )?;
    Ok(value)
}

fn store_prefix(connection: &Connection, value: &StreamPrefix) -> Result<()> {
    connection.execute("UPDATE stream_prefix SET next_sequence=?2,source_end=?3,recorded_units=?4,gap_units=?5,gap_chunks=?6 WHERE stream_id=?1",params![value.stream_id,sql_integer(value.next_sequence)?,sql_integer(value.source_end.value())?,sql_integer(value.recorded_units)?,sql_integer(value.gap_units)?,sql_integer(value.gap_chunks)?])?;
    Ok(())
}

pub(crate) fn load_descriptor(connection: &Connection) -> Result<TakeDescriptor> {
    let (len, hash): (u64, String) = connection.query_row(
        "SELECT length(descriptor),substr(descriptor_sha256,1,65) FROM take_meta WHERE singleton=1",
        [],
        |row| Ok((read_unsigned(row, 0)?, row.get(1)?)),
    )?;
    require(
        len <= MAX_METADATA_BYTES as u64,
        "Stored descriptor exceeds byte limit",
    )?;
    let bytes: Vec<u8> = connection.query_row(
        "SELECT descriptor FROM take_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let descriptor: TakeDescriptor = decode(&bytes, MAX_METADATA_BYTES)?;
    descriptor.validate()?;
    require(
        descriptor.sha256()? == hash && bounded_json(&descriptor, MAX_METADATA_BYTES)? == bytes,
        "Stored descriptor hash/canonical interpretation mismatch",
    )?;
    Ok(descriptor)
}

pub(crate) fn load_chunk(
    connection: &Connection,
    take: &TakeDescriptor,
    stream: &str,
    sequence: u64,
) -> Result<ChunkRecord> {
    let (header_len, payload_len): (u64, u64) = connection.query_row(
        "SELECT length(header),length(payload) FROM chunks WHERE stream_id=?1 AND sequence=?2",
        params![stream, sql_integer(sequence)?],
        |row| Ok((read_unsigned(row, 0)?, read_unsigned(row, 1)?)),
    )?;
    require(
        header_len <= MAX_HEADER_BYTES as u64 && payload_len <= MAX_AUDIO_CHUNK_BYTES as u64,
        "Stored chunk exceeds byte limit",
    )?;
    let bytes: Vec<u8> = connection.query_row(
        "SELECT header FROM chunks WHERE stream_id=?1 AND sequence=?2",
        params![stream, sql_integer(sequence)?],
        |row| row.get(0),
    )?;
    let header: ChunkHeader = decode(&bytes, MAX_HEADER_BYTES)?;
    let descriptor = take
        .streams
        .iter()
        .find(|descriptor| descriptor.stream_id == stream)
        .ok_or_else(|| RecordingError::Invalid("Unknown stored recording stream".into()))?;
    let maximum = match (&header.kind, &descriptor.kind) {
        (ChunkKind::Gap { .. }, _) => 0,
        (_, StreamKind::Audio { .. }) => MAX_AUDIO_CHUNK_BYTES,
        (_, StreamKind::RawEvents { .. }) => MAX_EVENT_CHUNK_BYTES,
    };
    require(
        payload_len <= maximum as u64,
        "Stored chunk exceeds its interpretation's byte limit",
    )?;
    let (payload, hash): (Vec<u8>, String) = connection.query_row(
        "SELECT payload,substr(sha256,1,65) FROM chunks WHERE stream_id=?1 AND sequence=?2",
        params![stream, sql_integer(sequence)?],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    require(
        header.stream_id == stream && header.sequence == sequence,
        "Stored chunk index/header mismatch",
    )?;
    header.validate(take, &payload)?;
    require(
        header.canonical_bytes()? == bytes && header.sha256(&payload)? == hash,
        "Stored chunk header/content hash mismatch",
    )?;
    Ok(ChunkRecord {
        header,
        payload,
        sha256: hash,
    })
}

fn problem(
    report: &mut RecoveryReport,
    stream: Option<String>,
    sequence: Option<u64>,
    message: impl Into<String>,
) {
    if report.problems.len() < MAX_STREAMS + 1 {
        report.problems.push(RecoveryProblem {
            stream_id: stream,
            sequence,
            message: message.into(),
        });
    }
    report.unknown_tail = true;
}

/// One chunk at a time; no list of chunk payloads/headers is accumulated.
pub(crate) fn scan(
    connection: &Connection,
    take: &TakeDescriptor,
    limit: Option<u64>,
) -> Result<RecoveryReport> {
    let current_state = state(connection)?;
    let mut report = RecoveryReport {
        take_id: take.take_id.clone(),
        state: current_state,
        streams: take.streams.iter().map(StreamPrefix::initial).collect(),
        verified_chunks: 0,
        verified_payload_bytes: 0,
        chunk_index_sha256: String::new(),
        problems: vec![],
        unknown_tail: current_state != TakeState::Sealed,
    };
    let indices: BTreeMap<&str, usize> = take
        .streams
        .iter()
        .enumerate()
        .map(|(index, stream)| (stream.stream_id.as_str(), index))
        .collect();
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.recording-index.v1\0");
    hash.update(take.sha256()?.as_bytes());
    let mut statement=connection.prepare("SELECT ordinal,substr(stream_id,1,97),sequence,length(header),length(payload) FROM chunks WHERE ordinal<=?1 ORDER BY ordinal")?;
    let mut rows = statement.query([sql_integer(limit.unwrap_or(i64::MAX as u64))?])?;
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => {
                problem(&mut report, None, None, error.to_string());
                break;
            }
        };
        let values: (u64, String, u64, u64, u64) = match (|| -> rusqlite::Result<_> {
            Ok((
                read_unsigned(row, 0)?,
                row.get(1)?,
                read_unsigned(row, 2)?,
                read_unsigned(row, 3)?,
                read_unsigned(row, 4)?,
            ))
        })() {
            Ok(values) => values,
            Err(error) => {
                problem(&mut report, None, None, error.to_string());
                break;
            }
        };
        let (ordinal, stream, sequence, header_len, payload_len) = values;
        let result = (|| -> Result<usize> {
            require(
                ordinal == report.verified_chunks + 1,
                "Missing/reordered journal chunk ordinal",
            )?;
            require(
                header_len <= MAX_HEADER_BYTES as u64
                    && payload_len <= MAX_AUDIO_CHUNK_BYTES as u64,
                "Stored chunk exceeds byte limit",
            )?;
            let index = *indices
                .get(stream.as_str())
                .ok_or_else(|| RecordingError::Invalid("Unknown stored recording stream".into()))?;
            let chunk = load_chunk(connection, take, &stream, sequence)?;
            let mut next = report.streams[index].clone();
            next.advance(&chunk.header)?;
            report.streams[index] = next;
            hash.update(ordinal.to_le_bytes());
            hash.update(chunk.sha256.as_bytes());
            Ok(chunk.payload.len())
        })();
        match result {
            Ok(bytes) => {
                report.verified_chunks += 1;
                report.verified_payload_bytes += bytes as u64;
            }
            Err(error) => {
                problem(&mut report, Some(stream), Some(sequence), error.to_string());
                break;
            }
        }
    }
    drop(rows);
    drop(statement);
    if report.problems.is_empty() {
        if let Some(limit) = limit {
            if report.verified_chunks != limit {
                problem(
                    &mut report,
                    None,
                    None,
                    "Sealed manifest references missing journal chunks",
                );
            }
        } else {
            for (index, stream) in take.streams.iter().enumerate() {
                match prefix(connection, stream) {
                    Ok(cached) if cached == report.streams[index] => {}
                    Ok(_) => {
                        problem(
                            &mut report,
                            Some(stream.stream_id.clone()),
                            None,
                            "Durable prefix differs from verified chunks",
                        );
                        break;
                    }
                    Err(error) => {
                        problem(
                            &mut report,
                            Some(stream.stream_id.clone()),
                            None,
                            error.to_string(),
                        );
                        break;
                    }
                }
            }
        }
    }
    report.chunk_index_sha256 = format!("{:x}", hash.finalize());
    Ok(report)
}

fn interrupted_prefix_hash(
    connection: &Connection,
    take: &TakeDescriptor,
    count: u64,
) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.recording-index.v1\0");
    hash.update(take.sha256()?.as_bytes());
    let mut statement = connection.prepare(
        "SELECT ordinal,substr(sha256,1,65) FROM recovery_index WHERE ordinal<=?1 ORDER BY ordinal",
    )?;
    let mut rows = statement.query([sql_integer(count)?])?;
    let mut seen = 0;
    while let Some(row) = rows.next()? {
        let ordinal = read_unsigned(row, 0)?;
        let sha: String = row.get(1)?;
        require(
            ordinal == seen + 1
                && sha.len() == 64
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Interrupted index is missing or damaged",
        )?;
        hash.update(ordinal.to_le_bytes());
        hash.update(sha.as_bytes());
        seen += 1;
    }
    require(seen == count, "Interrupted index is truncated")?;
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn load_interrupted(connection: &Connection) -> Result<InterruptedReceipt> {
    let length: u64 = connection.query_row(
        "SELECT length(receipt) FROM interruptions WHERE singleton=1",
        [],
        |row| read_unsigned(row, 0),
    )?;
    require(
        length <= MAX_METADATA_BYTES as u64,
        "Interruption receipt exceeds byte limit",
    )?;
    let (bytes, stored): (Vec<u8>, String) = connection.query_row(
        "SELECT receipt,substr(sha256,1,65) FROM interruptions WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let receipt: InterruptedReceipt = decode(&bytes, MAX_METADATA_BYTES)?;
    let hash = digest(
        b"OmniuSong.interrupted-prefix.v1\0",
        &bounded_json(
            &(receipt.receipt_id.clone(), &receipt.report),
            MAX_METADATA_BYTES,
        )?,
    );
    require(
        receipt.sha256 == hash
            && stored == hash
            && receipt.report.state == TakeState::Interrupted
            && receipt.report.unknown_tail,
        "Interrupted receipt is inconsistent",
    )?;
    let take = load_descriptor(connection)?;
    require(
        receipt.report.take_id == take.take_id
            && interrupted_prefix_hash(connection, &take, receipt.report.verified_chunks)?
                == receipt.report.chunk_index_sha256,
        "Interrupted receipt differs from its immutable original chunk index",
    )?;
    Ok(receipt)
}

pub(crate) fn verify_sealed(
    connection: &Connection,
    take: &TakeDescriptor,
) -> Result<SealedManifest> {
    require(
        state(connection)? == TakeState::Sealed,
        "Take is not sealed",
    )?;
    let length: u64 = connection.query_row(
        "SELECT length(body) FROM sealed_manifests WHERE singleton=1",
        [],
        |row| read_unsigned(row, 0),
    )?;
    require(
        length <= (MAX_METADATA_BYTES * 2) as u64,
        "Sealed manifest exceeds byte limit",
    )?;
    let (bytes,stored,operation):(Vec<u8>,String,String)=connection.query_row("SELECT body,substr(sha256,1,65),substr(operation_id,1,97) FROM sealed_manifests WHERE singleton=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
    let manifest: SealedManifest = decode(&bytes, MAX_METADATA_BYTES * 2)?;
    manifest.verify_hash()?;
    require(
        manifest.sha256 == stored
            && manifest.body.operation_id == operation
            && manifest.body.descriptor == *take
            && manifest.canonical_bytes()? == bytes,
        "Stored sealed manifest differs from journal identity",
    )?;
    let report = scan(connection, take, Some(manifest.body.chunk_count))?;
    require(
        report.problems.is_empty()
            && report.streams == manifest.body.streams
            && report.verified_payload_bytes == manifest.body.payload_bytes
            && report.chunk_index_sha256 == manifest.body.chunk_index_sha256,
        "Sealed manifest references an invalid original prefix",
    )?;
    if let Some(interrupted) = &manifest.body.interrupted {
        require(
            *interrupted == load_interrupted(connection)?,
            "Sealed recovery manifest lost its original interruption receipt",
        )?;
        require(
            manifest.body.chunk_count <= interrupted.report.verified_chunks,
            "Recovered manifest exceeds its interrupted proven prefix",
        )?;
        require(
            manifest.body.chunk_index_sha256
                == interrupted_prefix_hash(connection, take, manifest.body.chunk_count)?,
            "Recovered manifest differs from its original interrupted prefix",
        )?;
    } else {
        let count: u64 = connection.query_row("SELECT count(*) FROM chunks", [], |row| {
            read_unsigned(row, 0)
        })?;
        require(
            count == manifest.body.chunk_count,
            "Unreferenced chunks in a normally sealed take",
        )?;
        let complete = scan(connection, take, None)?;
        require(
            complete.problems.is_empty(),
            "Sealed take durable prefix is inconsistent",
        )?;
    }
    Ok(manifest)
}

fn invoke(hook: &mut BoundaryHook, boundary: WriteBoundary) -> Result<()> {
    if let Some(hook) = hook {
        hook(boundary)?;
    }
    Ok(())
}

enum Finalization<'a> {
    Normal,
    Recovered {
        original_rows: Option<&'a str>,
        prefix: Option<&'a RecoveryPrefix>,
    },
    Complete(&'a CompletePrefix),
}

fn require_performance_v2(take: &TakeDescriptor) -> Result<()> {
    let owned: Vec<_> = take
        .provenance
        .iter()
        .filter(|(key, _)| key.starts_with("omniusong.performance."))
        .collect();
    require(
        owned.len() == 1 && owned[0].0 == "omniusong.performance.v2",
        "Complete normal prefix requires only supported version-2 performance ownership",
    )?;
    let value = owned[0].1.as_object().ok_or_else(|| {
        RecordingError::Invalid("Invalid version-2 performance configuration".into())
    })?;
    require(
        value.len() == 2
            && value.get("version").and_then(serde_json::Value::as_u64) == Some(2)
            && value
                .get("config")
                .is_some_and(serde_json::Value::is_object),
        "Complete normal prefix requires a version-2 performance configuration object",
    )
}

/// Checks the whole physical original, never a selected/recovered prefix.
/// The engine separately validates performance records and actual lifetimes.
fn complete_snapshot(connection: &Connection, take: &TakeDescriptor) -> Result<CompletePrefix> {
    let current_descriptor = load_descriptor(connection)?;
    require(
        current_descriptor == *take,
        "Original descriptor changed after journal acquisition",
    )?;
    require_performance_v2(&current_descriptor)?;
    match state(connection)? {
        TakeState::Recording => {}
        TakeState::Sealed => {
            require(
                verify_sealed(connection, take)?.body.interrupted.is_none(),
                "Recovered original cannot become a normally completed take",
            )?;
        }
        TakeState::Interrupted => return Err(RecordingError::Interrupted),
    }
    let derived: (u64, u64) = connection.query_row(
        "SELECT (SELECT count(*) FROM interruptions),(SELECT count(*) FROM recovery_index)",
        [],
        |row| Ok((read_unsigned(row, 0)?, read_unsigned(row, 1)?)),
    )?;
    require(
        derived == (0, 0),
        "Normal completion cannot retain recovered-mode metadata",
    )?;
    let report = scan(connection, take, None)?;
    let (physical_chunks, physical_streams): (u64, u64) = connection.query_row(
        "SELECT (SELECT count(*) FROM chunks),(SELECT count(*) FROM stream_prefix)",
        [],
        |row| Ok((read_unsigned(row, 0)?, read_unsigned(row, 1)?)),
    )?;
    require(
        report.problems.is_empty()
            && report.verified_chunks == physical_chunks
            && physical_streams == take.streams.len() as u64
            && report
                .streams
                .iter()
                .all(|prefix| prefix.gap_units == 0 && prefix.gap_chunks == 0),
        "Complete normal prefix requires every original chunk and stream prefix without gaps or damaged tails",
    )?;
    let receipt = CompletePrefix {
        descriptor_sha256: current_descriptor.sha256()?,
        original_rows_sha256: crate::inspection::original_rows_hash(connection)?,
        prefix: RecoveryPrefix {
            chunk_count: report.verified_chunks,
            chunk_index_sha256: report.chunk_index_sha256,
        },
    };
    receipt.validate()?;
    Ok(receipt)
}

impl Journal {
    pub fn create(path: impl AsRef<Path>, descriptor: &TakeDescriptor) -> Result<Self> {
        descriptor.validate()?;
        let path = path.as_ref();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let path = fs::canonicalize(parent)?.join(
            path.file_name()
                .ok_or_else(|| RecordingError::Invalid("Take journal needs a file name".into()))?,
        );
        let lock = owner(&path)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        reject_hardlinks(&path)?;
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure(&connection)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        tx.execute(
            "INSERT INTO take_meta VALUES(1,?1,?2,'recording')",
            params![
                bounded_json(descriptor, MAX_METADATA_BYTES)?,
                descriptor.sha256()?
            ],
        )?;
        for stream in &descriptor.streams {
            tx.execute(
                "INSERT INTO stream_prefix VALUES(?1,0,?2,0,0,0)",
                params![
                    stream.stream_id,
                    sql_integer(stream.initial_source_position)?
                ],
            )?;
        }
        tx.commit()?;
        #[cfg(unix)]
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok(Self {
            connection,
            descriptor: descriptor.clone(),
            path,
            hook: None,
            _owner: lock,
        })
    }

    /// Acquires exclusive OS ownership. A live owner causes `Owned`, without
    /// changing state. An unfinished journal is interrupted, never resumed.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = fs::canonicalize(path)?;
        reject_hardlinks(&path)?;
        let lock = owner(&path)?;
        Self::open_owned(path, lock, None)
    }
    pub(crate) fn open_owned(
        path: PathBuf,
        lock: File,
        expected: Option<&crate::inspection::JournalInspection>,
    ) -> Result<Self> {
        reject_hardlinks(&path)?;
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        require(
            version == JOURNAL_VERSION,
            "Unsupported recording journal version",
        )?;
        configure(&connection)?;
        let descriptor = load_descriptor(&connection)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(expected) = expected {
            let current = crate::inspection::inspect_connection(&tx, &path, &descriptor)?;
            require(
                current == *expected,
                "Capture changed after reviewed read snapshot; review it again",
            )?;
        }
        match state(&tx)? {
            TakeState::Recording => {
                reject_hardlinks(&path)?;
                let mut report = scan(&tx, &descriptor, None)?;
                report.state = TakeState::Interrupted;
                report.unknown_tail = true;
                let receipt_id = uuid::Uuid::new_v4().to_string();
                let sha256 = digest(
                    b"OmniuSong.interrupted-prefix.v1\0",
                    &bounded_json(&(receipt_id.clone(), &report), MAX_METADATA_BYTES)?,
                );
                let receipt = InterruptedReceipt {
                    receipt_id,
                    report,
                    sha256: sha256.clone(),
                };
                tx.execute("INSERT INTO recovery_index(ordinal,sha256) SELECT ordinal,sha256 FROM chunks WHERE ordinal<=?1 ORDER BY ordinal",[sql_integer(receipt.report.verified_chunks)?])?;
                tx.execute(
                    "INSERT INTO interruptions VALUES(1,?1,?2)",
                    params![bounded_json(&receipt, MAX_METADATA_BYTES)?, sha256],
                )?;
                tx.execute(
                    "UPDATE take_meta SET state='interrupted' WHERE singleton=1",
                    [],
                )?;
            }
            TakeState::Interrupted => {
                load_interrupted(&tx)?;
            }
            TakeState::Sealed => {
                verify_sealed(&tx, &descriptor)?;
            }
        }
        if let Some(expected) = expected {
            require(
                crate::inspection::original_rows_hash(&tx)? == expected.original_rows_sha256,
                "Recovery acquisition changed original rows; transaction rolled back",
            )?;
        }
        tx.commit()?;
        Ok(Self {
            connection,
            descriptor,
            path,
            hook: None,
            _owner: lock,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn descriptor(&self) -> &TakeDescriptor {
        &self.descriptor
    }
    /// Read a consistent snapshot while retaining this journal's exclusive
    /// owner. The borrow prevents writes or owner release until the view drops.
    pub fn inspect_owned(&self) -> Result<crate::inspection::OwnedJournalView<'_>> {
        crate::inspection::OwnedJournalView::new(&self.connection, &self.path, &self.descriptor)
    }
    pub fn state(&self) -> Result<TakeState> {
        state(&self.connection)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_boundary_hook(
        &mut self,
        hook: impl FnMut(WriteBoundary) -> Result<()> + Send + 'static,
    ) {
        self.hook = Some(Box::new(hook));
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn clear_boundary_hook(&mut self) {
        self.hook = None;
    }

    pub fn read_chunk(&self, stream_id: &str, sequence: u64) -> Result<ChunkRecord> {
        load_chunk(&self.connection, &self.descriptor, stream_id, sequence)
    }

    /// Atomically append 1..=64 chunks, up to 8 MiB of canonical headers and
    /// payloads. All-old exact consecutive originals replay; mixed old/new
    /// requests conflict. The content digest is not a historical commit proof.
    /// Existing chunk hooks fire per insert, then once before/after group commit.
    pub fn append_group(&mut self, chunks: &[AppendChunk<'_>]) -> Result<AppendGroupReceipt> {
        require(
            !chunks.is_empty() && chunks.len() <= MAX_APPEND_GROUP_CHUNKS,
            "Append group must contain 1..64 chunks",
        )?;
        let mut total = 0usize;
        let mut prepared = Vec::with_capacity(chunks.len());
        let mut addresses = std::collections::BTreeSet::new();
        let mut group_hash = Sha256::new();
        group_hash.update(b"OmniuSong.recording-append-group.v1\0");
        group_hash.update(self.descriptor.sha256()?.as_bytes());
        // Complete input validation and byte accounting precede any SQL mutation.
        for chunk in chunks {
            let bytes = chunk.header.canonical_bytes()?;
            total = total
                .checked_add(bytes.len())
                .and_then(|v| v.checked_add(chunk.payload.len()))
                .filter(|v| *v <= MAX_APPEND_GROUP_BYTES)
                .ok_or_else(|| {
                    RecordingError::Invalid(
                        "Append group exceeds 8 MiB of canonical headers and payloads".into(),
                    )
                })?;
            chunk.header.validate(&self.descriptor, chunk.payload)?;
            if !addresses.insert((chunk.header.stream_id.as_str(), chunk.header.sequence)) {
                return Err(RecordingError::Conflict(
                    "Append group repeats an immutable chunk address".into(),
                ));
            }
            let sha256 = chunk.header.sha256(chunk.payload)?;
            group_hash.update(sha256.as_bytes());
            prepared.push((bytes, sha256));
        }
        let group_sha256 = format!("{:x}", group_hash.finalize());
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ordinals = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let ordinal = tx
                .query_row(
                    "SELECT ordinal FROM chunks WHERE stream_id=?1 AND sequence=?2",
                    params![chunk.header.stream_id, sql_integer(chunk.header.sequence)?],
                    |row| read_unsigned(row, 0),
                )
                .optional()?;
            ordinals.push(ordinal);
        }
        let old = ordinals.iter().filter(|ordinal| ordinal.is_some()).count();
        if old != 0 && old != chunks.len() {
            return Err(RecordingError::Conflict(
                "Append group mixes old and new immutable chunks".into(),
            ));
        }
        if old == chunks.len() {
            let first_ordinal = ordinals[0].expect("all chunks exist");
            for (index, ordinal) in ordinals.iter().enumerate() {
                if first_ordinal.checked_add(index as u64) != *ordinal {
                    return Err(RecordingError::Conflict(
                        "Append group retry is not in consecutive global original order".into(),
                    ));
                }
            }
            let report = match state(&tx)? {
                TakeState::Sealed => {
                    let manifest = verify_sealed(&tx, &self.descriptor)?;
                    scan(&tx, &self.descriptor, Some(manifest.body.chunk_count))?
                }
                TakeState::Interrupted => {
                    let receipt = load_interrupted(&tx)?;
                    let report = scan(&tx, &self.descriptor, Some(receipt.report.verified_chunks))?;
                    require(
                        report.chunk_index_sha256
                            == interrupted_prefix_hash(
                                &tx,
                                &self.descriptor,
                                report.verified_chunks,
                            )?,
                        "Group retry differs from its interrupted original prefix",
                    )?;
                    report
                }
                TakeState::Recording => scan(&tx, &self.descriptor, None)?,
            };
            let last_ordinal = ordinals
                .last()
                .copied()
                .flatten()
                .expect("nonempty old group");
            require(
                first_ordinal > 0 && last_ordinal <= report.verified_chunks,
                "Group retry lies outside the proven original prefix",
            )?;
            let mut receipts = Vec::with_capacity(chunks.len());
            for (chunk, (bytes, sha256)) in chunks.iter().zip(&prepared) {
                let stored = load_chunk(
                    &tx,
                    &self.descriptor,
                    &chunk.header.stream_id,
                    chunk.header.sequence,
                )?;
                if stored.sha256 != *sha256
                    || stored.header.canonical_bytes()? != *bytes
                    || stored.payload != chunk.payload
                {
                    return Err(RecordingError::Conflict(
                        "Conflicting retry of immutable append group".into(),
                    ));
                }
                let durable = report
                    .streams
                    .iter()
                    .find(|prefix| prefix.stream_id == chunk.header.stream_id)
                    .expect("validated stream")
                    .clone();
                require(
                    chunk.header.sequence < durable.next_sequence,
                    "Group retry lies outside a proven stream prefix",
                )?;
                receipts.push(ChunkReceipt {
                    take_id: self.descriptor.take_id.clone(),
                    stream_id: chunk.header.stream_id.clone(),
                    sequence: chunk.header.sequence,
                    sha256: sha256.clone(),
                    durable_prefix: durable,
                    replayed: true,
                });
            }
            tx.commit()?;
            return Ok(AppendGroupReceipt {
                group_sha256,
                first_ordinal,
                last_ordinal,
                chunks: receipts,
                replayed: true,
            });
        }
        match state(&tx)? {
            TakeState::Interrupted => return Err(RecordingError::Interrupted),
            TakeState::Sealed => {
                return Err(RecordingError::Invalid(
                    "Sealed take cannot append original data".into(),
                ));
            }
            TakeState::Recording => {}
        }
        let mut prefixes = BTreeMap::new();
        for chunk in chunks {
            if !prefixes.contains_key(&chunk.header.stream_id) {
                let stream = self
                    .descriptor
                    .streams
                    .iter()
                    .find(|stream| stream.stream_id == chunk.header.stream_id)
                    .expect("validated stream");
                prefixes.insert(chunk.header.stream_id.clone(), prefix(&tx, stream)?);
            }
            prefixes
                .get_mut(&chunk.header.stream_id)
                .expect("simulated stream")
                .advance(chunk.header)?;
        }
        let previous: u64 =
            tx.query_row("SELECT COALESCE(MAX(ordinal),0) FROM chunks", [], |row| {
                read_unsigned(row, 0)
            })?;
        let first_ordinal = previous
            .checked_add(1)
            .ok_or_else(|| RecordingError::Invalid("Append group ordinal overflow".into()))?;
        let last_ordinal = previous
            .checked_add(chunks.len() as u64)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or_else(|| {
                RecordingError::Invalid("Append group ordinal exceeds SQLite range".into())
            })?;
        for (index, (chunk, (bytes, sha256))) in chunks.iter().zip(&prepared).enumerate() {
            tx.execute("INSERT INTO chunks(stream_id,sequence,header,payload,sha256) VALUES(?1,?2,?3,?4,?5)", params![chunk.header.stream_id, sql_integer(chunk.header.sequence)?, bytes, chunk.payload, sha256])?;
            require(
                u64::try_from(tx.last_insert_rowid()).ok()
                    == first_ordinal.checked_add(index as u64),
                "Append group assigned a nonconsecutive global ordinal",
            )?;
            invoke(&mut self.hook, WriteBoundary::AfterChunkInsert)?;
        }
        for durable in prefixes.values() {
            store_prefix(&tx, durable)?;
        }
        let receipts = chunks
            .iter()
            .zip(&prepared)
            .map(|(chunk, (_, sha256))| ChunkReceipt {
                take_id: self.descriptor.take_id.clone(),
                stream_id: chunk.header.stream_id.clone(),
                sequence: chunk.header.sequence,
                sha256: sha256.clone(),
                durable_prefix: prefixes[&chunk.header.stream_id].clone(),
                replayed: false,
            })
            .collect();
        invoke(&mut self.hook, WriteBoundary::BeforeChunkCommit)?;
        tx.commit()?;
        invoke(&mut self.hook, WriteBoundary::AfterChunkCommit)?;
        Ok(AppendGroupReceipt {
            group_sha256,
            first_ordinal,
            last_ordinal,
            chunks: receipts,
            replayed: false,
        })
    }

    pub fn append(&mut self, header: &ChunkHeader, payload: &[u8]) -> Result<ChunkReceipt> {
        header.validate(&self.descriptor, payload)?;
        let sha256 = header.sha256(payload)?;
        let header_bytes = header.canonical_bytes()?;
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM chunks WHERE stream_id=?1 AND sequence=?2)",
            params![header.stream_id, sql_integer(header.sequence)?],
            |row| row.get(0),
        )?;
        if exists {
            let stored = self.read_chunk(&header.stream_id, header.sequence)?;
            if stored.sha256 != sha256 || stored.header != *header || stored.payload != payload {
                return Err(RecordingError::Conflict(
                    "Conflicting retry of immutable recording chunk".into(),
                ));
            }
            let report = self.recovery_report()?;
            let durable = report
                .streams
                .iter()
                .find(|prefix| prefix.stream_id == header.stream_id)
                .unwrap()
                .clone();
            require(
                header.sequence < durable.next_sequence,
                "Retry lies outside the proven original prefix",
            )?;
            return Ok(ChunkReceipt {
                take_id: self.descriptor.take_id.clone(),
                stream_id: header.stream_id.clone(),
                sequence: header.sequence,
                sha256,
                durable_prefix: durable,
                replayed: true,
            });
        }
        if state(&self.connection)? == TakeState::Interrupted {
            return Err(RecordingError::Interrupted);
        }
        require(
            state(&self.connection)? == TakeState::Recording,
            "Sealed take cannot append original data",
        )?;
        let stream = self
            .descriptor
            .streams
            .iter()
            .find(|stream| stream.stream_id == header.stream_id)
            .unwrap();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut durable = prefix(&tx, stream)?;
        durable.advance(header)?;
        tx.execute(
            "INSERT INTO chunks(stream_id,sequence,header,payload,sha256) VALUES(?1,?2,?3,?4,?5)",
            params![
                header.stream_id,
                sql_integer(header.sequence)?,
                header_bytes,
                payload,
                sha256
            ],
        )?;
        invoke(&mut self.hook, WriteBoundary::AfterChunkInsert)?;
        store_prefix(&tx, &durable)?;
        invoke(&mut self.hook, WriteBoundary::BeforeChunkCommit)?;
        tx.commit()?;
        invoke(&mut self.hook, WriteBoundary::AfterChunkCommit)?;
        Ok(ChunkReceipt {
            take_id: self.descriptor.take_id.clone(),
            stream_id: header.stream_id.clone(),
            sequence: header.sequence,
            sha256,
            durable_prefix: durable,
            replayed: false,
        })
    }

    pub fn recovery_report(&self) -> Result<RecoveryReport> {
        let tx = self.connection.unchecked_transaction()?;
        let report = if state(&tx)? == TakeState::Sealed {
            let manifest = verify_sealed(&tx, &self.descriptor)?;
            let mut report = scan(&tx, &self.descriptor, Some(manifest.body.chunk_count))?;
            report.unknown_tail = manifest.body.interrupted.is_some();
            report
        } else {
            scan(&tx, &self.descriptor, None)?
        };
        tx.commit()?;
        Ok(report)
    }

    pub fn sealed_manifest(&self) -> Result<Option<SealedManifest>> {
        let tx = self.connection.unchecked_transaction()?;
        let result = if state(&tx)? == TakeState::Sealed {
            Some(verify_sealed(&tx, &self.descriptor)?)
        } else {
            None
        };
        tx.commit()?;
        Ok(result)
    }

    pub fn seal(&mut self, operation_id: &str) -> Result<SealedManifest> {
        self.finish(operation_id, Finalization::Normal)
    }
    pub fn seal_recovered(&mut self, operation_id: &str) -> Result<SealedManifest> {
        self.finish(
            operation_id,
            Finalization::Recovered {
                original_rows: None,
                prefix: None,
            },
        )
    }

    pub(crate) fn seal_recovered_checked(
        &mut self,
        operation_id: &str,
        original_rows: &str,
    ) -> Result<SealedManifest> {
        self.finish(
            operation_id,
            Finalization::Recovered {
                original_rows: Some(original_rows),
                prefix: None,
            },
        )
    }
    pub(crate) fn seal_recovered_prefix_checked(
        &mut self,
        operation_id: &str,
        original_rows: &str,
        selection: &RecoveryPrefix,
    ) -> Result<SealedManifest> {
        self.finish(
            operation_id,
            Finalization::Recovered {
                original_rows: Some(original_rows),
                prefix: Some(selection),
            },
        )
    }
    /// Snapshot all original rows. This trusted low-level receipt does not prove
    /// native Finish, callback destruction, cancellation ordering or seal authority.
    pub fn complete_prefix(&self) -> Result<CompletePrefix> {
        let tx = self.connection.unchecked_transaction()?;
        let complete = complete_snapshot(&tx, &self.descriptor)?;
        tx.commit()?;
        Ok(complete)
    }
    pub fn seal_complete_prefix(
        &mut self,
        operation_id: &str,
        complete: &CompletePrefix,
    ) -> Result<SealedManifest> {
        complete.validate()?;
        self.finish(operation_id, Finalization::Complete(complete))
    }
    fn finish(
        &mut self,
        operation_id: &str,
        finalization: Finalization<'_>,
    ) -> Result<SealedManifest> {
        identifier(operation_id)?;
        let (recovered, original_rows, selected, complete) = match finalization {
            Finalization::Normal => (false, None, None, None),
            Finalization::Recovered {
                original_rows,
                prefix,
            } => (true, original_rows, prefix, None),
            Finalization::Complete(complete) => (
                false,
                Some(complete.original_rows_sha256.as_str()),
                None,
                Some(complete),
            ),
        };
        if let Some(selection) = selected {
            selection.validate()?;
            require(
                recovered,
                "Explicit prefix selection requires recovered finalization",
            )?;
        } else if complete.is_none() {
            require(
                !self.descriptor.provenance.keys().any(|key| {
                    matches!(
                        key.as_str(),
                        "omniusong.performance.v1" | "omniusong.performance.v2"
                    )
                }),
                "Performance originals require an explicit format-aware selected recovery prefix",
            )?;
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = state(&tx)?;
        if let Some(expected) = complete {
            require(
                complete_snapshot(&tx, &self.descriptor)? == *expected,
                "Complete original differs from its admitted full-prefix receipt",
            )?;
        }
        if let Some(expected) = original_rows {
            require(
                crate::inspection::original_rows_hash(&tx)? == expected,
                "Original capture rows changed after recovery admission",
            )?;
        }
        if current == TakeState::Sealed {
            let manifest = verify_sealed(&tx, &self.descriptor)?;
            if manifest.body.operation_id != operation_id
                || manifest.body.interrupted.is_some() != recovered
                || selected.is_some_and(|selection| {
                    manifest.body.chunk_count != selection.chunk_count
                        || manifest.body.chunk_index_sha256 != selection.chunk_index_sha256
                })
                || complete.is_some_and(|expected| {
                    manifest.body.descriptor_sha256 != expected.descriptor_sha256
                        || manifest.body.chunk_count != expected.prefix.chunk_count
                        || manifest.body.chunk_index_sha256 != expected.prefix.chunk_index_sha256
                })
            {
                return Err(RecordingError::Conflict(
                    "Conflicting retry of sealed take finalization".into(),
                ));
            }
            tx.commit()?;
            return Ok(manifest);
        }
        require(
            (!recovered && current == TakeState::Recording)
                || (recovered && current == TakeState::Interrupted),
            "Finalization mode does not match take state",
        )?;
        let report = scan(
            &tx,
            &self.descriptor,
            selected.map(|selection| selection.chunk_count),
        )?;
        if let Some(selection) = selected {
            require(
                report.problems.is_empty()
                    && report.verified_chunks == selection.chunk_count
                    && report.chunk_index_sha256 == selection.chunk_index_sha256,
                "Selected recovery prefix differs from verified original chunks",
            )?;
            require(
                report
                    .streams
                    .iter()
                    .zip(&self.descriptor.streams)
                    .any(|(prefix, stream)| {
                        matches!(stream.kind, StreamKind::Audio { .. }) && prefix.recorded_units > 0
                    }),
                "Selected recovery prefix must contain recorded audio",
            )?;
        }
        let interrupted = if recovered {
            let receipt = load_interrupted(&tx)?;
            require(
                report.verified_chunks <= receipt.report.verified_chunks,
                "Recovery cannot extend its original proven prefix",
            )?;
            require(
                report.chunk_index_sha256
                    == interrupted_prefix_hash(&tx, &self.descriptor, report.verified_chunks)?,
                "Recovered bytes differ from the retained interrupted original prefix",
            )?;
            Some(receipt)
        } else {
            require(
                report.problems.is_empty(),
                "Cannot seal damaged or discontinuous original capture",
            )?;
            None
        };
        let body = ManifestBody {
            version: JOURNAL_VERSION,
            manifest_id: uuid::Uuid::new_v4().to_string(),
            operation_id: operation_id.into(),
            descriptor: self.descriptor.clone(),
            descriptor_sha256: self.descriptor.sha256()?,
            streams: report.streams,
            chunk_count: report.verified_chunks,
            payload_bytes: report.verified_payload_bytes,
            chunk_index_sha256: report.chunk_index_sha256,
            interrupted,
        };
        let sha256 = digest(
            b"OmniuSong.sealed-take.v1\0",
            &bounded_json(&body, MAX_METADATA_BYTES * 2)?,
        );
        let manifest = SealedManifest {
            body,
            sha256: sha256.clone(),
        };
        manifest.verify_hash()?;
        tx.execute(
            "INSERT INTO sealed_manifests VALUES(1,?1,?2,?3)",
            params![operation_id, manifest.canonical_bytes()?, sha256],
        )?;
        invoke(&mut self.hook, WriteBoundary::AfterManifestInsert)?;
        tx.execute("UPDATE take_meta SET state='sealed' WHERE singleton=1", [])?;
        if let Some(expected) = original_rows {
            require(
                crate::inspection::original_rows_hash(&tx)? == expected,
                "Checked finalization changed original rows; transaction rolled back",
            )?;
        }
        if let Some(expected) = complete {
            require(
                state(&tx)? == TakeState::Sealed
                    && complete_snapshot(&tx, &self.descriptor)? == *expected
                    && verify_sealed(&tx, &self.descriptor)? == manifest,
                "Complete finalization changed mode, metadata or manifest; transaction rolled back",
            )?;
        }
        invoke(&mut self.hook, WriteBoundary::BeforeSealCommit)?;
        tx.commit()?;
        invoke(&mut self.hook, WriteBoundary::AfterSealCommit)?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod selected_prefix_boundaries {
    use super::*;
    fn fixture() -> (PathBuf, Journal, RecoveryPrefix, String) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/selected-prefix-boundaries")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&path).unwrap();
        let path = path.join("take.sqlite3");
        let take = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                source_id: "synthetic".into(),
                source_name: "Synthetic PCM, no device".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("clock".into()),
                    epoch: ClockEpochId("epoch".into()),
                    ticks_per_second_numerator: 48_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 1,
                        encoding: PcmEncoding::F32Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("audio".into()),
                },
                initial_source_position: 0,
            }],
            provenance: BTreeMap::from([(
                "omniusong.performance.v1".into(),
                serde_json::Value::Null,
            )]),
        };
        let mut journal = Journal::create(&path, &take).unwrap();
        for sequence in 0..3 {
            let stream = &take.streams[0];
            let stamp = HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: sequence,
            };
            let header = ChunkHeader {
                version: CHUNK_VERSION,
                take_id: take.take_id.clone(),
                stream_id: stream.stream_id.clone(),
                segment_id: stream.segment_id.clone(),
                sequence,
                range: SourceRange::AudioFrames {
                    start: sequence,
                    count: 1,
                },
                interpretation: stream.kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            };
            journal.append(&header, &0.25f32.to_le_bytes()).unwrap();
        }
        drop(journal);
        let journal = Journal::open(&path).unwrap();
        let report = scan(&journal.connection, &take, Some(1)).unwrap();
        let selection = RecoveryPrefix {
            chunk_count: 1,
            chunk_index_sha256: report.chunk_index_sha256,
        };
        let original = crate::inspection::original_rows_hash(&journal.connection).unwrap();
        (path, journal, selection, original)
    }
    #[test]
    fn selected_seal_boundaries_rollback_or_replay_lost_ack_without_original_row_change() {
        for boundary in [
            WriteBoundary::AfterManifestInsert,
            WriteBoundary::BeforeSealCommit,
            WriteBoundary::AfterSealCommit,
        ] {
            let (path, mut journal, selection, original) = fixture();
            journal.set_boundary_hook(move |point| {
                if point == boundary {
                    Err(RecordingError::Invalid(
                        "Injected selected seal boundary".into(),
                    ))
                } else {
                    Ok(())
                }
            });
            assert!(
                journal
                    .seal_recovered_prefix_checked("selected-hook", &original, &selection)
                    .is_err()
            );
            journal.clear_boundary_hook();
            assert_eq!(
                crate::inspection::original_rows_hash(&journal.connection).unwrap(),
                original
            );
            let count: u64 = journal
                .connection
                .query_row("SELECT count(*) FROM chunks", [], |row| {
                    read_unsigned(row, 0)
                })
                .unwrap();
            assert_eq!(count, 3);
            if boundary == WriteBoundary::AfterSealCommit {
                assert_eq!(journal.state().unwrap(), TakeState::Sealed);
            } else {
                assert_eq!(journal.state().unwrap(), TakeState::Interrupted);
                assert!(journal.sealed_manifest().unwrap().is_none());
            }
            let manifest = journal
                .seal_recovered_prefix_checked("selected-hook", &original, &selection)
                .unwrap();
            assert_eq!(manifest.body.chunk_count, 1);
            assert_eq!(
                journal
                    .seal_recovered_prefix_checked("selected-hook", &original, &selection)
                    .unwrap(),
                manifest
            );
            assert!(
                journal
                    .seal_recovered_prefix_checked("selected-hook", &"0".repeat(64), &selection)
                    .is_err()
            );
            drop(journal);
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }
}
