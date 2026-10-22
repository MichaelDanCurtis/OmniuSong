//! Bounded write-once evidence metadata. No archive or NativeStore is opened.
use super::*;
use serde::de::DeserializeOwned;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Read,
    sync::{Mutex, OnceLock},
};

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;

const PREFIX: &str = ".omniusong-evidence-jobs-v1-";
const HISTORY_DOMAIN: &[u8] = b"OmniuSong.EvidenceJobHistory.v1\0";
const BODY_DOMAIN: &[u8] = b"OmniuSong.EvidenceJobRecordBody.v1\0";

fn io(error: std::io::Error) -> ServiceError {
    ServiceError::backend(EvidenceMessage::new(error.to_string()).message, true)
}
fn busy() -> ServiceError {
    ServiceError {
        kind: FailureKind::SourceBusy,
        message: "Evidence journal is already owned".into(),
        outcome_unknown: false,
    }
}
fn finish<T>(
    budget: &InspectionBudget,
    result: Result<T, ServiceError>,
) -> Result<T, ServiceError> {
    checkpoint(budget)?;
    result
}

#[derive(Default)]
struct Registry {
    next: u64,
    paths: BTreeMap<PathBuf, u64>,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
struct ProcessGuard {
    path: PathBuf,
    identity: u64,
}
impl ProcessGuard {
    fn acquire(path: &Path) -> Result<Self, ServiceError> {
        let mut registry = REGISTRY
            .get_or_init(|| Mutex::new(Registry::default()))
            .lock()
            .map_err(|_| ServiceError::rejected("Evidence journal ownership was interrupted"))?;
        if registry.paths.contains_key(path) {
            return Err(busy());
        }
        registry.next = registry.next.checked_add(1).ok_or_else(|| {
            ServiceError::rejected("Evidence journal ownership identity exhausted")
        })?;
        let identity = registry.next;
        registry.paths.insert(path.to_owned(), identity);
        Ok(Self {
            path: path.to_owned(),
            identity,
        })
    }
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Ok(mut registry) = REGISTRY
            .get_or_init(|| Mutex::new(Registry::default()))
            .lock()
        {
            registry.paths.remove(&self.path);
        }
    }
}

fn metadata_credit(path: &Path, budget: &InspectionBudget) -> Result<(), ServiceError> {
    charge(budget, path.as_os_str().len() as u64 + 256)
}
fn ordinary_parent(path: &Path, budget: &InspectionBudget) -> Result<(), ServiceError> {
    require(
        cfg!(unix),
        "Evidence journal ownership is qualified on Unix only",
    )?;
    for ancestor in path.ancestors() {
        metadata_credit(ancestor, budget)?;
        let metadata = fs::symlink_metadata(ancestor).map_err(io)?;
        require(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Evidence journal ancestry is redirected or nonordinary",
        )?;
    }
    require(
        fs::canonicalize(path).map_err(io)?.as_os_str() == path.as_os_str(),
        "Evidence journal ancestry is not exact canonical text",
    )
}
fn private_directory(path: &Path) -> Result<fs::Metadata, ServiceError> {
    let metadata = fs::symlink_metadata(path).map_err(io)?;
    require(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Evidence journal needs an ordinary directory",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            metadata.mode() & 0o077 == 0,
            "Evidence journal directory is not private",
        )?;
    }
    Ok(metadata)
}
fn ordinary_file(path: &Path, maximum: usize) -> Result<fs::Metadata, ServiceError> {
    let metadata = fs::symlink_metadata(path).map_err(io)?;
    require(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= maximum as u64,
        "Evidence journal slot is redirected, nonordinary or oversized",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            metadata.nlink() == 1 && metadata.mode() & 0o077 == 0,
            "Evidence journal slot is linked or exposed",
        )?;
    }
    Ok(metadata)
}
fn same(before: &fs::Metadata, after: &fs::Metadata, exact: bool) -> Result<(), ServiceError> {
    require(
        before.is_file() == after.is_file()
            && before.is_dir() == after.is_dir()
            && !after.file_type().is_symlink(),
        "Evidence journal member type changed",
    )?;
    if exact {
        require(
            before.len() == after.len(),
            "Evidence journal member length changed",
        )?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let links_match = if before.is_file() {
            before.nlink() == 1 && after.nlink() == 1
        } else {
            !exact || before.nlink() == after.nlink()
        };
        require(
            before.dev() == after.dev()
                && before.ino() == after.ino()
                && before.mode() == after.mode()
                && links_match,
            "Evidence journal member identity or permissions changed",
        )?;
        if exact {
            require(
                before.len() == after.len()
                    && before.mtime() == after.mtime()
                    && before.mtime_nsec() == after.mtime_nsec()
                    && before.ctime() == after.ctime()
                    && before.ctime_nsec() == after.ctime_nsec(),
                "Evidence journal member bytes or membership changed",
            )?;
        }
    }
    Ok(())
}
fn create_directory(path: &Path) -> Result<(), ServiceError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(io)
}
fn create_file(path: &Path) -> Result<File, ServiceError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(io)
}
fn sync_directory(path: &Path) -> Result<(), ServiceError> {
    File::open(path).map_err(io)?.sync_all().map_err(io)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Created,
    Written,
    FileSynced,
    DirectorySynced,
}
#[cfg(test)]
type Observer = Box<dyn FnMut(Boundary, &Path) -> Result<(), ServiceError> + Send>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeaderBody {
    version: u32,
    origin_root: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    origin_root: String,
    body_sha256: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Request,
    Started,
    Terminal,
    Observation,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    version: u32,
    request_sha256: String,
    kind: Kind,
    sequence: u32,
    body_sha256: String,
    body: T,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Started {
    version: u32,
}

struct Slot {
    name: String,
    path: PathBuf,
    bytes: Vec<u8>,
    stamp: fs::Metadata,
}
pub(super) struct JobEntry {
    pub(super) view: EvidenceJobView,
    slots: Vec<Slot>,
    directory: File,
    directory_stamp: fs::Metadata,
    history: Sha256,
    path: PathBuf,
    view_bytes: usize,
}
pub(super) enum Admission {
    Fresh(JobEntry),
    Existing(JobEntry),
}
/// A one-use, journal-bound allowance. It cannot grant any artifact operation.
pub(super) struct Finalization {
    owner: u64,
}

pub(super) struct EvidenceJournal {
    origin: String,
    path: PathBuf,
    parent: File,
    parent_stamp: fs::Metadata,
    root: File,
    root_stamp: RefCell<fs::Metadata>,
    header: Slot,
    owner: File,
    owner_path: PathBuf,
    owner_stamp: fs::Metadata,
    guard: ProcessGuard,
    #[cfg(test)]
    observer: RefCell<Option<Observer>>,
}

fn names(
    path: &Path,
    maximum: usize,
    budget: &InspectionBudget,
) -> Result<BTreeSet<String>, ServiceError> {
    metadata_credit(path, budget)?;
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(path).map_err(io)? {
        checkpoint(budget)?;
        require(
            names.len() < maximum,
            "Evidence journal member count exceeds its bound",
        )?;
        charge(budget, 4096 + 160)?;
        let name = entry
            .map_err(io)?
            .file_name()
            .into_string()
            .map_err(|_| ServiceError::rejected("Evidence journal member name is not UTF8"))?;
        require(names.insert(name), "Repeated evidence journal member")?;
    }
    Ok(names)
}
fn slot_names() -> Vec<String> {
    let mut names = vec![
        "request.json".into(),
        "started.json".into(),
        "terminal.json".into(),
    ];
    names.extend((1..=MAX_EVIDENCE_JOB_OBSERVATIONS).map(|n| format!("observation-{n:02}.json")));
    names
}
fn maximum(name: &str) -> usize {
    if name == "request.json" {
        MAX_EVIDENCE_JOB_REQUEST_BYTES
    } else if name == "started.json" {
        MAX_EVIDENCE_JOB_STARTED_BYTES
    } else {
        MAX_EVIDENCE_JOB_RECORD_BYTES
    }
}
fn history_add(history: &mut Sha256, name: &str, bytes: &[u8]) {
    history.update((name.len() as u64).to_le_bytes());
    history.update(name.as_bytes());
    history.update((bytes.len() as u64).to_le_bytes());
    history.update(bytes);
}
fn history_sha(history: &Sha256) -> String {
    format!("{:x}", history.clone().finalize())
}
fn read_slot(
    path: &Path,
    name: &str,
    maximum: usize,
    budget: &InspectionBudget,
) -> Result<Slot, ServiceError> {
    metadata_credit(path, budget)?;
    let before = ordinary_file(path, maximum)?;
    budget
        .observe_file_bytes(before.len())
        .map_err(budget_error)?;
    charge(budget, maximum as u64 * 8 + path.as_os_str().len() as u64)?;
    let mut file = File::open(path).map_err(io)?;
    same(&before, &file.metadata().map_err(io)?, true)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(before.len() as usize)
        .map_err(|_| ServiceError::rejected("Cannot reserve bounded evidence record bytes"))?;
    bytes.resize(before.len() as usize, 0);
    charge(budget, before.len().saturating_mul(2) + 1)?;
    file.read_exact(&mut bytes).map_err(io)?;
    let mut extra = [0u8; 1];
    require(
        file.read(&mut extra).map_err(io)? == 0,
        "Evidence journal slot grew during read",
    )?;
    same(&before, &file.metadata().map_err(io)?, true)?;
    same(&before, &ordinary_file(path, maximum)?, true)?;
    Ok(Slot {
        name: name.into(),
        path: path.to_owned(),
        bytes,
        stamp: before,
    })
}
fn decode<T: DeserializeOwned + Serialize>(
    bytes: &[u8],
    maximum: usize,
) -> Result<T, ServiceError> {
    let value: T =
        serde_json::from_slice(bytes).map_err(|error| ServiceError::rejected(error.to_string()))?;
    require(
        encode_bounded(&value, maximum)? == bytes,
        "Evidence record is not exact canonical JSON",
    )?;
    Ok(value)
}
fn envelope<T: Serialize>(
    body: T,
    request_sha256: &str,
    kind: Kind,
    sequence: u32,
    maximum: usize,
) -> Result<Vec<u8>, ServiceError> {
    let body_bytes = encode_bounded(&body, maximum)?;
    let value = Envelope {
        version: 1,
        request_sha256: request_sha256.into(),
        kind,
        sequence,
        body_sha256: domain_sha(BODY_DOMAIN, &body_bytes),
        body,
    };
    encode_bounded(&value, maximum)
}
fn validate_envelope<T: DeserializeOwned + Serialize>(
    slot: &Slot,
    request: &str,
    kind: Kind,
    sequence: u32,
) -> Result<T, ServiceError> {
    let record: Envelope<T> = decode(&slot.bytes, maximum(&slot.name))?;
    require(
        record.version == 1
            && record.request_sha256 == request
            && record.kind == kind
            && record.sequence == sequence,
        "Evidence record request/kind/sequence differs",
    )?;
    require(
        valid_sha(&record.body_sha256)
            && domain_sha(
                BODY_DOMAIN,
                &encode_bounded(&record.body, maximum(&slot.name))?,
            ) == record.body_sha256,
        "Evidence record body fingerprint differs",
    )?;
    Ok(record.body)
}

fn write_created(
    path: PathBuf,
    bytes: Vec<u8>,
    mut observe: impl FnMut(Boundary, &Path) -> Result<(), ServiceError>,
) -> Result<(Slot, File), ServiceError> {
    require(
        bytes.len() <= MAX_EVIDENCE_JOB_RECORD_BYTES,
        "Evidence record exceeds its fixed write bound",
    )?;
    let mut file = create_file(&path)?;
    let created = file.metadata().map_err(io)?;
    observe(Boundary::Created, &path)?;
    same(&created, &ordinary_file(&path, 0)?, true)?;
    file.write_all(&bytes).map_err(io)?;
    let written = file.metadata().map_err(io)?;
    same(&created, &written, false)?;
    require(
        written.len() == bytes.len() as u64,
        "Written evidence record has a different byte length",
    )?;
    observe(Boundary::Written, &path)?;
    file.sync_all().map_err(io)?;
    observe(Boundary::FileSynced, &path)?;
    same(&written, &file.metadata().map_err(io)?, true)?;
    same(&written, &ordinary_file(&path, bytes.len())?, true)?;
    sync_directory(path.parent().expect("record parent"))?;
    observe(Boundary::DirectorySynced, &path)?;
    same(&written, &file.metadata().map_err(io)?, true)?;
    same(&written, &ordinary_file(&path, bytes.len())?, true)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("compiled record name")
        .to_owned();
    Ok((
        Slot {
            name,
            path,
            bytes,
            stamp: written,
        },
        file,
    ))
}

// Only used under the prepaid finalization allowance. There are at most eleven
// known slots; one bounded surplus name refuses before unbounded allocation.
fn fixed_names(path: &Path) -> Result<BTreeSet<String>, ServiceError> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(path).map_err(io)? {
        require(
            names.len() < 3 + MAX_EVIDENCE_JOB_OBSERVATIONS,
            "Surplus evidence job slot after write",
        )?;
        let name = entry.map_err(io)?.file_name();
        require(
            name.as_encoded_bytes().len() <= 64,
            "Unexpected evidence job slot name after write",
        )?;
        let name = name
            .into_string()
            .map_err(|_| ServiceError::rejected("NonUTF8 evidence job slot after write"))?;
        require(names.insert(name), "Repeated evidence job slot after write")?;
    }
    Ok(names)
}

impl EvidenceJournal {
    pub(super) fn open(
        origin: &Path,
        create: bool,
        budget: &InspectionBudget,
    ) -> Result<Option<Self>, ServiceError> {
        Self::open_using(origin, create, budget, |_, _| Ok(()))
    }
    fn open_using(
        origin: &Path,
        create: bool,
        budget: &InspectionBudget,
        mut observe: impl FnMut(Boundary, &Path) -> Result<(), ServiceError>,
    ) -> Result<Option<Self>, ServiceError> {
        let result = (|| {
            checkpoint(budget)?;
            let origin_text = path_text(origin)?;
            charge(budget, origin_text.len() as u64 + 4096)?;
            let parent = origin.parent().ok_or_else(|| {
                ServiceError::rejected("Evidence journal origin lacks its canonical parent")
            })?;
            ordinary_parent(parent, budget)?;
            require(
                parent
                    .join(origin.file_name().expect("validated origin"))
                    .as_os_str()
                    == origin.as_os_str(),
                "Evidence origin is not exact canonical provenance",
            )?;
            let path = parent.join(format!(
                "{PREFIX}{}",
                domain_sha(b"", origin_text.as_bytes())
            ));
            let guard = ProcessGuard::acquire(&path)?;
            let exists = match fs::symlink_metadata(&path) {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(io(error)),
            };
            if !exists && !create {
                return Ok(None);
            }
            let mut initialized_owner = None;
            let mut initialized_root = None;
            if !exists {
                charge(budget, MAX_EVIDENCE_JOB_HEADER_BYTES as u64 * 3)?;
                let body = HeaderBody {
                    version: 1,
                    origin_root: origin_text.into(),
                };
                let header = Header {
                    version: 1,
                    origin_root: origin_text.into(),
                    body_sha256: domain_sha(
                        b"OmniuSong.EvidenceJobNamespace.v1\0",
                        &encode_bounded(&body, MAX_EVIDENCE_JOB_HEADER_BYTES)?,
                    ),
                };
                let bytes = encode_bounded(&header, MAX_EVIDENCE_JOB_HEADER_BYTES)?;
                create_directory(&path)?;
                let created_stamp = private_directory(&path)?;
                let created_root = File::open(&path).map_err(io)?;
                same(&created_stamp, &created_root.metadata().map_err(io)?, true)?;
                observe(Boundary::Created, &path)?;
                same(&created_stamp, &created_root.metadata().map_err(io)?, true)?;
                same(&created_stamp, &private_directory(&path)?, true)?;
                sync_directory(parent)?;
                observe(Boundary::DirectorySynced, &path)?;
                same(&created_stamp, &created_root.metadata().map_err(io)?, true)?;
                same(&created_stamp, &private_directory(&path)?, true)?;
                let (_, owner) = write_created(path.join(".owner.lock"), Vec::new(), &mut observe)?;
                same(&created_stamp, &created_root.metadata().map_err(io)?, false)?;
                same(&created_stamp, &private_directory(&path)?, false)?;
                owner.try_lock().map_err(|error| match error {
                    std::fs::TryLockError::WouldBlock => busy(),
                    std::fs::TryLockError::Error(error) => io(error),
                })?;
                initialized_owner = Some(owner);
                let _ = write_created(path.join("namespace.json"), bytes, &mut observe)?;
                require(
                    names(&path, MAX_EVIDENCE_JOBS + 2, budget)?
                        == BTreeSet::from([".owner.lock".into(), "namespace.json".into()]),
                    "New evidence namespace contains unexpected members",
                )?;
                same(&created_stamp, &created_root.metadata().map_err(io)?, false)?;
                same(&created_stamp, &private_directory(&path)?, false)?;
                initialized_root = Some(created_root);
            }
            ordinary_parent(&path, budget)?;
            metadata_credit(&path, budget)?;
            let root_stamp = private_directory(&path)?;
            let members = names(&path, MAX_EVIDENCE_JOBS + 2, budget)?;
            require(
                members.contains(".owner.lock")
                    && members.contains("namespace.json")
                    && members.iter().all(|name| {
                        name == ".owner.lock" || name == "namespace.json" || valid_id(name)
                    }),
                "Incomplete or unknown evidence journal namespace membership",
            )?;
            let owner_path = path.join(".owner.lock");
            let owner_stamp = ordinary_file(&owner_path, 0)?;
            let owner = match initialized_owner {
                Some(owner) => owner,
                None => File::open(&owner_path).map_err(io)?,
            };
            same(&owner_stamp, &owner.metadata().map_err(io)?, true)?;
            owner.try_lock().map_err(|error| match error {
                std::fs::TryLockError::WouldBlock => busy(),
                std::fs::TryLockError::Error(error) => io(error),
            })?;
            same(&owner_stamp, &ordinary_file(&owner_path, 0)?, true)?;
            let root = match initialized_root {
                Some(root) => root,
                None => File::open(&path).map_err(io)?,
            };
            same(&root_stamp, &root.metadata().map_err(io)?, true)?;
            let parent_file = File::open(parent).map_err(io)?;
            let parent_stamp = parent_file.metadata().map_err(io)?;
            let header = read_slot(
                &path.join("namespace.json"),
                "namespace.json",
                MAX_EVIDENCE_JOB_HEADER_BYTES,
                budget,
            )?;
            let parsed: Header = decode(&header.bytes, MAX_EVIDENCE_JOB_HEADER_BYTES)?;
            let body = HeaderBody {
                version: parsed.version,
                origin_root: parsed.origin_root.clone(),
            };
            require(
                parsed.version == 1
                    && parsed.origin_root == origin_text
                    && parsed.body_sha256
                        == domain_sha(
                            b"OmniuSong.EvidenceJobNamespace.v1\0",
                            &encode_bounded(&body, MAX_EVIDENCE_JOB_HEADER_BYTES)?,
                        ),
                "Evidence journal namespace header differs from the exact configured origin",
            )?;
            let value = Self {
                origin: origin_text.into(),
                path,
                parent: parent_file,
                parent_stamp,
                root,
                root_stamp: RefCell::new(root_stamp),
                header,
                owner,
                owner_path,
                owner_stamp,
                guard,
                #[cfg(test)]
                observer: RefCell::new(None),
            };
            value.namespace(budget)?;
            Ok(Some(value))
        })();
        finish(budget, result)
    }
    fn boundary(&self, point: Boundary, path: &Path) -> Result<(), ServiceError> {
        #[cfg(test)]
        if let Some(observer) = self.observer.borrow_mut().as_mut() {
            observer(point, path)?;
        }
        let _ = (point, path);
        Ok(())
    }
    fn fixed_anchors(&self) -> Result<(), ServiceError> {
        self.fixed_anchors_with_root(true)
    }
    fn fixed_anchors_with_root(&self, exact_root: bool) -> Result<(), ServiceError> {
        let parent = self.path.parent().expect("journal parent");
        same(
            &self.parent_stamp,
            &self.parent.metadata().map_err(io)?,
            false,
        )?;
        same(
            &self.parent_stamp,
            &fs::symlink_metadata(parent).map_err(io)?,
            false,
        )?;
        same(
            &self.root_stamp.borrow(),
            &private_directory(&self.path)?,
            exact_root,
        )?;
        same(
            &self.root_stamp.borrow(),
            &self.root.metadata().map_err(io)?,
            exact_root,
        )?;
        same(&self.owner_stamp, &self.owner.metadata().map_err(io)?, true)?;
        same(
            &self.owner_stamp,
            &ordinary_file(&self.owner_path, 0)?,
            true,
        )?;
        same(
            &self.header.stamp,
            &ordinary_file(&self.header.path, MAX_EVIDENCE_JOB_HEADER_BYTES)?,
            true,
        )
    }
    fn namespace(&self, budget: &InspectionBudget) -> Result<Vec<String>, ServiceError> {
        charge(budget, 4096)?;
        ordinary_parent(self.path.parent().expect("journal parent"), budget)?;
        self.fixed_anchors()?;
        let names = names(&self.path, MAX_EVIDENCE_JOBS + 2, budget)?;
        require(
            names.contains(".owner.lock") && names.contains("namespace.json"),
            "Evidence journal namespace is incomplete",
        )?;
        let mut jobs = Vec::new();
        charge(budget, MAX_EVIDENCE_JOBS as u64 * 160)?;
        for name in names {
            if name == ".owner.lock" || name == "namespace.json" {
                continue;
            }
            require(valid_id(&name), "Unknown evidence journal namespace member")?;
            metadata_credit(&self.path.join(&name), budget)?;
            private_directory(&self.path.join(&name))?;
            jobs.push(name);
        }
        self.fixed_anchors()?;
        Ok(jobs)
    }
    pub(super) fn list(&self, budget: &InspectionBudget) -> Result<EvidenceJobList, ServiceError> {
        let result = (|| {
            let jobs = self.namespace(budget)?;
            charge(budget, MAX_EVIDENCE_JOB_LIST_BYTES as u64)?;
            let mut list = EvidenceJobList::default();
            for id in jobs {
                let entry = self.read(&id, budget)?;
                list.jobs.push(entry.view.summary());
            }
            check_bounded(&list, MAX_EVIDENCE_JOB_LIST_BYTES)?;
            self.namespace(budget)?;
            Ok(list)
        })();
        finish(budget, result)
    }
    pub(super) fn read(
        &self,
        id: &str,
        budget: &InspectionBudget,
    ) -> Result<JobEntry, ServiceError> {
        let result = (|| {
            checkpoint(budget)?;
            require(valid_id(id), "Invalid evidence job identity")?;
            require(
                self.namespace(budget)?.iter().any(|name| name == id),
                "Evidence job is not present",
            )?;
            let path = self.path.join(id);
            metadata_credit(&path, budget)?;
            let directory_stamp = private_directory(&path)?;
            let occupied = names(&path, 3 + MAX_EVIDENCE_JOB_OBSERVATIONS, budget)?;
            charge(budget, 4096 + MAX_EVIDENCE_JOB_VIEW_BYTES as u64)?;
            let order = slot_names();
            require(
                occupied.iter().all(|name| order.contains(name)),
                "Unknown evidence job slot",
            )?;
            for name in &occupied {
                metadata_credit(&path.join(name), budget)?;
                ordinary_file(&path.join(name), maximum(name))?;
            }
            let directory = File::open(&path).map_err(io)?;
            same(&directory_stamp, &directory.metadata().map_err(io)?, true)?;
            let mut view = EvidenceJobView {
                job_id: id.into(),
                directory: path.clone(),
                request: None,
                request_sha256: None,
                history_sha256: String::new(),
                started: false,
                terminal: None,
                observations: Vec::new(),
                occupied_observations: 0,
                metadata_problems: Vec::new(),
            };
            let mut history = Sha256::new();
            history.update(HISTORY_DOMAIN);
            let mut slots = Vec::new();
            if !occupied.contains("request.json") {
                view.metadata_problems.push(EvidenceMetadataProblem {
                    slot: "request.json".into(),
                    detail: EvidenceMessage::new(
                        "Request record is missing; no artifact operation is authorized",
                    ),
                });
            }
            for name in order {
                if !occupied.contains(&name) {
                    continue;
                }
                let slot = read_slot(&path.join(&name), &name, maximum(&name), budget)?;
                let previous = history_sha(&history);
                if name.starts_with("observation-") {
                    let sequence = name[12..14]
                        .parse::<usize>()
                        .expect("compiled observation name");
                    if sequence > view.occupied_observations + 1 {
                        view.metadata_problems.push(EvidenceMetadataProblem { slot: name.clone(), detail: EvidenceMessage::new("Earlier observation slots are missing; consumed indices will never be repaired") });
                    }
                }
                let validated = (|| {
                    if name == "request.json" {
                        let record: Envelope<EvidenceJobRequest> =
                            decode(&slot.bytes, MAX_EVIDENCE_JOB_REQUEST_BYTES)?;
                        record.body.validate()?;
                        self.request_fences(&record.body)?;
                        let fingerprint = record.body.fingerprint()?;
                        require(
                            record.version == 1
                                && record.kind == Kind::Request
                                && record.sequence == 0
                                && record.request_sha256 == fingerprint
                                && record.body.job_id == id
                                && record.body.origin_root == self.origin,
                            "Evidence request identity differs from its exact job/origin",
                        )?;
                        let request: EvidenceJobRequest =
                            validate_envelope(&slot, &fingerprint, Kind::Request, 0)?;
                        view.request_sha256 = Some(fingerprint);
                        view.request = Some(request);
                    } else {
                        let request = view.request.as_ref().ok_or_else(|| {
                            ServiceError::rejected("Later record has no valid exact request")
                        })?;
                        let fingerprint = view
                            .request_sha256
                            .as_deref()
                            .expect("valid request fingerprint");
                        if name == "started.json" {
                            let started: Started =
                                validate_envelope(&slot, fingerprint, Kind::Started, 0)?;
                            require(started.version == 1, "Unsupported started record")?;
                            view.started = true;
                        } else if name == "terminal.json" {
                            let outcome: EvidenceOutcome =
                                validate_envelope(&slot, fingerprint, Kind::Terminal, 0)?;
                            outcome.validate_for(request)?;
                            require(view.started, "Terminal record has no valid started record")?;
                            view.terminal = Some(outcome);
                        } else {
                            let sequence = name[12..14].parse::<u32>().map_err(|_| {
                                ServiceError::rejected("Invalid observation slot sequence")
                            })?;
                            let observation: EvidenceObservation =
                                validate_envelope(&slot, fingerprint, Kind::Observation, sequence)?;
                            require(
                                observation.sequence == sequence
                                    && observation.previous_history_sha256 == previous,
                                "Observation differs from the full previous raw history",
                            )?;
                            observation.outcome.validate_for(request)?;
                            view.observations.push(observation);
                        }
                    }
                    Ok::<(), ServiceError>(())
                })();
                if name.starts_with("observation-") {
                    view.occupied_observations = view.occupied_observations.max(
                        name[12..14]
                            .parse::<usize>()
                            .expect("compiled observation name"),
                    );
                }
                if let Err(error) = validated {
                    view.metadata_problems.push(EvidenceMetadataProblem {
                        slot: name.clone(),
                        detail: EvidenceMessage::new(error.message),
                    });
                }
                charge(budget, slot.bytes.len() as u64 + name.len() as u64 + 16)?;
                history_add(&mut history, &name, &slot.bytes);
                slots.push(slot);
            }
            view.history_sha256 = history_sha(&history);
            let view_bytes = encode_bounded(&view, MAX_EVIDENCE_JOB_VIEW_BYTES)?.len();
            let entry = JobEntry {
                view,
                slots,
                directory,
                directory_stamp,
                history,
                path,
                view_bytes,
            };
            self.verify_entry(&entry, budget)?;
            Ok(entry)
        })();
        finish(budget, result)
    }
    pub(super) fn read_if_present(
        &self,
        id: &str,
        budget: &InspectionBudget,
    ) -> Result<Option<JobEntry>, ServiceError> {
        let result = (|| {
            checkpoint(budget)?;
            require(valid_id(id), "Invalid evidence job identity")?;
            if !self.namespace(budget)?.iter().any(|name| name == id) {
                return Ok(None);
            }
            self.read(id, budget).map(Some)
        })();
        finish(budget, result)
    }
    fn verify_fixed_entry(&self, entry: &JobEntry) -> Result<(), ServiceError> {
        self.fixed_anchors()?;
        require(
            entry.view.directory.as_os_str() == entry.path.as_os_str()
                && entry
                    .path
                    .parent()
                    .is_some_and(|parent| parent.as_os_str() == self.path.as_os_str())
                && entry
                    .path
                    .file_name()
                    .is_some_and(|name| name == entry.view.job_id.as_str()),
            "Evidence entry belongs to a different exact namespace",
        )?;
        same(
            &entry.directory_stamp,
            &private_directory(&entry.view.directory)?,
            true,
        )?;
        same(
            &entry.directory_stamp,
            &entry.directory.metadata().map_err(io)?,
            true,
        )?;
        for slot in &entry.slots {
            same(
                &slot.stamp,
                &ordinary_file(&slot.path, maximum(&slot.name))?,
                true,
            )?;
        }
        Ok(())
    }
    pub(super) fn verify_entry(
        &self,
        entry: &JobEntry,
        budget: &InspectionBudget,
    ) -> Result<(), ServiceError> {
        let result = (|| {
            self.namespace(budget)?;
            charge(budget, 4096 + entry.slots.len() as u64 * 512)?;
            self.verify_fixed_entry(entry)?;
            let actual = names(
                &entry.view.directory,
                3 + MAX_EVIDENCE_JOB_OBSERVATIONS,
                budget,
            )?;
            require(
                actual == entry.slots.iter().map(|slot| slot.name.clone()).collect(),
                "Evidence job occupied slots changed",
            )?;
            if let Some(request) = &entry.view.request {
                charge(budget, MAX_EVIDENCE_JOB_REQUEST_BYTES as u64 * 3)?;
                require(
                    request.fingerprint()?
                        == *entry
                            .view
                            .request_sha256
                            .as_ref()
                            .expect("valid request fingerprint"),
                    "Evidence request fingerprint changed",
                )?;
            }
            require(
                history_sha(&entry.history) == entry.view.history_sha256,
                "Evidence preloaded history changed",
            )?;
            self.verify_fixed_entry(entry)
        })();
        finish(budget, result)
    }
    fn request_fences(&self, request: &EvidenceJobRequest) -> Result<(), ServiceError> {
        let excluded = |path: &Path| {
            path.components().any(|part| {
                part.as_os_str()
                    .to_str()
                    .is_some_and(|name| name.starts_with(PREFIX))
            })
        };
        require(
            !excluded(request.destination())
                && !request.destination().starts_with(Path::new(&self.origin))
                && !Path::new(&self.origin).starts_with(request.destination()),
            "Evidence destination overlaps the live root or a journal namespace",
        )?;
        if let EvidenceOperation::Restore { archive_path, .. } = &request.operation {
            require(
                !excluded(archive_path),
                "Evidence archive input overlaps a journal namespace",
            )?;
        }
        Ok(())
    }
    fn exclusions(
        &self,
        request: &EvidenceJobRequest,
        budget: &InspectionBudget,
    ) -> Result<(), ServiceError> {
        self.request_fences(request)?;
        if let EvidenceOperation::Restore { archive_path, .. } = &request.operation {
            ordinary_parent(
                archive_path.parent().expect("validated archive path"),
                budget,
            )?;
        }
        ordinary_parent(
            request
                .destination()
                .parent()
                .expect("validated destination"),
            budget,
        )?;
        match fs::symlink_metadata(request.destination()) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io(error)),
            Ok(_) => Err(ServiceError::rejected(
                "Fresh evidence destination already exists; reconcile its existing job",
            )),
        }
    }
    pub(super) fn admit(
        &self,
        request: &EvidenceJobRequest,
        budget: &InspectionBudget,
    ) -> Result<Admission, ServiceError> {
        let result = (|| {
            checkpoint(budget)?;
            request.validate()?;
            require(
                request.origin_root == self.origin,
                "Evidence request origin differs from this exact journal",
            )?;
            charge(budget, MAX_EVIDENCE_JOB_REQUEST_BYTES as u64 * 4)?;
            let fingerprint = request.fingerprint()?;
            let bytes = envelope(
                request,
                &fingerprint,
                Kind::Request,
                0,
                MAX_EVIDENCE_JOB_REQUEST_BYTES,
            )?;
            let jobs = self.namespace(budget)?;
            if jobs.iter().any(|id| id == &request.job_id) {
                let entry = self.read(&request.job_id, budget)?;
                require(
                    entry
                        .slots
                        .iter()
                        .find(|slot| slot.name == "request.json")
                        .is_some_and(|slot| slot.bytes == bytes)
                        && entry.view.request_sha256.as_deref() == Some(fingerprint.as_str()),
                    "Existing evidence job identity has different or invalid request bytes",
                )?;
                return Ok(Admission::Existing(entry));
            }
            require(
                jobs.len() < MAX_EVIDENCE_JOBS,
                "Evidence journal job capacity is full; nothing was evicted",
            )?;
            self.exclusions(request, budget)?;
            charge(budget, (jobs.len() as u64 + 3) * 160)?;
            let expected_namespace = jobs
                .iter()
                .cloned()
                .chain([
                    ".owner.lock".into(),
                    "namespace.json".into(),
                    request.job_id.clone(),
                ])
                .collect::<BTreeSet<_>>();
            for id in jobs {
                let entry = self.read(&id, budget)?;
                let existing = entry.view.request.as_ref().ok_or_else(|| {
                    ServiceError::rejected(
                        "An invalid prior request prevents reliable destination reservations",
                    )
                })?;
                require(
                    !request.destination().starts_with(existing.destination())
                        && !existing.destination().starts_with(request.destination()),
                    "Evidence destination is already reserved by an immutable job request",
                )?;
            }
            checkpoint(budget)?;
            let path = self.path.join(&request.job_id);
            charge(budget, bytes.len() as u64 + 4096)?;
            create_directory(&path)?;
            let directory_stamp = private_directory(&path)?;
            let directory = File::open(&path).map_err(io)?;
            self.boundary(Boundary::Created, &path)?;
            same(&directory_stamp, &directory.metadata().map_err(io)?, true)?;
            same(&directory_stamp, &private_directory(&path)?, true)?;
            sync_directory(&self.path)?;
            self.boundary(Boundary::DirectorySynced, &path)?;
            same(&directory_stamp, &directory.metadata().map_err(io)?, true)?;
            same(&directory_stamp, &private_directory(&path)?, true)?;
            self.fixed_anchors_with_root(false)?;
            require(
                names(&self.path, MAX_EVIDENCE_JOBS + 2, budget)? == expected_namespace,
                "Evidence namespace membership changed during admission",
            )?;
            let root_stamp = self.root.metadata().map_err(io)?;
            same(&root_stamp, &private_directory(&self.path)?, true)?;
            *self.root_stamp.borrow_mut() = root_stamp;
            let written = self.write_normal(path.join("request.json"), bytes, budget)?;
            self.fixed_anchors()?;
            same(&directory_stamp, &directory.metadata().map_err(io)?, false)?;
            same(&directory_stamp, &private_directory(&path)?, false)?;
            require(
                names(&path, 3 + MAX_EVIDENCE_JOB_OBSERVATIONS, budget)?
                    == BTreeSet::from(["request.json".into()]),
                "New request directory contains unexpected members",
            )?;
            same(
                &written.stamp,
                &ordinary_file(&written.path, MAX_EVIDENCE_JOB_REQUEST_BYTES)?,
                true,
            )?;
            let entry = self.read(&request.job_id, budget)?;
            require(
                entry
                    .slots
                    .iter()
                    .find(|slot| slot.name == "request.json")
                    .is_some_and(|slot| slot.bytes == written.bytes),
                "Written request differs from its admitted exact bytes",
            )?;
            Ok(Admission::Fresh(entry))
        })();
        finish(budget, result)
    }
    fn write_normal(
        &self,
        path: PathBuf,
        bytes: Vec<u8>,
        budget: &InspectionBudget,
    ) -> Result<Slot, ServiceError> {
        checkpoint(budget)?;
        charge(budget, bytes.len() as u64 * 2 + 4096)?;
        self.write_once(path, bytes)
    }
    fn write_once(&self, path: PathBuf, bytes: Vec<u8>) -> Result<Slot, ServiceError> {
        let (slot, _held) = write_created(path, bytes, |point, path| self.boundary(point, path))?;
        Ok(slot)
    }
    fn append_proof(&self, entry: &JobEntry, written: &Slot) -> Result<fs::Metadata, ServiceError> {
        self.fixed_anchors()?;
        let after_write = entry.directory.metadata().map_err(io)?;
        same(&entry.directory_stamp, &after_write, false)?;
        same(&after_write, &private_directory(&entry.path)?, true)?;
        for slot in &entry.slots {
            same(
                &slot.stamp,
                &ordinary_file(&slot.path, maximum(&slot.name))?,
                true,
            )?;
        }
        same(
            &written.stamp,
            &ordinary_file(&written.path, maximum(&written.name))?,
            true,
        )?;
        let mut expected = entry
            .slots
            .iter()
            .map(|slot| slot.name.clone())
            .collect::<BTreeSet<_>>();
        require(
            expected.insert(written.name.clone()),
            "Evidence slot was already occupied",
        )?;
        require(
            fixed_names(&entry.path)? == expected,
            "Evidence occupied slots differ from prior history plus the new record",
        )?;
        same(&after_write, &entry.directory.metadata().map_err(io)?, true)?;
        same(&after_write, &private_directory(&entry.path)?, true)?;
        Ok(after_write)
    }
    pub(super) fn mark_started(
        &self,
        entry: JobEntry,
        budget: &InspectionBudget,
    ) -> Result<JobEntry, ServiceError> {
        let result = (|| {
            self.verify_entry(&entry, budget)?;
            require(
                entry.view.request.is_some()
                    && entry.slots.iter().all(|slot| slot.name == "request.json"),
                "Evidence job already started or has later occupied slots",
            )?;
            charge(budget, MAX_EVIDENCE_JOB_STARTED_BYTES as u64 * 3)?;
            let bytes = envelope(
                Started { version: 1 },
                entry.view.request_sha256.as_deref().expect("valid request"),
                Kind::Started,
                0,
                MAX_EVIDENCE_JOB_STARTED_BYTES,
            )?;
            let written =
                self.write_normal(entry.view.directory.join("started.json"), bytes, budget)?;
            charge(budget, 4096 + entry.slots.len() as u64 * 512)?;
            self.append_proof(&entry, &written)?;
            let reread = self.read(&entry.view.job_id, budget)?;
            require(
                reread
                    .slots
                    .iter()
                    .find(|slot| slot.name == "started.json")
                    .is_some_and(|slot| slot.bytes == written.bytes),
                "Written started record differs from its exact bytes",
            )?;
            Ok(reread)
        })();
        finish(budget, result)
    }
    pub(super) fn reserve_finalization(
        &self,
        budget: &InspectionBudget,
    ) -> Result<Finalization, ServiceError> {
        let result = (|| {
            self.namespace(budget)?;
            charge(budget, EVIDENCE_JOB_FINALIZATION_WORK)?;
            Ok(Finalization {
                owner: self.guard.identity,
            })
        })();
        finish(budget, result)
    }
    pub(super) fn complete(
        &self,
        entry: JobEntry,
        outcome: &EvidenceOutcome,
        permit: Finalization,
    ) -> Result<JobEntry, ServiceError> {
        require(
            entry.view.started
                && !entry.slots.iter().any(|slot| {
                    slot.name == "terminal.json" || slot.name.starts_with("observation-")
                }),
            "Evidence terminal slot is already occupied or execution never started",
        )?;
        self.final_record(entry, outcome, permit, false)
    }
    pub(super) fn observe(
        &self,
        entry: JobEntry,
        outcome: &EvidenceOutcome,
        permit: Finalization,
    ) -> Result<JobEntry, ServiceError> {
        require(
            entry.view.occupied_observations < MAX_EVIDENCE_JOB_OBSERVATIONS,
            "Evidence observation capacity is full; prior history was retained",
        )?;
        self.final_record(entry, outcome, permit, true)
    }
    fn final_record(
        &self,
        mut entry: JobEntry,
        outcome: &EvidenceOutcome,
        permit: Finalization,
        observation: bool,
    ) -> Result<JobEntry, ServiceError> {
        require(
            permit.owner == self.guard.identity,
            "Finalization allowance belongs to a different journal owner",
        )?;
        // All subsequent work is the prepaid bounded private metadata operation:
        // <=16*16KiB byte passes, <=64 bounded path/stamp observations and one
        // <=12-member bounded scan. The parent constant prepays these ceilings.
        // It never consults/reset a governor and cannot perform artifact work.
        self.verify_fixed_entry(&entry)?;
        let request = entry.view.request.as_ref().ok_or_else(|| {
            ServiceError::rejected("Invalid request authorizes no journalled artifact observation")
        })?;
        outcome.validate_for(request)?;
        let fingerprint = entry
            .view
            .request_sha256
            .as_deref()
            .expect("valid request fingerprint");
        let (name, bytes, observed) = if observation {
            let sequence = entry.view.occupied_observations + 1;
            let record = EvidenceObservation {
                sequence: sequence as u32,
                previous_history_sha256: entry.view.history_sha256.clone(),
                outcome: outcome.clone(),
            };
            let bytes = envelope(
                &record,
                fingerprint,
                Kind::Observation,
                sequence as u32,
                MAX_EVIDENCE_JOB_RECORD_BYTES,
            )?;
            (
                format!("observation-{sequence:02}.json"),
                bytes,
                Some(record),
            )
        } else {
            (
                "terminal.json".into(),
                envelope(
                    outcome,
                    fingerprint,
                    Kind::Terminal,
                    0,
                    MAX_EVIDENCE_JOB_RECORD_BYTES,
                )?,
                None,
            )
        };
        require(
            bytes.len() <= MAX_EVIDENCE_JOB_RECORD_BYTES,
            "Final evidence record exceeds its prepaid bound",
        )?;
        let new_view_bytes = entry
            .view_bytes
            .checked_add(bytes.len() + 64)
            .ok_or_else(|| ServiceError::rejected("Evidence selected-job view size overflow"))?;
        require(
            new_view_bytes <= MAX_EVIDENCE_JOB_VIEW_BYTES,
            "Evidence selected-job view exceeds its bound",
        )?;
        let path = entry.view.directory.join(&name);
        let written = self.write_once(path, bytes)?;
        let directory_stamp = self.append_proof(&entry, &written)?;
        history_add(&mut entry.history, &written.name, &written.bytes);
        entry.slots.push(written);
        entry.view.history_sha256 = history_sha(&entry.history);
        if let Some(record) = observed {
            entry.view.occupied_observations = record.sequence as usize;
            entry.view.observations.push(record);
        } else {
            entry.view.terminal = Some(outcome.clone());
        }
        entry.directory_stamp = directory_stamp;
        entry.view_bytes = new_view_bytes;
        self.fixed_anchors()?;
        Ok(entry)
    }
}
