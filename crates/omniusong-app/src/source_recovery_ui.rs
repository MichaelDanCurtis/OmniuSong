//! Pure copied-recording recovery choices. No filesystem, engine or service access.
use eframe::egui;
use omniusong_core::{
    contracts::{OperationId, ProjectId, TakeId, TrackId, Validate},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_session::{
    Command, FailureKind, MAX_SOURCE_ACTION_DETAIL_BYTES, PerformanceSourceActionDetail,
    PerformanceSourceActionReference, PerformanceSourceActionState,
    PerformanceSourceAttachmentChoice, PerformanceSourceAttachmentStatus,
    PerformanceSourceDecision, PerformanceSourceExecutionPhase, PerformanceSourceReview,
    PreparedPerformanceSourceAction, RecoveredPerformanceSourceResult, Reply, RequestId,
    ServiceError, attachment_operation_id,
};
use omniusong_store::{
    MAX_PERFORMANCE_SOURCE_ACTIONS, MAX_PERFORMANCE_SOURCE_REVIEW_BYTES, TakePublication,
};
use std::{
    collections::{BTreeSet, VecDeque},
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(test)]
#[path = "source_recovery_ui/tests.rs"]
mod tests;
#[cfg(test)]
pub(super) use tests::test_support;

const LIST_BYTES: usize = 576 * 1024;
const STATE_BYTES: usize = 64 * 1024;
const MESSAGE_BYTES: usize = 2048;
const CARD_BYTES: usize = 8192;
const MAX_CARDS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceRecoveryOffer {
    pub(super) generation: u64,
    pub(super) epoch: u64,
    pub(super) serial: u64,
    pub(super) review: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceRecoveryTarget {
    pub(super) project_id: ProjectId,
    pub(super) revision: u64,
    pub(super) requires_native_audio_upgrade: bool,
}
pub(super) struct SourceRecoveryAvailability {
    pub(super) can_submit: bool,
    pub(super) can_mutate: bool,
    pub(super) can_read_artifacts: bool,
    pub(super) generation: Option<u64>,
    pub(super) origin: Option<PathBuf>,
    pub(super) review: Option<SourceRecoveryOffer>,
    pub(super) target: Option<SourceRecoveryTarget>,
    pub(super) unavailable_reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SourceRecoveryOperation {
    Prepare {
        action: Box<PreparedPerformanceSourceAction>,
        review: Vec<u8>,
    },
    List,
    Read {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    Execute {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    Reconcile {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    Decision {
        id: String,
        revision: u64,
        payload_sha256: String,
        decision: Box<PerformanceSourceDecision>,
    },
    Attach {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
    ReconcileAttachment {
        id: String,
        revision: u64,
        payload_sha256: String,
    },
}
#[derive(Clone, Debug)]
pub(super) struct SourceRecoveryRequest {
    pub(super) operation: SourceRecoveryOperation,
    epoch: u64,
    generation: u64,
    serial: u64,
    origin: PathBuf,
    detail_serial: Option<u64>,
    proof_serial: Option<u64>,
    expected_choice: Option<Box<PerformanceSourceAttachmentChoice>>,
    approved_target: Option<SourceRecoveryTarget>,
    upgrade_consent: bool,
}
impl SourceRecoveryRequest {
    pub(super) fn command(&self) -> Command {
        match &self.operation {
            SourceRecoveryOperation::Prepare { action, review } => {
                Command::SavePreparedPerformanceSourceAction {
                    expected_revision: None,
                    action: action.clone(),
                    review: review.clone(),
                }
            }
            SourceRecoveryOperation::List => Command::ListPerformanceSourceActionStates,
            SourceRecoveryOperation::Read {
                id,
                revision,
                payload_sha256,
            } => Command::ReadPerformanceSourceAction {
                id: id.clone(),
                revision: *revision,
                payload_sha256: payload_sha256.clone(),
            },
            SourceRecoveryOperation::Execute {
                id,
                revision,
                payload_sha256,
            } => Command::ExecutePerformanceSourceAction {
                id: id.clone(),
                revision: *revision,
                payload_sha256: payload_sha256.clone(),
            },
            SourceRecoveryOperation::Reconcile {
                id,
                revision,
                payload_sha256,
            } => Command::ReconcilePerformanceSourceAction {
                id: id.clone(),
                revision: *revision,
                payload_sha256: payload_sha256.clone(),
            },
            SourceRecoveryOperation::Decision {
                id,
                revision,
                payload_sha256,
                decision,
            } => Command::SavePerformanceSourceDecision {
                id: id.clone(),
                revision: *revision,
                payload_sha256: payload_sha256.clone(),
                decision: decision.clone(),
            },
            SourceRecoveryOperation::Attach {
                id,
                revision,
                payload_sha256,
            } => Command::AttachPerformanceSource {
                id: id.clone(),
                revision: *revision,
                payload_sha256: payload_sha256.clone(),
            },
            SourceRecoveryOperation::ReconcileAttachment {
                id,
                revision,
                payload_sha256,
            } => Command::ReconcilePerformanceSourceAttachment {
                id: id.clone(),
                revision: *revision,
                payload_sha256: payload_sha256.clone(),
            },
        }
    }
    pub(super) fn is_artifact(&self) -> bool {
        matches!(
            self.operation,
            SourceRecoveryOperation::Execute { .. }
                | SourceRecoveryOperation::Reconcile { .. }
                | SourceRecoveryOperation::Attach { .. }
                | SourceRecoveryOperation::ReconcileAttachment { .. }
        )
    }
    pub(super) fn is_mutation(&self) -> bool {
        matches!(
            self.operation,
            SourceRecoveryOperation::Prepare { .. }
                | SourceRecoveryOperation::Execute { .. }
                | SourceRecoveryOperation::Decision { .. }
                | SourceRecoveryOperation::Attach { .. }
        )
    }
    fn same(&self, other: &Self) -> bool {
        self.operation == other.operation
            && self.epoch == other.epoch
            && self.generation == other.generation
            && self.serial == other.serial
            && exact(&self.origin, &other.origin)
            && self.detail_serial == other.detail_serial
            && self.proof_serial == other.proof_serial
            && self.expected_choice == other.expected_choice
            && self.approved_target == other.approved_target
            && self.upgrade_consent == other.upgrade_consent
    }
    pub(super) fn approved_target(&self) -> Option<&SourceRecoveryTarget> {
        self.approved_target.as_ref()
    }
    pub(super) fn permits_native_audio_upgrade(&self) -> bool {
        self.upgrade_consent
            && self
                .approved_target
                .as_ref()
                .is_some_and(|t| t.requires_native_audio_upgrade)
    }
    pub(super) fn attachment_publication(&self) -> Option<&TakePublication> {
        match &self.operation {
            SourceRecoveryOperation::Decision { decision, .. } => match decision.as_ref() {
                PerformanceSourceDecision::PrepareAttachment { choice } => {
                    Some(&choice.publication)
                }
                _ => None,
            },
            SourceRecoveryOperation::Attach { .. }
            | SourceRecoveryOperation::ReconcileAttachment { .. } => {
                self.expected_choice.as_ref().map(|c| &c.publication)
            }
            _ => None,
        }
    }
    fn action_id(&self) -> Option<&str> {
        match &self.operation {
            SourceRecoveryOperation::Prepare { action, .. } => Some(&action.id),
            SourceRecoveryOperation::List => None,
            SourceRecoveryOperation::Read { id, .. }
            | SourceRecoveryOperation::Execute { id, .. }
            | SourceRecoveryOperation::Reconcile { id, .. }
            | SourceRecoveryOperation::Decision { id, .. }
            | SourceRecoveryOperation::Attach { id, .. }
            | SourceRecoveryOperation::ReconcileAttachment { id, .. } => Some(id),
        }
    }
    fn reference(&self) -> Option<PerformanceSourceActionReference> {
        match &self.operation {
            SourceRecoveryOperation::Read {
                id,
                revision,
                payload_sha256,
            }
            | SourceRecoveryOperation::Execute {
                id,
                revision,
                payload_sha256,
            }
            | SourceRecoveryOperation::Reconcile {
                id,
                revision,
                payload_sha256,
            }
            | SourceRecoveryOperation::Decision {
                id,
                revision,
                payload_sha256,
                ..
            }
            | SourceRecoveryOperation::Attach {
                id,
                revision,
                payload_sha256,
            }
            | SourceRecoveryOperation::ReconcileAttachment {
                id,
                revision,
                payload_sha256,
            } => Some(PerformanceSourceActionReference {
                id: id.clone(),
                revision: *revision,
                payload_sha256: payload_sha256.clone(),
            }),
            _ => None,
        }
    }
    pub(super) fn matches_command(&self, command: &Command) -> bool {
        match (&self.operation, command) {
            (
                SourceRecoveryOperation::Prepare { action, review },
                Command::SavePreparedPerformanceSourceAction {
                    expected_revision,
                    action: actual,
                    review: raw,
                },
            ) => expected_revision.is_none() && action == actual && review == raw,
            (SourceRecoveryOperation::List, Command::ListPerformanceSourceActionStates) => true,
            (
                SourceRecoveryOperation::Read { .. },
                Command::ReadPerformanceSourceAction {
                    id,
                    revision,
                    payload_sha256,
                },
            )
            | (
                SourceRecoveryOperation::Execute { .. },
                Command::ExecutePerformanceSourceAction {
                    id,
                    revision,
                    payload_sha256,
                },
            )
            | (
                SourceRecoveryOperation::Reconcile { .. },
                Command::ReconcilePerformanceSourceAction {
                    id,
                    revision,
                    payload_sha256,
                },
            )
            | (
                SourceRecoveryOperation::Attach { .. },
                Command::AttachPerformanceSource {
                    id,
                    revision,
                    payload_sha256,
                },
            )
            | (
                SourceRecoveryOperation::ReconcileAttachment { .. },
                Command::ReconcilePerformanceSourceAttachment {
                    id,
                    revision,
                    payload_sha256,
                },
            ) => self
                .reference()
                .is_some_and(|r| r.matches(id, *revision, payload_sha256)),
            (
                SourceRecoveryOperation::Decision { decision, .. },
                Command::SavePerformanceSourceDecision {
                    id,
                    revision,
                    payload_sha256,
                    decision: actual,
                },
            ) => {
                decision == actual
                    && self
                        .reference()
                        .is_some_and(|r| r.matches(id, *revision, payload_sha256))
            }
            _ => false,
        }
    }
    /// Additional validation before native installation. Shared service
    /// correlation separately retains credit for a wrong requested reference;
    /// a matching reference with a malformed body becomes Protocol uncertainty.
    pub(super) fn matches_reply(&self, reply: &Reply) -> bool {
        if omniusong_session::validate_performance_source_reply(reply, &self.command()).is_err() {
            return false;
        }
        let bound = |requested: &PerformanceSourceActionReference,
                     state: &PerformanceSourceActionState| {
            self.reference().as_ref() == Some(requested)
                && self.action_id() == Some(state.prepared.action.id.as_str())
        };
        match (&self.operation, reply) {
            (
                SourceRecoveryOperation::Prepare { action, .. },
                Reply::PreparedPerformanceSourceAction(receipt),
            ) => receipt.revision == 1 && receipt.action == **action,
            (SourceRecoveryOperation::List, Reply::PerformanceSourceActionStates(_)) => true,
            (
                SourceRecoveryOperation::Read {
                    id,
                    revision,
                    payload_sha256,
                },
                Reply::PerformanceSourceActionDetail(detail),
            ) => {
                detail.state.prepared.action.id == *id
                    && detail.state.revision == *revision
                    && detail.state.payload_sha256 == *payload_sha256
                    && exact(&self.origin, &detail.origin_root)
            }
            (
                SourceRecoveryOperation::Execute { .. } | SourceRecoveryOperation::Reconcile { .. },
                Reply::PerformanceSourceExecution(receipt),
            ) => bound(&receipt.requested, &receipt.state),
            (
                SourceRecoveryOperation::Decision { decision, .. },
                Reply::PerformanceSourceDecision(receipt),
            ) => {
                bound(&receipt.requested, &receipt.state)
                    && decision_matches(decision, self.expected_choice.as_deref(), &receipt.state)
            }
            (
                SourceRecoveryOperation::Attach { .. }
                | SourceRecoveryOperation::ReconcileAttachment { .. },
                Reply::PerformanceSourceAttachment(receipt),
            ) => {
                bound(&receipt.requested, &receipt.state)
                    && self
                        .expected_choice
                        .as_ref()
                        .is_some_and(|choice| choice.publication == receipt.publication)
            }
            _ => false,
        }
    }
}
#[derive(Clone, Debug)]
pub(super) enum SourceRecoveryAction {
    Submit(SourceRecoveryRequest),
    Cancel(RequestId),
}

fn require(ok: bool, text: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(text.into()) }
}
fn exact(a: &Path, b: &Path) -> bool {
    a.as_os_str() == b.as_os_str()
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && path
            .to_str()
            .is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}
fn short(text: &str) -> String {
    if text.chars().count() <= 512 && text.len() <= MESSAGE_BYTES {
        return text.into();
    }
    let mut value: String = text.chars().take(480).collect();
    value.push_str("… (message shortened)");
    value
}
struct ByteCap {
    used: usize,
    maximum: usize,
}
impl Write for ByteCap {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.used) {
            return Err(std::io::Error::other(
                "Recording-recovery metadata exceeds its display bound",
            ));
        }
        self.used += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
macro_rules! bounded {
    ($value:expr, $limit:expr) => {{
        let mut cap = ByteCap {
            used: 0,
            maximum: $limit,
        };
        serde_json::to_writer(&mut cap, $value)
            .map(|_| cap.used)
            .map_err(|error| error.to_string())
    }};
}
fn reference(state: &PerformanceSourceActionState) -> PerformanceSourceActionReference {
    PerformanceSourceActionReference {
        id: state.prepared.action.id.clone(),
        revision: state.revision,
        payload_sha256: state.payload_sha256.clone(),
    }
}
fn verified(state: &PerformanceSourceActionState) -> Option<&RecoveredPerformanceSourceResult> {
    match &state.execution {
        Some(execution) => match &execution.phase {
            PerformanceSourceExecutionPhase::Verified { result, .. } => Some(result),
            _ => None,
        },
        _ => None,
    }
}
fn state_valid(state: &PerformanceSourceActionState) -> Result<(), String> {
    bounded!(state, STATE_BYTES)?;
    state.prepared.action.validate().map_err(|e| e.message)?;
    reference(state).validate().map_err(|e| e.message)?;
    require(
        state.prepared.revision == 1 && sha(&state.prepared.payload_sha256),
        "Malformed immutable Prepared receipt",
    )?;
    match &state.execution {
        None => require(
            state.revision == 1
                && state.payload_sha256 == state.prepared.payload_sha256
                && !state.decisions.kept_locally
                && state.decisions.attachment.is_none(),
            "Prepared has unsupported later state",
        )?,
        Some(execution) => {
            require(
                execution.version == 1
                    && execution.id == state.prepared.action.id
                    && execution.prepared_payload_sha256 == state.prepared.payload_sha256,
                "Execution differs from its immutable Prepared identity",
            )?;
            match &execution.phase {
                PerformanceSourceExecutionPhase::Started => require(
                    state.revision == 2
                        && !state.decisions.kept_locally
                        && state.decisions.attachment.is_none(),
                    "Started carries unsupported decisions",
                )?,
                PerformanceSourceExecutionPhase::Verified { result, .. } => {
                    require(
                        state.revision >= 3
                            && result.take_id == state.prepared.action.take_id
                            && result.unknown_tail
                            && result.evidence_bytes > 0
                            && result.evidence_bytes <= 536_870_912
                            && result.working_bytes > 0
                            && result.working_bytes <= 536_870_912
                            && result.selected_chunk_count > 0
                            && [
                                &result.descriptor_sha256,
                                &result.original_rows_sha256,
                                &result.evidence_sha256,
                                &result.working_sha256,
                                &result.manifest_sha256,
                                &result.selected_chunk_index_sha256,
                            ]
                            .into_iter()
                            .all(|s| sha(s)),
                        "Malformed recorded Verified result",
                    )?;
                }
            }
        }
    }
    if let Some(attachment) = &state.decisions.attachment {
        PerformanceSourceDecision::PrepareAttachment {
            choice: attachment.choice.clone(),
        }
        .validate()
        .map_err(|e| e.message)?;
        require(
            verified(state).is_some()
                && (4..=state.revision).contains(&attachment.revision)
                && sha(&attachment.payload_sha256)
                && attachment.choice.publication.operation_id
                    == attachment_operation_id(&state.prepared.action.id, &attachment.choice.id)
                        .map_err(|e| e.message)?,
            "Malformed retained attachment identity",
        )?;
    }
    Ok(())
}
fn decision_matches(
    decision: &PerformanceSourceDecision,
    previous: Option<&PerformanceSourceAttachmentChoice>,
    state: &PerformanceSourceActionState,
) -> bool {
    match decision {
        PerformanceSourceDecision::KeepLocally => {
            state.decisions.kept_locally
                && previous.is_none_or(|choice| {
                    state
                        .decisions
                        .attachment
                        .as_ref()
                        .is_some_and(|a| *a.choice == *choice)
                })
        }
        PerformanceSourceDecision::ReturnToReview => {
            !state.decisions.kept_locally
                && previous.is_none_or(|choice| {
                    state
                        .decisions
                        .attachment
                        .as_ref()
                        .is_some_and(|a| *a.choice == *choice)
                })
        }
        PerformanceSourceDecision::PrepareAttachment { choice } => {
            state.decisions.attachment.as_ref().is_some_and(|a| {
                a.choice == *choice
                    && a.status == PerformanceSourceAttachmentStatus::Pending
                    && a.revision == state.revision
                    && a.payload_sha256 == state.payload_sha256
            })
        }
        PerformanceSourceDecision::RetireAttachment => previous.is_some_and(|choice| {
            state.decisions.attachment.as_ref().is_some_and(|a| {
                *a.choice == *choice && a.status == PerformanceSourceAttachmentStatus::Retired
            })
        }),
    }
}

struct Facts {
    path: PathBuf,
    stage: String,
    take: String,
    frames: u64,
    rate: u32,
    channels: u16,
    prefixes: Vec<String>,
    diagnostics: Vec<String>,
    omitted: usize,
    blob_sha: String,
    review_sha: String,
    admission_sha: String,
    usage: [u64; 8],
}
fn facts(detail: &PerformanceSourceActionDetail) -> Result<Facts, String> {
    review_facts(&detail.review, &detail.state.prepared.action)
}
fn review_facts(raw: &[u8], action: &PreparedPerformanceSourceAction) -> Result<Facts, String> {
    require(
        raw.len() <= MAX_PERFORMANCE_SOURCE_REVIEW_BYTES,
        "Retained review exceeds 64 KiB",
    )?;
    let review: PerformanceSourceReview = serde_json::from_slice(raw).map_err(|e| e.to_string())?;
    let checkpoint = review
        .performance
        .checkpoint
        .as_ref()
        .ok_or("No retained checkpoint")?;
    let mut diagnostics = vec!["The final tail is unknown. This is a recovered checkpoint copy, not a normal recording Finish.".into()];
    if let Some(problem) = &review.performance.problem {
        diagnostics.push(short(problem));
    }
    if let Some(fault) = &review.performance.capture_fault {
        diagnostics.push(short(&format!("Recorded capture fault: {fault:?}")));
    }
    let shown = review
        .generic_problems
        .len()
        .min(16usize.saturating_sub(diagnostics.len()));
    for problem in review.generic_problems.iter().take(shown) {
        diagnostics.push(short(&format!("{problem:?}")));
    }
    let u = review.usage;
    Ok(Facts {
        path: review.journal_path,
        stage: review.stage_id,
        take: review.take_id,
        frames: checkpoint.audio_end,
        rate: review.config.info.sample_rate,
        channels: review.config.info.channels,
        prefixes: checkpoint
            .prefixes
            .iter()
            .take(6)
            .map(|p| {
                short(&format!(
                    "{} · {} recorded units · {} gap units",
                    p.stream_id, p.recorded_units, p.gap_units
                ))
            })
            .collect(),
        diagnostics,
        omitted: review.generic_problems.len().saturating_sub(shown),
        blob_sha: action.review_blob_sha256.clone(),
        review_sha: review.review_sha256,
        admission_sha: action.admission_sha256.clone(),
        usage: [
            u.file_bytes,
            u.physical_rows,
            u.work_bytes,
            u.row_visits,
            u.chunk_visits,
            u.record_visits,
            u.groups,
            u.vm_steps,
        ],
    })
}
struct Detail {
    dto: Box<PerformanceSourceActionDetail>,
    facts: Facts,
    serial: u64,
    current: bool,
    catalog_read: bool,
}
struct Summary {
    reference: PerformanceSourceActionReference,
    stage: String,
    take: String,
    phase: &'static str,
}
struct Proof {
    detail_serial: u64,
    serial: u64,
    result: RecoveredPerformanceSourceResult,
    usage: [u64; 8],
}
struct Confirmation {
    request: SourceRecoveryRequest,
    approved: bool,
    replay: bool,
    preview: Option<Facts>,
}
struct Pending {
    request: SourceRecoveryRequest,
    id: RequestId,
    detail: Option<Box<PerformanceSourceActionDetail>>,
    cancel_attempted: bool,
    cancel_requested: bool,
}
struct Card {
    title: String,
    detail: String,
    generation: u64,
    historical: bool,
    requested: Option<PerformanceSourceActionReference>,
    head: Option<PerformanceSourceActionReference>,
    action_id: Option<String>,
    choice: Option<Box<PerformanceSourceAttachmentChoice>>,
    observed: Option<RecoveredPerformanceSourceResult>,
    committed: Option<bool>,
    nested: Option<(FailureKind, bool, String)>,
}
#[derive(Default)]
pub(super) struct SourceRecoveryUi {
    pub(super) open: bool,
    generation: Option<u64>,
    lost_generation: Option<u64>,
    epoch: u64,
    serial: u64,
    exhausted: bool,
    origin: Option<PathBuf>,
    offer: Option<SourceRecoveryOffer>,
    target: Option<SourceRecoveryTarget>,
    rows: Vec<Summary>,
    list_generation: Option<u64>,
    selected: Option<String>,
    detail: Option<Detail>,
    proof: Option<Proof>,
    offered: Option<SourceRecoveryRequest>,
    confirmation: Option<Confirmation>,
    pending: Option<Pending>,
    uncertain: Option<SourceRecoveryRequest>,
    cards: VecDeque<Card>,
    track_name: String,
    timeline_frame: String,
    upgrade_consent: bool,
    notice: Option<String>,
}
impl SourceRecoveryUi {
    fn invalidate(&mut self, keep_detail: bool) {
        match self.epoch.checked_add(1) {
            Some(epoch) => self.epoch = epoch,
            None => {
                self.exhausted = true;
                self.notice = Some(
                    "This review counter is exhausted. Use a fresh app view before another choice."
                        .into(),
                );
            }
        }
        self.offered = None;
        self.confirmation = None;
        self.proof = None;
        if !keep_detail {
            self.upgrade_consent = false;
            if let Some(detail) = &mut self.detail {
                detail.current = false;
            }
        }
    }
    pub(super) fn context_changed(&mut self) {
        self.invalidate(false);
    }
    pub(super) fn set_track_name(&mut self, value: String) {
        if self.track_name != value {
            self.track_name = value;
            self.invalidate(true);
        }
    }
    pub(super) fn set_timeline_frame(&mut self, value: String) {
        if self.timeline_frame != value {
            self.timeline_frame = value;
            self.invalidate(true);
        }
    }
    pub(super) fn set_upgrade_consent(&mut self, value: bool) {
        if self.upgrade_consent != value {
            self.upgrade_consent = value;
            self.invalidate(true);
        }
    }
    fn current_generation(&self, generation: u64) -> bool {
        self.generation == Some(generation) && self.lost_generation != Some(generation)
    }
    fn next_serial(&mut self) -> Option<u64> {
        if self.exhausted {
            return None;
        }
        match self.serial.checked_add(1) {
            Some(serial) => {
                self.serial = serial;
                Some(serial)
            }
            None => {
                self.exhausted = true;
                self.notice =
                    Some("This choice sequence is exhausted. No request was prepared.".into());
                None
            }
        }
    }
    pub(super) fn generation_changed(&mut self, generation: u64) {
        if self.generation.is_some_and(|old| generation <= old) {
            return;
        }
        if let Some(old) = self.generation {
            self.worker_lost(
                old,
                "A fresh storage connection replaced the previous worker.",
            );
        }
        self.generation = Some(generation);
        self.lost_generation = None;
        self.list_generation = None;
        self.invalidate(false);
    }
    fn push_card(&mut self, card: Card) {
        while self.cards.len() >= MAX_CARDS {
            self.cards.pop_front();
        }
        self.cards.push_back(card);
    }
    fn card(request: &SourceRecoveryRequest, title: &str, detail: &str, historical: bool) -> Card {
        Card {
            title: title.into(),
            detail: short(detail),
            generation: request.generation,
            historical,
            requested: request.reference(),
            head: None,
            action_id: request.action_id().map(str::to_owned),
            choice: None,
            observed: None,
            committed: None,
            nested: None,
        }
    }
    pub(super) fn worker_lost(&mut self, generation: u64, reason: &str) {
        if self.generation != Some(generation) || self.lost_generation == Some(generation) {
            return;
        }
        self.lost_generation = Some(generation);
        self.list_generation = None;
        self.invalidate(false);
        if let Some(pending) = self.pending.take() {
            if pending.request.is_mutation() && self.uncertain.is_none() {
                self.uncertain = Some(pending.request.clone());
            }
            self.push_card(Self::card(&pending.request, "Previous worker result is uncertain", "The acknowledgement was lost. Keep these exact identities. List and read explicitly; partial or complete recovery or publication may remain.", true));
        }
        self.notice = Some(short(reason));
    }
    fn sync(&mut self, availability: &SourceRecoveryAvailability) {
        if let Some(generation) = availability.generation {
            self.generation_changed(generation);
        }
        let origin = availability.origin.as_ref().filter(|p| valid_path(p));
        let origin_changed = match (&self.origin, origin) {
            (Some(a), Some(b)) => !exact(a, b),
            (None, None) => false,
            _ => true,
        };
        let review = availability.review.as_ref().filter(|offer| {
            offer.review.len() <= MAX_PERFORMANCE_SOURCE_REVIEW_BYTES
                && !offer.review.is_empty()
                && self.current_generation(offer.generation)
        });
        if origin_changed || self.offer.as_ref() != review || self.target != availability.target {
            if origin_changed {
                self.list_generation = None;
            }
            self.origin = origin.cloned();
            self.offer = review.cloned();
            self.target = availability.target.clone();
            self.invalidate(false);
        }
    }
    fn current_detail(&self) -> Option<&Detail> {
        self.detail.as_ref().filter(|d| {
            d.current
                && self.current_generation(self.generation.unwrap_or_default())
                && self.selected.as_deref() == Some(d.dto.state.prepared.action.id.as_str())
                && self
                    .origin
                    .as_ref()
                    .is_some_and(|root| exact(root, &d.dto.origin_root))
        })
    }
    pub(super) fn select_action(&mut self, id: &str) -> bool {
        if !self.rows.iter().any(|row| row.reference.id == id)
            && !self
                .cards
                .iter()
                .any(|card| card.action_id.as_deref() == Some(id))
            && self
                .detail
                .as_ref()
                .is_none_or(|detail| detail.dto.state.prepared.action.id != id)
        {
            return false;
        }
        if self.selected.as_deref() != Some(id) {
            self.invalidate(false);
            self.selected = Some(id.into());
        }
        true
    }
    fn make(
        &mut self,
        operation: SourceRecoveryOperation,
        detail_serial: Option<u64>,
        proof_serial: Option<u64>,
        expected_choice: Option<Box<PerformanceSourceAttachmentChoice>>,
    ) -> Option<SourceRecoveryRequest> {
        let generation = self.generation?;
        let origin = self.origin.clone()?;
        if self.pending.is_some() || !self.current_generation(generation) || !valid_path(&origin) {
            return None;
        }
        let placement = matches!(&operation, SourceRecoveryOperation::Attach { .. })
            || matches!(&operation, SourceRecoveryOperation::Decision { decision, .. } if matches!(decision.as_ref(), PerformanceSourceDecision::PrepareAttachment { .. }));
        let approved_target = if placement { self.target.clone() } else { None };
        let request = SourceRecoveryRequest {
            operation,
            epoch: self.epoch,
            generation,
            serial: self.next_serial()?,
            origin,
            detail_serial,
            proof_serial,
            expected_choice,
            approved_target,
            upgrade_consent: placement && self.upgrade_consent,
        };
        if request.command().validate().is_err() {
            return None;
        }
        Some(request)
    }
    fn offer_request(&mut self, request: SourceRecoveryRequest) -> Option<SourceRecoveryRequest> {
        self.offered = Some(request.clone());
        Some(request)
    }
    pub(super) fn list_request(&mut self) -> Option<SourceRecoveryRequest> {
        let request = self.make(SourceRecoveryOperation::List, None, None, None)?;
        self.offer_request(request)
    }
    pub(super) fn read_request(&mut self) -> Option<SourceRecoveryRequest> {
        let id = self.selected.as_ref()?;
        let head = self
            .current_detail()
            .filter(|d| &d.dto.state.prepared.action.id == id)
            .map(|d| reference(&d.dto.state))
            .or_else(|| {
                self.rows
                    .iter()
                    .find(|r| &r.reference.id == id)
                    .map(|r| r.reference.clone())
            })
            .or_else(|| {
                self.detail
                    .as_ref()
                    .filter(|d| &d.dto.state.prepared.action.id == id)
                    .map(|d| reference(&d.dto.state))
            })
            .or_else(|| {
                self.cards
                    .iter()
                    .rev()
                    .filter(|c| c.action_id.as_ref() == Some(id))
                    .find_map(|c| c.head.clone())
            })?;
        let request = self.make(
            SourceRecoveryOperation::Read {
                id: head.id,
                revision: head.revision,
                payload_sha256: head.payload_sha256,
            },
            None,
            None,
            None,
        )?;
        self.offer_request(request)
    }
    fn mutation_ready(
        &mut self,
        availability: &SourceRecoveryAvailability,
        replay: bool,
    ) -> Result<(), String> {
        self.sync(availability);
        require(
            availability.can_submit
                && availability.can_mutate
                && self.pending.is_none()
                && !self.exhausted
                && availability
                    .generation
                    .is_some_and(|g| self.current_generation(g))
                && self.origin.is_some(),
            "Resolve drafts, audio, pending choices and storage before reviewing this change",
        )?;
        require(
            replay || self.uncertain.is_none(),
            "An unacknowledged request is pinned. Read or reconcile that same action before a new change",
        )
    }
    fn confirm(&mut self, request: SourceRecoveryRequest, replay: bool) {
        let preview = match &request.operation {
            SourceRecoveryOperation::Prepare { action, review } => {
                review_facts(review, action).ok()
            }
            _ => None,
        };
        self.offered = None;
        self.confirmation = Some(Confirmation {
            request,
            approved: false,
            replay,
            preview,
        });
        self.notice = None;
    }
    pub(super) fn prepare_review(
        &mut self,
        availability: &SourceRecoveryAvailability,
    ) -> Result<(), String> {
        self.mutation_ready(availability, false)?;
        require(
            self.list_generation == self.generation,
            "List recovery choices explicitly before preparing a new action",
        )?;
        let offer = self
            .offer
            .as_ref()
            .ok_or("Inspect and select a current unsealed recording first")?;
        require(
            offer.generation == self.generation.unwrap_or_default(),
            "This source review belongs to an older worker",
        )?;
        let source: PerformanceSourceReview =
            serde_json::from_slice(&offer.review).map_err(|e| e.to_string())?;
        require(
            !self
                .rows
                .iter()
                .any(|row| row.stage == source.stage_id || row.take == source.take_id),
            "A saved choice already refers to this recording. Select and read that exact action",
        )?;
        require(
            self.rows.len() < MAX_PERFORMANCE_SOURCE_ACTIONS,
            "The 32 retained recovery choices are full; no choice is evicted",
        )?;
        if self.confirmation.as_ref().is_some_and(|c| c.request.epoch == self.epoch && matches!(&c.request.operation, SourceRecoveryOperation::Prepare { review, .. } if review == &offer.review)) { return Ok(()); }
        let action = PreparedPerformanceSourceAction::prepare(
            uuid::Uuid::new_v4().simple().to_string(),
            &offer.review,
            OperationId(format!("source-recovery:{}", uuid::Uuid::new_v4().simple())),
            None,
        )
        .map_err(|e| e.message)?;
        let expected = self
            .origin
            .as_ref()
            .expect("checked origin")
            .join("captures")
            .join(&action.stage_id)
            .join("take.sqlite3");
        require(
            exact(&source.journal_path, &expected),
            "Source review belongs to another studio origin",
        )?;
        let operation = SourceRecoveryOperation::Prepare {
            action: Box::new(action),
            review: offer.review.clone(),
        };
        let request = self
            .make(operation, None, None, None)
            .ok_or("No current preparation could be offered")?;
        self.confirm(request, false);
        Ok(())
    }
    pub(super) fn review_execute(
        &mut self,
        availability: &SourceRecoveryAvailability,
    ) -> Result<(), String> {
        self.mutation_ready(availability, false)?;
        let detail = self
            .current_detail()
            .ok_or("Read this exact saved action before reviewing execution")?;
        let state = &detail.dto.state;
        let proof_serial = match &state.execution {
            None => None,
            Some(execution) if matches!(execution.phase, PerformanceSourceExecutionPhase::Started) => Some(self.proof.as_ref().filter(|proof| proof.detail_serial == detail.serial).ok_or("Check the same existing recovery first. Started never permits another copy")?.serial),
            _ => return Err("This recovery is already recorded Verified. Check existing recovery or choose a disposition".into()),
        };
        let head = reference(state);
        let serial = detail.serial;
        let request = self
            .make(
                SourceRecoveryOperation::Execute {
                    id: head.id,
                    revision: head.revision,
                    payload_sha256: head.payload_sha256,
                },
                Some(serial),
                proof_serial,
                None,
            )
            .ok_or("Execution review is no longer current")?;
        self.confirm(request, false);
        Ok(())
    }
    pub(super) fn review_decision(
        &mut self,
        decision: PerformanceSourceDecision,
        availability: &SourceRecoveryAvailability,
    ) -> Result<(), String> {
        self.mutation_ready(availability, false)?;
        let detail = self
            .current_detail()
            .ok_or("Read a current saved action first")?;
        let state = &detail.dto.state;
        require(
            verified(state).is_some() && state.revision < 16,
            "This choice needs recorded Verified and space in its retained history",
        )?;
        let attachment = state.decisions.attachment.as_ref();
        let pending =
            attachment.is_some_and(|a| a.status == PerformanceSourceAttachmentStatus::Pending);
        match &decision {
            PerformanceSourceDecision::KeepLocally => require(
                !state.decisions.kept_locally && (!pending || state.revision < 15),
                "Keep needs a real toggle and room to settle the pending attachment",
            )?,
            PerformanceSourceDecision::ReturnToReview => require(
                state.decisions.kept_locally && (!pending || state.revision < 15),
                "Return needs a real toggle and room to settle the pending attachment",
            )?,
            PerformanceSourceDecision::RetireAttachment => require(
                pending,
                "Only the exact pending uncommitted choice can be retired",
            )?,
            PerformanceSourceDecision::PrepareAttachment { .. } => {
                return Err(
                    "Review the target and placement through Save attachment choice".into(),
                );
            }
        }
        let choice = attachment.map(|a| a.choice.clone());
        let serial = detail.serial;
        let head = reference(state);
        let request = self
            .make(
                SourceRecoveryOperation::Decision {
                    id: head.id,
                    revision: head.revision,
                    payload_sha256: head.payload_sha256,
                    decision: Box::new(decision),
                },
                Some(serial),
                None,
                choice,
            )
            .ok_or("Decision review is no longer current")?;
        self.confirm(request, false);
        Ok(())
    }
    pub(super) fn review_attachment(
        &mut self,
        availability: &SourceRecoveryAvailability,
    ) -> Result<(), String> {
        self.mutation_ready(availability, false)?;
        let detail = self
            .current_detail()
            .ok_or("Read the current Verified recovery first")?;
        let state = &detail.dto.state;
        let result = verified(state).ok_or("Attachment requires recorded Verified")?;
        require(
            !state.decisions.kept_locally && state.revision.checked_add(3).is_some_and(|n| n <= 16),
            "Return to review and retain room for choice, settlement and Keep",
        )?;
        let supersedes = match &state.decisions.attachment {
            None => None,
            Some(a) if a.status == PerformanceSourceAttachmentStatus::Retired => {
                require(
                    detail.catalog_read,
                    "Read this saved action again after atomic retirement before reviewing a replacement",
                )?;
                Some(a.choice.id.clone())
            }
            _ => return Err("A pending or committed attachment cannot be replaced".into()),
        };
        let target = self
            .target
            .as_ref()
            .ok_or("Select the saved target session explicitly")?;
        target.project_id.validate().map_err(|e| e.to_string())?;
        require(
            !target.requires_native_audio_upgrade || self.upgrade_consent,
            "Confirm native-audio format compatibility for this exact target before saving its attachment choice",
        )?;
        require(
            !self.track_name.trim().is_empty()
                && self.track_name.chars().count() <= 160
                && !self.track_name.chars().any(char::is_control),
            "Enter a new recording-track name of at most 160 characters",
        )?;
        require(
            !self.timeline_frame.is_empty()
                && self.timeline_frame.len() <= 19
                && self.timeline_frame.bytes().all(|b| b.is_ascii_digit()),
            "Enter a nonnegative whole timeline frame",
        )?;
        let frame: i64 = self
            .timeline_frame
            .parse()
            .map_err(|_| "Timeline frame is outside the native integer range")?;
        require(
            u64::try_from(frame)
                .ok()
                .and_then(|n| n.checked_add(detail.facts.frames))
                .is_some_and(|end| end <= i64::MAX as u64),
            "Placement end exceeds the native timeline range",
        )?;
        if self.confirmation.as_ref().is_some_and(|c| c.request.epoch == self.epoch && matches!(c.request.operation, SourceRecoveryOperation::Decision { ref decision, .. } if matches!(decision.as_ref(), PerformanceSourceDecision::PrepareAttachment { .. }))) { return Ok(()); }
        let choice_id = uuid::Uuid::new_v4().simple().to_string();
        let publication = TakePublication {
            operation_id: attachment_operation_id(&state.prepared.action.id, &choice_id)
                .map_err(|e| e.message)?,
            project_id: target.project_id.clone(),
            expected_revision: target.revision,
            track_id: TrackId(format!("recovered-track-{}", uuid::Uuid::new_v4().simple())),
            track_name: self.track_name.clone(),
            clip: TakeClip {
                id: format!("recovered-clip-{}", uuid::Uuid::new_v4().simple()),
                source: TakeReference {
                    take_id: TakeId(result.take_id.clone()),
                    manifest_sha256: result.manifest_sha256.clone(),
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 0,
                    count: detail.facts.frames,
                },
                position: TimelinePosition {
                    frame,
                    sample_rate: detail.facts.rate,
                },
                fades: None,
                edit_source: None,
            },
        };
        publication.validate().map_err(|e| e.to_string())?;
        let choice = PerformanceSourceAttachmentChoice {
            id: choice_id,
            supersedes,
            publication,
        };
        let decision = PerformanceSourceDecision::PrepareAttachment {
            choice: Box::new(choice),
        };
        decision.validate().map_err(|e| e.message)?;
        let serial = detail.serial;
        let head = reference(state);
        let request = self
            .make(
                SourceRecoveryOperation::Decision {
                    id: head.id,
                    revision: head.revision,
                    payload_sha256: head.payload_sha256,
                    decision: Box::new(decision),
                },
                Some(serial),
                None,
                state
                    .decisions
                    .attachment
                    .as_ref()
                    .map(|a| a.choice.clone()),
            )
            .ok_or("Attachment review is no longer current")?;
        self.confirm(request, false);
        Ok(())
    }
    pub(super) fn review_attach(
        &mut self,
        availability: &SourceRecoveryAvailability,
    ) -> Result<(), String> {
        self.mutation_ready(availability, false)?;
        let detail = self
            .current_detail()
            .ok_or("Read the saved immutable attachment choice first")?;
        let attachment = detail
            .dto
            .state
            .decisions
            .attachment
            .as_ref()
            .ok_or("Save an attachment choice first")?;
        require(
            !detail.dto.state.decisions.kept_locally
                && attachment.status == PerformanceSourceAttachmentStatus::Pending,
            "Only the pending choice in Review disposition can attach",
        )?;
        let target = self
            .target
            .as_ref()
            .ok_or("Select the saved choice's target session explicitly")?;
        require(
            target.project_id == attachment.choice.publication.project_id,
            "The selected session differs from this immutable attachment target",
        )?;
        require(
            !target.requires_native_audio_upgrade || self.upgrade_consent,
            "Confirm native-audio format compatibility for this exact target before attachment",
        )?;
        let request = self
            .make(
                SourceRecoveryOperation::Attach {
                    id: detail.dto.state.prepared.action.id.clone(),
                    revision: attachment.revision,
                    payload_sha256: attachment.payload_sha256.clone(),
                },
                Some(detail.serial),
                None,
                Some(attachment.choice.clone()),
            )
            .ok_or("Attachment is no longer current")?;
        self.confirm(request, false);
        Ok(())
    }
    pub(super) fn reconcile_request(&mut self) -> Option<SourceRecoveryRequest> {
        let detail = self.current_detail()?;
        detail.dto.state.execution.as_ref()?;
        let head = reference(&detail.dto.state);
        let serial = detail.serial;
        let request = self.make(
            SourceRecoveryOperation::Reconcile {
                id: head.id,
                revision: head.revision,
                payload_sha256: head.payload_sha256,
            },
            Some(serial),
            None,
            None,
        )?;
        self.offer_request(request)
    }
    pub(super) fn reconcile_attachment_request(&mut self) -> Option<SourceRecoveryRequest> {
        let detail = self.current_detail()?;
        let attachment = detail.dto.state.decisions.attachment.as_ref()?;
        let request = self.make(
            SourceRecoveryOperation::ReconcileAttachment {
                id: detail.dto.state.prepared.action.id.clone(),
                revision: attachment.revision,
                payload_sha256: attachment.payload_sha256.clone(),
            },
            Some(detail.serial),
            None,
            Some(attachment.choice.clone()),
        )?;
        self.offer_request(request)
    }
    pub(super) fn review_uncertain(
        &mut self,
        availability: &SourceRecoveryAvailability,
    ) -> Result<(), String> {
        self.mutation_ready(availability, true)?;
        let old = self
            .uncertain
            .as_ref()
            .ok_or("No unacknowledged request is pinned")?;
        require(
            exact(&old.origin, self.origin.as_ref().expect("checked origin"))
                && matches!(
                    old.operation,
                    SourceRecoveryOperation::Prepare { .. }
                        | SourceRecoveryOperation::Decision { .. }
                ),
            "Read and reconcile this exact action before any new artifact work",
        )?;
        let request = self
            .make(
                old.operation.clone(),
                None,
                None,
                old.expected_choice.clone(),
            )
            .ok_or("No exact metadata replay could be reviewed")?;
        self.confirm(request, true);
        Ok(())
    }
    fn request_current(&self, request: &SourceRecoveryRequest) -> bool {
        if self.pending.is_some()
            || self.exhausted
            || self.epoch != request.epoch
            || !self.current_generation(request.generation)
            || self
                .origin
                .as_ref()
                .is_none_or(|root| !exact(root, &request.origin))
        {
            return false;
        }
        if let Some(serial) = request.detail_serial
            && self.current_detail().is_none_or(|d| d.serial != serial)
        {
            return false;
        }
        if let Some(serial) = request.proof_serial
            && self.proof.as_ref().is_none_or(|p| p.serial != serial)
        {
            return false;
        }
        if let Some(target) = &request.approved_target
            && (self.target.as_ref() != Some(target)
                || (target.requires_native_audio_upgrade
                    && (!self.upgrade_consent || !request.upgrade_consent)))
        {
            return false;
        }
        true
    }
    pub(super) fn confirm_request(&mut self) -> Option<SourceRecoveryRequest> {
        let confirmation = self.confirmation.as_ref()?;
        if !self.request_current(&confirmation.request) {
            return None;
        }
        self.confirmation.as_mut()?.approved = true;
        self.confirmation.as_ref().map(|c| c.request.clone())
    }
    pub(super) fn authorizes(&self, request: &SourceRecoveryRequest, generation: u64) -> bool {
        if request.generation != generation || !self.request_current(request) {
            return false;
        }
        if request.is_mutation() {
            self.confirmation.as_ref().is_some_and(|c| {
                c.approved && c.request.same(request) && (self.uncertain.is_none() || c.replay)
            })
        } else {
            self.offered.as_ref().is_some_and(|r| r.same(request))
        }
    }
    pub(super) fn submitted(&mut self, request: &SourceRecoveryRequest, id: RequestId) {
        if !self.authorizes(request, id.generation) {
            self.notice = Some(
                "A stale recording-recovery request was not associated with this view.".into(),
            );
            return;
        }
        let detail = self.current_detail().map(|d| d.dto.clone());
        if matches!(
            request.operation,
            SourceRecoveryOperation::Read { .. }
                | SourceRecoveryOperation::Reconcile { .. }
                | SourceRecoveryOperation::ReconcileAttachment { .. }
        ) || request.is_mutation()
        {
            if let Some(detail) = &mut self.detail {
                detail.current = false;
            }
            self.proof = None;
        }
        self.pending = Some(Pending {
            request: request.clone(),
            id,
            detail,
            cancel_attempted: false,
            cancel_requested: false,
        });
        self.offered = None;
        self.confirmation = None;
        self.notice = None;
    }
    pub(super) fn submission_failed(&mut self, request: &SourceRecoveryRequest, error: &str) {
        if self.offered.as_ref().is_some_and(|r| r.same(request))
            || self
                .confirmation
                .as_ref()
                .is_some_and(|c| c.request.same(request))
        {
            self.offered = None;
            if let Some(c) = &mut self.confirmation {
                c.approved = false;
            }
            self.notice = Some(short(error));
        }
    }
    pub(super) fn pending_cancel(&self) -> Option<RequestId> {
        self.pending
            .as_ref()
            .filter(|p| p.request.is_artifact())
            .map(|p| p.id)
    }
    pub(super) fn cancel_requested(&mut self, id: RequestId, accepted: bool) {
        if let Some(p) = &mut self.pending
            && p.id == id
            && p.request.is_artifact()
            && !p.cancel_attempted
        {
            p.cancel_attempted = true;
            p.cancel_requested = accepted;
            self.notice = Some(if accepted { "Cancellation requested. Waiting for this exact result; it does not promise rollback." } else { "Cancellation was not accepted. The pending request was kept." }.into());
        }
    }

    fn next_detail(
        pending: &Pending,
        state: &PerformanceSourceActionState,
    ) -> Result<Box<PerformanceSourceActionDetail>, String> {
        state_valid(state)?;
        let review = match &pending.request.operation {
            SourceRecoveryOperation::Prepare { action, review } => {
                require(
                    **action == state.prepared.action,
                    "Prepared reply changed its immutable request",
                )?;
                review
            }
            _ => {
                let previous = pending
                    .detail
                    .as_ref()
                    .ok_or("No retained exact review binds this result")?;
                require(
                    previous.state.prepared == state.prepared,
                    "Result changed its immutable Prepared receipt",
                )?;
                &previous.review
            }
        };
        // Count the exact struct encoding before cloning either state or bytes.
        let root_size = bounded!(&pending.request.origin, MAX_SOURCE_ACTION_DETAIL_BYTES)?;
        let state_size = bounded!(state, MAX_SOURCE_ACTION_DETAIL_BYTES)?;
        let review_size = bounded!(review, MAX_SOURCE_ACTION_DETAIL_BYTES)?;
        let size = b"{\"version\":1,\"origin_root\":".len()
            + root_size
            + b",\"state\":".len()
            + state_size
            + b",\"review\":".len()
            + review_size
            + 1;
        require(
            review.len() <= MAX_PERFORMANCE_SOURCE_REVIEW_BYTES
                && size <= MAX_SOURCE_ACTION_DETAIL_BYTES,
            "Result detail exceeds its complete 320-KiB bound",
        )?;
        let detail = Box::new(PerformanceSourceActionDetail {
            version: 1,
            origin_root: pending.request.origin.clone(),
            state: state.clone(),
            review: review.clone(),
        });
        detail.validate().map_err(|e| e.message)?;
        Ok(detail)
    }
    fn result_valid(
        result: &RecoveredPerformanceSourceResult,
        detail: &PerformanceSourceActionDetail,
    ) -> Result<(), String> {
        bounded!(result, 16 * 1024)?;
        let source: PerformanceSourceReview =
            serde_json::from_slice(&detail.review).map_err(|e| e.to_string())?;
        let selection = source
            .performance
            .selection
            .as_ref()
            .ok_or("No retained checkpoint selection")?;
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
                && result.working_bytes > 0
                && result.working_bytes <= 536_870_912
                && sha(&result.working_sha256)
                && sha(&result.manifest_sha256),
            "Observed recovery differs from its retained original, selection or bounded copy",
        )
    }
    fn resolve_uncertain(
        &mut self,
        detail: &PerformanceSourceActionDetail,
        attachment_observed: bool,
    ) {
        let Some(request) = &self.uncertain else {
            return;
        };
        if request.action_id() != Some(detail.state.prepared.action.id.as_str())
            || !exact(&request.origin, &detail.origin_root)
        {
            return;
        }
        let resolved = match &request.operation {
            SourceRecoveryOperation::Prepare { action, review } => {
                **action == detail.state.prepared.action && *review == detail.review
            }
            SourceRecoveryOperation::Execute { .. } => {
                detail.state.execution.is_some()
                    // An authenticated current Prepared head can also resolve
                    // an acknowledgement lost before Started. This only clears
                    // the local block; another Execute needs a new review.
                    || (detail.state.revision == 1
                        && request.reference().is_some_and(|requested| {
                            requested == reference(&detail.state)
                        }))
            }
            SourceRecoveryOperation::Decision {
                decision, revision, ..
            } => {
                detail.state.revision >= *revision
                    && decision_matches(decision, request.expected_choice.as_deref(), &detail.state)
            }
            SourceRecoveryOperation::Attach { .. } => {
                attachment_observed
                    || detail.state.decisions.attachment.as_ref().is_some_and(|a| {
                        matches!(
                            a.status,
                            PerformanceSourceAttachmentStatus::Committed { .. }
                        ) && request
                            .expected_choice
                            .as_ref()
                            .is_some_and(|choice| choice.publication == a.choice.publication)
                    })
            }
            _ => false,
        };
        if resolved {
            self.uncertain = None;
        }
    }
    fn install_detail(
        &mut self,
        dto: Box<PerformanceSourceActionDetail>,
        observed: Option<(&RecoveredPerformanceSourceResult, [u64; 8])>,
        attachment_observed: bool,
    ) -> Result<(), String> {
        let facts = facts(&dto)?;
        let row_index = self
            .rows
            .iter()
            .position(|row| row.reference.id == dto.state.prepared.action.id);
        require(
            row_index.is_some() || self.rows.len() < MAX_PERFORMANCE_SOURCE_ACTIONS,
            "The known recovery rows are full. List recovery choices explicitly before reading another action",
        )?;
        let row = Summary {
            reference: reference(&dto.state),
            stage: dto.state.prepared.action.stage_id.clone(),
            take: dto.state.prepared.action.take_id.clone(),
            phase: phase(&dto.state),
        };
        let serial = self.next_serial().ok_or("Detail sequence exhausted")?;
        let proof = if let Some((result, usage)) = observed {
            let proof_serial = self.next_serial().ok_or("Proof sequence exhausted")?;
            Some(Proof {
                detail_serial: serial,
                serial: proof_serial,
                result: result.clone(),
                usage,
            })
        } else {
            None
        };
        self.resolve_uncertain(&dto, attachment_observed);
        // A fully accepted current result advances this known row without
        // claiming that the whole catalog has been listed again. Stale replies
        // never reach this installation path.
        if let Some(index) = row_index {
            self.rows[index] = row;
        } else {
            self.rows.push(row);
        }
        self.selected = Some(dto.state.prepared.action.id.clone());
        self.detail = Some(Detail {
            dto,
            facts,
            serial,
            current: true,
            catalog_read: false,
        });
        self.proof = proof;
        Ok(())
    }
    /// True permits the coordinator to consider a current document from this
    /// accepted body. It still checks the target and its own musical context.
    pub(super) fn completed(
        &mut self,
        request: &SourceRecoveryRequest,
        id: RequestId,
        result: Result<&Reply, &ServiceError>,
    ) -> bool {
        if !self
            .pending
            .as_ref()
            .is_some_and(|p| p.id == id && p.request.same(request))
        {
            return false;
        }
        if let Ok(reply) = result
            && !request.matches_reply(reply)
        {
            self.notice = Some(
                "A different recovery reply was ignored. Waiting for the exact request and choice."
                    .into(),
            );
            return false;
        }
        let pending = self.pending.take().expect("matched pending request");
        let current = self.current_generation(id.generation)
            && request.epoch == self.epoch
            && self
                .origin
                .as_ref()
                .is_some_and(|root| exact(root, &request.origin));
        let mut card = Self::card(request, "Recording recovery result", "", !current);
        let mut retire = false;
        let prepared = (|| {
            match result {
                Err(error) => {
                    card.title = if request.is_mutation() {
                        "Change is not confirmed"
                    } else {
                        "Check did not complete"
                    }
                    .into();
                    let stop = match error.kind {
                        FailureKind::InspectionCancelled => "Cancellation was observed. ",
                        FailureKind::InspectionLimit => "An inspection policy limit was observed. ",
                        _ => "",
                    };
                    card.detail = short(&format!(
                        "{stop}{}{}",
                        error.message,
                        if request.is_mutation() || error.outcome_unknown {
                            " Partial or complete output may remain; use this exact action's history. Cancellation is not rollback."
                        } else {
                            " The prior observation remains historical."
                        }
                    ));
                    if request.is_mutation()
                        && (error.outcome_unknown || error.kind == FailureKind::Protocol)
                        && self.uncertain.is_none()
                    {
                        self.uncertain = Some(request.clone());
                    }
                    retire = matches!(
                        error.kind,
                        FailureKind::StorageUnavailable | FailureKind::Disconnected
                    );
                }
                Ok(Reply::PerformanceSourceActionStates(states)) => {
                    bounded!(states, LIST_BYTES)?;
                    require(
                        states.len() <= MAX_PERFORMANCE_SOURCE_ACTIONS,
                        "More than 32 retained recovery actions",
                    )?;
                    let mut ids = BTreeSet::new();
                    for state in states {
                        state_valid(state)?;
                        require(
                            ids.insert(&state.prepared.action.id),
                            "Repeated recovery action in list",
                        )?;
                    }
                    card.title = "Recovery choices listed".into();
                    card.detail = format!(
                        "{} retained choices. Selecting a row reads nothing; Read and artifact checks remain separate buttons.",
                        states.len()
                    );
                    if current {
                        self.rows = states
                            .iter()
                            .map(|state| Summary {
                                reference: reference(state),
                                stage: state.prepared.action.stage_id.clone(),
                                take: state.prepared.action.take_id.clone(),
                                phase: phase(state),
                            })
                            .collect();
                        self.list_generation = Some(id.generation);
                        if let Some(detail) = &mut self.detail
                            && !states
                                .iter()
                                .any(|state| reference(state) == reference(&detail.dto.state))
                        {
                            detail.current = false;
                            self.proof = None;
                        }
                    }
                }
                Ok(Reply::PerformanceSourceActionDetail(dto)) => {
                    bounded!(dto, MAX_SOURCE_ACTION_DETAIL_BYTES)?;
                    require(
                        dto.review.len() <= MAX_PERFORMANCE_SOURCE_REVIEW_BYTES,
                        "Retained review exceeds 64 KiB",
                    )?;
                    dto.validate().map_err(|e| e.message)?;
                    card.title = "Saved recovery detail read".into();
                    card.detail = "Catalog metadata and exact retained review read. This did not reopen the recording or check current recovery files.".into();
                    card.head = Some(reference(&dto.state));
                    if current {
                        self.install_detail(dto.clone(), None, false)?;
                        self.detail
                            .as_mut()
                            .expect("accepted catalog detail")
                            .catalog_read = true;
                    }
                }
                Ok(Reply::PreparedPerformanceSourceAction(receipt)) => {
                    bounded!(receipt, STATE_BYTES)?;
                    let state = PerformanceSourceActionState {
                        prepared: *receipt.clone(),
                        revision: receipt.revision,
                        payload_sha256: receipt.payload_sha256.clone(),
                        execution: None,
                        decisions: Default::default(),
                    };
                    let dto = Self::next_detail(&pending, &state)?;
                    card.title = "Choice saved; recovery has not been requested".into();
                    card.detail = "Prepare saved one immutable choice. Review and confirm Execute separately to request a checkpoint copy.".into();
                    card.head = Some(reference(&state));
                    if current {
                        self.install_detail(dto, None, false)?;
                        self.list_generation = None;
                    }
                }
                Ok(Reply::PerformanceSourceExecution(receipt)) => {
                    bounded!(receipt, LIST_BYTES)?;
                    let dto = Self::next_detail(&pending, &receipt.state)?;
                    Self::result_valid(&receipt.observed, &dto)?;
                    let usage = receipt.usage;
                    require(
                        usage.file_bytes <= 536_870_912
                            && usage.physical_rows <= 262_144
                            && usage.work_bytes <= 1_073_741_824
                            && usage.row_visits <= 4_194_304
                            && usage.chunk_visits <= 524_288
                            && usage.record_visits <= 2_097_152
                            && usage.groups <= 65_536
                            && usage.vm_steps <= 32_000_000,
                        "Reported execution usage exceeds compiled inspection limits",
                    )?;
                    let recorded = verified(&receipt.state);
                    if matches!(request.operation, SourceRecoveryOperation::Execute { .. }) {
                        require(
                            recorded == Some(&receipt.observed),
                            "Execute did not return its recorded Verified result",
                        )?;
                    }
                    if let Some(recorded) = recorded {
                        require(
                            recorded == &receipt.observed,
                            "Current observed copy differs from recorded Verified",
                        )?;
                    }
                    bounded!(
                        &(
                            &receipt.requested,
                            reference(&receipt.state),
                            &receipt.observed
                        ),
                        CARD_BYTES
                    )?;
                    card.title = if recorded.is_some() {
                        "Recorded Verified · existing copy observed"
                    } else {
                        "Existing copy checked · recorded Started"
                    }
                    .into();
                    card.detail = format!(
                        "{} {}{}",
                        if receipt.replayed {
                            "The same recovery was observed; replay does not mean a second copy."
                        } else {
                            "The checkpoint copy was reported."
                        },
                        if receipt.original_reopened {
                            "The original recording was reopened for this operation."
                        } else {
                            "This check did not reopen the original; that does not establish whether it exists."
                        },
                        if pending.cancel_requested {
                            " Completion won the cancellation race; nothing was rolled back."
                        } else {
                            ""
                        }
                    );
                    card.head = Some(reference(&receipt.state));
                    card.observed = Some(receipt.observed.clone());
                    if current {
                        self.install_detail(
                            dto,
                            Some((
                                &receipt.observed,
                                [
                                    usage.file_bytes,
                                    usage.physical_rows,
                                    usage.work_bytes,
                                    usage.row_visits,
                                    usage.chunk_visits,
                                    usage.record_visits,
                                    usage.groups,
                                    usage.vm_steps,
                                ],
                            )),
                            false,
                        )?;
                    }
                }
                Ok(Reply::PerformanceSourceDecision(receipt)) => {
                    bounded!(receipt, LIST_BYTES)?;
                    let dto = Self::next_detail(&pending, &receipt.state)?;
                    let SourceRecoveryOperation::Decision {
                        decision,
                        revision,
                        payload_sha256,
                        ..
                    } = &request.operation
                    else {
                        unreachable!()
                    };
                    require(
                        receipt.state.revision == *revision
                            || revision.checked_add(1) == Some(receipt.state.revision),
                        "Decision changed an unrelated action revision",
                    )?;
                    if receipt.state.revision == *revision {
                        require(
                            receipt.state.payload_sha256 == *payload_sha256,
                            "Decision replay changed its exact requested head",
                        )?;
                    }
                    require(
                        decision_matches(
                            decision,
                            request.expected_choice.as_deref(),
                            &receipt.state,
                        ),
                        "Decision result differs from its immutable pending choice",
                    )?;
                    bounded!(
                        &(
                            &receipt.requested,
                            reference(&receipt.state),
                            &receipt.state.decisions.attachment
                        ),
                        CARD_BYTES
                    )?;
                    card.title = match decision.as_ref() {
                        PerformanceSourceDecision::KeepLocally => "Kept locally",
                        PerformanceSourceDecision::ReturnToReview => "Returned to review",
                        PerformanceSourceDecision::PrepareAttachment { .. } => {
                            "Attachment choice saved · not attached"
                        }
                        PerformanceSourceDecision::RetireAttachment => "Attachment choice retired",
                    }
                    .into();
                    card.detail = match decision.as_ref() { PerformanceSourceDecision::KeepLocally | PerformanceSourceDecision::ReturnToReview => "This changes disposition only. It does not cancel, release, delete or settle an attachment.", PerformanceSourceDecision::PrepareAttachment { .. } => "Target, placement and publication identities are immutable. Review and confirm Attach separately.", PerformanceSourceDecision::RetireAttachment => "Atomic retirement confirmed publication absence for this choice and blocks its later publication. Retained owned bytes may remain." }.into();
                    card.head = Some(reference(&receipt.state));
                    card.choice = receipt
                        .state
                        .decisions
                        .attachment
                        .as_ref()
                        .map(|a| a.choice.clone());
                    if current {
                        self.install_detail(dto, None, false)?;
                    }
                }
                Ok(Reply::PerformanceSourceAttachment(receipt)) => {
                    bounded!(
                        &(
                            &receipt.requested,
                            &receipt.state,
                            &receipt.publication,
                            receipt.committed,
                            receipt.replayed
                        ),
                        LIST_BYTES
                    )?;
                    let dto = Self::next_detail(&pending, &receipt.state)?;
                    receipt.publication.validate().map_err(|e| e.to_string())?;
                    let choice = request
                        .expected_choice
                        .as_ref()
                        .ok_or("No exact pending publication binds this result")?;
                    require(
                        choice.publication == receipt.publication,
                        "Attachment changed the pending immutable publication",
                    )?;
                    if receipt.committed {
                        require(
                            receipt
                                .state
                                .decisions
                                .attachment
                                .as_ref()
                                .is_some_and(|a| {
                                    a.choice.publication == receipt.publication
                                        && matches!(
                                            a.status,
                                            PerformanceSourceAttachmentStatus::Committed { .. }
                                        )
                                }),
                            "Historical commitment differs from the action's committed choice",
                        )?;
                    }
                    bounded!(
                        &(&receipt.requested, reference(&receipt.state), &choice),
                        CARD_BYTES
                    )?;
                    if let Some(error) = &receipt.current_document_error {
                        card.nested =
                            Some((error.kind, error.outcome_unknown, short(&error.message)));
                        retire = error.kind == FailureKind::StorageUnavailable;
                    }
                    card.title = if receipt.committed {
                        "Attachment committed · historical publication"
                    } else {
                        "This attachment remains uncommitted"
                    }
                    .into();
                    card.detail = if receipt.committed { "The exact publication has a historical receipt. Later edits or Undo may change its current placement; this card never installs a historical document." } else { "A check does not reserve publication absence. Explicit same-choice Attach or atomic Retire remains a separate choice; ingested owned bytes may already remain." }.into();
                    if pending.cancel_requested && receipt.committed {
                        card.detail.push_str(" Commitment was reported before cancellation took effect; nothing was rolled back.");
                    }
                    card.head = Some(reference(&receipt.state));
                    card.choice = Some(choice.clone());
                    card.committed = Some(receipt.committed);
                    if current {
                        self.install_detail(dto, None, true)?;
                    }
                }
                Ok(_) => unreachable!("matched source-action reply"),
            }
            Ok::<(), String>(())
        })();
        let accepted = prepared.is_ok() && result.is_ok() && current && !retire;
        if let Err(error) = prepared {
            card.title = "Recovery reply was not installed".into();
            card.detail = short(&error);
            card.observed = None;
            card.choice = None;
            card.committed = None;
            if request.is_mutation() && self.uncertain.is_none() {
                self.uncertain = Some(request.clone());
            }
        }
        if current {
            self.notice = if result.is_err() || card.title == "Recovery reply was not installed" {
                Some(card.detail.clone())
            } else {
                None
            };
        }
        self.push_card(card);
        if retire {
            self.worker_lost(id.generation, "Storage stopped after delivering this result. Its primary receipt was kept. Reconnect explicitly before another read.");
        }
        accepted
    }
}

fn phase(state: &PerformanceSourceActionState) -> &'static str {
    if let Some(attachment) = &state.decisions.attachment {
        return match attachment.status {
            PerformanceSourceAttachmentStatus::Pending => "Attachment pending",
            PerformanceSourceAttachmentStatus::Retired => "Attachment retired",
            PerformanceSourceAttachmentStatus::Committed { .. } => {
                "Publication committed · historical"
            }
        };
    }
    if state.decisions.kept_locally {
        return "Verified · kept locally";
    }
    match &state.execution {
        None => "Prepared · not executed",
        Some(execution) => match execution.phase {
            PerformanceSourceExecutionPhase::Started => "Started · unresolved",
            PerformanceSourceExecutionPhase::Verified { .. } => "Verified · historical record",
        },
    }
}

impl SourceRecoveryUi {
    fn details(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
        // egui's built-in CollapsingHeader forces Extend even in a wrapping
        // layout. Keep the complete header copyable/readable at narrow widths.
        let id = ui.make_persistent_id(("source-recovery-details", label));
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let header = ui.horizontal(|ui| {
            state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
            if ui
                .add(egui::Button::new(label).frame(false).wrap())
                .clicked()
            {
                state.toggle(ui);
            }
        });
        let _ = state.show_body_indented(&header.response, ui, body);
    }
    fn identity(ui: &mut egui::Ui, label: &str, text: &str) {
        ui.small(label);
        ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
        if ui.small_button(format!("Copy {label}")).clicked() {
            ui.ctx().copy_text(text.to_owned());
        }
    }
    fn reference_ui(ui: &mut egui::Ui, label: &str, reference: &PerformanceSourceActionReference) {
        ui.strong(label);
        Self::identity(ui, "Action ID", &reference.id);
        ui.label(format!("Revision {}", reference.revision));
        Self::identity(ui, "Payload SHA-256", &reference.payload_sha256);
    }
    fn result_ui(ui: &mut egui::Ui, result: &RecoveredPerformanceSourceResult) {
        Self::identity(ui, "Take ID", &result.take_id);
        for (label, hash) in [
            ("Descriptor SHA-256", &result.descriptor_sha256),
            ("All original rows SHA-256", &result.original_rows_sha256),
            ("Immutable evidence SHA-256", &result.evidence_sha256),
            ("Working copy SHA-256", &result.working_sha256),
            ("Recovered manifest SHA-256", &result.manifest_sha256),
            (
                "Selected chunk index SHA-256",
                &result.selected_chunk_index_sha256,
            ),
        ] {
            Self::identity(ui, label, hash);
        }
        ui.add(egui::Label::new(format!("Evidence {} bytes · working {} bytes · {} selected chunks · final tail remains unknown", result.evidence_bytes, result.working_bytes, result.selected_chunk_count)).wrap());
    }
    fn usage_ui(ui: &mut egui::Ui, values: &[u64; 8]) {
        for (name, value) in [
            "File-byte high water",
            "Physical rows",
            "Cumulative work bytes",
            "Row visits",
            "Chunk visits",
            "Record visits",
            "Groups",
            "SQLite VM steps",
        ]
        .into_iter()
        .zip(values)
        {
            ui.label(format!("{name}: {value}"));
        }
    }
    fn publication_ui(ui: &mut egui::Ui, choice: &PerformanceSourceAttachmentChoice) {
        Self::identity(ui, "Attachment choice ID", &choice.id);
        if let Some(id) = &choice.supersedes {
            Self::identity(ui, "Replaces retired choice", id);
        }
        let p = &choice.publication;
        Self::identity(ui, "Publication operation", &p.operation_id.0);
        Self::identity(ui, "Saved target session", &p.project_id.0);
        ui.label(format!("Saved target revision {}", p.expected_revision));
        Self::identity(ui, "New track ID", &p.track_id.0);
        Self::identity(ui, "New track name", &p.track_name);
        Self::identity(ui, "New clip ID", &p.clip.id);
        Self::identity(ui, "Exact take ID", &p.clip.source.take_id.0);
        Self::identity(ui, "Exact manifest SHA-256", &p.clip.source.manifest_sha256);
        if let TakeRange::AudioFrames { start, count } = p.clip.range {
            ui.add(
                egui::Label::new(format!(
                    "Source frames [{start}, {}) · {} Hz · timeline frame {}",
                    start + count,
                    p.clip.position.sample_rate,
                    p.clip.position.frame
                ))
                .wrap(),
            );
        }
        ui.small(
            "Full selected audio range. No fades, source trimming or claimed synchronization.",
        );
    }
    fn request_ui(ui: &mut egui::Ui, request: &SourceRecoveryRequest) {
        if let Some(head) = request.reference() {
            Self::reference_ui(ui, "Exact requested row", &head);
        }
        if let SourceRecoveryOperation::Prepare { action, review } = &request.operation {
            Self::identity(ui, "New action ID", &action.id);
            Self::identity(ui, "Source stage ID", &action.stage_id);
            Self::identity(ui, "Take ID", &action.take_id);
            Self::identity(ui, "Recovery operation", &action.recovery_operation_id.0);
            Self::identity(ui, "Exact review blob SHA-256", &action.review_blob_sha256);
            Self::identity(
                ui,
                "Stable source admission SHA-256",
                &action.admission_sha256,
            );
            ui.label(format!(
                "{} exact retained review bytes · unlinked recording choice",
                review.len()
            ));
            ui.small("No recording intent or target session is inferred. The original stays unchanged; evidence image and working copy are separate.");
            if ui.small_button("Copy exact retained review JSON").clicked()
                && let Ok(text) = String::from_utf8(review.clone())
            {
                ui.ctx().copy_text(text);
            }
        }
        if let Some(publication) = request.attachment_publication() {
            let choice = match &request.operation {
                SourceRecoveryOperation::Decision { decision, .. } => match decision.as_ref() {
                    PerformanceSourceDecision::PrepareAttachment { choice } => {
                        Some(choice.as_ref())
                    }
                    _ => None,
                },
                _ => request.expected_choice.as_deref(),
            };
            if let Some(choice) = choice {
                Self::publication_ui(ui, choice);
            }
            if let Some(target) = &request.approved_target {
                ui.add(
                    egui::Label::new(format!(
                        "Currently selected target revision {} · saved choice revision {}",
                        target.revision, publication.expected_revision
                    ))
                    .wrap(),
                );
            }
            if request.permits_native_audio_upgrade() {
                ui.small("Native-audio format upgrade consent is bound to this exact target and revision.");
            }
        }
    }
    fn review_label(request: &SourceRecoveryRequest) -> &'static str {
        match &request.operation {
            SourceRecoveryOperation::Prepare { .. } => "Save copied-recovery choice",
            SourceRecoveryOperation::Execute { .. } if request.proof_serial.is_some() => {
                "Record verified existing result"
            }
            SourceRecoveryOperation::Execute { .. } => "Execute checkpoint copy",
            SourceRecoveryOperation::Decision { decision, .. } => match decision.as_ref() {
                PerformanceSourceDecision::KeepLocally => "Keep recovered copy locally",
                PerformanceSourceDecision::ReturnToReview => "Return recovered copy to review",
                PerformanceSourceDecision::PrepareAttachment { .. } => {
                    "Save immutable attachment choice"
                }
                PerformanceSourceDecision::RetireAttachment => "Retire this uncommitted choice",
            },
            SourceRecoveryOperation::Attach { .. } => "Attach this saved choice",
            _ => "Confirm exact choice",
        }
    }
    fn compatibility_ui(&mut self, ui: &mut egui::Ui) {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.requires_native_audio_upgrade)
        {
            ui.add(egui::Label::new("Adding recorded audio upgrades this session to the native-audio format. Older versions may not open it.").wrap());
            let mut consent = self.upgrade_consent;
            if ui
                .checkbox(
                    &mut consent,
                    "Allow native-audio format for this exact session revision",
                )
                .changed()
            {
                self.set_upgrade_consent(consent);
            }
        }
    }
    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        availability: &SourceRecoveryAvailability,
    ) -> Option<SourceRecoveryAction> {
        self.sync(availability);
        let ready = availability.can_submit
            && self.pending.is_none()
            && !self.exhausted
            && self.origin.is_some()
            && availability
                .generation
                .is_some_and(|g| self.current_generation(g));
        let mutate = ready && availability.can_mutate && self.uncertain.is_none();
        let artifact = ready && availability.can_read_artifacts;
        let mut action = None;
        ui.strong("Recovered checkpoint copies");
        ui.add(egui::Label::new("Preserve the original, recover a separate copy, then choose whether to keep it or add it to a session.").wrap());
        ui.add(egui::Label::new(egui::RichText::new("512 MiB per file · 1 GiB cumulative inspection work · 32 retained choices. The final tail remains unknown.").small()).wrap());
        // Exact cancellation stays above every scroll/collapsed detail.
        if let Some(pending) = &self.pending {
            ui.group(|ui| {
                ui.strong(if pending.request.is_mutation() {
                    "Recording-recovery change is pending"
                } else {
                    "Recording-recovery read is pending"
                });
                if let Some(id) = pending.request.action_id() {
                    ui.add(egui::Label::new(format!("Action {id}")).wrap());
                }
                if pending.cancel_requested {
                    ui.label("Cancellation requested · waiting for the exact result.");
                }
                if pending.request.is_artifact()
                    && ui
                        .add_enabled(
                            !pending.cancel_attempted,
                            egui::Button::new("Cancel recovery request"),
                        )
                        .clicked()
                {
                    action = Some(SourceRecoveryAction::Cancel(pending.id));
                }
            });
        }
        if self.uncertain.is_some() {
            ui.add(egui::Label::new("An unacknowledged choice is retained outside the recent cards. Read or check that same action before a new change.").wrap());
        }
        if let Some(reason) = &availability.unavailable_reason {
            ui.add(egui::Label::new(short(reason)).wrap());
        }
        if let Some(notice) = &self.notice {
            ui.add(egui::Label::new(notice).wrap());
        }
        ui.separator();
        egui::ScrollArea::vertical().id_salt("source-recovery-content").max_height(ui.available_height().clamp(0.0, 640.0)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(ready, egui::Button::new("List recovery choices")).clicked() && let Some(request) = self.list_request() { action = Some(SourceRecoveryAction::Submit(request)); }
                if ui.add_enabled(ready && self.selected.is_some(), egui::Button::new("Read selected recovery")).clicked() && let Some(request) = self.read_request() { action = Some(SourceRecoveryAction::Submit(request)); }
            });
            ui.small("List and Read inspect catalog metadata. They do not reopen recordings, resume copying or publish a clip.");
            let mut selected = None;
            egui::ScrollArea::vertical().id_salt("source-recovery-rows").max_height(150.0).show_rows(ui, 46.0, self.rows.len(), |ui, range| {
                for index in range { let row = &self.rows[index]; ui.push_id(&row.reference.id, |ui| { if ui.add_sized([ui.available_width(), 22.0], egui::Button::selectable(self.selected.as_deref() == Some(row.reference.id.as_str()), &row.reference.id).wrap()).clicked() { selected = Some(row.reference.id.clone()); } ui.small(row.phase); }); }
            });
            if let Some(id) = selected { self.select_action(&id); }
            ui.separator();
            ui.strong("Prepare from the current reviewed recording");
            ui.small("First list existing choices. A recording with a saved action must use that action; missing or partial output is never replaced by a new copy.");
            if ui.add_enabled(mutate && self.offer.is_some() && self.list_generation == self.generation, egui::Button::new("Review copied recovery")).clicked() && let Err(error) = self.prepare_review(availability) { self.notice = Some(short(&error)); }
            if let Some(detail) = &self.detail {
                let current = self.current_detail().is_some();
                ui.separator(); ui.strong(if current { "Selected recovery · current catalog observation" } else { "Previous recovery detail · read again" });
                ui.add(egui::Label::new(phase(&detail.dto.state)).wrap());
                Self::reference_ui(ui, "Current saved head", &reference(&detail.dto.state));
                ui.add(egui::Label::new(format!("Selected source: {} frames · {} Hz · {} channels · {:.3} seconds", detail.facts.frames, detail.facts.rate, detail.facts.channels, detail.facts.frames as f64 / f64::from(detail.facts.rate))).wrap());
                ui.small("A saved Verified row is historical. It is separate from a fresh check of current artifacts.");
                Self::details(ui, "Retained original and checkpoint details", |ui| {
                    Self::identity(ui, "Original recording path", detail.facts.path.to_str().expect("validated retained path")); Self::identity(ui, "Source stage ID", &detail.facts.stage); Self::identity(ui, "Source take ID", &detail.facts.take);
                    Self::identity(ui, "Exact review blob SHA-256", &detail.facts.blob_sha); Self::identity(ui, "Review observation SHA-256", &detail.facts.review_sha); Self::identity(ui, "Stable source admission SHA-256", &detail.facts.admission_sha);
                    for prefix in &detail.facts.prefixes { ui.add(egui::Label::new(prefix).wrap()); }
                    for diagnostic in &detail.facts.diagnostics { ui.add(egui::Label::new(diagnostic).wrap()); }
                    if detail.facts.omitted > 0 { ui.small(format!("{} additional diagnostic rows omitted", detail.facts.omitted)); }
                    Self::details(ui, "Retained source-review resource usage", |ui| Self::usage_ui(ui, &detail.facts.usage));
                });
                if let Some(result) = verified(&detail.dto.state) { Self::details(ui, "Recorded Verified result · historical", |ui| { Self::result_ui(ui, result); if let Some(execution) = &detail.dto.state.execution && let PerformanceSourceExecutionPhase::Verified { usage: u, .. } = &execution.phase { Self::usage_ui(ui, &[u.file_bytes,u.physical_rows,u.work_bytes,u.row_visits,u.chunk_visits,u.record_visits,u.groups,u.vm_steps]); } }); }
                if let Some(proof) = &self.proof { Self::details(ui, "Fresh matching existing-copy observation", |ui| { Self::result_ui(ui, &proof.result); Self::usage_ui(ui, &proof.usage); }); }
            }
            let current = self.current_detail();
            let prepared = current.is_some_and(|d| d.dto.state.execution.is_none());
            let started = current.is_some_and(|d| d.dto.state.execution.as_ref().is_some_and(|e| matches!(e.phase, PerformanceSourceExecutionPhase::Started)));
            let verified = current.is_some_and(|d| verified(&d.dto.state).is_some());
            let kept = current.is_some_and(|d| d.dto.state.decisions.kept_locally);
            let attachment = current.and_then(|d| d.dto.state.decisions.attachment.as_ref());
            let has_attachment = attachment.is_some(); let pending_attachment = attachment.is_some_and(|a| a.status == PerformanceSourceAttachmentStatus::Pending);
            let replacement = attachment.is_none_or(|a| a.status == PerformanceSourceAttachmentStatus::Retired);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(mutate && (prepared || (started && self.proof.is_some())), egui::Button::new(if started { "Review recording existing result" } else { "Review execution" })).clicked() && let Err(error) = self.review_execute(availability) { self.notice = Some(short(&error)); }
                if ui.add_enabled(artifact && (started || verified), egui::Button::new("Check existing recovery")).clicked() && let Some(request) = self.reconcile_request() { action = Some(SourceRecoveryAction::Submit(request)); }
            });
            if verified {
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(mutate, egui::Button::new(if kept { "Review return to recovery" } else { "Review keep locally" })).clicked() { let decision = if kept { PerformanceSourceDecision::ReturnToReview } else { PerformanceSourceDecision::KeepLocally }; if let Err(error) = self.review_decision(decision, availability) { self.notice = Some(short(&error)); } }
                });
                ui.small("Keep changes disposition only. It never cancels, releases ownership or settles a pending attachment.");
                if let Some(detail) = self.current_detail() && let Some(attachment) = &detail.dto.state.decisions.attachment { Self::details(ui, "Saved immutable attachment choice", |ui| Self::publication_ui(ui, &attachment.choice)); }
                if replacement && !kept {
                    ui.separator(); ui.strong("Choose a new recording track and placement");
                    if let Some(target) = &self.target { Self::identity(ui, "Selected target session", &target.project_id.0); ui.label(format!("Current target revision {}", target.revision)); } else { ui.label("Select a saved target session explicitly."); }
                    let label = ui.label("New recording-track name"); let mut name = self.track_name.clone();
                    if ui.add(egui::TextEdit::singleline(&mut name).id_salt("source-recovery-track-name").char_limit(160).desired_width(ui.available_width())).labelled_by(label.id).changed() { self.set_track_name(name); }
                    let label = ui.label("Timeline frame (whole, nonnegative)"); let mut frame = self.timeline_frame.clone();
                    if ui.add(egui::TextEdit::singleline(&mut frame).id_salt("source-recovery-timeline-frame").char_limit(19).desired_width(ui.available_width()).hint_text("Enter 0 explicitly for the beginning")).labelled_by(label.id).changed() { self.set_timeline_frame(frame); }
                    self.compatibility_ui(ui);
                    if ui.add_enabled(mutate, egui::Button::new("Review attachment choice")).clicked() && let Err(error) = self.review_attachment(availability) { self.notice = Some(short(&error)); }
                }
                if has_attachment {
                    if pending_attachment { self.compatibility_ui(ui); }
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(mutate && pending_attachment && !kept, egui::Button::new("Review attach saved choice")).clicked() && let Err(error) = self.review_attach(availability) { self.notice = Some(short(&error)); }
                        if ui.add_enabled(artifact, egui::Button::new("Check attachment result")).clicked() && let Some(request) = self.reconcile_attachment_request() { action = Some(SourceRecoveryAction::Submit(request)); }
                        if ui.add_enabled(mutate && pending_attachment, egui::Button::new("Review retire attachment choice")).clicked() && let Err(error) = self.review_decision(PerformanceSourceDecision::RetireAttachment, availability) { self.notice = Some(short(&error)); }
                    });
                }
            }
            if let Some(confirmation) = &self.confirmation {
                ui.separator(); ui.group(|ui| { ui.strong("Review this exact choice"); ui.strong(Self::review_label(&confirmation.request)); Self::request_ui(ui, &confirmation.request); if let Some(preview) = &confirmation.preview { ui.add(egui::Label::new(format!("Selected checkpoint: {} audio frames · {} Hz · {} channels", preview.frames, preview.rate, preview.channels)).wrap()); Self::identity(ui, "Original recording path", preview.path.to_str().expect("validated offered source path")); for line in &preview.diagnostics { ui.add(egui::Label::new(line).wrap()); } Self::details(ui, "Six retained stream prefixes", |ui| { for prefix in &preview.prefixes { ui.add(egui::Label::new(prefix).wrap()); } }); } ui.small("This confirmation submits one step. No Execute, attachment, retry or replacement follows automatically. Partial or complete output can remain after an error."); });
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(ready && availability.can_mutate, egui::Button::new("Confirm this recovery choice")).clicked() && let Some(request) = self.confirm_request() { action = Some(SourceRecoveryAction::Submit(request)); }
                    if ui.button("Cancel local review").clicked() { self.invalidate(true); }
                });
            }
            if let Some(request) = &self.uncertain { Self::details(ui, "Exact unacknowledged request · retained", |ui| Self::request_ui(ui, request)); if ui.add_enabled(ready && availability.can_mutate && matches!(request.operation, SourceRecoveryOperation::Prepare { .. } | SourceRecoveryOperation::Decision { .. }), egui::Button::new("Review exact uncertain metadata request")).clicked() && let Err(error) = self.review_uncertain(availability) { self.notice = Some(short(&error)); } }
            if !self.cards.is_empty() { ui.separator(); ui.strong("Recent recovery observations"); }
            let mut read = None;
            for (index, card) in self.cards.iter().enumerate().rev() { ui.push_id(("source-recovery-card", index), |ui| { Self::details(ui, &format!("{}{} · worker {}", if card.historical { "Previous context · " } else { "" }, card.title, card.generation), |ui| { ui.add(egui::Label::new(&card.detail).wrap()); if let Some(reference) = &card.requested { Self::reference_ui(ui, "Requested historical row", reference); }
 if let Some(reference) = &card.head { Self::reference_ui(ui, "Returned latest head", reference); }
 if let Some(choice) = &card.choice { Self::publication_ui(ui, choice); }
 if let Some(observed) = &card.observed { Self::result_ui(ui, observed); }
 if let Some(committed) = card.committed { ui.strong(if committed { "Historical publication is committed" } else { "No committed receipt was reported by this observation" }); }
 if let Some((kind, unknown, error)) = &card.nested { ui.strong("Current session observation has a separate problem"); ui.add(egui::Label::new(error).wrap()); ui.small(format!("{kind:?} · outcome {}", if *unknown { "uncertain" } else { "reported" })); }
 if let Some(id) = &card.action_id && ui.add_enabled(ready, egui::Button::new("Read this recovery action")).clicked() { read = Some(id.clone()); } }); }); }
            if let Some(id) = read && self.select_action(&id) && let Some(request) = self.read_request() { action = Some(SourceRecoveryAction::Submit(request)); }
        });
        action
    }
}
