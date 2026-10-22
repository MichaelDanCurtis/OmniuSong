//! Durable copied recovery. Prepared is immutable; Started is uncertainty,
//! and only an authenticated existing recovered artifact can become Verified.
use super::{
    PreparedPerformanceSourceActionReceipt, decode, digest, encode, require, review, sha,
    validate_id,
};
use crate::{
    FailureKind, NativeBackend, PerformanceSourceReview, Reply, ServiceError, store_failure,
};
use omniusong_engine::performance_copy::{reconcile_performance_copy, recover_performance_copy};
use omniusong_recording::{
    CaptureReader, CopyActionOwner, CopyRecoveryReceipt, InspectionBudget, InspectionLimits,
    InspectionUsage, RecordingError, RecoveredCopyView,
};
use omniusong_store::{
    MAX_PERFORMANCE_SOURCE_ACTION_BYTES, MAX_PERFORMANCE_SOURCE_ACTIONS,
    MAX_PERFORMANCE_SOURCE_REVIEW_BYTES, PerformanceSourceActionHistory,
    StoredPerformanceSourceAction,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveredPerformanceSourceResult {
    pub take_id: String,
    pub descriptor_sha256: String,
    pub original_rows_sha256: String,
    pub evidence_sha256: String,
    pub evidence_bytes: u64,
    pub working_sha256: String,
    pub working_bytes: u64,
    pub manifest_sha256: String,
    pub selected_chunk_count: u64,
    pub selected_chunk_index_sha256: String,
    pub unknown_tail: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerformanceSourceExecutionPhase {
    Started,
    Verified {
        result: Box<RecoveredPerformanceSourceResult>,
        usage: InspectionUsage,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceExecutionRevision {
    pub version: u32,
    pub id: String,
    pub prepared_payload_sha256: String,
    pub phase: PerformanceSourceExecutionPhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceActionState {
    pub prepared: PreparedPerformanceSourceActionReceipt,
    pub revision: u64,
    pub payload_sha256: String,
    pub execution: Option<PerformanceSourceExecutionRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceExecutionReceipt {
    pub state: PerformanceSourceActionState,
    pub observed: RecoveredPerformanceSourceResult,
    pub usage: InspectionUsage,
    pub replayed: bool,
    pub original_reopened: bool,
}

fn validate_usage(usage: InspectionUsage) -> Result<(), ServiceError> {
    let limits = InspectionLimits::default();
    require(
        usage.file_bytes <= limits.max_file_bytes
            && usage.physical_rows <= limits.max_physical_rows
            && usage.work_bytes <= limits.max_work_bytes
            && usage.row_visits <= limits.max_row_visits
            && usage.chunk_visits <= limits.max_chunk_visits
            && usage.record_visits <= limits.max_record_visits
            && usage.groups <= limits.max_groups
            && usage.vm_steps <= limits.max_vm_steps,
        "Execution usage exceeds the compiled inspection ceilings",
    )
}

fn validate_result(
    result: &RecoveredPerformanceSourceResult,
    source: &PerformanceSourceReview,
) -> Result<(), ServiceError> {
    let selection = source
        .performance
        .selection
        .as_ref()
        .ok_or_else(|| ServiceError::rejected("Retained source has no selected checkpoint"))?;
    require(
        result.take_id == source.take_id
            && result.descriptor_sha256 == source.performance.descriptor_sha256
            && result.original_rows_sha256 == source.performance.original_rows_sha256
            && result.evidence_sha256 == source.performance.journal_sha256_at_review
            && result.evidence_bytes == source.byte_length
            && result.selected_chunk_count == selection.chunk_count
            && result.selected_chunk_index_sha256 == selection.chunk_index_sha256
            && result.unknown_tail
            && source.performance.unknown_tail
            && [
                &result.descriptor_sha256,
                &result.original_rows_sha256,
                &result.evidence_sha256,
                &result.working_sha256,
                &result.manifest_sha256,
                &result.selected_chunk_index_sha256,
            ]
            .into_iter()
            .all(|value| sha(value))
            && result.working_bytes > 0
            && result.working_bytes <= InspectionLimits::default().max_file_bytes
            && result
                .evidence_bytes
                .checked_add(result.working_bytes)
                .is_some_and(|n| n <= 1_073_741_824),
        "Recovered result contradicts its exact retained source review",
    )
}

/// Validate complete typed history without calling the parent's decoder.
/// The caller separately validates Prepared paths and recording-intent origins.
pub(super) fn validate_history(
    history: &PerformanceSourceActionHistory,
    prepared: &[PreparedPerformanceSourceActionReceipt],
) -> Result<Vec<PerformanceSourceActionState>, ServiceError> {
    require(
        prepared.len() <= MAX_PERFORMANCE_SOURCE_ACTIONS
            && history.reviews.len() == prepared.len()
            && history.revisions.len() <= MAX_PERFORMANCE_SOURCE_ACTIONS * 16,
        "Source execution history exceeds its bounded identity or row count",
    )?;
    let mut states = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut operations = BTreeSet::new();
    for receipt in prepared {
        receipt.action.validate()?;
        require(
            receipt.revision == 1
                && sha(&receipt.payload_sha256)
                && operations.insert(receipt.action.recovery_operation_id.0.clone()),
            "Prepared execution identity, revision or operation is invalid",
        )?;
        let blobs: Vec<_> = history
            .reviews
            .iter()
            .filter(|b| b.id == receipt.action.id)
            .collect();
        require(
            blobs.len() == 1,
            "Source execution review is missing or duplicated",
        )?;
        let blob = blobs[0];
        require(
            blob.payload_sha256 == receipt.action.review_blob_sha256
                && digest(b"", &blob.payload) == blob.payload_sha256,
            "Source execution review bytes differ from their immutable reference",
        )?;
        let (source, admission) = review(&blob.payload)?;
        require(
            source.stage_id == receipt.action.stage_id
                && source.take_id == receipt.action.take_id
                && admission == receipt.action.admission_sha256,
            "Prepared execution admission differs from its retained review",
        )?;
        require(
            states
                .insert(
                    receipt.action.id.clone(),
                    PerformanceSourceActionState {
                        prepared: receipt.clone(),
                        revision: 0,
                        payload_sha256: String::new(),
                        execution: None,
                    },
                )
                .is_none(),
            "Duplicate Prepared execution identity",
        )?;
        sources.insert(receipt.action.id.clone(), source);
    }
    for row in &history.revisions {
        let state = states
            .get_mut(&row.id)
            .ok_or_else(|| ServiceError::rejected("Execution history has an orphan action"))?;
        require(
            row.revision == state.revision + 1
                && row.revision <= 3
                && row.review_blob_sha256 == state.prepared.action.review_blob_sha256
                && row.payload.len() <= MAX_PERFORMANCE_SOURCE_ACTION_BYTES
                && digest(b"", &row.payload) == row.payload_sha256,
            "Execution history is noncontiguous or changes its immutable review",
        )?;
        if row.revision == 1 {
            require(
                encode(&state.prepared.action, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)? == row.payload
                    && row.payload_sha256 == state.prepared.payload_sha256,
                "Execution Prepared row differs from its exact immutable receipt",
            )?;
        } else {
            let execution: PerformanceSourceExecutionRevision =
                serde_json::from_slice(&row.payload)
                    .map_err(|e| ServiceError::rejected(e.to_string()))?;
            require(
                execution.version == 1
                    && execution.id == row.id
                    && execution.prepared_payload_sha256 == state.prepared.payload_sha256
                    && encode(&execution, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)? == row.payload,
                "Execution row has an unsupported identity, version or canonical payload",
            )?;
            match (
                &execution.phase,
                row.revision,
                state.execution.as_ref().map(|e| &e.phase),
            ) {
                (PerformanceSourceExecutionPhase::Started, 2, None) => (),
                (
                    PerformanceSourceExecutionPhase::Verified { result, usage },
                    3,
                    Some(PerformanceSourceExecutionPhase::Started),
                ) => {
                    validate_result(result, &sources[&row.id])?;
                    validate_usage(*usage)?;
                }
                _ => {
                    return Err(ServiceError::rejected(
                        "Execution history must be Prepared, Started, then Verified",
                    ));
                }
            }
            state.execution = Some(execution);
        }
        state.revision = row.revision;
        state.payload_sha256 = row.payload_sha256.clone();
    }
    require(
        states.values().all(|s| s.revision > 0),
        "Execution history is missing a Prepared row",
    )?;
    let result: Vec<_> = states.into_values().collect();
    encode(&result, 576 * 1024)?;
    Ok(result)
}

fn local(error: RecordingError) -> ServiceError {
    let kind = match error {
        RecordingError::InspectionCancelled => FailureKind::InspectionCancelled,
        RecordingError::InspectionLimit { .. } => FailureKind::InspectionLimit,
        RecordingError::Owned => FailureKind::SourceBusy,
        _ => FailureKind::SourceUnavailable,
    };
    ServiceError {
        kind,
        message: error.to_string(),
        outcome_unknown: false,
    }
}
fn unavailable(message: impl Into<String>) -> ServiceError {
    ServiceError {
        kind: FailureKind::SourceUnavailable,
        message: message.into(),
        outcome_unknown: false,
    }
}
fn uncertain(mut error: ServiceError) -> ServiceError {
    error.outcome_unknown = true;
    error
}

struct Loaded {
    root: PathBuf,
    history: PerformanceSourceActionHistory,
    states: Vec<PerformanceSourceActionState>,
}
struct Request<'a> {
    id: &'a str,
    revision: u64,
    payload_sha256: &'a str,
}
impl Loaded {
    fn authenticate(
        &self,
        request: &Request<'_>,
    ) -> Result<PerformanceSourceActionState, ServiceError> {
        validate_id(request.id)?;
        require(
            (1..=3).contains(&request.revision) && sha(request.payload_sha256),
            "Invalid historical source execution request",
        )?;
        require(
            self.history.revisions.iter().any(|r| {
                r.id == request.id
                    && r.revision == request.revision
                    && r.payload_sha256 == request.payload_sha256
            }),
            "Source execution request has no exact retained historical row",
        )?;
        self.states
            .iter()
            .find(|s| s.prepared.action.id == request.id)
            .cloned()
            .ok_or_else(|| ServiceError::rejected("Source execution choice is missing"))
    }
    fn raw_review(&self, id: &str) -> Result<&[u8], ServiceError> {
        self.history
            .reviews
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.payload.as_slice())
            .ok_or_else(|| ServiceError::rejected("Source execution review is missing"))
    }
}

// Private observation boundaries allow tests to model acknowledged process
// gaps and a concurrent catalog append without changing production requests.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Started,
    Recovered,
}

struct CompletionProof<'a, 'b> {
    view: &'a RecoveredCopyView<'b>,
    summary: RecoveredPerformanceSourceResult,
    fresh: bool,
    execute: bool,
}

fn parent_directory(root: &Path, create: bool) -> Result<PathBuf, ServiceError> {
    parent_directory_using(root, create, |path| fs::create_dir(path))
}

fn parent_directory_using(
    root: &Path,
    create: bool,
    create_directory: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<PathBuf, ServiceError> {
    // Root was canonicalized by NativeStore. Reject redirected ancestors and a
    // redirected leaf, including on an existing-result read-only request.
    for path in root.ancestors() {
        let metadata = fs::symlink_metadata(path).map_err(|e| local(e.into()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(unavailable(
                "Source action root is not an ordinary directory",
            ));
        }
    }
    if fs::canonicalize(root).map_err(|e| local(e.into()))? != root {
        return Err(unavailable("Source action root changed its canonical path"));
    }
    let parent = root.join("source-actions");
    match fs::symlink_metadata(&parent) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            match create_directory(&parent) {
                Ok(()) => (),
                // Another independently Started action may have created the
                // shared parent. Its ordinary identity is rechecked below;
                // per-action lease and artifact creation remain exclusive.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(local(e.into())),
            }
            #[cfg(unix)]
            fs::File::open(root)
                .and_then(|f| f.sync_all())
                .map_err(|e| local(e.into()))?;
        }
        Err(e) => return Err(local(e.into())),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(unavailable(
                "Source action parent is not an ordinary directory",
            ));
        }
        Ok(_) => (),
    }
    let metadata = fs::symlink_metadata(&parent).map_err(|e| local(e.into()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(unavailable(
            "Source action parent is not an ordinary directory",
        ));
    }
    if fs::canonicalize(&parent).map_err(|e| local(e.into()))? != parent {
        return Err(unavailable(
            "Source action parent changed its canonical path",
        ));
    }
    Ok(parent)
}

fn committed(
    row: &StoredPerformanceSourceAction,
    state: &PerformanceSourceActionState,
    revision: u64,
    payload: &[u8],
) -> Result<(), ServiceError> {
    require(
        row.id == state.prepared.action.id
            && row.revision == revision
            && row.review_blob_sha256 == state.prepared.action.review_blob_sha256
            && row.payload == payload
            && row.payload_sha256 == digest(b"", payload),
        "Source execution commit did not acknowledge the exact requested row",
    )
    .map_err(uncertain)
}

fn observed(
    proof: &CopyRecoveryReceipt,
    source: &PerformanceSourceReview,
) -> Result<RecoveredPerformanceSourceResult, ServiceError> {
    let result = RecoveredPerformanceSourceResult {
        take_id: proof.inspection.descriptor.take_id.0.clone(),
        descriptor_sha256: proof.inspection.descriptor_sha256.clone(),
        original_rows_sha256: proof.inspection.original_rows_sha256.clone(),
        evidence_sha256: source.performance.journal_sha256_at_review.clone(),
        evidence_bytes: source.byte_length,
        working_sha256: proof.inspection.journal_sha256.clone(),
        working_bytes: proof.inspection.byte_length,
        manifest_sha256: proof.manifest.sha256.clone(),
        selected_chunk_count: proof.manifest.body.chunk_count,
        selected_chunk_index_sha256: proof.manifest.body.chunk_index_sha256.clone(),
        unknown_tail: proof.inspection.report.unknown_tail,
    };
    require(
        proof.manifest.body.interrupted.is_some(),
        "Copied recovery must retain interruption authority",
    )?;
    validate_result(&result, source)?;
    Ok(result)
}

impl NativeBackend {
    fn load_execution_history(&mut self) -> Result<Loaded, ServiceError> {
        let root = self.store()?.root().to_owned();
        let history = self
            .store()?
            .list_performance_source_actions()
            .map_err(|e| store_failure(e, false))?;
        let prepared = decode(&history, &root)?;
        self.validate_source_origins(&prepared, &history)?;
        let states = validate_history(&history, &prepared)?;
        Ok(Loaded {
            root,
            history,
            states,
        })
    }

    pub(crate) fn list_source_action_states(&mut self) -> Result<Reply, ServiceError> {
        Ok(Reply::PerformanceSourceActionStates(
            self.load_execution_history()?.states,
        ))
    }

    pub(crate) fn execute_source_action(
        &mut self,
        id: &str,
        revision: u64,
        payload_sha256: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.run_source_action(
            Request {
                id,
                revision,
                payload_sha256,
            },
            cancelled,
            true,
            |_, _| Ok(()),
        )
        .map(|r| Reply::PerformanceSourceExecution(Box::new(r)))
    }

    pub(crate) fn reconcile_source_action(
        &mut self,
        id: &str,
        revision: u64,
        payload_sha256: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.run_source_action(
            Request {
                id,
                revision,
                payload_sha256,
            },
            cancelled,
            false,
            |_, _| Ok(()),
        )
        .map(|r| Reply::PerformanceSourceExecution(Box::new(r)))
    }

    fn run_source_action(
        &mut self,
        request: Request<'_>,
        cancelled: Arc<AtomicBool>,
        execute: bool,
        mut observe: impl FnMut(Boundary, &mut Self) -> Result<(), ServiceError>,
    ) -> Result<PerformanceSourceExecutionReceipt, ServiceError> {
        let loaded = self.load_execution_history()?;
        let mut state = loaded.authenticate(&request)?;
        require(
            execute || state.revision > 1,
            "Prepared choices have no existing execution to reconcile",
        )?;
        if !cfg!(unix) {
            let error =
                unavailable("Copied recovery file identity is not qualified on this platform");
            return Err(if state.revision > 1 {
                uncertain(error)
            } else {
                error
            });
        }
        let source = review(loaded.raw_review(request.id)?)?.0;
        let budget =
            InspectionBudget::new(InspectionLimits::default(), cancelled).map_err(local)?;
        let fresh = state.revision == 1;
        let reader = if fresh {
            // No filesystem writes precede an exact, newly confirmed Started.
            budget.checkpoint().map_err(local)?;
            let path = crate::performance_sources::source_path(&loaded.root, &source.stage_id)?;
            let reader = CaptureReader::open_bounded(&path, &budget).map_err(local)?;
            let current = crate::performance_sources::review_reader(&source.stage_id, &reader)?;
            let admission = review(&encode(&current, MAX_PERFORMANCE_SOURCE_REVIEW_BYTES)?)
                .map_err(|e| unavailable(e.message))?
                .1;
            if admission != state.prepared.action.admission_sha256 {
                return Err(unavailable(
                    "Fresh source admission differs from the immutable Prepared review",
                ));
            }
            budget.checkpoint().map_err(local)?;
            let started = PerformanceSourceExecutionRevision {
                version: 1,
                id: request.id.into(),
                prepared_payload_sha256: state.prepared.payload_sha256.clone(),
                phase: PerformanceSourceExecutionPhase::Started,
            };
            let payload = encode(&started, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?;
            let row = self
                .store()?
                .save_performance_source_action_if_history_matches(
                    request.id,
                    Some(1),
                    loaded.raw_review(request.id)?,
                    &payload,
                    &loaded.history,
                )
                .map_err(|e| store_failure(e, true))?;
            // An uncertain acknowledgement ends here, without creating even the
            // parent directory. Retrying Started never restarts this branch.
            committed(&row, &state, 2, &payload)?;
            state.revision = 2;
            state.payload_sha256 = row.payload_sha256;
            state.execution = Some(started);
            Some(reader)
        } else {
            None
        };

        // Every error beyond this durable boundary retains outcome uncertainty,
        // including a no-commit conflict publishing Verified after file commit.
        let result = (|| {
            if fresh {
                observe(Boundary::Started, self)?;
            }
            budget.checkpoint().map_err(local)?;
            let parent = parent_directory(&loaded.root, fresh)?;
            budget.checkpoint().map_err(local)?;
            let owner = if fresh {
                CopyActionOwner::create(&parent, request.id)
            } else {
                CopyActionOwner::open_existing(&parent, request.id)
            }
            .map_err(local)?;
            owner.verify().map_err(local)?;
            if let Some(reader) = reader {
                recover_performance_copy(
                    reader,
                    &source.performance,
                    &parent,
                    request.id,
                    &state.prepared.action.recovery_operation_id.0,
                )
                .map_err(|e| local(e.cause))?;
            }
            // The fresh library has released its working owner. Reacquire it
            // with a full proof under this same governor, retaining the lease.
            let view = RecoveredCopyView::open(
                &owner,
                &source.performance.journal_sha256_at_review,
                source.byte_length,
                &budget,
            )
            .map_err(local)?;
            let proof = reconcile_performance_copy(
                &view,
                &source.performance,
                request.id,
                &state.prepared.action.recovery_operation_id.0,
            )
            .map_err(local)?;
            let summary = observed(&proof, &source)?;
            self.finish_source_action(
                &request,
                &state,
                &source,
                CompletionProof {
                    view: &view,
                    summary,
                    fresh,
                    execute,
                },
                &mut observe,
            )
        })();
        result.map_err(uncertain)
    }

    fn finish_source_action(
        &mut self,
        request: &Request<'_>,
        admitted: &PerformanceSourceActionState,
        source: &PerformanceSourceReview,
        proof: CompletionProof<'_, '_>,
        observe: &mut impl FnMut(Boundary, &mut Self) -> Result<(), ServiceError>,
    ) -> Result<PerformanceSourceExecutionReceipt, ServiceError> {
        let CompletionProof {
            view,
            summary,
            fresh,
            execute,
        } = proof;
        let current = self.load_execution_history()?;
        let mut state = current.authenticate(request)?;
        require(
            state.prepared == admitted.prepared && state.revision >= 2,
            "Execution changed its immutable Prepared choice",
        )?;
        validate_result(&summary, source)?;
        view.verify_unchanged().map_err(local)?;
        if let Some(PerformanceSourceExecutionRevision {
            phase: PerformanceSourceExecutionPhase::Verified { result, .. },
            ..
        }) = &state.execution
        {
            if **result != summary {
                return Err(unavailable(
                    "Current artifact differs from the immutable recorded Verified result",
                ));
            }
        } else if execute {
            let usage = view.budget().usage();
            validate_usage(usage)?;
            let verified = PerformanceSourceExecutionRevision {
                version: 1,
                id: request.id.into(),
                prepared_payload_sha256: state.prepared.payload_sha256.clone(),
                phase: PerformanceSourceExecutionPhase::Verified {
                    result: Box::new(summary.clone()),
                    usage,
                },
            };
            let payload = encode(&verified, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?;
            observe(Boundary::Recovered, self)?;
            view.verify_unchanged().map_err(local)?;
            let row = self
                .store()?
                .save_performance_source_action_if_history_matches(
                    request.id,
                    Some(2),
                    current.raw_review(request.id)?,
                    &payload,
                    &current.history,
                )
                .map_err(|e| store_failure(e, true))?;
            committed(&row, &state, 3, &payload)?;
            state.revision = 3;
            state.payload_sha256 = row.payload_sha256;
            state.execution = Some(verified);
        }
        view.verify_unchanged().map_err(local)?;
        let receipt = PerformanceSourceExecutionReceipt {
            state,
            observed: summary,
            usage: view.budget().usage(),
            replayed: !fresh,
            original_reopened: fresh,
        };
        validate_usage(receipt.usage)?;
        encode(&receipt, 576 * 1024)?;
        Ok(receipt)
    }
}
