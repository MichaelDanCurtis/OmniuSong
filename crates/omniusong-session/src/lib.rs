//! Bounded session storage service. UI callers only submit/poll nonblocking channels.
mod evidence_jobs;
mod performance_intents;
mod performance_source_actions;
mod performance_sources;
#[cfg(test)]
mod source_action_controls_review;
pub use evidence_jobs::{
    EvidenceBudgetStop, EvidenceJobList, EvidenceJobPhase, EvidenceJobReply, EvidenceJobRequest,
    EvidenceJobSummary, EvidenceJobView, EvidenceMessage, EvidenceMetadataProblem,
    EvidenceObservation, EvidenceObservationStatus, EvidenceOperation, EvidenceOutcome,
    EvidenceProblem, EvidenceProblemKind, EvidenceResourceOrigin, MAX_EVIDENCE_JOB_HEADER_BYTES,
    MAX_EVIDENCE_JOB_LIST_BYTES, MAX_EVIDENCE_JOB_MESSAGE_BYTES, MAX_EVIDENCE_JOB_OBSERVATIONS,
    MAX_EVIDENCE_JOB_RECORD_BYTES, MAX_EVIDENCE_JOB_REQUEST_BYTES, MAX_EVIDENCE_JOB_STARTED_BYTES,
    MAX_EVIDENCE_JOB_VIEW_BYTES, MAX_EVIDENCE_JOBS, MAX_EVIDENCE_OUTCOME_REPLY_BYTES,
};
use omniusong_core::{
    MAX_PROJECT_BYTES, ProjectDocument,
    contracts::{OperationId, ProjectEditIntent, ProjectId, Validate, WORKER_MAX_BYTES},
    native::RecordingPayload,
};
use omniusong_store::NativeStore;
pub use omniusong_store::{
    ArchiveInspection, ArchiveReceipt, CaptureInspection, CaptureRecoveryReceipt, CaptureStageScan,
    ProjectSummary, RestoreCopyIntent,
};
pub use performance_intents::{
    PerformanceFinishIntent, PerformanceIntent, PerformanceIntentReceipt,
};
pub use performance_source_actions::{
    MAX_SOURCE_ACTION_DETAIL_BYTES, PerformanceSourceActionDetail, PerformanceSourceActionKind,
    PerformanceSourceActionReference, PerformanceSourceActionState,
    PerformanceSourceAttachmentChoice, PerformanceSourceAttachmentReceipt,
    PerformanceSourceAttachmentState, PerformanceSourceAttachmentStatus, PerformanceSourceDecision,
    PerformanceSourceDecisionEvent, PerformanceSourceDecisionReceipt,
    PerformanceSourceDecisionRevision, PerformanceSourceDecisionState,
    PerformanceSourceExecutionPhase, PerformanceSourceExecutionReceipt,
    PerformanceSourceExecutionRevision, PerformanceSourceIntentReference,
    PreparedPerformanceSourceAction, PreparedPerformanceSourceActionReceipt,
    RecoveredPerformanceSourceResult, attachment_operation_id, validate_performance_source_reply,
};
pub use performance_sources::{PerformanceSource, PerformanceSourceReview, PerformanceSourceScan};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub native: PathBuf,
    pub legacy: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyProject {
    pub path: PathBuf,
    pub name: String,
    pub tracks: usize,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyScan {
    pub projects: Vec<LegacyProject>,
    pub skipped: usize,
    pub limited: bool,
}
#[derive(Debug, Clone, Default)]
pub struct Library {
    /// Established by the storage worker after an actual open, never inferred by paint.
    pub native_root: Option<PathBuf>,
    pub sessions: Vec<ProjectSummary>,
    pub legacy: LegacyScan,
    pub legacy_notice: Option<String>,
}

/// Prepared only by a backend/storage worker. No full-document parsing in paint.
#[derive(Debug)]
pub struct DocumentView {
    pub document: ProjectDocument,
    pub recordings: Vec<Option<RecordingPayload>>,
    pub provenance: Vec<Option<String>>,
}
impl DocumentView {
    pub fn prepare(document: ProjectDocument) -> Result<Self, ServiceError> {
        let mut recordings = Vec::with_capacity(document.tracks().len());
        let mut provenance = Vec::with_capacity(document.tracks().len());
        for track in document.tracks() {
            if document.supports_native_takes() && track["kind"] == "recording" {
                let payload: RecordingPayload = serde_json::from_value(track["recording"].clone())
                    .map_err(|e| ServiceError::rejected(e.to_string()))?;
                payload
                    .validate()
                    .map_err(|e| ServiceError::rejected(e.to_string()))?;
                recordings.push(Some(payload));
                provenance.push(None);
            } else {
                recordings.push(None);
                provenance.push(track.get("provenance").map(|value| {
                    let text = value.to_string();
                    let mut preview: String = text.chars().take(4096).collect();
                    if preview.len() < text.len() {
                        preview.push_str("… (full provenance retained)");
                    }
                    preview
                }));
            }
        }
        Ok(Self {
            document,
            recordings,
            provenance,
        })
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    RunEvidenceJob {
        request: Box<EvidenceJobRequest>,
    },
    InspectRetainedEvidence {
        path: PathBuf,
    },
    ListEvidenceJobs,
    ReadEvidenceJob {
        job_id: String,
    },
    ReconcileEvidenceJob {
        job_id: String,
        expected_request_sha256: String,
        expected_history_sha256: String,
    },
    Initialize,
    Refresh,
    Create {
        name: String,
    },
    Open {
        project_id: ProjectId,
    },
    ImportLegacy {
        path: PathBuf,
    },
    Edit {
        intent: ProjectEditIntent,
    },
    AttachCapture {
        journal_path: PathBuf,
        publication: omniusong_store::TakePublication,
    },
    SavePerformanceIntent {
        expected_revision: Option<u64>,
        intent: Box<PerformanceIntent>,
    },
    ListPerformanceIntents,
    SavePreparedPerformanceSourceAction {
        expected_revision: Option<u64>,
        action: Box<PreparedPerformanceSourceAction>,
        review: Vec<u8>,
    },
    ListPreparedPerformanceSourceActions,
    ListPerformanceSourceActionStates,
    ReadPerformanceSourceAction {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    ExecutePerformanceSourceAction {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    ReconcilePerformanceSourceAction {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    SavePerformanceSourceDecision {
        id: String,
        revision: u64,
        payload_sha256: String,
        decision: Box<PerformanceSourceDecision>,
    },
    AttachPerformanceSource {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    ReconcilePerformanceSourceAttachment {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    AttachPerformance {
        id: omniusong_engine::LiveId,
        revision: u64,
    },
    ScanPerformanceSources,
    InspectPerformanceSource {
        stage_id: String,
    },
    ScanCaptures,
    InspectCapture {
        stage_id: String,
    },
    RecoverCapture {
        inspection: Box<CaptureInspection>,
        operation_id: OperationId,
    },
    ExportArchive {
        project_id: ProjectId,
        destination: PathBuf,
    },
    InspectArchive {
        path: PathBuf,
    },
    RestoreCopy {
        path: PathBuf,
        intent: RestoreCopyIntent,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    RunEvidenceJob,
    InspectRetainedEvidence,
    ListEvidenceJobs,
    ReadEvidenceJob,
    ReconcileEvidenceJob,
    Initialize,
    Refresh,
    Create,
    Open,
    ImportLegacy,
    Edit,
    AttachCapture,
    SavePerformanceIntent,
    ListPerformanceIntents,
    SavePreparedPerformanceSourceAction,
    ListPreparedPerformanceSourceActions,
    ListPerformanceSourceActionStates,
    ReadPerformanceSourceAction,
    ExecutePerformanceSourceAction,
    ReconcilePerformanceSourceAction,
    SavePerformanceSourceDecision,
    AttachPerformanceSource,
    ReconcilePerformanceSourceAttachment,
    AttachPerformance,
    ScanPerformanceSources,
    InspectPerformanceSource,
    ScanCaptures,
    InspectCapture,
    RecoverCapture,
    ExportArchive,
    InspectArchive,
    RestoreCopy,
}
impl Command {
    pub fn kind(&self) -> CommandKind {
        match self {
            Self::RunEvidenceJob { .. } => CommandKind::RunEvidenceJob,
            Self::InspectRetainedEvidence { .. } => CommandKind::InspectRetainedEvidence,
            Self::ListEvidenceJobs => CommandKind::ListEvidenceJobs,
            Self::ReadEvidenceJob { .. } => CommandKind::ReadEvidenceJob,
            Self::ReconcileEvidenceJob { .. } => CommandKind::ReconcileEvidenceJob,
            Self::Initialize => CommandKind::Initialize,
            Self::Refresh => CommandKind::Refresh,
            Self::Create { .. } => CommandKind::Create,
            Self::Open { .. } => CommandKind::Open,
            Self::ImportLegacy { .. } => CommandKind::ImportLegacy,
            Self::Edit { .. } => CommandKind::Edit,
            Self::AttachCapture { .. } => CommandKind::AttachCapture,
            Self::SavePerformanceIntent { .. } => CommandKind::SavePerformanceIntent,
            Self::ListPerformanceIntents => CommandKind::ListPerformanceIntents,
            Self::SavePreparedPerformanceSourceAction { .. } => {
                CommandKind::SavePreparedPerformanceSourceAction
            }
            Self::ListPreparedPerformanceSourceActions => {
                CommandKind::ListPreparedPerformanceSourceActions
            }
            Self::ListPerformanceSourceActionStates => {
                CommandKind::ListPerformanceSourceActionStates
            }
            Self::ReadPerformanceSourceAction { .. } => CommandKind::ReadPerformanceSourceAction,
            Self::ExecutePerformanceSourceAction { .. } => {
                CommandKind::ExecutePerformanceSourceAction
            }
            Self::ReconcilePerformanceSourceAction { .. } => {
                CommandKind::ReconcilePerformanceSourceAction
            }
            Self::SavePerformanceSourceDecision { .. } => {
                CommandKind::SavePerformanceSourceDecision
            }
            Self::AttachPerformanceSource { .. } => CommandKind::AttachPerformanceSource,
            Self::ReconcilePerformanceSourceAttachment { .. } => {
                CommandKind::ReconcilePerformanceSourceAttachment
            }
            Self::AttachPerformance { .. } => CommandKind::AttachPerformance,
            Self::ScanPerformanceSources => CommandKind::ScanPerformanceSources,
            Self::InspectPerformanceSource { .. } => CommandKind::InspectPerformanceSource,
            Self::ScanCaptures => CommandKind::ScanCaptures,
            Self::InspectCapture { .. } => CommandKind::InspectCapture,
            Self::RecoverCapture { .. } => CommandKind::RecoverCapture,
            Self::ExportArchive { .. } => CommandKind::ExportArchive,
            Self::InspectArchive { .. } => CommandKind::InspectArchive,
            Self::RestoreCopy { .. } => CommandKind::RestoreCopy,
        }
    }
    pub fn retry_safe(&self) -> bool {
        !matches!(
            self,
            Self::Create { .. }
                | Self::ImportLegacy { .. }
                | Self::ExportArchive { .. }
                | Self::RunEvidenceJob { .. }
                | Self::ReconcileEvidenceJob { .. }
        )
    }
    pub fn mutates(&self) -> bool {
        matches!(
            self,
            Self::RunEvidenceJob { .. }
                | Self::Create { .. }
                | Self::ImportLegacy { .. }
                | Self::Edit { .. }
                | Self::AttachCapture { .. }
                | Self::SavePerformanceIntent { .. }
                | Self::SavePreparedPerformanceSourceAction { .. }
                | Self::ExecutePerformanceSourceAction { .. }
                | Self::SavePerformanceSourceDecision { .. }
                | Self::AttachPerformanceSource { .. }
                | Self::AttachPerformance { .. }
                | Self::RecoverCapture { .. }
                | Self::ExportArchive { .. }
                | Self::RestoreCopy { .. }
        )
    }
    // Read-only reconciliation observes a prior durable execution. Losing its
    // worker/reply cannot establish that execution's outcome or roll it back.
    fn outcome_unknown_on_worker_failure(&self) -> bool {
        self.mutates()
            || matches!(
                self,
                Self::ReconcilePerformanceSourceAction { .. }
                    | Self::ReconcilePerformanceSourceAttachment { .. }
                    | Self::ReconcileEvidenceJob { .. }
            )
    }
    pub fn validate(&self) -> Result<(), ServiceError> {
        let path = |path: &Path| {
            if !path.as_os_str().is_empty() && path.as_os_str().len() <= 4096 {
                Ok(())
            } else {
                Err(ServiceError::rejected(
                    "Choose a nonempty path of at most 4096 bytes",
                ))
            }
        };
        let id = |id: &ProjectId| {
            id.validate()
                .map_err(|e| ServiceError::rejected(e.to_string()))
        };
        match self {
            Self::RunEvidenceJob { request } => request.validate(),
            Self::InspectRetainedEvidence { path } => evidence_jobs::path_text(path).map(|_| ()),
            Self::ListEvidenceJobs => Ok(()),
            Self::ReadEvidenceJob { job_id } => evidence_jobs::require(
                evidence_jobs::valid_id(job_id),
                "Choose an exact evidence job identity",
            ),
            Self::ReconcileEvidenceJob {
                job_id,
                expected_request_sha256,
                expected_history_sha256,
            } => evidence_jobs::require(
                evidence_jobs::valid_id(job_id)
                    && evidence_jobs::valid_sha(expected_request_sha256)
                    && evidence_jobs::valid_sha(expected_history_sha256),
                "Choose the exact evidence job/request/history fingerprints",
            ),
            Self::Initialize
            | Self::Refresh
            | Self::ScanCaptures
            | Self::ScanPerformanceSources
            | Self::ListPerformanceIntents
            | Self::ListPreparedPerformanceSourceActions
            | Self::ListPerformanceSourceActionStates => Ok(()),
            Self::ReadPerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            }
            | Self::ExecutePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            }
            | Self::ReconcilePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            }
            | Self::SavePerformanceSourceDecision {
                id,
                revision,
                payload_sha256,
                ..
            }
            | Self::AttachPerformanceSource {
                id,
                revision,
                payload_sha256,
            }
            | Self::ReconcilePerformanceSourceAttachment {
                id,
                revision,
                payload_sha256,
            } => {
                performance_sources::validate_stage_id(id)
                    .map_err(|e| ServiceError::rejected(e.message))?;
                if let Self::SavePerformanceSourceDecision { decision, .. } = self {
                    decision.validate()?;
                }
                if (1..=16).contains(revision)
                    && payload_sha256.len() == 64
                    && payload_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    Ok(())
                } else {
                    Err(ServiceError::rejected(
                        "Choose an exact retained source-action revision and SHA-256",
                    ))
                }
            }
            Self::SavePreparedPerformanceSourceAction { action, review, .. } => {
                action.validate()?;
                if review.is_empty()
                    || review.len() > omniusong_store::MAX_PERFORMANCE_SOURCE_REVIEW_BYTES
                {
                    Err(ServiceError::rejected(
                        "Retain an exact source review of at most64KiB",
                    ))
                } else {
                    Ok(())
                }
            }
            Self::InspectPerformanceSource { stage_id } => {
                performance_sources::validate_stage_id(stage_id)
            }
            Self::SavePerformanceIntent { intent, .. } => intent.validate(),
            Self::AttachPerformance { id, revision } => {
                if id.is_valid() && (1..=16).contains(revision) {
                    Ok(())
                } else {
                    Err(ServiceError::rejected(
                        "Choose a valid exact durable performance intention revision",
                    ))
                }
            }
            Self::InspectCapture { stage_id } => validate_stage_id(stage_id),
            Self::RecoverCapture {
                inspection,
                operation_id,
            } => {
                validate_stage_id(&inspection.stage_id)?;
                operation_id
                    .validate()
                    .map_err(|error| ServiceError::rejected(error.to_string()))?;
                struct Budget(usize);
                impl std::io::Write for Budget {
                    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                        if bytes.len() > WORKER_MAX_BYTES.saturating_sub(self.0) {
                            return Err(std::io::Error::other(
                                "Capture review exceeds request limit",
                            ));
                        }
                        self.0 += bytes.len();
                        Ok(bytes.len())
                    }
                    fn flush(&mut self) -> std::io::Result<()> {
                        Ok(())
                    }
                }
                serde_json::to_writer(Budget(0), inspection)
                    .map_err(|error| ServiceError::rejected(error.to_string()))
            }
            Self::Create { name } => {
                if !name.trim().is_empty() && name.chars().count() <= 160 {
                    Ok(())
                } else {
                    Err(ServiceError::rejected(
                        "Session name must contain 1–160 characters",
                    ))
                }
            }
            Self::Open { project_id } => id(project_id),
            Self::AttachCapture {
                journal_path,
                publication,
            } => {
                path(journal_path)?;
                publication
                    .validate()
                    .map_err(|e| ServiceError::rejected(e.to_string()))
            }
            Self::ImportLegacy { path: p } | Self::InspectArchive { path: p } => path(p),
            Self::Edit { intent } => {
                intent
                    .validate()
                    .map_err(|e| ServiceError::rejected(e.to_string()))?;
                struct Budget(usize);
                impl std::io::Write for Budget {
                    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                        if bytes.len() > WORKER_MAX_BYTES.saturating_sub(self.0) {
                            return Err(std::io::Error::other(
                                "Session edit exceeds request limit",
                            ));
                        }
                        self.0 += bytes.len();
                        Ok(bytes.len())
                    }
                    fn flush(&mut self) -> std::io::Result<()> {
                        Ok(())
                    }
                }
                serde_json::to_writer(Budget(0), intent)
                    .map_err(|e| ServiceError::rejected(e.to_string()))
            }
            Self::ExportArchive {
                project_id,
                destination,
            } => {
                id(project_id)?;
                path(destination)
            }
            Self::RestoreCopy { path: p, intent } => {
                path(p)?;
                intent
                    .operation_id
                    .validate()
                    .map_err(|e| ServiceError::rejected(e.to_string()))?;
                if intent.expected_archive_sha256.len() == 64
                    && intent
                        .expected_archive_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    Ok(())
                } else {
                    Err(ServiceError::rejected(
                        "Inspect this backup before restoring it",
                    ))
                }
            }
        }
    }
}

fn validate_stage_id(id: &str) -> Result<(), ServiceError> {
    let valid = uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id);
    if valid {
        Ok(())
    } else {
        Err(ServiceError::rejected(
            "Choose a retained capture from the owned stage list",
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId {
    pub generation: u64,
    pub sequence: u64,
}
#[derive(Debug, Clone)]
pub struct Request {
    pub id: RequestId,
    pub command: Command,
}
#[derive(Debug)]
pub enum Reply {
    EvidenceJobOutcome(Box<EvidenceJobReply>),
    RetainedEvidenceInspected(Box<omniusong_store::EvidenceArchiveInspection>),
    EvidenceJobs(EvidenceJobList),
    EvidenceJob(Box<EvidenceJobView>),
    Library(Library),
    Document {
        view: DocumentView,
        library: Library,
        replayed: bool,
    },
    Exported(ArchiveReceipt),
    Inspected(ArchiveInspection),
    CaptureStages(CaptureStageScan),
    CaptureInspected(Box<CaptureInspection>),
    CaptureRecovered(Box<CaptureRecoveryReceipt>),
    PerformanceIntent(Box<PerformanceIntentReceipt>),
    PerformanceIntents(Vec<PerformanceIntentReceipt>),
    PerformanceSources(PerformanceSourceScan),
    PerformanceSourceInspected(Box<PerformanceSourceReview>),
    PreparedPerformanceSourceAction(Box<PreparedPerformanceSourceActionReceipt>),
    PreparedPerformanceSourceActions(Vec<PreparedPerformanceSourceActionReceipt>),
    PerformanceSourceActionStates(Vec<PerformanceSourceActionState>),
    PerformanceSourceExecution(Box<PerformanceSourceExecutionReceipt>),
    PerformanceSourceActionDetail(Box<PerformanceSourceActionDetail>),
    PerformanceSourceDecision(Box<PerformanceSourceDecisionReceipt>),
    PerformanceSourceAttachment(Box<PerformanceSourceAttachmentReceipt>),
}
impl Reply {
    fn matches_command(&self, command: &Command) -> bool {
        if !self.matches(command.kind()) {
            return false;
        }
        if let Some(matches) = performance_source_actions::reply_matches(self, command) {
            return matches;
        }
        match (self, command) {
            (Self::EvidenceJobOutcome(reply), Command::RunEvidenceJob { request }) => {
                reply.job_id == request.job_id
                    && request
                        .fingerprint()
                        .is_ok_and(|sha| sha == reply.request_sha256)
                    && reply
                        .history_sha256
                        .as_ref()
                        .is_none_or(|sha| evidence_jobs::valid_sha(sha))
                    && reply
                        .outcome
                        .as_ref()
                        .is_none_or(|outcome| outcome.validate_for(request).is_ok())
                    && reply
                        .metadata_problem
                        .as_ref()
                        .is_none_or(|problem| problem.validate().is_ok())
                    && reply
                        .catalog_retirement
                        .as_ref()
                        .is_none_or(|error| error.kind == FailureKind::StorageUnavailable)
            }
            (
                Self::EvidenceJobOutcome(reply),
                Command::ReconcileEvidenceJob {
                    job_id,
                    expected_request_sha256,
                    ..
                },
            ) => {
                reply.job_id == *job_id
                    && reply.request_sha256 == *expected_request_sha256
                    && reply.outcome.as_ref().is_none_or(|outcome| {
                        evidence_jobs::check_bounded(
                            outcome,
                            evidence_jobs::MAX_EVIDENCE_OUTCOME_REPLY_BYTES,
                        )
                        .is_ok()
                    })
                    && reply
                        .history_sha256
                        .as_ref()
                        .is_none_or(|sha| evidence_jobs::valid_sha(sha))
                    && reply
                        .metadata_problem
                        .as_ref()
                        .is_none_or(|problem| problem.validate().is_ok())
                    && reply.catalog_retirement.is_none()
            }
            (Self::EvidenceJob(view), Command::ReadEvidenceJob { job_id }) => {
                view.job_id == *job_id
            }
            (
                Self::RetainedEvidenceInspected(inspection),
                Command::InspectRetainedEvidence { path },
            ) => inspection.receipt.path.as_os_str() == path.as_os_str(),
            _ => true,
        }
    }
    fn matches(&self, kind: CommandKind) -> bool {
        matches!(
            (self, kind),
            (
                Self::EvidenceJobOutcome(_),
                CommandKind::RunEvidenceJob | CommandKind::ReconcileEvidenceJob
            ) | (
                Self::RetainedEvidenceInspected(_),
                CommandKind::InspectRetainedEvidence
            ) | (Self::EvidenceJobs(_), CommandKind::ListEvidenceJobs)
                | (Self::EvidenceJob(_), CommandKind::ReadEvidenceJob)
                | (
                    Self::Library(_),
                    CommandKind::Initialize | CommandKind::Refresh
                )
                | (
                    Self::Document { .. },
                    CommandKind::Create
                        | CommandKind::Open
                        | CommandKind::ImportLegacy
                        | CommandKind::Edit
                        | CommandKind::AttachCapture
                        | CommandKind::AttachPerformance
                        | CommandKind::RestoreCopy
                )
                | (Self::Exported(_), CommandKind::ExportArchive)
                | (Self::Inspected(_), CommandKind::InspectArchive)
                | (Self::CaptureStages(_), CommandKind::ScanCaptures)
                | (Self::CaptureInspected(_), CommandKind::InspectCapture)
                | (Self::CaptureRecovered(_), CommandKind::RecoverCapture)
                | (
                    Self::PerformanceSources(_),
                    CommandKind::ScanPerformanceSources
                )
                | (
                    Self::PerformanceSourceInspected(_),
                    CommandKind::InspectPerformanceSource
                )
                | (
                    Self::PerformanceIntent(_),
                    CommandKind::SavePerformanceIntent
                )
                | (
                    Self::PerformanceIntents(_),
                    CommandKind::ListPerformanceIntents
                )
                | (
                    Self::PreparedPerformanceSourceAction(_),
                    CommandKind::SavePreparedPerformanceSourceAction
                )
                | (
                    Self::PreparedPerformanceSourceActions(_),
                    CommandKind::ListPreparedPerformanceSourceActions
                )
                | (
                    Self::PerformanceSourceActionStates(_),
                    CommandKind::ListPerformanceSourceActionStates
                )
                | (
                    Self::PerformanceSourceActionDetail(_),
                    CommandKind::ReadPerformanceSourceAction
                )
                | (
                    Self::PerformanceSourceDecision(_),
                    CommandKind::SavePerformanceSourceDecision
                )
                | (
                    Self::PerformanceSourceAttachment(_),
                    CommandKind::AttachPerformanceSource
                        | CommandKind::ReconcilePerformanceSourceAttachment
                )
                | (
                    Self::PerformanceSourceExecution(_),
                    CommandKind::ExecutePerformanceSourceAction
                        | CommandKind::ReconcilePerformanceSourceAction
                )
        )
    }
}
fn terminal_storage_failure(result: &Result<Reply, ServiceError>) -> Option<ServiceError> {
    let error = match result {
        Err(error) => Some(error),
        Ok(Reply::PerformanceSourceAttachment(receipt)) => receipt.current_document_error.as_ref(),
        Ok(Reply::EvidenceJobOutcome(receipt)) => receipt.catalog_retirement.as_ref(),
        _ => None,
    };
    error
        .filter(|e| e.kind == FailureKind::StorageUnavailable)
        .cloned()
}

#[derive(Debug)]
pub struct Completion {
    pub id: RequestId,
    pub kind: CommandKind,
    pub result: Result<Reply, ServiceError>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Rejected,
    /// A pure semantic edit was rejected before transaction publication.
    EditRejected,
    /// A new performance-intent semantic check rejected before any publication.
    IntentRejected,
    /// Bounded source work stopped. Interpret mutation/recovery state using the
    /// command and outcome_unknown; accepted execution may already be durable.
    InspectionCancelled,
    InspectionLimit,
    SourceBusy,
    SourceUnavailable,
    /// The project transaction rejected its expected revision and did not commit.
    RevisionConflict,
    Backend,
    Disconnected,
    Protocol,
    /// This generation's storage connection must never be reused.
    StorageUnavailable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceError {
    pub kind: FailureKind,
    pub message: String,
    pub outcome_unknown: bool,
}
impl ServiceError {
    pub fn rejected(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Rejected,
            message: message.into(),
            outcome_unknown: false,
        }
    }
    pub fn backend(message: impl Into<String>, outcome_unknown: bool) -> Self {
        Self {
            kind: FailureKind::Backend,
            message: message.into(),
            outcome_unknown,
        }
    }
    pub fn storage_unavailable(message: impl Into<String>, outcome_unknown: bool) -> Self {
        Self {
            kind: FailureKind::StorageUnavailable,
            message: message.into(),
            outcome_unknown,
        }
    }
}

// SQLite's stable primary result byte, verified against libsqlite3-sys 0.38.2:
// SQLITE_IOERR=10, SQLITE_CORRUPT=11, SQLITE_NOTADB=26. sqlite_error() returns
// Some only for rusqlite::Error::SqliteFailure; no string/ordinary-I/O heuristics.
fn fatal_sqlite_primary(extended_code: i32) -> bool {
    matches!(extended_code & 0xff, 10 | 11 | 26)
}
fn store_failure(error: omniusong_store::StoreError, outcome_unknown: bool) -> ServiceError {
    if matches!(error, omniusong_store::StoreError::EditRejected(_)) {
        return ServiceError {
            kind: FailureKind::EditRejected,
            message: error.to_string(),
            outcome_unknown: false,
        };
    }
    if matches!(error, omniusong_store::StoreError::Conflict) {
        return ServiceError {
            kind: FailureKind::RevisionConflict,
            message: error.to_string(),
            outcome_unknown: false,
        };
    }
    let fatal = matches!(&error,omniusong_store::StoreError::Database(error)
        if error.sqlite_error().is_some_and(|code|fatal_sqlite_primary(code.extended_code)));
    if fatal {
        ServiceError::storage_unavailable(
            format!(
                "Session storage is unavailable: {error}. This worker was stopped. Reconnect explicitly after checking the storage location; no repair was attempted."
            ),
            outcome_unknown,
        )
    } else {
        ServiceError::backend(error.to_string(), outcome_unknown)
    }
}
impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ServiceError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitFailure {
    Busy,
    Full,
    Disconnected,
    Invalid,
}
#[derive(Debug)]
pub struct SubmitError {
    pub kind: SubmitFailure,
    pub command: Command,
    pub message: String,
}

/// A backend exists only on its owning worker. Test injection is opt-in.
pub trait Backend: Send + 'static {
    /// Compatible entry point for the evidence token; older injected backends
    /// retain their established inspection/source-action handling.
    fn handle_with_evidence_cancellation(
        &mut self,
        command: &Command,
        inspection_cancelled: &Arc<AtomicBool>,
        source_action_cancelled: &Arc<AtomicBool>,
        _evidence_cancelled: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.handle_with_cancellations(command, inspection_cancelled, source_action_cancelled)
    }
    fn handle(&mut self, command: &Command) -> Result<Reply, ServiceError>;
    fn handle_with_cancel(
        &mut self,
        command: &Command,
        _cancelled: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.handle(command)
    }
    fn handle_with_cancellations(
        &mut self,
        command: &Command,
        inspection_cancelled: &Arc<AtomicBool>,
        _source_action_cancelled: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        if matches!(
            command,
            Command::ExecutePerformanceSourceAction { .. }
                | Command::ReconcilePerformanceSourceAction { .. }
                | Command::AttachPerformanceSource { .. }
                | Command::ReconcilePerformanceSourceAttachment { .. }
                | Command::SavePerformanceSourceDecision { .. }
        ) {
            self.handle(command)
        } else {
            self.handle_with_cancel(command, inspection_cancelled)
        }
    }
}

pub struct SessionService {
    requests: SyncSender<Request>,
    responses: Receiver<Completion>,
    generation: u64,
    next_sequence: u64,
    pending: Option<Request>,
    retry: Option<Command>,
    disconnected: bool,
    terminal_failure: Option<ServiceError>,
    inspection_cancelled: Arc<AtomicBool>,
    source_action_cancelled: Arc<AtomicBool>,
    evidence_cancelled: Arc<AtomicBool>,
}
static GENERATION: AtomicU64 = AtomicU64::new(1);
impl SessionService {
    fn channels() -> (Self, Receiver<Request>, SyncSender<Completion>) {
        let (sender, requests) = mpsc::sync_channel(1);
        let (responses, receiver) = mpsc::sync_channel(1);
        (
            Self {
                requests: sender,
                responses: receiver,
                generation: GENERATION.fetch_add(1, Ordering::Relaxed),
                next_sequence: 1,
                pending: None,
                retry: None,
                disconnected: false,
                terminal_failure: None,
                inspection_cancelled: Arc::new(AtomicBool::new(false)),
                source_action_cancelled: Arc::new(AtomicBool::new(false)),
                evidence_cancelled: Arc::new(AtomicBool::new(false)),
            },
            requests,
            responses,
        )
    }
    pub fn spawn(paths: Paths) -> Result<Self, ServiceError> {
        Self::spawn_internal(NativeBackend {
            paths,
            store: None,
            legacy: LegacyScan::default(),
            legacy_notice: None,
            require_existing: false,
        })
    }
    /// Explicit fresh generation after failure; never creates an empty replacement.
    pub fn reconnect(paths: Paths) -> Result<Self, ServiceError> {
        Self::spawn_internal(NativeBackend {
            paths,
            store: None,
            legacy: LegacyScan::default(),
            legacy_notice: None,
            require_existing: true,
        })
    }
    fn spawn_internal(mut backend: impl Backend) -> Result<Self, ServiceError> {
        let (client, requests, responses) = Self::channels();
        let inspection_cancelled = Arc::clone(&client.inspection_cancelled);
        let source_action_cancelled = Arc::clone(&client.source_action_cancelled);
        let evidence_cancelled = Arc::clone(&client.evidence_cancelled);
        std::thread::Builder::new()
            .name("omniusong-sessions".into())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    let kind = request.command.kind();
                    let result = backend.handle_with_evidence_cancellation(
                        &request.command,
                        &inspection_cancelled,
                        &source_action_cancelled,
                        &evidence_cancelled,
                    );
                    let terminal = terminal_storage_failure(&result).is_some();
                    if responses
                        .try_send(Completion {
                            id: request.id,
                            kind,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    // The bounded known failure is now queued. Retire this
                    // backend on its worker; never dequeue more work from it.
                    if terminal {
                        break;
                    }
                }
            })
            .map_err(|e| {
                ServiceError::backend(format!("Could not start session worker: {e}"), false)
            })?;
        Ok(client)
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }
    pub fn pending(&self) -> Option<&Request> {
        self.pending.as_ref()
    }
    pub fn retry_command(&self) -> Option<&Command> {
        self.retry.as_ref()
    }
    /// Cancellation retains the pending request and worker credit until its
    /// exact completion is polled. A stale identity cannot cancel newer work.
    pub fn cancel_source_inspection(&self, id: RequestId) -> bool {
        if self.pending.as_ref().is_some_and(|request| {
            request.id == id && matches!(request.command, Command::InspectPerformanceSource { .. })
        }) {
            self.inspection_cancelled.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
    /// A distinct token for copy execution/reconciliation. Cancellation retains
    /// credit until the exact result arrives and cannot promise rollback.
    pub fn cancel_source_action(&self, id: RequestId) -> bool {
        if self.pending.as_ref().is_some_and(|request| {
            request.id == id
                && matches!(
                    request.command,
                    Command::ExecutePerformanceSourceAction { .. }
                        | Command::ReconcilePerformanceSourceAction { .. }
                        | Command::AttachPerformanceSource { .. }
                        | Command::ReconcilePerformanceSourceAttachment { .. }
                )
        }) {
            self.source_action_cancelled.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
    /// Exact evidence cancellation requests cessation, never rollback. Credit is
    /// retained until this request completes or the worker actually disconnects.
    pub fn cancel_evidence_job(&self, id: RequestId) -> bool {
        if self.pending.as_ref().is_some_and(|request| {
            request.id == id
                && matches!(
                    request.command,
                    Command::RunEvidenceJob { .. }
                        | Command::InspectRetainedEvidence { .. }
                        | Command::ReconcileEvidenceJob { .. }
                )
        }) {
            self.evidence_cancelled.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
    pub fn try_submit(&mut self, command: Command) -> Result<RequestId, Box<SubmitError>> {
        let fail = |kind, message: &str, command| {
            Box::new(SubmitError {
                kind,
                command,
                message: message.into(),
            })
        };
        if self.disconnected {
            return Err(fail(
                SubmitFailure::Disconnected,
                "Session worker is disconnected",
                command,
            ));
        }
        if self.is_busy() {
            return Err(fail(
                SubmitFailure::Busy,
                "Another action is still running",
                command,
            ));
        }
        if let Err(error) = command.validate() {
            return Err(Box::new(SubmitError {
                kind: SubmitFailure::Invalid,
                command,
                message: error.message,
            }));
        }
        let Some(next) = self.next_sequence.checked_add(1) else {
            return Err(fail(
                SubmitFailure::Invalid,
                "Session request identity exhausted",
                command,
            ));
        };
        let id = RequestId {
            generation: self.generation,
            sequence: self.next_sequence,
        };
        let request = Request { id, command };
        // No worker is active while credit is free. Reset before publishing
        // the next accepted command; a fast consumer sees the new token state.
        self.inspection_cancelled.store(false, Ordering::Release);
        self.source_action_cancelled.store(false, Ordering::Release);
        self.evidence_cancelled.store(false, Ordering::Release);
        match self.requests.try_send(request.clone()) {
            Ok(()) => {
                self.next_sequence = next;
                self.pending = Some(request);
                Ok(id)
            }
            Err(TrySendError::Full(request)) => Err(fail(
                SubmitFailure::Full,
                "Session request queue is full",
                request.command,
            )),
            Err(TrySendError::Disconnected(request)) => {
                self.disconnected = true;
                Err(fail(
                    SubmitFailure::Disconnected,
                    "Session worker is disconnected",
                    request.command,
                ))
            }
        }
    }
    pub fn try_poll(&mut self) -> Result<Option<Completion>, ServiceError> {
        if self.disconnected {
            if let Some(error) = &self.terminal_failure {
                return Err(error.clone());
            }
            return Err(ServiceError {
                kind: FailureKind::Disconnected,
                message: "Session worker is disconnected; reconnect explicitly before retrying."
                    .into(),
                outcome_unknown: self
                    .retry
                    .as_ref()
                    .is_some_and(Command::outcome_unknown_on_worker_failure),
            });
        }
        match self.responses.try_recv() {
            Ok(completion) => {
                let valid = self.pending.as_ref().is_some_and(|request| {
                    request.id == completion.id
                        && request.command.kind() == completion.kind
                        && match &completion.result {
                            Ok(reply) => reply.matches_command(&request.command),
                            Err(_) => true,
                        }
                });
                if !valid {
                    return Err(ServiceError {
                        kind: FailureKind::Protocol,
                        message: "Ignored a stale or mismatched session completion".into(),
                        outcome_unknown: self.pending.as_ref().is_some_and(|request| {
                            request.command.outcome_unknown_on_worker_failure()
                        }),
                    });
                }
                let request = self.pending.take().expect("validated pending request");
                self.retry = if completion.result.is_err() && request.command.retry_safe() {
                    Some(request.command)
                } else {
                    None
                };
                if let Some(error) = terminal_storage_failure(&completion.result) {
                    self.disconnected = true;
                    self.terminal_failure = Some(error);
                }
                Ok(Some(completion))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.disconnected = true;
                let request = self.pending.take();
                let unknown = request
                    .as_ref()
                    .is_some_and(|request| request.command.outcome_unknown_on_worker_failure());
                self.retry = request
                    .filter(|request| request.command.retry_safe())
                    .map(|request| request.command);
                Err(ServiceError{kind:FailureKind::Disconnected,message:"Session worker stopped. Accepted work may have completed; no automatic retry was started.".into(),outcome_unknown:unknown})
            }
        }
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn spawn_with_backend(backend: impl Backend) -> Result<Self, ServiceError> {
        Self::spawn_internal(backend)
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn test_pair() -> (Self, TestPeer) {
        let (client, requests, responses) = Self::channels();
        (
            client,
            TestPeer {
                requests,
                responses,
            },
        )
    }
}

impl Drop for SessionService {
    fn drop(&mut self) {
        self.inspection_cancelled.store(true, Ordering::Release);
        if self.pending.as_ref().is_some_and(|request| {
            matches!(request.command, Command::InspectRetainedEvidence { .. })
        }) {
            self.evidence_cancelled.store(true, Ordering::Release);
        }
    }
}
// No JoinHandle: closing a caller never blocks on storage. Drop cancels only
// read-only inspection; an accepted execution retains its distinct token.
#[cfg(any(test, feature = "test-hooks"))]
pub struct TestPeer {
    pub requests: Receiver<Request>,
    pub responses: SyncSender<Completion>,
}

pub fn scan_legacy(root: &Path) -> Result<LegacyScan, ServiceError> {
    let read = || -> std::io::Result<LegacyScan> {
        let directory = root.join("projects");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LegacyScan::default()),
            Err(e) => return Err(e),
        };
        let mut scan = LegacyScan::default();
        for (index, entry) in entries.enumerate() {
            if index >= 512 {
                scan.limited = true;
                break;
            }
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().is_none_or(|ext| ext != "json")
            {
                continue;
            }
            let mut bytes = Vec::new();
            File::open(entry.path())?
                .take(MAX_PROJECT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            let document = if bytes.len() <= MAX_PROJECT_BYTES {
                ProjectDocument::from_json(&bytes)
                    .ok()
                    .filter(|doc| doc.schema_version() == 1)
            } else {
                None
            };
            if let Some(doc) = document {
                scan.projects.push(LegacyProject {
                    path: entry.path(),
                    name: doc.name().into(),
                    tracks: doc.tracks().len(),
                });
            } else {
                scan.skipped += 1;
            }
        }
        scan.projects
            .sort_by_key(|project| project.name.to_lowercase());
        Ok(scan)
    };
    read().map_err(|e| ServiceError::backend(format!("Could not read legacy sources: {e}"), false))
}

struct NativeBackend {
    paths: Paths,
    store: Option<NativeStore>,
    legacy: LegacyScan,
    legacy_notice: Option<String>,
    require_existing: bool,
}
impl NativeBackend {
    fn scan(&mut self) {
        match scan_legacy(&self.paths.legacy) {
            Ok(scan) => {
                self.legacy_notice = if scan.skipped > 0 || scan.limited {
                    Some(format!(
                        "{} unsupported or damaged source(s) skipped.{} Originals were not changed.",
                        scan.skipped,
                        if scan.limited {
                            " Directory scan limited to 512 entries."
                        } else {
                            ""
                        }
                    ))
                } else {
                    None
                };
                self.legacy = scan;
            }
            Err(error) => {
                self.legacy = LegacyScan::default();
                self.legacy_notice = Some(error.message);
            }
        }
    }
    fn store(&mut self) -> Result<&mut NativeStore, ServiceError> {
        self.store
            .as_mut()
            .ok_or_else(|| ServiceError::rejected("Open session storage first"))
    }
    fn library(&mut self) -> Result<Library, ServiceError> {
        let mut sessions = self.store()?.list().map_err(|e| store_failure(e, false))?;
        sessions.truncate(512);
        Ok(Library {
            native_root: Some(self.store()?.root().to_owned()),
            sessions,
            legacy: self.legacy.clone(),
            legacy_notice: self.legacy_notice.clone(),
        })
    }
    fn document(
        &mut self,
        document: ProjectDocument,
        replayed: bool,
    ) -> Result<Reply, ServiceError> {
        Ok(Reply::Document {
            view: DocumentView::prepare(document)?,
            library: self.library()?,
            replayed,
        })
    }
}
impl Backend for NativeBackend {
    fn handle_with_evidence_cancellation(
        &mut self,
        command: &Command,
        inspection_cancelled: &Arc<AtomicBool>,
        source_action_cancelled: &Arc<AtomicBool>,
        evidence_cancelled: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        command.validate()?;
        match command {
            Command::RunEvidenceJob { request } => {
                self.run_evidence_job(request, Arc::clone(evidence_cancelled))
            }
            Command::InspectRetainedEvidence { path } => {
                self.inspect_retained_evidence(path, Arc::clone(evidence_cancelled))
            }
            Command::ListEvidenceJobs => self.list_evidence_jobs(Arc::clone(evidence_cancelled)),
            Command::ReadEvidenceJob { job_id } => {
                self.read_evidence_job(job_id, Arc::clone(evidence_cancelled))
            }
            Command::ReconcileEvidenceJob {
                job_id,
                expected_request_sha256,
                expected_history_sha256,
            } => self.reconcile_evidence_job(
                job_id,
                expected_request_sha256,
                expected_history_sha256,
                Arc::clone(evidence_cancelled),
            ),
            _ => self.handle_with_cancellations(
                command,
                inspection_cancelled,
                source_action_cancelled,
            ),
        }
    }
    fn handle_with_cancellations(
        &mut self,
        command: &Command,
        inspection_cancelled: &Arc<AtomicBool>,
        source_action_cancelled: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        command.validate()?;
        match command {
            Command::ExecutePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            } => self.execute_source_action(
                id,
                *revision,
                payload_sha256,
                Arc::clone(source_action_cancelled),
            ),
            Command::ReconcilePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            } => self.reconcile_source_action(
                id,
                *revision,
                payload_sha256,
                Arc::clone(source_action_cancelled),
            ),
            Command::AttachPerformanceSource {
                id,
                revision,
                payload_sha256,
            } => self.attach_source_choice(
                id,
                *revision,
                payload_sha256,
                Arc::clone(source_action_cancelled),
            ),
            Command::ReconcilePerformanceSourceAttachment {
                id,
                revision,
                payload_sha256,
            } => self.reconcile_source_attachment(
                id,
                *revision,
                payload_sha256,
                Arc::clone(source_action_cancelled),
            ),
            _ => self.handle_with_cancel(command, inspection_cancelled),
        }
    }
    fn handle_with_cancel(
        &mut self,
        command: &Command,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        if let Command::InspectPerformanceSource { stage_id } = command {
            command.validate()?;
            self.inspect_performance_source(stage_id, Arc::clone(cancelled))
        } else {
            self.handle(command)
        }
    }
    fn handle(&mut self, command: &Command) -> Result<Reply, ServiceError> {
        command.validate()?;
        let work = |error: omniusong_store::StoreError| store_failure(error, command.mutates());
        let result = (|| match command {
            Command::RunEvidenceJob { request } => {
                self.run_evidence_job(request, Arc::new(AtomicBool::new(false)))
            }
            Command::InspectRetainedEvidence { path } => {
                self.inspect_retained_evidence(path, Arc::new(AtomicBool::new(false)))
            }
            Command::ListEvidenceJobs => self.list_evidence_jobs(Arc::new(AtomicBool::new(false))),
            Command::ReadEvidenceJob { job_id } => {
                self.read_evidence_job(job_id, Arc::new(AtomicBool::new(false)))
            }
            Command::ReconcileEvidenceJob {
                job_id,
                expected_request_sha256,
                expected_history_sha256,
            } => self.reconcile_evidence_job(
                job_id,
                expected_request_sha256,
                expected_history_sha256,
                Arc::new(AtomicBool::new(false)),
            ),
            Command::Initialize => {
                if self.store.is_none() {
                    if self.require_existing {
                        let path = self.paths.native.join("studio.sqlite3");
                        match fs::symlink_metadata(&path) {
                            Ok(metadata)
                                if metadata.is_file() && !metadata.file_type().is_symlink() => {}
                            Ok(_) => {
                                return Err(ServiceError::storage_unavailable(
                                    "The existing session storage is not an ordinary file. No replacement store was created.",
                                    false,
                                ));
                            }
                            Err(error) => {
                                return Err(ServiceError::storage_unavailable(
                                    format!(
                                        "Could not reopen existing session storage: {error}. No replacement store was created."
                                    ),
                                    false,
                                ));
                            }
                        }
                    }
                    let opened = if self.require_existing {
                        NativeStore::open_existing(&self.paths.native)
                    } else {
                        NativeStore::open(&self.paths.native)
                    };
                    let store = opened.map_err(work)?;
                    self.paths.native = store.root().to_owned();
                    self.store = Some(store);
                }
                self.scan();
                Ok(Reply::Library(self.library()?))
            }
            Command::Refresh => {
                self.scan();
                Ok(Reply::Library(self.library()?))
            }
            Command::Create { name } => {
                let document = ProjectDocument::new(name)
                    .map_err(|e| ServiceError::rejected(e.to_string()))?;
                let document = self.store()?.create(&document).map_err(work)?;
                self.document(document, false)
            }
            Command::Open { project_id } => {
                let document = self.store()?.load(&project_id.0).map_err(work)?;
                self.document(document, false)
            }
            Command::ImportLegacy { path } => {
                let assets = self.paths.legacy.join("assets");
                let document = self.store()?.import_legacy(path, &assets).map_err(work)?;
                self.document(document, false)
            }
            Command::Edit { intent } => {
                let receipt = self.store()?.apply_edit(intent).map_err(work)?;
                let replayed = receipt.replayed;
                let document = if replayed {
                    self.store()?.load(&intent.project_id.0).map_err(work)?
                } else {
                    receipt.document
                };
                self.document(document, replayed)
            }
            Command::AttachCapture {
                journal_path,
                publication,
            } => {
                let receipt = self
                    .store()?
                    .attach_capture(journal_path, publication)
                    .map_err(work)?;
                let replayed = receipt.replayed;
                let document = if replayed {
                    self.store()?
                        .load(&publication.project_id.0)
                        .map_err(work)?
                } else {
                    receipt.document
                };
                self.document(document, replayed)
            }
            Command::SavePerformanceIntent {
                expected_revision,
                intent,
            } => self
                .save_performance_intention(*expected_revision, intent)
                .map_err(performance_intents::intent_rejection),
            Command::ListPerformanceIntents => Ok(Reply::PerformanceIntents(
                self.list_performance_intentions()?,
            )),
            Command::SavePreparedPerformanceSourceAction {
                expected_revision,
                action,
                review,
            } => self
                .save_prepared_source_action(*expected_revision, action, review)
                .map_err(performance_intents::intent_rejection),
            Command::ListPreparedPerformanceSourceActions => self.list_prepared_source_actions(),
            Command::ListPerformanceSourceActionStates => self.list_source_action_states(),
            Command::ReadPerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            } => self.read_source_action(id, *revision, payload_sha256),
            Command::ExecutePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            } => self.execute_source_action(
                id,
                *revision,
                payload_sha256,
                Arc::new(AtomicBool::new(false)),
            ),
            Command::ReconcilePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            } => self.reconcile_source_action(
                id,
                *revision,
                payload_sha256,
                Arc::new(AtomicBool::new(false)),
            ),
            Command::SavePerformanceSourceDecision {
                id,
                revision,
                payload_sha256,
                decision,
            } => self.save_source_decision(id, *revision, payload_sha256, decision),
            Command::AttachPerformanceSource {
                id,
                revision,
                payload_sha256,
            } => self.attach_source_choice(
                id,
                *revision,
                payload_sha256,
                Arc::new(AtomicBool::new(false)),
            ),
            Command::ReconcilePerformanceSourceAttachment {
                id,
                revision,
                payload_sha256,
            } => self.reconcile_source_attachment(
                id,
                *revision,
                payload_sha256,
                Arc::new(AtomicBool::new(false)),
            ),
            Command::AttachPerformance { id, revision } => {
                self.attach_performance_intention(*id, *revision)
            }
            Command::ScanPerformanceSources => self.scan_performance_sources(),
            Command::InspectPerformanceSource { stage_id } => {
                self.inspect_performance_source(stage_id, Arc::new(AtomicBool::new(false)))
            }
            Command::ScanCaptures => Ok(Reply::CaptureStages(self.store()?.scan_captures())),
            Command::InspectCapture { stage_id } => Ok(Reply::CaptureInspected(Box::new(
                self.store()?.inspect_capture(stage_id).map_err(work)?,
            ))),
            Command::RecoverCapture {
                inspection,
                operation_id,
            } => Ok(Reply::CaptureRecovered(Box::new(
                self.store()?
                    .recover_capture(inspection, &operation_id.0)
                    .map_err(work)?,
            ))),
            Command::ExportArchive {
                project_id,
                destination,
            } => Ok(Reply::Exported(
                self.store()?
                    .export_archive(&project_id.0, destination)
                    .map_err(work)?,
            )),
            Command::InspectArchive { path } => Ok(Reply::Inspected(
                self.store()?.inspect_archive(path).map_err(work)?,
            )),
            Command::RestoreCopy { path, intent } => {
                let receipt = self
                    .store()?
                    .restore_archive_as_copy(path, intent)
                    .map_err(work)?;
                let replayed = receipt.replayed;
                let document = if replayed {
                    self.store()?.load(receipt.document.id()).map_err(work)?
                } else {
                    receipt.document
                };
                self.document(document, replayed)
            }
        })();
        result.map_err(|mut error: ServiceError| {
            if command.mutates()
                && !matches!(
                    command,
                    Command::ExecutePerformanceSourceAction { .. }
                        | Command::SavePerformanceSourceDecision { .. }
                        | Command::AttachPerformanceSource { .. }
                        | Command::RunEvidenceJob { .. }
                )
                && !matches!(
                    error.kind,
                    FailureKind::RevisionConflict
                        | FailureKind::EditRejected
                        | FailureKind::IntentRejected
                )
            {
                error.outcome_unknown = true;
            }
            error
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::contracts::{EditOperation, EditScope, OperationId};
    use std::time::{Duration, Instant};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/rust-migration/session-service-tests");
            fs::create_dir_all(&parent).unwrap();
            let path = parent.join(uuid::Uuid::new_v4().to_string());
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn paths(&self) -> Paths {
            Paths {
                native: self.0.join("native"),
                legacy: self.0.join("legacy"),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn call(service: &mut SessionService, command: Command) -> Result<Reply, ServiceError> {
        service.try_submit(command).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(completion) = service.try_poll()? {
                return completion.result;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn bounded_legacy_scan_keeps_schema_one_only_and_preserves_originals() {
        let fixture = Fixture::new();
        let root = fixture.paths().legacy;
        let projects = root.join("projects");
        fs::create_dir_all(&projects).unwrap();
        let one = ProjectDocument::new("Legacy source")
            .unwrap()
            .to_json()
            .unwrap();
        let two = ProjectDocument::new("Native source")
            .unwrap()
            .upgrade_native()
            .unwrap()
            .to_json()
            .unwrap();
        fs::write(projects.join("one.json"), &one).unwrap();
        fs::write(projects.join("two.json"), &two).unwrap();
        fs::write(projects.join("broken.json"), b"{broken").unwrap();
        let scan = scan_legacy(&root).unwrap();
        assert_eq!(scan.projects.len(), 1);
        assert_eq!(scan.skipped, 2);
        assert_eq!(fs::read(projects.join("one.json")).unwrap(), one);
        assert_eq!(fs::read(projects.join("two.json")).unwrap(), two);
    }

    #[test]
    fn document_view_requires_schema_two_and_recording_kind_before_interpreting_payload() {
        let mut value = ProjectDocument::new("Opaque legacy recording field")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["recording"] =
            serde_json::json!(["malformed foreign payload",{"takeId":"opaque"}]);
        let legacy = ProjectDocument::from_value(value.clone()).unwrap();
        let expected = legacy.to_json().unwrap();
        let view = DocumentView::prepare(legacy).unwrap();
        assert!(view.recordings.iter().all(Option::is_none));
        assert_eq!(view.document.to_json().unwrap(), expected);
        value["tracks"][0]["kind"] = "recording".into();
        assert!(
            ProjectDocument::from_value(value).is_err(),
            "Schema one cannot declare a native recording track"
        );
    }
    #[test]
    fn native_backend_lost_ack_retry_uses_durable_id_and_returns_current_session() {
        struct LostAck {
            inner: NativeBackend,
            lost: bool,
        }
        impl Backend for LostAck {
            fn handle(&mut self, command: &Command) -> Result<Reply, ServiceError> {
                let reply = self.inner.handle(command)?;
                if matches!(command, Command::Edit { .. }) && !self.lost {
                    self.lost = true;
                    return Err(ServiceError::backend(
                        "Injected acknowledgement loss after commit",
                        true,
                    ));
                }
                Ok(reply)
            }
        }
        let fixture = Fixture::new();
        let mut service = SessionService::spawn_with_backend(LostAck {
            inner: NativeBackend {
                paths: fixture.paths(),
                store: None,
                legacy: LegacyScan::default(),
                legacy_notice: None,
                require_existing: false,
            },
            lost: false,
        })
        .unwrap();
        call(&mut service, Command::Initialize).unwrap();
        let Reply::Document { view, .. } = call(
            &mut service,
            Command::Create {
                name: "Before".into(),
            },
        )
        .unwrap() else {
            panic!()
        };
        let id = ProjectId(view.document.id().into());
        let intent = ProjectEditIntent {
            operation_id: OperationId("atomic-save".into()),
            project_id: id.clone(),
            expected_revision: 0,
            scope: EditScope::Project,
            edit: EditOperation::UpdateSession {
                name: "First committed".into(),
                tempo: 125.5,
            },
            extensions: Default::default(),
        };
        let error = call(&mut service, Command::Edit { intent }).unwrap_err();
        assert!(error.outcome_unknown);
        let retry = service.retry_command().unwrap().clone();
        let later = ProjectEditIntent {
            operation_id: OperationId("later-save".into()),
            project_id: id.clone(),
            expected_revision: 1,
            scope: EditScope::Project,
            edit: EditOperation::Rename {
                name: "Later committed".into(),
            },
            extensions: Default::default(),
        };
        call(&mut service, Command::Edit { intent: later }).unwrap();
        let Reply::Document { view, replayed, .. } = call(&mut service, retry).unwrap() else {
            panic!()
        };
        assert!(replayed);
        assert_eq!(view.document.revision(), 2);
        assert_eq!(view.document.name(), "Later committed");
        assert_eq!(view.document.tempo(), 125.5);
        let store = NativeStore::open(fixture.paths().native).unwrap();
        assert_eq!(store.load(&id.0).unwrap(), view.document);
    }

    #[test]
    fn fatal_primary_codes_include_extended_io_and_exclude_ordinary_failures() {
        for code in [10, 11, 26, 10 | (1 << 8), 11 | (2 << 8)] {
            assert!(fatal_sqlite_primary(code));
        }
        for code in [0, 1, 5, 6, 8, 13, 14, 19, 20, 21, 19 | (8 << 8)] {
            assert!(!fatal_sqlite_primary(code));
        }
    }

    #[test]
    fn truncated_synthetic_store_delivers_known_fatal_completion_before_generation_retires() {
        struct Watched {
            inner: NativeBackend,
            dropped: SyncSender<std::thread::ThreadId>,
        }
        impl Backend for Watched {
            fn handle(&mut self, command: &Command) -> Result<Reply, ServiceError> {
                self.inner.handle(command)
            }
        }
        impl Drop for Watched {
            fn drop(&mut self) {
                let _ = self.dropped.try_send(std::thread::current().id());
            }
        }
        let fixture = Fixture::new();
        let paths = fixture.paths();
        {
            let mut store = NativeStore::open(&paths.native).unwrap();
            store
                .create(&ProjectDocument::new("Synthetic before truncation").unwrap())
                .unwrap();
        }
        let database = paths.native.join("studio.sqlite3");
        assert!(fs::metadata(&database).unwrap().len() > 4096);
        fs::OpenOptions::new()
            .write(true)
            .open(&database)
            .unwrap()
            .set_len(4096)
            .unwrap();
        let evidence = fs::read(&database).unwrap();
        let (dropped, drop_signal) = mpsc::sync_channel(1);
        let mut service = SessionService::spawn_with_backend(Watched {
            inner: NativeBackend {
                paths: paths.clone(),
                store: None,
                legacy: LegacyScan::default(),
                legacy_notice: None,
                require_existing: false,
            },
            dropped,
        })
        .unwrap();
        let generation = service.generation();
        let id = service.try_submit(Command::Initialize).unwrap();
        assert_ne!(
            drop_signal.recv_timeout(Duration::from_secs(5)).unwrap(),
            std::thread::current().id()
        );
        // The response is intentionally not read until its producer has exited.
        assert_eq!(
            service.try_submit(Command::Refresh).unwrap_err().kind,
            SubmitFailure::Busy
        );
        let completion = service
            .try_poll()
            .unwrap()
            .expect("Known error must precede channel disconnection");
        assert_eq!(completion.id, id);
        let failure = completion.result.unwrap_err();
        assert_eq!(failure.kind, FailureKind::StorageUnavailable);
        assert!(!failure.outcome_unknown);
        assert!(service.is_disconnected());
        assert_eq!(
            service.try_poll().unwrap_err(),
            failure,
            "Terminal cause must not be replaced by generic EOF"
        );
        assert_eq!(
            service.try_submit(Command::Refresh).unwrap_err().kind,
            SubmitFailure::Disconnected
        );
        assert_eq!(
            fs::read(&database).unwrap(),
            evidence,
            "No automatic repair of even this synthetic input"
        );
        let mut fresh = SessionService::reconnect(paths).unwrap();
        assert_ne!(fresh.generation(), generation);
        assert_eq!(
            call(&mut fresh, Command::Initialize).unwrap_err().kind,
            FailureKind::StorageUnavailable
        );
        assert_eq!(fs::read(database).unwrap(), evidence);
    }

    #[test]
    fn explicit_reconnect_does_not_create_missing_expected_store() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let mut service = SessionService::reconnect(paths.clone()).unwrap();
        let error = call(&mut service, Command::Initialize).unwrap_err();
        assert_eq!(error.kind, FailureKind::StorageUnavailable);
        assert!(service.is_disconnected());
        assert!(!paths.native.exists());
    }

    #[test]
    fn invalid_archive_missing_input_conflict_and_missing_project_do_not_quarantine() {
        let fixture = Fixture::new();
        let mut service = SessionService::spawn(fixture.paths()).unwrap();
        call(&mut service, Command::Initialize).unwrap();
        let Reply::Document { view, .. } = call(
            &mut service,
            Command::Create {
                name: "Healthy session".into(),
            },
        )
        .unwrap() else {
            panic!()
        };
        let generation = service.generation();
        let archive = fixture.0.join("bad-backup");
        fs::write(&archive, b"not an archive").unwrap();
        let commands = [
            Command::InspectArchive {
                path: archive.clone(),
            },
            Command::ImportLegacy {
                path: fixture.0.join("absent-source.json"),
            },
            Command::Open {
                project_id: ProjectId("missing-project".into()),
            },
            Command::Edit {
                intent: ProjectEditIntent {
                    operation_id: OperationId("conflicting-revision".into()),
                    project_id: ProjectId(view.document.id().into()),
                    expected_revision: 99,
                    scope: EditScope::Project,
                    edit: EditOperation::Rename {
                        name: "Rejected".into(),
                    },
                    extensions: Default::default(),
                },
            },
        ];
        for command in commands {
            let error = call(&mut service, command).unwrap_err();
            assert_ne!(error.kind, FailureKind::StorageUnavailable);
            assert!(!service.is_disconnected());
            assert_eq!(service.generation(), generation);
            assert!(matches!(
                call(&mut service, Command::Refresh).unwrap(),
                Reply::Library(_)
            ));
        }
        assert_eq!(fs::read(archive).unwrap(), b"not an archive");
    }
}
