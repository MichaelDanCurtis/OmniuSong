//! Explicit, bounded saved-performance review. This module performs no source,
//! filesystem, engine, recovery, publication or musical actions.
use eframe::egui;
use omniusong_session::{
    Command, FailureKind, PerformanceIntentReceipt, PerformanceSourceReview, PerformanceSourceScan,
    Reply, RequestId, ServiceError,
};
use std::{collections::BTreeMap, io::Write, path::Path};

const MAX_ROWS: usize = 160;
const STREAMS: [&str; 6] = ["audio", "gui-1", "gui-2", "gui-3", "gui-4", "control"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SourceRequest {
    Scan,
    RefreshRequests,
    Inspect { stage_id: String, view_epoch: u64 },
}

impl SourceRequest {
    pub(super) fn command(&self) -> Command {
        match self {
            Self::Scan => Command::ScanPerformanceSources,
            Self::RefreshRequests => Command::ListPerformanceIntents,
            Self::Inspect { stage_id, .. } => Command::InspectPerformanceSource {
                stage_id: stage_id.clone(),
            },
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SourceAction {
    Submit(SourceRequest),
    CancelInspection(RequestId),
}
pub(super) struct SourceAvailability {
    pub(super) can_submit: bool,
    pub(super) generation: Option<u64>,
    pub(super) unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ComparisonReason {
    StartIdentity,
    TakeIdentity,
    JournalPath,
    GuiClock,
    Descriptor,
    Finish,
    PublicationManifest,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FactState {
    Matches,
    Conflict,
    NotObserved,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ComparisonFact {
    pub(super) reason: ComparisonReason,
    pub(super) state: FactState,
    pub(super) text: String,
}
fn fact(
    reason: ComparisonReason,
    equal: bool,
    label: &str,
    saved: String,
    observed: String,
) -> ComparisonFact {
    ComparisonFact {
        reason,
        state: if equal {
            FactState::Matches
        } else {
            FactState::Conflict
        },
        text: summary(&format!("{label}: saved {saved}; observed {observed}")),
    }
}
fn clock_text(clock: &omniusong_engine::LiveClock) -> String {
    summary(&format!(
        "{} / {} · {}/{}",
        clock.domain,
        clock.epoch,
        clock.ticks_per_second_numerator,
        clock.ticks_per_second_denominator
    ))
}
fn scope_text(scope: &omniusong_engine::LiveScope) -> String {
    format!(
        "worker {} · session {} · Panic {}",
        hex(&scope.worker_generation.0),
        hex(&scope.session_id.0),
        scope.panic_generation
    )
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
/// Reconciliation facts only. No eligibility, replay or attachment capability.
pub(super) fn compare_request(
    saved: &PerformanceIntentReceipt,
    observed: &PerformanceSourceReview,
) -> Vec<ComparisonFact> {
    let intent = &saved.intent;
    let mut facts = vec![
        fact(
            ComparisonReason::StartIdentity,
            intent.start.directory_name() == observed.stage_id,
            "Start identity",
            intent.start.directory_name(),
            observed.stage_id.clone(),
        ),
        fact(
            ComparisonReason::TakeIdentity,
            intent.start.take_id() == observed.take_id,
            "Take identity",
            intent.start.take_id(),
            observed.take_id.clone(),
        ),
        fact(
            ComparisonReason::JournalPath,
            intent.journal_path == observed.journal_path,
            "Journal path",
            intent.journal_path.to_string_lossy().into_owned(),
            observed.journal_path.to_string_lossy().into_owned(),
        ),
        fact(
            ComparisonReason::GuiClock,
            intent.start.gui_clock == observed.config.gui_clock,
            "GUI clock",
            clock_text(&intent.start.gui_clock),
            clock_text(&observed.config.gui_clock),
        ),
    ];
    facts.push(match &intent.descriptor_sha256 {
        Some(sha) => fact(
            ComparisonReason::Descriptor,
            *sha == observed.performance.descriptor_sha256,
            "Descriptor",
            sha.clone(),
            observed.performance.descriptor_sha256.clone(),
        ),
        None => ComparisonFact {
            reason: ComparisonReason::Descriptor,
            state: FactState::NotObserved,
            text: "No descriptor identity was saved with this request.".into(),
        },
    });
    facts.push(match (&intent.finish, &observed.performance.finish_cut) {
        (Some(request), Some(cut))
            if request.id == cut.fence.id && request.scope == cut.fence.scope =>
        {
            ComparisonFact {
                reason: ComparisonReason::Finish,
                state: FactState::Matches,
                text: "Saved Finish request matches observed callback cut.".into(),
            }
        }
        (Some(request), Some(cut)) => fact(
            ComparisonReason::Finish,
            false,
            "Finish conflict",
            format!("{} · {}", hex(&request.id.0), scope_text(&request.scope)),
            format!(
                "{} · {}",
                hex(&cut.fence.id.0),
                scope_text(&cut.fence.scope)
            ),
        ),
        (Some(_), None) => ComparisonFact {
            reason: ComparisonReason::Finish,
            state: FactState::NotObserved,
            text: "Finish request retained · callback cut not observed.".into(),
        },
        (None, Some(_)) => ComparisonFact {
            reason: ComparisonReason::Finish,
            state: FactState::NotObserved,
            text: "Callback Finish observed · no saved Finish request listed.".into(),
        },
        (None, None) => ComparisonFact {
            reason: ComparisonReason::Finish,
            state: FactState::NotObserved,
            text: "No saved Finish request or callback cut was observed.".into(),
        },
    });
    if let Some(publication) = &intent.attachment {
        facts.push(match &observed.manifest_sha256 {
            Some(sha) => fact(
                ComparisonReason::PublicationManifest,
                publication.clip.source.manifest_sha256 == *sha,
                "Saved attachment manifest",
                publication.clip.source.manifest_sha256.clone(),
                sha.clone(),
            ),
            None => ComparisonFact {
                reason: ComparisonReason::PublicationManifest,
                state: FactState::NotObserved,
                text: "Saved attachment request retained · source manifest not observed.".into(),
            },
        });
    }
    facts
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NoticeKind {
    Information,
    Submission,
    Protocol,
    Connection,
    Service(FailureKind),
}
struct Notice {
    kind: NoticeKind,
    text: String,
}
impl Notice {
    fn new(kind: NoticeKind, text: impl AsRef<str>) -> Self {
        Self {
            kind,
            text: summary(text.as_ref()),
        }
    }
    fn failure(error: &ServiceError) -> Self {
        let lead = match error.kind {
            FailureKind::InspectionCancelled => "Inspection cancelled. Original unchanged.",
            FailureKind::InspectionLimit => {
                "This original exceeds the current inspection limit. Original retained."
            }
            FailureKind::SourceBusy => {
                "Original is in use. Stop/release its owner, then inspect again."
            }
            FailureKind::SourceUnavailable => "Original could not be inspected.",
            FailureKind::StorageUnavailable => {
                "Storage connection unavailable. Previous observations retained."
            }
            FailureKind::Disconnected => {
                "Storage worker disconnected. Previous observations retained."
            }
            _ => "The read did not complete.",
        };
        Self::new(
            NoticeKind::Service(error.kind),
            if error.kind == FailureKind::InspectionCancelled {
                lead.to_owned()
            } else {
                format!("{lead} {}", error.message)
            },
        )
    }
}
fn summary(text: &str) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(511).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}
fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && id.bytes().any(|b| b != b'0')
}
fn valid_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_path(path: &Path, stage: &str) -> bool {
    path.is_absolute()
        && path.as_os_str().len() <= 4096
        && !path.to_string_lossy().contains('\0')
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        && path.file_name().is_some_and(|v| v == "take.sqlite3")
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|v| v == stage)
}
struct ByteCap {
    used: usize,
    limit: usize,
}
impl Write for ByteCap {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.used) {
            return Err(std::io::Error::other(
                "Saved performance reply exceeds its UI bound",
            ));
        }
        self.used += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OriginalState {
    Recording,
    Interrupted,
    NormalManifest,
    RecoveredManifest,
}
struct Review {
    generation: u64,
    epoch: u64,
    serial: u64,
    current: bool,
    dto: Box<PerformanceSourceReview>,
    lines: Vec<String>,
    evidence: Vec<String>,
}
impl Review {
    fn prepare(
        dto: &PerformanceSourceReview,
        generation: u64,
        epoch: u64,
        serial: u64,
    ) -> Result<Self, String> {
        serde_json::to_writer(
            ByteCap {
                used: 0,
                limit: 65536,
            },
            dto,
        )
        .map_err(|e| e.to_string())?;
        if dto.version != 1
            || !valid_id(&dto.stage_id)
            || !matches!(dto.profile_version, 1 | 2)
            || dto.take_id != format!("performance-{}", dto.stage_id)
            || dto.config.take_id.0 != dto.take_id
            || !valid_path(&dto.journal_path, &dto.stage_id)
            || dto.byte_length > 536_870_912
            || !dto.performance.unknown_tail
            || !valid_sha(&dto.review_sha256)
            || !valid_sha(&dto.performance.descriptor_sha256)
            || !valid_sha(&dto.performance.original_rows_sha256)
            || !valid_sha(&dto.performance.journal_sha256_at_review)
            || dto.manifest_sha256.as_ref().is_some_and(|v| !valid_sha(v))
        {
            return Err("Source reply identity or bounded shape did not match its request.".into());
        }
        dto.config.validate()?;
        let state = match serde_json::to_value(dto.state)
            .map_err(|e| e.to_string())?
            .as_str()
        {
            Some("recording") if dto.manifest_sha256.is_none() && !dto.manifest_interrupted => {
                OriginalState::Recording
            }
            Some("interrupted") if dto.manifest_sha256.is_none() && !dto.manifest_interrupted => {
                OriginalState::Interrupted
            }
            Some("sealed") if dto.manifest_sha256.is_some() && !dto.manifest_interrupted => {
                OriginalState::NormalManifest
            }
            Some("sealed") if dto.manifest_sha256.is_some() && dto.manifest_interrupted => {
                OriginalState::RecoveredManifest
            }
            _ => return Err("Source state and manifest fields contradict one another.".into()),
        };
        let usage = dto.usage;
        if usage.file_bytes > 536_870_912
            || usage.physical_rows > 262_144
            || usage.work_bytes > 1_073_741_824
            || usage.row_visits > 4_194_304
            || usage.chunk_visits > 524_288
            || usage.record_visits > 2_097_152
            || usage.groups > 65_536
            || usage.vm_steps > 32_000_000
        {
            return Err("Source reply usage exceeds the fixed inspection ceilings.".into());
        }
        let p = &dto.performance;
        match (&p.selection, &p.checkpoint, &p.checkpoint_sha256) {
            (None, None, None) => {}
            (Some(selection), Some(cp), Some(hash))
                if selection.chunk_count > 0
                    && selection.chunk_count <= p.received_generic_chunks
                    && valid_sha(&selection.chunk_index_sha256)
                    && valid_sha(hash)
                    && cp.group_sequence > 0
                    && cp.first_callback_index > 0
                    && cp.first_callback_index <= cp.last_callback_index
                    && cp.audio_end > 0
                    && cp.scope_after.is_valid()
                    && cp.prefixes.len() == 6
                    && cp.scope_after.worker_generation
                        == dto.config.info.scope.worker_generation
                    && cp.scope_after.session_id == dto.config.info.scope.session_id
                    && cp.scope_after.panic_generation
                        >= dto.config.info.scope.panic_generation
                    && cp.prefixes.iter().zip(STREAMS).all(|(prefix, name)| {
                        prefix.stream_id == name
                            && serde_json::to_value(&prefix.source_end)
                                .ok()
                                .is_some_and(|value| {
                                    value["unit"].as_str()
                                        == Some(if name == "audio" {
                                            "audio_frames"
                                        } else {
                                            "events"
                                        })
                                })
                    })
                    && !cp.chunks.is_empty()
                    && cp.chunks.len() <= 63
                    && cp.chunks.iter().all(|chunk| {
                        STREAMS.contains(&chunk.stream_id.as_str()) && valid_sha(&chunk.sha256)
                    })
                    && cp
                        .previous_checkpoint_sha256
                        .as_ref()
                        .is_none_or(|v| valid_sha(v)) => {}
            _ => {
                return Err("Source reply has an incomplete or malformed checkpoint shape.".into());
            }
        }
        if let Some(cut) = &p.finish_cut
            && (dto.profile_version != 2
                || !cut.fence.id.is_valid()
                || !cut.fence.scope.is_valid()
                || cut.fence.scope.worker_generation != dto.config.info.scope.worker_generation
                || cut.fence.scope.session_id != dto.config.info.scope.session_id
                || cut.fence.scope.panic_generation < dto.config.info.scope.panic_generation
                || cut.cut_frame == 0
                || cut.callback_index == 0
                || cut.voices_cut > 64
                || cut.release_tails_cut > cut.voices_cut
                || cut.fence.accepted_sequence == Some(0))
        {
            return Err("Source reply has an invalid observed Finish shape.".into());
        }
        let mut lines = vec![match state {
            OriginalState::Recording | OriginalState::Interrupted => {
                "Original retained · no sealed manifest observed.".into()
            }
            OriginalState::NormalManifest => "Existing normal manifest observed.".into(),
            OriginalState::RecoveredManifest => {
                "Recovered manifest observed · final tail unknown.".into()
            }
        }];
        if let Some(cp) = &p.checkpoint {
            lines.push(format!(
                "Verified checkpoint · {} frames · {:.3} s · {} Hz · {} channels",
                cp.audio_end,
                cp.audio_end as f64 / f64::from(dto.config.info.sample_rate),
                dto.config.info.sample_rate,
                dto.config.info.channels
            ));
            lines.push(format!(
                "Group {} · callbacks {}–{} · {} checkpoint members",
                cp.group_sequence,
                cp.first_callback_index,
                cp.last_callback_index,
                cp.chunks.len()
            ));
            for prefix in &cp.prefixes {
                lines.push(format!(
                    "{} · {} {} · {} chunks · {} gap units",
                    prefix.stream_id,
                    prefix.source_end.value(),
                    if prefix.stream_id == "audio" {
                        "audio frames"
                    } else {
                        "event ordinals"
                    },
                    prefix.next_sequence,
                    prefix.gap_units
                ));
            }
        } else {
            lines.push("No complete performance checkpoint was verified.".into());
        }
        if p.unknown_tail {
            lines.push("This checkpoint review keeps the final tail conservative; it does not prove every GUI event was saved.".into());
        }
        if let Some(fault) = &p.capture_fault {
            lines.push(summary(&format!("Observed first fault: {fault:?}")));
        }
        if p.capture_fault_pending {
            lines.push("A capture fault was pending in the observed evidence.".into());
        }
        if let Some(problem) = &p.problem {
            lines.push(summary(&format!("Performance problem: {problem}")));
        }
        for problem in dto.generic_problems.iter().take(16) {
            lines.push(summary(&format!("Source problem: {}", problem.message)));
        }
        if dto.generic_problems.len() > 16 {
            lines.push(format!(
                "{} additional source problems retained.",
                dto.generic_problems.len() - 16
            ));
        }
        if let Some(cut) = &p.finish_cut {
            lines.push(format!(
                "Observed callback Finish · frame {} · callback {} · accepted sequence {:?}",
                cut.cut_frame, cut.callback_index, cut.fence.accepted_sequence
            ));
            lines.push(format!(
                "Finish voices cut {} · release tails cut {}",
                cut.voices_cut, cut.release_tails_cut
            ));
        }
        let mut evidence = vec![
            format!("Journal: {}", dto.journal_path.to_string_lossy()),
            format!(
                "Stored state: {state:?} · {} file bytes · {} verified payload bytes",
                dto.byte_length, dto.verified_payload_bytes
            ),
            format!("Take: {}", dto.take_id),
            format!("Review observation SHA-256: {}", dto.review_sha256),
            format!("Physical journal SHA-256: {}", p.journal_sha256_at_review),
            format!("Original-row SHA-256: {}", p.original_rows_sha256),
            format!("Descriptor SHA-256: {}", p.descriptor_sha256),
        ];
        if let Some(hash) = &dto.manifest_sha256 {
            evidence.push(format!("Existing manifest SHA-256: {hash}"));
        }
        if let Some(hash) = &p.checkpoint_sha256 {
            evidence.push(format!("Checkpoint SHA-256: {hash}"));
        }
        if let Some(cut) = &p.finish_cut {
            evidence.push(format!(
                "Observed Finish {} · {}",
                hex(&cut.fence.id.0),
                scope_text(&cut.fence.scope)
            ));
        }
        for source in &dto.config.sources {
            evidence.push(summary(&format!(
                "GUI source {} · {:?} · generation {} · {}",
                source.binding.handle,
                source.binding.kind,
                source.binding.connection_generation,
                source.endpoint.0
            )));
        }
        evidence.extend([
            format!("Usage · file bytes {}", usage.file_bytes),
            format!("Usage · physical rows {}", usage.physical_rows),
            format!("Usage · work bytes {}", usage.work_bytes),
            format!("Usage · row visits {}", usage.row_visits),
            format!("Usage · chunk visits {}", usage.chunk_visits),
            format!("Usage · record visits {}", usage.record_visits),
            format!("Usage · groups {}", usage.groups),
            format!("Usage · SQLite VM steps {}", usage.vm_steps),
        ]);
        Ok(Self {
            generation,
            epoch,
            serial,
            current: true,
            dto: Box::new(dto.clone()),
            lines,
            evidence,
        })
    }
}
struct PreparedReceipt {
    stage: String,
    dto: PerformanceIntentReceipt,
    lines: Vec<String>,
    evidence: Vec<String>,
}
impl PreparedReceipt {
    fn prepare(receipt: &PerformanceIntentReceipt) -> Result<Self, String> {
        receipt.intent.validate().map_err(|e| e.message)?;
        serde_json::to_writer(
            ByteCap {
                used: 0,
                limit: 16384,
            },
            &receipt.intent,
        )
        .map_err(|e| e.to_string())?;
        let i = &receipt.intent;
        let stage = i.start.directory_name();
        if !valid_id(&stage)
            || !(1..=16).contains(&receipt.revision)
            || !valid_sha(&receipt.payload_sha256)
            || !valid_path(&i.journal_path, &stage)
        {
            return Err("Saved request reply has an invalid identity, revision or path.".into());
        }
        let mut lines = vec![
            format!(
                "Saved Start request · revision {} · {}",
                receipt.revision, stage
            ),
            summary(&format!("Original target session: {}", i.project_id.0)),
            summary(&format!(
                "Requested output: {}",
                i.start.output_device.as_deref().unwrap_or("System default")
            )),
            format!("Saved GUI clock: {}", clock_text(&i.start.gui_clock)),
        ];
        if let Some(finish) = &i.finish {
            lines.push(format!(
                "Saved Finish request: {} · {}",
                hex(&finish.id.0),
                scope_text(&finish.scope)
            ));
        } else {
            lines.push("No Finish request saved.".into());
        }
        if let Some(publication) = &i.attachment {
            lines.push("Saved attachment request.".into());
            lines.push(summary(&format!(
                "Target {} · expected revision {} · operation {}",
                publication.project_id.0, publication.expected_revision, publication.operation_id.0
            )));
            lines.push(summary(&format!(
                "Requested source {} · stream {} · manifest {}",
                publication.clip.source.take_id.0,
                publication.clip.source.stream_id,
                publication.clip.source.manifest_sha256
            )));
        }
        if i.released && i.attachment.is_none() {
            lines.push("Recording choice released.".into());
        }
        let evidence = vec![
            format!("Saved journal: {}", i.journal_path.to_string_lossy()),
            format!("Saved request payload SHA-256: {}", receipt.payload_sha256),
            format!(
                "Saved descriptor SHA-256: {}",
                i.descriptor_sha256.as_deref().unwrap_or("not observed")
            ),
            format!("Released request flag: {}", i.released),
        ];
        Ok(Self {
            stage,
            dto: receipt.clone(),
            lines,
            evidence,
        })
    }
}
struct ScanSnapshot {
    generation: u64,
    serial: u64,
    current: bool,
    dto: PerformanceSourceScan,
    summary: String,
}
struct RequestSnapshot {
    generation: u64,
    serial: u64,
    current: bool,
    receipts: Vec<PreparedReceipt>,
}
struct Row {
    stage: String,
    label: String,
    problem: Option<String>,
}
struct Pending {
    request: SourceRequest,
    id: RequestId,
    cancel_requested: bool,
    cancel_attempted: bool,
}
pub(super) struct PerformanceSourceUi {
    generation: Option<u64>,
    lost_generation: Option<u64>,
    epoch: u64,
    epoch_exhausted: bool,
    serial: u64,
    scan: Option<ScanSnapshot>,
    requests: Option<RequestSnapshot>,
    rows: Vec<Row>,
    selected: Option<String>,
    pending: Option<Pending>,
    review: Option<Review>,
    comparison: Vec<ComparisonFact>,
    scan_notice: Option<Notice>,
    request_notice: Option<Notice>,
    inspection_notice: Option<Notice>,
    details_open: bool,
}
impl PerformanceSourceUi {
    pub(super) fn new() -> Self {
        Self {
            generation: None,
            lost_generation: None,
            epoch: 0,
            epoch_exhausted: false,
            serial: 0,
            scan: None,
            requests: None,
            rows: Vec::new(),
            selected: None,
            pending: None,
            review: None,
            comparison: Vec::new(),
            scan_notice: None,
            request_notice: None,
            inspection_notice: None,
            details_open: true,
        }
    }
    fn advance_epoch(&mut self) {
        if let Some(next) = self.epoch.checked_add(1) {
            self.epoch = next;
        } else {
            self.epoch_exhausted = true;
            self.inspection_notice = Some(Notice::new(
                NoticeKind::Protocol,
                "The review selection counter is exhausted. Further inspection is disabled in this view.",
            ));
        }
    }
    fn next_serial(&mut self) -> Option<u64> {
        let next = self.serial.checked_add(1)?;
        self.serial = next;
        Some(next)
    }
    fn current_generation(&self, generation: u64) -> bool {
        self.generation == Some(generation) && self.lost_generation != Some(generation)
    }
    fn review_current(&self, review: &Review) -> bool {
        review.current
            && self.current_generation(review.generation)
            && review.epoch == self.epoch
            && self.selected.as_deref() == Some(&review.dto.stage_id)
            && !self.epoch_exhausted
    }
    pub(super) fn select(&mut self, stage: &str) -> bool {
        if !valid_id(stage) || !self.rows.iter().any(|row| row.stage == stage) {
            return false;
        }
        if self.selected.as_deref() != Some(stage) {
            self.advance_epoch();
            self.selected = Some(stage.to_owned());
            self.rebuild_comparison();
        }
        true
    }
    pub(super) fn inspection_request(&self) -> Option<SourceRequest> {
        if self.epoch_exhausted
            || self.pending.is_some()
            || self
                .generation
                .is_some_and(|g| self.lost_generation == Some(g))
        {
            return None;
        }
        self.selected
            .as_ref()
            .filter(|id| valid_id(id))
            .map(|stage_id| SourceRequest::Inspect {
                stage_id: stage_id.clone(),
                view_epoch: self.epoch,
            })
    }
    pub(super) fn pending_inspection(&self) -> Option<RequestId> {
        self.pending
            .as_ref()
            .filter(|p| matches!(p.request, SourceRequest::Inspect { .. }))
            .map(|p| p.id)
    }
    pub(super) fn generation_changed(&mut self, generation: u64) {
        if self.generation.is_some_and(|current| generation <= current) {
            return;
        }
        self.generation = Some(generation);
        self.lost_generation = None;
        self.advance_epoch();
        self.pending = None;
        self.scan_notice = None;
        self.request_notice = None;
        self.inspection_notice = None;
        self.rebuild_comparison();
    }
    pub(super) fn worker_lost(&mut self, generation: u64, error: &str) {
        if self.generation != Some(generation) {
            return;
        }
        self.lost_generation = Some(generation);
        self.advance_epoch();
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.id.generation == generation)
        {
            self.pending = None;
        }
        self.scan_notice = None;
        self.request_notice = None;
        self.inspection_notice = Some(Notice::new(
            NoticeKind::Connection,
            format!("Storage worker unavailable. Previous-connection evidence retained. {error}"),
        ));
        self.rebuild_comparison();
    }
    pub(super) fn submitted(&mut self, request: &SourceRequest, id: RequestId) {
        if self.pending.is_some() {
            self.notice(
                request,
                Notice::new(
                    NoticeKind::Protocol,
                    "A source read is already pending; its exact identity was retained.",
                ),
            );
            return;
        }
        if self.generation != Some(id.generation) {
            self.generation_changed(id.generation);
        }
        self.pending = Some(Pending {
            request: request.clone(),
            id,
            cancel_requested: false,
            cancel_attempted: false,
        });
        if matches!(request, SourceRequest::Inspect { .. })
            && let Some(review) = &mut self.review
        {
            review.current = false;
        }
        self.notice(
            request,
            Notice::new(
                NoticeKind::Information,
                match request {
                    SourceRequest::Scan => "Scanning saved recording directories.",
                    SourceRequest::RefreshRequests => "Loading saved requests.",
                    SourceRequest::Inspect { .. } => {
                        "Inspecting saved original. Cancellation is cooperative."
                    }
                },
            ),
        );
    }
    pub(super) fn submission_failed(&mut self, request: &SourceRequest, message: &str) {
        self.invalidate_read(request);
        self.notice(
            request,
            Notice::new(
                NoticeKind::Submission,
                format!("Read not submitted. {message}"),
            ),
        );
    }
    fn invalidate_read(&mut self, request: &SourceRequest) {
        match request {
            SourceRequest::Scan => {
                if let Some(scan) = &mut self.scan {
                    scan.current = false;
                }
            }
            SourceRequest::RefreshRequests => {
                if let Some(requests) = &mut self.requests {
                    requests.current = false;
                }
            }
            SourceRequest::Inspect { .. } => {
                if let Some(review) = &mut self.review {
                    review.current = false;
                }
            }
        }
    }
    fn notice(&mut self, request: &SourceRequest, notice: Notice) {
        *match request {
            SourceRequest::Scan => &mut self.scan_notice,
            SourceRequest::RefreshRequests => &mut self.request_notice,
            SourceRequest::Inspect { .. } => &mut self.inspection_notice,
        } = Some(notice);
    }
    pub(super) fn completed(
        &mut self,
        request: &SourceRequest,
        id: RequestId,
        result: Result<&Reply, &ServiceError>,
    ) {
        if !self
            .pending
            .as_ref()
            .is_some_and(|p| p.id == id && p.request == *request)
        {
            return;
        }
        if let Ok(reply) = result {
            let kind_matches = matches!(
                (request, reply),
                (SourceRequest::Scan, Reply::PerformanceSources(_))
                    | (SourceRequest::RefreshRequests, Reply::PerformanceIntents(_))
                    | (
                        SourceRequest::Inspect { .. },
                        Reply::PerformanceSourceInspected(_)
                    )
            );
            if !kind_matches {
                self.notice(
                    request,
                    Notice::new(
                        NoticeKind::Protocol,
                        "Reply kind did not match this read. Waiting for its matching completion.",
                    ),
                );
                return;
            }
        }
        let cancelled = self.pending.take().is_some_and(|p| p.cancel_requested);
        match result {
            Err(error) => {
                self.invalidate_read(request);
                self.notice(request, Notice::failure(error));
            }
            Ok(Reply::PerformanceSources(scan)) => {
                if let Err(error) = self.install_scan(id.generation, scan) {
                    self.invalidate_read(request);
                    self.notice(request, Notice::new(NoticeKind::Protocol, error));
                } else {
                    self.scan_notice = None;
                }
            }
            Ok(Reply::PerformanceIntents(receipts)) => {
                self.install_requests(id.generation, receipts);
            }
            Ok(Reply::PerformanceSourceInspected(dto)) => {
                let SourceRequest::Inspect {
                    stage_id,
                    view_epoch,
                } = request
                else {
                    return;
                };
                let prepared = if dto.stage_id != *stage_id {
                    Err("Source reply did not match the requested saved recording.".into())
                } else if let Some(serial) = self.next_serial() {
                    Review::prepare(dto, id.generation, *view_epoch, serial)
                } else {
                    Err("The source observation counter is exhausted.".into())
                };
                match prepared {
                    Ok(mut review) => {
                        let previous = !self.current_generation(id.generation)
                            || self.epoch_exhausted
                            || self.epoch != *view_epoch
                            || self.selected.as_deref() != Some(stage_id);
                        review.current = !previous;
                        self.review = Some(review);
                        self.rebuild_rows();
                        self.inspection_notice = Some(Notice::new(
                            NoticeKind::Information,
                            if previous {
                                "Inspection finished for the previous selection."
                            } else if cancelled {
                                "Inspection finished before cancellation took effect."
                            } else {
                                "Inspection complete. Original unchanged."
                            },
                        ));
                        self.rebuild_comparison();
                    }
                    Err(error) => self.notice(request, Notice::new(NoticeKind::Protocol, error)),
                }
            }
            Ok(_) => {}
        }
    }
    pub(super) fn cancel_requested(&mut self, id: RequestId, accepted: bool) {
        if let Some(pending) = &mut self.pending
            && pending.id == id
            && matches!(pending.request, SourceRequest::Inspect { .. })
            && !pending.cancel_attempted
        {
            pending.cancel_attempted = true;
            pending.cancel_requested = accepted;
            self.inspection_notice = Some(Notice::new(
                NoticeKind::Information,
                if accepted {
                    "Cancellation requested · waiting for the inspection worker."
                } else {
                    "This inspection is no longer cancellable. Waiting for its matching completion."
                },
            ));
        }
    }
    pub(super) fn observe_requests(
        &mut self,
        generation: u64,
        receipts: &[PerformanceIntentReceipt],
    ) {
        if self.current_generation(generation) {
            self.install_requests(generation, receipts);
        }
    }
    fn install_requests(&mut self, generation: u64, receipts: &[PerformanceIntentReceipt]) {
        let prepared = || -> Result<Vec<PreparedReceipt>, String> {
            if receipts.len() > 32 {
                return Err("Saved request list exceeds 32 identities.".into());
            }
            let mut seen = std::collections::BTreeSet::new();
            let mut result = Vec::with_capacity(receipts.len());
            for receipt in receipts {
                let receipt = PreparedReceipt::prepare(receipt)?;
                if !seen.insert(receipt.stage.clone()) {
                    return Err("Saved request list repeats an identity.".into());
                }
                result.push(receipt);
            }
            Ok(result)
        };
        match prepared() {
            Ok(receipts) if self.current_generation(generation) => {
                if let Some(serial) = self.next_serial() {
                    self.requests = Some(RequestSnapshot {
                        generation,
                        serial,
                        current: true,
                        receipts,
                    });
                    self.request_notice = None;
                    self.rebuild_rows();
                    self.rebuild_comparison();
                } else {
                    self.invalidate_read(&SourceRequest::RefreshRequests);
                    self.request_notice = Some(Notice::new(
                        NoticeKind::Protocol,
                        "The request observation counter is exhausted.",
                    ));
                }
            }
            Ok(_) => {}
            Err(error) => {
                self.invalidate_read(&SourceRequest::RefreshRequests);
                self.request_notice = Some(Notice::new(NoticeKind::Protocol, error));
            }
        }
    }
    fn install_scan(
        &mut self,
        generation: u64,
        scan: &PerformanceSourceScan,
    ) -> Result<(), String> {
        serde_json::to_writer(
            ByteCap {
                used: 0,
                limit: 65536,
            },
            scan,
        )
        .map_err(|e| e.to_string())?;
        if scan.sources.len() > 128
            || scan.examined_entries > 128
            || scan.skipped_generic > scan.examined_entries
            || scan.skipped_unknown > scan.examined_entries
            || scan
                .sources
                .len()
                .checked_add(scan.skipped_generic)
                .and_then(|n| n.checked_add(scan.skipped_unknown))
                .is_none_or(|n| n > scan.examined_entries)
        {
            return Err("Directory scan exceeds its bounded shape.".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        if scan.sources.iter().any(|s| {
            !valid_id(&s.stage_id)
                || !valid_path(&s.journal_path, &s.stage_id)
                || !seen.insert(s.stage_id.clone())
        }) {
            return Err("Directory scan contains an invalid or repeated identity.".into());
        }
        let serial = self
            .next_serial()
            .ok_or("The scan observation counter is exhausted.")?;
        self.scan = Some(ScanSnapshot {
            generation,
            serial,
            current: self.current_generation(generation),
            dto: scan.clone(),
            summary: format!(
                "Examined {} entries · skipped {} generic / {} unknown{}",
                scan.examined_entries,
                scan.skipped_generic,
                scan.skipped_unknown,
                if scan.truncated {
                    " · Some sources were not listed."
                } else {
                    ""
                }
            ),
        });
        self.rebuild_rows();
        Ok(())
    }
    fn rebuild_rows(&mut self) {
        let mut rows = BTreeMap::new();
        if let Some(scan) = &self.scan {
            for source in &scan.dto.sources {
                rows.insert(
                    source.stage_id.clone(),
                    (
                        true,
                        source.problem.as_deref().map(|problem| {
                            summary(&format!("Directory scan observation: {problem}"))
                        }),
                    ),
                );
            }
        }
        if let Some(requests) = &self.requests {
            for receipt in &requests.receipts {
                rows.entry(receipt.stage.clone()).or_insert((false, None));
            }
        }
        self.rows = rows
            .into_iter()
            .take(MAX_ROWS)
            .map(|(stage, (discovered, problem))| {
                let inspected = self
                    .review
                    .as_ref()
                    .is_some_and(|review| review.dto.stage_id == stage);
                Row {
                    label: format!(
                        "{stage} · {} · {}",
                        if discovered {
                            "Directory candidate"
                        } else {
                            "Saved request"
                        },
                        if inspected {
                            "inspection available"
                        } else {
                            "source not inspected in this view"
                        }
                    ),
                    stage,
                    problem,
                }
            })
            .collect();
    }
    fn rebuild_comparison(&mut self) {
        self.comparison.clear();
        if let (Some(selected), Some(review), Some(requests)) =
            (&self.selected, &self.review, &self.requests)
            && review.dto.stage_id == *selected
            && let Some(receipt) = requests.receipts.iter().find(|r| r.stage == *selected)
        {
            self.comparison = compare_request(&receipt.dto, &review.dto);
        }
    }
    #[cfg(test)]
    pub(super) fn visible_review(&self) -> Option<&PerformanceSourceReview> {
        self.review
            .as_ref()
            .filter(|r| self.review_current(r))
            .map(|r| r.dto.as_ref())
    }
    #[cfg(test)]
    pub(super) fn last_review(&self) -> Option<&PerformanceSourceReview> {
        self.review.as_ref().map(|r| r.dto.as_ref())
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        availability: &SourceAvailability,
    ) -> Option<SourceAction> {
        let mut action = None;
        let ready = availability.can_submit
            && self.pending.is_none()
            && availability.generation.is_some()
            && self.generation.is_none_or(|g| {
                Some(g) == availability.generation && self.lost_generation != Some(g)
            });
        ui.strong("Saved performances");
        ui.small("This review checks the saved original. Recovery and adding it to a session are separate later steps.");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new("Scan saved recordings"))
                .clicked()
            {
                action = Some(SourceAction::Submit(SourceRequest::Scan));
            }
            if ui
                .add_enabled(ready, egui::Button::new("Refresh saved requests"))
                .clicked()
            {
                action = Some(SourceAction::Submit(SourceRequest::RefreshRequests));
            }
        });
        if !ready && let Some(reason) = &availability.unavailable_reason {
            ui.small(summary(reason));
        }
        if let Some(pending) = &self.pending {
            match &pending.request {
                SourceRequest::Inspect { stage_id, .. } => {
                    ui.label(format!("Inspecting saved original {stage_id}"));
                    if pending.cancel_requested {
                        ui.label("Cancellation requested · waiting for the inspection worker.");
                    }
                    if ui
                        .add_enabled(
                            !pending.cancel_attempted,
                            egui::Button::new("Cancel inspection"),
                        )
                        .clicked()
                    {
                        action = Some(SourceAction::CancelInspection(pending.id));
                    }
                }
                SourceRequest::Scan => {
                    ui.label("Scanning saved recordings…");
                }
                SourceRequest::RefreshRequests => {
                    ui.label("Loading saved requests…");
                }
            }
        }
        for notice in [
            &self.scan_notice,
            &self.request_notice,
            &self.inspection_notice,
        ]
        .into_iter()
        .flatten()
        {
            if matches!(
                notice.kind,
                NoticeKind::Protocol | NoticeKind::Service(FailureKind::StorageUnavailable)
            ) {
                ui.colored_label(ui.visuals().error_fg_color, &notice.text);
            } else {
                ui.label(&notice.text);
            }
        }
        if let Some(scan) = &self.scan {
            if !scan.current || !self.current_generation(scan.generation) {
                ui.small("Previous directory scan.");
            }
            ui.small(format!(
                "Directory observation {} · generation {}",
                scan.serial, scan.generation
            ));
            ui.add(egui::Label::new(&scan.summary).wrap());
        } else {
            ui.small("Saved recording directories have not been scanned.");
        }
        let mut choose = None;
        egui::ScrollArea::vertical()
            .id_salt("performance-source-rows")
            .max_height(150.0)
            .show_rows(ui, 24.0, self.rows.len(), |ui, range| {
                for index in range {
                    let row = &self.rows[index];
                    ui.push_id(&row.stage, |ui| {
                        if ui
                            .add_sized(
                                [ui.available_width(), 22.0],
                                egui::Button::selectable(
                                    self.selected.as_deref() == Some(&row.stage),
                                    &row.label,
                                )
                                .truncate(),
                            )
                            .on_hover_text(&row.stage)
                            .clicked()
                        {
                            choose = Some(row.stage.clone());
                        }
                    });
                }
            });
        if let Some(stage) = choose {
            self.select(&stage);
        }
        let selected = self.selected.is_some();
        if ui
            .add_enabled(
                ready && selected && !self.epoch_exhausted,
                egui::Button::new("Inspect selected recording"),
            )
            .clicked()
            && let Some(request) = self.inspection_request()
        {
            action = Some(SourceAction::Submit(request));
        }
        let was_open = self.details_open;
        ui.checkbox(&mut self.details_open, "Show recording details");
        if was_open && !self.details_open {
            self.advance_epoch();
        }
        if self.details_open {
            self.details(ui);
        }
        action
    }
    fn details(&self, ui: &mut egui::Ui) {
        let Some(selected) = &self.selected else {
            ui.small("Choose a saved recording to review.");
            return;
        };
        ui.add(egui::Label::new(format!("Selected original: {selected}")).wrap());
        if let Some(problem) = self
            .rows
            .iter()
            .find(|row| row.stage == *selected)
            .and_then(|row| row.problem.as_ref())
        {
            ui.add(egui::Label::new(problem).wrap());
        }
        ui.strong("Observed original");
        let associated = self.review.as_ref().filter(|r| r.dto.stage_id == *selected);
        if let Some(review) = associated {
            if !self.review_current(review) {
                ui.strong("Previous inspection");
            }
            ui.small(format!(
                "Inspection observation {} · generation {}",
                review.serial, review.generation
            ));
            for line in &review.lines {
                ui.add(egui::Label::new(line).wrap());
            }
            ui.collapsing("Source evidence", |ui| {
                for line in &review.evidence {
                    ui.add(egui::Label::new(line).wrap());
                }
            });
        } else {
            ui.label("This selected original has not been inspected in this view.");
            if let Some(review) = &self.review {
                ui.small(format!(
                    "Previous inspection retained for {}.",
                    review.dto.stage_id
                ));
            }
        }
        ui.strong("Saved requests");
        let checked = self
            .requests
            .as_ref()
            .is_some_and(|r| r.current && self.current_generation(r.generation));
        if !checked {
            ui.label("Saved requests have not been checked for this connection.");
        }
        if let Some(requests) = &self.requests {
            if !checked {
                ui.small("Previous saved requests.");
            }
            ui.small(format!(
                "Request observation {} · generation {}",
                requests.serial, requests.generation
            ));
            if let Some(saved) = requests.receipts.iter().find(|r| r.stage == *selected) {
                for line in &saved.lines {
                    ui.add(egui::Label::new(line).wrap());
                }
                if checked && saved.dto.intent.released && saved.dto.intent.attachment.is_some() {
                    ui.label("Historical attachment verified when requests were loaded.");
                    ui.small("Current session placement and owned-file availability were not checked here.");
                }
                if associated.is_some() {
                    if !checked || associated.is_some_and(|r| !self.review_current(r)) {
                        ui.small("Comparison with previous observations.");
                    }
                    for fact in &self.comparison {
                        if fact.state == FactState::Conflict {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                format!("Conflict · {}", fact.text),
                            );
                        } else {
                            ui.add(egui::Label::new(&fact.text).wrap());
                        }
                    }
                }
                ui.collapsing("Saved request evidence", |ui| {
                    for line in &saved.evidence {
                        ui.add(egui::Label::new(line).wrap());
                    }
                });
            } else if checked {
                ui.label("No saved Start request was listed for this original.");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_session::PerformanceSource;
    use serde_json::{Value, json};

    const A: &str = "11111111111111111111111111111111";
    const B: &str = "22222222222222222222222222222222";

    fn id(sequence: u64) -> RequestId {
        RequestId {
            generation: 1,
            sequence,
        }
    }
    fn path(stage: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(if cfg!(windows) {
            r"C:\synthetic\captures"
        } else {
            "/synthetic/captures"
        })
        .join(stage)
        .join("take.sqlite3")
    }
    fn scan(stages: &[&str]) -> PerformanceSourceScan {
        PerformanceSourceScan {
            sources: stages
                .iter()
                .map(|stage| PerformanceSource {
                    stage_id: (*stage).into(),
                    journal_path: path(stage),
                    problem: None,
                })
                .collect(),
            examined_entries: stages.len(),
            ..Default::default()
        }
    }
    fn seeded() -> PerformanceSourceUi {
        let mut model = PerformanceSourceUi::new();
        model.generation_changed(1);
        let request = SourceRequest::Scan;
        model.submitted(&request, id(1));
        model.completed(
            &request,
            id(1),
            Ok(&Reply::PerformanceSources(scan(&[A, B]))),
        );
        assert!(model.select(A));
        model
    }
    fn review(stage: &str, checkpoint: bool) -> PerformanceSourceReview {
        let session: Vec<u8> = (0..16)
            .map(|i| u8::from_str_radix(&stage[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        let scope =
            json!({"worker_generation":vec![33u8;16],"session_id":session,"panic_generation":1});
        let mut value = json!({
            "version":1,"stage_id":stage,"journal_path":path(stage),"take_id":format!("performance-{stage}"),
            "profile_version":2,"state":"sealed","byte_length":8192,
            "manifest_sha256":"f".repeat(64),"manifest_interrupted":false,
            "verified_payload_bytes":4096,"generic_problems":[],
            "config":{
                "take_id":format!("performance-{stage}"),
                "info":{"scope":scope,"sample_rate":48000,"channels":2},
                "gui_clock":{"domain":"synthetic-gui","epoch":"synthetic-clock",
                    "ticks_per_second_numerator":1000000000u64,"ticks_per_second_denominator":1},
                "output_epoch":"synthetic-output",
                "sources":[
                    {"binding":{"kind":"computer_keys","handle":1,"connection_generation":1},"endpoint":"synthetic-source-1"},
                    {"binding":{"kind":"pointer_keys","handle":2,"connection_generation":1},"endpoint":"synthetic-source-2"},
                    {"binding":{"kind":"pads","handle":3,"connection_generation":1},"endpoint":"synthetic-source-3"},
                    {"binding":{"kind":"accessible_pulse","handle":4,"connection_generation":1},"endpoint":"synthetic-source-4"}
                ]
            },
            "performance":{"descriptor_sha256":"a".repeat(64),"original_rows_sha256":"b".repeat(64),
                "journal_sha256_at_review":"c".repeat(64),"selection":null,"checkpoint_sha256":null,"checkpoint":null,
                "received_generic_chunks":0,"unknown_tail":true,"problem":null,"capture_fault":null,
                "capture_fault_pending":false,"finish_cut":null},
            "usage":{"file_bytes":8192,"physical_rows":0,"work_bytes":0,"row_visits":0,
                "chunk_visits":0,"record_visits":0,"groups":0,"vm_steps":0},
            "review_sha256":"d".repeat(64)
        });
        if checkpoint {
            let prefixes: Vec<Value> = STREAMS
                .iter()
                .map(|stream| {
                    json!({
                        "stream_id":stream,"next_sequence":1,"source_end":{
                            "unit":if *stream=="audio" {"audio_frames"} else {"events"},
                            "end":if *stream=="audio" {1024} else {1}},
                        "recorded_units":if *stream=="audio" {1024} else {1},
                        "gap_units":0,"gap_chunks":0
                    })
                })
                .collect();
            value["performance"]["received_generic_chunks"] = json!(2);
            value["performance"]["selection"] =
                json!({"chunk_count":2,"chunk_index_sha256":"9".repeat(64)});
            value["performance"]["checkpoint_sha256"] = json!("8".repeat(64));
            value["performance"]["checkpoint"] = json!({
                "group_sequence":1,"previous_checkpoint_sha256":null,"first_callback_index":1,
                "last_callback_index":2,"audio_end":1024,"scope_after":scope,
                "chunks":[{"stream_id":"audio","sequence":0,"sha256":"7".repeat(64)}],
                "prefixes":prefixes
            });
            value["performance"]["finish_cut"] = json!({
                "fence":{"id":vec![99u8;16],"scope":scope,"accepted_sequence":3},
                "callback_index":2,"cut_frame":1024,"voices_cut":0,"release_tails_cut":0,
                "policy":"cut_at_complete_callback_end"
            });
        }
        serde_json::from_value(value).unwrap()
    }
    fn receipt(
        source: &PerformanceSourceReview,
        released: bool,
        attachment: bool,
    ) -> PerformanceIntentReceipt {
        let finish = source.performance.finish_cut.as_ref().map(|cut| {
            json!({
                "id":cut.fence.id,"scope":cut.fence.scope
            })
        });
        let publication = attachment.then(|| {
            json!({
                "operation_id":"44444444-4444-4444-8444-444444444444",
                "project_id":"55555555-5555-4555-8555-555555555555","expected_revision":1,
                "track_id":"66666666-6666-4666-8666-666666666666","track_name":"Saved original",
                "clip":{"id":"77777777-7777-4777-8777-777777777777",
                    "source":{"takeId":source.take_id,"manifestSha256":source.manifest_sha256,
                              "streamId":"audio"},
                    "range":{"unit":"audio_frames","start":0,"count":1024},
                    "position":{"frame":0,"sampleRate":48000}}
            })
        });
        serde_json::from_value(json!({
            "revision":if released {4} else {2},"payload_sha256":"e".repeat(64),
            "intent":{"version":1,"start":{"id":source.config.info.scope.session_id,
                "output_device":null,"gui_clock":source.config.gui_clock},
                "project_id":"55555555-5555-4555-8555-555555555555",
                "journal_path":source.journal_path,"descriptor_sha256":source.performance.descriptor_sha256,
                "finish":finish,"attachment":publication,"released":released}
        })).unwrap()
    }
    fn inspected(model: &mut PerformanceSourceUi, sequence: u64) {
        let request = model.inspection_request().unwrap();
        let SourceRequest::Inspect { stage_id, .. } = &request else {
            unreachable!()
        };
        let reply = Reply::PerformanceSourceInspected(Box::new(review(stage_id, true)));
        model.submitted(&request, id(sequence));
        model.completed(&request, id(sequence), Ok(&reply));
        assert!(model.visible_review().is_some());
    }
    fn frame(
        context: &egui::Context,
        model: &mut PerformanceSourceUi,
        width: f32,
    ) -> (egui::FullOutput, Option<SourceAction>, f32) {
        frame_at(context, model, width, 0.0)
    }
    fn frame_at(
        context: &egui::Context,
        model: &mut PerformanceSourceUi,
        width: f32,
        scroll_offset: f32,
    ) -> (egui::FullOutput, Option<SourceAction>, f32) {
        let mut action = None;
        let mut content_width = 0.0;
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 1000.0),
            )),
            ..Default::default()
        };
        // Supply the backend's device scale directly. A pending zoom change on
        // the first pass would replace this screen rect with egui's prior
        // default viewport scaled from 10,000 points.
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(1.5);
        let mut output = context.run_ui(input, |ui| {
            // Match the parent's vertically scrolling review panel, with
            // the viewport width serving as the panel's inner bound.
            egui::ScrollArea::vertical()
                .vertical_scroll_offset(scroll_offset)
                .show(ui, |ui| {
                    assert!(ui.available_width() <= width + 1.0);
                    for name in ["Source evidence", "Saved request evidence"] {
                        // Pinned egui passes IdSalt values through Id::with
                        // both for its vertical child and the header salt.
                        let header_id = ui
                            .scope_id()
                            .with(egui::IdSalt::new("child"))
                            .with(egui::IdSalt::new(name));
                        let mut state =
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                header_id,
                                true,
                            );
                        state.set_open(true);
                        state.store(ui.ctx());
                    }
                    action = model.ui(
                        ui,
                        &SourceAvailability {
                            can_submit: true,
                            generation: Some(1),
                            unavailable_reason: None,
                        },
                    );
                    content_width = ui.min_rect().width();
                });
        });
        output.textures_delta.clear();
        (output, action, content_width)
    }

    #[test]
    fn construction_selection_refresh_and_reconnect_emit_no_automatic_request() {
        let context = egui::Context::default();
        let mut model = PerformanceSourceUi::new();
        assert!(frame(&context, &mut model, 350.0).1.is_none());
        assert!(!model.select(A));
        model = seeded();
        assert!(!model.select(&"0".repeat(32)));
        assert!(!model.select("not-a-stage"));
        assert!(!model.select("33333333333333333333333333333333"));
        assert!(frame(&context, &mut model, 350.0).1.is_none());
        model.observe_requests(1, &[receipt(&review(A, true), false, false)]);
        assert!(frame(&context, &mut model, 350.0).1.is_none());
        model.worker_lost(1, "synthetic disconnection");
        model.generation_changed(1);
        assert!(
            model.inspection_request().is_none(),
            "same lost generation must not revive"
        );
        assert!(!model.current_generation(1));
        assert!(frame(&context, &mut model, 350.0).1.is_none());
        model.generation_changed(2);
        assert!(model.current_generation(2));
        assert!(model.pending.is_none());
        assert!(frame(&context, &mut model, 350.0).1.is_none());
        assert!(matches!(
            SourceRequest::Scan.command(),
            Command::ScanPerformanceSources
        ));
        assert!(matches!(
            SourceRequest::RefreshRequests.command(),
            Command::ListPerformanceIntents
        ));
        assert!(matches!(
            model.inspection_request().unwrap().command(),
            Command::InspectPerformanceSource { .. }
        ));
    }

    #[test]
    fn exact_ids_epochs_and_reply_kinds_preserve_worker_credit_and_old_association() {
        let mut model = seeded();
        let request = model.inspection_request().unwrap();
        model.submitted(&request, id(2));
        model.completed(
            &request,
            id(99),
            Ok(&Reply::PerformanceSourceInspected(Box::new(review(
                A, true,
            )))),
        );
        assert_eq!(model.pending_inspection(), Some(id(2)));
        let mut wrong_request = request.clone();
        if let SourceRequest::Inspect { view_epoch, .. } = &mut wrong_request {
            *view_epoch += 1;
        }
        model.completed(
            &wrong_request,
            id(2),
            Ok(&Reply::PerformanceSourceInspected(Box::new(review(
                A, true,
            )))),
        );
        assert_eq!(model.pending_inspection(), Some(id(2)));
        model.completed(
            &request,
            id(2),
            Ok(&Reply::PerformanceSources(scan(&[A, B]))),
        );
        assert_eq!(
            model.pending_inspection(),
            Some(id(2)),
            "wrong-kind reply must not free credit"
        );
        assert!(model.select(B));
        assert!(model.select(A));
        model.completed(
            &request,
            id(2),
            Ok(&Reply::PerformanceSourceInspected(Box::new(review(
                A, true,
            )))),
        );
        assert!(model.pending_inspection().is_none());
        assert!(model.visible_review().is_none());
        assert_eq!(model.last_review().unwrap().stage_id, A);
        assert!(
            model
                .inspection_notice
                .as_ref()
                .unwrap()
                .text
                .contains("previous selection")
        );
        inspected(&mut model, 3);
        assert!(
            model
                .rows
                .iter()
                .find(|row| row.stage == A)
                .unwrap()
                .label
                .contains("inspection available")
        );
        assert!(model.select(B));
        assert!(model.visible_review().is_none());
        assert_eq!(model.last_review().unwrap().stage_id, A);
    }

    #[test]
    fn cancellation_is_exact_idempotent_and_success_is_not_relabelled() {
        let mut model = seeded();
        inspected(&mut model, 2);
        for (sequence, accepted) in [(3, true), (4, false)] {
            let request = model.inspection_request().unwrap();
            model.submitted(&request, id(sequence));
            model.cancel_requested(id(999), true);
            assert!(!model.pending.as_ref().unwrap().cancel_attempted);
            model.cancel_requested(id(sequence), accepted);
            model.cancel_requested(id(sequence), !accepted);
            assert_eq!(model.pending_inspection(), Some(id(sequence)));
            assert_eq!(model.pending.as_ref().unwrap().cancel_requested, accepted);
            assert!(model.inspection_request().is_none());
            model.completed(
                &request,
                id(sequence),
                Ok(&Reply::PerformanceSourceInspected(Box::new(review(
                    A, true,
                )))),
            );
            assert!(model.visible_review().is_some());
            assert_eq!(
                model.inspection_notice.as_ref().unwrap().text,
                if accepted {
                    "Inspection finished before cancellation took effect."
                } else {
                    "Inspection complete. Original unchanged."
                }
            );
        }
        let request = model.inspection_request().unwrap();
        model.submitted(&request, id(5));
        model.cancel_requested(id(5), true);
        let error = ServiceError {
            kind: FailureKind::InspectionCancelled,
            message: "interrupted".into(),
            outcome_unknown: false,
        };
        model.completed(&request, id(5), Err(&error));
        assert!(model.pending.is_none() && model.visible_review().is_none());
        assert!(model.last_review().is_some());
        assert_eq!(
            model.inspection_notice.as_ref().unwrap().text,
            "Inspection cancelled. Original unchanged."
        );
    }

    #[test]
    fn source_local_errors_and_storage_errors_keep_typed_notices_and_prior_data() {
        for kind in [
            FailureKind::SourceBusy,
            FailureKind::SourceUnavailable,
            FailureKind::InspectionLimit,
            FailureKind::StorageUnavailable,
        ] {
            let mut model = seeded();
            inspected(&mut model, 2);
            let original = model.last_review().unwrap().clone();
            let request = model.inspection_request().unwrap();
            model.submitted(&request, id(3));
            let error = ServiceError {
                kind,
                message: "bounded synthetic refusal".into(),
                outcome_unknown: false,
            };
            model.completed(&request, id(3), Err(&error));
            assert!(model.pending.is_none());
            assert_eq!(model.last_review(), Some(&original));
            assert!(model.visible_review().is_none());
            assert_eq!(
                model.inspection_notice.as_ref().unwrap().kind,
                NoticeKind::Service(kind)
            );
        }
        let mut model = seeded();
        model.observe_requests(1, &[receipt(&review(A, true), false, false)]);
        let request = SourceRequest::RefreshRequests;
        model.submitted(&request, id(4));
        model.completed(
            &request,
            id(4),
            Err(&ServiceError {
                kind: FailureKind::SourceUnavailable,
                message: "synthetic list failure".into(),
                outcome_unknown: false,
            }),
        );
        assert!(!model.requests.as_ref().unwrap().current);
        assert_eq!(model.requests.as_ref().unwrap().receipts.len(), 1);
    }

    #[test]
    fn normal_manifest_keeps_conservative_tail_and_malformed_shapes_install_nothing() {
        let normal = review(A, true);
        let prepared = Review::prepare(&normal, 1, 1, 1).unwrap();
        assert!(
            prepared
                .lines
                .iter()
                .any(|line| line == "Existing normal manifest observed.")
        );
        assert!(
            prepared
                .lines
                .iter()
                .any(|line| line.contains("keeps the final tail conservative"))
        );
        assert!(prepared.dto.performance.unknown_tail);
        assert!(
            prepared
                .lines
                .iter()
                .any(|line| line.contains("Verified checkpoint"))
        );
        let mut recovered = serde_json::to_value(&normal).unwrap();
        recovered["manifest_interrupted"] = json!(true);
        assert!(
            Review::prepare(&serde_json::from_value(recovered).unwrap(), 1, 1, 1)
                .unwrap()
                .lines
                .iter()
                .any(|line| line.contains("Recovered manifest observed"))
        );
        for field in ["tail", "version", "stage", "units", "usage", "manifest"] {
            let mut value = serde_json::to_value(&normal).unwrap();
            match field {
                "tail" => value["performance"]["unknown_tail"] = json!(false),
                "version" => value["version"] = json!(2),
                "stage" => value["config"]["take_id"] = json!("other-take"),
                "units" => {
                    value["performance"]["checkpoint"]["prefixes"][0]["source_end"]["unit"] =
                        json!("events")
                }
                "usage" => value["usage"]["work_bytes"] = json!(1_073_741_825u64),
                "manifest" => value["manifest_sha256"] = Value::Null,
                _ => unreachable!(),
            }
            assert!(
                Review::prepare(&serde_json::from_value(value).unwrap(), 1, 1, 1).is_err(),
                "{field}"
            );
        }
        let mut model = seeded();
        let request = model.inspection_request().unwrap();
        model.submitted(&request, id(2));
        model.completed(
            &request,
            id(2),
            Ok(&Reply::PerformanceSourceInspected(Box::new(review(
                B, true,
            )))),
        );
        assert!(model.pending.is_none() && model.last_review().is_none());
        assert_eq!(
            model.inspection_notice.as_ref().unwrap().kind,
            NoticeKind::Protocol
        );
    }

    #[test]
    fn full_request_conflicts_and_historical_publication_are_separate_facts() {
        let source = review(A, true);
        let saved = receipt(&source, true, true);
        assert!(
            compare_request(&saved, &source)
                .iter()
                .all(|f| f.state != FactState::Conflict)
        );
        let mut changed = saved.clone();
        changed.intent.journal_path = path(B);
        changed.intent.start.gui_clock.epoch.push_str("-changed");
        changed.intent.descriptor_sha256 = Some("0".repeat(64));
        changed
            .intent
            .finish
            .as_mut()
            .unwrap()
            .scope
            .panic_generation += 1;
        changed
            .intent
            .attachment
            .as_mut()
            .unwrap()
            .clip
            .source
            .manifest_sha256 = "1".repeat(64);
        let facts = compare_request(&changed, &source);
        for reason in [
            ComparisonReason::JournalPath,
            ComparisonReason::GuiClock,
            ComparisonReason::Descriptor,
            ComparisonReason::Finish,
            ComparisonReason::PublicationManifest,
        ] {
            assert!(
                facts
                    .iter()
                    .any(|fact| fact.reason == reason && fact.state == FactState::Conflict)
            );
        }
        let mut model = seeded();
        model.observe_requests(1, &[saved]);
        assert!(model.requests.as_ref().unwrap().current);
        assert!(
            model.requests.as_ref().unwrap().receipts[0]
                .dto
                .intent
                .released
        );
        model.worker_lost(1, "synthetic loss");
        model.generation_changed(1);
        assert!(!model.current_generation(model.requests.as_ref().unwrap().generation));
        model.observe_requests(1, &[]);
        assert_eq!(
            model.requests.as_ref().unwrap().receipts.len(),
            1,
            "stale passive list ignored"
        );
    }

    #[test]
    fn bounded_union_request_only_selection_and_scan_diagnostics_survive_refresh() {
        let mut model = PerformanceSourceUi::new();
        model.generation_changed(1);
        let ids: Vec<String> = (1..=128).map(|n| format!("{n:032x}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut directory = scan(&refs);
        directory.truncated = true;
        directory.sources[0].problem = Some("x".repeat(1000));
        model.install_scan(1, &directory).unwrap();
        let requests: Vec<PerformanceIntentReceipt> = (1001..=1032)
            .map(|n| receipt(&review(&format!("{n:032x}"), false), false, false))
            .collect();
        model.observe_requests(1, &requests);
        assert_eq!(model.rows.len(), 160);
        assert!(model.rows[0].problem.as_ref().unwrap().chars().count() <= 512);
        let requested = requests[0].intent.start.directory_name();
        assert!(model.select(&requested));
        assert!(
            model
                .rows
                .iter()
                .find(|row| row.stage == requested)
                .unwrap()
                .label
                .starts_with(&format!("{requested} · Saved request"))
        );
        assert!(model.inspection_request().is_some());
        let mut oversize = requests.clone();
        oversize.push(requests[0].clone());
        model.observe_requests(1, &oversize);
        assert_eq!(model.rows.len(), 160);
        assert!(!model.requests.as_ref().unwrap().current);
        let before = model.selected.clone();
        model
            .install_scan(1, &PerformanceSourceScan::default())
            .unwrap();
        assert_eq!(model.selected, before);
        assert!(
            model.inspection_request().is_some(),
            "known selected ID stays explicit after an incomplete scan"
        );
        model.epoch = u64::MAX;
        assert!(model.select(&requests[1].intent.start.directory_name()));
        assert!(model.epoch_exhausted && model.inspection_request().is_none());
    }

    #[test]
    fn headless_light_dark_narrow_layout_virtualizes_rows_and_keeps_cancel_visible() {
        for dark in [false, true] {
            for width in [240.0, 480.0] {
                let context = egui::Context::default();
                context.enable_accesskit();
                context.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                context.all_styles_mut(|style| style.animation_time = 0.0);
                let mut model = PerformanceSourceUi::new();
                model.generation_changed(1);
                let ids: Vec<String> = (1..=128).map(|n| format!("{n:032x}")).collect();
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                model.install_scan(1, &scan(&refs)).unwrap();
                assert!(model.select(&ids[0]));
                let request = model.inspection_request().unwrap();
                model.submitted(&request, id(2));
                model.details_open = false;
                let (output, action, content_width) = frame(&context, &mut model, width);
                assert!(action.is_none());
                assert!(
                    content_width <= width + 1.0,
                    "content overflow at {width}: {content_width}"
                );
                let tree = output.platform_output.accesskit_update.unwrap();
                assert!(
                    tree.nodes
                        .iter()
                        .any(|(_, node)| node.label() == Some("Cancel inspection"))
                );
                let rows = tree
                    .nodes
                    .iter()
                    .filter(|(_, node)| {
                        node.label()
                            .is_some_and(|label| label.contains("Directory candidate"))
                    })
                    .count();
                assert!(rows > 0 && rows < 32, "virtualized row count {rows}");
                assert_eq!(model.pending_inspection(), Some(id(2)));

                let mut completed = review(&ids[0], true);
                completed.journal_path = std::path::PathBuf::from(if cfg!(windows) {
                    r"C:\synthetic"
                } else {
                    "/synthetic"
                })
                .join("long-directory-segment".repeat(100))
                .join(&ids[0])
                .join("take.sqlite3");
                let expected_path = format!("Journal: {}", completed.journal_path.display());
                let saved = receipt(&completed, true, true);
                model.completed(
                    &request,
                    id(2),
                    Ok(&Reply::PerformanceSourceInspected(Box::new(completed))),
                );
                model.observe_requests(1, &[saved]);
                model.details_open = true;
                assert!(
                    model.visible_review().is_some(),
                    "completed review was not installed at {width}, dark={dark}: notice={:?}",
                    model
                        .inspection_notice
                        .as_ref()
                        .map(|notice| (&notice.kind, &notice.text))
                );
                let (output, action, content_width) = frame(&context, &mut model, width);
                assert!(action.is_none());
                assert!(
                    content_width <= width + 1.0,
                    "completed detail overflow at {width}: {content_width}"
                );
                let tree = output.platform_output.accesskit_update.unwrap();
                assert!(
                    tree.nodes.iter().any(|(_, node)| {
                        node.label() == Some("Source evidence") && node.is_expanded() == Some(true)
                    }),
                    "source evidence header must be expanded at {width}, dark={dark}"
                );
                assert!(
                    tree.nodes
                        .iter()
                        .any(|(_, node)| node.value() == Some("Existing normal manifest observed.")),
                    "normal detail absent at {width}, dark={dark}: visible={}, notice={:?}, text={:?}",
                    model.visible_review().is_some(),
                    model.inspection_notice.as_ref().map(|notice| (&notice.kind, &notice.text)),
                    tree.nodes.iter().filter_map(|(_, node)| node.label().or_else(|| node.value()).map(|text| (text, node.bounds()))).collect::<Vec<_>>()
                );
                // Evidence follows the bounded semantic summary. Bring that
                // section into the viewport rather than expecting AccessKit
                // to publish static text wholly outside its clip rect.
                let (output, action, content_width) = frame_at(&context, &mut model, width, 1000.0);
                assert!(action.is_none());
                assert!(
                    content_width <= width + 1.0,
                    "expanded detail overflow at {width}: {content_width}"
                );
                let tree = output.platform_output.accesskit_update.unwrap();
                assert!(
                    tree.nodes
                        .iter()
                        .any(|(_, node)| node.value() == Some(expected_path.as_str())),
                    "expanded evidence must render the full long path at {width}, dark={dark}: text={:?}",
                    tree.nodes
                        .iter()
                        .filter_map(|(_, node)| node
                            .label()
                            .or_else(|| node.value())
                            .map(|text| (text, node.bounds())))
                        .collect::<Vec<_>>()
                );
                assert!(model.pending_inspection().is_none());
            }
        }
    }
}
