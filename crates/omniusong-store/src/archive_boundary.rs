//! Outcomes of ordinary project archives. Completion and private cleanup are
//! separate: removing a temporary directory cannot undo a durable publication.
use crate::{Result, StoreError};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveAction {
    Export,
    Restore,
    Copy,
    Inspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivePhase {
    NotStarted,
    ObjectsPlacementAttempted,
    ObjectsPlaced,
    OutputLinkAttempted,
    OutputLinked,
    ExportDurable,
    CatalogCommitAttempted,
    CatalogCommitted,
    RecordedReplay,
    ReadOnlyCompleted,
}

/// Facts observed in this attempt, not an instruction to retry or repair it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveContext {
    pub action: ArchiveAction,
    pub staging: Option<PathBuf>,
    pub destination: PathBuf,
    pub source: Option<PathBuf>,
    pub operation_id: Option<String>,
    /// Requested restore-copy pin, distinct from an observed input/output hash.
    pub expected_archive_sha256: Option<String>,
    pub archive_sha256: Option<String>,
    /// Confirmed placements only. Zero does not prove no filesystem effects.
    pub published_objects: usize,
    pub phase: ArchivePhase,
}
impl ArchiveContext {
    pub fn new(action: ArchiveAction, destination: &Path) -> Self {
        Self {
            action,
            staging: None,
            destination: destination.to_owned(),
            source: None,
            operation_id: None,
            expected_archive_sha256: None,
            archive_sha256: None,
            published_objects: 0,
            phase: ArchivePhase::NotStarted,
        }
    }
    pub fn publication_uncertain(&self) -> bool {
        matches!(
            self.phase,
            ArchivePhase::OutputLinkAttempted
                | ArchivePhase::OutputLinked
                | ArchivePhase::ExportDurable
                | ArchivePhase::CatalogCommitAttempted
                | ArchivePhase::CatalogCommitted
        )
    }
}

#[derive(Debug)]
pub struct ArchiveFailure {
    context: Box<ArchiveContext>,
    operation: Box<StoreError>,
}
impl ArchiveFailure {
    pub fn new(context: ArchiveContext, error: StoreError) -> Self {
        Self {
            context: Box::new(context),
            operation: Box::new(error),
        }
    }
    pub fn context(&self) -> &ArchiveContext {
        &self.context
    }
    pub fn operation_error(&self) -> &StoreError {
        &self.operation
    }
    pub fn publication_uncertain(&self) -> bool {
        self.context.publication_uncertain() || self.operation.archive_publication_uncertain()
    }
    pub fn catalog_sqlite_error_code(&self) -> Option<i32> {
        self.operation.catalog_sqlite_error_code()
    }
    pub fn catalog_connection_unusable(&self) -> bool {
        self.operation.catalog_connection_unusable()
    }
}
impl fmt::Display for ArchiveFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Archive {:?} failed at {:?}: {}; destination={:?}; staging={:?}; source={:?}; operation={:?}; expected_archive_sha256={:?}; archive_sha256={:?}; confirmed_objects={}",
            self.context.action,
            self.context.phase,
            self.operation,
            self.context.destination,
            self.context.staging,
            self.context.source,
            self.context.operation_id,
            self.context.expected_archive_sha256,
            self.context.archive_sha256,
            self.context.published_objects
        )
    }
}
impl std::error::Error for ArchiveFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.operation.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveSqlOrigin {
    Input,
    PrivateOutput,
    LiveCatalog,
}

/// SQLite provenance is explicit at the handle boundary, never inferred from
/// its error message, phase or the fact that an archive operation is running.
#[derive(Debug)]
pub struct ArchiveSqlFailure {
    origin: ArchiveSqlOrigin,
    operation: Box<StoreError>,
}
impl ArchiveSqlFailure {
    pub fn new(origin: ArchiveSqlOrigin, error: rusqlite::Error) -> Self {
        Self {
            origin,
            operation: Box::new(StoreError::Database(error)),
        }
    }
    pub fn origin(&self) -> ArchiveSqlOrigin {
        self.origin
    }
    pub fn operation_error(&self) -> &StoreError {
        &self.operation
    }
    pub fn catalog_sqlite_error_code(&self) -> Option<i32> {
        (self.origin == ArchiveSqlOrigin::LiveCatalog)
            .then(|| self.operation.catalog_sqlite_error_code())
            .flatten()
    }
    pub fn catalog_connection_unusable(&self) -> bool {
        self.origin == ArchiveSqlOrigin::LiveCatalog && self.operation.catalog_connection_unusable()
    }
}
impl fmt::Display for ArchiveSqlFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Archive {:?} SQLite: {}", self.origin, self.operation)
    }
}
impl std::error::Error for ArchiveSqlFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.operation.as_ref())
    }
}
pub(crate) fn sql_error(origin: ArchiveSqlOrigin, error: rusqlite::Error) -> StoreError {
    StoreError::ArchiveSql(Box::new(ArchiveSqlFailure::new(origin, error)))
}
fn tag<T>(result: Result<T>, origin: ArchiveSqlOrigin) -> Result<T> {
    result.map_err(|error| match error {
        StoreError::Database(error) => sql_error(origin, error),
        // Nested Catalog failures carry live rollback information; never hide
        // it under a private-input tag. Mixed helpers tag each destination.
        other => other,
    })
}
pub(crate) fn input<T>(result: Result<T>) -> Result<T> {
    tag(result, ArchiveSqlOrigin::Input)
}
pub(crate) fn output<T>(result: Result<T>) -> Result<T> {
    tag(result, ArchiveSqlOrigin::PrivateOutput)
}
pub(crate) fn live<T>(result: Result<T>) -> Result<T> {
    tag(result, ArchiveSqlOrigin::LiveCatalog)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveCompletion {
    ExportDurable,
    CatalogCommitted,
    RecordedReplay,
    ReadOnlyCompleted,
}
#[derive(Debug)]
pub struct ArchiveCleanupIssue {
    pub path: PathBuf,
    pub error: io::Error,
}
#[derive(Debug)]
pub struct ArchiveOutcome<T> {
    pub value: T,
    pub completion: ArchiveCompletion,
    /// Private temporary files only; never live catalog or rollback failures.
    pub cleanup: Vec<ArchiveCleanupIssue>,
}
impl<T> ArchiveOutcome<T> {
    pub fn new(value: T, completion: ArchiveCompletion) -> Self {
        Self {
            value,
            completion,
            cleanup: Vec::new(),
        }
    }
    fn cleanup(&mut self, path: &Path, remove: impl FnOnce() -> io::Result<()>) {
        #[cfg(test)]
        let result = test_checkpoint(
            TestEvent::PrivateCleanup,
            &ArchiveContext::new(ArchiveAction::Inspect, path),
        )
        .and_then(|()| remove());
        #[cfg(not(test))]
        let result = remove();
        if let Err(error) = result {
            self.cleanup.push(ArchiveCleanupIssue {
                path: path.to_owned(),
                error,
            });
        }
    }
    pub(crate) fn cleanup_file(&mut self, path: &Path) {
        self.cleanup(path, || fs::remove_file(path));
    }
    pub(crate) fn cleanup_empty_dir(&mut self, path: &Path) {
        self.cleanup(path, || fs::remove_dir(path));
    }
    pub(crate) fn cleanup_tree(&mut self, path: &Path) {
        self.cleanup(path, || fs::remove_dir_all(path));
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestEvent {
    ExportSchema,
    RestoreAcquired,
    CopyPlanFrozen,
    OutputLink,
    ExportSync,
    RestoreCommit,
    CopyCommit,
    PrivateCleanup,
}
#[cfg(test)]
type TestHook = Box<dyn FnMut(TestEvent, &ArchiveContext) -> io::Result<()>>;
#[cfg(test)]
thread_local! { static TEST_HOOK: std::cell::RefCell<Option<TestHook>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
fn test_checkpoint(event: TestEvent, context: &ArchiveContext) -> io::Result<()> {
    TEST_HOOK.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map_or(Ok(()), |hook| hook(event, context))
    })
}
#[cfg(test)]
pub(crate) fn checkpoint(event: TestEvent, context: &ArchiveContext) -> Result<()> {
    test_checkpoint(event, context).map_err(StoreError::Io)
}
#[cfg(test)]
pub(crate) fn with_test_hook<T>(hook: TestHook, operation: impl FnOnce() -> T) -> T {
    struct Reset(Option<TestHook>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_HOOK.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let reset = Reset(TEST_HOOK.with(|slot| slot.replace(Some(hook))));
    let result = operation();
    drop(reset);
    result
}

#[cfg(test)]
mod tests;
