//! Selected-source inspection without catalog mutation, migration or device I/O.
use super::{
    Result, STORE_SCHEMA_VERSION, StoreError, TakeObject, load_document, read_asset, takes,
};
use omniusong_core::{ProjectDocument, contracts::TakeId, native::TakeReference, valid_asset_id};
use omniusong_recording::SealedJournal;
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct NativeReader {
    connection: Connection,
    root: PathBuf,
}

fn ordinary(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StoreError::Invalid(
            "Read-only catalog must be an ordinary nonsymlink file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(StoreError::Invalid(
                "Hardlinked read-only catalogs are unsupported".into(),
            ));
        }
    }
    Ok(metadata)
}

fn same(left: &fs::Metadata, right: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if left.dev() != right.dev() || left.ino() != right.ino() {
            return Err(StoreError::Invalid(
                "Catalog path changed during read-only open".into(),
            ));
        }
    }
    if left.len() != right.len() {
        return Err(StoreError::Invalid(
            "Catalog length changed during read-only open; retry inspection".into(),
        ));
    }
    Ok(())
}

impl NativeReader {
    /// Existing current-schema, rollback-journal catalog only. The header check
    /// precedes SQLite open so a WAL reader cannot create a shared-memory file.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let metadata = fs::symlink_metadata(root.as_ref())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(StoreError::Invalid(
                "Read-only store root must be a real directory".into(),
            ));
        }
        let root = fs::canonicalize(root.as_ref())?;
        let path = root.join("studio.sqlite3");
        let before = ordinary(&path)?;
        let mut file = File::open(&path)?;
        same(&before, &file.metadata()?)?;
        let mut header = [0u8; 20];
        file.read_exact(&mut header)?;
        if &header[..16] != b"SQLite format 3\0" || header[18] != 1 || header[19] != 1 {
            return Err(StoreError::Invalid(
                "Read-only inspection requires a rollback-journal SQLite catalog".into(),
            ));
        }
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;")?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != STORE_SCHEMA_VERSION {
            return Err(StoreError::Invalid("Read-only inspection requires the current native store schema; no migration was attempted".into()));
        }
        same(&before, &file.metadata()?)?;
        same(&before, &ordinary(&path)?)?;
        Ok(Self { connection, root })
    }

    /// Metadata/asset copying is one consistent, short transaction. RAII also
    /// rolls back on panic. Expensive immutable take verification belongs after
    /// this method returns, not inside the supplied closure.
    pub fn read_snapshot<T>(&mut self, read: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let transaction = self.connection.unchecked_transaction()?;
        let result = read(self);
        transaction.rollback()?;
        result
    }

    /// Does not authenticate unrelated source files referenced by this document.
    pub fn load_document(&self, id: &str) -> Result<ProjectDocument> {
        load_document(&self.connection, id)
    }

    pub fn asset(&self, id: &str) -> Result<Vec<u8>> {
        if !valid_asset_id(id) {
            return Err(StoreError::Invalid(
                "Invalid read-only audio asset identity".into(),
            ));
        }
        read_asset(&self.connection, id)
    }

    pub fn take_metadata(&self, take_id: &TakeId) -> Result<TakeObject> {
        takes::load_take(&self.connection, take_id)
    }

    /// No write or migration capability is exposed. Cancellation is cooperative
    /// around the journal's initial manifest scan and every physical hash block;
    /// the initial scan and an OS file read are not interruptible operations.
    pub fn open_take_bounded(
        &self,
        reference: &TakeReference,
        max_bytes: u64,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SealedJournal> {
        let object = self.take_metadata(&reference.take_id)?;
        takes::check_reference(&object, reference)?;
        takes::verify_owned_take_bounded(&self.root, &object, max_bytes, cancelled)
    }
}
