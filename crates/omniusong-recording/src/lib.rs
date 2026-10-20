//! Writer-owned immutable native take storage. Never call from a realtime callback.
mod inspection;
mod journal;
mod snapshot;
mod types;

pub use inspection::{CaptureReader, JournalInspection};
pub use journal::Journal;
pub use snapshot::{BackupReceipt, SealedJournal};
pub use types::*;

#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;
