//! Explicit six-stream recovery into a distinct evidence/working copy. This
//! library call grants no native Finish, normal seal, publication, or replay.
//! A caller must separately authenticate the durable Prepared request. This
//! primitive neither accepts nor proves one; no service/UI route is provided.
use crate::performance_writer::{PerformanceInspection, inspect_performance_bounded};
use omniusong_core::contracts::{OperationId, Validate};
use omniusong_recording::{
    CaptureReader, CopyRecoveryBoundary, CopyRecoveryError, CopyRecoveryReceipt, InspectionBudget,
    RecordingError, RecoveryCopy, RecoveryCopyPaths, TakeState,
};
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};

const OPERATION_DOMAIN: &[u8] = b"OmniuSong.performance-copy-recovery.v1\0";

fn invalid(message: &str) -> RecordingError {
    RecordingError::Invalid(message.into())
}

struct OperationHash<'a> {
    budget: &'a InspectionBudget,
    hash: Sha256,
}
impl Write for OperationHash<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // Serialization and hashing are separate work, streamed without an
        // owned copy of the complete semantic review or a fresh governor.
        let work = u64::try_from(bytes.len())
            .ok()
            .and_then(|n| n.checked_mul(2))
            .ok_or_else(|| std::io::Error::other("Recovery operation byte count overflow"))?;
        self.budget
            .charge_work(work)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        self.hash.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn operation(
    budget: &InspectionBudget,
    action_id: &str,
    operation_id: &str,
    expected: &PerformanceInspection,
) -> omniusong_recording::Result<String> {
    budget.checkpoint()?;
    budget.charge_work(OPERATION_DOMAIN.len() as u64)?;
    let mut output = OperationHash {
        budget,
        hash: Sha256::new(),
    };
    output.hash.update(OPERATION_DOMAIN);
    let encoded = serde_json::to_writer(&mut output, &(action_id, operation_id, expected));
    // A sticky cancellation/limit must survive serde_json's I/O wrapper.
    budget.checkpoint()?;
    encoded.map_err(|error| RecordingError::Invalid(error.to_string()))?;
    Ok(format!("performance-copy:{:x}", output.hash.finalize()))
}

/// Re-authenticates a retained six-stream review before creating artifacts and
/// again on the held read-only working copy before recovered-only finalization.
/// Errors retain every derived path and never remove partial or existing work.
pub fn recover_performance_copy(
    reader: CaptureReader,
    expected: &PerformanceInspection,
    action_parent: impl AsRef<Path>,
    action_id: &str,
    operation_id: &str,
) -> std::result::Result<CopyRecoveryReceipt, CopyRecoveryError> {
    let action_parent = action_parent.as_ref();
    let action = action_parent.join(action_id);
    let paths = RecoveryCopyPaths {
        evidence: action.join("evidence/take.sqlite3"),
        working: action.join("recovery/take.sqlite3"),
        action,
    };
    let before_copy = |cause| CopyRecoveryError {
        cause,
        paths: Box::new(paths.clone()),
        boundary: CopyRecoveryBoundary::NoArtifacts,
    };
    let preflight = (|| -> omniusong_recording::Result<String> {
        let budget = reader.inspection_budget().ok_or_else(|| {
            invalid("Performance copy recovery requires the original bounded reader")
        })?;
        budget.checkpoint()?;
        if action_id.len() != 32
            || !action_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !action_id.bytes().any(|b| b != b'0')
        {
            return Err(invalid(
                "Copy action ID must be nonzero lowercase hexadecimal",
            ));
        }
        // The core contract bounds the owned identifier to 96 ASCII bytes.
        if operation_id.len() > 96 {
            return Err(invalid("Invalid OperationId"));
        }
        OperationId(operation_id.into())
            .validate()
            .map_err(|error| RecordingError::Invalid(error.to_string()))?;
        if reader.inspection().manifest.is_some()
            || reader.inspection().report.state == TakeState::Sealed
        {
            return Err(invalid(
                "Performance copy recovery requires an unsealed source",
            ));
        }
        if expected
            .selection
            .as_ref()
            .is_none_or(|selection| selection.chunk_count == 0)
            || expected
                .checkpoint
                .as_ref()
                .is_none_or(|checkpoint| checkpoint.audio_end == 0)
        {
            return Err(invalid(
                "No positive complete performance checkpoint can be copied for recovery",
            ));
        }
        let current = inspect_performance_bounded(&reader)?;
        if current != *expected {
            return Err(invalid(
                "Performance originals or full semantic review changed; inspect again",
            ));
        }
        operation(budget, action_id, operation_id, expected)
    })();
    // A concurrent sticky stop takes precedence over any ordinary preflight
    // rejection, including paths that returned before operation serialization.
    if let Some(budget) = reader.inspection_budget() {
        budget.checkpoint().map_err(before_copy)?;
    }
    let derived = preflight.map_err(before_copy)?;
    let copy = RecoveryCopy::create(reader, action_parent, action_id)?;
    let after_copy = |cause| CopyRecoveryError {
        cause,
        paths: Box::new(copy.paths().clone()),
        boundary: CopyRecoveryBoundary::WorkingVerified,
    };
    let working_result = inspect_performance_bounded(copy.reader());
    if let Some(budget) = copy.reader().inspection_budget() {
        budget.checkpoint().map_err(after_copy)?;
    }
    let working_review = working_result.map_err(after_copy)?;
    if working_review != *expected {
        return Err(CopyRecoveryError {
            cause: invalid("Working copy differs from the exact six-stream performance review"),
            paths: Box::new(copy.paths().clone()),
            boundary: CopyRecoveryBoundary::WorkingVerified,
        });
    }
    // This consumes only recovered-prefix copy authority. A retained Finish cut
    // remains evidence and cannot reach the native/Complete finalizer here.
    copy.finalize(
        expected
            .selection
            .as_ref()
            .expect("checked positive selection"),
        &derived,
    )
}
