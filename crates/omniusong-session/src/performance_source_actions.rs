//! Durable Prepared requests only. No source opens, artifacts, recovery or audio.
use crate::{
    NativeBackend, PerformanceIntent, PerformanceSourceReview, Reply, ServiceError, store_failure,
};
use omniusong_core::contracts::{OperationId, Validate};
use omniusong_engine::{
    LiveId,
    performance_records::{LocalRecord, PerformanceRecord},
};
use omniusong_recording::{InspectionLimits, TakeState};
use omniusong_store::{
    MAX_PERFORMANCE_SOURCE_ACTION_BYTES, MAX_PERFORMANCE_SOURCE_REVIEW_BYTES,
    PerformanceSourceActionHistory,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceSourceActionKind {
    RecoverCopy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceIntentReference {
    pub id: LiveId,
    pub revision: u64,
    pub payload_sha256: String,
}

#[cfg(test)]
#[path = "performance_source_actions_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedPerformanceSourceAction {
    pub version: u32,
    pub id: String,
    pub kind: PerformanceSourceActionKind,
    pub stage_id: String,
    pub take_id: String,
    pub review_blob_sha256: String,
    pub admission_sha256: String,
    pub recovery_operation_id: OperationId,
    pub policy_version: u32,
    pub origin: Option<PerformanceSourceIntentReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedPerformanceSourceActionReceipt {
    pub revision: u64,
    pub action: PreparedPerformanceSourceAction,
    pub payload_sha256: String,
}

fn require(ok: bool, message: &str) -> Result<(), ServiceError> {
    if ok {
        Ok(())
    } else {
        Err(ServiceError::rejected(message))
    }
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_id(value: &str) -> Result<(), ServiceError> {
    // Here the identity is request metadata, not an attempted source read.
    crate::performance_sources::validate_stage_id(value)
        .map_err(|error| ServiceError::rejected(error.message))
}
fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}
struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "Prepared source metadata exceeds its byte limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, ServiceError> {
    let mut output = BoundedBytes {
        bytes: vec![],
        limit,
    };
    serde_json::to_writer(&mut output, value).map_err(|e| ServiceError::rejected(e.to_string()))?;
    Ok(output.bytes)
}
fn review(raw: &[u8]) -> Result<(PerformanceSourceReview, String), ServiceError> {
    require(
        !raw.is_empty() && raw.len() <= MAX_PERFORMANCE_SOURCE_REVIEW_BYTES,
        "Retain an exact source review of at most64KiB",
    )?;
    let mut review: PerformanceSourceReview =
        serde_json::from_slice(raw).map_err(|e| ServiceError::rejected(e.to_string()))?;
    validate_id(&review.stage_id)?;
    review.config.validate().map_err(ServiceError::rejected)?;
    require(
        review.version == 1
            && matches!(review.profile_version, 1 | 2)
            && review.take_id == format!("performance-{}", review.stage_id)
            && review.config.take_id.0 == review.take_id
            && review.journal_path.is_absolute()
            && review.journal_path.as_os_str().len() <= 4096
            && !review.journal_path.to_string_lossy().contains('\0')
            && review.byte_length > 0
            && review.byte_length <= InspectionLimits::default().max_file_bytes
            && review.performance.unknown_tail
            && sha(&review.performance.descriptor_sha256)
            && sha(&review.performance.original_rows_sha256)
            && sha(&review.performance.journal_sha256_at_review)
            && sha(&review.review_sha256),
        "Invalid retained performance review identity or version",
    )?;
    require(
        review.manifest_sha256.is_none()
            && !review.manifest_interrupted
            && matches!(review.state, TakeState::Recording | TakeState::Interrupted),
        "Copy recovery preparation requires an unsealed source review; an existing manifest is a separate reconciliation workflow",
    )?;
    let selection = review
        .performance
        .selection
        .as_ref()
        .ok_or_else(|| ServiceError::rejected("No complete checkpoint was selected"))?;
    let checkpoint = review
        .performance
        .checkpoint
        .as_ref()
        .ok_or_else(|| ServiceError::rejected("No complete checkpoint was retained"))?;
    LocalRecord {
        version: review.profile_version,
        record: PerformanceRecord::Checkpoint {
            checkpoint: checkpoint.clone(),
        },
    }
    .validate()
    .map_err(ServiceError::rejected)?;
    if let Some(finish) = &review.performance.finish_cut {
        require(
            finish.fence.scope.worker_generation == review.config.info.scope.worker_generation
                && finish.fence.scope.session_id == review.config.info.scope.session_id
                && finish.fence.scope.panic_generation >= review.config.info.scope.panic_generation,
            "Prepared Finish observation contradicts the initial performance scope",
        )?;
        LocalRecord {
            version: review.profile_version,
            record: PerformanceRecord::Finish {
                callback_index: finish.callback_index,
                finish: *finish,
            },
        }
        .validate()
        .map_err(ServiceError::rejected)?;
    }
    require(
        selection.chunk_count > 0
            && selection.chunk_count <= review.performance.received_generic_chunks
            && sha(&selection.chunk_index_sha256)
            && checkpoint.audio_end > 0
            && review
                .performance
                .checkpoint_sha256
                .as_deref()
                .is_some_and(sha)
            && (review.profile_version == 2 || review.performance.finish_cut.is_none()),
        "Copy recovery preparation needs a positive selected checkpoint with supported semantics",
    )?;
    require(
        !checkpoint.chunks.is_empty()
            && checkpoint.scope_after.worker_generation
                == review.config.info.scope.worker_generation
            && checkpoint.scope_after.session_id == review.config.info.scope.session_id
            && checkpoint.scope_after.panic_generation >= review.config.info.scope.panic_generation,
        "Prepared checkpoint contradicts the initial performance scope",
    )?;
    let usage = review.usage;
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
        "Retained review exceeds the supported inspection policy ceilings",
    )?;
    let claimed = std::mem::take(&mut review.review_sha256);
    let expected = digest(
        b"OmniuSong.performance-source-review.v1\0",
        &encode(&review, MAX_PERFORMANCE_SOURCE_REVIEW_BYTES)?,
    );
    require(
        claimed == expected,
        "Retained source review digest differs from its full typed observation",
    )?;
    review.review_sha256 = claimed;
    // Explicit versioned projection. Diagnostic usage and the review digest are
    // excluded; the complete original bytes remain independently stored.
    let projection = (
        1u32,
        &review.stage_id,
        &review.journal_path,
        &review.take_id,
        review.profile_version,
        &review.config,
        review.state,
        review.byte_length,
        &review.manifest_sha256,
        review.manifest_interrupted,
        review.verified_payload_bytes,
        &review.generic_problems,
        &review.performance,
    );
    let admission = digest(
        b"OmniuSong.performance-source-admission.v1\0",
        &encode(&projection, MAX_PERFORMANCE_SOURCE_REVIEW_BYTES)?,
    );
    Ok((review, admission))
}

impl PreparedPerformanceSourceAction {
    /// Shapes a request from retained review bytes; does not re-open the source.
    pub fn prepare(
        id: String,
        review_bytes: &[u8],
        recovery_operation_id: OperationId,
        origin: Option<PerformanceSourceIntentReference>,
    ) -> Result<Self, ServiceError> {
        let (review, admission_sha256) = review(review_bytes)?;
        let action = Self {
            version: 1,
            id,
            kind: PerformanceSourceActionKind::RecoverCopy,
            stage_id: review.stage_id,
            take_id: review.take_id,
            review_blob_sha256: digest(b"", review_bytes),
            admission_sha256,
            recovery_operation_id,
            policy_version: 1,
            origin,
        };
        action.validate()?;
        Ok(action)
    }
    pub fn validate(&self) -> Result<(), ServiceError> {
        validate_id(&self.id)?;
        validate_id(&self.stage_id)?;
        require(
            self.version == 1
                && self.policy_version == 1
                && self.take_id == format!("performance-{}", self.stage_id)
                && sha(&self.review_blob_sha256)
                && sha(&self.admission_sha256),
            "Invalid Prepared source action version, policy or identity",
        )?;
        self.recovery_operation_id
            .validate()
            .map_err(|e| ServiceError::rejected(e.to_string()))?;
        if let Some(origin) = &self.origin {
            require(
                origin.id.is_valid()
                    && (1..=16).contains(&origin.revision)
                    && sha(&origin.payload_sha256),
                "Invalid historical recording-intent reference",
            )?;
        }
        encode(self, MAX_PERFORMANCE_SOURCE_ACTION_BYTES).map(|_| ())
    }
}

fn validate_review(
    action: &PreparedPerformanceSourceAction,
    bytes: &[u8],
    root: &Path,
) -> Result<PerformanceSourceReview, ServiceError> {
    action.validate()?;
    let rebuilt = PreparedPerformanceSourceAction::prepare(
        action.id.clone(),
        bytes,
        action.recovery_operation_id.clone(),
        action.origin.clone(),
    )?;
    require(
        *action == rebuilt,
        "Prepared source choice differs from its exact retained review",
    )?;
    let (review, _) = review(bytes)?;
    require(
        review.journal_path
            == root
                .join("captures")
                .join(&action.stage_id)
                .join("take.sqlite3"),
        "Prepared review path differs from this store's owned original stage",
    )?;
    Ok(review)
}

fn decode(
    history: &PerformanceSourceActionHistory,
    root: &Path,
) -> Result<Vec<PreparedPerformanceSourceActionReceipt>, ServiceError> {
    let reviews: BTreeMap<_, _> = history.reviews.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut result = Vec::new();
    let mut operations = BTreeSet::new();
    for row in &history.revisions {
        let action: PreparedPerformanceSourceAction = serde_json::from_slice(&row.payload)
            .map_err(|e| ServiceError::rejected(e.to_string()))?;
        require(
            row.revision == 1
                && row.id == action.id
                && row.review_blob_sha256 == action.review_blob_sha256,
            "Stored Prepared history contains an unsupported phase, revision or reference",
        )?;
        require(
            encode(&action, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)? == row.payload,
            "Stored Prepared action differs from its versioned canonical payload",
        )?;
        let blob = reviews
            .get(row.id.as_str())
            .ok_or_else(|| ServiceError::rejected("Stored Prepared review is missing"))?;
        validate_review(&action, &blob.payload, root)?;
        require(
            operations.insert(action.recovery_operation_id.0.clone()),
            "Different Prepared actions claim the same recovery operation",
        )?;
        result.push(PreparedPerformanceSourceActionReceipt {
            revision: row.revision,
            action,
            payload_sha256: row.payload_sha256.clone(),
        });
    }
    encode(&result, 576 * 1024)?;
    Ok(result)
}

impl NativeBackend {
    fn validate_source_origins(
        &mut self,
        actions: &[PreparedPerformanceSourceActionReceipt],
        history: &PerformanceSourceActionHistory,
    ) -> Result<(), ServiceError> {
        if !actions.iter().any(|r| r.action.origin.is_some()) {
            return Ok(());
        }
        // This validates every typed recording-intent transition and any released
        // publication proof, before selecting an exact historical row below.
        self.list_performance_intentions()?;
        let rows = self
            .store()?
            .list_performance_intents()
            .map_err(|e| store_failure(e, false))?;
        for action in actions.iter().map(|r| &r.action) {
            let Some(origin) = &action.origin else {
                continue;
            };
            let id: String = origin.id.0.iter().map(|b| format!("{b:02x}")).collect();
            let row = rows
                .iter()
                .find(|r| {
                    r.id == id
                        && r.revision == origin.revision
                        && r.payload_sha256 == origin.payload_sha256
                })
                .ok_or_else(|| {
                    ServiceError::rejected(
                        "Prepared source reference has no exact historical recording intention",
                    )
                })?;
            let intent: PerformanceIntent = serde_json::from_slice(&row.payload)
                .map_err(|e| ServiceError::rejected(e.to_string()))?;
            let blob = history
                .reviews
                .iter()
                .find(|r| r.id == action.id)
                .ok_or_else(|| ServiceError::rejected("Prepared source review is missing"))?;
            let (review, _) = review(&blob.payload)?;
            require(
                intent.start.directory_name() == action.stage_id
                    && intent.start.take_id() == action.take_id
                    && intent.journal_path == review.journal_path
                    && intent.start.gui_clock == review.config.gui_clock
                    && intent.start.id == review.config.info.scope.session_id
                    && intent
                        .descriptor_sha256
                        .as_ref()
                        .is_none_or(|s| *s == review.performance.descriptor_sha256),
                "Prepared review contradicts its linked historical recording intention",
            )?;
        }
        Ok(())
    }
    pub(super) fn list_prepared_source_actions(&mut self) -> Result<Reply, ServiceError> {
        let root = self.store()?.root().to_owned();
        let history = self
            .store()?
            .list_performance_source_actions()
            .map_err(|e| store_failure(e, false))?;
        let actions = decode(&history, &root)?;
        self.validate_source_origins(&actions, &history)?;
        Ok(Reply::PreparedPerformanceSourceActions(actions))
    }
    pub(super) fn save_prepared_source_action(
        &mut self,
        expected_revision: Option<u64>,
        action: &PreparedPerformanceSourceAction,
        raw: &[u8],
    ) -> Result<Reply, ServiceError> {
        let root = self.store()?.root().to_owned();
        validate_review(action, raw, &root)?;
        let history = self
            .store()?
            .list_performance_source_actions()
            .map_err(|e| store_failure(e, false))?;
        let mut previous = decode(&history, &root)?;
        self.validate_source_origins(&previous, &history)?;
        if let Some(old) = previous.iter().find(|r| r.action.id == action.id) {
            require(
                old.action == *action,
                "Prepared source choice is immutable; do not replace an uncertain operation",
            )?;
        } else {
            require(
                !previous
                    .iter()
                    .any(|r| r.action.recovery_operation_id == action.recovery_operation_id),
                "Recovery operation already belongs to another Prepared source action",
            )?;
        }
        // Validate an optional new origin against the same exact review, without
        // inserting anything or opening the original.
        let mut proposed = history.clone();
        if !proposed.reviews.iter().any(|r| r.id == action.id) {
            proposed
                .reviews
                .push(omniusong_store::StoredPerformanceSourceReview {
                    id: action.id.clone(),
                    payload: raw.to_vec(),
                    payload_sha256: action.review_blob_sha256.clone(),
                });
        }
        previous.push(PreparedPerformanceSourceActionReceipt {
            revision: 1,
            action: action.clone(),
            payload_sha256: String::new(),
        });
        self.validate_source_origins(&previous, &proposed)?;
        let bytes = encode(action, MAX_PERFORMANCE_SOURCE_ACTION_BYTES)?;
        let row = self
            .store()?
            .save_performance_source_action_if_history_matches(
                &action.id,
                expected_revision,
                raw,
                &bytes,
                &history,
            )
            .map_err(|e| store_failure(e, true))?;
        Ok(Reply::PreparedPerformanceSourceAction(Box::new(
            PreparedPerformanceSourceActionReceipt {
                revision: row.revision,
                action: action.clone(),
                payload_sha256: row.payload_sha256,
            },
        )))
    }
}
