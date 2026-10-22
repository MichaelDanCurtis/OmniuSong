//! Process-local path leases for raw evidence and NativeStore file operations.
//! Raw descriptor close can release same-process SQLite transaction locks. The
//! brief registry mutex covers only lease admission/removal, never file I/O.
use crate::{Result, StoreError};
use omniusong_recording::RecordingError;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

struct Lease {
    paths: Vec<PathBuf>,
    exclusive: bool,
}
#[derive(Default)]
struct Registry {
    next: u64,
    active: BTreeMap<u64, Lease>,
}
static LEASES: OnceLock<Mutex<Registry>> = OnceLock::new();

pub(crate) struct Guard(u64);
impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut registry) = registry().lock() {
            registry.active.remove(&self.0);
        }
        // Poisoning fails future admission closed; never panic while unwinding.
    }
}
fn registry() -> &'static Mutex<Registry> {
    LEASES.get_or_init(|| Mutex::new(Registry::default()))
}
fn acquire(mut paths: Vec<PathBuf>, exclusive: bool) -> Result<Guard> {
    paths.sort();
    paths.dedup();
    let mut registry = registry().lock().map_err(|_| {
        StoreError::Invalid("Retained-file coordination was interrupted; restart explicitly".into())
    })?;
    if registry.active.values().any(|lease| {
        (exclusive || lease.exclusive)
            && paths.iter().any(|a| {
                lease
                    .paths
                    .iter()
                    .any(|b| a.starts_with(b) || b.starts_with(a))
            })
    }) {
        return Err(StoreError::Recording(RecordingError::Owned));
    }
    let id = registry.next.checked_add(1).ok_or_else(|| {
        StoreError::Invalid("Retained-file coordination identity exhausted".into())
    })?;
    registry.next = id;
    registry.active.insert(id, Lease { paths, exclusive });
    Ok(Guard(id))
}

/// NativeStore roots are canonical. Writers may coexist, preserving existing
/// exclusive-create and dedup contracts, but exclude overlapping raw exports.
pub(crate) fn writer(root: &Path) -> Result<Guard> {
    acquire(vec![root.to_owned()], false)
}

/// Archive I/O may target another store's retained tree. Include its resolved
/// file path as well as this store's root before opening any file descriptor.
pub(crate) fn writer_with_file(root: &Path, path: &Path) -> Result<Guard> {
    acquire(vec![root.to_owned(), file_parent(path)?], false)
}

fn file_path(path: &Path) -> Result<PathBuf> {
    Ok(match fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            fs::canonicalize(parent)?.join(path.file_name().ok_or_else(|| {
                StoreError::Invalid("File coordination requires a file name".into())
            })?)
        }
        Err(error) => return Err(error.into()),
    })
}

fn file_parent(path: &Path) -> Result<PathBuf> {
    let resolved = file_path(path)?;
    // SQLite/private archive stages are siblings, so reserve the whole parent.
    let parent = resolved
        .parent()
        .ok_or_else(|| StoreError::Invalid("File coordination requires a parent".into()))?;
    Ok(parent.to_owned())
}

/// A raw evidence lease excludes every overlapping store/path lease, including
/// nested roots. Guard lifetime must outlast closure of every raw descriptor.
pub(crate) fn evidence(root: &Path, destination: &Path) -> Result<Guard> {
    acquire(vec![root.to_owned(), file_parent(destination)?], true)
}

pub(crate) fn evidence_input(path: &Path) -> Result<Guard> {
    acquire(vec![file_path(path)?], true)
}

pub(crate) fn evidence_restore(input: &Path, directory: &Path) -> Result<Guard> {
    acquire(vec![file_path(input)?, file_path(directory)?], true)
}
