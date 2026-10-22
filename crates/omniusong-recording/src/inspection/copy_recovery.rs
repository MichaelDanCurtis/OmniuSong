//! Explicit whole-file evidence preservation and recovered-only copy authority.
use super::*;
use crate::InspectionUsage;
use std::io::Write;

pub const COPY_RECOVERY_MAX_ARTIFACT_BYTES: u64 = 1_073_741_824;
pub const COPY_RECOVERY_MAX_WORKING_BYTES: u64 = 536_870_912;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCopyPaths {
    pub action: PathBuf,
    pub evidence: PathBuf,
    pub working: PathBuf,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyRecoveryBoundary {
    NoArtifacts,
    ExistingArtifacts,
    ActionCreated,
    EvidenceCreated,
    EvidenceVerified,
    WorkingCreated,
    WorkingVerified,
    RecoveryMayHaveWritten,
    RecoveredVerified,
}
#[derive(Debug)]
pub struct CopyRecoveryError {
    pub cause: RecordingError,
    pub paths: Box<RecoveryCopyPaths>,
    pub boundary: CopyRecoveryBoundary,
}
impl std::fmt::Display for CopyRecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Copy recovery {:?}: {}; artifact location {}",
            self.boundary,
            self.cause,
            self.paths.action.display()
        )
    }
}
impl std::error::Error for CopyRecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
#[derive(Debug)]
pub struct CopyRecoveryReceipt {
    pub paths: RecoveryCopyPaths,
    pub manifest: SealedManifest,
    pub inspection: JournalInspection,
    pub usage: InspectionUsage,
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
}
#[cfg(not(unix))]
#[derive(Debug, PartialEq, Eq)]
struct Stamp;
fn stamp(metadata: &fs::Metadata) -> Result<Stamp> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            metadata.is_file() && metadata.nlink() == 1,
            "Copy recovery requires a single-link ordinary file",
        )?;
        Ok(Stamp {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            mtime: metadata.mtime(),
            mtime_ns: metadata.mtime_nsec(),
            ctime: metadata.ctime(),
            ctime_ns: metadata.ctime_nsec(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(RecordingError::Invalid(
            "Copy recovery file identity is not qualified on this platform".into(),
        ))
    }
}
pub(super) struct SourceAnchors {
    data: File,
    data_stamp: Stamp,
    owner_stamp: Stamp,
}
fn owner_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".owner.lock");
    PathBuf::from(name)
}
impl SourceAnchors {
    pub(super) fn capture(path: &Path, owner: &File) -> Result<Option<Self>> {
        #[cfg(not(unix))]
        {
            let _ = (path, owner);
            Ok(None)
        }
        #[cfg(unix)]
        {
            ordinary(path)?;
            let data = File::open(path)?;
            let anchors = Self {
                data_stamp: stamp(&data.metadata()?)?,
                owner_stamp: stamp(&owner.metadata()?)?,
                data,
            };
            anchors.check(path, owner)?;
            Ok(Some(anchors))
        }
    }
    pub(super) fn check(&self, path: &Path, owner: &File) -> Result<()> {
        directory(
            path.parent()
                .ok_or_else(|| RecordingError::Invalid("Missing source parent".into()))?,
        )?;
        ordinary(path)?;
        ordinary(&owner_path(path))?;
        require(
            stamp(&self.data.metadata()?)? == self.data_stamp
                && stamp(&fs::symlink_metadata(path)?)? == self.data_stamp
                && stamp(&owner.metadata()?)? == self.owner_stamp
                && stamp(&fs::symlink_metadata(owner_path(path))?)? == self.owner_stamp,
            "Capture file or owner identity changed after bounded acquisition",
        )
    }
    fn same_objects(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            self.data_stamp.dev == other.data_stamp.dev
                && self.data_stamp.ino == other.data_stamp.ino
                && self.owner_stamp == other.owner_stamp
        }
        #[cfg(not(unix))]
        {
            let _ = other;
            false
        }
    }
    fn digest(
        &self,
        path: &Path,
        owner: &File,
        budget: &InspectionBudget,
    ) -> Result<(u64, String)> {
        self.check(path, owner)?;
        let result = hash_file(&self.data, budget);
        self.check(path, owner)?;
        result
    }
}
fn hash_file(file: &File, budget: &InspectionBudget) -> Result<(u64, String)> {
    let length = file.metadata()?.len();
    budget.file(length)?;
    let mut input = file;
    input.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut bytes = [0u8; 65_536];
    loop {
        budget.checkpoint()?;
        // Charge before the read, including the final bounded EOF probe.
        let capacity = length
            .saturating_sub(total)
            .saturating_add(1)
            .min(bytes.len() as u64) as usize;
        budget.charge_work(capacity as u64)?;
        let count = input.read(&mut bytes[..capacity])?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| RecordingError::Invalid("Copy length overflow".into()))?;
        require(total <= length, "File grew while hashing")?;
        budget.charge_work(count as u64)?;
        hasher.update(&bytes[..count]);
    }
    require(
        total == length && file.metadata()?.len() == length,
        "File length changed while hashing",
    )?;
    Ok((total, format!("{:x}", hasher.finalize())))
}
fn copy_file(
    input: &File,
    output: &mut File,
    expected: &JournalInspection,
    budget: &InspectionBudget,
) -> Result<()> {
    let mut input = input;
    input.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut bytes = [0u8; 65_536];
    loop {
        budget.checkpoint()?;
        let remaining = expected.byte_length.saturating_sub(total);
        let capacity = remaining.saturating_add(1).min(bytes.len() as u64) as usize;
        budget.charge_work(capacity as u64)?;
        let count = input.read(&mut bytes[..capacity])?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| RecordingError::Invalid("Copy length overflow".into()))?;
        require(total <= expected.byte_length, "Source grew during copy")?;
        budget.charge_work(count as u64)?;
        output.write_all(&bytes[..count])?;
        budget.charge_work(count as u64)?;
        hasher.update(&bytes[..count]);
    }
    require(
        total == expected.byte_length
            && format!("{:x}", hasher.finalize()) == expected.journal_sha256,
        "Whole-file copy differs from reviewed source",
    )?;
    output.sync_all()?;
    budget.checkpoint()
}
fn directory(path: &Path) -> Result<()> {
    require(path.is_absolute(), "Copy artifact parent must be absolute")?;
    for component in path.ancestors() {
        let metadata = fs::symlink_metadata(component)?;
        require(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Copy artifact parent must have ordinary non-symlink directories",
        )?;
    }
    require(
        fs::canonicalize(path)? == path,
        "Copy artifact parent must be canonical",
    )
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
fn exclusive_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?)
}

/// A one-use mutation capability for a separate complete working copy. No
/// native seal authority, durable Prepared receipt or semantic checkpoint is
/// implied. Drop/errors retain all artifacts. Existing actions never overwrite.
pub struct RecoveryCopy {
    source: CaptureReader,
    working: CaptureReader,
    evidence: File,
    evidence_stamp: Stamp,
    paths: RecoveryCopyPaths,
}
impl RecoveryCopy {
    pub fn create(
        source: CaptureReader,
        action_parent: impl AsRef<Path>,
        action_id: &str,
    ) -> std::result::Result<Self, CopyRecoveryError> {
        let action = action_parent.as_ref().join(action_id);
        let paths = RecoveryCopyPaths {
            evidence: action.join("evidence/take.sqlite3"),
            working: action.join("recovery/take.sqlite3"),
            action,
        };
        let governor = source.budget.as_ref().map(InspectionBudget::share);
        let mut boundary = CopyRecoveryBoundary::NoArtifacts;
        let result = (|| -> Result<Self> {
            require(
                action_id.len() == 32
                    && action_id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && action_id.bytes().any(|b| b != b'0'),
                "Copy action ID must be nonzero lowercase hexadecimal",
            )?;
            let budget = source.budget.as_ref().ok_or_else(|| {
                RecordingError::Invalid("Copy recovery requires the original bounded reader".into())
            })?;
            budget.checkpoint()?;
            require(
                source.inspection.manifest.is_none()
                    && source.inspection.report.state != TakeState::Sealed,
                "Copy recovery requires an unsealed source",
            )?;
            require(
                source
                    .inspection
                    .byte_length
                    .checked_add(COPY_RECOVERY_MAX_WORKING_BYTES)
                    .is_some_and(|n| n <= COPY_RECOVERY_MAX_ARTIFACT_BYTES),
                "Copy artifact reservation exceeds its fixed capacity",
            )?;
            directory(action_parent.as_ref())?;
            require(
                !action_parent.as_ref().starts_with(
                    source.inspection.path.parent().ok_or_else(|| {
                        RecordingError::Invalid("Missing source directory".into())
                    })?,
                ),
                "Copy artifact parent must be outside the original source directory",
            )?;
            verify_source(&source)?;
            if let Err(error) = fs::create_dir(&paths.action) {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    boundary = CopyRecoveryBoundary::ExistingArtifacts;
                }
                return Err(error.into());
            }
            boundary = CopyRecoveryBoundary::ActionCreated;
            sync_directory(action_parent.as_ref())?;
            let evidence_parent = paths.evidence.parent().expect("derived parent");
            let working_parent = paths.working.parent().expect("derived parent");
            fs::create_dir(evidence_parent)?;
            fs::create_dir(working_parent)?;
            sync_directory(&paths.action)?;
            let mut evidence = exclusive_file(&paths.evidence)?;
            boundary = CopyRecoveryBoundary::EvidenceCreated;
            copy_file(
                &source.anchors.as_ref().expect("bounded anchor").data,
                &mut evidence,
                &source.inspection,
                budget,
            )?;
            sync_directory(evidence_parent)?;
            require(
                hash_file(&evidence, budget)?
                    == (
                        source.inspection.byte_length,
                        source.inspection.journal_sha256.clone(),
                    ),
                "Evidence image verification differs",
            )?;
            verify_source(&source)?;
            boundary = CopyRecoveryBoundary::EvidenceVerified;
            // Discard writable handles before retaining the immutable image.
            let evidence_stamp = stamp(&evidence.metadata()?)?;
            drop(evidence);
            let evidence = File::open(&paths.evidence)?;
            require(
                stamp(&evidence.metadata()?)? == evidence_stamp,
                "Evidence image replaced during acquisition",
            )?;
            let mut working_file = exclusive_file(&paths.working)?;
            boundary = CopyRecoveryBoundary::WorkingCreated;
            copy_file(&evidence, &mut working_file, &source.inspection, budget)?;
            let working_stamp = stamp(&working_file.metadata()?)?;
            sync_directory(working_parent)?;
            let owner = exclusive_file(&owner_path(&paths.working))?;
            owner.try_lock().map_err(|error| match error {
                std::fs::TryLockError::WouldBlock => RecordingError::Owned,
                std::fs::TryLockError::Error(e) => RecordingError::Io(e),
            })?;
            owner.sync_all()?;
            sync_directory(working_parent)?;
            let working = CaptureReader::open_bounded_owned(&paths.working, budget, owner)?;
            require(
                stamp(
                    &working
                        .anchors
                        .as_ref()
                        .expect("bounded anchor")
                        .data
                        .metadata()?,
                )? == working_stamp,
                "Working file replaced during acquisition",
            )?;
            drop(working_file);
            let mut expected = source.inspection.clone();
            expected.path = working.inspection.path.clone();
            require(
                working.inspection == expected,
                "Initial working copy differs from source inspection",
            )?;
            verify_source(&source)?;
            boundary = CopyRecoveryBoundary::WorkingVerified;
            budget.checkpoint()?;
            Ok(Self {
                source,
                working,
                evidence,
                evidence_stamp,
                paths: paths.clone(),
            })
        })();
        crate::budget::finish(governor.as_ref(), result).map_err(|cause| CopyRecoveryError {
            cause,
            paths: Box::new(paths),
            boundary,
        })
    }
    pub fn reader(&self) -> &CaptureReader {
        &self.working
    }
    pub fn paths(&self) -> &RecoveryCopyPaths {
        &self.paths
    }
    pub fn finalize(
        self,
        selection: &RecoveryPrefix,
        operation_id: &str,
    ) -> std::result::Result<CopyRecoveryReceipt, CopyRecoveryError> {
        let governor = self
            .source
            .budget
            .as_ref()
            .expect("bounded capability")
            .share();
        let paths = self.paths.clone();
        let mut boundary = CopyRecoveryBoundary::WorkingVerified;
        let result = (|| -> Result<CopyRecoveryReceipt> {
            let budget = self.source.budget.as_ref().expect("bounded capability");
            verify_source(&self.source)?;
            self.verify_evidence(budget)?;
            self.working
                .anchors
                .as_ref()
                .expect("bounded anchor")
                .check(&paths.working, &self.working.owner)?;
            preflight_path(&paths.working, budget)?;
            // Unsupported schema is refused before writable SQLite acquisition.
            journal::copy_finalization::validate_copy_schema(&self.working.connection, budget)?;
            selection.validate()?;
            identifier(operation_id)?;
            let expected = self.working.inspection.clone();
            let CaptureReader {
                connection,
                owner,
                anchors,
                ..
            } = self.working;
            drop(connection);
            anchors
                .as_ref()
                .expect("bounded anchor")
                .check(&paths.working, &owner)?;
            boundary = CopyRecoveryBoundary::RecoveryMayHaveWritten;
            let manifest = journal::copy_finalization::recover_copy_bounded(
                &paths.working,
                &expected,
                selection,
                operation_id,
                budget,
            )?;
            // Hold the same owner while reopening for a complete bounded proof.
            let working = CaptureReader::open_bounded_owned(&paths.working, budget, owner)?;
            require(
                anchors
                    .as_ref()
                    .expect("bounded anchor")
                    .same_objects(working.anchors.as_ref().expect("bounded anchor")),
                "Recovery working file or owner identity changed across finalization",
            )?;
            require(
                working.inspection.original_rows_sha256 == expected.original_rows_sha256
                    && working.inspection.descriptor_sha256 == expected.descriptor_sha256
                    && working.inspection.manifest.as_ref() == Some(&manifest)
                    && manifest.body.interrupted.is_some()
                    && working.inspection.report.unknown_tail,
                "Recovered copy postimage authentication failed",
            )?;
            verify_source(&self.source)?;
            verify_image(
                &self.evidence,
                &self.evidence_stamp,
                &paths.evidence,
                &self.source.inspection,
                budget,
            )?;
            let working_anchors = working.anchors.as_ref().expect("bounded anchor");
            working_anchors.data.sync_all()?;
            sync_directory(paths.working.parent().expect("derived parent"))?;
            working_anchors.check(&paths.working, &working.owner)?;
            self.source
                .anchors
                .as_ref()
                .expect("bounded anchor")
                .check(&self.source.inspection.path, &self.source.owner)?;
            require(
                preflight_path(&paths.working, budget)? == paths.working
                    && preflight_path(&self.source.inspection.path, budget)?
                        == self.source.inspection.path
                    && preflight_path(&paths.evidence, budget)? == paths.evidence,
                "Copy recovery artifact path was redirected before receipt",
            )?;
            budget.checkpoint()?;
            boundary = CopyRecoveryBoundary::RecoveredVerified;
            Ok(CopyRecoveryReceipt {
                paths: paths.clone(),
                manifest,
                inspection: working.inspection.clone(),
                usage: budget.usage(),
            })
        })();
        governor.finish(result).map_err(|cause| CopyRecoveryError {
            cause,
            paths: Box::new(paths),
            boundary,
        })
    }
    fn verify_evidence(&self, budget: &InspectionBudget) -> Result<()> {
        verify_image(
            &self.evidence,
            &self.evidence_stamp,
            &self.paths.evidence,
            &self.source.inspection,
            budget,
        )
    }
}
fn verify_source(source: &CaptureReader) -> Result<()> {
    let budget = source
        .budget
        .as_ref()
        .ok_or_else(|| RecordingError::Invalid("Missing bounded source governor".into()))?;
    require(
        preflight_path(&source.inspection.path, budget)? == source.inspection.path,
        "Source parent was redirected after acquisition",
    )?;
    let anchors = source
        .anchors
        .as_ref()
        .ok_or_else(|| RecordingError::Invalid("Missing source identity anchor".into()))?;
    require(
        anchors.digest(&source.inspection.path, &source.owner, budget)?
            == (
                source.inspection.byte_length,
                source.inspection.journal_sha256.clone(),
            ),
        "Source physical bytes changed after review",
    )?;
    require(
        preflight_path(&source.inspection.path, budget)? == source.inspection.path,
        "Source parent was redirected during authentication",
    )
}
fn verify_image(
    file: &File,
    expected_stamp: &Stamp,
    path: &Path,
    expected: &JournalInspection,
    budget: &InspectionBudget,
) -> Result<()> {
    directory(
        path.parent()
            .ok_or_else(|| RecordingError::Invalid("Missing evidence parent".into()))?,
    )?;
    require(
        preflight_path(path, budget)? == path,
        "Evidence image path was redirected",
    )?;
    require(
        stamp(&file.metadata()?)? == *expected_stamp
            && stamp(&fs::symlink_metadata(path)?)? == *expected_stamp,
        "Evidence image identity changed",
    )?;
    require(
        hash_file(file, budget)? == (expected.byte_length, expected.journal_sha256.clone()),
        "Evidence image bytes changed",
    )?;
    require(
        stamp(&file.metadata()?)? == *expected_stamp
            && stamp(&fs::symlink_metadata(path)?)? == *expected_stamp,
        "Evidence image changed during authentication",
    )?;
    require(
        preflight_path(path, budget)? == path,
        "Evidence image path was redirected during authentication",
    )
}
