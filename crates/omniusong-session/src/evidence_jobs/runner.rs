//! One explicit artifact attempt or one read-only reconciliation, under the
//! independent journal owner. Historical records never retire a fresh worker.
use super::journal::{Admission, EvidenceJournal, JobEntry};
use super::*;
use crate::{NativeBackend, Reply};
use omniusong_recording::InspectionLimits;
use omniusong_store::{EvidenceFailure, EvidenceFailureOrigin, StoreError};
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceRunBoundary {
    RequestPersisted,
    StartedPersisted,
    BeforeTerminal,
}

fn stopped(error: &RecordingError) -> Option<EvidenceBudgetStop> {
    match error {
        RecordingError::InspectionCancelled => Some(EvidenceBudgetStop::Cancelled),
        RecordingError::InspectionLimit {
            resource,
            limit,
            attempted,
        } => Some(EvidenceBudgetStop::Limit {
            resource: *resource,
            limit: *limit,
            attempted: *attempted,
        }),
        _ => None,
    }
}
fn recording_kind(error: &RecordingError) -> EvidenceProblemKind {
    match error {
        RecordingError::InspectionCancelled => EvidenceProblemKind::Cancelled,
        RecordingError::InspectionLimit { .. } => EvidenceProblemKind::Limit,
        RecordingError::Owned => EvidenceProblemKind::Busy,
        RecordingError::Io(_) => EvidenceProblemKind::Io,
        RecordingError::Database(_) => EvidenceProblemKind::Sqlite,
        RecordingError::Invalid(_) | RecordingError::Conflict(_) => EvidenceProblemKind::Rejected,
        _ => EvidenceProblemKind::Unavailable,
    }
}
fn store_stop(error: &StoreError) -> Option<EvidenceBudgetStop> {
    match error {
        StoreError::Recording(error) => stopped(error),
        StoreError::Catalog(failure) => store_stop(failure.operation_error()),
        StoreError::Archive(failure) => store_stop(failure.operation_error()),
        StoreError::ArchiveSql(failure) => store_stop(failure.operation_error()),
        _ => None,
    }
}
pub(super) fn problem_from_store(
    error: &StoreError,
    origin: EvidenceResourceOrigin,
    later_stop: Option<&RecordingError>,
) -> EvidenceProblem {
    // Preserve explicit nested SQL provenance if an archive failure reaches
    // this diagnostic mapper. Private SQLite must never retire the studio.
    let nested = match error {
        StoreError::Archive(failure) => Some((failure.operation_error(), origin)),
        StoreError::ArchiveSql(failure) => Some((
            failure.operation_error(),
            if failure.origin() == omniusong_store::ArchiveSqlOrigin::LiveCatalog {
                EvidenceResourceOrigin::LiveCatalog
            } else {
                EvidenceResourceOrigin::Evidence
            },
        )),
        _ => None,
    };
    if let Some((inner, inner_origin)) = nested {
        let mut problem = problem_from_store(inner, inner_origin, later_stop);
        problem.detail = EvidenceMessage::new(error.to_string());
        return problem;
    }
    let (kind, sqlite_code, operation_stop) = match error {
        StoreError::Database(sqlite) => (
            EvidenceProblemKind::Sqlite,
            sqlite.sqlite_error().map(|e| e.extended_code),
            None,
        ),
        StoreError::Catalog(_) => (
            EvidenceProblemKind::Sqlite,
            error.catalog_sqlite_error_code(),
            store_stop(error),
        ),
        StoreError::Io(_) => (EvidenceProblemKind::Io, None, None),
        StoreError::Recording(recording) => (
            recording_kind(recording),
            match recording {
                RecordingError::Database(sqlite) => sqlite.sqlite_error().map(|e| e.extended_code),
                _ => None,
            },
            stopped(recording),
        ),
        StoreError::Invalid(_)
        | StoreError::Document(_)
        | StoreError::EditRejected(_)
        | StoreError::Conflict => (EvidenceProblemKind::Rejected, None, None),
        StoreError::Archive(_) | StoreError::ArchiveSql(_) => unreachable!("handled above"),
    };
    EvidenceProblem {
        kind,
        origin,
        detail: EvidenceMessage::new(error.to_string()),
        sqlite_code,
        budget_stop: later_stop.and_then(stopped).or(operation_stop),
    }
}
fn problem_from_service(error: &ServiceError) -> EvidenceProblem {
    let kind = match error.kind {
        FailureKind::InspectionCancelled => EvidenceProblemKind::Cancelled,
        FailureKind::InspectionLimit => EvidenceProblemKind::Limit,
        FailureKind::SourceBusy => EvidenceProblemKind::Busy,
        FailureKind::Rejected => EvidenceProblemKind::Rejected,
        _ => EvidenceProblemKind::Unavailable,
    };
    EvidenceProblem {
        kind,
        origin: EvidenceResourceOrigin::Evidence,
        detail: EvidenceMessage::new(&error.message),
        sqlite_code: None,
        budget_stop: if kind == EvidenceProblemKind::Cancelled {
            Some(EvidenceBudgetStop::Cancelled)
        } else {
            None
        },
    }
}
fn preflight_problem(error: &ServiceError, budget: &InspectionBudget) -> EvidenceProblem {
    if let Err(stop) = budget.checkpoint() {
        problem_from_store(
            &StoreError::Recording(stop),
            EvidenceResourceOrigin::Evidence,
            None,
        )
    } else {
        problem_from_service(error)
    }
}
pub(super) fn current_catalog_retirement(
    origin: EvidenceResourceOrigin,
    code: Option<i32>,
    detail: &EvidenceMessage,
) -> Option<ServiceError> {
    if origin != EvidenceResourceOrigin::Evidence && code.is_some_and(crate::fatal_sqlite_primary) {
        Some(ServiceError::storage_unavailable(EvidenceMessage::new(format!("Session storage could not be used safely during evidence export (SQLite code {}). This worker was stopped; no repair was attempted. {}",code.unwrap_or_default(),detail.message)).message,true))
    } else {
        None
    }
}
fn typed_export_problem(error: &EvidenceFailure) -> (EvidenceProblem, Option<ServiceError>) {
    let origin = match error.origin() {
        EvidenceFailureOrigin::Evidence => EvidenceResourceOrigin::Evidence,
        EvidenceFailureOrigin::LiveCatalog => EvidenceResourceOrigin::LiveCatalog,
        EvidenceFailureOrigin::BackupTransfer => EvidenceResourceOrigin::BackupTransfer,
    };
    let problem = problem_from_store(error.operation_error(), origin, error.budget_stop());
    let retirement =
        current_catalog_retirement(origin, error.catalog_sqlite_error_code(), &problem.detail);
    (problem, retirement)
}
fn historical(entry: &JobEntry) -> Result<Reply, ServiceError> {
    let request_sha256 = entry.view.request_sha256.clone().ok_or_else(|| {
        ServiceError::rejected(
            "This evidence job has no valid request; its files were retained without execution",
        )
    })?;
    Ok(Reply::EvidenceJobOutcome(Box::new(EvidenceJobReply {
        job_id:entry.view.job_id.clone(),request_sha256,history_sha256:Some(entry.view.history_sha256.clone()),attempted:false,
        outcome:entry.view.latest_recorded_outcome().cloned(),
        metadata_problem:(!entry.view.metadata_problems.is_empty()).then(||EvidenceMessage::new("This job has damaged or incomplete metadata. Read its retained history and reconcile explicitly; no artifact operation was retried.")),
        catalog_retirement:None,
    })))
}
fn matching_existing(entry: &JobEntry, expected_sha: &str) -> Result<(), ServiceError> {
    require(
        entry.view.request.is_some() && entry.view.request_sha256.as_deref() == Some(expected_sha),
        "Existing evidence job does not match this exact canonical request; no artifact operation was retried",
    )
}
fn uncertain(mut error: ServiceError) -> ServiceError {
    error.outcome_unknown = true;
    error
}

impl NativeBackend {
    fn evidence_origin(&self) -> Result<PathBuf, ServiceError> {
        let origin = self
            .store
            .as_ref()
            .map_or(self.paths.native.as_path(), |store| store.root());
        path_text(origin)?;
        Ok(origin.to_owned())
    }
    pub(crate) fn run_evidence_job(
        &mut self,
        request: &EvidenceJobRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        let budget =
            InspectionBudget::new(InspectionLimits::default(), cancelled).map_err(budget_error)?;
        self.run_evidence_job_using(request, &budget, |_| Ok(()))
    }
    pub(super) fn run_evidence_job_using(
        &mut self,
        request: &EvidenceJobRequest,
        budget: &InspectionBudget,
        mut observe: impl FnMut(EvidenceRunBoundary) -> Result<(), ServiceError>,
    ) -> Result<Reply, ServiceError> {
        checkpoint(budget)?;
        request.validate()?;
        charge(budget, 2 * MAX_EVIDENCE_JOB_REQUEST_BYTES as u64)?;
        let expected_sha = request.fingerprint()?;
        let origin = self.evidence_origin()?;
        require(
            origin.as_os_str() == Path::new(&request.origin_root).as_os_str(),
            "Evidence request origin differs from the configured exact studio root",
        )?;
        // Existing identities are resolved before needing the live source. A
        // request-only record is historical too and never resumes execution.
        let existing = EvidenceJournal::open(&origin, false, budget)?;
        if let Some(journal) = &existing
            && let Some(entry) = journal.read_if_present(&request.job_id, budget)?
        {
            matching_existing(&entry, &expected_sha).map_err(uncertain)?;
            return historical(&entry);
        }
        let actual = self.store()?.root();
        require(
            actual.as_os_str() == origin.as_os_str(),
            "New evidence execution requires its actual canonical source store",
        )?;
        let journal = match existing {
            Some(journal) => journal,
            None => EvidenceJournal::open(&origin, true, budget)?.ok_or_else(|| {
                ServiceError::backend("Evidence metadata initialization did not complete", false)
            })?,
        };
        let entry = match journal.admit(request, budget).map_err(uncertain)? {
            Admission::Existing(entry) => {
                matching_existing(&entry, &expected_sha).map_err(uncertain)?;
                return historical(&entry);
            }
            Admission::Fresh(entry) => entry,
        };
        observe(EvidenceRunBoundary::RequestPersisted).map_err(uncertain)?;
        let permit = journal.reserve_finalization(budget)?;
        let entry = journal.mark_started(entry, budget).map_err(uncertain)?;
        observe(EvidenceRunBoundary::StartedPersisted).map_err(uncertain)?;
        let mut attempted = false;
        let mut catalog_retirement = None;
        let outcome = match journal
            .verify_entry(&entry, budget)
            .and_then(|_| checkpoint(budget))
        {
            Err(error) => EvidenceOutcome::Stopped {
                problem: preflight_problem(&error, budget),
            },
            Ok(()) => {
                attempted = true;
                match &request.operation {
                    EvidenceOperation::Export {
                        export_id,
                        destination,
                        ..
                    } => match self.store()?.export_retained_evidence_full_root_typed(
                        destination,
                        export_id,
                        budget,
                    ) {
                        Ok(receipt) => EvidenceOutcome::Exported { receipt },
                        Err(error) => {
                            let (problem, retirement) = typed_export_problem(&error);
                            catalog_retirement = retirement;
                            EvidenceOutcome::Stopped { problem }
                        }
                    },
                    EvidenceOperation::Restore {
                        archive_path,
                        directory,
                        expected_archive_sha256,
                    } => match self.store()?.restore_retained_evidence_reviewed(
                        archive_path,
                        directory,
                        expected_archive_sha256,
                        budget,
                    ) {
                        Ok(receipt) => EvidenceOutcome::Restored { receipt },
                        Err(error) => EvidenceOutcome::Stopped {
                            problem: problem_from_store(
                                &error,
                                EvidenceResourceOrigin::Evidence,
                                budget.checkpoint().err().as_ref(),
                            ),
                        },
                    },
                }
            }
        };
        // The artifact method's returned result wins a cancellation arriving
        // after it completed. Only this bounded prepaid metadata append follows.
        // Never replace a live-related fatal with a terminal-write problem.
        let persisted = observe(EvidenceRunBoundary::BeforeTerminal)
            .and_then(|_| journal.complete(entry, &outcome, permit));
        let (history_sha256, metadata_problem) = match persisted {
            Ok(entry) => (Some(entry.view.history_sha256.clone()), None),
            Err(error) => (None, Some(EvidenceMessage::new(error.message))),
        };
        Ok(Reply::EvidenceJobOutcome(Box::new(EvidenceJobReply {
            job_id: request.job_id.clone(),
            request_sha256: expected_sha,
            history_sha256,
            attempted,
            outcome: Some(outcome),
            metadata_problem,
            catalog_retirement,
        })))
    }
    pub(crate) fn inspect_retained_evidence(
        &mut self,
        path: &Path,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        let budget =
            InspectionBudget::new(InspectionLimits::default(), cancelled).map_err(budget_error)?;
        checkpoint(&budget)?;
        path_text(path)?;
        charge(&budget, path.as_os_str().len() as u64 + 160)?;
        // Archive-only SQLite errors cannot retire the unrelated live store.
        omniusong_store::inspect_evidence_archive(path, &budget)
            .map(|inspection| Reply::RetainedEvidenceInspected(Box::new(inspection)))
            .map_err(|error| local_artifact_error(&error, &budget, false))
    }
    pub(crate) fn list_evidence_jobs(
        &mut self,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        let budget =
            InspectionBudget::new(InspectionLimits::default(), cancelled).map_err(budget_error)?;
        let origin = self.evidence_origin()?;
        let list = match EvidenceJournal::open(&origin, false, &budget)? {
            Some(journal) => journal.list(&budget)?,
            None => EvidenceJobList::default(),
        };
        checkpoint(&budget)?;
        check_bounded(&list, MAX_EVIDENCE_JOB_LIST_BYTES)?;
        Ok(Reply::EvidenceJobs(list))
    }
    pub(crate) fn read_evidence_job(
        &mut self,
        id: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        let budget =
            InspectionBudget::new(InspectionLimits::default(), cancelled).map_err(budget_error)?;
        let origin = self.evidence_origin()?;
        let journal = EvidenceJournal::open(&origin, false, &budget)?.ok_or_else(|| {
            ServiceError::rejected("No evidence job journal exists for this exact studio root")
        })?;
        let entry = journal.read(id, &budget)?;
        checkpoint(&budget)?;
        check_bounded(&entry.view, MAX_EVIDENCE_JOB_VIEW_BYTES)?;
        Ok(Reply::EvidenceJob(Box::new(entry.view)))
    }
    pub(crate) fn reconcile_evidence_job(
        &mut self,
        id: &str,
        expected_request_sha: &str,
        expected_history_sha: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        let budget =
            InspectionBudget::new(InspectionLimits::default(), cancelled).map_err(budget_error)?;
        self.reconcile_evidence_job_using(id, expected_request_sha, expected_history_sha, &budget)
            .map_err(uncertain)
    }
    pub(super) fn reconcile_evidence_job_using(
        &mut self,
        id: &str,
        expected_request_sha: &str,
        expected_history_sha: &str,
        budget: &InspectionBudget,
    ) -> Result<Reply, ServiceError> {
        checkpoint(budget)?;
        require(
            valid_id(id) && valid_sha(expected_request_sha) && valid_sha(expected_history_sha),
            "Reconciliation needs its exact job/request/history identity",
        )?;
        let origin = self.evidence_origin()?;
        let journal = EvidenceJournal::open(&origin, false, budget)?.ok_or_else(|| {
            ServiceError::rejected("No evidence job journal exists; no artifact was created")
        })?;
        let entry = journal.read(id, budget)?;
        matching_existing(&entry, expected_request_sha)?;
        require(
            entry.view.history_sha256 == expected_history_sha,
            "Evidence job history changed; read it again before reconciling",
        )?;
        require(
            entry.view.occupied_observations < MAX_EVIDENCE_JOB_OBSERVATIONS,
            "Evidence job observation capacity is full; no history was evicted",
        )?;
        let request = entry.view.request.clone().ok_or_else(|| {
            ServiceError::rejected("A valid exact request is required for reconciliation")
        })?;
        let permit = journal.reserve_finalization(budget)?;
        let mut attempted = false;
        let outcome = match journal
            .verify_entry(&entry, budget)
            .and_then(|_| checkpoint(budget))
        {
            Err(error) => EvidenceOutcome::Stopped {
                problem: preflight_problem(&error, budget),
            },
            Ok(()) => {
                attempted = true;
                match &request.operation {
                    EvidenceOperation::Export {
                        export_id,
                        destination,
                        ..
                    } => match omniusong_store::reconcile_evidence_export(
                        destination,
                        export_id,
                        &request.origin_root,
                        budget,
                    ) {
                        Ok(inspection) => EvidenceOutcome::Exported {
                            receipt: inspection.receipt,
                        },
                        Err(error) => EvidenceOutcome::Stopped {
                            problem: problem_from_store(
                                &error,
                                EvidenceResourceOrigin::Evidence,
                                budget.checkpoint().err().as_ref(),
                            ),
                        },
                    },
                    EvidenceOperation::Restore {
                        archive_path,
                        directory,
                        expected_archive_sha256,
                    } => match omniusong_store::inspect_restored_evidence(
                        directory,
                        archive_path,
                        expected_archive_sha256,
                        budget,
                    ) {
                        Ok(receipt) => EvidenceOutcome::Restored { receipt },
                        Err(error) => EvidenceOutcome::Stopped {
                            problem: problem_from_store(
                                &error,
                                EvidenceResourceOrigin::Evidence,
                                budget.checkpoint().err().as_ref(),
                            ),
                        },
                    },
                }
            }
        };
        let persisted = journal.observe(entry, &outcome, permit);
        let (history_sha256, metadata_problem) = match persisted {
            Ok(entry) => (Some(entry.view.history_sha256.clone()), None),
            Err(error) => (None, Some(EvidenceMessage::new(error.message))),
        };
        Ok(Reply::EvidenceJobOutcome(Box::new(EvidenceJobReply {
            job_id: id.to_owned(),
            request_sha256: expected_request_sha.to_owned(),
            history_sha256,
            attempted,
            outcome: Some(outcome),
            metadata_problem,
            catalog_retirement: None,
        })))
    }
}
fn local_artifact_error(
    error: &StoreError,
    budget: &InspectionBudget,
    outcome_unknown: bool,
) -> ServiceError {
    let stop = budget.checkpoint().err();
    let mut mapped = if let Some(stop) = stop {
        budget_error(stop)
    } else {
        let kind = match error {
            StoreError::Recording(RecordingError::Owned) => FailureKind::SourceBusy,
            _ => FailureKind::SourceUnavailable,
        };
        ServiceError {
            kind,
            message: EvidenceMessage::new(error.to_string()).message,
            outcome_unknown,
        }
    };
    mapped.outcome_unknown = outcome_unknown;
    mapped
}
