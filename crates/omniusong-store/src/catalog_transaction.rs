//! Scoped SQL VM governance for the store's private reusable connection.
//! Transaction-control fences run with hooks detached; source readers retain
//! their own governor. This is never used on archived/private-source handles.
use crate::{Result, StoreError};
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, DropBehavior, Transaction, TransactionBehavior};
use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn fatal(code: i32) -> bool {
    matches!(code & 0xff, 10 | 11 | 26)
}

/// A failure during cleanup of an actual live-catalog transaction. The
/// original operation is retained separately from every cleanup error.
#[derive(Debug)]
pub struct CatalogFailure {
    operation: Box<StoreError>,
    cleanup: Vec<rusqlite::Error>,
    transaction_active: bool,
}
impl CatalogFailure {
    pub fn operation_error(&self) -> &StoreError {
        &self.operation
    }
    pub fn cleanup_errors(&self) -> &[rusqlite::Error] {
        &self.cleanup
    }
    /// Discard this connection instead of retrying another operation on it.
    pub fn connection_unusable(&self) -> bool {
        self.transaction_active || !self.cleanup.is_empty()
    }
    pub fn catalog_sqlite_error_code(&self) -> Option<i32> {
        let primary = self.operation.catalog_sqlite_error_code();
        if primary.is_some_and(fatal) {
            return primary;
        }
        self.cleanup
            .iter()
            .filter_map(|e| e.sqlite_error().map(|e| e.extended_code))
            .find(|code| fatal(*code))
            .or(primary)
            .or_else(|| {
                self.cleanup
                    .iter()
                    .find_map(|e| e.sqlite_error().map(|e| e.extended_code))
            })
    }
}
impl std::fmt::Display for CatalogFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; catalog cleanup failed", self.operation)?;
        for error in &self.cleanup {
            write!(f, ": {error}")?;
        }
        if self.transaction_active {
            f.write_str("; transaction remains active")?;
        }
        Ok(())
    }
}
impl std::error::Error for CatalogFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.operation.as_ref())
    }
}

struct CatalogProgress<'c> {
    connection: &'c Connection,
    enabled: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    detached: bool,
}
impl<'c> CatalogProgress<'c> {
    fn install(connection: &'c Connection, budget: &InspectionBudget) -> Result<Self> {
        budget.checkpoint()?;
        let scope = Self {
            connection,
            enabled: Arc::new(AtomicBool::new(true)),
            interrupted: Arc::new(AtomicBool::new(false)),
            detached: false,
        };
        let rollback_enabled = Arc::clone(&scope.enabled);
        // Automatic rollback happens inside SQLite before the final progress
        // catch-up. This observer only touches an atomic, never the connection.
        connection.rollback_hook(Some(move || {
            rollback_enabled.store(false, Ordering::Release)
        }))?;
        let enabled = Arc::clone(&scope.enabled);
        let interrupted = Arc::clone(&scope.interrupted);
        let mut governor = budget.sqlite_vm_governor();
        connection.progress_handler(
            1,
            Some(move || {
                if !enabled.load(Ordering::Acquire) {
                    return false;
                }
                let stop = governor();
                if stop {
                    interrupted.store(true, Ordering::Release);
                }
                stop
            }),
        )?;
        Ok(scope)
    }
    fn detach(&mut self) -> Vec<rusqlite::Error> {
        if self.detached {
            return Vec::new();
        }
        self.detached = true;
        // Disable first. Even an unexpected failure to unregister cannot leave
        // the old cancelled callback interrupting rollback or future cleanup.
        self.enabled.store(false, Ordering::Release);
        let mut errors = Vec::new();
        if let Err(e) = self.connection.progress_handler(0, None::<fn() -> bool>) {
            errors.push(e);
        }
        if let Err(e) = self.connection.rollback_hook(None::<fn()>) {
            errors.push(e);
        }
        errors
    }
}
impl Drop for CatalogProgress<'_> {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}

pub(crate) struct CatalogTransaction<'c, 'b> {
    connection: &'c Connection,
    transaction: Option<Transaction<'c>>,
    progress: Option<CatalogProgress<'c>>,
    budget: Option<&'b InspectionBudget>,
}
impl<'c, 'b> CatalogTransaction<'c, 'b> {
    pub(crate) fn immediate(
        connection: &'c mut Connection,
        budget: &'b InspectionBudget,
    ) -> Result<Self> {
        Self::begin(connection, budget, TransactionBehavior::Immediate)
    }
    pub(crate) fn deferred(
        connection: &'c Connection,
        budget: &'b InspectionBudget,
    ) -> Result<Self> {
        Self::begin(connection, budget, TransactionBehavior::Deferred)
    }
    fn begin(
        connection: &'c Connection,
        budget: &'b InspectionBudget,
        behavior: TransactionBehavior,
    ) -> Result<Self> {
        // Check before installing/replacing either hook. The private writer
        // constructor retains its caller's exclusive borrow for this lifetime.
        if !connection.is_autocommit() {
            return Err(StoreError::Invalid(
                "Catalog scope requires an idle connection".into(),
            ));
        }
        budget.checkpoint()?;
        // Fixed BEGIN/busy-wait is intentionally outside the VM hook. SQLite
        // can otherwise finish BEGIN, then report INTERRUPT without returning
        // a Transaction for Rust to own. No substantive SQL runs here.
        let transaction = match Transaction::new_unchecked(connection, behavior) {
            Ok(tx) => tx,
            Err(error) => {
                return Err(Self::cleanup_connection(
                    connection,
                    StoreError::Database(error),
                    Vec::new(),
                ));
            }
        };
        let mut scope = Self {
            connection,
            transaction: Some(transaction),
            progress: None,
            budget: Some(budget),
        };
        match CatalogProgress::install(connection, budget) {
            Ok(progress) => {
                scope.progress = Some(progress);
                Ok(scope)
            }
            Err(error) => Err(scope.abort(error)),
        }
    }
    pub(crate) fn run<T>(mut self, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        match operation(&mut self) {
            Ok(value) if self.transaction.is_none() => Ok(value),
            Ok(_) => Err(self.abort(StoreError::Invalid(
                "Catalog operation did not finish its transaction".into(),
            ))),
            Err(error) => Err(self.abort(error)),
        }
    }
    fn detach(&mut self) -> Vec<rusqlite::Error> {
        self.progress
            .take()
            .map_or_else(Vec::new, |mut progress| progress.detach())
    }
    pub(crate) fn switch_governor(&mut self, budget: &'b InspectionBudget) -> Result<()> {
        let cleanup = self.detach();
        if !cleanup.is_empty() {
            return Err(StoreError::Catalog(Box::new(CatalogFailure {
                operation: Box::new(StoreError::Invalid(
                    "Cannot replace catalog governor".into(),
                )),
                cleanup,
                transaction_active: !self.connection.is_autocommit(),
            })));
        }
        budget.checkpoint()?;
        if self.transaction.is_none() || self.connection.is_autocommit() {
            return Err(StoreError::Invalid(
                "Catalog transaction is no longer active".into(),
            ));
        }
        self.budget = Some(budget);
        self.progress = Some(CatalogProgress::install(self.connection, budget)?);
        Ok(())
    }
    /// Only the already-guarded ordinary branch may leave this qualified scope.
    /// It retains prior file/SQL behavior and receives no VM-bound claim.
    pub(crate) fn release_governor_for_legacy(&mut self) -> Result<()> {
        let cleanup = self.detach();
        self.budget = None;
        if cleanup.is_empty() {
            Ok(())
        } else {
            Err(StoreError::Catalog(Box::new(CatalogFailure {
                operation: Box::new(StoreError::Invalid(
                    "Cannot release catalog governor".into(),
                )),
                cleanup,
                transaction_active: !self.connection.is_autocommit(),
            })))
        }
    }
    pub(crate) fn commit(&mut self) -> Result<()> {
        if self.transaction.is_none() || self.connection.is_autocommit() {
            return Err(StoreError::Invalid(
                "Catalog transaction is no longer active".into(),
            ));
        }
        if let Some(budget) = self.budget {
            budget.checkpoint()?;
        }
        let cleanup = self.detach();
        if !cleanup.is_empty() {
            return Err(StoreError::Catalog(Box::new(CatalogFailure {
                operation: Box::new(StoreError::Invalid(
                    "Cannot enter catalog commit fence".into(),
                )),
                cleanup,
                transaction_active: !self.connection.is_autocommit(),
            })));
        }
        // No cancellable callback, budget checkpoint or fallible validation may
        // turn successful COMMIT into a cancellation after SQLite commits.
        self.connection.execute_batch("COMMIT")?;
        self.discard_transaction();
        Ok(())
    }
    pub(crate) fn rollback(&mut self) -> Result<()> {
        let cleanup = self.detach();
        if !cleanup.is_empty() {
            return Err(StoreError::Catalog(Box::new(CatalogFailure {
                operation: Box::new(StoreError::Invalid(
                    "Cannot enter catalog rollback fence".into(),
                )),
                cleanup,
                transaction_active: !self.connection.is_autocommit(),
            })));
        }
        if !self.connection.is_autocommit() {
            self.connection.execute_batch("ROLLBACK")?;
        }
        self.discard_transaction();
        Ok(())
    }
    fn discard_transaction(&mut self) {
        if let Some(mut tx) = self.transaction.take() {
            tx.set_drop_behavior(DropBehavior::Ignore);
        }
    }
    fn cleanup_connection(
        connection: &Connection,
        operation: StoreError,
        mut cleanup: Vec<rusqlite::Error>,
    ) -> StoreError {
        if !connection.is_autocommit()
            && let Err(error) = connection.execute_batch("ROLLBACK")
        {
            cleanup.push(error);
        }
        let transaction_active = !connection.is_autocommit();
        if cleanup.is_empty() && !transaction_active {
            operation
        } else {
            StoreError::Catalog(Box::new(CatalogFailure {
                operation: Box::new(operation),
                cleanup,
                transaction_active,
            }))
        }
    }
    fn abort(&mut self, error: StoreError) -> StoreError {
        // Only an INTERRUPT requested by OUR hook is a typed governor stop.
        // Returned actual catalog fatal errors and unrelated interrupts survive.
        let own_interrupt = self
            .progress
            .as_ref()
            .is_some_and(|p| p.interrupted.load(Ordering::Acquire));
        let error = if own_interrupt
            && matches!(&error,StoreError::Database(e) if e.sqlite_error().is_some_and(|c|c.extended_code&0xff==9))
        {
            match self.budget.and_then(|b| b.checkpoint().err()) {
                Some(stop) => StoreError::Recording(stop),
                None => error,
            }
        } else {
            error
        };
        let cleanup = self.detach();
        let error = Self::cleanup_connection(self.connection, error, cleanup);
        self.discard_transaction();
        error
    }
}
impl Deref for CatalogTransaction<'_, '_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection
    }
}
impl Drop for CatalogTransaction<'_, '_> {
    fn drop(&mut self) {
        let _ = self.detach();
        // Panic backstop only. Normal errors use abort and preserve cleanup
        // failures. SQLite may already have rolled back interrupted DML.
        if self.transaction.is_some() && !self.connection.is_autocommit() {
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
}

#[cfg(test)]
mod tests;
