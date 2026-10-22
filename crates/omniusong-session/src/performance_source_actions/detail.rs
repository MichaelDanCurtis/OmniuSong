//! Bounded catalog observations, distinct from authenticating source artifacts.
use super::*;
use crate::Command;
use std::path::PathBuf;

pub const MAX_SOURCE_ACTION_DETAIL_BYTES: usize = 320 * 1024;
const MAX_STATE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceActionReference {
    pub id: String,
    pub revision: u64,
    pub payload_sha256: String,
}
impl PerformanceSourceActionReference {
    pub(super) fn new(id: &str, revision: u64, payload_sha256: &str) -> Self {
        Self {
            id: id.into(),
            revision,
            payload_sha256: payload_sha256.into(),
        }
    }
    pub fn validate(&self) -> Result<(), ServiceError> {
        validate_id(&self.id)?;
        require(
            (1..=16).contains(&self.revision) && sha(&self.payload_sha256),
            "Choose an exact retained source-action reference",
        )
    }
    pub fn matches(&self, id: &str, revision: u64, payload_sha256: &str) -> bool {
        self.id == id && self.revision == revision && self.payload_sha256 == payload_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceDecisionReceipt {
    pub requested: PerformanceSourceActionReference,
    pub state: PerformanceSourceActionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceActionDetail {
    pub version: u32,
    pub origin_root: PathBuf,
    pub state: PerformanceSourceActionState,
    /// Exact retained bytes; a catalog observation, never a new source read.
    pub review: Vec<u8>,
}

pub(super) fn state_shape(state: &PerformanceSourceActionState) -> Result<(), ServiceError> {
    encode(state, MAX_STATE_BYTES)?;
    state.prepared.action.validate()?;
    require(
        state.prepared.revision == 1
            && sha(&state.prepared.payload_sha256)
            && state.prepared.payload_sha256
                == digest(
                    b"",
                    &encode(&state.prepared.action, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?,
                )
            && (1..=16).contains(&state.revision)
            && sha(&state.payload_sha256),
        "Invalid retained source-action state identity",
    )?;
    match (&state.execution, state.revision) {
        (None, 1) => require(
            state.payload_sha256 == state.prepared.payload_sha256,
            "Prepared state differs from its retained row",
        )?,
        (Some(execution), 2..=16) => {
            require(
                execution.version == 1
                    && execution.id == state.prepared.action.id
                    && execution.prepared_payload_sha256 == state.prepared.payload_sha256,
                "Execution differs from its immutable Prepared reference",
            )?;
            require(
                matches!(
                    (&execution.phase, state.revision),
                    (PerformanceSourceExecutionPhase::Started, 2)
                        | (PerformanceSourceExecutionPhase::Verified { .. }, 3..=16)
                ),
                "Source execution phase contradicts its retained revision",
            )?;
            if state.revision <= 3 {
                require(
                    state.payload_sha256
                        == digest(
                            b"",
                            &encode(execution, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?,
                        ),
                    "Execution state differs from its retained row",
                )?;
            }
        }
        _ => {
            return Err(ServiceError::rejected(
                "Source state has no supported execution phase",
            ));
        }
    }
    require(
        state.revision > 3
            || (!state.decisions.kept_locally && state.decisions.attachment.is_none()),
        "Prepared or execution state cannot contain later decisions",
    )?;
    if let Some(attachment) = &state.decisions.attachment {
        validate_id(&attachment.choice.id)?;
        if let Some(id) = &attachment.choice.supersedes {
            validate_id(id)?;
        }
        attachment
            .choice
            .publication
            .validate()
            .map_err(|e| ServiceError::rejected(e.to_string()))?;
        require(
            (4..=state.revision).contains(&attachment.revision)
                && sha(&attachment.payload_sha256)
                && attachment.choice.publication.operation_id
                    == attachment_operation_id(&state.prepared.action.id, &attachment.choice.id)?,
            "Attachment metadata differs from its retained choice",
        )?;
    }
    Ok(())
}

impl PerformanceSourceActionDetail {
    /// Validate bounded presentation and retained-byte bindings. Only the worker
    /// validates the complete catalog history; this performs no artifact I/O.
    pub fn validate(&self) -> Result<(), ServiceError> {
        encode(self, MAX_SOURCE_ACTION_DETAIL_BYTES)?;
        require(
            self.version == 1,
            "Unsupported source-action detail version",
        )?;
        crate::evidence_jobs::path_text(&self.origin_root)?;
        state_shape(&self.state)?;
        let source = validate_review(&self.state.prepared.action, &self.review, &self.origin_root)?;
        crate::evidence_jobs::path_text(&source.journal_path)?;
        let expected = self
            .origin_root
            .join("captures")
            .join(&self.state.prepared.action.stage_id)
            .join("take.sqlite3");
        require(
            source.journal_path.as_os_str() == expected.as_os_str(),
            "Retained source review path differs lexically from this canonical studio",
        )?;
        if let Some(PerformanceSourceExecutionRevision {
            phase: PerformanceSourceExecutionPhase::Verified { result, usage },
            ..
        }) = &self.state.execution
        {
            execution::validate_result(result, &source)?;
            execution::validate_usage(*usage)?;
        }
        Ok(())
    }
}

fn reference(
    requested: &PerformanceSourceActionReference,
    state: &PerformanceSourceActionState,
    id: &str,
    revision: u64,
    payload: &str,
) -> bool {
    requested.validate().is_ok()
        && requested.matches(id, revision, payload)
        && state.prepared.action.id == id
        && state.revision >= requested.revision
}

/// Identity only, before releasing service credit. Correlated semantic failures
/// are terminal protocol results for the native caller, not stale identities.
pub(crate) fn reply_matches(reply: &Reply, command: &Command) -> Option<bool> {
    Some(match (reply, command) {
        (
            Reply::PreparedPerformanceSourceAction(receipt),
            Command::SavePreparedPerformanceSourceAction { action, review, .. },
        ) => {
            receipt.revision == 1
                && receipt.action == **action
                && encode(action, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)
                    .is_ok_and(|raw| receipt.payload_sha256 == digest(b"", &raw))
                && receipt.action.review_blob_sha256 == digest(b"", review)
        }
        (
            Reply::PerformanceSourceActionDetail(detail),
            Command::ReadPerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            },
        ) => {
            detail.state.prepared.action.id == *id
                && detail.state.revision == *revision
                && detail.state.payload_sha256 == *payload_sha256
        }
        (
            Reply::PerformanceSourceExecution(receipt),
            Command::ExecutePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            }
            | Command::ReconcilePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            },
        ) => reference(
            &receipt.requested,
            &receipt.state,
            id,
            *revision,
            payload_sha256,
        ),
        (
            Reply::PerformanceSourceDecision(receipt),
            Command::SavePerformanceSourceDecision {
                id,
                revision,
                payload_sha256,
                ..
            },
        ) => reference(
            &receipt.requested,
            &receipt.state,
            id,
            *revision,
            payload_sha256,
        ),
        (
            Reply::PerformanceSourceAttachment(receipt),
            Command::AttachPerformanceSource {
                id,
                revision,
                payload_sha256,
            }
            | Command::ReconcilePerformanceSourceAttachment {
                id,
                revision,
                payload_sha256,
            },
        ) => reference(
            &receipt.requested,
            &receipt.state,
            id,
            *revision,
            payload_sha256,
        ),
        _ => return None,
    })
}

/// Pure bounded presentation validation after exact request correlation. An
/// error grants no action/artifact authority and must preserve mutation doubt.
pub fn validate_performance_source_reply(
    reply: &Reply,
    command: &Command,
) -> Result<(), ServiceError> {
    if let Some(matches) = reply_matches(reply, command) {
        require(matches, "Source reply has a different request identity")?;
    }
    match (reply, command) {
        (
            Reply::PreparedPerformanceSourceAction(receipt),
            Command::SavePreparedPerformanceSourceAction { .. },
        ) => {
            encode(receipt, 576 * 1024)?;
            receipt.action.validate()
        }
        (
            Reply::PreparedPerformanceSourceActions(receipts),
            Command::ListPreparedPerformanceSourceActions,
        ) => {
            require(
                receipts.len() <= omniusong_store::MAX_PERFORMANCE_SOURCE_ACTIONS,
                "Too many retained source choices",
            )?;
            encode(receipts, 576 * 1024)?;
            let mut ids = BTreeSet::new();
            for receipt in receipts {
                receipt.action.validate()?;
                require(
                    receipt.revision == 1
                        && ids.insert(receipt.action.id.as_str())
                        && receipt.payload_sha256
                            == digest(
                                b"",
                                &encode(&receipt.action, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?,
                            ),
                    "Prepared list contains an invalid or duplicate identity",
                )?;
            }
            Ok(())
        }
        (
            Reply::PerformanceSourceActionStates(states),
            Command::ListPerformanceSourceActionStates,
        ) => {
            require(
                states.len() <= omniusong_store::MAX_PERFORMANCE_SOURCE_ACTIONS,
                "Too many retained source choices",
            )?;
            encode(states, 576 * 1024)?;
            let mut ids = BTreeSet::new();
            for state in states {
                state_shape(state)?;
                require(
                    ids.insert(state.prepared.action.id.as_str()),
                    "Source list repeats an action identity",
                )?;
            }
            Ok(())
        }
        (
            Reply::PerformanceSourceActionDetail(detail),
            Command::ReadPerformanceSourceAction { .. },
        ) => detail.validate(),
        (
            Reply::PerformanceSourceExecution(receipt),
            Command::ExecutePerformanceSourceAction { .. }
            | Command::ReconcilePerformanceSourceAction { .. },
        ) => {
            encode(receipt, 576 * 1024)?;
            state_shape(&receipt.state)?;
            execution::validate_usage(receipt.usage)?;
            let result = &receipt.observed;
            require(
                result.take_id == receipt.state.prepared.action.take_id
                    && result.unknown_tail
                    && result.evidence_bytes > 0
                    && result.working_bytes > 0
                    && result.evidence_bytes <= InspectionLimits::default().max_file_bytes
                    && result.working_bytes <= InspectionLimits::default().max_file_bytes
                    && result
                        .evidence_bytes
                        .checked_add(result.working_bytes)
                        .is_some_and(|n| n <= 1_073_741_824)
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
                "Invalid observed recovered result",
            )?;
            require(
                match &receipt.state.execution {
                    Some(PerformanceSourceExecutionRevision {
                        phase: PerformanceSourceExecutionPhase::Verified { result, .. },
                        ..
                    }) => **result == receipt.observed,
                    Some(PerformanceSourceExecutionRevision {
                        phase: PerformanceSourceExecutionPhase::Started,
                        ..
                    }) => {
                        matches!(command, Command::ReconcilePerformanceSourceAction { .. })
                            && receipt.replayed
                            && !receipt.original_reopened
                    }
                    _ => false,
                },
                "Observed recovery contradicts its recorded phase",
            )
        }
        (
            Reply::PerformanceSourceDecision(receipt),
            Command::SavePerformanceSourceDecision {
                revision,
                payload_sha256,
                decision,
                ..
            },
        ) => {
            encode(receipt, 576 * 1024)?;
            state_shape(&receipt.state)?;
            decision_result(&receipt.state, *revision, payload_sha256, decision)
        }
        (
            Reply::PerformanceSourceAttachment(receipt),
            Command::AttachPerformanceSource { .. }
            | Command::ReconcilePerformanceSourceAttachment { .. },
        ) => {
            encode(
                &(
                    &receipt.requested,
                    &receipt.state,
                    &receipt.publication,
                    receipt.committed,
                    receipt.replayed,
                ),
                576 * 1024,
            )?;
            state_shape(&receipt.state)?;
            receipt
                .publication
                .validate()
                .map_err(|e| ServiceError::rejected(e.to_string()))?;
            require(
                receipt.publication.clip.source.take_id.0 == receipt.state.prepared.action.take_id,
                "Attachment reply changed the recovered original",
            )?;
            if let Some(view) = &receipt.current_document {
                require(
                    receipt.committed && view.document.id() == receipt.publication.project_id.0,
                    "Current document belongs to another publication",
                )?;
            }
            require(
                receipt.current_document.is_none() || receipt.current_document_error.is_none(),
                "Attachment has both a current document and a current document error",
            )
        }
        _ => Err(ServiceError::rejected(
            "Unexpected source-action reply kind",
        )),
    }
}

fn decision_result(
    state: &PerformanceSourceActionState,
    revision: u64,
    payload: &str,
    decision: &PerformanceSourceDecision,
) -> Result<(), ServiceError> {
    require(
        state.revision == revision || revision.checked_add(1) == Some(state.revision),
        "Decision completion changed an unrelated source revision",
    )?;
    if state.revision == revision {
        require(
            state.payload_sha256 == payload,
            "Decision replay changed the requested head",
        )?;
    }
    let event = match decision {
        PerformanceSourceDecision::KeepLocally => {
            require(
                state.decisions.kept_locally,
                "Keep completion did not retain local disposition",
            )?;
            PerformanceSourceDecisionEvent::KeptLocally
        }
        PerformanceSourceDecision::ReturnToReview => {
            require(
                !state.decisions.kept_locally,
                "Return completion did not retain review disposition",
            )?;
            PerformanceSourceDecisionEvent::ReturnedToReview
        }
        PerformanceSourceDecision::PrepareAttachment { choice } => {
            require(
                state.decisions.attachment.as_ref().is_some_and(|a| {
                    *a.choice == **choice
                        && a.status == PerformanceSourceAttachmentStatus::Pending
                        && a.revision == state.revision
                        && a.payload_sha256 == state.payload_sha256
                }),
                "Attachment preparation differs from its immutable request",
            )?;
            PerformanceSourceDecisionEvent::AttachmentPrepared {
                choice: choice.clone(),
            }
        }
        PerformanceSourceDecision::RetireAttachment => {
            let attachment = state
                .decisions
                .attachment
                .as_ref()
                .ok_or_else(|| ServiceError::rejected("Retired attachment is missing"))?;
            require(
                attachment.status == PerformanceSourceAttachmentStatus::Retired,
                "Retirement completion remains eligible for publication",
            )?;
            PerformanceSourceDecisionEvent::AttachmentRetired {
                choice_id: attachment.choice.id.clone(),
            }
        }
    };
    let verified = state
        .execution
        .as_ref()
        .ok_or_else(|| ServiceError::rejected("Decision has no Verified recovery"))?;
    require(
        matches!(
            verified.phase,
            PerformanceSourceExecutionPhase::Verified { .. }
        ),
        "Decision has no Verified recovery",
    )?;
    let row = PerformanceSourceDecisionRevision {
        version: 1,
        id: state.prepared.action.id.clone(),
        prepared_payload_sha256: state.prepared.payload_sha256.clone(),
        verified_payload_sha256: digest(
            b"",
            &encode(verified, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?,
        ),
        decision: event,
    };
    require(
        state.payload_sha256 == digest(b"", &encode(&row, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?),
        "Decision completion differs from the canonical requested event",
    )
}
