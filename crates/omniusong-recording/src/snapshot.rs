use crate::{
    journal::{load_chunk, load_descriptor, read_unsigned, sql_integer, verify_sealed},
    types::*,
};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

/// One verified sealed view, held in a read transaction until dropped.
///
/// This is a reader, not a writer/recovery owner. Opening it neither creates an
/// owner sidecar nor changes capture state. Calls belong on a storage thread.
pub struct SealedJournal {
    connection: Connection,
    path: PathBuf,
    descriptor: TakeDescriptor,
    manifest: SealedManifest,
}

/// Identity of the durable exported SQLite file, distinct from logical identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupReceipt {
    pub path: PathBuf,
    pub byte_len: u64,
    /// SHA-256 of the complete exported file, read with a fixed-size buffer.
    pub sha256: String,
    pub manifest_sha256: String,
    pub take_id: TakeId,
}

impl SealedJournal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = fs::canonicalize(path)?;
        require(
            fs::metadata(&path)?.is_file(),
            "Sealed journal is not a file",
        )?;
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        // These connection-local settings do not modify database bytes. The
        // first SELECT establishes the snapshot held by the explicit read tx.
        connection.execute_batch(
            "PRAGMA query_only=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; BEGIN DEFERRED;",
        )?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        require(
            version == JOURNAL_VERSION,
            "Unsupported take journal version",
        )?;
        let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        require(
            mode.eq_ignore_ascii_case("delete"),
            "Sealed journal must use rollback journal mode",
        )?;
        let descriptor = load_descriptor(&connection)?;
        let manifest = verify_sealed(&connection, &descriptor)?;
        Ok(Self {
            connection,
            path,
            descriptor,
            manifest,
        })
    }

    /// The resolved path used to open this view; identity comes from its manifest.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn descriptor(&self) -> &TakeDescriptor {
        &self.descriptor
    }

    pub fn manifest(&self) -> &SealedManifest {
        &self.manifest
    }

    /// Authenticate the full retained original under this existing read-only
    /// snapshot, including any rows outside a recovered selected prefix.
    /// This never acquires a capture owner, creates a sidecar, or permits writes.
    /// The result is review evidence, not authority to resume or normally seal.
    pub fn inspect_snapshot(&self) -> Result<crate::JournalInspection> {
        let inspection =
            crate::inspection::inspect_connection(&self.connection, &self.path, &self.descriptor)?;
        require(
            inspection.manifest.as_ref() == Some(&self.manifest),
            "Sealed inspection differs from its held manifest",
        )?;
        Ok(inspection)
    }

    /// Read only chunks selected by the sealed manifest, never an unknown tail.
    pub fn read_chunk(&self, stream: &str, sequence: u64) -> Result<ChunkRecord> {
        let prefix = self
            .manifest
            .body
            .streams
            .iter()
            .find(|prefix| prefix.stream_id == stream)
            .ok_or_else(|| RecordingError::Invalid("Unknown sealed recording stream".into()))?;
        require(
            sequence < prefix.next_sequence,
            "Chunk is outside the sealed original prefix",
        )?;
        load_chunk(&self.connection, &self.descriptor, stream, sequence)
    }

    /// Global ordinal is one-based, unlike the zero-based per-stream sequence.
    pub fn read_ordinal(&self, ordinal: u64) -> Result<ChunkRecord> {
        require(
            ordinal > 0 && ordinal <= self.manifest.body.chunk_count,
            "Chunk ordinal is outside the sealed original prefix",
        )?;
        let (stream, sequence): (String, u64) = self.connection.query_row(
            "SELECT substr(stream_id,1,97),sequence FROM chunks WHERE ordinal=?1",
            [sql_integer(ordinal)?],
            |row| Ok((row.get(0)?, read_unsigned(row, 1)?)),
        )?;
        self.read_chunk(&stream, sequence)
    }

    /// Visit selected chunks in global order with one bounded chunk in memory.
    ///
    /// A visitor may return an error to stop. Only success means the complete
    /// ordered range/root was visited. Retaining payloads is the caller's choice.
    pub fn visit_chunks<F>(&self, mut visitor: F) -> Result<()>
    where
        F: FnMut(u64, &ChunkRecord) -> Result<()>,
    {
        let mut prefixes: Vec<_> = self
            .descriptor
            .streams
            .iter()
            .map(StreamPrefix::initial)
            .collect();
        let indices: BTreeMap<_, _> = self
            .descriptor
            .streams
            .iter()
            .enumerate()
            .map(|(index, stream)| (stream.stream_id.as_str(), index))
            .collect();
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.recording-index.v1\0");
        hash.update(self.manifest.body.descriptor_sha256.as_bytes());
        let mut bytes = 0_u64;
        for ordinal in 1..=self.manifest.body.chunk_count {
            let chunk = self.read_ordinal(ordinal)?;
            let index = *indices
                .get(chunk.header.stream_id.as_str())
                .ok_or_else(|| RecordingError::Invalid("Unknown sealed recording stream".into()))?;
            prefixes[index].advance(&chunk.header)?;
            bytes = bytes
                .checked_add(chunk.payload.len() as u64)
                .ok_or_else(|| {
                    RecordingError::Invalid("Sealed payload byte count overflow".into())
                })?;
            hash.update(ordinal.to_le_bytes());
            hash.update(chunk.sha256.as_bytes());
            visitor(ordinal, &chunk)?;
        }
        require(
            prefixes == self.manifest.body.streams
                && bytes == self.manifest.body.payload_bytes
                && format!("{:x}", hash.finalize()) == self.manifest.body.chunk_index_sha256,
            "Visited chunks differ from the sealed manifest",
        )
    }

    /// Export the held logical snapshot to a new file, never overwriting a name.
    ///
    /// The existing destination parent must be a trusted local directory. A
    /// private exclusive staging directory limits accidental staging interference;
    /// hostile same-user path replacement is outside this filesystem contract.
    /// Errors retain staging or a published orphan and report those paths. This
    /// function never publishes a project reference or deletes source originals.
    pub fn export_backup(&self, destination: impl AsRef<Path>) -> Result<BackupReceipt> {
        let mut destination = destination.as_ref().to_path_buf();
        let mut staging_path: Option<PathBuf> = None;
        let mut published = false;
        let outcome = (|| -> Result<BackupReceipt> {
            let parent = destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let parent = fs::canonicalize(parent)?;
            let name = destination.file_name().ok_or_else(|| {
                RecordingError::Invalid("Backup destination needs a file name".into())
            })?;
            destination = parent.join(name);
            match fs::symlink_metadata(&destination) {
                Ok(_) => {
                    return Err(RecordingError::Conflict(
                        "Backup destination already exists".into(),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }

            let staging_dir = parent.join(format!(
                ".omniusong-backup-{}.partial",
                uuid::Uuid::new_v4()
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&staging_dir)?;
            staging_path = Some(staging_dir.clone());
            let staging_file = staging_dir.join("journal.sqlite3");
            let reservation = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&staging_file)?;
            let mut target = Connection::open_with_flags(
                &staging_file,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            target.busy_timeout(Duration::from_secs(2))?;
            target.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;")?;
            {
                let backup = Backup::new(&self.connection, &mut target)?;
                loop {
                    // SQLite pages are <=64 KiB; each step moves <=1 MiB.
                    match backup.step(16)? {
                        StepResult::Done => break,
                        StepResult::More => {}
                        StepResult::Busy | StepResult::Locked => return Err(RecordingError::Conflict(
                            "SQLite backup is busy; retained staging can be inspected before an explicit retry".into(),
                        )),
                        _ => return Err(RecordingError::Invalid("Unsupported SQLite backup result".into())),
                    }
                }
            }
            target
                .close()
                .map_err(|(_, error)| RecordingError::Database(error))?;

            // Revalidate the copied selected prefix and retained interruption
            // index. SQLite Backup copies every table/page, including bad tail
            // bytes that intentionally cannot be exposed as selected chunks.
            let copied = Self::open(&staging_file)?;
            require(
                copied.descriptor == self.descriptor && copied.manifest == self.manifest,
                "Backup differs from the selected sealed identity",
            )?;
            for table in [
                "take_meta",
                "stream_prefix",
                "chunks",
                "interruptions",
                "recovery_index",
                "sealed_manifests",
            ] {
                let query = format!("SELECT count(*) FROM {table}");
                let source_count = self
                    .connection
                    .query_row(&query, [], |row| read_unsigned(row, 0))?;
                let copied_count = copied
                    .connection
                    .query_row(&query, [], |row| read_unsigned(row, 0))?;
                require(
                    source_count == copied_count,
                    "Backup lost logical journal rows",
                )?;
            }
            drop(copied);
            let (byte_len, sha256) = hash_file(&reservation)?;
            reservation.sync_all()?;
            drop(reservation);

            // hard_link is an atomic create-if-absent operation. rename would
            // overwrite on Unix. No existing destination or symlink is followed.
            fs::hard_link(&staging_file, &destination)?;
            published = true;
            fs::remove_file(&staging_file)?;
            fs::remove_dir(&staging_dir)?;
            staging_path = None;
            File::open(&parent)?.sync_all()?;
            Ok(BackupReceipt {
                path: destination.clone(),
                byte_len,
                sha256,
                manifest_sha256: self.manifest.sha256.clone(),
                take_id: self.descriptor.take_id.clone(),
            })
        })();
        outcome.map_err(|error| RecordingError::Backup {
            staging_path,
            destination_path: destination,
            published,
            message: error.to_string(),
        })
    }
}

fn hash_file(mut file: &File) -> Result<(u64, String)> {
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| RecordingError::Invalid("Backup file byte count overflow".into()))?;
        hash.update(&buffer[..read]);
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}
