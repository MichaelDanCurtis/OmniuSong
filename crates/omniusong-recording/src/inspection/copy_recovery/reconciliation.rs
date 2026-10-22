//! Existing-copy authentication under a durable action's separate owner.
//!
//! This module does not authenticate a Prepared ledger row. Its caller must do
//! that before creating an execution owner, and retain the owner/view through
//! any catalog acknowledgement. Reconciliation never creates or repairs files,
//! resumes an incomplete copy, or grants normal-seal authority.
use super::*;

fn action_paths(parent: &Path, id: &str) -> Result<(RecoveryCopyPaths, PathBuf)> {
    require(
        cfg!(unix),
        "Copy recovery file identity is not qualified on this platform",
    )?;
    require(
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && id.bytes().any(|b| b != b'0'),
        "Copy action ID must be nonzero lowercase hexadecimal",
    )?;
    directory(parent)?;
    let action = parent.join(id);
    Ok((
        RecoveryCopyPaths {
            evidence: action.join("evidence/take.sqlite3"),
            working: action.join("recovery/take.sqlite3"),
            action,
        },
        parent.join(format!("{id}.execution.lock")),
    ))
}

/// Cooperative cross-service exclusion outside the action directory. Creation
/// is permitted only after a newly confirmed durable Started row. An existing
/// owner, even with no action directory, never permits a new copy attempt.
#[derive(Debug)]
pub struct CopyActionOwner {
    file: File,
    stamp: Stamp,
    path: PathBuf,
    paths: RecoveryCopyPaths,
}
impl CopyActionOwner {
    pub fn create(parent: &Path, id: &str) -> Result<Self> {
        let (paths, path) = action_paths(parent, id)?;
        let file = exclusive_file(&path)?;
        let owner = Self::acquire(file, path, paths)?;
        owner.file.sync_all()?;
        sync_directory(parent)?;
        owner.verify()?;
        Ok(owner)
    }

    /// Opens only an already present lease. No CREATE, truncation, cleanup or
    /// continuation of a previously interrupted filesystem operation occurs.
    pub fn open_existing(parent: &Path, id: &str) -> Result<Self> {
        let (paths, path) = action_paths(parent, id)?;
        ordinary(&path)?;
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        Self::acquire(file, path, paths)
    }

    fn acquire(file: File, path: PathBuf, paths: RecoveryCopyPaths) -> Result<Self> {
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => RecordingError::Owned,
            std::fs::TryLockError::Error(error) => RecordingError::Io(error),
        })?;
        let owner = Self {
            stamp: stamp(&file.metadata()?)?,
            file,
            path,
            paths,
        };
        owner.verify()?;
        Ok(owner)
    }

    pub fn paths(&self) -> &RecoveryCopyPaths {
        &self.paths
    }

    pub fn verify(&self) -> Result<()> {
        directory(
            self.path
                .parent()
                .ok_or_else(|| RecordingError::Invalid("Missing action owner parent".into()))?,
        )?;
        ordinary(&self.path)?;
        require(
            self.file.metadata()?.len() == 0
                && stamp(&self.file.metadata()?)? == self.stamp
                && stamp(&fs::symlink_metadata(&self.path)?)? == self.stamp,
            "Copy action owner is nonempty or changed identity",
        )
    }
}

fn derived_counts(connection: &Connection, budget: &InspectionBudget) -> Result<(u64, u64, u64)> {
    // All physical rows are bounded by preflight_connection before this query.
    budget.visit_rows(3)?;
    budget.charge_work(24)?;
    Ok(connection.query_row(
        "SELECT (SELECT count(*) FROM interruptions),
                (SELECT count(*) FROM recovery_index),
                (SELECT count(*) FROM sealed_manifests)",
        [],
        |row| {
            Ok((
                journal::read_unsigned(row, 0)?,
                journal::read_unsigned(row, 1)?,
                journal::read_unsigned(row, 2)?,
            ))
        },
    )?)
}

/// A read-only proof of an existing whole evidence image and recovered working
/// copy. The borrowed execution owner and owned working CaptureReader remain
/// held together. No source path is required or consulted by this view.
pub struct RecoveredCopyView<'a> {
    owner: &'a CopyActionOwner,
    evidence: File,
    evidence_stamp: Stamp,
    evidence_connection: Connection,
    evidence_inspection: JournalInspection,
    working: CaptureReader,
    budget: InspectionBudget,
}
impl<'a> RecoveredCopyView<'a> {
    pub fn open(
        owner: &'a CopyActionOwner,
        expected_image_sha: &str,
        expected_image_bytes: u64,
        budget: &InspectionBudget,
    ) -> Result<Self> {
        let result = (|| {
            budget.checkpoint()?;
            require(
                expected_image_sha.len() == 64
                    && expected_image_sha
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && expected_image_bytes > 0
                    && expected_image_bytes <= COPY_RECOVERY_MAX_WORKING_BYTES,
                "Evidence image requires a bounded exact byte identity",
            )?;
            budget.file(expected_image_bytes)?;
            owner.verify()?;
            let paths = owner.paths();
            directory(&paths.action)?;
            directory(paths.evidence.parent().expect("derived evidence parent"))?;
            directory(paths.working.parent().expect("derived working parent"))?;
            require(
                preflight_path(&paths.evidence, budget)? == paths.evidence,
                "Evidence image path was redirected",
            )?;
            let evidence = File::open(&paths.evidence)?;
            let evidence_stamp = stamp(&evidence.metadata()?)?;
            require(
                stamp(&fs::symlink_metadata(&paths.evidence)?)? == evidence_stamp
                    && hash_file(&evidence, budget)?
                        == (expected_image_bytes, expected_image_sha.to_owned()),
                "Evidence image differs from the persisted exact byte identity",
            )?;
            // There is deliberately no image.owner.lock acquisition or CREATE.
            // The action lease protects this immutable evidence namespace.
            let evidence_connection = Connection::open_with_flags(
                &paths.evidence,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            budget.install(&evidence_connection)?;
            evidence_connection.busy_timeout(Duration::from_secs(2))?;
            evidence_connection.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; BEGIN DEFERRED;")?;
            journal::copy_finalization::validate_copy_schema(&evidence_connection, budget)?;
            preflight_connection(&evidence_connection, budget)?;
            let descriptor = journal::load_descriptor_bounded(&evidence_connection, Some(budget))?;
            let evidence_inspection = inspect_connection_bounded(
                &evidence_connection,
                &paths.evidence,
                &descriptor,
                Some(budget),
            )?;
            require(
                evidence_inspection.byte_length == expected_image_bytes
                    && evidence_inspection.journal_sha256 == expected_image_sha
                    && evidence_inspection.manifest.is_none()
                    && matches!(
                        evidence_inspection.report.state,
                        TakeState::Recording | TakeState::Interrupted
                    )
                    && evidence_inspection.report.unknown_tail,
                "Evidence image is not the exact retained unsealed original",
            )?;
            verify_image(
                &evidence,
                &evidence_stamp,
                &paths.evidence,
                &evidence_inspection,
                budget,
            )?;
            // CaptureReader requires the existing working owner; missing owner
            // files, partial copies and hot SQLite sidecars are refused.
            let working = CaptureReader::open_bounded(&paths.working, budget)?;
            journal::copy_finalization::validate_copy_schema(&working.connection, budget)?;
            let manifest = working.inspection.manifest.as_ref().ok_or_else(|| {
                RecordingError::Invalid("Working copy has no complete sealed manifest".into())
            })?;
            let interruption = manifest.body.interrupted.as_ref().ok_or_else(|| {
                RecordingError::Invalid("Working copy is not a recovered manifest".into())
            })?;
            require(
                working.inspection.path == paths.working
                    && working.inspection.report.state == TakeState::Sealed
                    && working.inspection.report.unknown_tail
                    && working.inspection.descriptor == evidence_inspection.descriptor
                    && working.inspection.descriptor_sha256
                        == evidence_inspection.descriptor_sha256
                    && working.inspection.original_rows_sha256
                        == evidence_inspection.original_rows_sha256
                    && manifest.body.chunk_count > 0
                    && manifest.body.chunk_count <= evidence_inspection.report.verified_chunks
                    && manifest
                        .body
                        .streams
                        .iter()
                        .zip(&manifest.body.descriptor.streams)
                        .any(|(prefix, stream)| {
                            matches!(stream.kind, StreamKind::Audio { .. })
                                && prefix.recorded_units > 0
                        }),
                "Recovered working copy differs from the full retained original",
            )?;
            let mut expected_report = evidence_inspection.report.clone();
            expected_report.state = TakeState::Interrupted;
            expected_report.unknown_tail = true;
            require(
                interruption.report == expected_report,
                "Recovered interruption does not retain the full original inspection",
            )?;
            let image_counts = derived_counts(&evidence_connection, budget)?;
            match evidence_inspection.report.state {
                TakeState::Recording => require(
                    image_counts == (0, 0, 0),
                    "Recording evidence contains unexpected derived metadata",
                )?,
                TakeState::Interrupted => {
                    let original_interruption =
                        journal::load_interrupted_bounded(&evidence_connection, Some(budget))?;
                    require(
                        interruption == &original_interruption
                            && image_counts == (1, original_interruption.report.verified_chunks, 0),
                        "Recovered working copy changed the original interruption receipt",
                    )?;
                }
                TakeState::Sealed => unreachable!("unsealed evidence checked above"),
            }
            require(
                derived_counts(&working.connection, budget)?
                    == (1, interruption.report.verified_chunks, 1),
                "Recovered working copy contains surplus or missing derived metadata",
            )?;
            let view = Self {
                owner,
                evidence,
                evidence_stamp,
                evidence_connection,
                evidence_inspection,
                working,
                budget: budget.share(),
            };
            view.verify_unchanged()?;
            Ok(view)
        })();
        budget.finish(result)
    }

    pub fn evidence_inspection(&self) -> &JournalInspection {
        &self.evidence_inspection
    }

    pub fn read_evidence_ordinal(&self, ordinal: u64) -> Result<ChunkRecord> {
        let result = read_ordinal_bounded(
            &self.evidence_connection,
            &self.evidence_inspection,
            ordinal,
            Some(&self.budget),
        );
        self.budget.finish(result)
    }

    pub fn working_reader(&self) -> &CaptureReader {
        &self.working
    }

    pub fn budget(&self) -> &InspectionBudget {
        &self.budget
    }

    /// Rechecks both frozen physical byte identities and every held owner/path.
    /// Call after semantic authentication and before acknowledging in the ledger.
    pub fn verify_unchanged(&self) -> Result<()> {
        let result = (|| {
            self.budget.checkpoint()?;
            self.owner.verify()?;
            verify_image(
                &self.evidence,
                &self.evidence_stamp,
                &self.owner.paths.evidence,
                &self.evidence_inspection,
                &self.budget,
            )?;
            verify_source(&self.working)?;
            // The working hash may be lengthy. Recheck both names and frozen
            // anchors together at the final boundary, without another full pass.
            require(
                preflight_path(&self.owner.paths.evidence, &self.budget)?
                    == self.owner.paths.evidence
                    && stamp(&self.evidence.metadata()?)? == self.evidence_stamp
                    && stamp(&fs::symlink_metadata(&self.owner.paths.evidence)?)?
                        == self.evidence_stamp,
                "Evidence image changed before recovered-copy acknowledgement",
            )?;
            self.working
                .anchors
                .as_ref()
                .ok_or_else(|| RecordingError::Invalid("Missing working identity anchor".into()))?
                .check(&self.owner.paths.working, &self.working.owner)?;
            self.owner.verify()?;
            self.budget.checkpoint()
        })();
        self.budget.finish(result)
    }

    /// This is generic recovered-copy evidence, not a semantic checkpoint,
    /// durable source-action outcome, attachment or native sealing capability.
    pub fn receipt(&self) -> Result<CopyRecoveryReceipt> {
        self.verify_unchanged()?;
        let result = Ok(CopyRecoveryReceipt {
            paths: self.owner.paths.clone(),
            manifest: self
                .working
                .inspection
                .manifest
                .as_ref()
                .expect("authenticated recovered manifest")
                .clone(),
            inspection: self.working.inspection.clone(),
            usage: self.budget.usage(),
        });
        self.budget.finish(result)
    }
}
