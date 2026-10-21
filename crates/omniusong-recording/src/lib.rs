//! Writer-owned immutable native take storage. Never call from a realtime callback.
mod budget;
mod inspection;
mod journal;
mod snapshot;
mod types;

pub use budget::{InspectionBudget, InspectionLimits, InspectionResource, InspectionUsage};
pub use inspection::{CaptureReader, JournalInspection, OwnedJournalView};
pub use journal::Journal;
pub use snapshot::{BackupReceipt, SealedJournal};
pub use types::*;

#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;
