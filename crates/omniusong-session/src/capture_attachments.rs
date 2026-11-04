//! Exact normal-capture requests and compact commitment acknowledgements.
//!
//! A publication receipt attests the publication, not its old journal locator.
//! The complete request remains the retry/correlation identity. This boundary
//! does not claim that legacy ingestion is cumulatively governed.
use crate::{FailureKind, NativeBackend, Reply, ServiceError, fatal_sqlite_primary};
use omniusong_core::contracts::{OperationId, ProjectId, Validate};
use omniusong_recording::{InspectionResource, RecordingError};
use omniusong_store::{ArchiveSqlOrigin, EditReceipt, StoreError, TakePublication};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::{self, Visitor}};
use sha2::{Digest, Sha256};
use std::{fmt, io::Write, path::{Path, PathBuf}};

pub use crate::project_archives::{
    ProjectArchiveBudgetStop as CaptureAttachmentBudgetStop,
    ProjectArchiveCatalogRetirement as CaptureAttachmentCatalogRetirement,
    ProjectArchiveMessage as CaptureAttachmentMessage,
    ProjectArchiveSqlOrigin as CaptureAttachmentSqlOrigin,
    ProjectArchiveSqlProblem as CaptureAttachmentSqlProblem,
};

pub const MAX_CAPTURE_ATTACHMENT_REQUEST_BYTES: usize = 65_536;
pub const MAX_CAPTURE_ATTACHMENT_REPLY_BYTES: usize = 16_384;
pub const MAX_CAPTURE_ATTACHMENT_PATH_BYTES: usize = 4096;
pub const MAX_CAPTURE_ATTACHMENT_MESSAGE_BYTES: usize = 1024;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(match self { $(Self::$variant => $wire),+ })
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Variant;
                impl Visitor<'_> for Variant {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!(stringify!($name), " string"))
                    }
                    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        match value { $($wire => Ok($name::$variant),)+ _ => Err(E::custom("Unknown capture attachment string variant")) }
                    }
                }
                deserializer.deserialize_str(Variant)
            }
        }
    };
}
string_enum!(CaptureAttachmentCompletion { FreshCommit => "fresh_commit", RecordedReplay => "recorded_replay" });
string_enum!(CaptureAttachmentProblemKind {
    Rejected => "rejected", EditRejected => "edit_rejected", Conflict => "conflict",
    Cancelled => "cancelled", Limit => "limit", Busy => "busy", Unavailable => "unavailable",
    Io => "io", Sqlite => "sqlite", Protocol => "protocol", Other => "other"
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureAttachmentRequest {
    pub version: u32,
    pub origin_root: PathBuf,
    pub journal_path: PathBuf,
    pub publication: TakePublication,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureAttachmentReply {
    pub version: u32,
    pub request_sha256: String,
    pub store_invoked: bool,
    pub outcome: CaptureAttachmentOutcome,
    pub catalog_retirement: Option<CaptureAttachmentCatalogRetirement>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureAttachmentOutcome {
    Committed { commitment: CaptureAttachmentCommitment },
    Rejected { problem: CaptureAttachmentProblem },
    Uncertain { problem: CaptureAttachmentProblem },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureAttachmentCommitment {
    pub operation_id: OperationId,
    pub project_id: ProjectId,
    pub previous_revision: u64,
    pub committed_revision: u64,
    pub completion: CaptureAttachmentCompletion,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureAttachmentProblem {
    pub kind: CaptureAttachmentProblemKind,
    pub detail: CaptureAttachmentMessage,
    pub sqlite: Option<CaptureAttachmentSqlProblem>,
    pub governor_stop: Option<CaptureAttachmentBudgetStop>,
}

fn require(ok: bool, message: &str) -> Result<(), ServiceError> {
    if ok { Ok(()) } else { Err(ServiceError::rejected(message)) }
}
fn valid_sha(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn normal_path(path: &Path) -> Result<(), ServiceError> {
    let text = path.to_str().ok_or_else(|| ServiceError::rejected("Capture attachment paths must be UTF-8"))?;
    require(
        path.is_absolute() && text.len() <= MAX_CAPTURE_ATTACHMENT_PATH_BYTES
            && !text.chars().any(char::is_control)
            && !path.components().any(|part| matches!(part, std::path::Component::CurDir | std::path::Component::ParentDir))
            && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str(),
        "Capture attachment paths require bounded absolute normal text",
    )
}

struct BoundedBytes { bytes: Vec<u8>, maximum: usize }
impl BoundedBytes {
    fn reserve(maximum: usize) -> Result<Self, ServiceError> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(maximum).map_err(|_| ServiceError::rejected("Cannot reserve bounded capture attachment metadata"))?;
        Ok(Self { bytes, maximum })
    }
    fn encode(&mut self, value: &impl Serialize) -> Result<(), ServiceError> {
        self.bytes.clear();
        serde_json::to_writer(self, value).map_err(|_| ServiceError::rejected("Capture attachment metadata exceeds its byte ceiling"))
    }
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("Capture attachment metadata exceeds its byte ceiling"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
fn encode(value: &impl Serialize, maximum: usize) -> Result<Vec<u8>, ServiceError> {
    let mut output = BoundedBytes::reserve(maximum)?;
    output.encode(value)?;
    Ok(output.bytes)
}

impl CaptureAttachmentRequest {
    pub fn validate(&self) -> Result<(), ServiceError> {
        require(self.version == 1, "Unsupported capture attachment request version")?;
        normal_path(&self.origin_root)?;
        normal_path(&self.journal_path)?;
        self.publication.validate().map_err(|_| ServiceError::rejected("Invalid capture attachment publication"))?;
        encode(self, MAX_CAPTURE_ATTACHMENT_REQUEST_BYTES).map(|_| ())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ServiceError> {
        self.validate()?;
        encode(self, MAX_CAPTURE_ATTACHMENT_REQUEST_BYTES)
    }
    pub fn fingerprint(&self) -> Result<String, ServiceError> {
        let bytes = self.canonical_bytes()?;
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.CaptureAttachmentRequest.v1\0");
        hash.update(bytes);
        Ok(format!("{:x}", hash.finalize()))
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, ServiceError> {
        require(bytes.len() <= MAX_CAPTURE_ATTACHMENT_REQUEST_BYTES, "Capture attachment request exceeds its byte ceiling")?;
        let request: Self = serde_json::from_slice(bytes).map_err(|_| ServiceError::rejected("Invalid capture attachment request JSON"))?;
        request.validate()?;
        Ok(request)
    }
}

fn validate_message(value: &CaptureAttachmentMessage) -> Result<(), ServiceError> {
    require(value.text.len() <= MAX_CAPTURE_ATTACHMENT_MESSAGE_BYTES, "Capture attachment diagnostic exceeds its byte ceiling")
}
fn validate_stop(value: &CaptureAttachmentBudgetStop) -> Result<(), ServiceError> {
    match value {
        CaptureAttachmentBudgetStop::Cancelled {} => Ok(()),
        CaptureAttachmentBudgetStop::Limit { resource, limit, attempted } => {
            let defaults = omniusong_recording::InspectionLimits::default();
            let maximum = match resource {
                InspectionResource::FileBytes => defaults.max_file_bytes,
                InspectionResource::PhysicalRows => defaults.max_physical_rows,
                InspectionResource::WorkBytes => defaults.max_work_bytes,
                InspectionResource::RowVisits => defaults.max_row_visits,
                InspectionResource::ChunkVisits => defaults.max_chunk_visits,
                InspectionResource::RecordVisits => defaults.max_record_visits,
                InspectionResource::Groups => defaults.max_groups,
                InspectionResource::VmSteps => defaults.max_vm_steps,
            };
            require(attempted > limit && *limit <= maximum, "Invalid capture attachment governor observation")
        }
    }
}
fn retirement_core(value: &CaptureAttachmentCatalogRetirement) -> bool {
    value.connection_unusable || value.sqlite_code.is_some_and(|code| code > 0 && fatal_sqlite_primary(code))
}
fn validate_retirement(value: &CaptureAttachmentCatalogRetirement) -> Result<(), ServiceError> {
    require(retirement_core(value) && value.sqlite_code.is_none_or(|code| code > 0 && code & 0xff != 0), "Invalid capture attachment catalog retirement")?;
    validate_message(&value.detail)
}
impl CaptureAttachmentProblem {
    fn validate(&self) -> Result<(), ServiceError> {
        validate_message(&self.detail)?;
        if let Some(sql) = &self.sqlite {
            require(self.kind == CaptureAttachmentProblemKind::Sqlite && sql.extended_code > 0 && sql.extended_code & 0xff != 0, "Invalid capture attachment SQL observation")?;
        }
        if let Some(stop) = &self.governor_stop { validate_stop(stop)?; }
        require(match self.kind {
            CaptureAttachmentProblemKind::Cancelled => matches!(self.governor_stop, Some(CaptureAttachmentBudgetStop::Cancelled {})),
            CaptureAttachmentProblemKind::Limit => matches!(self.governor_stop, Some(CaptureAttachmentBudgetStop::Limit { .. })),
            _ => true,
        }, "Capture attachment stop kind differs from its observation")
    }
    fn failure(&self, uncertain: bool) -> ServiceError {
        let kind = match self.kind {
            CaptureAttachmentProblemKind::Rejected => FailureKind::Rejected,
            CaptureAttachmentProblemKind::EditRejected => FailureKind::EditRejected,
            CaptureAttachmentProblemKind::Conflict => FailureKind::RevisionConflict,
            CaptureAttachmentProblemKind::Cancelled => FailureKind::InspectionCancelled,
            CaptureAttachmentProblemKind::Limit => FailureKind::InspectionLimit,
            CaptureAttachmentProblemKind::Busy => FailureKind::SourceBusy,
            CaptureAttachmentProblemKind::Unavailable => FailureKind::SourceUnavailable,
            CaptureAttachmentProblemKind::Protocol => FailureKind::Protocol,
            _ => FailureKind::Backend,
        };
        ServiceError { kind, message: bounded_message(&self.detail.text).text, outcome_unknown: uncertain }
    }
}
impl CaptureAttachmentReply {
    /// Decode bounded JSON only. Correlation and semantic acceptance are separate.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ServiceError> {
        require(bytes.len() <= MAX_CAPTURE_ATTACHMENT_REPLY_BYTES, "Capture attachment reply exceeds its byte ceiling")?;
        serde_json::from_slice(bytes).map_err(|_| ServiceError::rejected("Invalid capture attachment reply JSON"))
    }
    /// A matching fingerprint with an unsupported reply version is still a
    /// matching reference; the service must consume it as a protocol failure.
    pub fn matches_request_reference(&self, request: &CaptureAttachmentRequest) -> bool {
        valid_sha(&self.request_sha256) && request.fingerprint().is_ok_and(|sha| sha == self.request_sha256)
    }
    pub fn validate_for(&self, request: &CaptureAttachmentRequest) -> Result<(), ServiceError> {
        if let Some(value) = &self.catalog_retirement { validate_retirement(value)?; }
        require(self.version == 1, "Unsupported capture attachment reply version")?;
        require(self.matches_request_reference(request), "Capture attachment reply differs from its exact request")?;
        match &self.outcome {
            CaptureAttachmentOutcome::Committed { commitment } => {
                require(self.store_invoked && self.catalog_retirement.is_none(), "Capture commitment requires an actual successful store invocation")?;
                require(commitment.operation_id.validate().is_ok() && commitment.project_id.validate().is_ok()
                    && commitment.operation_id == request.publication.operation_id
                    && commitment.project_id == request.publication.project_id
                    && commitment.previous_revision == request.publication.expected_revision
                    && commitment.previous_revision.checked_add(1) == Some(commitment.committed_revision)
                    && commitment.committed_revision <= i64::MAX as u64,
                    "Capture commitment differs from its bounded publication")?;
            }
            CaptureAttachmentOutcome::Rejected { problem } => {
                problem.validate()?;
                require(if self.store_invoked {
                    matches!(problem.kind, CaptureAttachmentProblemKind::Conflict | CaptureAttachmentProblemKind::EditRejected)
                        && self.catalog_retirement.is_none() && problem.sqlite.is_none() && problem.governor_stop.is_none()
                } else {
                    problem.kind == CaptureAttachmentProblemKind::Rejected && self.catalog_retirement.is_none()
                        && problem.sqlite.is_none() && problem.governor_stop.is_none()
                }, "Capture refusal does not prove absence of project commitment")?;
            }
            CaptureAttachmentOutcome::Uncertain { problem } => {
                require(self.store_invoked, "Uninvoked capture reply cannot claim store uncertainty")?;
                problem.validate()?;
            }
        }
        if let CaptureAttachmentOutcome::Uncertain { problem } = &self.outcome {
            if problem.sqlite.as_ref().is_some_and(|sql| sql.origin == CaptureAttachmentSqlOrigin::LiveCatalog && fatal_sqlite_primary(sql.extended_code)) {
                require(self.catalog_retirement.as_ref().is_some_and(retirement_core), "Fatal live-catalog capture failure omitted retirement")?;
            }
        }
        encode(self, MAX_CAPTURE_ATTACHMENT_REPLY_BYTES).map(|_| ())
    }
    /// Independently validated retirement survives an otherwise invalid body.
    /// An owned-source diagnostic never supplies live-catalog retirement.
    pub fn retirement_error(&self) -> Option<ServiceError> {
        let retirement = self.catalog_retirement.as_ref()?;
        if !retirement_core(retirement) { return None; }
        Some(ServiceError::storage_unavailable(bounded_message(&retirement.detail.text).text, self.store_invoked))
    }
    pub fn failure_error(&self) -> Option<ServiceError> {
        match &self.outcome {
            CaptureAttachmentOutcome::Rejected { problem } => Some(problem.failure(false)),
            CaptureAttachmentOutcome::Uncertain { problem } => Some(problem.failure(true)),
            CaptureAttachmentOutcome::Committed { .. } => None,
        }
    }
}

fn reserved_text() -> Result<String, ServiceError> {
    let mut value = String::new();
    value.try_reserve_exact(MAX_CAPTURE_ATTACHMENT_MESSAGE_BYTES).map_err(|_| ServiceError::rejected("Cannot reserve capture attachment diagnostic"))?;
    Ok(value)
}
fn write_message(output: &mut CaptureAttachmentMessage, value: &impl fmt::Display) {
    struct Text<'a>(&'a mut CaptureAttachmentMessage);
    impl fmt::Write for Text<'_> {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            let available = MAX_CAPTURE_ATTACHMENT_MESSAGE_BYTES.saturating_sub(self.0.text.len());
            let mut end = value.len().min(available);
            while !value.is_char_boundary(end) { end -= 1; }
            self.0.text.push_str(&value[..end]);
            if end == value.len() { Ok(()) } else { self.0.truncated = true; Err(fmt::Error) }
        }
    }
    output.text.clear();
    output.truncated = false;
    let _ = fmt::write(&mut Text(output), format_args!("{value}"));
}
fn bounded_message(value: &impl fmt::Display) -> CaptureAttachmentMessage {
    let mut message = CaptureAttachmentMessage { text: String::with_capacity(MAX_CAPTURE_ATTACHMENT_MESSAGE_BYTES), truncated: false };
    write_message(&mut message, value);
    message
}

fn primary(error: &StoreError, origin: Option<CaptureAttachmentSqlOrigin>) -> (&StoreError, Option<CaptureAttachmentSqlOrigin>) {
    match error {
        StoreError::Catalog(value) => primary(value.operation_error(), origin),
        StoreError::Archive(value) => primary(value.operation_error(), origin),
        StoreError::ArchiveSql(value) => primary(value.operation_error(), Some(match value.origin() {
            ArchiveSqlOrigin::Input => CaptureAttachmentSqlOrigin::Input,
            ArchiveSqlOrigin::PrivateOutput => CaptureAttachmentSqlOrigin::PrivateOutput,
            ArchiveSqlOrigin::LiveCatalog => CaptureAttachmentSqlOrigin::LiveCatalog,
        })),
        _ => (error, origin),
    }
}
fn stop(error: &StoreError) -> Option<CaptureAttachmentBudgetStop> {
    fn recording(error: &RecordingError) -> Option<CaptureAttachmentBudgetStop> {
        match error {
            RecordingError::InspectionCancelled => Some(CaptureAttachmentBudgetStop::Cancelled {}),
            RecordingError::InspectionLimit { resource, limit, attempted } => Some(CaptureAttachmentBudgetStop::Limit { resource: *resource, limit: *limit, attempted: *attempted }),
            _ => None,
        }
    }
    match error {
        StoreError::Catalog(value) => stop(value.operation_error()),
        StoreError::Archive(value) => value.governor_stop().and_then(recording).or_else(|| stop(value.operation_error())),
        StoreError::ArchiveSql(value) => stop(value.operation_error()),
        StoreError::Recording(value) => recording(value),
        _ => None,
    }
}

/// All owned IDs, diagnostics, result envelope and wire/conversion capacity are
/// reserved before storage. No allocation proportional to a result document is
/// performed here, and no current-document work is part of completion.
struct PreparedReply {
    reply: Box<CaptureAttachmentReply>,
    commitment: CaptureAttachmentCommitment,
    problem: CaptureAttachmentProblem,
    retirement: CaptureAttachmentCatalogRetirement,
    wire: BoundedBytes,
    fallback: CaptureAttachmentProblem,
}
impl PreparedReply {
    fn new(request: &CaptureAttachmentRequest, request_sha256: String) -> Result<Self, ServiceError> {
        let message = || -> Result<CaptureAttachmentMessage, ServiceError> { Ok(CaptureAttachmentMessage { text: reserved_text()?, truncated: false }) };
        let problem = CaptureAttachmentProblem { kind: CaptureAttachmentProblemKind::Other, detail: message()?, sqlite: None, governor_stop: None };
        let fallback = CaptureAttachmentProblem {
            kind: CaptureAttachmentProblemKind::Protocol,
            detail: bounded_message(&"Storage returned inconsistent compact capture commitment metadata"),
            sqlite: None, governor_stop: None,
        };
        let commitment = CaptureAttachmentCommitment {
            operation_id: request.publication.operation_id.clone(), project_id: request.publication.project_id.clone(),
            previous_revision: request.publication.expected_revision,
            committed_revision: request.publication.expected_revision.checked_add(1).ok_or_else(|| ServiceError::rejected("Capture commitment revision is out of range"))?,
            completion: CaptureAttachmentCompletion::FreshCommit,
        };
        let mut wire = BoundedBytes::reserve(MAX_CAPTURE_ATTACHMENT_REPLY_BYTES)?;
        // Establish an exact worst-case encoded failure bound (six JSON bytes
        // for every diagnostic byte), including every optional observation.
        let worst_message = CaptureAttachmentMessage { text: "\0".repeat(MAX_CAPTURE_ATTACHMENT_MESSAGE_BYTES), truncated: false };
        let worst = CaptureAttachmentReply {
            version: u32::MAX, request_sha256: request_sha256.clone(), store_invoked: false,
            outcome: CaptureAttachmentOutcome::Uncertain { problem: CaptureAttachmentProblem {
                kind: CaptureAttachmentProblemKind::EditRejected, detail: worst_message.clone(),
                sqlite: Some(CaptureAttachmentSqlProblem { origin: CaptureAttachmentSqlOrigin::PrivateOutput, extended_code: i32::MAX }),
                governor_stop: Some(CaptureAttachmentBudgetStop::Limit { resource: InspectionResource::PhysicalRows, limit: u64::MAX, attempted: u64::MAX }),
            } },
            catalog_retirement: Some(CaptureAttachmentCatalogRetirement { sqlite_code: Some(i32::MAX), connection_unusable: false, detail: worst_message }),
        };
        wire.encode(&worst)?;
        let reply = Box::new(CaptureAttachmentReply {
            version: 1, request_sha256, store_invoked: false,
            outcome: CaptureAttachmentOutcome::Committed { commitment: commitment.clone() },
            catalog_retirement: None,
        });
        wire.encode(reply.as_ref())?;
        Ok(Self { reply, commitment, problem, retirement: CaptureAttachmentCatalogRetirement { sqlite_code: None, connection_unusable: false, detail: message()? }, wire, fallback })
    }
    fn reject(mut self, detail: &str) -> Box<CaptureAttachmentReply> {
        self.problem.kind = CaptureAttachmentProblemKind::Rejected;
        write_message(&mut self.problem.detail, &detail);
        self.reply.outcome = CaptureAttachmentOutcome::Rejected { problem: self.problem };
        self.reply
    }
    fn success(mut self, receipt: EditReceipt) -> Box<CaptureAttachmentReply> {
        self.reply.store_invoked = true;
        if receipt.operation_id == self.commitment.operation_id.0
            && receipt.previous_revision == self.commitment.previous_revision
            && receipt.document.id() == self.commitment.project_id.0
            && receipt.document.revision() == self.commitment.committed_revision {
            self.commitment.completion = if receipt.replayed { CaptureAttachmentCompletion::RecordedReplay } else { CaptureAttachmentCompletion::FreshCommit };
            self.reply.outcome = CaptureAttachmentOutcome::Committed { commitment: self.commitment };
        } else {
            self.reply.outcome = CaptureAttachmentOutcome::Uncertain { problem: self.fallback };
            return self.reply;
        }
        // The buffer was reserved and the complete success shape was encoded
        // before invocation. Only the fixed completion string can now differ.
        if self.wire.encode(self.reply.as_ref()).is_err() {
            self.reply.outcome = CaptureAttachmentOutcome::Uncertain { problem: self.fallback };
        }
        self.reply
    }
    fn failure(mut self, error: StoreError) -> Box<CaptureAttachmentReply> {
        self.reply.store_invoked = true;
        let rejected = matches!(error, StoreError::Conflict | StoreError::EditRejected(_));
        // Retirement uses the ENTIRE error wrapper, including catalog cleanup.
        let code = error.catalog_sqlite_error_code();
        let unusable = error.catalog_connection_unusable();
        if unusable || code.is_some_and(fatal_sqlite_primary) {
            self.retirement.sqlite_code = code;
            self.retirement.connection_unusable = unusable;
            write_message(&mut self.retirement.detail, &error);
            self.reply.catalog_retirement = Some(self.retirement);
        }
        self.problem.governor_stop = stop(&error);
        let (original, origin) = primary(&error, None);
        self.problem.kind = match original {
            StoreError::Database(value) => {
                self.problem.sqlite = value.sqlite_error().map(|code| CaptureAttachmentSqlProblem { origin: origin.unwrap_or(CaptureAttachmentSqlOrigin::LiveCatalog), extended_code: code.extended_code });
                CaptureAttachmentProblemKind::Sqlite
            }
            StoreError::Recording(RecordingError::Database(value)) => {
                self.problem.sqlite = value.sqlite_error().map(|code| CaptureAttachmentSqlProblem { origin: CaptureAttachmentSqlOrigin::OwnedSource, extended_code: code.extended_code });
                CaptureAttachmentProblemKind::Sqlite
            }
            StoreError::Conflict | StoreError::Recording(RecordingError::Conflict(_)) => CaptureAttachmentProblemKind::Conflict,
            StoreError::EditRejected(_) => CaptureAttachmentProblemKind::EditRejected,
            StoreError::Recording(RecordingError::InspectionCancelled) => CaptureAttachmentProblemKind::Cancelled,
            StoreError::Recording(RecordingError::InspectionLimit { .. }) => CaptureAttachmentProblemKind::Limit,
            StoreError::Recording(RecordingError::Owned) => CaptureAttachmentProblemKind::Busy,
            StoreError::Recording(RecordingError::Interrupted | RecordingError::Backup { .. }) => CaptureAttachmentProblemKind::Unavailable,
            StoreError::Io(_) | StoreError::Recording(RecordingError::Io(_)) => CaptureAttachmentProblemKind::Io,
            StoreError::Invalid(_) | StoreError::Document(_) | StoreError::Recording(RecordingError::Invalid(_)) => CaptureAttachmentProblemKind::Rejected,
            _ => CaptureAttachmentProblemKind::Other,
        };
        write_message(&mut self.problem.detail, original);
        self.reply.outcome = if rejected { CaptureAttachmentOutcome::Rejected { problem: self.problem } } else { CaptureAttachmentOutcome::Uncertain { problem: self.problem } };
        if self.wire.encode(self.reply.as_ref()).is_err() {
            self.reply.outcome = CaptureAttachmentOutcome::Uncertain { problem: self.fallback };
        }
        self.reply
    }
}
impl NativeBackend {
    pub(crate) fn attach_capture_compact(&mut self, request: &CaptureAttachmentRequest) -> Result<Reply, ServiceError> {
        request.validate()?;
        let prepared = PreparedReply::new(request, request.fingerprint()?)?;
        let Some(store) = self.store.as_mut() else {
            return Ok(Reply::CaptureAttachment(prepared.reject("Open session storage before attaching a capture")));
        };
        if store.root().as_os_str() != request.origin_root.as_os_str() {
            return Ok(Reply::CaptureAttachment(prepared.reject("Capture attachment origin does not match the open store")));
        }
        // Exactly one existing storage invocation. In particular, no source
        // locator stat/canonicalize precedes its historical receipt lookup.
        let outcome = match store.attach_capture(&request.journal_path, &request.publication) {
            Ok(receipt) => prepared.success(receipt),
            Err(error) => prepared.failure(error),
        };
        Ok(Reply::CaptureAttachment(outcome))
    }
}

#[cfg(test)]
mod tests;
