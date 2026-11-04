//! Exact bounded requests and compact governed project-archive outcomes.
//!
//! Conversions run only for admitted requests and their actual store contexts.
//! Their exact UTF-8 paths therefore remain bounded by the validated input plus
//! fixed staging suffixes. They never truncate or reinterpret an authority path.
//! The runner reserves conversion work before invoking storage. Conversions
//! perform no I/O, hashing, document serialization or governor checkpoint.
//! Success helpers consume the corresponding governed API's actual outcome;
//! their intrinsic completion variants follow that API's completion contract.
use crate::{ServiceError, fatal_sqlite_primary};
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, Validate},
};
use omniusong_recording::{InspectionResource, RecordingError};
use omniusong_store::{
    ArchiveAction, ArchiveCleanupIssue, ArchiveCompletion, ArchiveContext, ArchiveFailure,
    ArchiveOutcome, ArchivePhase, ArchiveReceipt, ArchiveSqlOrigin, RestoreCopyIntent,
    RestoreCopyReceipt, StoreError,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    io::Write,
    path::{Path, PathBuf},
};

pub const MAX_PROJECT_ARCHIVE_REQUEST_BYTES: usize = 65_536;
pub const MAX_PROJECT_ARCHIVE_REPLY_BYTES: usize = 262_144;
pub const MAX_PROJECT_ARCHIVE_PATH_BYTES: usize = 4096;
pub const MAX_PROJECT_ARCHIVE_REPORT_PATH_BYTES: usize = 8192;
pub const MAX_PROJECT_ARCHIVE_MESSAGE_BYTES: usize = 1024;
pub const MAX_PROJECT_ARCHIVE_CLEANUP_ISSUES: usize = 4;

// A string enum must accept a JSON string only, including on deserialization.
// Derived unit-enum decoding can also accept map-shaped unit variants.
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
                struct StringVariant;
                impl Visitor<'_> for StringVariant {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!(stringify!($name), " string"))
                    }
                    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        match value { $($wire => Ok($name::$variant),)+ _ => Err(E::custom("Unknown project archive string variant")) }
                    }
                }
                deserializer.deserialize_str(StringVariant)
            }
        }
    };
}

string_enum!(ProjectArchiveAction { Export => "export", Restore => "restore", Copy => "copy", Inspect => "inspect" });
string_enum!(ProjectArchivePhase {
    NotStarted => "not_started", ObjectsPlacementAttempted => "objects_placement_attempted",
    ObjectsPlaced => "objects_placed", OutputLinkAttempted => "output_link_attempted",
    OutputLinked => "output_linked", ExportDurable => "export_durable",
    CatalogCommitAttempted => "catalog_commit_attempted", CatalogCommitted => "catalog_committed",
    RecordedReplay => "recorded_replay", ReadOnlyCompleted => "read_only_completed"
});
string_enum!(ProjectArchiveCopyCompletion { CatalogCommitted => "catalog_committed", RecordedReplay => "recorded_replay" });
string_enum!(ProjectArchiveSqlOrigin { Input => "input", PrivateOutput => "private_output", LiveCatalog => "live_catalog", OwnedSource => "owned_source" });
string_enum!(ProjectArchiveProblemKind {
    Rejected => "rejected", Conflict => "conflict", Cancelled => "cancelled", Limit => "limit",
    Busy => "busy", Unavailable => "unavailable", Io => "io", Sqlite => "sqlite", Other => "other"
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveRequest {
    pub version: u32,
    pub origin_root: PathBuf,
    pub operation: ProjectArchiveOperation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectArchiveOperation {
    Export {
        project_id: ProjectId,
        destination: PathBuf,
    },
    RestoreIdentity {
        archive_path: PathBuf,
    },
    RestoreCopy {
        archive_path: PathBuf,
        intent: RestoreCopyIntent,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveReply {
    pub version: u32,
    pub request_sha256: String,
    pub invoked: bool,
    pub outcome: ProjectArchiveOutcome,
    pub catalog_retirement: Option<ProjectArchiveCatalogRetirement>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectArchiveOutcome {
    Exported {
        receipt: ProjectArchiveExportReceipt,
        cleanup: ProjectArchiveCleanup,
    },
    RestoredIdentity {
        project: ProjectArchiveProject,
        cleanup: ProjectArchiveCleanup,
    },
    RestoredCopy {
        operation_id: OperationId,
        source_project_id: ProjectId,
        source_archive_sha256: String,
        project: ProjectArchiveProject,
        completion: ProjectArchiveCopyCompletion,
        cleanup: ProjectArchiveCleanup,
    },
    Failed {
        context: ProjectArchiveContext,
        problem: ProjectArchiveProblem,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveExportReceipt {
    pub path: PathBuf,
    pub project_id: ProjectId,
    pub sha256: String,
    pub byte_length: u64,
    pub history_rows: u64,
    pub operation_rows: u64,
    pub asset_count: u64,
    pub take_count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveProject {
    pub project_id: ProjectId,
    pub revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveContext {
    pub action: ProjectArchiveAction,
    pub phase: ProjectArchivePhase,
    pub staging: Option<PathBuf>,
    pub destination: PathBuf,
    pub source: Option<PathBuf>,
    pub operation_id: Option<OperationId>,
    pub expected_archive_sha256: Option<String>,
    pub archive_sha256: Option<String>,
    pub confirmed_objects: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveMessage {
    pub text: String,
    pub truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveSqlProblem {
    pub origin: ProjectArchiveSqlOrigin,
    pub extended_code: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectArchiveBudgetStop {
    Cancelled {},
    Limit {
        #[serde(deserialize_with = "inspection_resource")]
        resource: InspectionResource,
        limit: u64,
        attempted: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveProblem {
    pub kind: ProjectArchiveProblemKind,
    pub detail: ProjectArchiveMessage,
    pub sqlite: Option<ProjectArchiveSqlProblem>,
    pub governor_stop: Option<ProjectArchiveBudgetStop>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveCatalogRetirement {
    pub sqlite_code: Option<i32>,
    pub connection_unusable: bool,
    pub detail: ProjectArchiveMessage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveCleanupIssue {
    pub path: PathBuf,
    pub detail: ProjectArchiveMessage,
    pub raw_os_error: Option<i32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArchiveCleanup {
    pub issues: Vec<ProjectArchiveCleanupIssue>,
    pub omitted: u64,
}

fn require(ok: bool, message: &str) -> Result<(), ServiceError> {
    if ok {
        Ok(())
    } else {
        Err(ServiceError::rejected(message))
    }
}
fn valid_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn valid_project(id: &ProjectId) -> bool {
    id.0.len() <= 96 && id.validate().is_ok()
}
fn valid_operation(id: &OperationId) -> bool {
    id.0.len() <= 96 && id.validate().is_ok()
}
fn path_text(path: &Path, maximum: usize) -> Result<&str, ServiceError> {
    let text = path
        .to_str()
        .ok_or_else(|| ServiceError::rejected("Project archive paths must be UTF-8"))?;
    require(
        path.is_absolute()
            && text.len() <= maximum
            && !text.chars().any(char::is_control)
            && !path.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str(),
        "Project archive paths need bounded absolute normal text",
    )?;
    Ok(text)
}
fn stage_base(request: &ProjectArchiveRequest) -> (&Path, &str) {
    match &request.operation {
        ProjectArchiveOperation::Export { destination, .. } => (
            destination.parent().unwrap_or(destination),
            ".omniusong-archive-export-",
        ),
        ProjectArchiveOperation::RestoreIdentity { .. } => {
            (&request.origin_root, ".omniusong-archive-restore-")
        }
        ProjectArchiveOperation::RestoreCopy { .. } => {
            (&request.origin_root, ".omniusong-archive-copy-")
        }
    }
}
fn associated_stage(path: &Path, request: &ProjectArchiveRequest, descendants: bool) -> bool {
    let (base, prefix) = stage_base(request);
    let Ok(relative) = path.strip_prefix(base) else {
        return false;
    };
    let mut parts = relative.components();
    let Some(std::path::Component::Normal(first)) = parts.next() else {
        return false;
    };
    let Some(name) = first.to_str() else {
        return false;
    };
    name.strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(".partial"))
        .is_some_and(|identity| {
            identity.len() == 36
                && identity.bytes().enumerate().all(|(index, byte)| {
                    if matches!(index, 8 | 13 | 18 | 23) {
                        byte == b'-'
                    } else {
                        byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                    }
                })
        })
        && (descendants || parts.next().is_none())
}
fn inspection_resource<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<InspectionResource, D::Error> {
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "FileBytes" => Ok(InspectionResource::FileBytes),
        "PhysicalRows" => Ok(InspectionResource::PhysicalRows),
        "WorkBytes" => Ok(InspectionResource::WorkBytes),
        "RowVisits" => Ok(InspectionResource::RowVisits),
        "ChunkVisits" => Ok(InspectionResource::ChunkVisits),
        "RecordVisits" => Ok(InspectionResource::RecordVisits),
        "Groups" => Ok(InspectionResource::Groups),
        "VmSteps" => Ok(InspectionResource::VmSteps),
        _ => Err(de::Error::custom(
            "Unknown project archive inspection resource",
        )),
    }
}

struct BoundedBytes {
    bytes: Vec<u8>,
    maximum: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "Project archive metadata exceeds its byte ceiling",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode(value: &impl Serialize, maximum: usize) -> Result<Vec<u8>, ServiceError> {
    struct Count {
        length: usize,
        maximum: usize,
    }
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.length = self
                .length
                .checked_add(bytes.len())
                .filter(|length| *length <= self.maximum)
                .ok_or_else(|| {
                    std::io::Error::other("Project archive metadata exceeds its byte ceiling")
                })?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count { length: 0, maximum };
    serde_json::to_writer(&mut count, value)
        .map_err(|error| ServiceError::rejected(error.to_string()))?;
    let mut output = BoundedBytes {
        bytes: Vec::new(),
        maximum: count.length,
    };
    output
        .bytes
        .try_reserve_exact(count.length)
        .map_err(|_| ServiceError::rejected("Cannot reserve bounded project archive metadata"))?;
    serde_json::to_writer(&mut output, value)
        .map_err(|error| ServiceError::rejected(error.to_string()))?;
    require(
        output.bytes.len() == count.length,
        "Project archive metadata changed during encoding",
    )?;
    Ok(output.bytes)
}

impl ProjectArchiveRequest {
    pub fn validate(&self) -> Result<(), ServiceError> {
        require(
            self.version == 1,
            "Unsupported project archive request version",
        )?;
        path_text(&self.origin_root, MAX_PROJECT_ARCHIVE_PATH_BYTES)?;
        match &self.operation {
            ProjectArchiveOperation::Export {
                project_id,
                destination,
            } => {
                require(
                    valid_project(project_id),
                    "Invalid project archive project identity",
                )?;
                path_text(destination, MAX_PROJECT_ARCHIVE_PATH_BYTES)?;
            }
            ProjectArchiveOperation::RestoreIdentity { archive_path } => {
                path_text(archive_path, MAX_PROJECT_ARCHIVE_PATH_BYTES)?;
            }
            ProjectArchiveOperation::RestoreCopy {
                archive_path,
                intent,
            } => {
                path_text(archive_path, MAX_PROJECT_ARCHIVE_PATH_BYTES)?;
                require(
                    valid_operation(&intent.operation_id)
                        && valid_sha(&intent.expected_archive_sha256),
                    "Invalid project archive copy identity or fingerprint",
                )?;
                intent
                    .validate()
                    .map_err(|_| ServiceError::rejected("Invalid project archive copy intent"))?;
            }
        }
        encode(self, MAX_PROJECT_ARCHIVE_REQUEST_BYTES).map(|_| ())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ServiceError> {
        self.validate()?;
        encode(self, MAX_PROJECT_ARCHIVE_REQUEST_BYTES)
    }
    pub fn fingerprint(&self) -> Result<String, ServiceError> {
        let bytes = self.canonical_bytes()?;
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.ProjectArchiveRequest.v1\0");
        hash.update(bytes);
        Ok(format!("{:x}", hash.finalize()))
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, ServiceError> {
        require(
            bytes.len() <= MAX_PROJECT_ARCHIVE_REQUEST_BYTES,
            "Project archive request exceeds its byte ceiling",
        )?;
        let request: Self = serde_json::from_slice(bytes)
            .map_err(|_| ServiceError::rejected("Invalid project archive request JSON"))?;
        request.validate()?;
        Ok(request)
    }
    /// Call only after `validate`: malformed requests never create store facts.
    pub(crate) fn preflight_context(&self) -> ArchiveContext {
        let mut context = match &self.operation {
            ProjectArchiveOperation::Export { destination, .. } => {
                ArchiveContext::new(ArchiveAction::Export, destination)
            }
            ProjectArchiveOperation::RestoreIdentity { archive_path } => {
                let mut context = ArchiveContext::new(ArchiveAction::Restore, &self.origin_root);
                context.source = Some(archive_path.clone());
                context
            }
            ProjectArchiveOperation::RestoreCopy {
                archive_path,
                intent,
            } => {
                let mut context = ArchiveContext::new(ArchiveAction::Copy, &self.origin_root);
                context.source = Some(archive_path.clone());
                context.operation_id = Some(intent.operation_id.0.clone());
                context.expected_archive_sha256 = Some(intent.expected_archive_sha256.clone());
                context
            }
        };
        context.phase = ArchivePhase::NotStarted;
        context
    }
}

impl ProjectArchiveMessage {
    pub(crate) fn from_display(value: &impl fmt::Display) -> Self {
        struct Text {
            text: String,
            truncated: bool,
        }
        impl fmt::Write for Text {
            fn write_str(&mut self, value: &str) -> fmt::Result {
                let available = MAX_PROJECT_ARCHIVE_MESSAGE_BYTES.saturating_sub(self.text.len());
                if value.len() <= available {
                    self.text.push_str(value);
                    return Ok(());
                }
                let mut end = available;
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                self.text.push_str(&value[..end]);
                self.truncated = true;
                Err(fmt::Error)
            }
        }
        let mut output = Text {
            text: String::with_capacity(MAX_PROJECT_ARCHIVE_MESSAGE_BYTES),
            truncated: false,
        };
        let _ = fmt::write(&mut output, format_args!("{value}"));
        Self {
            text: output.text,
            truncated: output.truncated,
        }
    }
    fn validate(&self) -> Result<(), ServiceError> {
        require(
            self.text.len() <= MAX_PROJECT_ARCHIVE_MESSAGE_BYTES,
            "Project archive message exceeds its byte ceiling",
        )
    }
}
impl ProjectArchiveProject {
    fn from_document(document: &ProjectDocument) -> Self {
        Self {
            project_id: ProjectId(document.id().into()),
            revision: document.revision(),
        }
    }
    fn validate(&self) -> Result<(), ServiceError> {
        require(
            valid_project(&self.project_id) && self.revision <= i64::MAX as u64,
            "Invalid compact project archive project reference",
        )
    }
}
impl ProjectArchiveCleanup {
    fn from_issues(issues: Vec<ArchiveCleanupIssue>) -> Self {
        let omitted = issues
            .len()
            .saturating_sub(MAX_PROJECT_ARCHIVE_CLEANUP_ISSUES) as u64;
        let issues = issues
            .into_iter()
            .take(MAX_PROJECT_ARCHIVE_CLEANUP_ISSUES)
            .map(|issue| ProjectArchiveCleanupIssue {
                path: issue.path,
                detail: ProjectArchiveMessage::from_display(&issue.error),
                raw_os_error: issue.error.raw_os_error(),
            })
            .collect();
        Self { issues, omitted }
    }
    fn validate_for(&self, request: &ProjectArchiveRequest) -> Result<(), ServiceError> {
        require(
            self.issues.len() <= MAX_PROJECT_ARCHIVE_CLEANUP_ISSUES
                && (self.omitted == 0 || self.issues.len() == MAX_PROJECT_ARCHIVE_CLEANUP_ISSUES),
            "Invalid project archive cleanup count",
        )?;
        for issue in &self.issues {
            path_text(&issue.path, MAX_PROJECT_ARCHIVE_REPORT_PATH_BYTES)?;
            require(
                associated_stage(&issue.path, request, true),
                "Project archive cleanup path is outside its private staging namespace",
            )?;
            issue.detail.validate()?;
        }
        Ok(())
    }
}

impl ProjectArchiveBudgetStop {
    fn from_recording(error: &RecordingError) -> Option<Self> {
        match error {
            RecordingError::InspectionCancelled => Some(Self::Cancelled {}),
            RecordingError::InspectionLimit {
                resource,
                limit,
                attempted,
            } => Some(Self::Limit {
                resource: *resource,
                limit: *limit,
                attempted: *attempted,
            }),
            _ => None,
        }
    }
    fn validate(&self) -> Result<(), ServiceError> {
        match self {
            Self::Cancelled {} => Ok(()),
            Self::Limit {
                resource,
                limit,
                attempted,
            } => {
                let limits = omniusong_recording::InspectionLimits::default();
                let ceiling = match resource {
                    InspectionResource::FileBytes => limits.max_file_bytes,
                    InspectionResource::PhysicalRows => limits.max_physical_rows,
                    InspectionResource::WorkBytes => limits.max_work_bytes,
                    InspectionResource::RowVisits => limits.max_row_visits,
                    InspectionResource::ChunkVisits => limits.max_chunk_visits,
                    InspectionResource::RecordVisits => limits.max_record_visits,
                    InspectionResource::Groups => limits.max_groups,
                    InspectionResource::VmSteps => limits.max_vm_steps,
                };
                require(
                    attempted > limit && *limit <= ceiling,
                    "Invalid project archive governor limit observation",
                )
            }
        }
    }
}
impl ProjectArchiveContext {
    fn from_store(value: &ArchiveContext) -> Self {
        Self {
            action: match value.action {
                ArchiveAction::Export => ProjectArchiveAction::Export,
                ArchiveAction::Restore => ProjectArchiveAction::Restore,
                ArchiveAction::Copy => ProjectArchiveAction::Copy,
                ArchiveAction::Inspect => ProjectArchiveAction::Inspect,
            },
            phase: match value.phase {
                ArchivePhase::NotStarted => ProjectArchivePhase::NotStarted,
                ArchivePhase::ObjectsPlacementAttempted => {
                    ProjectArchivePhase::ObjectsPlacementAttempted
                }
                ArchivePhase::ObjectsPlaced => ProjectArchivePhase::ObjectsPlaced,
                ArchivePhase::OutputLinkAttempted => ProjectArchivePhase::OutputLinkAttempted,
                ArchivePhase::OutputLinked => ProjectArchivePhase::OutputLinked,
                ArchivePhase::ExportDurable => ProjectArchivePhase::ExportDurable,
                ArchivePhase::CatalogCommitAttempted => ProjectArchivePhase::CatalogCommitAttempted,
                ArchivePhase::CatalogCommitted => ProjectArchivePhase::CatalogCommitted,
                ArchivePhase::RecordedReplay => ProjectArchivePhase::RecordedReplay,
                ArchivePhase::ReadOnlyCompleted => ProjectArchivePhase::ReadOnlyCompleted,
            },
            staging: value.staging.clone(),
            destination: value.destination.clone(),
            source: value.source.clone(),
            operation_id: value
                .operation_id
                .as_ref()
                .map(|id| OperationId(id.clone())),
            expected_archive_sha256: value.expected_archive_sha256.clone(),
            archive_sha256: value.archive_sha256.clone(),
            confirmed_objects: value.published_objects as u64,
        }
    }
    fn validate_for(
        &self,
        request: &ProjectArchiveRequest,
        invoked: bool,
    ) -> Result<(), ServiceError> {
        path_text(&self.destination, MAX_PROJECT_ARCHIVE_REPORT_PATH_BYTES)?;
        for path in self.staging.iter().chain(self.source.iter()) {
            path_text(path, MAX_PROJECT_ARCHIVE_REPORT_PATH_BYTES)?;
        }
        require(
            self.operation_id.as_ref().is_none_or(valid_operation)
                && self
                    .expected_archive_sha256
                    .as_deref()
                    .is_none_or(valid_sha)
                && self.archive_sha256.as_deref().is_none_or(valid_sha)
                && self.confirmed_objects <= 4096,
            "Invalid project archive failure identity/count",
        )?;
        let catalog_source = request.origin_root.join("studio.sqlite3");
        let (action, destination, source, operation, expected) = match &request.operation {
            ProjectArchiveOperation::Export { destination, .. } => (
                ProjectArchiveAction::Export,
                destination,
                Some(&catalog_source),
                None,
                None,
            ),
            ProjectArchiveOperation::RestoreIdentity { archive_path } => (
                ProjectArchiveAction::Restore,
                &request.origin_root,
                Some(archive_path),
                None,
                None,
            ),
            ProjectArchiveOperation::RestoreCopy {
                archive_path,
                intent,
            } => (
                ProjectArchiveAction::Copy,
                &request.origin_root,
                Some(archive_path),
                Some(&intent.operation_id),
                Some(intent.expected_archive_sha256.as_str()),
            ),
        };
        let partial = invoked
            && self.phase == ProjectArchivePhase::NotStarted
            && self.staging.is_none()
            && self.confirmed_objects == 0;
        let destination_matches = self.destination.as_os_str() == destination.as_os_str()
            || (partial
                && action == ProjectArchiveAction::Export
                && self.destination.as_os_str() == request.origin_root.as_os_str());
        let expected_source = if !invoked && action == ProjectArchiveAction::Export {
            None
        } else {
            source
        };
        let source_matches = self.source.as_ref().map(|path| path.as_os_str())
            == expected_source.map(|path| path.as_os_str())
            || (partial && self.source.is_none());
        require(
            self.action == action
                && destination_matches
                && source_matches
                && (self.operation_id.as_ref() == operation
                    || (partial && self.operation_id.is_none()))
                && (self.expected_archive_sha256.as_deref() == expected
                    || (partial && self.expected_archive_sha256.is_none())),
            "Project archive failure context differs from its request",
        )?;
        if let Some(staging) = &self.staging {
            require(
                associated_stage(staging, request, false),
                "Project archive staging path is outside its private namespace",
            )?;
        }
        let phase_valid = match action {
            ProjectArchiveAction::Export => matches!(
                self.phase,
                ProjectArchivePhase::NotStarted
                    | ProjectArchivePhase::OutputLinkAttempted
                    | ProjectArchivePhase::OutputLinked
                    | ProjectArchivePhase::ExportDurable
            ),
            ProjectArchiveAction::Restore => matches!(
                self.phase,
                ProjectArchivePhase::NotStarted
                    | ProjectArchivePhase::ObjectsPlacementAttempted
                    | ProjectArchivePhase::ObjectsPlaced
                    | ProjectArchivePhase::CatalogCommitAttempted
                    | ProjectArchivePhase::CatalogCommitted
            ),
            ProjectArchiveAction::Copy => matches!(
                self.phase,
                ProjectArchivePhase::NotStarted
                    | ProjectArchivePhase::ObjectsPlacementAttempted
                    | ProjectArchivePhase::ObjectsPlaced
                    | ProjectArchivePhase::CatalogCommitAttempted
                    | ProjectArchivePhase::CatalogCommitted
                    | ProjectArchivePhase::RecordedReplay
            ),
            ProjectArchiveAction::Inspect => false,
        };
        require(
            phase_valid,
            "Project archive failure has an incompatible phase",
        )?;
        let before_placement = self.phase == ProjectArchivePhase::NotStarted;
        require(
            (action != ProjectArchiveAction::Export
                && !before_placement
                && self.phase != ProjectArchivePhase::RecordedReplay)
                || self.confirmed_objects == 0,
            "Project archive failure reports placements before a placement phase",
        )?;
        if self.phase == ProjectArchivePhase::RecordedReplay {
            // The second replay check runs after private acquisition: a raced
            // exact copy can retain that stage/hash without installing objects.
            require(
                self.staging.is_some() == self.archive_sha256.is_some(),
                "Recorded replay acquisition facts are incomplete",
            )?;
        } else if !before_placement {
            require(
                self.staging.is_some() && self.archive_sha256.is_some(),
                "Project archive publication phase omitted staging or observed archive fingerprint",
            )?;
        }
        require(
            invoked
                || (self.phase == ProjectArchivePhase::NotStarted
                    && self.staging.is_none()
                    && self.archive_sha256.is_none()
                    && self.confirmed_objects == 0),
            "Uninvoked archive reply claims store execution facts",
        )
    }
}

fn original(
    error: &StoreError,
    origin: Option<ProjectArchiveSqlOrigin>,
) -> (&StoreError, Option<ProjectArchiveSqlOrigin>) {
    match error {
        StoreError::Catalog(value) => original(value.operation_error(), origin),
        StoreError::Archive(value) => original(value.operation_error(), origin),
        StoreError::ArchiveSql(value) => original(
            value.operation_error(),
            Some(match value.origin() {
                ArchiveSqlOrigin::Input => ProjectArchiveSqlOrigin::Input,
                ArchiveSqlOrigin::PrivateOutput => ProjectArchiveSqlOrigin::PrivateOutput,
                ArchiveSqlOrigin::LiveCatalog => ProjectArchiveSqlOrigin::LiveCatalog,
            }),
        ),
        _ => (error, origin),
    }
}
fn nested_stop(error: &StoreError) -> Option<ProjectArchiveBudgetStop> {
    match error {
        StoreError::Catalog(value) => nested_stop(value.operation_error()),
        StoreError::Archive(value) => value
            .governor_stop()
            .and_then(ProjectArchiveBudgetStop::from_recording)
            .or_else(|| nested_stop(value.operation_error())),
        StoreError::ArchiveSql(value) => nested_stop(value.operation_error()),
        StoreError::Recording(value) => ProjectArchiveBudgetStop::from_recording(value),
        _ => None,
    }
}
fn problem(error: &StoreError, stop: Option<ProjectArchiveBudgetStop>) -> ProjectArchiveProblem {
    let (primary, origin) = original(error, None);
    let (kind, sqlite) = match primary {
        StoreError::Database(value) => (
            ProjectArchiveProblemKind::Sqlite,
            value.sqlite_error().map(|code| ProjectArchiveSqlProblem {
                origin: origin.unwrap_or(ProjectArchiveSqlOrigin::LiveCatalog),
                extended_code: code.extended_code,
            }),
        ),
        StoreError::Recording(RecordingError::Database(value)) => (
            ProjectArchiveProblemKind::Sqlite,
            value.sqlite_error().map(|code| ProjectArchiveSqlProblem {
                origin: ProjectArchiveSqlOrigin::OwnedSource,
                extended_code: code.extended_code,
            }),
        ),
        StoreError::Recording(RecordingError::InspectionCancelled) => {
            (ProjectArchiveProblemKind::Cancelled, None)
        }
        StoreError::Recording(RecordingError::InspectionLimit { .. }) => {
            (ProjectArchiveProblemKind::Limit, None)
        }
        StoreError::Recording(RecordingError::Owned) => (ProjectArchiveProblemKind::Busy, None),
        StoreError::Recording(RecordingError::Interrupted | RecordingError::Backup { .. }) => {
            (ProjectArchiveProblemKind::Unavailable, None)
        }
        StoreError::Io(_) | StoreError::Recording(RecordingError::Io(_)) => {
            (ProjectArchiveProblemKind::Io, None)
        }
        StoreError::Conflict | StoreError::Recording(RecordingError::Conflict(_)) => {
            (ProjectArchiveProblemKind::Conflict, None)
        }
        StoreError::Invalid(_)
        | StoreError::Document(_)
        | StoreError::EditRejected(_)
        | StoreError::Recording(RecordingError::Invalid(_)) => {
            (ProjectArchiveProblemKind::Rejected, None)
        }
        _ => (ProjectArchiveProblemKind::Other, None),
    };
    ProjectArchiveProblem {
        kind,
        detail: ProjectArchiveMessage::from_display(primary),
        sqlite,
        governor_stop: stop,
    }
}
impl ProjectArchiveProblem {
    fn validate(&self) -> Result<(), ServiceError> {
        self.detail.validate()?;
        if let Some(sql) = &self.sqlite {
            require(
                self.kind == ProjectArchiveProblemKind::Sqlite
                    && sql.extended_code > 0
                    && sql.extended_code & 0xff != 0,
                "Invalid project archive SQL diagnostic",
            )?;
        }
        if let Some(stop) = &self.governor_stop {
            stop.validate()?;
        }
        require(
            match self.kind {
                ProjectArchiveProblemKind::Cancelled => matches!(
                    self.governor_stop,
                    Some(ProjectArchiveBudgetStop::Cancelled {})
                ),
                ProjectArchiveProblemKind::Limit => matches!(
                    self.governor_stop,
                    Some(ProjectArchiveBudgetStop::Limit { .. })
                ),
                _ => true,
            },
            "Project archive stop kind differs from its original error",
        )
    }
}
impl ProjectArchiveCatalogRetirement {
    fn valid_core(&self) -> bool {
        self.connection_unusable
            || self
                .sqlite_code
                .is_some_and(|code| code > 0 && fatal_sqlite_primary(code))
    }
    fn validate(&self) -> Result<(), ServiceError> {
        require(
            self.valid_core()
                && self
                    .sqlite_code
                    .is_none_or(|code| code > 0 && code & 0xff != 0),
            "Invalid project archive catalog retirement",
        )?;
        self.detail.validate()
    }
}
impl ProjectArchiveReply {
    /// Bounds raw decoding, not semantic acceptance. Callers must check the
    /// exact reference, extract independent retirement, then `validate_for`.
    /// Retaining an invalid semantic body preserves that independent core.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ServiceError> {
        require(
            bytes.len() <= MAX_PROJECT_ARCHIVE_REPLY_BYTES,
            "Project archive reply exceeds its byte ceiling",
        )?;
        serde_json::from_slice(bytes)
            .map_err(|_| ServiceError::rejected("Invalid project archive reply JSON"))
    }
    pub fn matches_request_reference(&self, request: &ProjectArchiveRequest) -> bool {
        self.version == 1
            && valid_sha(&self.request_sha256)
            && request
                .fingerprint()
                .is_ok_and(|sha| sha == self.request_sha256)
    }
    pub fn validate_for(&self, request: &ProjectArchiveRequest) -> Result<(), ServiceError> {
        require(
            self.matches_request_reference(request),
            "Project archive reply does not match its exact request",
        )?;
        if let Some(retirement) = &self.catalog_retirement {
            retirement.validate()?;
        }
        match (&self.outcome, &request.operation) {
            (
                ProjectArchiveOutcome::Exported { receipt, cleanup },
                ProjectArchiveOperation::Export {
                    project_id,
                    destination,
                },
            ) => {
                require(
                    self.invoked && self.catalog_retirement.is_none(),
                    "Completed export must describe an actual successful invocation",
                )?;
                path_text(&receipt.path, MAX_PROJECT_ARCHIVE_REPORT_PATH_BYTES)?;
                require(
                    receipt.project_id == *project_id
                        && valid_project(&receipt.project_id)
                        && receipt.path.as_os_str() == destination.as_os_str()
                        && valid_sha(&receipt.sha256)
                        && (1..=536_870_912).contains(&receipt.byte_length)
                        && receipt.history_rows <= 10_000
                        && receipt.operation_rows <= 100_000
                        && receipt.asset_count <= 4096
                        && receipt.take_count <= 4096,
                    "Project archive export receipt differs from its bounded request",
                )?;
                cleanup.validate_for(request)?;
            }
            (
                ProjectArchiveOutcome::RestoredIdentity { project, cleanup },
                ProjectArchiveOperation::RestoreIdentity { .. },
            ) => {
                require(
                    self.invoked && self.catalog_retirement.is_none(),
                    "Completed identity restore must describe an actual successful invocation",
                )?;
                project.validate()?;
                cleanup.validate_for(request)?;
            }
            (
                ProjectArchiveOutcome::RestoredCopy {
                    operation_id,
                    source_project_id,
                    source_archive_sha256,
                    project,
                    cleanup,
                    ..
                },
                ProjectArchiveOperation::RestoreCopy { intent, .. },
            ) => {
                require(
                    self.invoked
                        && self.catalog_retirement.is_none()
                        && *operation_id == intent.operation_id
                        && valid_operation(operation_id)
                        && valid_project(source_project_id)
                        && source_archive_sha256 == &intent.expected_archive_sha256
                        && valid_sha(source_archive_sha256)
                        && source_project_id != &project.project_id,
                    "Project archive copy receipt differs from its bounded request",
                )?;
                project.validate()?;
                cleanup.validate_for(request)?;
            }
            (ProjectArchiveOutcome::Failed { context, problem }, _) => {
                context.validate_for(request, self.invoked)?;
                problem.validate()?;
                if problem.sqlite.as_ref().is_some_and(|sql| {
                    sql.origin == ProjectArchiveSqlOrigin::LiveCatalog
                        && fatal_sqlite_primary(sql.extended_code)
                }) {
                    require(
                        self.catalog_retirement
                            .as_ref()
                            .is_some_and(ProjectArchiveCatalogRetirement::valid_core),
                        "Actual catalog fatal reply omitted retirement",
                    )?;
                }
            }
            _ => {
                return Err(ServiceError::rejected(
                    "Project archive outcome operation differs from its request",
                ));
            }
        }
        encode(self, MAX_PROJECT_ARCHIVE_REPLY_BYTES).map(|_| ())
    }
    /// The fatal/unusable core survives an otherwise malformed matching body.
    /// Never derive retirement from a private/source SQL diagnostic or prose.
    pub fn retirement_error(&self) -> Option<ServiceError> {
        let detail = if let Some(retirement) = self
            .catalog_retirement
            .as_ref()
            .filter(|value| value.valid_core())
        {
            &retirement.detail
        } else if let ProjectArchiveOutcome::Failed { problem, .. } = &self.outcome {
            let sql = problem.sqlite.as_ref()?;
            if sql.origin != ProjectArchiveSqlOrigin::LiveCatalog
                || sql.extended_code <= 0
                || !fatal_sqlite_primary(sql.extended_code)
            {
                return None;
            }
            &problem.detail
        } else {
            return None;
        };
        let detail = ProjectArchiveMessage::from_display(&detail.text);
        Some(ServiceError::storage_unavailable(detail.text, true))
    }
}

pub(crate) fn from_export(
    request_sha: &str,
    outcome: ArchiveOutcome<ArchiveReceipt>,
) -> ProjectArchiveReply {
    let receipt = outcome.value;
    ProjectArchiveReply {
        version: 1,
        request_sha256: request_sha.into(),
        invoked: true,
        catalog_retirement: None,
        outcome: ProjectArchiveOutcome::Exported {
            receipt: ProjectArchiveExportReceipt {
                path: receipt.path,
                project_id: ProjectId(receipt.project_id),
                sha256: receipt.sha256,
                byte_length: receipt.byte_length,
                history_rows: receipt.history_rows,
                operation_rows: receipt.operation_rows,
                asset_count: receipt.asset_count,
                take_count: receipt.take_count,
            },
            cleanup: ProjectArchiveCleanup::from_issues(outcome.cleanup),
        },
    }
}
pub(crate) fn from_identity(
    request_sha: &str,
    outcome: ArchiveOutcome<ProjectDocument>,
) -> ProjectArchiveReply {
    ProjectArchiveReply {
        version: 1,
        request_sha256: request_sha.into(),
        invoked: true,
        catalog_retirement: None,
        outcome: ProjectArchiveOutcome::RestoredIdentity {
            project: ProjectArchiveProject::from_document(&outcome.value),
            cleanup: ProjectArchiveCleanup::from_issues(outcome.cleanup),
        },
    }
}
pub(crate) fn from_copy(
    request_sha: &str,
    outcome: ArchiveOutcome<RestoreCopyReceipt>,
) -> ProjectArchiveReply {
    let completion = if outcome.completion == ArchiveCompletion::RecordedReplay {
        ProjectArchiveCopyCompletion::RecordedReplay
    } else {
        ProjectArchiveCopyCompletion::CatalogCommitted
    };
    ProjectArchiveReply {
        version: 1,
        request_sha256: request_sha.into(),
        invoked: true,
        catalog_retirement: None,
        outcome: ProjectArchiveOutcome::RestoredCopy {
            operation_id: outcome.value.operation_id,
            source_project_id: ProjectId(outcome.value.source_project_id),
            source_archive_sha256: outcome.value.source_archive_sha256,
            project: ProjectArchiveProject::from_document(&outcome.value.document),
            completion,
            cleanup: ProjectArchiveCleanup::from_issues(outcome.cleanup),
        },
    }
}
pub(crate) fn from_failure(
    request_sha: &str,
    failure: ArchiveFailure,
    invoked: bool,
) -> ProjectArchiveReply {
    // Inspect the complete chain before formatting/truncation or primary-domain
    // selection. Cleanup can retire the catalog beside a private primary fault.
    let code = failure.catalog_sqlite_error_code();
    let unusable = failure.catalog_connection_unusable();
    let catalog_retirement = (unusable || code.is_some_and(fatal_sqlite_primary)).then(|| {
        ProjectArchiveCatalogRetirement {
            sqlite_code: code,
            connection_unusable: unusable,
            detail: ProjectArchiveMessage::from_display(&failure),
        }
    });
    let stop = failure
        .governor_stop()
        .and_then(ProjectArchiveBudgetStop::from_recording)
        .or_else(|| nested_stop(failure.operation_error()));
    let problem = problem(failure.operation_error(), stop);
    ProjectArchiveReply {
        version: 1,
        request_sha256: request_sha.into(),
        invoked,
        catalog_retirement,
        outcome: ProjectArchiveOutcome::Failed {
            context: ProjectArchiveContext::from_store(failure.context()),
            problem,
        },
    }
}

mod runner;
#[cfg(test)]
mod tests;
