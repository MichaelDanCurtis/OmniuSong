//! Pure, explicit studio-evidence presentation. This module never accesses a
//! filesystem, worker, engine or journal and never executes an action itself.
use eframe::egui;
use omniusong_session::{
    Command, EvidenceBudgetStop, EvidenceJobList, EvidenceJobPhase, EvidenceJobReply,
    EvidenceJobRequest, EvidenceJobView, EvidenceMessage, EvidenceOperation, EvidenceOutcome,
    EvidenceProblemKind, FailureKind, MAX_EVIDENCE_JOB_LIST_BYTES, MAX_EVIDENCE_JOB_MESSAGE_BYTES,
    MAX_EVIDENCE_JOB_OBSERVATIONS, MAX_EVIDENCE_JOB_RECORD_BYTES, MAX_EVIDENCE_JOB_REQUEST_BYTES,
    MAX_EVIDENCE_JOB_VIEW_BYTES, MAX_EVIDENCE_JOBS, MAX_EVIDENCE_OUTCOME_REPLY_BYTES, Reply,
    RequestId, ServiceError,
};
use omniusong_store::{
    EVIDENCE_SCOPE, EvidenceArchiveInspection, EvidenceArchiveReceipt, EvidenceEntryKind,
    MAX_EVIDENCE_ENTRIES, MAX_EVIDENCE_MANIFEST_BYTES,
};
use std::{
    collections::{BTreeSet, VecDeque},
    io::Write,
    path::{Path, PathBuf},
};

const MAX_CARDS: usize = 8;
const MAX_METADATA_PROBLEMS: usize = 23;

#[derive(Clone, Debug)]
pub(super) enum EvidenceUiRequest {
    Inspect {
        path: PathBuf,
        view_epoch: u64,
        generation: u64,
        serial: u64,
    },
    List {
        view_epoch: u64,
        generation: u64,
        serial: u64,
    },
    Read {
        job_id: String,
        view_epoch: u64,
        generation: u64,
        serial: u64,
    },
    Run {
        request: Box<EvidenceJobRequest>,
        view_epoch: u64,
        generation: u64,
        confirmation_serial: u64,
    },
    Reconcile {
        job_id: String,
        expected_request_sha256: String,
        expected_history_sha256: String,
        view_epoch: u64,
        generation: u64,
        serial: u64,
    },
}
impl EvidenceUiRequest {
    pub(super) fn command(&self) -> Command {
        match self {
            Self::Inspect { path, .. } => Command::InspectRetainedEvidence { path: path.clone() },
            Self::List { .. } => Command::ListEvidenceJobs,
            Self::Read { job_id, .. } => Command::ReadEvidenceJob {
                job_id: job_id.clone(),
            },
            Self::Run { request, .. } => Command::RunEvidenceJob {
                request: request.clone(),
            },
            Self::Reconcile {
                job_id,
                expected_request_sha256,
                expected_history_sha256,
                ..
            } => Command::ReconcileEvidenceJob {
                job_id: job_id.clone(),
                expected_request_sha256: expected_request_sha256.clone(),
                expected_history_sha256: expected_history_sha256.clone(),
            },
        }
    }
    pub(super) fn is_run(&self) -> bool {
        matches!(self, Self::Run { .. })
    }
    pub(super) fn reads_artifact(&self) -> bool {
        matches!(self, Self::Inspect { .. } | Self::Reconcile { .. })
    }
    fn cancellable(&self) -> bool {
        self.is_run() || self.reads_artifact()
    }
    fn context(&self) -> (u64, u64, u64) {
        match self {
            Self::Inspect {
                view_epoch,
                generation,
                serial,
                ..
            }
            | Self::List {
                view_epoch,
                generation,
                serial,
            }
            | Self::Read {
                view_epoch,
                generation,
                serial,
                ..
            }
            | Self::Reconcile {
                view_epoch,
                generation,
                serial,
                ..
            } => (*view_epoch, *generation, *serial),
            Self::Run {
                view_epoch,
                generation,
                confirmation_serial,
                ..
            } => (*view_epoch, *generation, *confirmation_serial),
        }
    }
    pub(super) fn matches_command(&self, command: &Command) -> bool {
        match (self, command) {
            (Self::Inspect { path, .. }, Command::InspectRetainedEvidence { path: other }) => {
                path.as_os_str() == other.as_os_str()
            }
            (Self::List { .. }, Command::ListEvidenceJobs) => true,
            (Self::Read { job_id, .. }, Command::ReadEvidenceJob { job_id: other }) => {
                job_id == other
            }
            (Self::Run { request, .. }, Command::RunEvidenceJob { request: other }) => request
                .fingerprint()
                .ok()
                .zip(other.fingerprint().ok())
                .is_some_and(|(a, b)| a == b),
            (
                Self::Reconcile {
                    job_id,
                    expected_request_sha256,
                    expected_history_sha256,
                    ..
                },
                Command::ReconcileEvidenceJob {
                    job_id: other,
                    expected_request_sha256: request_sha,
                    expected_history_sha256: history_sha,
                },
            ) => {
                job_id == other
                    && expected_request_sha256 == request_sha
                    && expected_history_sha256 == history_sha
            }
            _ => false,
        }
    }
    fn same(&self, other: &Self) -> bool {
        self.context() == other.context() && self.matches_command(&other.command())
    }
    fn job_id(&self) -> Option<&str> {
        match self {
            Self::Run { request, .. } => Some(&request.job_id),
            Self::Read { job_id, .. } | Self::Reconcile { job_id, .. } => Some(job_id),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub(super) enum EvidenceAction {
    Submit(EvidenceUiRequest),
    Cancel(RequestId),
    MetadataConnect,
}
pub(super) struct EvidenceAvailability {
    pub(super) can_submit: bool,
    pub(super) can_read_artifacts: bool,
    pub(super) can_run: bool,
    pub(super) generation: Option<u64>,
    pub(super) origin: Option<PathBuf>,
    pub(super) unavailable_reason: Option<String>,
}

fn ensure(ok: bool, message: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn valid_id(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && text.bytes().any(|b| b != b'0')
}
fn valid_sha(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_path(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|text| text.len() <= 4096 && !text.chars().any(char::is_control))
        && path.is_absolute()
        && path.file_name().is_some()
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
        && !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
}
fn exact_path(a: &Path, b: &Path) -> bool {
    a.as_os_str() == b.as_os_str()
}
fn message(text: &str) -> String {
    if text.len() <= MAX_EVIDENCE_JOB_MESSAGE_BYTES {
        return text.into();
    }
    let suffix = "… (message shortened)";
    let mut end = MAX_EVIDENCE_JOB_MESSAGE_BYTES - suffix.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &text[..end])
}
struct ByteCap {
    used: usize,
    maximum: usize,
}
impl Write for ByteCap {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.used) {
            return Err(std::io::Error::other(
                "Studio-evidence reply exceeds its display bound",
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
    ($value:expr, $maximum:expr) => {{
        let mut cap = ByteCap {
            used: 0,
            maximum: $maximum,
        };
        serde_json::to_writer(&mut cap, $value)
            .map(|_| cap.used)
            .map_err(|error| error.to_string())
    }};
}
fn receipt_valid(receipt: &EvidenceArchiveReceipt) -> bool {
    valid_path(&receipt.path)
        && valid_id(&receipt.export_id)
        && valid_sha(&receipt.sha256)
        && receipt.byte_length > 56
        && receipt.byte_length <= 536_870_912
        && receipt.file_count > 0
        && receipt
            .file_count
            .checked_add(receipt.directory_count)
            .is_some_and(|n| n <= MAX_EVIDENCE_ENTRIES as u64)
        && receipt.payload_bytes <= receipt.byte_length
}
fn outcome_valid(outcome: &EvidenceOutcome, request: &EvidenceJobRequest) -> Result<(), String> {
    bounded!(outcome, MAX_EVIDENCE_OUTCOME_REPLY_BYTES)?;
    match (outcome, &request.operation) {
        (
            EvidenceOutcome::Exported { receipt },
            EvidenceOperation::Export {
                export_id,
                destination,
                ..
            },
        ) => ensure(
            receipt_valid(receipt)
                && receipt.export_id == *export_id
                && exact_path(&receipt.path, destination),
            "Archive receipt differs from this exact export request",
        ),
        (
            EvidenceOutcome::Restored { receipt },
            EvidenceOperation::Restore {
                archive_path,
                directory,
                expected_archive_sha256,
            },
        ) => ensure(
            receipt_valid(&receipt.archive)
                && valid_path(&receipt.directory)
                && exact_path(&receipt.directory, directory)
                && exact_path(&receipt.archive.path, archive_path)
                && receipt.archive.sha256 == *expected_archive_sha256,
            "Restore receipt differs from this reviewed request",
        ),
        (EvidenceOutcome::Stopped { problem }, _) => ensure(
            problem.detail.message.len() <= MAX_EVIDENCE_JOB_MESSAGE_BYTES,
            "Evidence problem message exceeds its bound",
        ),
        _ => Err("The result operation differs from its request".into()),
    }
}
fn list_valid(list: &EvidenceJobList) -> Result<(), String> {
    bounded!(list, MAX_EVIDENCE_JOB_LIST_BYTES)?;
    ensure(
        list.jobs.len() <= MAX_EVIDENCE_JOBS,
        "Too many evidence jobs in this reply",
    )?;
    let mut seen = BTreeSet::new();
    for job in &list.jobs {
        ensure(
            valid_id(&job.job_id)
                && valid_sha(&job.history_sha256)
                && job.request_sha256.as_ref().is_none_or(|sha| valid_sha(sha))
                && job.metadata_problem_count <= MAX_METADATA_PROBLEMS
                && seen.insert(&job.job_id),
            "Malformed or repeated evidence job summary",
        )?;
    }
    Ok(())
}
fn view_valid(view: &EvidenceJobView, expected_id: &str) -> Result<(), String> {
    bounded!(view, MAX_EVIDENCE_JOB_VIEW_BYTES)?;
    ensure(
        view.job_id == expected_id
            && valid_id(&view.job_id)
            && valid_path(&view.directory)
            && valid_sha(&view.history_sha256)
            && view.occupied_observations <= MAX_EVIDENCE_JOB_OBSERVATIONS
            && view.observations.len() <= view.occupied_observations
            && view.metadata_problems.len() <= MAX_METADATA_PROBLEMS,
        "Malformed selected evidence job",
    )?;
    for problem in &view.metadata_problems {
        ensure(
            problem.slot.len() <= 64
                && problem.detail.message.len() <= MAX_EVIDENCE_JOB_MESSAGE_BYTES,
            "Evidence metadata problem exceeds its bound",
        )?;
    }
    match (&view.request, &view.request_sha256) {
        (Some(request), Some(sha)) => {
            ensure(
                request.job_id == view.job_id
                    && valid_sha(sha)
                    && request.fingerprint().is_ok_and(|actual| actual == *sha),
                "Selected job request fingerprint differs",
            )?;
            if let Some(outcome) = &view.terminal {
                outcome_valid(outcome, request)?;
            }
            let mut previous = 0;
            for observation in &view.observations {
                ensure(
                    observation.sequence > previous
                        && observation.sequence as usize <= view.occupied_observations
                        && valid_sha(&observation.previous_history_sha256),
                    "Malformed observation order or identity",
                )?;
                outcome_valid(&observation.outcome, request)?;
                previous = observation.sequence;
            }
        }
        (None, None) => ensure(
            view.terminal.is_none()
                && view.observations.is_empty()
                && !view.metadata_problems.is_empty(),
            "Invalid request carries an unbound outcome",
        )?,
        _ => return Err("Incomplete selected-job request identity".into()),
    }
    Ok(())
}

#[derive(Clone)]
struct Inspection {
    receipt: EvidenceArchiveReceipt,
    origin: String,
    absences: usize,
    generation: u64,
    serial: u64,
    current: bool,
}
fn prepare_inspection(
    dto: &EvidenceArchiveInspection,
    path: &Path,
    generation: u64,
    serial: u64,
) -> Result<Inspection, String> {
    // A receipt includes an independently bounded, JSON-escaped 4096-byte path.
    bounded!(
        dto,
        MAX_EVIDENCE_MANIFEST_BYTES + MAX_EVIDENCE_JOB_RECORD_BYTES
    )?;
    let manifest_size = bounded!(&dto.manifest, MAX_EVIDENCE_MANIFEST_BYTES)?;
    let manifest = &dto.manifest;
    ensure(
        receipt_valid(&dto.receipt)
            && exact_path(&dto.receipt.path, path)
            && manifest.version == 1
            && manifest.export_id == dto.receipt.export_id
            && manifest.scope == EVIDENCE_SCOPE
            && manifest.catalog_schema == omniusong_store::STORE_SCHEMA_VERSION
            && valid_path(Path::new(&manifest.origin_root))
            && !manifest.entries.is_empty()
            && manifest.entries.len() <= MAX_EVIDENCE_ENTRIES
            && manifest.absences.len() <= MAX_EVIDENCE_ENTRIES
            && manifest.roots.len() == 3
            && manifest.roots.iter().map(|root| root.path.as_str()).eq([
                "captures",
                "source-actions",
                "takes",
            ]),
        "Archive inspection identity, scope or bounds are invalid",
    )?;
    let mut files = 0u64;
    let mut directories = 0u64;
    let mut payload = 0u64;
    let mut seen = BTreeSet::new();
    for entry in &manifest.entries {
        ensure(
            !entry.path.is_empty()
                && entry.path.len() <= 4096
                && !entry.path.starts_with('/')
                && !entry.path.contains(['\\', ':'])
                && !entry.path.chars().any(char::is_control)
                && entry
                    .path
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..")
                && seen.insert(entry.path.to_lowercase()),
            "Archive member path is malformed or repeated",
        )?;
        match entry.kind {
            EvidenceEntryKind::File => {
                ensure(
                    entry.sha256.as_ref().is_some_and(|sha| valid_sha(sha))
                        && entry.byte_length <= 536_870_912,
                    "Archive object identity is invalid",
                )?;
                files += 1;
                payload = payload
                    .checked_add(entry.byte_length)
                    .ok_or("Archive payload length overflow")?;
            }
            EvidenceEntryKind::Directory => {
                ensure(
                    entry.byte_length == 0 && entry.sha256.is_none(),
                    "Archive directory carries file bytes",
                )?;
                directories += 1;
            }
        }
    }
    ensure(
        files == dto.receipt.file_count
            && directories == dto.receipt.directory_count
            && payload == dto.receipt.payload_bytes
            && payload.checked_add(56 + manifest_size as u64) == Some(dto.receipt.byte_length),
        "Archive receipt counts or container length differ from its manifest",
    )?;
    for absence in &manifest.absences {
        ensure(
            absence.path.len() <= 4096
                && !absence.reason.is_empty()
                && absence.reason.len() <= 256
                && !absence.reason.chars().any(char::is_control),
            "Archive absence facts exceed their bounds",
        )?;
    }
    Ok(Inspection {
        receipt: dto.receipt.clone(),
        origin: manifest.origin_root.clone(),
        absences: manifest.absences.len(),
        generation,
        serial,
        current: true,
    })
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Export,
    Restore,
    History,
}
struct Confirmation {
    request: Box<EvidenceJobRequest>,
    fingerprint: String,
    epoch: u64,
    generation: u64,
    serial: u64,
    inspection_serial: Option<u64>,
    approved: bool,
}
struct Pending {
    request: EvidenceUiRequest,
    id: RequestId,
    expected_request: Option<Box<EvidenceJobRequest>>,
    cancel_attempted: bool,
    cancel_requested: bool,
}
struct SelectedView {
    generation: u64,
    current: bool,
    value: Box<EvidenceJobView>,
}
struct Card {
    title: String,
    detail: String,
    generation: u64,
    historical: bool,
    job_id: Option<String>,
    request_sha: Option<String>,
    history_sha: Option<String>,
    request: Option<Box<EvidenceJobRequest>>,
    outcome: Option<EvidenceOutcome>,
    inspection: Option<Inspection>,
    metadata: Option<EvidenceMessage>,
    retirement: Option<String>,
}
#[derive(Default)]
pub(super) struct EvidenceUi {
    pub(super) open: bool,
    page: Page,
    export_path: String,
    archive_path: String,
    restore_directory: String,
    generation: Option<u64>,
    lost_generation: Option<u64>,
    origin: Option<PathBuf>,
    epoch: u64,
    serial: u64,
    exhausted: bool,
    offered: Option<EvidenceUiRequest>,
    confirmation: Option<Confirmation>,
    pending: Option<Pending>,
    inspection: Option<Inspection>,
    jobs: Option<EvidenceJobList>,
    jobs_generation: Option<u64>,
    selected: Option<String>,
    selected_view: Option<SelectedView>,
    cards: VecDeque<Card>,
    notice: Option<String>,
}
impl EvidenceUi {
    fn invalidate(&mut self, keep_inspection: bool) {
        match self.epoch.checked_add(1) {
            Some(next) => self.epoch = next,
            None => {
                self.exhausted = true;
                self.notice = Some("This review counter is exhausted. Open a fresh app view before another request.".into());
            }
        }
        self.offered = None;
        self.confirmation = None;
        if !keep_inspection && let Some(inspection) = &mut self.inspection {
            inspection.current = false;
        }
        if let Some(view) = &mut self.selected_view {
            view.current = false;
        }
    }
    pub(super) fn context_changed(&mut self) {
        self.invalidate(false);
    }
    pub(super) fn set_export_path(&mut self, text: String) {
        if self.export_path != text {
            self.export_path = text;
            self.invalidate(true);
        }
    }
    pub(super) fn set_archive_path(&mut self, text: String) {
        if self.archive_path != text {
            self.archive_path = text;
            self.invalidate(false);
        }
    }
    pub(super) fn set_restore_directory(&mut self, text: String) {
        if self.restore_directory != text {
            self.restore_directory = text;
            self.invalidate(true);
        }
    }
    pub(super) fn select_job(&mut self, id: &str) -> bool {
        if !valid_id(id)
            || !(self
                .jobs
                .as_ref()
                .is_some_and(|jobs| jobs.jobs.iter().any(|job| job.job_id == id))
                || self
                    .cards
                    .iter()
                    .any(|card| card.job_id.as_deref() == Some(id)))
        {
            return false;
        }
        if self.selected.as_deref() != Some(id) {
            self.invalidate(false);
            self.selected = Some(id.into());
        }
        true
    }
    fn current_generation(&self, generation: u64) -> bool {
        self.generation == Some(generation) && self.lost_generation != Some(generation)
    }
    fn next_serial(&mut self) -> Option<u64> {
        if self.exhausted {
            return None;
        }
        match self.serial.checked_add(1) {
            Some(next) => {
                self.serial = next;
                Some(next)
            }
            None => {
                self.exhausted = true;
                self.notice = Some(
                    "This review sequence is exhausted. No further request was prepared.".into(),
                );
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
        self.invalidate(false);
    }
    fn push_card(&mut self, card: Card) {
        while self.cards.len() >= MAX_CARDS {
            self.cards.pop_front();
        }
        self.cards.push_back(card);
    }
    fn request_card(
        request: &EvidenceUiRequest,
        expected: Option<Box<EvidenceJobRequest>>,
        title: &str,
        detail: &str,
        generation: u64,
        historical: bool,
    ) -> Card {
        let job_id = request.job_id().map(str::to_owned);
        let request_sha = expected
            .as_ref()
            .and_then(|request| request.fingerprint().ok())
            .or_else(|| match request {
                EvidenceUiRequest::Reconcile {
                    expected_request_sha256,
                    ..
                } => Some(expected_request_sha256.clone()),
                _ => None,
            });
        let history_sha = match request {
            EvidenceUiRequest::Reconcile {
                expected_history_sha256,
                ..
            } => Some(expected_history_sha256.clone()),
            _ => None,
        };
        Card {
            title: title.into(),
            detail: message(detail),
            generation,
            historical,
            job_id,
            request_sha,
            history_sha,
            request: expected,
            outcome: None,
            inspection: None,
            metadata: None,
            retirement: None,
        }
    }
    pub(super) fn worker_lost(&mut self, generation: u64, detail: &str) {
        if self.generation != Some(generation) || self.lost_generation == Some(generation) {
            return;
        }
        self.lost_generation = Some(generation);
        self.invalidate(false);
        if let Some(pending) = self.pending.take() {
            let explanation = if pending.request.is_run()
                || matches!(pending.request, EvidenceUiRequest::Reconcile { .. })
            {
                "The worker stopped without a confirmed reply. Partial or complete output may remain. Connect, list this job and reconcile explicitly."
            } else {
                "The read lost its worker. Previous observations were kept; choose another read explicitly."
            };
            self.push_card(Self::request_card(
                &pending.request,
                pending.expected_request,
                "Previous worker result is uncertain",
                explanation,
                generation,
                true,
            ));
        }
        self.notice = Some(message(detail));
    }
    fn offer(&mut self, request: EvidenceUiRequest) -> Option<EvidenceUiRequest> {
        if self.pending.is_some()
            || !self.current_generation(request.context().1)
            || request.command().validate().is_err()
        {
            return None;
        }
        self.offered = Some(request.clone());
        Some(request)
    }
    pub(super) fn inspect_request(&mut self) -> Option<EvidenceUiRequest> {
        let generation = self.generation?;
        let path = PathBuf::from(&self.archive_path);
        if !valid_path(&path) {
            self.notice =
                Some("Choose an exact absolute archive path of at most 4096 UTF-8 bytes.".into());
            return None;
        }
        let serial = self.next_serial()?;
        self.offer(EvidenceUiRequest::Inspect {
            path,
            view_epoch: self.epoch,
            generation,
            serial,
        })
    }
    pub(super) fn list_request(&mut self) -> Option<EvidenceUiRequest> {
        let generation = self.generation?;
        let serial = self.next_serial()?;
        self.offer(EvidenceUiRequest::List {
            view_epoch: self.epoch,
            generation,
            serial,
        })
    }
    pub(super) fn read_request(&mut self) -> Option<EvidenceUiRequest> {
        let job_id = self.selected.clone()?;
        let generation = self.generation?;
        let serial = self.next_serial()?;
        self.offer(EvidenceUiRequest::Read {
            job_id,
            view_epoch: self.epoch,
            generation,
            serial,
        })
    }
    fn current_view(&self) -> Option<&EvidenceJobView> {
        self.selected_view
            .as_ref()
            .filter(|view| {
                view.current
                    && self.current_generation(view.generation)
                    && self.selected.as_deref() == Some(view.value.job_id.as_str())
            })
            .map(|view| view.value.as_ref())
    }
    pub(super) fn reconcile_request(&mut self) -> Option<EvidenceUiRequest> {
        let view = self.current_view()?;
        if view.request.is_none() || view.occupied_observations >= MAX_EVIDENCE_JOB_OBSERVATIONS {
            return None;
        }
        let job_id = view.job_id.clone();
        let expected_request_sha256 = view.request_sha256.clone()?;
        let expected_history_sha256 = view.history_sha256.clone();
        let generation = self.generation?;
        let serial = self.next_serial()?;
        self.offer(EvidenceUiRequest::Reconcile {
            job_id,
            expected_request_sha256,
            expected_history_sha256,
            view_epoch: self.epoch,
            generation,
            serial,
        })
    }
    fn inspection_current(&self) -> Option<&Inspection> {
        self.inspection.as_ref().filter(|review| {
            review.current
                && self.current_generation(review.generation)
                && exact_path(&review.receipt.path, Path::new(&self.archive_path))
        })
    }
    fn sync_context(&mut self, availability: &EvidenceAvailability) {
        if let Some(generation) = availability.generation {
            self.generation_changed(generation);
        }
        let origin = availability.origin.as_ref().filter(|path| valid_path(path));
        let changed = match (&self.origin, origin) {
            (None, None) => false,
            (Some(old), Some(new)) => !exact_path(old, new),
            _ => true,
        };
        if changed {
            self.origin = origin.cloned();
            self.invalidate(false);
        }
    }
    fn review_origin(
        &mut self,
        availability: &EvidenceAvailability,
    ) -> Result<(u64, String), String> {
        self.sync_context(availability);
        let generation = availability
            .generation
            .ok_or("Connect storage before reviewing an operation")?;
        ensure(
            availability.can_run
                && availability.can_submit
                && self.pending.is_none()
                && self.current_generation(generation)
                && !self.exhausted,
            "Resolve local drafts, audio and other pending choices before reviewing this operation",
        )?;
        let origin = availability
            .origin
            .as_ref()
            .filter(|origin| valid_path(origin))
            .ok_or(
                "Open the existing studio to establish its exact origin before a new operation",
            )?;
        Ok((generation, origin.to_str().expect("validated path").into()))
    }
    pub(super) fn review_export(
        &mut self,
        availability: &EvidenceAvailability,
    ) -> Result<(), String> {
        let (generation, origin) = self.review_origin(availability)?;
        let destination = PathBuf::from(&self.export_path);
        ensure(
            valid_path(&destination),
            "Choose an exact absolute archive destination",
        )?;
        if self.confirmation.as_ref().is_some_and(|confirmation| confirmation.epoch == self.epoch && confirmation.generation == generation && confirmation.request.origin_root == origin && matches!(&confirmation.request.operation, EvidenceOperation::Export { destination: old, .. } if exact_path(old,&destination))) { return Ok(()); }
        let request = EvidenceJobRequest {
            version: 1,
            job_id: uuid::Uuid::new_v4().simple().to_string(),
            origin_root: origin,
            operation: EvidenceOperation::Export {
                scope: EVIDENCE_SCOPE.into(),
                export_id: uuid::Uuid::new_v4().simple().to_string(),
                destination,
            },
        };
        self.prepare_confirmation(request, generation, None)
    }
    pub(super) fn review_restore(
        &mut self,
        availability: &EvidenceAvailability,
    ) -> Result<(), String> {
        let (generation, origin) = self.review_origin(availability)?;
        let directory = PathBuf::from(&self.restore_directory);
        ensure(
            valid_path(&directory),
            "Choose an exact absolute new review folder",
        )?;
        let review = self
            .inspection_current()
            .ok_or("Inspect this archive in the current worker before reviewing its restore")?;
        let archive_path = review.receipt.path.clone();
        let expected_archive_sha256 = review.receipt.sha256.clone();
        let inspection_serial = review.serial;
        if self.confirmation.as_ref().is_some_and(|confirmation| confirmation.epoch == self.epoch && confirmation.generation == generation && confirmation.inspection_serial == Some(inspection_serial) && confirmation.request.origin_root == origin && matches!(&confirmation.request.operation, EvidenceOperation::Restore { archive_path: old, directory: target, expected_archive_sha256: sha } if exact_path(old,&archive_path) && exact_path(target,&directory) && sha == &expected_archive_sha256)) { return Ok(()); }
        let request = EvidenceJobRequest {
            version: 1,
            job_id: uuid::Uuid::new_v4().simple().to_string(),
            origin_root: origin,
            operation: EvidenceOperation::Restore {
                archive_path,
                directory,
                expected_archive_sha256,
            },
        };
        self.prepare_confirmation(request, generation, Some(inspection_serial))
    }
    fn prepare_confirmation(
        &mut self,
        request: EvidenceJobRequest,
        generation: u64,
        inspection_serial: Option<u64>,
    ) -> Result<(), String> {
        bounded!(&request, MAX_EVIDENCE_JOB_REQUEST_BYTES)?;
        let fingerprint = request.fingerprint().map_err(|error| error.message)?;
        let serial = self
            .next_serial()
            .ok_or("The confirmation counter is exhausted")?;
        self.offered = None;
        self.confirmation = Some(Confirmation {
            request: Box::new(request),
            fingerprint,
            epoch: self.epoch,
            generation,
            serial,
            inspection_serial,
            approved: false,
        });
        self.notice = None;
        Ok(())
    }
    pub(super) fn confirm_request(&mut self) -> Option<EvidenceUiRequest> {
        let confirmation = self.confirmation.as_ref()?;
        if confirmation.epoch != self.epoch
            || !self.current_generation(confirmation.generation)
            || self.pending.is_some()
            || self.exhausted
        {
            return None;
        }
        if let Some(serial) = confirmation.inspection_serial
            && self
                .inspection_current()
                .is_none_or(|review| review.serial != serial)
        {
            return None;
        }
        self.confirmation.as_mut()?.approved = true;
        self.run_request()
    }
    pub(super) fn run_request(&self) -> Option<EvidenceUiRequest> {
        let confirmation = self.confirmation.as_ref()?;
        if !confirmation.approved
            || confirmation.epoch != self.epoch
            || !self.current_generation(confirmation.generation)
            || self.pending.is_some()
            || self.exhausted
        {
            return None;
        }
        if let Some(serial) = confirmation.inspection_serial
            && self
                .inspection_current()
                .is_none_or(|review| review.serial != serial)
        {
            return None;
        }
        Some(EvidenceUiRequest::Run {
            request: confirmation.request.clone(),
            view_epoch: confirmation.epoch,
            generation: confirmation.generation,
            confirmation_serial: confirmation.serial,
        })
    }
    pub(super) fn authorizes(&self, request: &EvidenceUiRequest, generation: u64) -> bool {
        let (epoch, requested_generation, _) = request.context();
        if self.pending.is_some()
            || self.exhausted
            || epoch != self.epoch
            || requested_generation != generation
            || !self.current_generation(generation)
            || request.command().validate().is_err()
        {
            return false;
        }
        if request.is_run() {
            self.run_request()
                .is_some_and(|expected| expected.same(request))
        } else {
            self.offered
                .as_ref()
                .is_some_and(|expected| expected.same(request))
        }
    }
    pub(super) fn submitted(&mut self, request: &EvidenceUiRequest, id: RequestId) {
        if !self.authorizes(request, id.generation) {
            self.notice =
                Some("A stale evidence request was not associated with this view.".into());
            return;
        }
        let expected_request = match request {
            EvidenceUiRequest::Run { request, .. } => Some(request.clone()),
            EvidenceUiRequest::Reconcile { .. } => self
                .current_view()
                .and_then(|view| view.request.clone())
                .map(Box::new),
            _ => None,
        };
        // Only accepted reads retire the prior proof. An offered or refused
        // request cannot change which observation the user is reviewing.
        if matches!(request, EvidenceUiRequest::Inspect { .. }) {
            if let Some(inspection) = &mut self.inspection {
                inspection.current = false;
            }
            if self
                .confirmation
                .as_ref()
                .is_some_and(|confirmation| confirmation.inspection_serial.is_some())
            {
                self.confirmation = None;
            }
        }
        if matches!(
            request,
            EvidenceUiRequest::Read { .. } | EvidenceUiRequest::Reconcile { .. }
        ) && let Some(view) = &mut self.selected_view
        {
            view.current = false;
        }
        self.pending = Some(Pending {
            request: request.clone(),
            id,
            expected_request,
            cancel_attempted: false,
            cancel_requested: false,
        });
        self.offered = None;
        if request.is_run() {
            self.confirmation = None;
        }
        self.notice = None;
    }
    pub(super) fn submission_failed(&mut self, request: &EvidenceUiRequest, detail: &str) {
        let exact = self
            .offered
            .as_ref()
            .is_some_and(|offered| offered.same(request))
            || self.run_request().is_some_and(|run| run.same(request));
        if !exact {
            return;
        }
        self.offered = None;
        if let Some(confirmation) = &mut self.confirmation {
            confirmation.approved = false;
        }
        self.notice = Some(message(detail));
    }
    pub(super) fn pending_cancel(&self) -> Option<RequestId> {
        self.pending
            .as_ref()
            .filter(|pending| pending.request.cancellable())
            .map(|pending| pending.id)
    }
    pub(super) fn cancel_requested(&mut self, id: RequestId, accepted: bool) {
        if let Some(pending) = &mut self.pending
            && pending.id == id
            && pending.request.cancellable()
            && !pending.cancel_attempted
        {
            pending.cancel_attempted = true;
            pending.cancel_requested = accepted;
            self.notice = Some(if accepted { "Cancellation requested. Waiting for the exact result; partial or complete output may remain." } else { "This cancellation was not accepted. The pending request was kept." }.into());
        }
    }

    fn reply_matches(request: &EvidenceUiRequest, reply: &Reply) -> bool {
        match (request, reply) {
            (
                EvidenceUiRequest::Inspect { path, .. },
                Reply::RetainedEvidenceInspected(inspection),
            ) => exact_path(path, &inspection.receipt.path),
            (EvidenceUiRequest::List { .. }, Reply::EvidenceJobs(_)) => true,
            (EvidenceUiRequest::Read { job_id, .. }, Reply::EvidenceJob(view)) => {
                job_id == &view.job_id
            }
            (EvidenceUiRequest::Run { request, .. }, Reply::EvidenceJobOutcome(reply)) => {
                request.job_id == reply.job_id
                    && request
                        .fingerprint()
                        .is_ok_and(|sha| sha == reply.request_sha256)
            }
            (
                EvidenceUiRequest::Reconcile {
                    job_id,
                    expected_request_sha256,
                    ..
                },
                Reply::EvidenceJobOutcome(reply),
            ) => job_id == &reply.job_id && expected_request_sha256 == &reply.request_sha256,
            _ => false,
        }
    }
    fn prepare_job_reply(
        reply: &EvidenceJobReply,
        expected: &EvidenceJobRequest,
    ) -> Result<(), String> {
        ensure(
            valid_id(&reply.job_id)
                && reply.job_id == expected.job_id
                && valid_sha(&reply.request_sha256)
                && expected
                    .fingerprint()
                    .is_ok_and(|sha| sha == reply.request_sha256)
                && reply
                    .history_sha256
                    .as_ref()
                    .is_none_or(|sha| valid_sha(sha)),
            "Evidence result identity differs from this exact request",
        )?;
        if let Some(outcome) = &reply.outcome {
            outcome_valid(outcome, expected)?;
        }
        if let Some(problem) = &reply.metadata_problem {
            ensure(
                problem.message.len() <= MAX_EVIDENCE_JOB_MESSAGE_BYTES,
                "Evidence history message exceeds its bound",
            )?;
        }
        if let Some(retirement) = &reply.catalog_retirement {
            ensure(
                retirement.kind == FailureKind::StorageUnavailable
                    && retirement.message.len() <= MAX_EVIDENCE_JOB_MESSAGE_BYTES,
                "Malformed storage-worker retirement notice",
            )?;
        }
        Ok(())
    }
    pub(super) fn completed(
        &mut self,
        request: &EvidenceUiRequest,
        id: RequestId,
        result: Result<&Reply, &ServiceError>,
    ) {
        if !self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == id && pending.request.same(request))
        {
            return;
        }
        if let Ok(reply) = result
            && !Self::reply_matches(request, reply)
        {
            self.notice = Some(
                "A different evidence reply was ignored. Waiting for the matching request.".into(),
            );
            return;
        }
        let pending = self.pending.take().expect("matching pending request");
        let current = self.current_generation(id.generation) && request.context().0 == self.epoch;
        let mut card = Self::request_card(
            request,
            pending.expected_request,
            "Evidence result",
            "",
            id.generation,
            !current,
        );
        let prepared = (|| {
            match result {
                Err(error) => {
                    card.title = if error.outcome_unknown
                        || request.is_run()
                        || matches!(request, EvidenceUiRequest::Reconcile { .. })
                    {
                        "Result remains uncertain"
                    } else {
                        "Read did not complete"
                    }
                    .into();
                    card.detail = message(&error.message);
                    if current {
                        self.notice = Some(message(&error.message));
                    }
                }
                Ok(Reply::RetainedEvidenceInspected(dto)) => {
                    let EvidenceUiRequest::Inspect { path, serial, .. } = request else {
                        unreachable!()
                    };
                    let mut inspection = prepare_inspection(dto, path, id.generation, *serial)?;
                    inspection.current = current;
                    card.title = if current {
                        "Archive reviewed"
                    } else {
                        "Previous archive review"
                    }
                    .into();
                    card.detail = if pending.cancel_requested { "Inspection finished before cancellation took effect. This review does not restore or add anything to a song." } else { "Archive bytes and retained-reference closure were checked by storage. Restore is a separate explicit choice." }.into();
                    if current {
                        self.inspection = Some(inspection.clone());
                    }
                    card.inspection = Some(inspection);
                }
                Ok(Reply::EvidenceJobs(jobs)) => {
                    list_valid(jobs)?;
                    card.title = "Job list read".into();
                    card.detail = format!(
                        "{} saved jobs observed. Selecting a row does not read or reconcile its output.",
                        jobs.jobs.len()
                    );
                    if current {
                        self.jobs = Some(jobs.clone());
                        self.jobs_generation = Some(id.generation);
                    }
                }
                Ok(Reply::EvidenceJob(view)) => {
                    let EvidenceUiRequest::Read { job_id, .. } = request else {
                        unreachable!()
                    };
                    view_valid(view, job_id)?;
                    card.title = "Saved job history".into();
                    card.detail = "Historical records are not a new check that output still exists. Reconcile explicitly to verify the recorded output.".into();
                    card.request_sha = view.request_sha256.clone();
                    card.history_sha = Some(view.history_sha256.clone());
                    card.request = view.request.clone().map(Box::new);
                    if current && self.selected.as_deref() == Some(job_id) {
                        self.selected_view = Some(SelectedView {
                            generation: id.generation,
                            current: true,
                            value: view.clone(),
                        });
                    }
                }
                Ok(Reply::EvidenceJobOutcome(reply)) => {
                    let expected = card
                        .request
                        .as_ref()
                        .ok_or("This outcome has no bound current request")?;
                    Self::prepare_job_reply(reply, expected)?;
                    let reconcile = matches!(request, EvidenceUiRequest::Reconcile { .. });
                    ensure(
                        !reconcile || reply.catalog_retirement.is_none(),
                        "Read-only reconciliation carried a live storage-retirement claim",
                    )?;
                    card.title = if reconcile {
                        if reply.attempted {
                            "Reconciliation observation"
                        } else {
                            "Reconciliation did not start"
                        }
                    } else if reply.attempted {
                        "Operation result"
                    } else {
                        "Existing job · no new execution"
                    }
                    .into();
                    card.detail = if reply.outcome.as_ref().is_some_and(EvidenceOutcome::completed) { if !reconcile && !reply.attempted { "Historical receipt only. No new execution or artifact check occurred." } else if pending.cancel_requested { "Completion was reported before cancellation stopped the operation. Nothing was rolled back." } else if reconcile { "The recorded existing output was checked. Its original terminal record was kept." } else { "This is the reported result for this exact job. A later error or lost reply can still require reconciliation." } } else { "Completion is not confirmed. Partial or complete output may remain. Read this job and reconcile explicitly; starting a new job is not a retry." }.into();
                    card.history_sha = reply.history_sha256.clone();
                    card.outcome = reply.outcome.clone();
                    card.metadata = reply.metadata_problem.clone();
                    card.retirement = reply
                        .catalog_retirement
                        .as_ref()
                        .map(|error| error.message.clone());
                }
                Ok(_) => unreachable!("matched evidence reply variant"),
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = prepared {
            card.title = "Evidence reply was not installed".into();
            card.detail = message(&error);
            card.outcome = None;
            card.inspection = None;
            if current {
                self.notice = Some(message(&error));
            }
        } else if result.is_ok() && current {
            self.notice = None;
        }
        let retired = card.retirement.is_some()
            || matches!(result, Err(error) if matches!(error.kind, FailureKind::StorageUnavailable | FailureKind::Disconnected));
        self.push_card(card);
        if retired {
            self.worker_lost(id.generation, "The storage worker stopped. Previous results and local fields were kept; connect explicitly to read history.");
        }
    }

    fn identity(ui: &mut egui::Ui, label: &str, text: &str) {
        ui.small(label);
        ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
        if ui.small_button(format!("Copy {label}")).clicked() {
            ui.ctx().copy_text(text.to_owned());
        }
    }
    fn request_details(ui: &mut egui::Ui, request: &EvidenceJobRequest) {
        Self::identity(ui, "Job ID", &request.job_id);
        Self::identity(ui, "Studio origin", &request.origin_root);
        match &request.operation {
            EvidenceOperation::Export {
                destination,
                export_id,
                ..
            } => {
                Self::identity(
                    ui,
                    "Archive destination",
                    destination.to_str().expect("validated request path"),
                );
                Self::identity(ui, "Export ID", export_id);
            }
            EvidenceOperation::Restore {
                archive_path,
                directory,
                expected_archive_sha256,
            } => {
                Self::identity(
                    ui,
                    "Reviewed archive",
                    archive_path.to_str().expect("validated request path"),
                );
                Self::identity(
                    ui,
                    "New review folder",
                    directory.to_str().expect("validated request path"),
                );
                Self::identity(
                    ui,
                    "Reviewed whole-archive SHA-256",
                    expected_archive_sha256,
                );
            }
        }
    }
    fn outcome_details(ui: &mut egui::Ui, outcome: &EvidenceOutcome) {
        match outcome {
            EvidenceOutcome::Exported { receipt } => {
                ui.strong("Archive completion reported");
                Self::identity(
                    ui,
                    "Archive path",
                    receipt.path.to_str().expect("validated receipt path"),
                );
                Self::identity(ui, "Whole-archive SHA-256", &receipt.sha256);
                ui.label(format!(
                    "{} bytes · {} files · {} directories",
                    receipt.byte_length, receipt.file_count, receipt.directory_count
                ));
            }
            EvidenceOutcome::Restored { receipt } => {
                ui.strong("Separate review folder completion reported");
                Self::identity(
                    ui,
                    "Review folder",
                    receipt.directory.to_str().expect("validated receipt path"),
                );
                Self::identity(ui, "Whole-archive SHA-256", &receipt.archive.sha256);
                ui.small("Numbered objects stay separate. No song, recording owner or action is adopted.");
            }
            EvidenceOutcome::Stopped { problem } => {
                ui.strong(match problem.kind {
                    EvidenceProblemKind::Cancelled => "Stopped after cancellation",
                    EvidenceProblemKind::Limit => "Stopped at a bounded inspection limit",
                    EvidenceProblemKind::Busy => "An existing owner was busy",
                    _ => "Completion was not confirmed",
                });
                ui.add(egui::Label::new(&problem.detail.message).wrap());
                if let Some(stop) = &problem.budget_stop {
                    match stop {
                        EvidenceBudgetStop::Cancelled => {
                            ui.label("Cancellation was also observed by the inspection governor.");
                        }
                        EvidenceBudgetStop::Limit {
                            resource,
                            limit,
                            attempted,
                        } => {
                            ui.add(egui::Label::new(format!("Inspection limit also observed: {resource:?} · limit {limit} · attempted {attempted}")).wrap());
                        }
                    }
                }
                if let Some(code) = problem.sqlite_code {
                    ui.small(format!("Recorded SQLite result code: {code}"));
                }
                if problem.detail.message_truncated {
                    ui.small(
                        "The explanatory message was shortened; identity fields were kept exact.",
                    );
                }
                ui.small("Partial or complete output can remain. Cancellation is not rollback.");
            }
        }
    }
    fn inspection_details(ui: &mut egui::Ui, inspection: &Inspection) {
        Self::identity(
            ui,
            "Archive path",
            inspection
                .receipt
                .path
                .to_str()
                .expect("validated inspection path"),
        );
        Self::identity(ui, "Whole-archive SHA-256", &inspection.receipt.sha256);
        Self::identity(ui, "Recorded studio origin", &inspection.origin);
        ui.label(format!(
            "{} bytes · {} files · {} directories · {} recorded absences",
            inspection.receipt.byte_length,
            inspection.receipt.file_count,
            inspection.receipt.directory_count,
            inspection.absences
        ));
    }
    fn phase_text(phase: EvidenceJobPhase) -> &'static str {
        match phase {
            EvidenceJobPhase::InvalidRequest => "Request metadata needs review",
            EvidenceJobPhase::Unresolved => "Request retained · unresolved",
            EvidenceJobPhase::Started => "Started · outcome uncertain",
            EvidenceJobPhase::HistoricalComplete => "Completion reported · historical",
            EvidenceJobPhase::Stopped => "Stopped · output may remain",
        }
    }
    fn history_ui(
        &mut self,
        ui: &mut egui::Ui,
        ready: bool,
        artifact_ready: bool,
    ) -> Option<EvidenceAction> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new("List jobs"))
                .clicked()
                && let Some(request) = self.list_request()
            {
                action = Some(EvidenceAction::Submit(request));
            }
            if ui
                .add_enabled(
                    ready && self.selected.is_some(),
                    egui::Button::new("Read selected job"),
                )
                .clicked()
                && let Some(request) = self.read_request()
            {
                action = Some(EvidenceAction::Submit(request));
            }
        });
        ui.small("List and Read inspect saved job metadata. They never resume an operation or check its archive automatically.");
        let mut selected = None;
        if let Some(jobs) = &self.jobs {
            ui.small(format!(
                "{} jobs · list from worker {}",
                jobs.jobs.len(),
                self.jobs_generation.unwrap_or_default()
            ));
            egui::ScrollArea::vertical()
                .id_salt("evidence-job-rows")
                .max_height(170.0)
                .show_rows(ui, 46.0, jobs.jobs.len(), |ui, range| {
                    for index in range {
                        let job = &jobs.jobs[index];
                        ui.push_id(&job.job_id, |ui| {
                            if ui
                                .add_sized(
                                    [ui.available_width(), 22.0],
                                    egui::Button::selectable(
                                        self.selected.as_deref() == Some(&job.job_id),
                                        &job.job_id,
                                    )
                                    .wrap(),
                                )
                                .clicked()
                            {
                                selected = Some(job.job_id.clone());
                            }
                            ui.small(Self::phase_text(job.phase));
                        });
                    }
                });
        } else {
            ui.small("No job list has been read in this view.");
        }
        if let Some(id) = selected {
            self.select_job(&id);
        }
        if let Some(view) = &self.selected_view {
            let current = self.current_view().is_some();
            ui.separator();
            ui.strong(if current {
                "Selected job history"
            } else {
                "Previous selected-job history"
            });
            Self::identity(ui, "Saved job ID", &view.value.job_id);
            Self::identity(ui, "History SHA-256", &view.value.history_sha256);
            if let Some(sha) = &view.value.request_sha256 {
                Self::identity(ui, "Request SHA-256", sha);
            }
            ui.label(format!(
                "{} of {} observation positions used",
                view.value.occupied_observations, MAX_EVIDENCE_JOB_OBSERVATIONS
            ));
            if let Some(request) = &view.value.request {
                ui.collapsing("Saved request", |ui| Self::request_details(ui, request));
            }
            if let Some(outcome) = &view.value.terminal {
                ui.collapsing("Original terminal record · historical", |ui| {
                    Self::outcome_details(ui, outcome)
                });
            }
            for observation in &view.value.observations {
                ui.collapsing(
                    format!("Observation {} · historical", observation.sequence),
                    |ui| Self::outcome_details(ui, &observation.outcome),
                );
            }
            for problem in &view.value.metadata_problems {
                ui.add(
                    egui::Label::new(format!("{}: {}", problem.slot, problem.detail.message))
                        .wrap(),
                );
            }
            let may_reconcile = artifact_ready
                && current
                && view.value.request.is_some()
                && view.value.request_sha256.is_some()
                && view.value.occupied_observations < MAX_EVIDENCE_JOB_OBSERVATIONS;
            if ui
                .add_enabled(may_reconcile, egui::Button::new("Reconcile selected job"))
                .clicked()
                && let Some(request) = self.reconcile_request()
            {
                action = Some(EvidenceAction::Submit(request));
            }
            ui.small("Reconciliation checks the recorded existing output and adds one observation. It never retries, repairs or adopts it.");
        }
        action
    }
    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        availability: &EvidenceAvailability,
    ) -> Option<EvidenceAction> {
        self.sync_context(availability);
        let ready = availability.can_submit
            && self.pending.is_none()
            && availability
                .generation
                .is_some_and(|generation| self.current_generation(generation))
            && !self.exhausted;
        let artifact_ready = ready && availability.can_read_artifacts;
        let run_ready = ready
            && availability.can_run
            && availability
                .origin
                .as_ref()
                .is_some_and(|path| valid_path(path));
        let mut action = None;
        ui.strong("Studio evidence");
        ui.add(egui::Label::new("Archive the catalog and retained recordings. Restore creates a separate review folder; nothing is added to a song.").wrap());
        ui.add(egui::Label::new(egui::RichText::new("Current limits: 512 MiB per archive · 1 GiB cumulative inspection work. Large studios may exceed these limits.").small()).wrap());
        // This urgent control stays above every scrolling/collapsible detail.
        if let Some(pending) = &self.pending {
            ui.group(|ui| {
                ui.strong(if pending.request.is_run() {
                    "Evidence operation is running"
                } else if matches!(pending.request, EvidenceUiRequest::Reconcile { .. }) {
                    "Reconciliation is running"
                } else {
                    "Evidence read is running"
                });
                if let Some(job) = pending.request.job_id() {
                    ui.add(egui::Label::new(format!("Job {job}")).wrap());
                }
                if pending.cancel_requested {
                    ui.label("Cancellation requested · waiting for the matching result.");
                }
                if pending.request.cancellable()
                    && ui
                        .add_enabled(
                            !pending.cancel_attempted,
                            egui::Button::new("Cancel evidence request"),
                        )
                        .clicked()
                {
                    action = Some(EvidenceAction::Cancel(pending.id));
                }
            });
        }
        if !ready && let Some(reason) = &availability.unavailable_reason {
            ui.add(egui::Label::new(message(reason)).wrap());
        }
        let disconnected = availability.generation.is_none()
            || availability
                .generation
                .is_some_and(|generation| self.lost_generation == Some(generation));
        if disconnected && self.pending.is_none() && ui.button("Connect history").clicked() {
            action = Some(EvidenceAction::MetadataConnect);
        }
        if let Some(notice) = &self.notice {
            ui.add(egui::Label::new(notice).wrap());
        }
        ui.separator();
        let height = ui.available_height().clamp(0.0, 640.0);
        egui::ScrollArea::vertical().id_salt("studio-evidence-content").max_height(height).show(ui, |ui| {
            let old_page = self.page;
            ui.horizontal_wrapped(|ui| { ui.selectable_value(&mut self.page, Page::Export, "Export"); ui.selectable_value(&mut self.page, Page::Restore, "Inspect & restore"); ui.selectable_value(&mut self.page, Page::History, "Job history"); });
            if self.page != old_page { self.invalidate(false); }
            ui.add_space(6.0);
            match self.page {
                Page::Export => {
                    ui.strong("Save full studio evidence"); ui.small("Catalog + all retained captures, source actions and takes. Choose a new destination.");
                    ui.label("Archive destination");
                    let mut text = self.export_path.clone();
                    if ui.add(egui::TextEdit::singleline(&mut text).char_limit(4096).desired_width(ui.available_width()).hint_text("/absolute/path/studio-evidence.osre")).accessible_name("Evidence archive destination").changed() { self.set_export_path(text); }
                    if ui.add_enabled(run_ready, egui::Button::new("Review export")).clicked() && let Err(error) = self.review_export(availability) { self.notice = Some(message(&error)); }
                }
                Page::Restore => {
                    ui.strong("Review an archive, then restore separately");
                    ui.label("Archive path"); let mut text = self.archive_path.clone();
                    if ui.add(egui::TextEdit::singleline(&mut text).char_limit(4096).desired_width(ui.available_width()).hint_text("/absolute/path/archive.osre")).accessible_name("Evidence archive to inspect").changed() { self.set_archive_path(text); }
                    ui.label("New review folder"); let mut text = self.restore_directory.clone();
                    if ui.add(egui::TextEdit::singleline(&mut text).char_limit(4096).desired_width(ui.available_width()).hint_text("/absolute/path/new-review-folder")).accessible_name("Evidence restore review folder").changed() { self.set_restore_directory(text); }
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(artifact_ready, egui::Button::new("Inspect archive")).clicked() && let Some(request) = self.inspect_request() { action = Some(EvidenceAction::Submit(request)); }
                        if ui.add_enabled(run_ready && self.inspection_current().is_some(), egui::Button::new("Review inert restore")).clicked() && let Err(error) = self.review_restore(availability) { self.notice = Some(message(&error)); }
                    });
                    if let Some(inspection) = &self.inspection { ui.collapsing(if self.inspection_current().is_some() { "Current archive review" } else { "Previous archive review · inspect again" }, |ui| Self::inspection_details(ui, inspection)); }
                }
                Page::History => { if let Some(request) = self.history_ui(ui, ready, artifact_ready) { action = Some(request); } }
            }
            if let Some(confirmation) = &self.confirmation {
                ui.separator(); ui.group(|ui| {
                    ui.strong("Review this exact operation"); Self::request_details(ui, &confirmation.request); Self::identity(ui, "Request fingerprint", &confirmation.fingerprint);
                    ui.small("A new job is a separate operation, not a retry of uncertain output. No existing destination is overwritten.");
                });
                let label = if matches!(confirmation.request.operation, EvidenceOperation::Restore { .. }) { "Confirm inert restore" } else { "Confirm export" };
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(run_ready, egui::Button::new(label)).clicked() && let Some(request) = self.confirm_request() { action = Some(EvidenceAction::Submit(request)); }
                    if ui.button("Cancel this review").clicked() { self.invalidate(true); }
                });
            }
            if !self.cards.is_empty() { ui.separator(); ui.strong("Recent results"); ui.small("Eight recent cards are shown here. Durable job history is read only when requested."); }
            let mut read_job = None;
            for (index, card) in self.cards.iter().enumerate().rev() {
                ui.push_id(("evidence-card", index), |ui| {
                    let label = format!("{}{} · worker {}", if card.historical { "Previous context · " } else { "" }, card.title, card.generation);
                    ui.collapsing(label, |ui| {
                        ui.add(egui::Label::new(&card.detail).wrap());
                        if let Some(request) = &card.request { Self::request_details(ui, request); }
                        else if let Some(job) = &card.job_id { Self::identity(ui, "Job ID", job); }
                        if let Some(sha) = &card.request_sha { Self::identity(ui, "Request SHA-256", sha); }
                        if let Some(sha) = &card.history_sha { Self::identity(ui, "History SHA-256", sha); }
                        if let Some(outcome) = &card.outcome { Self::outcome_details(ui, outcome); }
                        if let Some(inspection) = &card.inspection { Self::inspection_details(ui, inspection); }
                        if let Some(problem) = &card.metadata { ui.strong("History persistence needs review"); ui.add(egui::Label::new(&problem.message).wrap()); if problem.message_truncated { ui.small("Explanatory message shortened."); } }
                        if let Some(retirement) = &card.retirement { ui.strong("The storage worker stopped; the primary result above was kept"); ui.add(egui::Label::new(retirement).wrap()); }
                        if let Some(job) = &card.job_id && ui.add_enabled(ready, egui::Button::new("Read this job")).clicked() { read_job = Some(job.clone()); }
                    });
                });
            }
            if let Some(job) = read_job && self.select_job(&job) {
                self.page = Page::History;
                if let Some(request) = self.read_request() { action = Some(EvidenceAction::Submit(request)); }
            }
        });
        action
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(super) struct EvidenceTestCard {
    pub(super) job_id: Option<String>,
    pub(super) request: Option<Box<EvidenceJobRequest>>,
    pub(super) outcome: Option<EvidenceOutcome>,
    pub(super) metadata: Option<EvidenceMessage>,
    pub(super) retirement: Option<String>,
    pub(super) historical: bool,
}
#[cfg(test)]
impl EvidenceUi {
    pub(super) fn cards_for_test(&self) -> Vec<EvidenceTestCard> {
        self.cards
            .iter()
            .map(|card| EvidenceTestCard {
                job_id: card.job_id.clone(),
                request: card.request.clone(),
                outcome: card.outcome.clone(),
                metadata: card.metadata.clone(),
                retirement: card.retirement.clone(),
                historical: card.historical,
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "evidence_ui/tests.rs"]
mod tests;
