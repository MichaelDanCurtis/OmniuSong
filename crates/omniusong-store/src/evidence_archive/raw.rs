//! Raw retained-tree proof. Source bytes are never opened through SQLite.
use super::{
    EvidenceEntry, EvidenceEntryKind, EvidenceRoot, MAX_EVIDENCE_ENTRIES, require, valid_id,
    valid_path,
};
use crate::{Result, StoreError};
use omniusong_recording::{InspectionBudget, RecordingError};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const ROOTS: [&str; 3] = ["captures", "source-actions", "takes"];
const BUFFER_BYTES: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    length: u64,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mtime: i64,
    #[cfg(unix)]
    mtime_ns: i64,
    #[cfg(unix)]
    ctime: i64,
    #[cfg(unix)]
    ctime_ns: i64,
}
impl Stamp {
    fn from(metadata: &fs::Metadata) -> Self {
        Self {
            length: metadata.len(),
            #[cfg(unix)]
            dev: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            mtime: metadata.mtime(),
            #[cfg(unix)]
            mtime_ns: metadata.mtime_nsec(),
            #[cfg(unix)]
            ctime: metadata.ctime(),
            #[cfg(unix)]
            ctime_ns: metadata.ctime_nsec(),
        }
    }
    fn same_object(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            self.dev == other.dev && self.inode == other.inode
        }
        #[cfg(not(unix))]
        {
            let _ = other;
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    kind: EvidenceEntryKind,
    stamp: Stamp,
}
struct Anchored {
    observed: Observed,
    file: File,
}
struct ParentAnchor {
    path: PathBuf,
    stamp: Stamp,
    file: File,
}
#[derive(Debug, PartialEq, Eq)]
struct Inventory {
    roots: Vec<EvidenceRoot>,
    nodes: BTreeMap<String, Observed>,
}

pub(super) struct RawEvidenceSet {
    root: PathBuf,
    parents: Vec<ParentAnchor>,
    inventory: Inventory,
    nodes: BTreeMap<String, Anchored>,
    owners: Vec<String>,
    entries: Vec<EvidenceEntry>,
}

impl Drop for RawEvidenceSet {
    fn drop(&mut self) {
        // Closing any raw descriptor for a SQLite inode can release this
        // process's POSIX SQLite locks. Keep every operational owner held until
        // all data descriptors close, including on partial acquisition errors.
        // Otherwise another same-process owner could start between lease drop
        // and data close. Remaining owner descriptors close by normal field drop.
        let owners = &self.owners;
        self.nodes
            .retain(|path, _| owners.binary_search(path).is_ok());
    }
}

fn metadata(path: &Path) -> Result<(fs::Metadata, EvidenceEntryKind)> {
    let metadata = fs::symlink_metadata(path)?;
    require(
        !metadata.file_type().is_symlink(),
        "Evidence refuses a symbolic link",
    )?;
    let kind = if metadata.is_dir() {
        EvidenceEntryKind::Directory
    } else {
        require(
            metadata.is_file(),
            "Evidence requires ordinary files and directories",
        )?;
        #[cfg(unix)]
        require(metadata.nlink() == 1, "Evidence refuses a hard-linked file")?;
        EvidenceEntryKind::File
    };
    require(
        fs::canonicalize(path)? == path,
        "Evidence path or parent was redirected",
    )?;
    Ok((metadata, kind))
}

fn account_path(path: &str, budget: &InspectionBudget) -> Result<()> {
    budget.checkpoint()?;
    valid_path(path)?;
    budget.visit_rows(1)?;
    budget.charge_work(path.len() as u64 + 80)?;
    Ok(())
}

fn inventory(root: &Path, budget: &InspectionBudget) -> Result<Inventory> {
    fn visit(
        root: &Path,
        relative: String,
        nodes: &mut BTreeMap<String, Observed>,
        budget: &InspectionBudget,
    ) -> Result<()> {
        account_path(&relative, budget)?;
        require(
            nodes.len() < MAX_EVIDENCE_ENTRIES - 1,
            "Raw evidence exceeds the 2047-entry ceiling",
        )?;
        let path = root.join(&relative);
        let (info, kind) = metadata(&path)?;
        if kind == EvidenceEntryKind::File {
            // A nested NativeStore catalog has independent writers without
            // capture owners. It cannot be archived as a raw recording file:
            // extra descriptor close could release its process SQLite locks.
            // Refuse from metadata alone, even when the nested store is idle.
            require(
                !relative
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("studio.sqlite3")),
                "Nested native catalog in retained evidence is unsupported",
            )?;
            budget.observe_file_bytes(info.len())?;
        }
        let observed = Observed {
            kind,
            stamp: Stamp::from(&info),
        };
        require(
            nodes.insert(relative.clone(), observed.clone()).is_none(),
            "Duplicate raw evidence path",
        )?;
        if kind == EvidenceEntryKind::Directory {
            // No collection before the entry bound. A single over-limit child
            // refuses the entire set; no truncated-success inventory exists.
            for entry in fs::read_dir(&path)? {
                budget.checkpoint()?;
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_str().ok_or_else(|| {
                    StoreError::Invalid("Evidence entry name is not UTF-8".into())
                })?;
                require(
                    !name.contains('/'),
                    "Evidence entry name contains a separator",
                )?;
                visit(root, format!("{relative}/{name}"), nodes, budget)?;
            }
            let (after, after_kind) = metadata(&path)?;
            require(
                after_kind == kind && Stamp::from(&after) == observed.stamp,
                "Evidence directory changed during enumeration",
            )?;
        }
        Ok(())
    }
    let mut result = Inventory {
        roots: Vec::with_capacity(3),
        nodes: BTreeMap::new(),
    };
    for name in ROOTS {
        account_path(name, budget)?;
        let path = root.join(name);
        let present = match fs::symlink_metadata(&path) {
            Ok(info) => {
                require(
                    info.is_dir() && !info.file_type().is_symlink(),
                    "Evidence root must be an ordinary directory",
                )?;
                visit(root, name.into(), &mut result.nodes, budget)?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        result.roots.push(EvidenceRoot {
            path: name.into(),
            present,
        });
    }
    Ok(result)
}

fn pin(path: &Path, observed: &Observed) -> Result<File> {
    let (before, kind) = metadata(path)?;
    require(
        kind == observed.kind && Stamp::from(&before) == observed.stamp,
        "Evidence changed before anchored open",
    )?;
    let file = File::open(path)?;
    let (after, after_kind) = metadata(path)?;
    require(
        after_kind == observed.kind
            && Stamp::from(&file.metadata()?) == observed.stamp
            && Stamp::from(&after) == observed.stamp,
        "Evidence changed during anchored open",
    )?;
    Ok(file)
}

fn parent_anchors(root: &Path, budget: &InspectionBudget) -> Result<Vec<ParentAnchor>> {
    require(root.is_absolute(), "Evidence root must be absolute")?;
    let mut anchors = Vec::new();
    for path in root.ancestors() {
        budget.checkpoint()?;
        budget.visit_rows(1)?;
        budget.charge_work(path.as_os_str().len() as u64 + 80)?;
        let (info, kind) = metadata(path)?;
        require(
            kind == EvidenceEntryKind::Directory,
            "Evidence ancestor must be an ordinary directory",
        )?;
        let stamp = Stamp::from(&info);
        let file = File::open(path)?;
        require(
            file.metadata()?.is_dir() && Stamp::from(&file.metadata()?).same_object(&stamp),
            "Evidence ancestor changed while anchoring",
        )?;
        anchors.push(ParentAnchor {
            path: path.to_owned(),
            stamp,
            file,
        });
    }
    Ok(anchors)
}

fn owner_paths(inventory: &Inventory, budget: &InspectionBudget) -> Result<Vec<String>> {
    let mut owners = BTreeSet::new();
    for (path, item) in &inventory.nodes {
        account_path(path, budget)?;
        if item.kind == EvidenceEntryKind::File
            && (path.ends_with(".owner.lock") || path.ends_with(".execution.lock"))
        {
            owners.insert(path.clone());
        }
        let parts: Vec<_> = path.split('/').collect();
        let required = if parts.len() == 3
            && parts[0] == "captures"
            && parts[2] == "take.sqlite3"
            && item.kind == EvidenceEntryKind::File
        {
            Some(format!("{path}.owner.lock"))
        } else if parts.len() == 2
            && parts[0] == "source-actions"
            && valid_id(parts[1])
            && item.kind == EvidenceEntryKind::Directory
        {
            Some(format!("source-actions/{}.execution.lock", parts[1]))
        } else {
            None
        };
        if let Some(required) = required {
            require(
                inventory
                    .nodes
                    .get(&required)
                    .is_some_and(|node| node.kind == EvidenceEntryKind::File),
                "Retained source/action lacks its existing ordinary owner",
            )?;
            owners.insert(required);
        }
    }
    Ok(owners.into_iter().collect())
}

impl RawEvidenceSet {
    pub(super) fn acquire(root: &Path, budget: &InspectionBudget) -> Result<Self> {
        let result = (|| {
            budget.checkpoint()?;
            require(
                cfg!(unix),
                "Raw retained evidence ownership is qualified only on Unix",
            )?;
            let parents = parent_anchors(root, budget)?;
            let inventory = inventory(root, budget)?;
            let owners = owner_paths(&inventory, budget)?;
            let mut nodes = BTreeMap::new();
            // Metadata inventory opens no data files. Acquire every existing
            // operational owner before opening even one SQLite data descriptor:
            // a refusal must not release a live same-process SQLite transaction
            // merely by opening and then closing a second raw descriptor.
            for path in &owners {
                budget.checkpoint()?;
                let observed = &inventory.nodes[path];
                let file = pin(&root.join(path), observed)?;
                nodes.insert(
                    path.clone(),
                    Anchored {
                        observed: observed.clone(),
                        file,
                    },
                );
            }
            // Existing files only. Lock acquisition never changes operational
            // bytes, fabricates an owner, waits indefinitely or retries a writer.
            for path in &owners {
                budget.checkpoint()?;
                nodes[path].file.try_lock().map_err(|error| match error {
                    std::fs::TryLockError::WouldBlock => {
                        StoreError::Recording(RecordingError::Owned)
                    }
                    std::fs::TryLockError::Error(error) => StoreError::Io(error),
                })?;
            }
            let mut set = Self {
                root: root.to_owned(),
                parents,
                inventory,
                nodes,
                owners,
                entries: Vec::new(),
            };
            set.verify_structure(budget)?;
            // Construct the owner-only set first so any later failure closes
            // data descriptors through its ordered Drop while owners stay held.
            for (path, observed) in &set.inventory.nodes {
                budget.checkpoint()?;
                if set.nodes.contains_key(path) {
                    continue;
                }
                let file = pin(&set.root.join(path), observed)?;
                set.nodes.insert(
                    path.clone(),
                    Anchored {
                        observed: observed.clone(),
                        file,
                    },
                );
            }
            set.verify_structure(budget)?;
            for (path, node) in &set.nodes {
                budget.checkpoint()?;
                let sha256 = if node.observed.kind == EvidenceEntryKind::File {
                    Some(set.hash(path, budget)?)
                } else {
                    None
                };
                set.entries.push(EvidenceEntry {
                    path: path.clone(),
                    kind: node.observed.kind,
                    byte_length: if node.observed.kind == EvidenceEntryKind::File {
                        node.observed.stamp.length
                    } else {
                        0
                    },
                    sha256,
                });
            }
            set.verify_structure(budget)?;
            Ok(set)
        })();
        budget.checkpoint()?;
        result
    }

    pub(super) fn entries(&self) -> &[EvidenceEntry] {
        &self.entries
    }
    pub(super) fn roots(&self) -> &[EvidenceRoot] {
        &self.inventory.roots
    }

    fn check_parents(&self, budget: &InspectionBudget) -> Result<()> {
        for anchor in &self.parents {
            budget.checkpoint()?;
            budget.visit_rows(1)?;
            budget.charge_work(anchor.path.as_os_str().len() as u64 + 80)?;
            let (info, kind) = metadata(&anchor.path)?;
            require(
                kind == EvidenceEntryKind::Directory
                    && anchor.file.metadata()?.is_dir()
                    && Stamp::from(&info).same_object(&anchor.stamp)
                    && Stamp::from(&anchor.file.metadata()?).same_object(&anchor.stamp),
                "Evidence root/ancestor identity changed",
            )?;
        }
        Ok(())
    }

    fn check_node(&self, relative: &str, budget: &InspectionBudget) -> Result<()> {
        account_path(relative, budget)?;
        let node = self.nodes.get(relative).ok_or_else(|| {
            StoreError::Invalid("Evidence path is outside the frozen inventory".into())
        })?;
        let (info, kind) = metadata(&self.root.join(relative))?;
        require(
            kind == node.observed.kind
                && Stamp::from(&info) == node.observed.stamp
                && Stamp::from(&node.file.metadata()?) == node.observed.stamp,
            "Anchored evidence file or directory changed",
        )
    }

    fn check_file_boundary(&self, relative: &str, budget: &InspectionBudget) -> Result<()> {
        self.check_parents(budget)?;
        self.check_node(relative, budget)?;
        let mut parent = relative;
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            self.check_node(prefix, budget)?;
            parent = prefix;
        }
        for owner in &self.owners {
            self.check_node(owner, budget)?;
        }
        Ok(())
    }

    fn verify_structure(&self, budget: &InspectionBudget) -> Result<()> {
        self.check_parents(budget)?;
        require(
            inventory(&self.root, budget)? == self.inventory,
            "Retained evidence membership, root presence or metadata changed",
        )?;
        for path in self.nodes.keys() {
            self.check_node(path, budget)?;
        }
        self.check_parents(budget)
    }

    fn hash(&self, relative: &str, budget: &InspectionBudget) -> Result<String> {
        self.stream(relative, None, budget)
    }

    fn stream(
        &self,
        relative: &str,
        mut output: Option<&mut dyn Write>,
        budget: &InspectionBudget,
    ) -> Result<String> {
        self.check_file_boundary(relative, budget)?;
        let node = &self.nodes[relative];
        require(
            node.observed.kind == EvidenceEntryKind::File,
            "Evidence copy requires a file entry",
        )?;
        let expected = node.observed.stamp.length;
        budget.observe_file_bytes(expected)?;
        let mut input = &node.file;
        input.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; BUFFER_BYTES];
        loop {
            budget.checkpoint()?;
            budget.visit_chunks(1)?;
            let capacity = expected
                .saturating_sub(total)
                .saturating_add(1)
                .min(buffer.len() as u64) as usize;
            budget.charge_work(capacity as u64)?;
            let count = input.read(&mut buffer[..capacity])?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or_else(|| StoreError::Invalid("Evidence length overflow".into()))?;
            require(total <= expected, "Evidence file grew while reading")?;
            budget.charge_work(count as u64)?;
            hash.update(&buffer[..count]);
            if let Some(output) = output.as_deref_mut() {
                budget.charge_work(count as u64)?;
                output.write_all(&buffer[..count])?;
            }
        }
        require(total == expected, "Evidence file shortened while reading")?;
        self.check_file_boundary(relative, budget)?;
        Ok(format!("{:x}", hash.finalize()))
    }

    pub(super) fn copy_file(
        &self,
        path: &str,
        out: &mut dyn Write,
        budget: &InspectionBudget,
    ) -> Result<()> {
        let result = (|| {
            budget.checkpoint()?;
            valid_path(path)?;
            let entry = self
                .entries
                .binary_search_by(|entry| entry.path.as_str().cmp(path))
                .ok()
                .map(|index| &self.entries[index])
                .ok_or_else(|| StoreError::Invalid("Evidence copy path was not admitted".into()))?;
            require(
                entry.kind == EvidenceEntryKind::File,
                "Evidence copy cannot stream a directory",
            )?;
            let hash = self.stream(path, Some(out), budget)?;
            require(
                entry.sha256.as_deref() == Some(hash.as_str()),
                "Copied evidence differs from the admitted raw hash",
            )
        })();
        budget.checkpoint()?;
        result
    }

    pub(super) fn verify(&self, budget: &InspectionBudget) -> Result<()> {
        let result = (|| {
            self.verify_structure(budget)?;
            for entry in &self.entries {
                if entry.kind == EvidenceEntryKind::File {
                    let hash = self.hash(&entry.path, budget)?;
                    require(
                        entry.sha256.as_deref() == Some(hash.as_str()),
                        "Evidence raw content changed before archive completion",
                    )?;
                }
            }
            self.verify_structure(budget)
        })();
        budget.checkpoint()?;
        result
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use omniusong_recording::InspectionLimits;
    use std::sync::{Arc, atomic::AtomicBool};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/evidence-raw-private")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(root.join("captures/partial/empty")).unwrap();
            let root = fs::canonicalize(root).unwrap();
            fs::write(
                root.join("captures/partial/take.sqlite3"),
                vec![0x5a; BUFFER_BYTES * 2],
            )
            .unwrap();
            fs::write(
                root.join("captures/partial/take.sqlite3.owner.lock"),
                b"opaque owner bytes",
            )
            .unwrap();
            fs::write(
                root.join("captures/partial/take.sqlite3-journal"),
                b"raw hot journal",
            )
            .unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fn budget() -> InspectionBudget {
        InspectionBudget::new(
            InspectionLimits::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
    }

    fn assert_sqlite_writer_probe(path: &Path, expect_busy: bool) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "evidence_archive::raw::tests::sqlite_writer_probe_subprocess",
                "--ignored",
                "--nocapture",
            ])
            .env("OMNIUSONG_RAW_SQLITE_PROBE_PATH", path)
            .env("OMNIUSONG_RAW_SQLITE_PROBE_OWNER", "yes")
            .env(
                "OMNIUSONG_RAW_SQLITE_PROBE_BUSY",
                if expect_busy { "yes" } else { "no" },
            )
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("RAW_SQLITE_PROBE_COMPLETED"),
            "SQLite subprocess probe failed: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "subprocess helper; invoked with an isolated synthetic SQLite fixture"]
    fn sqlite_writer_probe_subprocess() {
        let path = PathBuf::from(
            std::env::var_os("OMNIUSONG_RAW_SQLITE_PROBE_PATH")
                .expect("probe requires explicit synthetic database path"),
        );
        let expect_busy = match std::env::var("OMNIUSONG_RAW_SQLITE_PROBE_BUSY").as_deref() {
            Ok("yes") => true,
            Ok("no") => false,
            other => panic!("invalid probe expectation: {other:?}"),
        };
        let owner = if std::env::var("OMNIUSONG_RAW_SQLITE_PROBE_OWNER").as_deref() == Ok("no") {
            None
        } else {
            let owner = File::open(path.with_extension("sqlite3.owner.lock")).unwrap();
            assert!(matches!(
                owner.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            Some(owner)
        };
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        connection.busy_timeout(std::time::Duration::ZERO).unwrap();
        let result = connection.execute_batch("BEGIN IMMEDIATE");
        if expect_busy {
            assert!(
                matches!(
                    result,
                    Err(rusqlite::Error::SqliteFailure(error, _))
                        if error.code == rusqlite::ErrorCode::DatabaseBusy
                ),
                "a second process must remain excluded by the original SQLite transaction: {result:?}"
            );
        } else {
            result.unwrap();
            connection.execute_batch("ROLLBACK").unwrap();
        }
        drop(owner);
        println!("RAW_SQLITE_PROBE_COMPLETED");
    }

    #[test]
    fn refused_acquisition_preserves_existing_sqlite_transaction_and_owner_locks() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/evidence-raw-private")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("captures/active")).unwrap();
        let f = Fixture(fs::canonicalize(root).unwrap());
        let path = f.0.join("captures/active/take.sqlite3");
        let owner_path = path.with_extension("sqlite3.owner.lock");
        fs::write(&owner_path, b"existing operational owner").unwrap();
        let owner = File::open(&owner_path).unwrap();
        owner.try_lock().unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=DELETE; CREATE TABLE probe(value INTEGER); \
                 INSERT INTO probe VALUES(7); BEGIN IMMEDIATE;",
            )
            .unwrap();
        assert_sqlite_writer_probe(&path, true);
        assert!(matches!(
            RawEvidenceSet::acquire(&f.0, &budget()),
            Err(StoreError::Recording(RecordingError::Owned))
        ));
        assert_sqlite_writer_probe(&path, true);
        connection.execute_batch("ROLLBACK").unwrap();
        // This positive control rules out a child that always reports BUSY.
        // The operational owner stays held while only the SQLite lock releases.
        assert_sqlite_writer_probe(&path, false);
        assert_eq!(
            connection
                .query_row("SELECT value FROM probe", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            7
        );
        drop(connection);
        drop(owner);
    }

    #[test]
    fn malformed_database_and_sidecars_are_raw_evidence_under_existing_owner() {
        let f = Fixture::new();
        let budget = budget();
        let set = RawEvidenceSet::acquire(&f.0, &budget).unwrap();
        assert_eq!(
            set.roots()
                .iter()
                .map(|root| root.present)
                .collect::<Vec<_>>(),
            vec![true, false, false]
        );
        assert!(
            set.entries()
                .iter()
                .any(|entry| entry.path == "captures/partial/empty"
                    && entry.kind == EvidenceEntryKind::Directory)
        );
        let mut bytes = Vec::new();
        set.copy_file("captures/partial/take.sqlite3-journal", &mut bytes, &budget)
            .unwrap();
        assert_eq!(bytes, b"raw hot journal");
        set.verify(&budget).unwrap();
        assert_eq!(
            fs::read(f.0.join("captures/partial/take.sqlite3.owner.lock")).unwrap(),
            b"opaque owner bytes"
        );
        assert!(matches!(
            RawEvidenceSet::acquire(&f.0, &budget),
            Err(StoreError::Recording(RecordingError::Owned))
        ));
    }

    #[test]
    fn path_replacement_during_copy_cannot_acknowledge_the_old_open_file() {
        struct ReplacingWriter {
            path: PathBuf,
            replaced: bool,
            copied: Vec<u8>,
        }
        impl Write for ReplacingWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if !self.replaced {
                    fs::rename(&self.path, self.path.with_extension("retained-old"))?;
                    fs::write(&self.path, vec![0x5a; BUFFER_BYTES * 2])?;
                    self.replaced = true;
                }
                self.copied.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let f = Fixture::new();
        let budget = budget();
        let set = RawEvidenceSet::acquire(&f.0, &budget).unwrap();
        let path = f.0.join("captures/partial/take.sqlite3");
        let mut output = ReplacingWriter {
            path: path.clone(),
            replaced: false,
            copied: Vec::new(),
        };
        assert!(
            set.copy_file("captures/partial/take.sqlite3", &mut output, &budget)
                .is_err()
        );
        assert!(output.replaced);
        assert_eq!(
            fs::read(path.with_extension("retained-old")).unwrap(),
            vec![0x5a; BUFFER_BYTES * 2]
        );
        assert!(set.verify(&budget).is_err());
    }

    #[test]
    fn absent_root_becoming_present_and_new_empty_directory_refuse_final_proof() {
        for change_absent_root in [false, true] {
            let f = Fixture::new();
            let budget = budget();
            let set = RawEvidenceSet::acquire(&f.0, &budget).unwrap();
            fs::create_dir(if change_absent_root {
                f.0.join("takes")
            } else {
                f.0.join("captures/partial/new-empty")
            })
            .unwrap();
            assert!(set.verify(&budget).is_err());
        }
    }
}
