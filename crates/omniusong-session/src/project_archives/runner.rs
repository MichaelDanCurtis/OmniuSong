//! Sole session-worker execution. Compact completion never depends on a later
//! document/library load; native admission remains a separate gate.
use super::*;
use crate::{FailureKind, NativeBackend, Reply};
use omniusong_recording::{InspectionBudget, InspectionLimits, RecordingError};
use omniusong_store::{ArchiveFailure, StoreError};
use std::sync::{Arc, atomic::AtomicBool};

fn admission_stop(error: RecordingError) -> ServiceError {
    let kind = match &error {
        RecordingError::InspectionCancelled => FailureKind::InspectionCancelled,
        RecordingError::InspectionLimit { .. } => FailureKind::InspectionLimit,
        _ => FailureKind::Backend,
    };
    ServiceError {
        kind,
        message: error.to_string(),
        outcome_unknown: false,
    }
}

impl NativeBackend {
    pub(crate) fn run_project_archive(
        &mut self,
        request: &ProjectArchiveRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        // Intrinsic bounded admission precedes every path/receipt conversion.
        request.validate()?;
        let budget = InspectionBudget::new(InspectionLimits::default(), cancelled)
            .map_err(admission_stop)?;
        // Reserve bounded request hashing and result formatting before any
        // artifact mutation. No checkpoint can relabel completed store work.
        budget
            .charge_work(
                8 * MAX_PROJECT_ARCHIVE_REQUEST_BYTES as u64
                    + 8 * MAX_PROJECT_ARCHIVE_REPLY_BYTES as u64,
            )
            .map_err(admission_stop)?;
        let request_sha = request.fingerprint()?;
        let context = request.preflight_context();
        let Some(store) = self.store.as_mut() else {
            return Ok(Reply::ProjectArchive(Box::new(from_failure(
                &request_sha,
                ArchiveFailure::new(
                    context,
                    StoreError::Invalid(
                        "Open session storage before running a project archive".into(),
                    ),
                ),
                false,
            ))));
        };
        let preflight = (|| -> Result<(), StoreError> {
            budget.checkpoint()?;
            if store.root().as_os_str() != request.origin_root.as_os_str() {
                return Err(StoreError::Invalid(
                    "Project archive origin does not match the open store".into(),
                ));
            }
            if let ProjectArchiveOperation::Export { destination, .. } = &request.operation {
                let parent = destination.parent().ok_or_else(|| {
                    StoreError::Invalid("Project archive destination requires its parent".into())
                })?;
                let metadata = parent.symlink_metadata()?;
                if !metadata.is_dir()
                    || metadata.file_type().is_symlink()
                    || parent.canonicalize()?.as_os_str() != parent.as_os_str()
                {
                    return Err(StoreError::Invalid(
                        "Confirm the canonical archive destination parent before execution".into(),
                    ));
                }
            }
            budget.checkpoint()?;
            Ok(())
        })();
        if let Err(error) = preflight {
            return Ok(Reply::ProjectArchive(Box::new(from_failure(
                &request_sha,
                ArchiveFailure::new(context, error),
                false,
            ))));
        }
        let outcome = match &request.operation {
            ProjectArchiveOperation::Export {
                project_id,
                destination,
            } => match store.export_archive_bounded_outcome(&project_id.0, destination, &budget) {
                Ok(value) => from_export(&request_sha, value),
                Err(failure) => from_failure(&request_sha, failure, true),
            },
            ProjectArchiveOperation::RestoreIdentity { archive_path } => {
                match store.restore_archive_bounded_outcome(archive_path, &budget) {
                    Ok(value) => from_identity(&request_sha, value),
                    Err(failure) => from_failure(&request_sha, failure, true),
                }
            }
            ProjectArchiveOperation::RestoreCopy {
                archive_path,
                intent,
            } => {
                match store.restore_archive_as_copy_bounded_outcome(archive_path, intent, &budget) {
                    Ok(value) => from_copy(&request_sha, value),
                    Err(failure) => from_failure(&request_sha, failure, true),
                }
            }
        };
        // In particular: no store(), load(), list(), serialization/hash of a
        // returned document, filesystem read or caller-token check follows.
        Ok(Reply::ProjectArchive(Box::new(outcome)))
    }
}
