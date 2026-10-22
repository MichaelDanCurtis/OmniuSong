//! Metadata decisions retain the immutable copied recovery and every choice.
//! Only the store's fenced publication bridge may append a committed event.
use super::{
    NativeBackend, PerformanceSourceActionReference, PerformanceSourceActionState,
    PerformanceSourceExecutionPhase, PerformanceSourceReview, Reply, ServiceError, digest, encode,
    require, sha, store_failure, validate_id,
};
use omniusong_core::{contracts::OperationId, native::TakeRange};
use omniusong_store::{
    MAX_PERFORMANCE_SOURCE_ACTION_BYTES, PerformanceSourceActionHistory,
    StoredPerformanceSourceAction, TakePublication,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
#[path = "decisions_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceAttachmentChoice {
    pub id: String,
    pub supersedes: Option<String>,
    pub publication: TakePublication,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerformanceSourceDecision {
    KeepLocally,
    ReturnToReview,
    PrepareAttachment {
        choice: Box<PerformanceSourceAttachmentChoice>,
    },
    RetireAttachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerformanceSourceDecisionEvent {
    KeptLocally,
    ReturnedToReview,
    AttachmentPrepared {
        choice: Box<PerformanceSourceAttachmentChoice>,
    },
    AttachmentRetired {
        choice_id: String,
    },
    AttachmentCommitted {
        choice_id: String,
        project_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceDecisionRevision {
    pub version: u32,
    pub id: String,
    pub prepared_payload_sha256: String,
    pub verified_payload_sha256: String,
    pub decision: PerformanceSourceDecisionEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerformanceSourceAttachmentStatus {
    Pending,
    Retired,
    Committed { project_revision: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceAttachmentState {
    /// Identity of the immutable AttachmentPrepared row, even after settlement.
    pub revision: u64,
    pub payload_sha256: String,
    pub choice: Box<PerformanceSourceAttachmentChoice>,
    pub status: PerformanceSourceAttachmentStatus,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceDecisionState {
    pub kept_locally: bool,
    pub attachment: Option<PerformanceSourceAttachmentState>,
}

// Internally tagged serde unit variants accept trailing fields. Decode through
// empty struct variants instead, preserving the public API and exact wire bytes.
impl<'de> Deserialize<'de> for PerformanceSourceDecision {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            KeepLocally {},
            ReturnToReview {},
            PrepareAttachment {
                choice: Box<PerformanceSourceAttachmentChoice>,
            },
            RetireAttachment {},
        }
        Ok(match Wire::deserialize(d)? {
            Wire::KeepLocally {} => Self::KeepLocally,
            Wire::ReturnToReview {} => Self::ReturnToReview,
            Wire::PrepareAttachment { choice } => Self::PrepareAttachment { choice },
            Wire::RetireAttachment {} => Self::RetireAttachment,
        })
    }
}
impl<'de> Deserialize<'de> for PerformanceSourceDecisionEvent {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            KeptLocally {},
            ReturnedToReview {},
            AttachmentPrepared {
                choice: Box<PerformanceSourceAttachmentChoice>,
            },
            AttachmentRetired {
                choice_id: String,
            },
            AttachmentCommitted {
                choice_id: String,
                project_revision: u64,
            },
        }
        Ok(match Wire::deserialize(d)? {
            Wire::KeptLocally {} => Self::KeptLocally,
            Wire::ReturnedToReview {} => Self::ReturnedToReview,
            Wire::AttachmentPrepared { choice } => Self::AttachmentPrepared { choice },
            Wire::AttachmentRetired { choice_id } => Self::AttachmentRetired { choice_id },
            Wire::AttachmentCommitted {
                choice_id,
                project_revision,
            } => Self::AttachmentCommitted {
                choice_id,
                project_revision,
            },
        })
    }
}
impl<'de> Deserialize<'de> for PerformanceSourceAttachmentStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Pending {},
            Retired {},
            Committed { project_revision: u64 },
        }
        Ok(match Wire::deserialize(d)? {
            Wire::Pending {} => Self::Pending,
            Wire::Retired {} => Self::Retired,
            Wire::Committed { project_revision } => Self::Committed { project_revision },
        })
    }
}

pub fn attachment_operation_id(
    action_id: &str,
    choice_id: &str,
) -> Result<OperationId, ServiceError> {
    validate_id(action_id)?;
    validate_id(choice_id)?;
    let mut framed = Vec::with_capacity(80);
    for value in [action_id, choice_id] {
        framed.extend_from_slice(&(value.len() as u64).to_le_bytes());
        framed.extend_from_slice(value.as_bytes());
    }
    Ok(OperationId(format!(
        "os-src-attach:{}",
        digest(b"OmniuSong.source-attachment.v1\0", &framed)
    )))
}

impl PerformanceSourceAttachmentChoice {
    fn validate_shape(&self) -> Result<(), ServiceError> {
        validate_id(&self.id)?;
        if let Some(previous) = &self.supersedes {
            validate_id(previous)?;
        }
        self.publication
            .validate()
            .map_err(|e| ServiceError::rejected(e.to_string()))?;
        require(
            self.publication
                .operation_id
                .0
                .strip_prefix("os-src-attach:")
                .is_some_and(sha),
            "Source attachment needs its reserved versioned operation identity",
        )?;
        encode(self, MAX_PERFORMANCE_SOURCE_ACTION_BYTES).map(|_| ())
    }
}
impl PerformanceSourceDecision {
    /// Bounded request shape only; no source or current-history authority.
    pub fn validate(&self) -> Result<(), ServiceError> {
        if let Self::PrepareAttachment { choice } = self {
            choice.validate_shape()?;
        }
        encode(self, MAX_PERFORMANCE_SOURCE_ACTION_BYTES).map(|_| ())
    }
}

fn verified_hash(state: &PerformanceSourceActionState) -> Result<String, ServiceError> {
    let execution = state
        .execution
        .as_ref()
        .ok_or_else(|| ServiceError::rejected("A decision requires an exact Verified recovery"))?;
    require(
        matches!(
            execution.phase,
            PerformanceSourceExecutionPhase::Verified { .. }
        ),
        "A decision requires an exact Verified recovery",
    )?;
    Ok(digest(
        b"",
        &encode(execution, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?,
    ))
}
fn envelope(
    state: &PerformanceSourceActionState,
    decision: PerformanceSourceDecisionEvent,
) -> Result<PerformanceSourceDecisionRevision, ServiceError> {
    Ok(PerformanceSourceDecisionRevision {
        version: 1,
        id: state.prepared.action.id.clone(),
        prepared_payload_sha256: state.prepared.payload_sha256.clone(),
        verified_payload_sha256: verified_hash(state)?,
        decision,
    })
}
fn pending(state: &PerformanceSourceActionState) -> bool {
    state
        .decisions
        .attachment
        .as_ref()
        .is_some_and(|a| a.status == PerformanceSourceAttachmentStatus::Pending)
}
fn publication_matches(
    choice: &PerformanceSourceAttachmentChoice,
    state: &PerformanceSourceActionState,
    source: &PerformanceSourceReview,
) -> Result<(), ServiceError> {
    choice.validate_shape()?;
    let Some(super::PerformanceSourceExecutionRevision {
        phase: PerformanceSourceExecutionPhase::Verified { result, .. },
        ..
    }) = &state.execution
    else {
        return Err(ServiceError::rejected(
            "Attachment requires the immutable Verified recovery",
        ));
    };
    let checkpoint = source
        .performance
        .checkpoint
        .as_ref()
        .ok_or_else(|| ServiceError::rejected("Retained source has no checkpoint"))?;
    let publication = &choice.publication;
    require(
        publication.operation_id == attachment_operation_id(&state.prepared.action.id, &choice.id)?
            && publication.clip.source.take_id.0 == result.take_id
            && publication.clip.source.manifest_sha256 == result.manifest_sha256
            && publication.clip.source.stream_id == "audio"
            && publication.clip.range
                == (TakeRange::AudioFrames {
                    start: 0,
                    count: checkpoint.audio_end,
                })
            && checkpoint.audio_end > 0
            && publication.clip.position.sample_rate == source.config.info.sample_rate
            && publication.clip.position.frame >= 0
            && publication.clip.edit_source.is_none()
            && publication.clip.fades.is_none(),
        "Publication differs from the exact recovered source, selected audio, placement or policy",
    )
}

/// Fold only rows after the immutable execution prefix. No source/file opens.
pub(super) fn apply_decisions(
    history: &PerformanceSourceActionHistory,
    states: &mut BTreeMap<String, PerformanceSourceActionState>,
    sources: &BTreeMap<String, PerformanceSourceReview>,
) -> Result<(), ServiceError> {
    let mut choices = BTreeSet::new();
    let mut operations: BTreeSet<_> = states
        .values()
        .map(|s| s.prepared.action.recovery_operation_id.0.clone())
        .collect();
    let mut clips: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in history.revisions.iter().filter(|r| r.revision > 3) {
        let state = states
            .get_mut(&row.id)
            .ok_or_else(|| ServiceError::rejected("Decision has no retained source action"))?;
        let source = sources
            .get(&row.id)
            .ok_or_else(|| ServiceError::rejected("Decision has no retained source review"))?;
        require(
            state.revision >= 3
                && row.revision == state.revision + 1
                && row.revision <= 16
                && row.review_blob_sha256 == state.prepared.action.review_blob_sha256
                && row.payload.len() <= MAX_PERFORMANCE_SOURCE_ACTION_BYTES
                && digest(b"", &row.payload) == row.payload_sha256,
            "Source decision history is noncontiguous or changes its immutable review",
        )?;
        let revision: PerformanceSourceDecisionRevision = serde_json::from_slice(&row.payload)
            .map_err(|e| ServiceError::rejected(e.to_string()))?;
        require(
            revision.version == 1
                && revision.id == row.id
                && revision.prepared_payload_sha256 == state.prepared.payload_sha256
                && revision.verified_payload_sha256 == verified_hash(state)?
                && encode(&revision, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)? == row.payload,
            "Decision has an unsupported envelope, canonical payload or recovery reference",
        )?;
        match revision.decision {
            PerformanceSourceDecisionEvent::KeptLocally => {
                require(
                    !state.decisions.kept_locally && (!pending(state) || row.revision < 16),
                    "Keep requires a real disposition toggle and room for pending settlement",
                )?;
                state.decisions.kept_locally = true;
            }
            PerformanceSourceDecisionEvent::ReturnedToReview => {
                require(
                    state.decisions.kept_locally && (!pending(state) || row.revision < 16),
                    "Return requires a real disposition toggle and room for pending settlement",
                )?;
                state.decisions.kept_locally = false;
            }
            PerformanceSourceDecisionEvent::AttachmentPrepared { choice } => {
                require(
                    !state.decisions.kept_locally && row.revision + 2 <= 16,
                    "Review disposition and room for choice, settlement and Keep are required",
                )?;
                match &state.decisions.attachment {
                    None => require(
                        choice.supersedes.is_none(),
                        "Initial attachment cannot supersede another choice",
                    )?,
                    Some(previous) => require(
                        previous.status == PerformanceSourceAttachmentStatus::Retired
                            && choice.supersedes.as_deref() == Some(previous.choice.id.as_str()),
                        "Replacement requires the exact previous retired attachment choice",
                    )?,
                }
                publication_matches(&choice, state, source)?;
                require(
                    choices.insert(choice.id.clone())
                        && operations.insert(choice.publication.operation_id.0.clone())
                        && clips
                            .entry(row.id.clone())
                            .or_default()
                            .insert(choice.publication.clip.id.clone()),
                    "Attachment choice, operation or new clip identity was already retained",
                )?;
                state.decisions.attachment = Some(PerformanceSourceAttachmentState {
                    revision: row.revision,
                    payload_sha256: row.payload_sha256.clone(),
                    choice,
                    status: PerformanceSourceAttachmentStatus::Pending,
                });
            }
            PerformanceSourceDecisionEvent::AttachmentRetired { choice_id } => {
                let attachment =
                    state.decisions.attachment.as_mut().ok_or_else(|| {
                        ServiceError::rejected("No pending attachment can retire")
                    })?;
                require(
                    attachment.status == PerformanceSourceAttachmentStatus::Pending
                        && attachment.choice.id == choice_id,
                    "Retirement must retain the exact pending choice",
                )?;
                attachment.status = PerformanceSourceAttachmentStatus::Retired;
            }
            PerformanceSourceDecisionEvent::AttachmentCommitted {
                choice_id,
                project_revision,
            } => {
                require(
                    !state.decisions.kept_locally,
                    "Publication requires Review disposition",
                )?;
                let attachment =
                    state.decisions.attachment.as_mut().ok_or_else(|| {
                        ServiceError::rejected("No pending attachment can commit")
                    })?;
                require(
                    attachment.status == PerformanceSourceAttachmentStatus::Pending
                        && attachment.choice.id == choice_id
                        && attachment
                            .choice
                            .publication
                            .expected_revision
                            .checked_add(1)
                            == Some(project_revision),
                    "Commit differs from its pending immutable choice or project revision",
                )?;
                attachment.status =
                    PerformanceSourceAttachmentStatus::Committed { project_revision };
            }
        }
        state.revision = row.revision;
        state.payload_sha256 = row.payload_sha256.clone();
    }
    Ok(())
}

pub(super) fn committed_payload(
    state: &PerformanceSourceActionState,
    project_revision: u64,
) -> Result<Vec<u8>, ServiceError> {
    require(
        !state.decisions.kept_locally && pending(state) && state.revision < 16,
        "Publication requires the current pending choice in Review disposition and settlement capacity",
    )?;
    let attachment = state
        .decisions
        .attachment
        .as_ref()
        .ok_or_else(|| ServiceError::rejected("No pending attachment"))?;
    require(
        attachment
            .choice
            .publication
            .expected_revision
            .checked_add(1)
            == Some(project_revision),
        "Publication project revision differs from its immutable choice",
    )?;
    encode(
        &envelope(
            state,
            PerformanceSourceDecisionEvent::AttachmentCommitted {
                choice_id: attachment.choice.id.clone(),
                project_revision,
            },
        )?,
        MAX_PERFORMANCE_SOURCE_ACTION_BYTES,
    )
}

fn requested_event(
    state: &PerformanceSourceActionState,
    request: &PerformanceSourceDecision,
) -> Result<PerformanceSourceDecisionEvent, ServiceError> {
    Ok(match request {
        PerformanceSourceDecision::KeepLocally => PerformanceSourceDecisionEvent::KeptLocally,
        PerformanceSourceDecision::ReturnToReview => {
            PerformanceSourceDecisionEvent::ReturnedToReview
        }
        PerformanceSourceDecision::PrepareAttachment { choice } => {
            PerformanceSourceDecisionEvent::AttachmentPrepared {
                choice: choice.clone(),
            }
        }
        PerformanceSourceDecision::RetireAttachment => {
            PerformanceSourceDecisionEvent::AttachmentRetired {
                choice_id: state
                    .decisions
                    .attachment
                    .as_ref()
                    .ok_or_else(|| ServiceError::rejected("No attachment choice can retire"))?
                    .choice
                    .id
                    .clone(),
            }
        }
    })
}

impl NativeBackend {
    fn save_decision_row(
        &mut self,
        state: &PerformanceSourceActionState,
        raw: &[u8],
        payload: &[u8],
        history: &PerformanceSourceActionHistory,
        retire: bool,
    ) -> Result<StoredPerformanceSourceAction, ServiceError> {
        let revision: PerformanceSourceDecisionRevision =
            serde_json::from_slice(payload).map_err(|e| ServiceError::rejected(e.to_string()))?;
        let unpublished = if retire {
            Some(
                state
                    .decisions
                    .attachment
                    .as_ref()
                    .ok_or_else(|| ServiceError::rejected("No pending attachment"))?
                    .choice
                    .publication
                    .clone(),
            )
        } else if let PerformanceSourceDecisionEvent::AttachmentPrepared { choice } =
            revision.decision
        {
            Some(choice.publication)
        } else {
            None
        };
        let row = if let Some(publication) = &unpublished {
            self.store()?
                .save_performance_source_action_if_history_matches_and_unpublished(
                    &state.prepared.action.id,
                    Some(state.revision),
                    raw,
                    payload,
                    history,
                    publication,
                )
        } else {
            self.store()?
                .save_performance_source_action_if_history_matches(
                    &state.prepared.action.id,
                    Some(state.revision),
                    raw,
                    payload,
                    history,
                )
        };
        row.map_err(|e| store_failure(e, true))
    }

    /// Metadata-only cross-ledger operation uniqueness, including unlinked actions.
    pub(crate) fn validate_source_decision_operations(
        &mut self,
        history: &PerformanceSourceActionHistory,
    ) -> Result<(), ServiceError> {
        if !history.revisions.iter().any(|r| r.revision > 3) {
            return Ok(());
        }
        self.list_performance_intentions()?;
        let intents = self
            .store()?
            .list_performance_intents()
            .map_err(|e| store_failure(e, false))?;
        let mut retained = BTreeSet::new();
        for row in intents {
            let intent: crate::PerformanceIntent = serde_json::from_slice(&row.payload)
                .map_err(|e| ServiceError::rejected(e.to_string()))?;
            if let Some(publication) = intent.attachment {
                retained.insert(publication.operation_id.0);
            }
        }
        for row in history.revisions.iter().filter(|r| r.revision > 3) {
            let revision: PerformanceSourceDecisionRevision = serde_json::from_slice(&row.payload)
                .map_err(|e| ServiceError::rejected(e.to_string()))?;
            if let PerformanceSourceDecisionEvent::AttachmentPrepared { choice } = revision.decision
            {
                require(
                    !retained.contains(&choice.publication.operation_id.0),
                    "Source publication operation already belongs to a historical recording intention",
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn save_source_decision(
        &mut self,
        id: &str,
        revision: u64,
        payload_sha256: &str,
        decision: &PerformanceSourceDecision,
    ) -> Result<Reply, ServiceError> {
        decision.validate()?;
        validate_id(id)?;
        require(
            (3..=16).contains(&revision) && sha(payload_sha256),
            "Choose an exact retained source decision head",
        )?;
        let loaded = self.load_execution_history()?;
        let state = loaded
            .states
            .iter()
            .find(|s| s.prepared.action.id == id)
            .cloned()
            .ok_or_else(|| ServiceError::rejected("Source decision action is missing"))?;
        require(
            loaded.history.revisions.iter().any(|r| {
                r.id == id && r.revision == revision && r.payload_sha256 == payload_sha256
            }),
            "Source decision reference has no exact retained row",
        )?;
        let payload = encode(
            &envelope(&state, requested_event(&state, decision)?)?,
            MAX_PERFORMANCE_SOURCE_ACTION_BYTES,
        )?;
        let current = loaded
            .history
            .revisions
            .iter()
            .find(|r| r.id == id && r.revision == state.revision)
            .ok_or_else(|| ServiceError::rejected("Source decision current row is missing"))?;
        // A retry can name the exact prior head on which this CURRENT row was
        // based, or that current row itself. Older historical choices never
        // reapply across an intervening event.
        if current.payload == payload
            && (revision == state.revision || revision.checked_add(1) == Some(state.revision))
        {
            let row = self.save_decision_row(
                &state,
                loaded.raw_review(id)?,
                &payload,
                &loaded.history,
                matches!(decision, PerformanceSourceDecision::RetireAttachment),
            )?;
            super::committed(&row, &state, state.revision, &payload)?;
            return Ok(Reply::PerformanceSourceDecision(Box::new(
                crate::PerformanceSourceDecisionReceipt {
                    requested: PerformanceSourceActionReference::new(id, revision, payload_sha256),
                    state,
                },
            )));
        }
        require(
            revision == state.revision && payload_sha256 == state.payload_sha256,
            "Source decision changed; review its exact current head",
        )?;
        let next_revision = state
            .revision
            .checked_add(1)
            .filter(|r| *r <= 16)
            .ok_or_else(|| {
                ServiceError::rejected("Source action has all sixteen retained revisions")
            })?;
        let raw = loaded.raw_review(id)?;
        let mut proposed = loaded.history.clone();
        proposed.revisions.push(StoredPerformanceSourceAction {
            id: id.into(),
            revision: next_revision,
            review_blob_sha256: state.prepared.action.review_blob_sha256.clone(),
            payload: payload.clone(),
            payload_sha256: digest(b"", &payload),
        });
        proposed
            .revisions
            .sort_by(|a, b| a.id.cmp(&b.id).then(a.revision.cmp(&b.revision)));
        let prepared: Vec<_> = loaded.states.iter().map(|s| s.prepared.clone()).collect();
        let expected = super::validate_history(&proposed, &prepared)?
            .into_iter()
            .find(|s| s.prepared.action.id == id)
            .ok_or_else(|| ServiceError::rejected("Proposed decision state is missing"))?;
        self.validate_source_decision_operations(&proposed)?;
        let row = self.save_decision_row(
            &state,
            raw,
            &payload,
            &loaded.history,
            matches!(decision, PerformanceSourceDecision::RetireAttachment),
        )?;
        super::committed(&row, &state, next_revision, &payload)?;
        Ok(Reply::PerformanceSourceDecision(Box::new(
            crate::PerformanceSourceDecisionReceipt {
                requested: PerformanceSourceActionReference::new(id, revision, payload_sha256),
                state: expected,
            },
        )))
    }
}
