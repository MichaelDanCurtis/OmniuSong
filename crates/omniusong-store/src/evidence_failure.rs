//! Error domains for studio evidence operations. Archived SQLite bytes and
//! private snapshots must never impersonate a failure of the live catalog.
use crate::StoreError;
use omniusong_recording::{InspectionBudget, RecordingError};
use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceFailureOrigin {
    Evidence,
    LiveCatalog,
    /// SQLite Backup reads the live source and writes a private destination.
    /// Its returned error cannot prove which side failed.
    BackupTransfer,
}

#[derive(Debug)]
pub struct EvidenceFailure {
    origin: EvidenceFailureOrigin,
    error: Box<StoreError>,
    budget_stop: Option<RecordingError>,
}

impl EvidenceFailure {
    fn new(origin: EvidenceFailureOrigin, error: impl Into<StoreError>) -> Self {
        Self {
            origin,
            error: Box::new(error.into()),
            budget_stop: None,
        }
    }
    pub(crate) fn live_catalog(error: impl Into<StoreError>) -> Self {
        Self::new(EvidenceFailureOrigin::LiveCatalog, error)
    }
    pub(crate) fn backup_transfer(error: impl Into<StoreError>) -> Self {
        Self::new(EvidenceFailureOrigin::BackupTransfer, error)
    }
    pub fn origin(&self) -> EvidenceFailureOrigin {
        self.origin
    }
    pub fn operation_error(&self) -> &StoreError {
        &self.error
    }
    /// A sticky budget stop observed after an operation had already failed.
    /// The original error/domain remain available independently.
    pub fn budget_stop(&self) -> Option<&RecordingError> {
        self.budget_stop.as_ref()
    }
    /// This is diagnostic input to worker retirement policy, not a claim that
    /// a mixed Backup failure proves catalog corruption. Evidence-only SQLite
    /// failures deliberately return None, even for IOERR/CORRUPT/NOTADB.
    pub fn catalog_sqlite_error_code(&self) -> Option<i32> {
        if self.origin == EvidenceFailureOrigin::Evidence {
            return None;
        }
        match self.error.as_ref() {
            StoreError::Database(error) => error.sqlite_error().map(|e| e.extended_code),
            _ => None,
        }
    }
    /// Compatibility for the older low-level API. New job callers must retain
    /// the typed failure so a displayed cancellation cannot hide a catalog fault.
    pub fn into_store_error(self) -> StoreError {
        self.budget_stop
            .map(StoreError::Recording)
            .unwrap_or(*self.error)
    }
}

impl From<StoreError> for EvidenceFailure {
    fn from(error: StoreError) -> Self {
        Self::new(EvidenceFailureOrigin::Evidence, error)
    }
}
impl From<rusqlite::Error> for EvidenceFailure {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Database(error).into()
    }
}
impl From<std::io::Error> for EvidenceFailure {
    fn from(error: std::io::Error) -> Self {
        StoreError::Io(error).into()
    }
}
impl From<RecordingError> for EvidenceFailure {
    fn from(error: RecordingError) -> Self {
        StoreError::Recording(error).into()
    }
}
impl fmt::Display for EvidenceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(stop) = &self.budget_stop {
            stop.fmt(f)
        } else {
            self.error.fmt(f)
        }
    }
}
impl Error for EvidenceFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.error.as_ref())
    }
}

pub(crate) fn finish<T>(
    budget: &InspectionBudget,
    result: Result<T, EvidenceFailure>,
) -> Result<T, EvidenceFailure> {
    match (result, budget.checkpoint()) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(stop)) => Err(stop.into()),
        (Err(mut failure), stop) => {
            failure.budget_stop = stop.err();
            Err(failure)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_recording::{InspectionLimits, InspectionResource};
    use std::sync::{Arc, atomic::AtomicBool};

    fn database(code: i32) -> StoreError {
        StoreError::Database(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            Some("synthetic error; message never determines origin".into()),
        ))
    }
    #[test]
    fn identical_sqlite_codes_remain_distinct_by_typed_origin() {
        for code in [10, 11, 26, 10 | (8 << 8)] {
            let local = EvidenceFailure::from(database(code));
            assert_eq!(local.origin(), EvidenceFailureOrigin::Evidence);
            assert_eq!(local.catalog_sqlite_error_code(), None);
            let live = EvidenceFailure::live_catalog(database(code));
            assert_eq!(live.origin(), EvidenceFailureOrigin::LiveCatalog);
            assert_eq!(live.catalog_sqlite_error_code(), Some(code));
            let transfer = EvidenceFailure::backup_transfer(database(code));
            assert_eq!(transfer.origin(), EvidenceFailureOrigin::BackupTransfer);
            assert_eq!(transfer.catalog_sqlite_error_code(), Some(code));
        }
        let ordinary = EvidenceFailure::live_catalog(StoreError::Conflict);
        assert_eq!(ordinary.catalog_sqlite_error_code(), None);
    }
    #[test]
    fn sticky_stop_does_not_erase_original_live_or_transfer_failure() {
        for origin in [
            EvidenceFailureOrigin::Evidence,
            EvidenceFailureOrigin::LiveCatalog,
            EvidenceFailureOrigin::BackupTransfer,
        ] {
            for cancelled in [false, true] {
                let limits = InspectionLimits {
                    max_work_bytes: 0,
                    ..Default::default()
                };
                let budget =
                    InspectionBudget::new(limits, Arc::new(AtomicBool::new(cancelled))).unwrap();
                if !cancelled {
                    assert!(budget.charge_work(1).is_err());
                }
                let failure =
                    finish::<()>(&budget, Err(EvidenceFailure::new(origin, database(10))))
                        .unwrap_err();
                assert_eq!(failure.origin(), origin);
                assert!(matches!(failure.operation_error(), StoreError::Database(_)));
                assert_eq!(
                    failure.catalog_sqlite_error_code(),
                    (origin != EvidenceFailureOrigin::Evidence).then_some(10)
                );
                if cancelled {
                    assert!(matches!(
                        failure.budget_stop(),
                        Some(RecordingError::InspectionCancelled)
                    ));
                    assert!(matches!(
                        failure.into_store_error(),
                        StoreError::Recording(RecordingError::InspectionCancelled)
                    ));
                } else {
                    assert!(matches!(
                        failure.budget_stop(),
                        Some(RecordingError::InspectionLimit {
                            resource: InspectionResource::WorkBytes,
                            limit: 0,
                            attempted: 1
                        })
                    ));
                    assert!(matches!(
                        failure.into_store_error(),
                        StoreError::Recording(RecordingError::InspectionLimit {
                            resource: InspectionResource::WorkBytes,
                            ..
                        })
                    ));
                }
            }
        }
    }
    #[test]
    fn successful_work_cannot_escape_a_sticky_stop_and_ordinary_errors_keep_identity() {
        let stopped =
            InspectionBudget::new(InspectionLimits::default(), Arc::new(AtomicBool::new(true)))
                .unwrap();
        let error = finish(&stopped, Ok(17)).unwrap_err();
        assert_eq!(error.origin(), EvidenceFailureOrigin::Evidence);
        assert!(matches!(
            error.into_store_error(),
            StoreError::Recording(RecordingError::InspectionCancelled)
        ));
        let budget = InspectionBudget::new(
            InspectionLimits::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let error =
            finish::<()>(&budget, Err(EvidenceFailure::from(StoreError::Conflict))).unwrap_err();
        assert!(error.budget_stop().is_none());
        assert!(matches!(error.into_store_error(), StoreError::Conflict));
        assert_eq!(finish(&budget, Ok(17)).unwrap(), 17);
    }
}
