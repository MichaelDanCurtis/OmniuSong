//! Explicit full-root retained evidence, restored only into an inert namespace.
use crate::{EvidenceFailure, NativeStore, Result, StoreError};
use omniusong_recording::InspectionBudget;
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

mod export_reconciliation;
mod inert_reconciliation;
mod raw;
pub use export_reconciliation::reconcile_evidence_export;
pub use inert_reconciliation::inspect_restored_evidence;
#[cfg(test)]
mod inert_reconciliation_tests;
#[cfg(test)]
mod reviewed_restore_tests;
#[cfg(test)]
mod tests;

pub const MAX_EVIDENCE_ENTRIES: usize = 2048;
pub const MAX_EVIDENCE_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
pub const EVIDENCE_SCOPE: &str = "catalog-and-all-retained-captures-source-actions-takes";
const MAGIC: &[u8; 8] = b"OSRE\x01\r\n\0";
const FOOTER: &[u8; 8] = b"OSREEND1";
const CATALOG_SCHEMA: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceEntryKind {
    Directory,
    File,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceEntry {
    pub path: String,
    pub kind: EvidenceEntryKind,
    pub byte_length: u64,
    pub sha256: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRoot {
    pub path: String,
    pub present: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceAbsence {
    pub path: String,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceManifest {
    pub version: u32,
    pub export_id: String,
    pub scope: String,
    pub origin_root: String,
    pub catalog_schema: u32,
    pub roots: Vec<EvidenceRoot>,
    pub absences: Vec<EvidenceAbsence>,
    pub entries: Vec<EvidenceEntry>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceArchiveReceipt {
    pub path: PathBuf,
    pub export_id: String,
    pub sha256: String,
    pub byte_length: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub payload_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceArchiveInspection {
    pub receipt: EvidenceArchiveReceipt,
    pub manifest: EvidenceManifest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRestoreReceipt {
    pub directory: PathBuf,
    pub archive: EvidenceArchiveReceipt,
}

fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(StoreError::Invalid(message.into()))
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && id.bytes().any(|b| b != b'0')
}
pub(super) fn valid_path(path: &str) -> Result<()> {
    require(
        !path.is_empty()
            && path.len() <= 4096
            && !path.starts_with('/')
            && !path.contains(['\\', ':'])
            && !path.chars().any(char::is_control)
            && path.split('/').count() <= 12
            && path
                .split('/')
                .all(|s| !s.is_empty() && s != "." && s != ".."),
        "Evidence path is not a bounded ordinary relative path",
    )
}

fn bounded_json(value: &impl Serialize, budget: &InspectionBudget) -> Result<Vec<u8>> {
    struct Bytes<'a>(Vec<u8>, &'a InspectionBudget);
    impl Write for Bytes<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.1
                .charge_work(bytes.len() as u64)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            if bytes.len() > MAX_EVIDENCE_MANIFEST_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("Evidence manifest exceeds 2 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut bytes = Bytes(Vec::new(), budget);
    let result = serde_json::to_writer(&mut bytes, value);
    budget.checkpoint()?;
    result.map_err(|e| StoreError::Invalid(e.to_string()))?;
    Ok(bytes.0)
}
fn validate_manifest(manifest: &EvidenceManifest, budget: &InspectionBudget) -> Result<()> {
    require(
        manifest.version == 1
            && valid_id(&manifest.export_id)
            && manifest.scope == EVIDENCE_SCOPE
            && manifest.catalog_schema == CATALOG_SCHEMA,
        "Unsupported evidence manifest identity/version/scope/schema",
    )?;
    require(
        manifest.origin_root.starts_with('/')
            && manifest.origin_root.len() <= 4096
            && !manifest.origin_root.chars().any(char::is_control),
        "Evidence origin root is not bounded absolute provenance",
    )?;
    require(
        manifest.roots.len() == 3
            && manifest.roots.iter().map(|r| r.path.as_str()).eq([
                "captures",
                "source-actions",
                "takes",
            ]),
        "Evidence root inventory differs from the full-root scope",
    )?;
    require(
        !manifest.entries.is_empty()
            && manifest.entries.len() <= MAX_EVIDENCE_ENTRIES
            && manifest.absences.len() <= MAX_EVIDENCE_ENTRIES,
        "Evidence inventory exceeds its member bounds",
    )?;
    let mut kinds = BTreeMap::new();
    let mut aliases = BTreeSet::new();
    let mut total = 0u64;
    for entry in &manifest.entries {
        budget.checkpoint()?;
        valid_path(&entry.path)?;
        budget.charge_work(entry.path.len() as u64 + 160)?;
        require(
            kinds
                .keys()
                .next_back()
                .is_none_or(|p: &&str| *p < entry.path.as_str())
                && aliases.insert(entry.path.to_lowercase()),
            "Evidence entries repeat, alias or are not in exact path order",
        )?;
        let root = entry.path.split('/').next().expect("valid path");
        require(
            root == "catalog.sqlite3" || manifest.roots.iter().any(|r| r.path == root && r.present),
            "Evidence entry lies outside a present declared root",
        )?;
        match entry.kind {
            EvidenceEntryKind::Directory => require(
                entry.byte_length == 0 && entry.sha256.is_none(),
                "Evidence directory carries file bytes",
            )?,
            EvidenceEntryKind::File => {
                budget.observe_file_bytes(entry.byte_length)?;
                require(
                    entry
                        .sha256
                        .as_ref()
                        .is_some_and(|s| crate::archive::digest_valid(s)),
                    "Evidence file lacks a strict raw SHA256",
                )?;
                total = total
                    .checked_add(entry.byte_length)
                    .filter(|n| *n <= 1_073_741_824)
                    .ok_or_else(|| {
                        StoreError::Invalid("Evidence aggregate bytes exceed 1 GiB".into())
                    })?;
            }
        }
        if let Some((parent, _)) = entry.path.rsplit_once('/') {
            require(
                kinds.get(parent) == Some(&EvidenceEntryKind::Directory),
                "Evidence parent directory is absent or is a file",
            )?;
        } else if root == "catalog.sqlite3" {
            require(
                entry.kind == EvidenceEntryKind::File,
                "Evidence catalog is not a file",
            )?;
        } else {
            require(
                entry.kind == EvidenceEntryKind::Directory,
                "Evidence root is not a directory",
            )?;
        }
        kinds.insert(entry.path.as_str(), entry.kind);
    }
    require(
        kinds.get("catalog.sqlite3") == Some(&EvidenceEntryKind::File),
        "Evidence catalog is missing",
    )?;
    for root in &manifest.roots {
        require(
            root.present == kinds.contains_key(root.path.as_str()),
            "Evidence root presence contradicts its entries",
        )?;
    }
    let mut missing = BTreeSet::new();
    for absence in &manifest.absences {
        budget.charge_work((absence.path.len() + absence.reason.len()) as u64 + 80)?;
        valid_path(&absence.path)?;
        require(
            absence.reason.len() <= 256
                && !absence.reason.is_empty()
                && !absence.reason.chars().any(char::is_control)
                && !kinds.contains_key(absence.path.as_str())
                && missing.insert(&absence.path),
            "Evidence absence is repeated or contradicts retained bytes",
        )?;
        let root = absence.path.split('/').next().expect("valid path");
        require(
            ["captures", "source-actions", "takes"].contains(&root),
            "Evidence absence escapes its scope",
        )?;
    }
    Ok(())
}
fn ordinary_directory(path: &Path) -> Result<()> {
    require(
        cfg!(unix),
        "Raw evidence identity is qualified on Unix only",
    )?;
    for ancestor in path.ancestors() {
        let m = fs::symlink_metadata(ancestor)?;
        require(
            m.is_dir() && !m.file_type().is_symlink(),
            "Evidence parent is redirected or nonordinary",
        )?;
    }
    require(
        fs::canonicalize(path)? == path,
        "Evidence parent is not its exact canonical path",
    )
}
fn destination(path: &Path, root: &Path) -> Result<PathBuf> {
    require(
        path.is_absolute() && path.file_name().is_some(),
        "Evidence destination needs an absolute file/directory path",
    )?;
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Invalid("Missing evidence destination parent".into()))?;
    ordinary_directory(parent)?;
    require(
        !parent.starts_with(root),
        "Evidence destination must be outside the live native root",
    )?;
    Ok(path.to_owned())
}
fn archive_filename(path: &Path) -> Result<()> {
    // Native catalog, recording and owned-object databases use this reserved
    // suffix. Refuse an accidental live-DB selection before a raw FD can close
    // and release its same-process SQLite transaction locks on header failure.
    let name = path
        .file_name()
        .ok_or_else(|| StoreError::Invalid("Evidence archive needs a file name".into()))?
        .as_encoded_bytes();
    require(
        ![
            b".sqlite3".as_slice(),
            b".sqlite3-wal",
            b".sqlite3-shm",
            b".sqlite3-journal",
        ]
        .iter()
        .any(|suffix| {
            name.get(name.len().saturating_sub(suffix.len())..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
        }),
        "Native SQLite names and sidecars are reserved; select an OSRE evidence archive",
    )?;
    let mut owner = path.as_os_str().to_os_string();
    owner.push(".owner.lock");
    match fs::symlink_metadata(PathBuf::from(owner)) {
        Ok(_) => Err(StoreError::Invalid(
            "Owned recording name is reserved; select an OSRE evidence archive".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
fn same(before: &fs::Metadata, after: &fs::Metadata) -> Result<()> {
    require(
        after.is_file() && !after.file_type().is_symlink() && before.len() == after.len(),
        "Evidence file identity or length changed",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            before.nlink() == 1
                && after.nlink() == 1
                && before.dev() == after.dev()
                && before.ino() == after.ino()
                && before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "Evidence file was aliased or changed during observation",
        )?;
    }
    Ok(())
}
fn open_locked(path: &Path) -> Result<(File, fs::Metadata)> {
    ordinary_directory(
        path.parent()
            .ok_or_else(|| StoreError::Invalid("Missing archive parent".into()))?,
    )?;
    let before = fs::symlink_metadata(path)?;
    same(&before, &before)?;
    let file = OpenOptions::new().read(true).open(path)?;
    same(&before, &file.metadata()?)?;
    file.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => {
            StoreError::Recording(omniusong_recording::RecordingError::Owned)
        }
        std::fs::TryLockError::Error(e) => StoreError::Io(e),
    })?;
    same(&before, &fs::symlink_metadata(path)?)?;
    Ok((file, before))
}
fn verify_archive_anchor(path: &Path, file: &File, before: &fs::Metadata) -> Result<()> {
    ordinary_directory(path.parent().expect("anchored parent"))?;
    same(before, &file.metadata()?)?;
    same(before, &fs::symlink_metadata(path)?)
}
fn read_exact(file: &mut File, bytes: &mut [u8], budget: &InspectionBudget) -> Result<()> {
    budget.charge_work((bytes.len() as u64).saturating_mul(2))?;
    file.read_exact(bytes)?;
    Ok(())
}
fn read_archive(
    file: &mut File,
    path: &Path,
    budget: &InspectionBudget,
    restored: Option<&Path>,
) -> Result<EvidenceArchiveInspection> {
    file.seek(SeekFrom::Start(0))?;
    budget.observe_file_bytes(file.metadata()?.len())?;
    let mut header = [0u8; 16];
    read_exact(file, &mut header, budget)?;
    require(
        &header[..8] == MAGIC,
        "Unsupported evidence container magic/version",
    )?;
    let n = u64::from_le_bytes(header[8..].try_into().expect("eight bytes"));
    require(
        n > 0 && n <= MAX_EVIDENCE_MANIFEST_BYTES as u64,
        "Evidence manifest length exceeds its bound",
    )?;
    budget.charge_work(n)?;
    let mut bytes = vec![0u8; n as usize];
    read_exact(file, &mut bytes, budget)?;
    let manifest: EvidenceManifest =
        serde_json::from_slice(&bytes).map_err(|e| StoreError::Invalid(e.to_string()))?;
    validate_manifest(&manifest, budget)?;
    require(
        bounded_json(&manifest, budget)? == bytes,
        "Evidence manifest is not exact canonical JSON",
    )?;
    let mut content = Sha256::new();
    content.update(header);
    content.update(&bytes);
    let mut length = 16 + n;
    let mut files = 0;
    let mut directories = 0;
    let mut payload = 0u64;
    let mut buffer = [0u8; 65536];
    let mut catalog = Vec::new();
    for (index, entry) in manifest.entries.iter().enumerate() {
        if entry.kind == EvidenceEntryKind::Directory {
            directories += 1;
            continue;
        }
        files += 1;
        if entry.path == "catalog.sqlite3" {
            // One bounded catalog buffer and one SQLite-owned in-memory copy.
            // Cursor cannot fail its exact read; never pass a fallible file or
            // governor reader to this rusqlite version's allocation helper.
            budget.charge_work(entry.byte_length.checked_mul(2).ok_or_else(|| {
                StoreError::Invalid("Catalog allocation credit overflow".into())
            })?)?;
            catalog
                .try_reserve_exact(entry.byte_length as usize)
                .map_err(|_| {
                    StoreError::Invalid("Cannot reserve bounded catalog evidence memory".into())
                })?;
        }
        let mut output = if let Some(directory) = restored {
            Some(exclusive_file(
                &directory.join("objects").join(format!("{index:04}.bin")),
                false,
            )?)
        } else {
            None
        };
        let mut remaining = entry.byte_length;
        let mut hash = Sha256::new();
        while remaining > 0 {
            let size = remaining.min(buffer.len() as u64) as usize;
            budget.charge_work(size as u64)?;
            read_exact(file, &mut buffer[..size], budget)?;
            hash.update(&buffer[..size]);
            content.update(&buffer[..size]);
            if entry.path == "catalog.sqlite3" {
                catalog.extend_from_slice(&buffer[..size]);
            }
            if let Some(output) = output.as_mut() {
                budget.charge_work(size as u64)?;
                output.write_all(&buffer[..size])?;
            }
            remaining -= size as u64;
        }
        require(
            Some(format!("{:x}", hash.finalize())) == entry.sha256,
            "Evidence object bytes differ from their inventory SHA256",
        )?;
        if let Some(output) = output {
            output.sync_all()?;
        }
        length = length
            .checked_add(entry.byte_length)
            .ok_or_else(|| StoreError::Invalid("Evidence length overflow".into()))?;
        payload += entry.byte_length;
    }
    let mut footer = [0u8; 40];
    read_exact(file, &mut footer, budget)?;
    require(
        &footer[..32] == content.clone().finalize().as_slice() && &footer[32..] == FOOTER,
        "Evidence final digest/footer is missing or differs",
    )?;
    budget.charge_work(1)?;
    let mut extra = [0u8; 1];
    require(
        file.read(&mut extra)? == 0,
        "Evidence container has surplus trailing bytes",
    )?;
    content.update(footer);
    length += 40;
    require(
        file.metadata()?.len() == length,
        "Evidence physical length differs from its declared closure",
    )?;
    validate_catalog_closure(&catalog, &manifest, budget)?;
    if let Some(directory) = restored {
        write_exclusive(&directory.join("manifest.json"), &bytes, budget)?;
    }
    Ok(EvidenceArchiveInspection {
        receipt: EvidenceArchiveReceipt {
            path: path.to_owned(),
            export_id: manifest.export_id.clone(),
            sha256: format!("{:x}", content.finalize()),
            byte_length: length,
            file_count: files,
            directory_count: directories,
            payload_bytes: payload,
        },
        manifest,
    })
}
fn write_exclusive(path: &Path, bytes: &[u8], budget: &InspectionBudget) -> Result<()> {
    budget.charge_work(bytes.len() as u64)?;
    let mut file = exclusive_file(path, false)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn exclusive_file(path: &Path, read: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(read).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
/// Authenticates byte and known-reference closure. Source journals are never
/// opened in SQLite; only the catalog is validated read-only in memory.
pub fn inspect_evidence_archive(
    path: impl AsRef<Path>,
    budget: &InspectionBudget,
) -> Result<EvidenceArchiveInspection> {
    let result = (|| {
        let path = path.as_ref();
        archive_filename(path)?;
        let _input = crate::retained_file_coordination::evidence_input(path)?;
        let (mut file, before) = open_locked(path)?;
        let result = read_archive(&mut file, path, budget, None)?;
        verify_archive_anchor(path, &file, &before)?;
        Ok(result)
    })();
    budget.checkpoint()?;
    result
}

fn required_links(
    connection: &Connection,
    origin_root: &Path,
    entries: &[EvidenceEntry],
    budget: &InspectionBudget,
) -> Result<Vec<EvidenceAbsence>> {
    // Prepay the complete compiled raw-history read/hash ceilings BEFORE the
    // bounded loaders allocate any SQL values. This is conservative credit,
    // not a claim that a small catalog actually consumed the maximum bytes.
    budget.charge_work(3 * (32 * 16 * 16_384 + 32 * 65_536 + 32 * 16 * 16_384))?;
    budget.visit_rows(32 + 512 + 512 + 4097)?;
    budget.charge_work(entries.iter().map(|e| e.path.len() as u64 + 80).sum())?;
    let existing: BTreeSet<&str> = entries
        .iter()
        .filter(|e| e.kind == EvidenceEntryKind::File)
        .map(|e| e.path.as_str())
        .collect();
    let mut absent = BTreeMap::new();
    let mut check = |path: String, required: bool, reason: &str| -> Result<()> {
        if !existing.contains(path.as_str()) {
            require(
                !required,
                &format!("Complete evidence export is missing required bytes: {path}"),
            )?;
            absent.insert(path, reason.to_owned());
        }
        Ok(())
    };
    let parse = |bytes: &[u8]| -> Result<serde_json::Value> {
        budget.charge_work(bytes.len() as u64)?;
        serde_json::from_slice(bytes).map_err(|e| StoreError::Invalid(e.to_string()))
    };
    for row in crate::performance_intents::load_history(connection)? {
        let value = parse(&row.payload)?;
        let path = format!("captures/{}/take.sqlite3", row.id);
        require(
            value["version"] == 1
                && value["journal_path"].as_str() == origin_root.join(&path).to_str()
                && value.get("descriptor_sha256").is_some(),
            "Unsupported recording-intent linkage; complete evidence was refused",
        )?;
        let descriptor = &value["descriptor_sha256"];
        require(
            descriptor.is_null()
                || descriptor
                    .as_str()
                    .is_some_and(crate::archive::digest_valid),
            "Unsupported recording descriptor linkage",
        )?;
        check(
            path,
            !descriptor.is_null(),
            "Unobserved recording request does not prove an original exists",
        )?;
    }
    let history = crate::performance_source_actions::load_history(connection)?;
    let mut stages = BTreeMap::new();
    for row in &history.reviews {
        let value = parse(&row.payload)?;
        let stage = value["stage_id"]
            .as_str()
            .filter(|s| valid_id(s))
            .ok_or_else(|| {
                StoreError::Invalid("Unsupported retained-review source linkage".into())
            })?;
        let path = format!("captures/{stage}/take.sqlite3");
        require(
            value["version"] == 1
                && value["journal_path"].as_str() == origin_root.join(&path).to_str(),
            "Retained review points outside its original namespace",
        )?;
        check(path, true, "Reviewed original is required")?;
        stages.insert(row.id.as_str(), stage.to_owned());
    }
    for row in &history.revisions {
        let value = parse(&row.payload)?;
        require(
            value["version"] == 1 && value["id"].as_str() == Some(row.id.as_str()),
            "Unsupported action identity/version linkage",
        )?;
        if row.revision == 1 {
            require(
                value["stage_id"].as_str() == stages.get(row.id.as_str()).map(String::as_str),
                "Prepared action differs from its review source",
            )?;
            for part in ["evidence", "recovery"] {
                check(
                    format!("source-actions/{}/{part}/take.sqlite3", row.id),
                    false,
                    "Prepared request does not prove filesystem execution",
                )?;
            }
        } else if let Some(phase) = value.get("phase") {
            let state = phase["state"].as_str();
            require(
                matches!(state, Some("started" | "verified")),
                "Unsupported source-action phase linkage",
            )?;
            for part in ["evidence", "recovery"] {
                check(
                    format!("source-actions/{}/{part}/take.sqlite3", row.id),
                    state == Some("verified"),
                    "Started does not prove artifact creation or completion",
                )?;
            }
        } else {
            require(
                matches!(
                    value["decision"]["state"].as_str(),
                    Some(
                        "kept_locally"
                            | "returned_to_review"
                            | "attachment_prepared"
                            | "attachment_retired"
                            | "attachment_committed"
                    )
                ),
                "Unsupported source-action decision linkage",
            )?;
        }
    }
    let mut statement=connection.prepare("SELECT CASE WHEN typeof(object_sha256)='text' AND octet_length(object_sha256)=64 THEN object_sha256 END FROM native_takes LIMIT 4097")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        budget.checkpoint()?;
        count += 1;
        require(count <= 4096, "Owned object linkage exceeds its row bound")?;
        let sha: Option<String> = row.get(0)?;
        let sha = sha
            .filter(|s| crate::archive::digest_valid(s))
            .ok_or_else(|| StoreError::Invalid("Invalid owned object linkage".into()))?;
        check(
            format!("takes/{sha}.sqlite3"),
            true,
            "Registered original is required",
        )?;
    }
    Ok(absent
        .into_iter()
        .map(|(path, reason)| EvidenceAbsence { path, reason })
        .collect())
}
fn validate_catalog_closure(
    bytes: &[u8],
    manifest: &EvidenceManifest,
    budget: &InspectionBudget,
) -> Result<()> {
    require(
        bytes.len() >= 100
            && &bytes[..16] == b"SQLite format 3\0"
            && bytes[18] == 1
            && bytes[19] == 1,
        "Archived catalog is not a complete rollback-mode SQLite snapshot",
    )?;
    budget.charge_work(4096)?;
    let mut connection = Connection::open_in_memory()?;
    connection.deserialize_read_exact(
        rusqlite::MAIN_DB,
        std::io::Cursor::new(bytes),
        bytes.len(),
        true,
    )?;
    budget.install_sqlite_progress(&connection)?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; PRAGMA temp_store=MEMORY;")?;
    require(
        connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?
            == manifest.catalog_schema,
        "Archived catalog schema differs from the declared snapshot",
    )?;
    for (name, columns) in [
        (
            "native_takes",
            &[
                "take_id",
                "manifest_sha256",
                "object_sha256",
                "byte_length",
                "manifest_json",
            ][..],
        ),
        (
            "recording_intents",
            &["id", "revision", "payload", "payload_sha256"][..],
        ),
        (
            "performance_source_reviews",
            &["id", "payload", "payload_sha256"][..],
        ),
        (
            "performance_source_actions",
            &[
                "id",
                "revision",
                "review_blob_sha256",
                "payload",
                "payload_sha256",
            ][..],
        ),
    ] {
        budget.charge_work(16_464)?;
        let row:(String,Option<String>)=connection.query_row("SELECT substr(type,1,17),CASE WHEN typeof(sql)='text' AND octet_length(sql)<=16384 THEN sql END FROM sqlite_schema WHERE name=?1",[name],|r|Ok((r.get(0)?,r.get(1)?)))?;
        require(
            row.0 == "table"
                && row.1.is_some_and(|sql| {
                    sql.trim_start()
                        .to_ascii_uppercase()
                        .starts_with("CREATE TABLE ")
                }),
            "Archived catalog linkage is not an ordinary supported table",
        )?;
        // A generated column could evaluate an allocating builtin before a
        // typeof/length guard. VM steps alone cannot bound one such C call.
        let mut statement = connection.prepare(&format!("PRAGMA main.table_xinfo({name})"))?;
        let mut rows = statement.query([])?;
        let mut observed = BTreeSet::new();
        while let Some(row) = rows.next()? {
            budget.visit_rows(1)?;
            budget.charge_work(16_464)?;
            require(
                observed.len() < columns.len(),
                "Archived catalog linkage column layout is generated, hidden or unsupported",
            )?;
            let hidden: i64 = row.get(6)?;
            let column: String = row.get(1)?;
            let default: Option<String> = row.get(4)?;
            require(
                hidden == 0
                    && default.is_none()
                    && column.len() <= 96
                    && columns.contains(&column.as_str())
                    && observed.insert(column),
                "Archived catalog linkage column layout is generated, hidden or unsupported",
            )?;
        }
        require(
            observed.len() == columns.len(),
            "Archived catalog linkage column layout is generated, hidden or unsupported",
        )?;
    }
    let absences = required_links(
        &connection,
        Path::new(&manifest.origin_root),
        &manifest.entries,
        budget,
    )?;
    require(
        absences == manifest.absences,
        "Archived catalog required members or exact absence facts differ from the declared inventory",
    )?;
    budget.checkpoint()?;
    Ok(())
}
fn catalog_snapshot(
    store: &NativeStore,
    path: &Path,
    budget: &InspectionBudget,
) -> std::result::Result<(), EvidenceFailure> {
    let reservation = exclusive_file(path, false)?;
    let mut target = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    target.busy_timeout(Duration::from_secs(2))?;
    target.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;")?;
    let page_size = crate::archive::unsigned(
        store
            .connection
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(EvidenceFailure::live_catalog)?,
        "page size",
    )?;
    let pages = crate::archive::unsigned(
        store
            .connection
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .map_err(EvidenceFailure::live_catalog)?,
        "page count",
    )?;
    budget.observe_file_bytes(
        pages
            .checked_mul(page_size)
            .ok_or_else(|| StoreError::Invalid("Catalog size overflow".into()))?,
    )?;
    {
        let backup = Backup::new(&store.connection, &mut target)
            .map_err(EvidenceFailure::backup_transfer)?;
        loop {
            budget.charge_work(32 * page_size)?;
            match backup.step(16).map_err(EvidenceFailure::backup_transfer)? {
                StepResult::Done => break,
                StepResult::More => (),
                StepResult::Busy | StepResult::Locked => {
                    return Err(EvidenceFailure::backup_transfer(StoreError::Conflict));
                }
                _ => {
                    return Err(EvidenceFailure::backup_transfer(StoreError::Invalid(
                        "Unsupported SQLite Backup result".into(),
                    )));
                }
            }
        }
    }
    target.close().map_err(|(_, e)| StoreError::Database(e))?;
    // Only this private logical snapshot is normalized. Reopen so its pager
    // sees a copied WAL header; never checkpoint or rewrite the live catalog.
    budget.charge_work(131_072)?;
    let normalized = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    normalized.busy_timeout(Duration::from_secs(2))?;
    budget.install_sqlite_progress(&normalized)?;
    normalized.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")?;
    normalized
        .close()
        .map_err(|(_, e)| StoreError::Database(e))?;
    reservation.sync_all()?;
    budget.observe_file_bytes(reservation.metadata()?.len())?;
    Ok(())
}
fn hash_file(file: &mut File, budget: &InspectionBudget) -> Result<(u64, String)> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut bytes = [0u8; 65536];
    let mut remaining = file.metadata()?.len();
    budget.observe_file_bytes(remaining)?;
    while remaining > 0 {
        let n = remaining.min(bytes.len() as u64) as usize;
        read_exact(file, &mut bytes[..n], budget)?;
        size += n as u64;
        remaining -= n as u64;
        hash.update(&bytes[..n]);
    }
    budget.charge_work(1)?;
    require(
        file.read(&mut bytes[..1])? == 0,
        "Catalog stage grew during hash",
    )?;
    Ok((size, format!("{:x}", hash.finalize())))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvidenceExportBoundary {
    AfterInventory,
    BeforeFinalProof,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvidenceRestoreBoundary {
    AfterDirectoryCreated,
    BeforeCompletion,
    AfterCompletionWritten,
}

struct RestoreAnchors {
    directory: PathBuf,
    root: File,
    root_stamp: fs::Metadata,
    objects: File,
    objects_stamp: fs::Metadata,
}
impl RestoreAnchors {
    fn new(directory: &Path) -> Result<Self> {
        let root = File::open(directory)?;
        let objects = File::open(directory.join("objects"))?;
        let value = Self {
            directory: directory.to_owned(),
            root_stamp: root.metadata()?,
            objects_stamp: objects.metadata()?,
            root,
            objects,
        };
        value.verify()?;
        Ok(value)
    }
    fn verify(&self) -> Result<()> {
        for (path, file, before) in [
            (&self.directory, &self.root, &self.root_stamp),
            (
                &self.directory.join("objects"),
                &self.objects,
                &self.objects_stamp,
            ),
        ] {
            ordinary_directory(path)?;
            let current = fs::symlink_metadata(path)?;
            let held = file.metadata()?;
            require(
                current.is_dir() && held.is_dir(),
                "Restored evidence directory became nonordinary",
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                require(
                    before.dev() == current.dev()
                        && before.ino() == current.ino()
                        && before.dev() == held.dev()
                        && before.ino() == held.ino()
                        && current.mode() & 0o077 == 0,
                    "Private restored evidence directory was replaced or exposed",
                )?;
            }
        }
        Ok(())
    }
    fn members(
        &self,
        manifest: &EvidenceManifest,
        complete: bool,
        budget: &InspectionBudget,
    ) -> Result<()> {
        self.verify()?;
        fn names(path: &Path, budget: &InspectionBudget) -> Result<BTreeSet<String>> {
            let mut result = BTreeSet::new();
            for entry in fs::read_dir(path)? {
                budget.checkpoint()?;
                require(
                    result.len() <= MAX_EVIDENCE_ENTRIES,
                    "Surplus restored evidence members",
                )?;
                let entry = entry?;
                let name = entry.file_name().into_string().map_err(|_| {
                    StoreError::Invalid("Unexpected nonUTF8 restored evidence member".into())
                })?;
                budget.charge_work(name.len() as u64 + 80)?;
                require(result.insert(name), "Duplicate restored evidence member")?;
            }
            Ok(result)
        }
        let mut top = BTreeSet::from(["objects".into(), "manifest.json".into()]);
        if complete {
            top.insert("complete.json".into());
        }
        require(
            names(&self.directory, budget)? == top,
            "Restored evidence top-level membership changed",
        )?;
        let expected = manifest
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == EvidenceEntryKind::File)
            .map(|(i, _)| format!("{i:04}.bin"))
            .collect();
        require(
            names(&self.directory.join("objects"), budget)? == expected,
            "Restored evidence object membership changed",
        )?;
        self.verify()
    }
    fn prove_objects(&self, manifest: &EvidenceManifest, budget: &InspectionBudget) -> Result<()> {
        self.members(manifest, false, budget)?;
        for (index, entry) in manifest.entries.iter().enumerate() {
            if entry.kind != EvidenceEntryKind::File {
                continue;
            }
            budget.checkpoint()?;
            let path = self
                .directory
                .join("objects")
                .join(format!("{index:04}.bin"));
            let (mut file, before) = open_locked(&path)?;
            require(
                hash_file(&mut file, budget)?
                    == (
                        entry.byte_length,
                        entry.sha256.clone().expect("validated file hash"),
                    ),
                "Restored evidence object changed before completion",
            )?;
            verify_archive_anchor(&path, &file, &before)?;
            self.verify()?;
        }
        let bytes = bounded_json(manifest, budget)?;
        budget.charge_work(bytes.len() as u64)?;
        let path = self.directory.join("manifest.json");
        let (mut file, before) = open_locked(&path)?;
        require(
            hash_file(&mut file, budget)? == (bytes.len() as u64, crate::checksum(&bytes)),
            "Restored evidence manifest changed before completion",
        )?;
        verify_archive_anchor(&path, &file, &before)?;
        self.members(manifest, false, budget)
    }
}
impl NativeStore {
    /// Explicitly selects every retained byte in the three native namespaces.
    /// A completed same-ID destination replays without consulting live sources.
    pub fn export_retained_evidence_full_root(
        &mut self,
        path: impl AsRef<Path>,
        export_id: &str,
        budget: &InspectionBudget,
    ) -> Result<EvidenceArchiveReceipt> {
        self.export_retained_evidence_full_root_typed(path, export_id, budget)
            .map_err(EvidenceFailure::into_store_error)
    }
    /// Preserves the failing resource domain and any later sticky budget stop.
    /// New background jobs must use this API to distinguish archived SQLite
    /// bytes from live-catalog or mixed-source SQLite Backup failures.
    pub fn export_retained_evidence_full_root_typed(
        &mut self,
        path: impl AsRef<Path>,
        export_id: &str,
        budget: &InspectionBudget,
    ) -> std::result::Result<EvidenceArchiveReceipt, EvidenceFailure> {
        let result = self.export_evidence_typed_using(path.as_ref(), export_id, budget, |_| Ok(()));
        crate::evidence_failure::finish(budget, result)
    }
    #[cfg(test)]
    fn export_evidence_using(
        &mut self,
        path: &Path,
        export_id: &str,
        budget: &InspectionBudget,
        observe: impl FnMut(EvidenceExportBoundary) -> Result<()>,
    ) -> Result<EvidenceArchiveReceipt> {
        self.export_evidence_typed_using(path, export_id, budget, observe)
            .map_err(EvidenceFailure::into_store_error)
    }
    // Private fault/race observations never become user actions or authority.
    fn export_evidence_typed_using(
        &mut self,
        path: &Path,
        export_id: &str,
        budget: &InspectionBudget,
        mut observe: impl FnMut(EvidenceExportBoundary) -> Result<()>,
    ) -> std::result::Result<EvidenceArchiveReceipt, EvidenceFailure> {
        require(
            valid_id(export_id),
            "Evidence export needs a fresh nonzero32hex identity",
        )?;
        budget.checkpoint()?;
        let path = destination(path, &self.root)?;
        archive_filename(&path)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let observed = inspect_evidence_archive(&path, budget)?;
                require(
                    observed.receipt.export_id == export_id
                        && Some(observed.manifest.origin_root.as_str()) == self.root.to_str(),
                    "Existing evidence destination belongs to a different export or origin root",
                )?;
                return Ok(observed.receipt);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let _evidence = crate::retained_file_coordination::evidence(&self.root, &path)?;
        let catalog_source_path = self.root.join("studio.sqlite3");
        // Do not open/close another raw FD or apply std file locks on an active
        // SQLite database: POSIX closing can release this process's SQLite
        // locks, and macOS whole-file locks directly conflict with them.
        ordinary_directory(catalog_source_path.parent().expect("native root"))?;
        let catalog_source_stamp = fs::symlink_metadata(&catalog_source_path)?;
        same(&catalog_source_stamp, &catalog_source_stamp)?;
        let data_version: i64 = self
            .connection
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .map_err(EvidenceFailure::live_catalog)?;
        let raw = raw::RawEvidenceSet::acquire(&self.root, budget)?;
        observe(EvidenceExportBoundary::AfterInventory)?;
        let absences = required_links(&self.connection, &self.root, raw.entries(), budget)
            .map_err(EvidenceFailure::live_catalog)?;
        let stage = crate::archive::private_directory(
            path.parent().expect("validated parent"),
            "evidence-catalog",
        )?;
        let catalog_path = stage.join("catalog.sqlite3");
        catalog_snapshot(self, &catalog_path, budget)?;
        let (mut catalog, catalog_stamp) = open_locked(&catalog_path)?;
        let (catalog_length, catalog_sha) = hash_file(&mut catalog, budget)?;
        budget.charge_work(
            raw.entries()
                .iter()
                .map(|e| e.path.len() as u64 + 160)
                .sum(),
        )?;
        let mut entries = raw.entries().to_vec();
        entries.push(EvidenceEntry {
            path: "catalog.sqlite3".into(),
            kind: EvidenceEntryKind::File,
            byte_length: catalog_length,
            sha256: Some(catalog_sha),
        });
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = EvidenceManifest {
            version: 1,
            export_id: export_id.into(),
            scope: EVIDENCE_SCOPE.into(),
            origin_root: self
                .root
                .to_str()
                .ok_or_else(|| StoreError::Invalid("Evidence origin is not UTF8".into()))?
                .into(),
            catalog_schema: self
                .connection
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(EvidenceFailure::live_catalog)?,
            roots: raw.roots().to_vec(),
            absences,
            entries,
        };
        validate_manifest(&manifest, budget)?;
        let bytes = bounded_json(&manifest, budget)?;
        let total = manifest
            .entries
            .iter()
            .try_fold(56 + bytes.len() as u64, |n, e| {
                n.checked_add(e.byte_length)
                    .ok_or_else(|| StoreError::Invalid("Evidence size overflow".into()))
            })?;
        budget.observe_file_bytes(total)?;
        let mut file = exclusive_file(&path, true)?;
        file.try_lock()
            .map_err(|e| StoreError::Invalid(e.to_string()))?;
        struct Hashed<'a> {
            file: &'a mut File,
            hash: Sha256,
            budget: &'a InspectionBudget,
        }
        impl Write for Hashed<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.budget
                    .charge_work(bytes.len() as u64)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                self.file.write_all(bytes)?;
                self.hash.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.file.flush()
            }
        }
        let mut out = Hashed {
            file: &mut file,
            hash: Sha256::new(),
            budget,
        };
        out.write_all(MAGIC)?;
        out.write_all(&(bytes.len() as u64).to_le_bytes())?;
        out.write_all(&bytes)?;
        for entry in &manifest.entries {
            if entry.kind != EvidenceEntryKind::File {
                continue;
            }
            if entry.path == "catalog.sqlite3" {
                catalog.seek(SeekFrom::Start(0))?;
                let mut remaining = catalog_length;
                let mut b = [0u8; 65536];
                while remaining > 0 {
                    let n = remaining.min(b.len() as u64) as usize;
                    read_exact(&mut catalog, &mut b[..n], budget)?;
                    out.write_all(&b[..n])?;
                    remaining -= n as u64;
                }
                verify_archive_anchor(&catalog_path, &catalog, &catalog_stamp)?;
            } else {
                raw.copy_file(&entry.path, &mut out, budget)?;
            }
        }
        observe(EvidenceExportBoundary::BeforeFinalProof)?;
        raw.verify(budget)?;
        require(
            self.connection
                .pragma_query_value(None, "data_version", |r| r.get::<_, i64>(0))
                .map_err(EvidenceFailure::live_catalog)?
                == data_version,
            "Catalog changed during evidence export; incomplete bytes were retained",
        )?;
        ordinary_directory(catalog_source_path.parent().expect("native root"))?;
        same(
            &catalog_source_stamp,
            &fs::symlink_metadata(&catalog_source_path)?,
        )?;
        budget.checkpoint()?;
        let digest = out.hash.finalize();
        file.write_all(&digest)?;
        file.write_all(FOOTER)?;
        file.sync_all()?;
        crate::archive::sync_directory(path.parent().expect("validated parent"))?;
        let before = file.metadata()?;
        same(&before, &fs::symlink_metadata(&path)?)?;
        let observed = read_archive(&mut file, &path, budget, None)?;
        verify_archive_anchor(&path, &file, &before)?;
        require(
            observed.manifest == manifest,
            "Written evidence archive differs from its admitted closure",
        )?;
        drop(catalog);
        fs::remove_file(&catalog_path)?;
        fs::remove_dir(&stage)?;
        Ok(observed.receipt)
    }
    /// Restores numbered raw objects outside this live root without binding a
    /// prior review. No database opens, action admission, project import,
    /// recovery or attachment follows.
    /// A late error/cancellation may coexist with complete inert evidence.
    /// Inspect existing artifacts; do not infer rollback or retry create-new.
    pub fn restore_retained_evidence(
        &self,
        archive: impl AsRef<Path>,
        directory: impl AsRef<Path>,
        budget: &InspectionBudget,
    ) -> Result<EvidenceRestoreReceipt> {
        self.restore_evidence_using(archive.as_ref(), directory.as_ref(), budget, |_, _| Ok(()))
    }
    /// Restores inert evidence only when the same held archive read matches the
    /// strict whole-container SHA256 from the caller's prior inspection.
    /// A digest mismatch precedes completion, but a later error/cancellation
    /// may leave `complete.json` and all objects. That outcome is uncertain and
    /// needs read-only artifact reconciliation, never a blind create-new retry.
    pub fn restore_retained_evidence_reviewed(
        &self,
        archive: impl AsRef<Path>,
        directory: impl AsRef<Path>,
        expected_archive_sha256: &str,
        budget: &InspectionBudget,
    ) -> Result<EvidenceRestoreReceipt> {
        self.restore_evidence_with_review_using(
            archive.as_ref(),
            directory.as_ref(),
            Some(expected_archive_sha256),
            budget,
            |_, _| Ok(()),
        )
    }
    fn restore_evidence_using(
        &self,
        archive: &Path,
        directory: &Path,
        budget: &InspectionBudget,
        observe: impl FnMut(EvidenceRestoreBoundary, &Path) -> Result<()>,
    ) -> Result<EvidenceRestoreReceipt> {
        self.restore_evidence_with_review_using(archive, directory, None, budget, observe)
    }
    fn restore_evidence_with_review_using(
        &self,
        archive: &Path,
        directory: &Path,
        expected_archive_sha256: Option<&str>,
        budget: &InspectionBudget,
        mut observe: impl FnMut(EvidenceRestoreBoundary, &Path) -> Result<()>,
    ) -> Result<EvidenceRestoreReceipt> {
        let result = (|| {
            budget.checkpoint()?;
            if let Some(expected) = expected_archive_sha256 {
                require(
                    crate::archive::digest_valid(expected),
                    "Reviewed evidence restore needs a strict lowercase64hex archive SHA256",
                )?;
            }
            let directory = destination(directory, &self.root)?;
            archive_filename(archive)?;
            let _paths = crate::retained_file_coordination::evidence_restore(archive, &directory)?;
            let (mut file, before) = open_locked(archive)?;
            budget.checkpoint()?;
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&directory)?;
            builder.create(directory.join("objects"))?;
            let anchors = RestoreAnchors::new(&directory)?;
            observe(EvidenceRestoreBoundary::AfterDirectoryCreated, &directory)?;
            anchors.verify()?;
            let inspected = read_archive(&mut file, archive, budget, Some(&directory))?;
            verify_archive_anchor(archive, &file, &before)?;
            if let Some(expected) = expected_archive_sha256 {
                require(
                    inspected.receipt.sha256 == expected,
                    "Evidence archive differs from the reviewed SHA256; inert partial evidence was retained",
                )?;
            }
            observe(EvidenceRestoreBoundary::BeforeCompletion, &directory)?;
            anchors.prove_objects(&inspected.manifest, budget)?;
            crate::archive::sync_directory(&directory.join("objects"))?;
            crate::archive::sync_directory(&directory)?;
            let receipt = EvidenceRestoreReceipt {
                directory: directory.clone(),
                archive: inspected.receipt,
            };
            budget.checkpoint()?;
            let completion = bounded_json(&receipt, budget)?;
            verify_archive_anchor(archive, &file, &before)?;
            write_exclusive(&directory.join("complete.json"), &completion, budget)?;
            observe(EvidenceRestoreBoundary::AfterCompletionWritten, &directory)?;
            crate::archive::sync_directory(&directory)?;
            crate::archive::sync_directory(directory.parent().expect("validated parent"))?;
            anchors.members(&inspected.manifest, true, budget)?;
            Ok(receipt)
        })();
        budget.checkpoint()?;
        result
    }
}
