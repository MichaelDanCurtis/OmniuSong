//! Bounded physical identity beside SQLite's held logical snapshot.
//! Metadata checks open no auxiliary descriptor on SQLite's locked inode.
use crate::{InspectionBudget, Result, types::require};
use rusqlite::{Connection, ffi};
use std::{fs, path::Path, time::SystemTime};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SnapshotIdentity {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    unix: (u64, u64, u64, i64, i64, i64, i64),
}

impl SnapshotIdentity {
    pub(crate) fn capture(path: &Path, budget: &InspectionBudget) -> Result<Self> {
        budget.checkpoint()?;
        budget.charge_work(512)?;
        let metadata = fs::symlink_metadata(path)?;
        require(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Bounded sealed source is no longer an ordinary file",
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            require(
                metadata.nlink() == 1,
                "Bounded sealed source gained an alias",
            )?;
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            budget.checkpoint()?;
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            require(
                fs::symlink_metadata(Path::new(&sidecar))
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
                "Bounded sealed source has an unexpected SQLite sidecar",
            )?;
        }
        budget.file(metadata.len())?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified()?,
            #[cfg(unix)]
            unix: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.nlink(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        })
    }

    pub(crate) fn verify(
        &self,
        connection: &Connection,
        path: &Path,
        budget: &InspectionBudget,
    ) -> Result<()> {
        require(
            *self == Self::capture(path, budget)?,
            "Bounded sealed source changed since its logical snapshot was opened",
        )?;
        let mut moved: std::ffi::c_int = 0;
        // SAFETY: this connection remains borrowed for the whole synchronous
        // call. SQLite's documented HAS_MOVED operation writes one c_int and
        // neither transfers the handle nor runs application callbacks.
        let result = unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_HAS_MOVED,
                (&mut moved as *mut std::ffi::c_int).cast(),
            )
        };
        require(
            result == ffi::SQLITE_OK,
            "The SQLite VFS cannot prove bounded sealed-source handle identity",
        )?;
        require(moved == 0, "SQLite's held sealed source has moved")?;
        require(
            *self == Self::capture(path, budget)?,
            "Bounded sealed source changed during native identity verification",
        )?;
        budget.checkpoint()
    }
}
