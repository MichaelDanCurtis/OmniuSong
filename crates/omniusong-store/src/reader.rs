//! Selected-source inspection without catalog mutation, migration or device I/O.
use super::{
    Result, STORE_SCHEMA_VERSION, StoreError, TakeObject, load_document, read_asset, takes,
};
use omniusong_core::{ProjectDocument, contracts::TakeId, native::TakeReference, valid_asset_id};
use omniusong_recording::SealedJournal;
use rusqlite::{Connection, OpenFlags, ffi};
use std::{
    fs,
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

/// Inspect only SQLite's borrowed main-file handle. Opening and then closing a
/// separate OS descriptor can release another connection's POSIX file locks.
/// SQLite must own every open/close of the catalog, including refusal paths.
/// See https://www.sqlite.org/howtocorrupt.html#_posix_advisory_locks_canceled_by_a_separate_thread_doing_close_
fn catalog_header(connection: &Connection, expected_len: u64) -> Result<[u8; 20]> {
    fn sqlite_result(code: i32) -> Result<()> {
        if code == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(rusqlite::Error::SqliteFailure(ffi::Error::new(code), None).into())
        }
    }

    let mut header = [0_u8; 20];
    // SAFETY: `connection` is a live, exclusively used read-only connection.
    // FILE_POINTER writes one borrowed sqlite3_file pointer into correctly
    // typed storage. Its methods remain owned by this connection. The pointer
    // never escapes, and we never close/free it or call any mutating method.
    // xRead writes exactly 20 bytes at offset zero into the stack buffer;
    // xFileSize/HAS_MOVED write into correctly typed initialized scalars.
    // No SQLite call or callback can close the connection during these calls.
    // Contracts: https://www.sqlite.org/c3ref/file_control.html and
    // https://www.sqlite.org/c3ref/io_methods.html .
    unsafe {
        let handle = connection.handle();
        let mut file: *mut ffi::sqlite3_file = std::ptr::null_mut();
        sqlite_result(ffi::sqlite3_file_control(
            handle,
            c"main".as_ptr(),
            ffi::SQLITE_FCNTL_FILE_POINTER,
            std::ptr::addr_of_mut!(file).cast(),
        ))?;
        let methods = file
            .as_ref()
            .and_then(|file| file.pMethods.as_ref())
            .filter(|methods| methods.iVersion >= 1)
            .ok_or_else(|| {
                StoreError::Invalid("SQLite VFS cannot inspect the read-only catalog".into())
            })?;
        let read = methods.xRead.ok_or_else(|| {
            StoreError::Invalid("SQLite VFS cannot read the catalog header".into())
        })?;
        let file_size = methods.xFileSize.ok_or_else(|| {
            StoreError::Invalid("SQLite VFS cannot inspect the catalog length".into())
        })?;
        let mut moved: i32 = -1;
        let code = ffi::sqlite3_file_control(
            handle,
            c"main".as_ptr(),
            ffi::SQLITE_FCNTL_HAS_MOVED,
            std::ptr::addr_of_mut!(moved).cast(),
        );
        if code == ffi::SQLITE_NOTFOUND {
            return Err(StoreError::Invalid(
                "Read-only inspection requires SQLite VFS file-identity checks".into(),
            ));
        }
        sqlite_result(code)?;
        if moved != 0 {
            return Err(StoreError::Invalid(
                "Catalog path changed during read-only open".into(),
            ));
        }
        let mut length: i64 = -1;
        sqlite_result(file_size(file, &mut length))?;
        if u64::try_from(length).ok() != Some(expected_len) {
            return Err(StoreError::Invalid(
                "Catalog length changed during read-only open; retry inspection".into(),
            ));
        }
        sqlite_result(read(file, header.as_mut_ptr().cast(), 20, 0))?;
    }
    Ok(header)
}

impl NativeReader {
    /// Existing current-schema, rollback-journal catalog only. SQLite owns the
    /// file from acquisition through close. The borrowed header check precedes
    /// all schema SQL so a refused WAL catalog needs no shared-memory file.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_checked(root, || Ok(()))
    }

    // The callback permits a deterministic path-replacement regression after
    // SQLite acquires its handle. Production callers always use `open` above.
    pub(super) fn open_checked(
        root: impl AsRef<Path>,
        after_open: impl FnOnce() -> Result<()>,
    ) -> Result<Self> {
        let metadata = fs::symlink_metadata(root.as_ref())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(StoreError::Invalid(
                "Read-only store root must be a real directory".into(),
            ));
        }
        let root = fs::canonicalize(root.as_ref())?;
        let path = root.join("studio.sqlite3");
        let before = ordinary(&path)?;
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        after_open()?;
        let header = catalog_header(&connection, before.len())?;
        if &header[..16] != b"SQLite format 3\0" || header[18] != 1 || header[19] != 1 {
            return Err(StoreError::Invalid(
                "Read-only inspection requires a rollback-journal SQLite catalog".into(),
            ));
        }
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;")?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != STORE_SCHEMA_VERSION {
            return Err(StoreError::Invalid("Read-only inspection requires the current native store schema; no migration was attempted".into()));
        }
        if catalog_header(&connection, before.len())? != header {
            return Err(StoreError::Invalid(
                "Catalog header changed during read-only open; retry inspection".into(),
            ));
        }
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

    /// A standalone current-document read with held registered originals.
    /// SQL refers to this same studio catalog; callers must not relabel a
    /// failure as an unrelated archived/input database. No writer is exposed.
    pub fn load_document_with_budget(
        &mut self,
        id: &str,
        budget: &omniusong_recording::InspectionBudget,
    ) -> Result<ProjectDocument> {
        crate::catalog_transaction::CatalogTransaction::deferred(&self.connection, budget)?.run(
            |tx| {
                let document = crate::read_document_metadata_with_budget(tx, id, budget)?;
                crate::active_projection_edits::load(tx, &self.root, document, budget)
            },
        )
    }

    /// Bounded structural metadata inside read_snapshot. This neither opens
    /// originals nor validates a claimed projection. Selected-source callers
    /// must separately authenticate the selected media and never adopt claims.
    /// The enclosing legacy snapshot retains its existing SQL-control limits.
    pub fn load_document_metadata_with_budget(
        &self,
        id: &str,
        budget: &omniusong_recording::InspectionBudget,
    ) -> Result<ProjectDocument> {
        crate::project_metadata_view::load(&self.connection, id, budget)
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
