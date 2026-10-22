//! Durable exact studio-evidence requests. Artifact work is always explicit;
//! startup/list/read never execute or inspect archives.
use crate::{FailureKind, ServiceError};
use omniusong_recording::{InspectionBudget, InspectionResource, RecordingError};
use omniusong_store::{EvidenceArchiveReceipt, EvidenceRestoreReceipt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

mod journal;
mod runner;
#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod tests;

pub const MAX_EVIDENCE_JOBS: usize = 128;
pub const MAX_EVIDENCE_JOB_HEADER_BYTES: usize = 4096;
pub const MAX_EVIDENCE_JOB_REQUEST_BYTES: usize = 16_384;
pub const MAX_EVIDENCE_JOB_STARTED_BYTES: usize = 1024;
pub const MAX_EVIDENCE_JOB_RECORD_BYTES: usize = 16_384;
/// A real result may be larger than its durable envelope; return it correlated
/// with an independent metadata-persistence problem rather than lose credit.
pub const MAX_EVIDENCE_OUTCOME_REPLY_BYTES: usize = 32_768;
pub const MAX_EVIDENCE_JOB_OBSERVATIONS: usize = 8;
pub const MAX_EVIDENCE_JOB_MESSAGE_BYTES: usize = 2048;
pub const MAX_EVIDENCE_JOB_LIST_BYTES: usize = 65_536;
pub const MAX_EVIDENCE_JOB_VIEW_BYTES: usize = 262_144;
/// Sixteen bounded byte passes, sixty-four path/stamp observations and one
/// maximum job-member scan. Reserved before artifacts; compiled ceilings stay.
pub const EVIDENCE_JOB_FINALIZATION_WORK: u64 = 16 * MAX_EVIDENCE_JOB_RECORD_BYTES as u64
    + 64 * (4096 + 512)
    + (3 + MAX_EVIDENCE_JOB_OBSERVATIONS as u64 + 1) * (4096 + 160);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceJobRequest {
    pub version: u32,
    pub job_id: String,
    pub origin_root: String,
    pub operation: EvidenceOperation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceOperation {
    Export {
        scope: String,
        export_id: String,
        destination: PathBuf,
    },
    Restore {
        archive_path: PathBuf,
        directory: PathBuf,
        expected_archive_sha256: String,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceProblemKind {
    Rejected,
    Cancelled,
    Limit,
    Busy,
    Unavailable,
    Io,
    Sqlite,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceResourceOrigin {
    Evidence,
    LiveCatalog,
    BackupTransfer,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceBudgetStop {
    Cancelled,
    Limit {
        resource: InspectionResource,
        limit: u64,
        attempted: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceMessage {
    pub message: String,
    pub message_truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceProblem {
    /// Original operation kind and origin survive a separately observed stop.
    pub kind: EvidenceProblemKind,
    pub origin: EvidenceResourceOrigin,
    pub detail: EvidenceMessage,
    pub sqlite_code: Option<i32>,
    pub budget_stop: Option<EvidenceBudgetStop>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceOutcome {
    Exported { receipt: EvidenceArchiveReceipt },
    Restored { receipt: EvidenceRestoreReceipt },
    Stopped { problem: EvidenceProblem },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceJobPhase {
    InvalidRequest,
    Unresolved,
    Started,
    HistoricalComplete,
    Stopped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceObservationStatus {
    VerifiedComplete,
    Unresolved,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceObservation {
    pub sequence: u32,
    pub previous_history_sha256: String,
    pub outcome: EvidenceOutcome,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceMetadataProblem {
    pub slot: String,
    pub detail: EvidenceMessage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceJobView {
    pub job_id: String,
    pub directory: PathBuf,
    pub request: Option<EvidenceJobRequest>,
    pub request_sha256: Option<String>,
    pub history_sha256: String,
    pub started: bool,
    pub terminal: Option<EvidenceOutcome>,
    pub observations: Vec<EvidenceObservation>,
    /// Capacity consumed through the highest occupied sequence, including gaps.
    pub occupied_observations: usize,
    pub metadata_problems: Vec<EvidenceMetadataProblem>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceJobSummary {
    pub job_id: String,
    pub request_sha256: Option<String>,
    pub history_sha256: String,
    pub phase: EvidenceJobPhase,
    pub latest_observation: Option<EvidenceObservationStatus>,
    pub metadata_problem_count: usize,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceJobList {
    pub jobs: Vec<EvidenceJobSummary>,
}

/// The reply describes this command separately from historical journal state.
/// A terminal-write failure never hides an independently observed live fatal.
#[derive(Debug, Clone)]
pub struct EvidenceJobReply {
    pub job_id: String,
    pub request_sha256: String,
    pub history_sha256: Option<String>,
    pub attempted: bool,
    pub outcome: Option<EvidenceOutcome>,
    pub metadata_problem: Option<EvidenceMessage>,
    pub catalog_retirement: Option<ServiceError>,
}

pub(super) fn require(ok: bool, message: &str) -> Result<(), ServiceError> {
    if ok {
        Ok(())
    } else {
        Err(ServiceError::rejected(message))
    }
}
pub(super) fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && id.bytes().any(|b| b != b'0')
}
pub(super) fn valid_sha(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn path_text(path: &Path) -> Result<&str, ServiceError> {
    let text = path
        .to_str()
        .ok_or_else(|| ServiceError::rejected("Evidence job paths must be UTF-8"))?;
    require(
        path.is_absolute()
            && path.file_name().is_some()
            && text.len() <= 4096
            && !text.chars().any(char::is_control)
            && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
            && !path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)),
        "Evidence job paths need exact bounded absolute text without aliases or control characters",
    )?;
    Ok(text)
}
impl EvidenceJobRequest {
    pub fn validate(&self) -> Result<(), ServiceError> {
        require(
            self.version == 1 && valid_id(&self.job_id),
            "Unsupported evidence request version or job identity",
        )?;
        path_text(Path::new(&self.origin_root))?;
        match &self.operation {
            EvidenceOperation::Export {
                scope,
                export_id,
                destination,
            } => {
                require(
                    scope == omniusong_store::EVIDENCE_SCOPE && valid_id(export_id),
                    "Evidence export needs its exact full-root scope and identity",
                )?;
                path_text(destination)?;
            }
            EvidenceOperation::Restore {
                archive_path,
                directory,
                expected_archive_sha256,
            } => {
                path_text(archive_path)?;
                path_text(directory)?;
                require(
                    valid_sha(expected_archive_sha256),
                    "Evidence restore needs the exact reviewed lowercase SHA256",
                )?;
            }
        }
        check_bounded(self, MAX_EVIDENCE_JOB_REQUEST_BYTES)
    }
    pub fn fingerprint(&self) -> Result<String, ServiceError> {
        self.validate()?;
        let bytes = encode_bounded(self, MAX_EVIDENCE_JOB_REQUEST_BYTES)?;
        Ok(domain_sha(b"OmniuSong.EvidenceJobRequest.v1\0", &bytes))
    }
    pub(super) fn destination(&self) -> &Path {
        match &self.operation {
            EvidenceOperation::Export { destination, .. } => destination,
            EvidenceOperation::Restore { directory, .. } => directory,
        }
    }
}
impl EvidenceMessage {
    pub(super) fn new(message: impl AsRef<str>) -> Self {
        let message = message.as_ref();
        let mut end = message.len().min(MAX_EVIDENCE_JOB_MESSAGE_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            message: message[..end].to_owned(),
            message_truncated: end < message.len(),
        }
    }
    pub(super) fn validate(&self) -> Result<(), ServiceError> {
        require(
            self.message.len() <= MAX_EVIDENCE_JOB_MESSAGE_BYTES,
            "Evidence explanatory message exceeds its bound",
        )
    }
}
impl EvidenceOutcome {
    pub fn completed(&self) -> bool {
        !matches!(self, Self::Stopped { .. })
    }
    pub(super) fn validate_for(&self, request: &EvidenceJobRequest) -> Result<(), ServiceError> {
        let receipt = |r: &EvidenceArchiveReceipt| {
            path_text(&r.path)?;
            require(
                valid_id(&r.export_id)
                    && valid_sha(&r.sha256)
                    && r.byte_length > 56
                    && r.byte_length <= 536_870_912
                    && r.file_count > 0
                    && r.file_count
                        .checked_add(r.directory_count)
                        .is_some_and(|n| n <= omniusong_store::MAX_EVIDENCE_ENTRIES as u64)
                    && r.payload_bytes <= r.byte_length - 57,
                "Invalid bounded evidence receipt",
            )
        };
        match (self, &request.operation) {
            (
                Self::Exported { receipt: r },
                EvidenceOperation::Export {
                    export_id,
                    destination,
                    ..
                },
            ) => {
                receipt(r)?;
                require(
                    r.export_id == *export_id && r.path.as_os_str() == destination.as_os_str(),
                    "Evidence export result differs from its exact request",
                )?;
            }
            (
                Self::Restored { receipt: r },
                EvidenceOperation::Restore {
                    archive_path,
                    directory,
                    expected_archive_sha256,
                },
            ) => {
                receipt(&r.archive)?;
                path_text(&r.directory)?;
                require(
                    r.directory.as_os_str() == directory.as_os_str()
                        && r.archive.path.as_os_str() == archive_path.as_os_str()
                        && r.archive.sha256 == *expected_archive_sha256,
                    "Evidence restore result differs from its exact reviewed request",
                )?;
            }
            (Self::Stopped { problem }, _) => {
                problem.detail.validate()?;
            }
            _ => {
                return Err(ServiceError::rejected(
                    "Evidence result operation differs from its request",
                ));
            }
        }
        check_bounded(self, MAX_EVIDENCE_OUTCOME_REPLY_BYTES)
    }
}
impl EvidenceJobView {
    pub(super) fn latest_recorded_outcome(&self) -> Option<&EvidenceOutcome> {
        if self.occupied_observations == 0 {
            self.terminal.as_ref()
        } else {
            self.observations
                .last()
                .filter(|o| o.sequence as usize == self.occupied_observations)
                .map(|o| &o.outcome)
        }
    }
    pub(super) fn summary(&self) -> EvidenceJobSummary {
        let phase = if self.request.is_none() {
            EvidenceJobPhase::InvalidRequest
        } else if let Some(outcome) = &self.terminal {
            if outcome.completed() {
                EvidenceJobPhase::HistoricalComplete
            } else {
                EvidenceJobPhase::Stopped
            }
        } else if self.started {
            EvidenceJobPhase::Started
        } else {
            EvidenceJobPhase::Unresolved
        };
        EvidenceJobSummary {
            job_id: self.job_id.clone(),
            request_sha256: self.request_sha256.clone(),
            history_sha256: self.history_sha256.clone(),
            phase,
            latest_observation: if self.occupied_observations == 0 {
                None
            } else {
                Some(self.latest_recorded_outcome().map_or(
                    EvidenceObservationStatus::Unresolved,
                    |outcome| {
                        if outcome.completed() {
                            EvidenceObservationStatus::VerifiedComplete
                        } else {
                            EvidenceObservationStatus::Unresolved
                        }
                    },
                ))
            },
            metadata_problem_count: self.metadata_problems.len(),
        }
    }
}
pub(super) fn domain_sha(domain: &[u8], bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}
pub(super) fn check_bounded(value: &impl Serialize, maximum: usize) -> Result<(), ServiceError> {
    struct Count(usize, usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.1.saturating_sub(self.0) {
                return Err(std::io::Error::other(
                    "Evidence metadata exceeds its compiled byte bound",
                ));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Count(0, maximum), value)
        .map_err(|error| ServiceError::rejected(error.to_string()))
}
pub(super) fn encode_bounded(
    value: &impl Serialize,
    maximum: usize,
) -> Result<Vec<u8>, ServiceError> {
    struct Bytes {
        bytes: Vec<u8>,
        maximum: usize,
    }
    impl Write for Bytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other(
                    "Evidence metadata exceeds its compiled byte bound",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(maximum)
        .map_err(|_| ServiceError::rejected("Cannot reserve bounded evidence metadata memory"))?;
    let mut output = Bytes { bytes, maximum };
    serde_json::to_writer(&mut output, value)
        .map_err(|error| ServiceError::rejected(error.to_string()))?;
    Ok(output.bytes)
}
pub(super) fn budget_error(error: RecordingError) -> ServiceError {
    let kind = match error {
        RecordingError::InspectionCancelled => FailureKind::InspectionCancelled,
        RecordingError::InspectionLimit { .. } => FailureKind::InspectionLimit,
        RecordingError::Owned => FailureKind::SourceBusy,
        _ => FailureKind::SourceUnavailable,
    };
    ServiceError {
        kind,
        message: EvidenceMessage::new(error.to_string()).message,
        outcome_unknown: false,
    }
}
pub(super) fn charge(budget: &InspectionBudget, bytes: u64) -> Result<(), ServiceError> {
    budget.charge_work(bytes).map_err(budget_error)
}
pub(super) fn checkpoint(budget: &InspectionBudget) -> Result<(), ServiceError> {
    budget.checkpoint().map_err(budget_error)
}
