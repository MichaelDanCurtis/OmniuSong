//! Writer-owned immutable native take storage. Never call from a realtime callback.
mod budget;
mod inspection;
mod journal;
mod snapshot;
mod types;

pub use budget::{InspectionBudget, InspectionLimits, InspectionResource, InspectionUsage};
pub use inspection::{
    COPY_RECOVERY_MAX_ARTIFACT_BYTES, COPY_RECOVERY_MAX_WORKING_BYTES, CaptureReader,
    CopyActionOwner, CopyRecoveryBoundary, CopyRecoveryError, CopyRecoveryReceipt,
    JournalInspection, OwnedJournalView, RecoveredCopyView, RecoveryCopy, RecoveryCopyPaths,
};
pub use journal::Journal;
pub use snapshot::{BackupReceipt, SealedJournal};
pub use types::*;

#[cfg(test)]
mod inspection_report_reuse_tests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;
