#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(test)]
mod arrangement_integration_review;
mod arrangement_math;
mod arrangement_types;
mod arrangement_ui;
mod audio_ui;
#[cfg(test)]
mod clip_editor_integration_review;
mod clip_ui;
mod evidence_integration;
#[cfg(test)]
mod evidence_integration_review;
mod evidence_ui;
mod live_ui;
#[cfg(test)]
mod note_editor_review;
mod note_ui;
#[cfg(test)]
mod performance_source_integration_review;
mod performance_source_ui;
mod performance_ui;
mod recovery_ui;
mod source_recovery_integration;
#[cfg(test)]
mod source_recovery_integration_review;
mod source_recovery_ui;
mod waveform_ui;

use arrangement_types::PlacementValue;
use audio_ui::AudioUi;
use clip_ui::{ClipAction, ClipContext, ClipEditor};
use eframe::egui;
use evidence_ui::{EvidenceUi, EvidenceUiRequest};
use note_ui::{NoteAction, NoteContext, NoteEditor, NoteSelection};
use omniusong_core::ProjectDocument;
use omniusong_core::contracts::{
    EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId,
};
use omniusong_core::native::RecordingPayload;
use omniusong_session::{
    ArchiveInspection, Command, Completion, DocumentView, FailureKind, LegacyProject, Library,
    Reply, RequestId, RestoreCopyIntent, SessionService,
};
use performance_source_ui::{PerformanceSourceUi, SourceAction, SourceAvailability, SourceRequest};
use recovery_ui::{RecoveryAction, RecoveryUi};
use serde_json::Value;
use source_recovery_ui::{SourceRecoveryRequest, SourceRecoveryUi};
use std::{
    error::Error,
    path::{Path, PathBuf},
    time::Duration,
};

type AppResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone)]
struct Paths {
    native: PathBuf,
    legacy: PathBuf,
}

impl Paths {
    fn parse(args: impl IntoIterator<Item = String>) -> AppResult<Option<Self>> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let mut paths = Self {
            native: workspace.join(".runtime/native-studio"),
            legacy: workspace.join("data"),
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    paths.native = args.next().ok_or("--data-dir requires a path")?.into()
                }
                "--legacy-dir" => {
                    paths.legacy = args.next().ok_or("--legacy-dir requires a path")?.into()
                }
                "--help" | "-h" => {
                    println!(
                        "OmniuSong native studio\n  --data-dir PATH    Native session store\n  --legacy-dir PATH  Read-only legacy data directory\n  --evidence-history Start with metadata history only; do not open/create the catalog\nNo server is required. Audio playback and explicit solo capture use a separate native process."
                    );
                    return Ok(None);
                }
                _ => return Err(format!("Unknown argument: {arg}").into()),
            }
        }
        Ok(Some(paths))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Draft {
    name: String,
    tempo: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftFamily {
    Metadata,
    Note,
    Clip,
}

fn note_command(command: &Command) -> bool {
    matches!(command,Command::Edit{intent} if matches!(intent.scope,EditScope::DocumentNote{..}))
}
fn clip_command(command: &Command) -> bool {
    matches!(command,Command::Edit{intent} if matches!(intent.scope,EditScope::DocumentClip{..}))
}

impl Draft {
    fn from_document(document: &ProjectDocument) -> Self {
        Self {
            name: document.name().into(),
            tempo: document.tempo().to_string(),
        }
    }
    fn is_dirty(&self, document: &ProjectDocument) -> bool {
        self.name != document.name() || self.tempo != document.tempo().to_string()
    }
    fn values(&self) -> AppResult<(String, f64)> {
        if self.name.trim().is_empty() || self.name.chars().count() > 160 {
            return Err("Session name must contain 1–160 characters".into());
        }
        let tempo = self
            .tempo
            .parse::<f64>()
            .map_err(|_| "Tempo must be a number from 30 to 300")?;
        if !tempo.is_finite() || !(30.0..=300.0).contains(&tempo) {
            return Err("Tempo must be a number from 30 to 300".into());
        }
        Ok((self.name.clone(), tempo))
    }
}

#[derive(Clone)]
enum Action {
    Create,
    Open(String),
    Import(PathBuf),
    Save,
    Undo,
    Redo,
    Refresh,
    Export(PathBuf),
    Inspect(PathBuf),
    Restore {
        path: PathBuf,
        intent: RestoreCopyIntent,
    },
    AttachCapture {
        journal_path: PathBuf,
        publication: Box<omniusong_store::TakePublication>,
    },
    Performance(Box<Command>),
    PerformanceSource(SourceRequest),
    Evidence(EvidenceUiRequest),
    SourceRecovery(SourceRecoveryRequest),
    Recovery(RecoveryAction),
    Note(NoteAction),
    Clip(ClipAction),
    StagePlacement {
        expected: Box<ClipContext>,
        value: PlacementValue,
    },
    Close,
}
#[derive(Clone)]
struct Active {
    id: RequestId,
    command: Command,
    action: Action,
    context: Option<(String, u64)>,
    draft: Draft,
    note_context: NoteContext,
    clip_context: ClipContext,
    after: Option<Action>,
}
struct InspectedBackup {
    path: PathBuf,
    inspection: ArchiveInspection,
}
struct StudioApp {
    paths: Paths,
    service: Option<SessionService>,
    ready: bool,
    native_origin: Option<PathBuf>,
    evidence_metadata_only: bool,
    evidence: EvidenceUi,
    evidence_context: Option<evidence_integration::EvidenceContext>,
    source_recovery: SourceRecoveryUi,
    source_recovery_context: Option<source_recovery_integration::SourceRecoveryContext>,
    reconnect_existing: bool,
    active: Option<Active>,
    retry: Option<Active>,
    sessions: Vec<omniusong_session::ProjectSummary>,
    legacy: Vec<LegacyProject>,
    legacy_notice: Option<String>,
    legacy_selection: usize,
    document: Option<ProjectDocument>,
    recordings: Vec<Option<RecordingPayload>>,
    provenance: Vec<Option<String>>,
    draft: Draft,
    new_name: String,
    selected_track: usize,
    notes: NoteEditor,
    clips: ClipEditor,
    waveform: waveform_ui::WaveformUi,
    arrangement: arrangement_ui::ArrangementUi,
    pending: Option<Action>,
    status: String,
    error: Option<String>,
    closing: bool,
    roll_needs_center: bool,
    export_path: String,
    restore_path: String,
    inspected: Option<InspectedBackup>,
    audio: AudioUi,
    audio_executable: Option<PathBuf>,
    audio_close_requested: bool,
    recovery: RecoveryUi,
    performance_sources: PerformanceSourceUi,
    recovery_close_requested: bool,
    note_close_requested: bool,
    clip_close_requested: bool,
}
impl StudioApp {
    // Construction is pure. The first rendered loading frame starts the worker.
    fn new(paths: Paths) -> Self {
        let mut audio = AudioUi::new();
        audio.set_store_root(paths.native.clone());
        Self {
            paths,
            service: None,
            ready: false,
            native_origin: None,
            evidence_metadata_only: false,
            evidence: EvidenceUi::default(),
            evidence_context: None,
            source_recovery: SourceRecoveryUi::default(),
            source_recovery_context: None,
            reconnect_existing: false,
            active: None,
            retry: None,
            sessions: vec![],
            legacy: vec![],
            legacy_notice: None,
            legacy_selection: 0,
            document: None,
            recordings: vec![],
            provenance: vec![],
            draft: Draft::default(),
            new_name: "My native session".into(),
            selected_track: 0,
            notes: NoteEditor::default(),
            clips: ClipEditor::default(),
            waveform: waveform_ui::WaveformUi::default(),
            arrangement: arrangement_ui::ArrangementUi::default(),
            pending: None,
            status: "Opening your local studio.".into(),
            error: None,
            closing: false,
            roll_needs_center: true,
            export_path: String::new(),
            restore_path: String::new(),
            inspected: None,
            audio,
            audio_executable: None,
            audio_close_requested: false,
            recovery: RecoveryUi::new(),
            performance_sources: PerformanceSourceUi::new(),
            recovery_close_requested: false,
            note_close_requested: false,
            clip_close_requested: false,
        }
    }
    fn with_evidence_history(mut self, enabled: bool) -> Self {
        if enabled {
            self.evidence_metadata_only = true;
            self.evidence.open = true;
            self.reconnect_existing = true;
            self.status =
                "Evidence history mode. Choose explicit reads; the studio catalog is not opened."
                    .into();
        }
        self
    }
    fn with_audio_executable(mut self, executable: PathBuf) -> Self {
        self.audio_executable = Some(executable);
        self
    }
    fn context(&self) -> Option<(String, u64)> {
        self.document
            .as_ref()
            .map(|doc| (doc.id().into(), doc.revision()))
    }
    fn busy(&self) -> bool {
        self.active.is_some() || self.service.as_ref().is_some_and(SessionService::is_busy)
    }
    fn disconnected(&self) -> bool {
        self.service
            .as_ref()
            .is_some_and(SessionService::is_disconnected)
    }
    fn metadata_dirty(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(|doc| self.draft.is_dirty(doc))
    }
    fn dirty(&self) -> bool {
        self.metadata_dirty() || self.musical_dirty()
    }
    fn musical_dirty(&self) -> bool {
        self.notes.dirty() || self.clips.dirty()
    }
    fn musical_unresolved(&self) -> bool {
        self.notes.unresolved() || self.clips.unresolved()
    }
    fn dirty_family(&self) -> Result<Option<DraftFamily>, String> {
        let mut selected = None;
        for (family, dirty) in [
            (DraftFamily::Metadata, self.metadata_dirty()),
            (DraftFamily::Note, self.notes.dirty()),
            (DraftFamily::Clip, self.clips.dirty()),
        ] {
            if dirty && selected.replace(family).is_some() {
                return Err("More than one draft family is active. All fields were kept; no combined save or discard was performed.".into());
            }
        }
        Ok(selected)
    }
    fn retained_action_allowed(&self, action: &Action) -> bool {
        let held = self.notes.failure.as_ref().is_some_and(|f| !f.suspended)
            || self.clips.failure.as_ref().is_some_and(|f| !f.suspended);
        !held
            || matches!(
                action,
                Action::Close
                    | Action::Refresh
                    | Action::PerformanceSource(_)
                    | Action::Note(
                        NoteAction::Retry
                            | NoteAction::Suspend
                            | NoteAction::ReviewLatest
                            | NoteAction::NewChoice
                            | NoteAction::ReleaseRejected
                    )
                    | Action::Clip(
                        ClipAction::Retry
                            | ClipAction::Suspend
                            | ClipAction::ReviewLatest
                            | ClipAction::NewChoice
                            | ClipAction::ReleaseRejected
                    )
            )
            || matches!(action,Action::Recovery(recovery) if recovery.read_only())
    }
    fn can_start_audio(&self) -> bool {
        self.ready
            && !self.busy()
            && !self.disconnected()
            && self.dirty_family().is_ok_and(|family| family.is_none())
            && !self.musical_unresolved()
            && self.pending.is_none()
            && !self.recovery.blocks_session()
            && !self.audio.blocks_session()
    }
    fn install_document(&mut self, view: DocumentView) {
        self.arrangement.document_installed();
        let old_track = self
            .document
            .as_ref()
            .filter(|doc| doc.id() == view.document.id())
            .and_then(|doc| doc.tracks().get(self.selected_track))
            .and_then(|track| track["id"].as_str())
            .map(str::to_owned);
        self.notes.document_installed(&view.document);
        self.clips.document_installed(&view.document);
        self.draft = Draft::from_document(&view.document);
        self.selected_track = old_track
            .and_then(|id| {
                view.document
                    .tracks()
                    .iter()
                    .position(|track| track["id"].as_str() == Some(&id))
            })
            .unwrap_or(0);
        self.document = Some(view.document);
        self.recordings = view.recordings;
        self.provenance = view.provenance;
        self.roll_needs_center = true;
    }
    fn library(&mut self, library: Library) {
        if let Some(origin) = &library.native_root {
            self.native_origin = Some(origin.clone());
            self.paths.native = origin.clone();
            self.audio.set_store_root(origin.clone());
        }
        self.sessions = library.sessions;
        self.legacy = library.legacy.projects;
        self.legacy_notice = library.legacy_notice;
    }
    fn start(&mut self) {
        if self.evidence_metadata_only {
            self.start_service_mode(true, false);
        } else {
            self.start_service(false);
        }
    }
    fn start_service(&mut self, require_existing: bool) {
        self.start_service_mode(require_existing, true);
    }
    fn start_service_mode(&mut self, require_existing: bool, initialize: bool) {
        self.ready = false;
        self.evidence_metadata_only = !initialize;
        let paths = omniusong_session::Paths {
            native: self.paths.native.clone(),
            legacy: self.paths.legacy.clone(),
        };
        let spawned = if require_existing {
            SessionService::reconnect(paths)
        } else {
            SessionService::spawn(paths)
        };
        match spawned {
            Ok(service) => {
                self.performance_sources
                    .generation_changed(service.generation());
                self.evidence.generation_changed(service.generation());
                self.source_recovery
                    .generation_changed(service.generation());
                self.service = Some(service);
                if initialize
                    && let Err(error) = self.submit(Command::Initialize, Action::Refresh, None)
                {
                    self.error = Some(error.to_string());
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    fn submit(&mut self, command: Command, action: Action, after: Option<Action>) -> AppResult<()> {
        if matches!(command, Command::RunProjectArchive { .. }) {
            return Err("Governed project archives require their durable confirmation and result flow before native admission".into());
        }
        if self.active.is_some() {
            return Err(
                "The current native action still owns its completion; wait for that exact result"
                    .into(),
            );
        }
        if evidence_integration::evidence_command(&command) || matches!(action, Action::Evidence(_))
        {
            self.validate_evidence_submission(&command, &action, after.as_ref())?;
        }
        if source_recovery_integration::source_recovery_command(&command)
            || matches!(action, Action::SourceRecovery(_))
        {
            self.validate_source_recovery_submission(&command, &action, after.as_ref())?;
        }
        if matches!(
            command,
            Command::ScanPerformanceSources | Command::InspectPerformanceSource { .. }
        ) && !matches!(action, Action::PerformanceSource(_))
        {
            return Err("Performance source reads require their read-only action route".into());
        }
        if let Action::PerformanceSource(request) = &action {
            let allowed = match (request, &command) {
                (SourceRequest::Scan, Command::ScanPerformanceSources)
                | (SourceRequest::RefreshRequests, Command::ListPerformanceIntents) => true,
                (
                    SourceRequest::Inspect { stage_id, .. },
                    Command::InspectPerformanceSource { stage_id: actual },
                ) => stage_id == actual,
                _ => false,
            };
            if !allowed
                || after.is_some()
                || (matches!(request, SourceRequest::Inspect { .. })
                    && self.performance_sources.inspection_request().as_ref() != Some(request))
            {
                return Err("Source review accepts only its exact read-only command, without a continuation".into());
            }
        }
        let context = self.context();
        let draft = self.draft.clone();
        let note_context = self.notes.context();
        let clip_context = self.clips.context();
        let service = self
            .service
            .as_mut()
            .ok_or("Session worker has not started")?;
        match service.try_submit(command) {
            Ok(id) => {
                let command = service.pending().expect("accepted request").command.clone();
                if let Action::Recovery(recovery) = &action {
                    self.recovery.started(recovery);
                }
                self.active = Some(Active {
                    id,
                    command,
                    action,
                    context,
                    draft,
                    note_context,
                    clip_context,
                    after,
                });
                if let Some(Active {
                    action: Action::PerformanceSource(request),
                    ..
                }) = &self.active
                {
                    self.performance_sources.submitted(request, id);
                }
                if let Some(Active {
                    action: Action::Evidence(request),
                    ..
                }) = &self.active
                {
                    self.evidence.submitted(request, id);
                }
                if let Some(Active {
                    action: Action::SourceRecovery(request),
                    ..
                }) = &self.active
                {
                    self.source_recovery.submitted(request, id);
                }
                self.error = None;
                self.status = if matches!(self.active.as_ref().map(|active| &active.action), Some(Action::PerformanceSource(_))) {
                    "Read-only saved-recording review is running in the background. The original is kept unchanged."
                } else {
                    "Work is running in the background. If the app closes, it may already have saved; reopening lets you inspect the result."
                }.into();
                Ok(())
            }
            Err(error) => {
                if error.kind == omniusong_session::SubmitFailure::Disconnected {
                    self.performance_sources
                        .worker_lost(service.generation(), &error.message);
                    self.evidence
                        .worker_lost(service.generation(), &error.message);
                    self.source_recovery
                        .worker_lost(service.generation(), &error.message);
                }
                if let Action::PerformanceSource(request) = &action {
                    self.performance_sources
                        .submission_failed(request, &error.message);
                }
                if let Action::Evidence(request) = &action {
                    self.evidence.submission_failed(request, &error.message);
                }
                if let Action::SourceRecovery(request) = &action {
                    self.source_recovery
                        .submission_failed(request, &error.message);
                }
                if let Action::Recovery(recovery) = &action {
                    self.recovery.failed(recovery);
                }
                if let Command::Edit { intent } = &error.command
                    && matches!(intent.scope, EditScope::DocumentNote { .. })
                {
                    self.notes
                        .not_admitted(intent.clone(), error.message.clone());
                }
                if let Command::Edit { intent } = &error.command
                    && clip_command(&error.command)
                {
                    self.clips
                        .not_admitted(intent.clone(), error.message.clone());
                }
                if error.command.retry_safe()
                    && !matches!(
                        action,
                        Action::Performance(_)
                            | Action::PerformanceSource(_)
                            | Action::Evidence(_)
                            | Action::SourceRecovery(_)
                    )
                    && !(matches!(error.command, Command::Initialize) && self.retry.is_some())
                {
                    self.retry = Some(Active {
                        id: RequestId {
                            generation: service.generation(),
                            sequence: 0,
                        },
                        command: error.command,
                        action,
                        context,
                        draft,
                        note_context,
                        clip_context,
                        after,
                    });
                }
                Err(error.message.into())
            }
        }
    }
    fn poll(&mut self) {
        let Some(service) = self.service.as_mut() else {
            return;
        };
        if service.is_disconnected() {
            return;
        }
        let generation = service.generation();
        match service.try_poll() {
            Ok(Some(completion)) => self.complete(completion),
            Ok(None) => {}
            Err(error) => {
                if error.kind == FailureKind::Disconnected {
                    self.performance_sources
                        .worker_lost(generation, &error.message);
                    self.evidence.worker_lost(generation, &error.message);
                    self.source_recovery.worker_lost(generation, &error.message);
                }
                if error.kind == FailureKind::Disconnected
                    && let Some(active) = self.active.take()
                    && active.command.retry_safe()
                    && !(matches!(active.command, Command::Initialize) && self.retry.is_some())
                {
                    if matches!(active.action, Action::Performance(_)) {
                        self.audio
                            .performance_completed(&active.command, Err(&error));
                    }
                    if let Action::Recovery(recovery) = &active.action {
                        self.recovery.failed(recovery);
                    }
                    if matches!(active.action, Action::AttachCapture { .. }) {
                        self.audio.publication_failed();
                    }
                    if let Command::Edit { intent } = &active.command
                        && matches!(intent.scope, EditScope::DocumentNote { .. })
                    {
                        self.notes.failed_with_draft(
                            intent.clone(),
                            error.message.clone(),
                            false,
                            active.note_context.draft.clone(),
                        );
                    }
                    if let Command::Edit { intent } = &active.command
                        && clip_command(&active.command)
                    {
                        self.clips.failed_with_draft(
                            intent.clone(),
                            error.message.clone(),
                            false,
                            active.clip_context.draft.clone(),
                        );
                    }
                    if !matches!(
                        active.action,
                        Action::Performance(_)
                            | Action::PerformanceSource(_)
                            | Action::Evidence(_)
                            | Action::SourceRecovery(_)
                    ) {
                        self.retry = Some(active);
                    }
                }
                if error.kind == FailureKind::Disconnected {
                    self.status="Storage worker stopped. Your draft was kept; reconnect explicitly before another action.".into();
                }
                self.error = Some(error.to_string());
            }
        }
    }
    fn complete(&mut self, completion: Completion) {
        let Some(active) = self.active.take() else {
            self.error = Some("Ignored a completion without an active action".into());
            return;
        };
        let source_reply_matches = match (&active.action, &completion.result) {
            (Action::PerformanceSource(SourceRequest::Scan), Ok(Reply::PerformanceSources(_)))
            | (
                Action::PerformanceSource(SourceRequest::RefreshRequests),
                Ok(Reply::PerformanceIntents(_)),
            )
            | (
                Action::PerformanceSource(SourceRequest::Inspect { .. }),
                Ok(Reply::PerformanceSourceInspected(_)),
            ) => true,
            (Action::PerformanceSource(_), Ok(_)) => false,
            _ => true,
        };
        let evidence_reply_matches = match &active.action {
            Action::Evidence(request) => {
                evidence_integration::completion_matches(request, &completion.result)
            }
            _ => true,
        };
        let recovery_reference_matches = match &active.action {
            Action::SourceRecovery(request) => {
                source_recovery_integration::reference_matches(request, &completion.result)
            }
            _ => true,
        };
        if active.id != completion.id
            || active.command.kind() != completion.kind
            || !source_reply_matches
            || !evidence_reply_matches
            || !recovery_reference_matches
        {
            self.active = Some(active);
            self.error = Some("Ignored a stale session completion".into());
            return;
        }
        if let Action::SourceRecovery(request) = &active.action {
            self.complete_source_recovery(request, active.id, completion.result);
            return;
        }
        if let Action::Evidence(request) = &active.action {
            self.complete_evidence(request, active.id, completion.result.as_ref());
            return;
        }
        // Read-only source review owns no document or mutation continuation.
        // In particular, neither success nor a source-local failure may retire
        // an exact project-edit retry retained elsewhere in this window.
        if let Action::PerformanceSource(request) = &active.action {
            self.performance_sources
                .completed(request, active.id, completion.result.as_ref());
            if let Err(error) = &completion.result {
                if error.kind == FailureKind::StorageUnavailable {
                    self.reconnect_existing = true;
                    self.performance_sources
                        .worker_lost(active.id.generation, &error.message);
                    self.evidence
                        .worker_lost(active.id.generation, &error.message);
                    self.source_recovery
                        .worker_lost(active.id.generation, &error.message);
                    self.status = "Storage worker stopped. Your draft was kept; reconnect explicitly before another action.".into();
                } else {
                    self.status =
                        "Saved-recording review finished. The original and local drafts were kept."
                            .into();
                }
                self.error = Some(error.message.chars().take(512).collect());
            } else {
                self.status = "Read-only saved-recording review finished. See the source card for observed facts.".into();
                self.error = None;
            }
            return;
        }
        if matches!(active.action, Action::Performance(_))
            && matches!(active.command, Command::ListPerformanceIntents)
            && let Ok(Reply::PerformanceIntents(receipts)) = &completion.result
        {
            self.performance_sources
                .observe_requests(active.id.generation, receipts);
        }
        if matches!(active.action, Action::Performance(_)) {
            self.audio
                .performance_completed(&active.command, completion.result.as_ref());
        }
        match completion.result {
            Err(error) => {
                if let Command::Edit { intent } = &active.command
                    && matches!(intent.scope, EditScope::DocumentNote { .. })
                {
                    self.notes.failed_with_draft(
                        intent.clone(),
                        error.message.clone(),
                        !error.outcome_unknown
                            && matches!(
                                error.kind,
                                FailureKind::EditRejected | FailureKind::RevisionConflict
                            ),
                        active.note_context.draft.clone(),
                    );
                }
                if let Command::Edit { intent } = &active.command
                    && clip_command(&active.command)
                {
                    self.clips.failed_with_draft(
                        intent.clone(),
                        error.message.clone(),
                        !error.outcome_unknown
                            && matches!(
                                error.kind,
                                FailureKind::EditRejected | FailureKind::RevisionConflict
                            ),
                        active.clip_context.draft.clone(),
                    );
                }
                if let Action::Recovery(recovery) = &active.action {
                    self.recovery.failed(recovery);
                    if error.kind == FailureKind::RevisionConflict && !error.outcome_unknown {
                        self.recovery.revision_conflict(recovery);
                    }
                }
                if matches!(active.action, Action::AttachCapture { .. }) {
                    self.audio.publication_failed();
                }
                if error.kind == FailureKind::StorageUnavailable {
                    self.performance_sources
                        .worker_lost(active.id.generation, &error.message);
                    self.evidence
                        .worker_lost(active.id.generation, &error.message);
                    self.source_recovery
                        .worker_lost(active.id.generation, &error.message);
                    self.reconnect_existing = true;
                    self.status="Storage worker stopped. Your draft was kept; reconnect explicitly before another action.".into();
                } else {
                    self.status = if error.outcome_unknown {
                        "Action ended without a confirmed result. Inspect or explicitly retry the retained action."
                    } else {
                        "Action finished with an error. Your saved session and local draft were kept."
                    }.into();
                }
                if !matches!(active.action, Action::Performance(_))
                    && active.command.retry_safe()
                    && !(matches!(active.command, Command::Initialize) && self.retry.is_some())
                {
                    self.retry = Some(active);
                } else if !matches!(active.action, Action::Performance(_))
                    && !matches!(active.command, Command::Initialize)
                {
                    self.retry = None;
                }
                self.error = Some(format!(
                    "{}{}",
                    error.message,
                    if error.outcome_unknown {
                        " The action may have completed. Retry the same saved action when available, or refresh sessions / inspect the backup before starting another."
                    } else {
                        ""
                    }
                ));
            }
            Ok(reply) => {
                let mut after = None;
                match reply {
                    Reply::ProjectArchive(_) => {
                        self.error=Some("An unadmitted project archive result was received; the current editor and drafts were kept.".into());
                        self.status="Review the exact archive operation before continuing.".into();
                    }
                    Reply::EvidenceJobOutcome(_) | Reply::RetainedEvidenceInspected(_)
                    | Reply::EvidenceJobs(_) | Reply::EvidenceJob(_) => {
                        self.status = "Studio-evidence result retained; review its exact job history explicitly.".into();
                    }
                    Reply::Library(library) => {
                        if matches!(active.command, Command::Initialize)
                            && self.retry.as_ref().is_some_and(|retry| matches!(retry.command, Command::Initialize)) {
                            self.retry = None;
                        }
                        self.performance_sources.generation_changed(active.id.generation);
                        self.library(library);
                        self.ready = true;
                        self.reconnect_existing = true;
                        self.status = "Choose a session or create one.".into();
                    }
                    Reply::Document {
                        view,
                        library,
                        replayed,
                    } => {
                        self.library(library);
                        if self.context() != active.context || self.draft != active.draft || self.notes.context()!=active.note_context || self.clips.context()!=active.clip_context {
                            if let Action::Recovery(recovery)=&active.action {self.recovery.failed(recovery);}
                            if matches!(active.action, Action::AttachCapture { .. }) {
                                self.audio.publication_failed();
                            }
                            self.status="Action completed in storage; the newer local session/draft was kept.".into();
                            self.error=Some("The session or draft changed while this action ran. Its completion was retained in storage; your current draft was kept.".into());
                            if let Command::Edit{intent}=&active.command
                                && matches!(intent.scope,EditScope::DocumentNote {..}) {
                                self.notes.failed_with_draft(intent.clone(),"The edit completed in storage, but the local context changed. Inspect the saved session or retry the exact receipt; newer local fields were preserved.".into(),false,active.note_context.draft.clone());
                            }
                            if let Command::Edit{intent}=&active.command && clip_command(&active.command) {
                                self.clips.failed_with_draft(intent.clone(),"The edit completed in storage, but the local context changed. Inspect the saved session or retry the exact receipt; newer local fields were preserved.".into(),false,active.clip_context.draft.clone());
                            }
                            return;
                        }
                        self.status = if replayed {
                            "Action already completed; refreshed the latest saved session.".into()
                        } else {
                            format!(
                                "Opened {} · native revision {}",
                                view.document.name(),
                                view.document.revision()
                            )
                        };
                        if matches!(&active.command,Command::Edit{intent}if matches!(intent.scope,EditScope::DocumentNote{..})) {self.notes.succeeded();}
                        if clip_command(&active.command) {self.clips.succeeded();}
                        self.install_document(view);
                        if matches!(active.action, Action::AttachCapture { .. }) {
                            self.audio.publication_succeeded();
                        }
                        if matches!(active.action,Action::Recovery(RecoveryAction::Attach {..})) {self.recovery.attached();}
                        after = active.after;
                    }
                    Reply::Exported(receipt) => {
                        self.status = format!(
                            "Backup created: {}. Originals and source session were preserved.",
                            receipt.path.display()
                        )
                    }
                    Reply::Inspected(inspection) => {
                        if let Command::InspectArchive { path } = active.command {
                            if self.restore_path == path.to_string_lossy() {
                                self.inspected = Some(InspectedBackup { path, inspection });
                                self.status="Backup fingerprint and summary checked. Full validation happens when restoring a new session.".into();
                            } else {
                                self.error = Some(
                                    "Backup path changed; inspect the selected path again.".into(),
                                );
                            }
                        }
                    }
                    Reply::CaptureStages(scan)=>{self.recovery.scanned(scan);self.status="Retained stages scanned. Inspect a source before choosing recovery or attachment.".into();}
                    Reply::CaptureInspected(inspection)=>{
                        match self.recovery.inspected(inspection) {Ok(())=>self.status="Retained original inspected without changing its bytes. Review the verified prefix and unknown tail.".into(),Err(error)=>{self.status="Inspection finished; the retained selection was kept.".into();self.error=Some(error);}}
                    }
                    Reply::CaptureRecovered(receipt)=>{
                        match self.recovery.recovered(receipt) {Ok(())=>self.status="Verified prefix sealed and original rows retained. Choose an audio stream and attach explicitly; recovery did not attach audio to the session.".into(),Err(error)=>{self.status="Recovery finished without a matching displayed receipt; no attachment was started.".into();self.error=Some(error);}}
                    }
                    Reply::PerformanceIntent(_) => {
                        self.status = "Recording choice saved locally. Audio and attachment follow only the explicit confirmed choice.".into();
                    }
                    Reply::PerformanceIntents(receipts) => {
                        self.status = if receipts.is_empty() {
                            "No retained recording choices. Recording starts only after an explicit request.".into()
                        } else {
                            format!("{} retained recording choice(s) refreshed. Review a choice before continuing.", receipts.len())
                        };
                    }
                    Reply::PerformanceSources(_) | Reply::PerformanceSourceInspected(_) => {
                        self.status = "Read-only performance source review completed. Recovery and attachment require separate explicit choices.".into();
                    }
                    Reply::PreparedPerformanceSourceAction(_) | Reply::PreparedPerformanceSourceActions(_) => {
                        self.status = "Prepared source choice retained locally. No copy, recovery, audio or attachment was started.".into();
                    }
                    Reply::PerformanceSourceActionStates(_) | Reply::PerformanceSourceActionDetail(_) | Reply::PerformanceSourceExecution(_)
                    | Reply::PerformanceSourceDecision(_) | Reply::PerformanceSourceAttachment(_) => {
                        self.status = "Source-action evidence returned to its backend route; no project attachment was started.".into();
                    }
                }
                if !matches!(active.action, Action::Refresh | Action::Performance(_)) {
                    self.retry = None;
                }
                if let Some(action) = after {
                    self.execute(action);
                }
            }
        }
    }
    fn save(&mut self) -> AppResult<()> {
        self.dispatch(Action::Save, None)
    }
    fn request(&mut self, action: Action) {
        if matches!(action, Action::Evidence(_) | Action::SourceRecovery(_)) {
            if let Err(error) = self.dispatch(action, None) {
                self.error = Some(error.to_string());
            }
            return;
        }
        if self.audio.blocks_session() {
            if matches!(action, Action::Close) {
                self.audio.cleanup_live_input();
                self.audio_close_requested = true;
            } else {
                self.error = Some("Finish or stop audio, and resolve any retained sealed take, before changing this session.".into());
            }
            return;
        }
        if self.recovery.blocks_session()
            && !matches!(&action,Action::Recovery(recovery)if recovery.read_only()||matches!(recovery,RecoveryAction::Attach {..}))
            && !matches!(action, Action::PerformanceSource(_))
        {
            if matches!(action, Action::Close) {
                self.recovery_close_requested = true;
            } else {
                self.error = Some(
                    "Resolve the retained capture attachment before changing this session.".into(),
                );
            }
            return;
        }
        if !self.retained_action_allowed(&action) {
            self.error=Some("Keep the retained musical draft locally or review its saved target before continuing. Its exact request remains available.".into());
            return;
        }
        let resolve = match &action {
            Action::Note(
                NoteAction::Select(_) | NoteAction::Track(_) | NoteAction::BeginMetadata,
            )
            | Action::Clip(ClipAction::Select(_)) => self.musical_dirty(),
            Action::Note(NoteAction::BeginNote) => self.metadata_dirty() || self.clips.dirty(),
            Action::Clip(ClipAction::BeginClip) => self.metadata_dirty() || self.notes.dirty(),
            Action::Note(NoteAction::ReviewLatest | NoteAction::NewChoice) => {
                self.metadata_dirty() || self.clips.dirty()
            }
            Action::Clip(ClipAction::ReviewLatest | ClipAction::NewChoice) => {
                self.metadata_dirty() || self.notes.dirty()
            }
            Action::Note(
                NoteAction::Save
                | NoteAction::Discard
                | NoteAction::Retry
                | NoteAction::Suspend
                | NoteAction::ReleaseRejected,
            )
            | Action::Clip(
                ClipAction::Save
                | ClipAction::Discard
                | ClipAction::Retry
                | ClipAction::Suspend
                | ClipAction::ReleaseRejected,
            )
            | Action::Save
            | Action::Refresh
            | Action::PerformanceSource(_)
            | Action::StagePlacement { .. } => false,
            Action::Recovery(recovery) if recovery.read_only() => false,
            _ => self.dirty(),
        };
        // Even an outstanding read-only request cannot discard an unsent draft.
        if resolve {
            self.pending = Some(action);
        } else if matches!(action, Action::Close) && self.musical_unresolved() {
            if self.notes.unresolved() {
                self.note_close_requested = true;
            } else {
                self.clip_close_requested = true;
            }
        } else if self.busy() {
            if matches!(action, Action::Close) {
                self.closing = true;
            }
        } else {
            self.execute(action);
        }
    }
    fn execute(&mut self, action: Action) {
        if let Err(error) = self.dispatch(action, None) {
            self.error = Some(error.to_string());
        }
    }
    fn handle_performance_source_action(&mut self, action: SourceAction) {
        match action {
            SourceAction::Submit(request) => self.request(Action::PerformanceSource(request)),
            SourceAction::CancelInspection(id) => {
                // Cancellation is an urgent control for the existing read,
                // not a new service submission. Keep both credits until its
                // exact completion or actual disconnection is observed.
                let exact = self.performance_sources.pending_inspection() == Some(id)
                    && self.active.as_ref().is_some_and(|active| {
                        active.id == id
                            && matches!(
                                &active.action,
                                Action::PerformanceSource(SourceRequest::Inspect { .. })
                            )
                            && matches!(active.command, Command::InspectPerformanceSource { .. })
                    });
                let accepted = exact
                    && self
                        .service
                        .as_ref()
                        .is_some_and(|service| service.cancel_source_inspection(id));
                self.performance_sources.cancel_requested(id, accepted);
            }
        }
    }
    fn dispatch(&mut self, action: Action, after: Option<Action>) -> AppResult<()> {
        if let Action::Evidence(request) = &action {
            return self.submit(request.command(), action.clone(), after);
        }
        if let Action::SourceRecovery(request) = &action {
            return self.submit(request.command(), action.clone(), after);
        }
        if let Action::PerformanceSource(request) = &action {
            // Unlike musical actions, these reads can retain every local
            // draft/failed choice. They still share the sole worker and obey
            // the native audio-owner guard; they never carry a continuation.
            let result = if after.is_some() {
                Err("Source review cannot carry an action continuation".into())
            } else if !self.ready || self.busy() || self.disconnected() || self.service.is_none() {
                Err(
                    "Session storage is unavailable or busy; inspect again explicitly when ready"
                        .into(),
                )
            } else if self.audio.blocks_session() {
                Err("Finish or stop audio and resolve the retained recording before inspecting originals".into())
            } else if matches!(request, SourceRequest::Inspect { .. })
                && self.performance_sources.inspection_request().as_ref() != Some(request)
            {
                Err(
                    "The source selection changed; choose Inspect again for the current source"
                        .into(),
                )
            } else {
                return self.submit(request.command(), action.clone(), None);
            };
            if let Err(error) = &result {
                self.performance_sources
                    .submission_failed(request, &format!("{error}"));
            }
            return result;
        }
        if let Action::Performance(command) = &action {
            // Recording intentions share the sole storage service. They must be
            // admitted even while the audio guard is held (Finish and release),
            // but publication still requires the original clean target session.
            if self.busy() || !self.ready || self.disconnected() {
                return Err(
                    "Session storage is unavailable or busy; the exact recording choice was kept"
                        .into(),
                );
            }
            if !matches!(
                command.as_ref(),
                Command::ListPerformanceIntents
                    | Command::SavePerformanceIntent { .. }
                    | Command::AttachPerformance { .. }
            ) {
                return Err("This command is not a recording-intent action".into());
            }
            let target = self.audio.performance_target(command.as_ref());
            if matches!(command.as_ref(), Command::AttachPerformance { .. })
                && (target.is_none()
                    || target.as_deref() != self.document.as_ref().map(ProjectDocument::id)
                    || self.dirty()
                    || self.musical_unresolved()
                    || self.pending.is_some()
                    || self.recovery.blocks_session())
            {
                return Err(
                    "Open the original recording target and resolve local drafts or retained edits before this exact attachment"
                        .into(),
                );
            }
            return self.submit(command.as_ref().clone(), action.clone(), after);
        }
        if self.audio.blocks_session() && !matches!(action, Action::AttachCapture { .. }) {
            return Err(
                "Finish or stop audio and resolve the retained original before this action".into(),
            );
        }
        if self.busy() {
            return Err("Another action is still running".into());
        }
        if self.recovery.blocks_session()
            && !matches!(&action,Action::Recovery(recovery)if recovery.read_only()||matches!(recovery,RecoveryAction::Attach {..}))
        {
            return Err("Resolve the retained capture attachment before this action".into());
        }
        if !self.retained_action_allowed(&action) {
            return Err(
                "Suspend or review the retained musical edit before another session action".into(),
            );
        }
        if let Action::StagePlacement { expected, value } = &action {
            if !self.ready
                || self.disconnected()
                || self.metadata_dirty()
                || self.notes.dirty()
                || self.musical_unresolved()
                || self.pending.is_some()
                || self.closing
                || self.audio_close_requested
                || self.recovery_close_requested
                || self.note_close_requested
                || self.clip_close_requested
            {
                return Err(
                    "Resolve session state and other drafts before moving this clip".into(),
                );
            }
            let doc = self.document.as_ref().ok_or("No open session")?;
            self.clips.stage_placement(doc, expected, *value)?;
            self.status =
                "Placement staged locally. Review the clip, then Save clip or Discard clip draft."
                    .into();
            return Ok(());
        }
        // Local choices never touch the service. Resolve guards run before here.
        if let Action::Note(choice) = &action {
            match choice {
                NoteAction::Select(selection) => {
                    if self.musical_dirty() {
                        return Err("Save or discard the active musical draft before selecting another note".into());
                    }
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    self.notes.select(doc, selection.clone())?;
                    self.selected_track = doc
                        .tracks()
                        .iter()
                        .position(|track| track["id"].as_str() == Some(&selection.address.track_id))
                        .ok_or("Track is no longer present")?;
                    self.clips.clear_selection();
                    self.roll_needs_center = true;
                    return Ok(());
                }
                NoteAction::Track(id) => {
                    if self.musical_dirty() {
                        return Err(
                            "Save or discard the active musical draft before changing track".into(),
                        );
                    }
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    self.selected_track = doc
                        .tracks()
                        .iter()
                        .position(|track| track["id"].as_str() == Some(id))
                        .ok_or("Track is no longer present")?;
                    self.notes.clear_selection();
                    self.clips.clear_selection();
                    self.roll_needs_center = true;
                    return Ok(());
                }
                NoteAction::Add(selection) => {
                    if self.musical_unresolved() || self.dirty() {
                        return Err("Resolve retained musical edits and the other draft before adding a note".into());
                    }
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    self.notes.add(doc, selection.clone())?;
                    return Ok(());
                }
                NoteAction::BeginMetadata => {
                    if self.musical_dirty() {
                        return Err("Save or discard the musical draft first".into());
                    }
                    return Ok(());
                }
                NoteAction::BeginNote => {
                    if self.metadata_dirty() || self.clips.dirty() || self.musical_unresolved() {
                        return Err(
                            "Resolve session details and clip edits before starting a note draft"
                                .into(),
                        );
                    }
                    return Ok(());
                }
                NoteAction::Discard => {
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    self.notes.discard(doc);
                    return Ok(());
                }
                NoteAction::Suspend => {
                    self.notes.suspend();
                    if let Some(retry) = &mut self.retry
                        && matches!(&retry.command,Command::Edit{intent}if matches!(intent.scope,EditScope::DocumentNote {..}))
                    {
                        retry.after = None;
                    }
                    self.pending = None;
                    return Ok(());
                }
                NoteAction::ReviewLatest => {
                    if self.metadata_dirty() || self.clips.dirty() {
                        return Err("Save or discard the other draft before reviewing this note's saved session".into());
                    }
                    let failure = self.notes.failure.as_ref().ok_or("No retained note edit")?;
                    if !failure.known_rejected {
                        return Err(
                            "An uncertain result cannot be replaced with a new choice".into()
                        );
                    }
                    let project_id = failure.intent.project_id.clone();
                    self.notes.suspend();
                    self.pending = None;
                    if let Some(retry) = &mut self.retry {
                        retry.after = None;
                    }
                    return self.submit(
                        Command::Open {
                            project_id: project_id.clone(),
                        },
                        Action::Open(project_id.0),
                        None,
                    );
                }
                NoteAction::NewChoice => {
                    if self.metadata_dirty() || self.clips.dirty() || self.clips.unresolved() {
                        return Err("Resolve the other draft and retained clip edit before making a new note choice".into());
                    }
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    let retired = self
                        .notes
                        .failure
                        .as_ref()
                        .ok_or("No retained musical edit")?
                        .intent
                        .clone();
                    self.notes.new_choice(doc)?;
                    self.clear_retry_for_intent(&retired);
                    return Ok(());
                }
                NoteAction::ReleaseRejected => {
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    let retired = self
                        .notes
                        .failure
                        .as_ref()
                        .ok_or("No retained musical edit")?
                        .intent
                        .clone();
                    let before_release = (self.notes.context(), self.clips.context());
                    self.notes.release_rejected(doc)?;
                    self.clear_retry_for_intent(&retired);
                    self.preserve_metadata_retry_after_release(before_release);
                    return Ok(());
                }
                NoteAction::Retry => return self.retry_note(),
                _ => {}
            }
        }
        if let Action::Clip(choice) = &action {
            match choice {
                ClipAction::Select(selection) => {
                    if self.musical_dirty() {
                        return Err("Save or discard the active musical draft before selecting another clip".into());
                    }
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    self.clips.select(doc, selection.clone())?;
                    let track_id = match &selection.address {
                        omniusong_core::DocumentClipAddress::LegacyAudio { track_id }
                        | omniusong_core::DocumentClipAddress::NativeAudio { track_id, .. } => {
                            track_id
                        }
                    };
                    self.selected_track = doc
                        .tracks()
                        .iter()
                        .position(|track| track["id"].as_str() == Some(track_id))
                        .ok_or("Track is no longer present")?;
                    self.notes.clear_selection();
                    return Ok(());
                }
                ClipAction::BeginClip => {
                    if self.metadata_dirty() || self.notes.dirty() || self.musical_unresolved() {
                        return Err("Resolve session details and retained musical edits before starting a clip draft".into());
                    }
                    return Ok(());
                }
                ClipAction::Discard => {
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    self.clips.discard(doc);
                    return Ok(());
                }
                ClipAction::Suspend => {
                    self.clips.suspend();
                    if let Some(retry) = &mut self.retry
                        && clip_command(&retry.command)
                    {
                        retry.after = None;
                    }
                    self.pending = None;
                    return Ok(());
                }
                ClipAction::ReviewLatest => {
                    if self.metadata_dirty() || self.notes.dirty() {
                        return Err("Save or discard the other draft before reviewing this clip's saved session".into());
                    }
                    let failure = self.clips.failure.as_ref().ok_or("No retained clip edit")?;
                    if !failure.known_rejected {
                        return Err(
                            "An uncertain result cannot be replaced with a new choice".into()
                        );
                    }
                    let project_id = failure.intent.project_id.clone();
                    self.clips.suspend();
                    self.pending = None;
                    if let Some(retry) = &mut self.retry {
                        retry.after = None;
                    }
                    return self.submit(
                        Command::Open {
                            project_id: project_id.clone(),
                        },
                        Action::Open(project_id.0),
                        None,
                    );
                }
                ClipAction::NewChoice => {
                    if self.metadata_dirty() || self.notes.dirty() || self.notes.unresolved() {
                        return Err("Resolve the other draft and retained note edit before making a new clip choice".into());
                    }
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    let retired = self
                        .clips
                        .failure
                        .as_ref()
                        .ok_or("No retained musical edit")?
                        .intent
                        .clone();
                    self.clips.new_choice(doc)?;
                    self.clear_retry_for_intent(&retired);
                    return Ok(());
                }
                ClipAction::ReleaseRejected => {
                    let doc = self.document.as_ref().ok_or("No open session")?;
                    let retired = self
                        .clips
                        .failure
                        .as_ref()
                        .ok_or("No retained musical edit")?
                        .intent
                        .clone();
                    let before_release = (self.notes.context(), self.clips.context());
                    self.clips.release_rejected(doc)?;
                    self.clear_retry_for_intent(&retired);
                    self.preserve_metadata_retry_after_release(before_release);
                    return Ok(());
                }
                ClipAction::Retry => return self.retry_clip(),
                _ => {}
            }
        }
        let action = if let Action::Recovery(RecoveryAction::ChooseAttachment(choice)) = action {
            let document = self
                .document
                .as_ref()
                .ok_or("Open a session before attaching a retained original")?;
            Action::Recovery(self.recovery.prepare_attachment(
                &choice,
                document.id(),
                document.revision(),
            )?)
        } else {
            action
        };
        if let Action::Recovery(RecoveryAction::Attach { publication, .. }) = &action {
            if self.document.as_ref().map(ProjectDocument::id)
                != Some(publication.project_id.0.as_str())
            {
                return Err(
                    "Reopen the original target session before retrying this exact attachment"
                        .into(),
                );
            }
            if self.dirty() {
                return Err("Save or discard local edits before retrying this exact attachment; its expected revision will not change".into());
            }
        }
        let command = match &action {
            Action::Create => Command::Create {
                name: self.new_name.clone(),
            },
            Action::Open(id) => Command::Open {
                project_id: ProjectId(id.clone()),
            },
            Action::Import(path) => Command::ImportLegacy { path: path.clone() },
            Action::Refresh => Command::Refresh,
            Action::Save | Action::Undo | Action::Redo => {
                let document = self.document.as_ref().ok_or("No open session")?;
                let edit = match &action {
                    Action::Save => {
                        if self.musical_dirty() {
                            return Err(
                                "Save or discard the musical draft before saving session details"
                                    .into(),
                            );
                        }
                        let (name, tempo) = self.draft.values()?;
                        EditOperation::UpdateSession { name, tempo }
                    }
                    Action::Undo => EditOperation::Undo,
                    _ => EditOperation::Redo,
                };
                Command::Edit {
                    intent: ProjectEditIntent {
                        operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
                        project_id: ProjectId(document.id().into()),
                        expected_revision: document.revision(),
                        scope: EditScope::Project,
                        edit,
                        extensions: Default::default(),
                    },
                }
            }
            Action::Export(path) => Command::ExportArchive {
                project_id: ProjectId(
                    self.document
                        .as_ref()
                        .ok_or("Open a session before creating a backup")?
                        .id()
                        .into(),
                ),
                destination: path.clone(),
            },
            Action::Inspect(path) => Command::InspectArchive { path: path.clone() },
            Action::Restore { path, intent } => Command::RestoreCopy {
                path: path.clone(),
                intent: intent.clone(),
            },
            Action::AttachCapture {
                journal_path,
                publication,
            } => Command::AttachCapture {
                journal_path: journal_path.clone(),
                publication: publication.as_ref().clone(),
            },
            Action::Recovery(recovery) => recovery.command()?,
            Action::Performance(_) => {
                unreachable!("recording intentions use the guarded service path")
            }
            Action::PerformanceSource(_) => {
                unreachable!("source review uses the read-only service path")
            }
            Action::Evidence(_) | Action::SourceRecovery(_) => {
                unreachable!("review actions use their exact guarded service path")
            }
            Action::Note(choice) => {
                if self.metadata_dirty() || self.clips.dirty() || self.clips.unresolved() {
                    return Err(
                        "Resolve session details and clip edits before submitting a note edit"
                            .into(),
                    );
                }
                let doc = self.document.as_ref().ok_or("No open session")?;
                Command::Edit {
                    intent: self.notes.intent(doc, choice)?,
                }
            }
            Action::Clip(choice) => {
                if self.metadata_dirty() || self.notes.dirty() || self.notes.unresolved() {
                    return Err(
                        "Resolve session details and note edits before submitting a clip edit"
                            .into(),
                    );
                }
                let doc = self.document.as_ref().ok_or("No open session")?;
                Command::Edit {
                    intent: self.clips.intent(doc, choice)?,
                }
            }
            Action::Close => {
                self.closing = true;
                return Ok(());
            }
            Action::StagePlacement { .. } => unreachable!("placement is handled locally"),
        };
        self.submit(command, action, after)
    }
    fn attach_capture(
        &mut self,
        journal_path: PathBuf,
        publication: omniusong_store::TakePublication,
    ) {
        match self.dispatch(
            Action::AttachCapture {
                journal_path,
                publication: Box::new(publication),
            },
            None,
        ) {
            Ok(()) => self.audio.publication_started(),
            Err(error) => {
                self.audio.publication_failed();
                self.error = Some(error.to_string());
            }
        }
    }
    fn poll_audio(&mut self) {
        self.audio.poll();
        if self.ready
            && !self.busy()
            && !self.disconnected()
            && !self.closing
            && let Some(command) = self.audio.take_performance_command()
            && let Err(error) = self.dispatch(Action::Performance(Box::new(command.clone())), None)
        {
            self.audio
                .performance_submission_failed(&command, error.to_string());
        }
        if !self.busy()
            && let Some((path, publication)) = self.audio.take_auto_publication()
        {
            if !self.ready || self.disconnected() {
                self.audio.publication_failed();
                self.error=Some("The sealed original is retained. Reconnect storage, then explicitly retry attaching it.".into());
            } else {
                self.attach_capture(path, publication);
            }
        }
    }
    fn clear_retry_for_intent(&mut self, retired: &ProjectEditIntent) {
        if self.retry.as_ref().is_some_and(|active| {
            matches!(&active.command,
            Command::Edit { intent } if intent == retired)
        }) {
            self.retry = None;
        }
    }
    /// Explicit release of an unrelated known-rejected musical card may change
    /// only its local serial. Preserve a still-exact metadata retry across that
    /// release, never capture newer fields or carry a deferred action forward.
    fn preserve_metadata_retry_after_release(&mut self, before: (NoteContext, ClipContext)) {
        if self.musical_dirty() {
            return;
        }
        let context = self.context();
        let note_context = self.notes.context();
        let clip_context = self.clips.context();
        if let Some(active) = &mut self.retry
            && matches!(&active.command, Command::Edit { intent }
                if matches!(intent.scope, EditScope::Project)
                && matches!(intent.edit, EditOperation::UpdateSession { .. } | EditOperation::Rename { .. } | EditOperation::SetTempo { .. }))
            && active.context == context
            && active.draft == self.draft
            && active.note_context == before.0
            && active.clip_context == before.1
        {
            active.note_context = note_context;
            active.clip_context = clip_context;
            active.after = None;
        }
    }
    fn retry_last(&mut self) {
        if self.busy() || self.audio.blocks_session() || self.recovery.blocks_session() {
            self.error =
                Some("Resolve active audio/recovery and background work before retrying".into());
            return;
        }
        let Some(active) = self.retry.clone() else {
            return;
        };
        if matches!(
            active.action,
            Action::PerformanceSource(_) | Action::Evidence(_) | Action::SourceRecovery(_)
        ) {
            self.error = Some("Choose a fresh explicit read in its review panel. Evidence jobs are never retried automatically.".into());
            return;
        }
        if note_command(&active.command) {
            if let Err(error) = self.retry_note() {
                self.error = Some(error.to_string());
            }
            return;
        }
        if clip_command(&active.command) {
            if let Err(error) = self.retry_clip() {
                self.error = Some(error.to_string());
            }
            return;
        }
        if self.context() != active.context
            || self.draft != active.draft
            || self.notes.context() != active.note_context
            || self.clips.context() != active.clip_context
        {
            self.error = Some(
                "Your draft changed. Keep these edits and choose a new save or reopen action."
                    .into(),
            );
            return;
        }
        if let Err(error) = self.submit(active.command, active.action, active.after) {
            self.error = Some(error.to_string());
        }
    }
    fn retry_note(&mut self) -> AppResult<()> {
        if self.busy() || self.audio.blocks_session() || self.recovery.blocks_session() {
            return Err(
                "Resolve active audio/recovery and background work before exact note retry".into(),
            );
        }
        let failure = self
            .notes
            .failure
            .as_ref()
            .ok_or("No retained note edit to retry")?;
        if self.document.as_ref().map(ProjectDocument::id)
            != Some(failure.intent.project_id.0.as_str())
        {
            return Err(
                "Reopen the original target session before retrying this exact note edit".into(),
            );
        }
        if self.metadata_dirty() || self.clips.dirty() {
            return Err("Save or discard the other draft before exact note retry".into());
        }
        let intent = failure.intent.clone();
        let after = self
            .retry
            .as_ref()
            .filter(|active| {
                !failure.suspended
                    && active.context == self.context()
                    && active.draft == self.draft
                    && active.note_context == self.notes.context()
                    && active.clip_context == self.clips.context()
            })
            .and_then(|active| active.after.clone());
        // Old expected revision belongs to the durable request; submit captures
        // a fresh current UI context for this attempt independently.
        self.submit(
            Command::Edit { intent },
            Action::Note(NoteAction::Retry),
            after,
        )
    }
    fn retry_clip(&mut self) -> AppResult<()> {
        if self.busy() || self.audio.blocks_session() || self.recovery.blocks_session() {
            return Err(
                "Resolve active audio/recovery and background work before exact clip retry".into(),
            );
        }
        let failure = self
            .clips
            .failure
            .as_ref()
            .ok_or("No retained clip edit to retry")?;
        if self.document.as_ref().map(ProjectDocument::id)
            != Some(failure.intent.project_id.0.as_str())
        {
            return Err(
                "Reopen the original target session before retrying this exact clip edit".into(),
            );
        }
        if self.metadata_dirty() || self.notes.dirty() {
            return Err("Save or discard the other draft before exact clip retry".into());
        }
        let intent = failure.intent.clone();
        let after = self
            .retry
            .as_ref()
            .filter(|active| {
                !failure.suspended
                    && active.context == self.context()
                    && active.draft == self.draft
                    && active.note_context == self.notes.context()
                    && active.clip_context == self.clips.context()
            })
            .and_then(|active| active.after.clone());
        self.submit(
            Command::Edit { intent },
            Action::Clip(ClipAction::Retry),
            after,
        )
    }
    fn save_active_family(&mut self, after: Option<Action>) -> AppResult<()> {
        match self.dirty_family()? {
            Some(DraftFamily::Note) => self.dispatch(Action::Note(NoteAction::Save), after),
            Some(DraftFamily::Clip) => self.dispatch(Action::Clip(ClipAction::Save), after),
            Some(DraftFamily::Metadata) => self.dispatch(Action::Save, after),
            None => Err("There are no unsaved fields to save".into()),
        }
    }
    fn discard_active_family(&mut self) -> AppResult<()> {
        let family = self.dirty_family()?;
        if let Some(doc) = &self.document {
            match family {
                Some(DraftFamily::Note) if self.notes.unresolved() => {
                    self.notes.suspend();
                    if let Some(retry) = &mut self.retry {
                        retry.after = None;
                    }
                }
                Some(DraftFamily::Note) => self.notes.discard(doc),
                Some(DraftFamily::Clip) if self.clips.unresolved() => {
                    self.clips.suspend();
                    if let Some(retry) = &mut self.retry {
                        retry.after = None;
                    }
                }
                Some(DraftFamily::Clip) => self.clips.discard(doc),
                Some(DraftFamily::Metadata) => self.draft = Draft::from_document(doc),
                None => {}
            }
        }
        Ok(())
    }
    fn loading_ui(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show(root,|ui|{
            ui.add_space(45.0);ui.heading(if self.evidence_metadata_only {"Studio evidence history"} else if self.disconnected(){"Session storage is unavailable"}else{"Opening your local studio"});
            if ui.button("Studio evidence and job history").clicked() { self.evidence.open = true; }
            if ui.button("Recording recovery and retained choices").clicked() { self.source_recovery.open = true; }
            if self.evidence_metadata_only {
                ui.label("Evidence history is connected without opening the studio catalog. Use explicit actions in Studio evidence.");
                if !self.busy() && !self.disconnected() && ui.button("Open studio catalog").clicked() {
                    self.evidence_metadata_only = false;
                    self.execute_initialize();
                }
            } else { ui.label("Session storage and source summaries are handled in the background."); }
            ui.label("macOS may ask OmniuSong Native for access to the external drive. A system prompt, if shown, controls access.");
            if let Some(error)=&self.error{ui.colored_label(egui::Color32::LIGHT_RED,error);}
            if !self.busy()&&self.error.is_some()&&ui.button("Try opening storage again").clicked(){
                if self.disconnected()||self.service.is_none(){self.service=None;self.start_service(self.reconnect_existing);}else{self.execute_initialize();}
            }
            if self.busy() { ui.label("Work is running in the background. If the app closes, it may already have saved; reopening lets you inspect the result."); }
            if ui.button("Close").clicked(){self.request(Action::Close);}
        });
    }
    fn execute_initialize(&mut self) {
        self.evidence_metadata_only = false;
        if let Err(error) = self.submit(Command::Initialize, Action::Refresh, None) {
            self.error = Some(error.to_string());
        }
    }
    fn show(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        self.poll();
        self.poll_audio();
        self.show_evidence_window(&ctx);
        self.show_source_recovery_window(&ctx);
        self.waveform
            .poll(self.document.as_ref(), self.clips.selection.as_ref());
        if self.waveform.pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if !self.ready {
            self.loading_ui(root);
            self.draft_guard_ui(&ctx);
            self.note_close_guard_ui(&ctx);
            self.clip_close_guard_ui(&ctx);
            self.audio_close_guard_ui(&ctx);
            self.recovery_close_guard_ui(&ctx);
            if self.service.is_none() && self.error.is_none() && !self.closing {
                self.start();
            }
            if self.busy() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            return;
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some(executable) = self.audio_executable.take() {
            self.audio.start(executable, self.paths.native.clone());
        }
        if self.audio.needs_poll() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let can_start_audio = self.can_start_audio();
        let can_retry_attachment = self.ready
            && !self.busy()
            && !self.disconnected()
            && !self.dirty()
            && !self.musical_unresolved()
            && self.pending.is_none()
            && !self.recovery.blocks_session();
        let audio_context = self.context();
        let mut retry_attach = false;
        let mut requested = None;
        egui::Panel::top("transport").show(root, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new("omniuSong").color(egui::Color32::from_rgb(100, 225, 187)),
                );
                ui.label("NATIVE STUDIO · FOUNDATION");
                if ui.button("Studio evidence").clicked() {
                    self.evidence.open = true;
                }
                if ui.button("Recording recovery").clicked() {
                    self.source_recovery.open = true;
                }
                ui.separator();
            });
            retry_attach = self
                .audio
                .ui(ui, can_start_audio, can_retry_attachment, audio_context);
            if self.dirty() {
                ui.small("Save or discard draft edits before playing, live audition or recording.");
            }
            ui.add_space(5.0);
        });
        if retry_attach && let Some((path, publication)) = self.audio.retry_publication() {
            self.attach_capture(path, publication);
        }
        let available = !self.busy()
            && !self.disconnected()
            && !self.audio.blocks_session()
            && !self.recovery.blocks_session();
        let mut note_requested = None;
        let mut clip_requested = None;
        egui::Panel::bottom("status").show(root, |ui| {
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            ui.label(&self.status);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(available, egui::Button::new("Refresh sessions"))
                    .clicked()
                {
                    requested = Some(Action::Refresh);
                }
                if ui
                    .add_enabled(
                        available && self.retry.is_some(),
                        egui::Button::new("Retry last action"),
                    )
                    .clicked()
                {
                    self.retry_last();
                }
                if !self.busy() && self.disconnected() && ui.button("Reconnect storage").clicked() {
                    self.service = None;
                    self.start_service(self.reconnect_existing);
                }
                if ui.button("Close window").clicked() {
                    requested = Some(Action::Close);
                }
            });
            ui.small(
                "Native audio runs in a separate process. MIDI, plugins and model execution remain pending.",
            );
            if let Some(choice)=self.notes.failure_ui(ui,available,self.document.as_ref()){note_requested=Some(Action::Note(choice));}
            if let Some(choice)=self.clips.failure_ui(ui,available,self.document.as_ref()){clip_requested=Some(Action::Clip(choice));}
        });
        let recovery_available =
            !self.busy() && !self.disconnected() && !self.audio.blocks_session();
        let mut recovery_requested = None;
        let mut performance_source_requested = None;
        let source_availability = SourceAvailability {
            can_submit: self.ready
                && self.service.is_some()
                && !self.busy()
                && !self.disconnected()
                && !self.audio.blocks_session()
                && !self.closing,
            generation: self.service.as_ref().map(SessionService::generation),
            unavailable_reason: if self.disconnected() || !self.ready {
                Some("Reconnect session storage before reading originals.".into())
            } else if self.audio.blocks_session() {
                Some("Finish or stop audio and resolve the retained recording first.".into())
            } else if self.busy() {
                Some(
                    "The session worker is busy. Inspection cancellation remains available.".into(),
                )
            } else if self.closing {
                Some("The window is closing.".into())
            } else {
                None
            },
        };
        let recovery_draft_dirty = self.dirty();
        egui::Panel::right("retained-captures")
            .default_size(340.0)
            .min_size(260.0)
            .show(root, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("retained-capture-review")
                    .show(ui, |ui| {
                        performance_source_requested =
                            self.performance_sources.ui(ui, &source_availability);
                        ui.separator();
                        recovery_requested = self.recovery.ui(
                            ui,
                            recovery_available,
                            self.document.as_ref().map(ProjectDocument::id),
                            recovery_draft_dirty,
                        );
                    });
            });
        if let Some(action) = performance_source_requested {
            self.handle_performance_source_action(action);
        }
        if let Some(recovery) = recovery_requested {
            self.request(Action::Recovery(recovery));
        }
        let available = !self.busy()
            && !self.disconnected()
            && !self.audio.blocks_session()
            && !self.recovery.blocks_session();
        egui::Panel::left("sessions")
            .default_size(270.0)
            .min_size(210.0)
            .show(root, |ui| {
                if !available {ui.disable();}
                egui::ScrollArea::vertical().id_salt("library-sidebar").show(ui,|ui|{
                ui.heading("Your sessions");
                ui.label("New session name");
                ui.add(egui::TextEdit::singleline(&mut self.new_name).char_limit(160).desired_width(f32::INFINITY))
                    .accessible_name("New session name");
                if ui.button("Create session").clicked() {
                    requested = Some(Action::Create);
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("native-sessions")
                    .max_height(240.0)
                    .show(ui, |ui| {
                        if self.sessions.is_empty() {
                            ui.label("No native sessions yet.");
                        }
                        for session in &self.sessions {
                            let selected =
                                self.document.as_ref().is_some_and(|d| d.id() == session.id);
                            if ui.selectable_label(selected, &session.name).clicked() {
                                requested = Some(Action::Open(session.id.clone()));
                            }
                            ui.small(format!(
                                "{} tracks · {} notes · r{}",
                                session.track_count, session.note_count, session.revision
                            ));
                            if let Some(problem) = &session.problem {
                                ui.colored_label(
                                    egui::Color32::GOLD,
                                    format!("Needs recovery: {problem}"),
                                );
                            }
                        }
                    });
                ui.separator();
                ui.heading("Bring your work");
                ui.label("Legacy projects are read-only sources.");
                if let Some(notice) = &self.legacy_notice {
                    ui.colored_label(egui::Color32::GOLD, notice);
                }
                if self.legacy.is_empty() {
                    ui.small("No supported projects found in the legacy directory.");
                } else {
                    self.legacy_selection = self.legacy_selection.min(self.legacy.len() - 1);
                    ui.label("Legacy source project");
                    egui::ComboBox::from_id_salt("legacy-project")
                        .selected_text(&self.legacy[self.legacy_selection].name)
                        .show_ui(ui, |ui| {
                            for (index, project) in self.legacy.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.legacy_selection,
                                    index,
                                    &project.name,
                                );
                            }
                        })
                        .response
                        .accessible_name("Legacy source project");
                    ui.small(format!(
                        "{} tracks · separate native copy",
                        self.legacy[self.legacy_selection].tracks
                    ));
                    if ui.button("Import selected project").clicked() {
                        requested = Some(Action::Import(
                            self.legacy[self.legacy_selection].path.clone(),
                        ));
                    }
                }
                ui.add_space(8.0);
                ui.collapsing("Back up and restore",|ui|{
                    ui.label("New backup path");
                    ui.add(egui::TextEdit::singleline(&mut self.export_path).char_limit(4096)).accessible_name("New backup path");
                    if ui.add_enabled(self.document.is_some(),egui::Button::new("Create backup")).clicked(){requested=Some(Action::Export(PathBuf::from(&self.export_path)));}
                    ui.small("Choose a new file. Existing files are never overwritten.");
                    ui.separator();ui.label("Backup to inspect");
                    if ui.add(egui::TextEdit::singleline(&mut self.restore_path).char_limit(4096)).accessible_name("Backup to inspect").changed(){self.inspected=None;}
                    if ui.button("Inspect backup").clicked(){requested=Some(Action::Inspect(PathBuf::from(&self.restore_path)));}
                    if let Some(backup)=&self.inspected {
                        ui.label(format!("{} · {} tracks · {} original take(s)",backup.inspection.name,backup.inspection.track_count,backup.inspection.take_count));
                        ui.small("Preliminary summary and fingerprint only. Restoring checks the complete backup.");
                        ui.small(format!("Fingerprint: {}",backup.inspection.sha256));
                        if ui.button("Restore as new session").clicked(){requested=Some(Action::Restore{path:backup.path.clone(),intent:RestoreCopyIntent{operation_id:OperationId(uuid::Uuid::new_v4().to_string()),expected_archive_sha256:backup.inspection.sha256.clone()}});}
                    }
                    ui.small("Restoring creates a separate session and preserves the backup and existing sessions.");
                });
                ui.collapsing("Storage locations", |ui| {
                    ui.small(format!("Native: {}", self.paths.native.display()));
                    ui.small(format!("Legacy: {}", self.paths.legacy.display()));
                });
                });
            });
        egui::CentralPanel::default().show(root, |ui| {
            egui::ScrollArea::vertical().id_salt("session-editor").show(ui,|ui|{
            if let Some(document) = &self.document {
                let allow_select = available && self.pending.is_none() && !self.closing
                    && !self.audio_close_requested && !self.recovery_close_requested
                    && !self.note_close_requested && !self.clip_close_requested
                    && !self.musical_unresolved();
                let selected_track = document.tracks().get(self.selected_track)
                    .and_then(|track| track["id"].as_str());
                let clip_context = self.clips.context();
                let allow_stage = allow_select && !self.metadata_dirty() && !self.notes.dirty();
                if let Some(choice) = self.arrangement.show(ui, document, selected_track,
                    &clip_context, arrangement_ui::ArrangementAccess {
                        allow_select,
                        allow_stage,
                    }) {
                    requested = Some(match choice {
                        arrangement_ui::ArrangementAction::SelectTrack(id) => Action::Note(NoteAction::Track(id)),
                        arrangement_ui::ArrangementAction::SelectClip(selection) => Action::Clip(ClipAction::Select(selection)),
                        arrangement_ui::ArrangementAction::StagePlacement { expected, value } => Action::StagePlacement { expected, value },
                        arrangement_ui::ArrangementAction::SaveClip => Action::Clip(ClipAction::Save),
                        arrangement_ui::ArrangementAction::DiscardClip => Action::Clip(ClipAction::Discard),
                    });
                }
                ui.separator();
            }
            ui.add_enabled_ui(available, |ui| {
            if self.document.is_none() {
                ui.add_space(60.0); ui.heading("Your music stays yours.");
                ui.label("Create a native session, or bring in an existing OmniuSong project.");
                ui.label("Import preserves source notes, expression, provenance and referenced audio.");
                ui.label("Open a saved session to play supported sources or record a new solo original using the audio controls.");
                return;
            }
            ui.horizontal(|ui| {
                ui.label("Session");
                ui.add_enabled(!self.musical_dirty(),egui::TextEdit::singleline(&mut self.draft.name).char_limit(160).desired_width(280.0)).accessible_name("Session name");
                ui.label("BPM");
                ui.add_enabled(!self.musical_dirty(),egui::TextEdit::singleline(&mut self.draft.tempo).char_limit(32).desired_width(65.0)).accessible_name("Tempo in BPM");
                if ui.add_enabled(self.metadata_dirty()&&!self.musical_dirty(), egui::Button::new("Save changes")).clicked()
                    && let Err(error) = self.save() { self.error = Some(error.to_string()); }
                if ui.add_enabled(self.metadata_dirty()&&!self.musical_dirty(), egui::Button::new("Discard edits")).clicked()
                    && let Some(document) = &self.document { self.draft = Draft::from_document(document); }
                if self.musical_dirty() && ui.button("Edit session details").clicked(){requested=Some(Action::Note(NoteAction::BeginMetadata));}
            });
            ui.horizontal(|ui| {
                if ui.button("Undo saved change").clicked() { requested = Some(Action::Undo); }
                if ui.button("Redo saved change").clicked() { requested = Some(Action::Redo); }
                if self.dirty() { ui.colored_label(recovery_warning_color(ui.visuals().dark_mode), "Unsaved edits"); }
                else { ui.label("Saved"); }
            });
            if let Some(document) = &self.document {
                ui.small(format!("Schema {} · revision {} · {} bars · 960 ticks/beat", document.schema_version(), document.revision(), document.bars()));
                let recovery=&document.value()["omniusongOperatorRecovery"];
                if recovery["format"]=="omniusong.operator-recovery-evidence" && recovery["version"].as_u64()==Some(1) && recovery["historicalLedgerComplete"].as_bool()==Some(false) {
                    ui.colored_label(recovery_warning_color(ui.visuals().dark_mode),"Recovered snapshot · earlier edit history is incomplete");
                    ui.small("This session starts new edit history. The source checkpoint and recovery evidence are retained in the project.");
                }
                if let Some(import) = document.value().get("nativeImport") {
                    ui.collapsing("Imported original preserved", |ui| {
                        ui.label("Source identity and provenance retained. Referenced audio remains separately stored.");
                        ui.small(format!("Source session: {} · revision {}", text(&import["sourceId"]), import["sourceRevision"]));
                        ui.small(format!("Source SHA-256: {}", text(&import["sourceSha256"])));
                    });
                }
                ui.separator();
                let tracks = document.tracks();
                if tracks.is_empty() { ui.label("This session has no tracks."); }
                else {
                    self.selected_track = self.selected_track.min(tracks.len() - 1);
                    ui.horizontal_wrapped(|ui| {
                        for (index, track) in tracks.iter().enumerate() {
                            let label = format!("{}{}", text(&track["name"]), if track["locked"] == true { " · locked" } else { "" });
                            if ui.scope_builder(egui::UiBuilder::new().scope_id(egui::Id::unique(("track-selector",document.id(),text(&track["id"])))),|ui|ui.selectable_label(self.selected_track == index, label)).inner.clicked() {
                                requested=Some(Action::Note(NoteAction::Track(track["id"].as_str().unwrap_or_default().into())));
                            }
                        }
                    });
                    let track = &tracks[self.selected_track];
                    if document.supports_native_takes() && track["kind"] == "recording" {
                        ui.label(format!("Recording · {}", if track["muted"] == true { "muted" } else { "unmuted" }));
                        if let Some(payload)=self.recordings.get(self.selected_track).and_then(Option::as_ref){ui.small(format!("{} declared source view(s); audio clips are editable, event sources remain read-only.",payload.clips.len()));}
                        if let Some(choice)=self.clips.ui(ui,document,track["id"].as_str().unwrap_or_default(),available,self.draft.is_dirty(document)||self.notes.dirty(),self.notes.unresolved()){requested=Some(Action::Clip(choice));}
                    } else {
                        ui.label(format!("{} · {} · {}", text(&track["kind"]), text(&track["instrument"]), if track["muted"] == true { "muted" } else { "unmuted" }));
                        if let Some(Some(provenance)) = self.provenance.get(self.selected_track) {
                            ui.collapsing("Track provenance", |ui| { ui.monospace(provenance); });
                        }
                        if track["kind"] == "audio" {
                            if let Some(choice)=self.clips.ui(ui,document,track["id"].as_str().unwrap_or_default(),available,self.draft.is_dirty(document)||self.notes.dirty(),self.notes.unresolved()){requested=Some(Action::Clip(choice));}
                        }
                        else if track["kind"]=="notes" {
                            ui.columns(2,|columns|{
                                if let Some(note_id)=piano_overview(&mut columns[0], document.id(), track, document.bars(), self.notes.selection.as_ref(), &mut self.roll_needs_center){requested=Some(Action::Note(NoteAction::Select(NoteSelection{project_id:document.id().into(),address:omniusong_core::DocumentNoteAddress{track_id:track["id"].as_str().unwrap_or_default().into(),note_id}})));}
                                let metadata_dirty=self.draft.is_dirty(document)||self.clips.dirty();
                                let selected_id=self.notes.selection.as_ref().map(|s|s.address.note_id.clone());
                                if let Some(choice)=columns[1].scope_builder(egui::UiBuilder::new().scope_id(egui::Id::unique(("note-inspector",document.id(),text(&track["id"]),selected_id))),|ui|self.notes.ui(ui,document,track["id"].as_str().unwrap_or_default(),available&&!self.clips.unresolved(),metadata_dirty)).inner{requested=Some(Action::Note(choice));}
                            });
                        }
                    }
                }
                ui.separator();
                ui.collapsing("Model and performance integration", |ui| {
                    ui.label("Existing model research and legacy outputs are preserved. Native inference adapters are pending qualification.");
                    ui.add_enabled(false, egui::Button::new("Generate candidate · pending"));
                    ui.add_enabled(false, egui::Button::new("Connect MIDI · pending"));
                    ui.add_enabled(false, egui::Button::new("Load audio plugin · pending"));
                });
            }
            });
            if let Some(document) = &self.document {
                ui.separator();
                self.waveform.ui(ui, &self.paths.native, document, self.clips.selection.as_ref(), self.clips.draft.as_ref(), waveform_ui::WaveformAccess { unresolved: self.clips.unresolved(), allow_start: available });
            }
            });
        });
        if let Some(action) = clip_requested.or(note_requested).or(requested) {
            self.request(action);
        }
        self.draft_guard_ui(&ctx);
        self.audio_close_guard_ui(&ctx);
        self.recovery_close_guard_ui(&ctx);
        self.note_close_guard_ui(&ctx);
        self.clip_close_guard_ui(&ctx);
    }
    fn draft_guard_ui(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            let mut resolution = 0;
            let response = egui::Modal::new(egui::Id::unique("unsaved-edits")).show(ctx, |ui| {
                ui.heading("Save your edits?");
                ui.label(match self.dirty_family(){Ok(Some(DraftFamily::Note))=>"This note has unsaved changes. Save or discard this note draft before continuing.",Ok(Some(DraftFamily::Clip))=>"This clip has unsaved changes. Save or discard this clip draft before continuing.",Err(_)=>"Multiple draft families are active. All fields are kept; resolve one family explicitly before continuing.",_=>"This session has unsaved changes. Save or discard them before continuing."});
                if self.busy(){ui.label("Background work is still running. You can keep the window open or explicitly discard this local draft and close without waiting.");}
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.busy() && self.dirty_family().is_ok_and(|family| family == Some(DraftFamily::Metadata) || !self.musical_unresolved()),egui::Button::new("Save and continue")).clicked() {
                        resolution = 1;
                    }
                    if ui.button("Discard and continue").clicked() {
                        resolution = 2;
                    }
                    if ui.button("Cancel").clicked() {
                        resolution = 3;
                    }
                });
            });
            if response.should_close() {
                resolution = 3;
            }
            match resolution {
                1 => match self.save_active_family(self.pending.clone()) {
                    Ok(()) => {
                        self.pending = None;
                    }
                    Err(error) => self.error = Some(error.to_string()),
                },
                2 => match self.discard_active_family() {
                    Ok(()) => {
                        if let Some(action) = self.pending.take() {
                            self.request(action);
                        }
                    }
                    Err(error) => self.error = Some(error.to_string()),
                },
                3 => {
                    self.pending = None;
                    self.error = None;
                }
                _ => {}
            }
        }
    }
    fn recovery_close_guard_ui(&mut self, ctx: &egui::Context) {
        if !self.recovery_close_requested {
            return;
        }
        let mut resolution = 0;
        egui::Modal::new(egui::Id::unique("retained-recovery-close")).show(ctx,|ui|{
            ui.heading("Retained capture work is unresolved");
            ui.label("The original journal is retained. Recovery or attachment may already have completed; closing does not prove cancellation or a complete final tail.");
            if ui.button("Keep recovery window open").clicked(){resolution=1;}
            if ui.button("Close recovery without waiting").clicked(){resolution=2;}
        });
        if resolution == 1 {
            self.recovery_close_requested = false;
        }
        if resolution == 2 {
            self.recovery_close_requested = false;
            self.closing = true;
        }
    }
    fn note_close_guard_ui(&mut self, ctx: &egui::Context) {
        if !self.note_close_requested {
            return;
        }
        let mut close = false;
        let mut keep = false;
        egui::Modal::new(egui::Id::unique("retained-note-close")).show(ctx,|ui|{
            ui.heading("Retained note edit is window-local");
            ui.label("The attempted values and exact retry choices are kept only in this window. Closing discards all local note and clip cards; a possibly completed edit is not cancelled or undone. Reopening shows saved reality.");
            if ui.button("Keep note draft window open").clicked(){keep=true;}
            if ui.button("Discard local note card and close without waiting").clicked(){close=true;}
        });
        if keep {
            self.note_close_requested = false;
        }
        if close {
            self.note_close_requested = false;
            self.closing = true;
        }
    }
    fn clip_close_guard_ui(&mut self, ctx: &egui::Context) {
        if !self.clip_close_requested {
            return;
        }
        let mut close = false;
        let mut keep = false;
        egui::Modal::new(egui::Id::unique("retained-clip-close")).show(ctx, |ui| {
            ui.heading("Retained clip edit is window-local");
            ui.label("The attempted values and exact retry choices are kept only in this window. Closing discards all local note and clip cards; a possibly completed edit is not cancelled or undone. Reopening shows saved reality.");
            if ui.button("Keep clip draft window open").clicked() { keep = true; }
            if ui.button("Discard local clip card and close without waiting").clicked() { close = true; }
        });
        if keep {
            self.clip_close_requested = false;
        }
        if close {
            self.clip_close_requested = false;
            self.closing = true;
        }
    }
    fn audio_close_guard_ui(&mut self, ctx: &egui::Context) {
        if self.audio_close_requested {
            let mut close = 0;
            egui::Modal::new(egui::Id::unique("audio-close-guard")).show(ctx,|ui|{
                ui.heading(if self.audio.live_guard(){"Stop live output before closing"}else{"Keep the original take safe"});
                ui.label(if self.audio.performance_guard(){"Your recorded output or retained recording choice is still active. Finish recording or Stop, then review the saved original. Attaching it to this session is a separate choice."}else if self.audio.live_guard(){"Live audition is output only. This performance is not recorded or saved. Stop output before closing, or explicitly close without waiting."}else if self.audio.capture_guard(){"Audio or take attachment is still active. Finish the take and wait for attachment, or stop audio before closing."}else{"Saved-session playback is still active. Stop audio before closing, or explicitly close without waiting."});
                if self.audio.performance_guard() || !self.audio.live_guard(){ui.label("Closing without waiting can interrupt an unfinished original. A sealed journal is retained; attachment may already have completed. Closing does not prove cancellation or a complete final tail.");}
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled(self.audio.can_finish(),egui::Button::new("Finish take and keep window open")).clicked(){self.audio.finish();}
                    if ui.button("Stop audio and keep window open").clicked(){self.audio.stop();}
                    if ui.button("Keep window open").clicked(){close=1;}
                    if ui.button("Close without waiting").clicked(){close=2;}
                });
            });
            if close == 1 {
                self.audio_close_requested = false;
            }
            if close == 2 {
                self.audio_close_requested = false;
                self.closing = true;
            }
        }
    }
}

fn recovery_warning_color(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(255, 210, 90)
    } else {
        egui::Color32::from_rgb(112, 64, 0)
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("Unknown")
}

fn piano_overview(
    ui: &mut egui::Ui,
    project_id: &str,
    track: &Value,
    bars: u64,
    selected: Option<&NoteSelection>,
    needs_center: &mut bool,
) -> Option<String> {
    let mut chosen = None;
    let notes = track["notes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    ui.heading("Piano roll · inspect original notes");
    ui.label(format!(
        "{} saved notes · select an exact note below or in the roll · edit values in the inspector",
        notes.len()
    ));
    let max_end = notes
        .iter()
        .map(|n| {
            n["startTick"]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(n["durationTicks"].as_u64().unwrap_or(0))
        })
        .max()
        .unwrap_or(0);
    let total_ticks = max_end.max(bars * 3840).max(1);
    let content_width = (total_ticks as f32 / 3840.0 * 160.0).clamp(700.0, 24000.0);
    let mut scroll = egui::ScrollArea::both()
        .id_salt("piano-overview")
        .max_height(340.0);
    if *needs_center {
        let pitch = notes
            .first()
            .and_then(|note| note["pitch"].as_u64())
            .unwrap_or(60)
            .min(127);
        scroll = scroll.vertical_scroll_offset(((127 - pitch) as f32 * 8.0 - 160.0).max(0.0));
        *needs_center = false;
    }
    scroll.show(ui, |ui| {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(content_width, 1024.0), egui::Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Piano roll: full MIDI pitch range, horizontal and vertical scrolling. Exact values are available in the note list."));
        let clip = ui.clip_rect().intersect(rect);
        let painter = ui.painter().with_clip_rect(clip);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(19, 24, 31));
        for pitch in 0..=127 {
            let y = rect.top() + (127-pitch) as f32 * 8.0;
            if y < clip.top()-8.0 || y > clip.bottom() { continue; }
            if [1,3,6,8,10].contains(&(pitch%12)) {
                painter.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.left(),y),egui::vec2(content_width,8.0)),0.0,egui::Color32::from_rgb(27,31,39));
            }
            if pitch%12==0 { painter.text(egui::pos2(clip.left()+3.0,y),egui::Align2::LEFT_TOP,format!("C{}",pitch/12-1),egui::FontId::monospace(9.0),egui::Color32::GRAY); }
        }
        let visible_first = (((clip.left()-rect.left()) / content_width * total_ticks as f32 / 3840.0).floor() as u64).min(1_000_000);
        let visible_last = (((clip.right()-rect.left()) / content_width * total_ticks as f32 / 3840.0).ceil() as u64).min(visible_first+256);
        for bar in visible_first..=visible_last {
            let x = rect.left()+bar as f32*3840.0/total_ticks as f32*content_width;
            painter.line_segment([egui::pos2(x,clip.top()),egui::pos2(x,clip.bottom())],egui::Stroke::new(1.0,egui::Color32::from_gray(65)));
        }
        for (index,note) in notes.iter().enumerate() {
            let selected_note=selected.is_some_and(|selected|selected.project_id==project_id&&selected.address.track_id==text(&track["id"])&&selected.address.note_id==text(&note["id"]));
            let start = note["startTick"].as_u64().unwrap_or(0);
            let duration = note["durationTicks"].as_u64().unwrap_or(1);
            let pitch = note["pitch"].as_u64().unwrap_or(0).min(127);
            let note_rect = egui::Rect::from_min_size(egui::pos2(rect.left()+start as f32/total_ticks as f32*content_width,rect.top()+(127-pitch) as f32*8.0),egui::vec2((duration as f32/total_ticks as f32*content_width).max(4.0),7.0));
            if !note_rect.intersects(clip) { continue; }
            let response = ui.interact(note_rect,egui::Id::unique(("native-note",project_id,text(&track["id"]),text(&note["id"]))),egui::Sense::click());
            response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button,true,selected_note,format!("Note {} pitch {} tick {} duration {} velocity {}",index+1,pitch,start,duration,note["velocity16"])));
            if response.clicked() { chosen=note["id"].as_str().map(str::to_owned); }
            painter.rect_filled(note_rect,1.5,if selected_note { egui::Color32::GOLD } else { egui::Color32::from_rgb(99,215,182) });
        }
    });
    if let Some(selection) = selected
        && selection.project_id == project_id
        && selection.address.track_id == text(&track["id"])
        && let Some(note) = notes
            .iter()
            .find(|note| note["id"].as_str() == Some(&selection.address.note_id))
    {
        ui.label(format!(
            "Selected note · pitch {} · start {} ticks · length {} ticks · velocity {}/65535",
            note["pitch"], note["startTick"], note["durationTicks"], note["velocity16"]
        ));
    }
    egui::ScrollArea::vertical()
        .id_salt("note-list")
        .max_height(150.0)
        .show_rows(ui, 20.0, notes.len(), |ui, range| {
            for index in range {
                let note = &notes[index];
                if ui
                    .scope_builder(
                        egui::UiBuilder::new().scope_id(egui::Id::unique((
                            "note-row",
                            project_id,
                            text(&track["id"]),
                            text(&note["id"]),
                        ))),
                        |ui| {
                            ui.selectable_label(
                                selected.is_some_and(|selection| {
                                    selection.project_id == project_id
                                        && selection.address.track_id == text(&track["id"])
                                        && selection.address.note_id == text(&note["id"])
                                }),
                                format!(
                                    "Note {} · pitch {} · tick {} · length {} · velocity {}",
                                    index + 1,
                                    note["pitch"],
                                    note["startTick"],
                                    note["durationTicks"],
                                    note["velocity16"]
                                ),
                            )
                        },
                    )
                    .inner
                    .clicked()
                {
                    chosen = note["id"].as_str().map(str::to_owned);
                }
            }
        });
    chosen
}

impl StudioApp {
    fn native_ui(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().input(|i| i.viewport().close_requested()) && self.audio.blocks_session() {
            self.audio.cleanup_live_input();
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.audio_close_requested = true;
        } else if ui.ctx().input(|i| i.viewport().close_requested())
            && self.recovery.blocks_session()
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.recovery_close_requested = true;
        } else if ui.ctx().input(|i| i.viewport().close_requested()) && self.dirty() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.pending.is_none() {
                self.pending = Some(Action::Close);
            }
        } else if ui.ctx().input(|i| i.viewport().close_requested()) && self.musical_unresolved() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.notes.unresolved() {
                self.note_close_requested = true;
            } else {
                self.clip_close_requested = true;
            }
        }
        self.show(ui);
        self.audio.live_input(
            ui.ctx(),
            self.pending.is_some()
                || self.audio_close_requested
                || self.recovery_close_requested
                || self.note_close_requested
                || self.clip_close_requested
                || self.closing,
        );
        if self.closing {
            self.audio.stop();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.native_ui(ui);
    }
}

fn launch_arguments(args: Vec<String>) -> AppResult<(Vec<String>, bool)> {
    let count = args
        .iter()
        .filter(|arg| arg.as_str() == "--evidence-history")
        .count();
    if count > 1 {
        return Err("--evidence-history may be specified only once".into());
    }
    Ok((
        args.into_iter()
            .filter(|arg| arg != "--evidence-history")
            .collect(),
        count == 1,
    ))
}

fn main() -> AppResult<()> {
    let mut args = std::env::args();
    let executable = PathBuf::from(args.next().ok_or("Missing executable path")?);
    let args: Vec<_> = args.collect();
    if args.first().is_some_and(|arg| arg == "--audio-worker") {
        return omniusong_engine::run_worker().map_err(Into::into);
    }
    let (args, history_only) = launch_arguments(args)?;
    let Some(paths) = Paths::parse(args)? else {
        return Ok(());
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 1000.0])
            .with_min_inner_size([1000.0, 720.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "OmniuSong — Native Studio",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(
                StudioApp::new(paths)
                    .with_audio_executable(executable)
                    .with_evidence_history(history_only),
            ))
        }),
    )
    .map_err(|e| format!("Native application failed: {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::native::TakeRange;
    use omniusong_session::{Backend, CommandKind, ServiceError};
    use std::{fs, sync::mpsc, time::Instant};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/rust-migration/session-app-tests");
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
    fn frame(
        ctx: &egui::Context,
        app: &mut StudioApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 1000.0),
                )),
                events,
                ..Default::default()
            },
            |ui| app.native_ui(ui),
        );
        output.textures_delta.clear();
        output
    }
    fn settle(ctx: &egui::Context, app: &mut StudioApp) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let _ = frame(ctx, app, vec![]);
            if !app.busy() && (app.ready || app.error.is_some()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Session worker did not complete; {:?}",
                app.error
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn click(ctx: &egui::Context, app: &mut StudioApp, label: &str) {
        // egui Area deliberately hides a newly opened modal's first sizing
        // frame and requests repaint. Render that frame before finding its AX
        // control; an invisible sizing pass cannot receive an accessible click.
        let _ = frame(ctx, app, vec![]);
        let tree = frame(ctx, app, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let target = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .unwrap_or_else(|| {
                panic!(
                    "Missing action {label}; ready={}, busy={}, pending={}, text={:?}",
                    app.ready,
                    app.busy(),
                    app.pending.is_some(),
                    tree.nodes
                        .iter()
                        .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
                        .collect::<Vec<_>>()
                )
            })
            .0;
        let _ = frame(
            ctx,
            app,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: tree.tree_id,
                    target_node: target,
                    data: None,
                },
            )],
        );
    }
    fn ready(fixture: &Fixture) -> (egui::Context, StudioApp) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = StudioApp::new(fixture.paths());
        settle(&ctx, &mut app);
        assert!(app.ready, "{:?}", app.error);
        (ctx, app)
    }
    fn manual(fixture: &Fixture) -> (egui::Context, StudioApp, omniusong_session::TestPeer) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (service, peer) = SessionService::test_pair();
        let mut app = StudioApp::new(fixture.paths());
        app.service = Some(service);
        app.ready = true;
        app.install_document(
            DocumentView::prepare(ProjectDocument::new("Manual session").unwrap()).unwrap(),
        );
        (ctx, app, peer)
    }

    #[test]
    fn governed_archive_raw_routes_keep_editor_drafts_and_never_queue_work() {
        use omniusong_session::{ProjectArchiveOperation, ProjectArchiveRequest};
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        let document = app.document.as_ref().unwrap().clone();
        app.draft.name = "Unsent human draft".into();
        let name = app.draft.name.clone();
        for operation in [
            ProjectArchiveOperation::Export {
                project_id: ProjectId(document.id().into()),
                destination: "/synthetic/backup.osar".into(),
            },
            ProjectArchiveOperation::RestoreIdentity {
                archive_path: "/synthetic/backup.osar".into(),
            },
            ProjectArchiveOperation::RestoreCopy {
                archive_path: "/synthetic/backup.osar".into(),
                intent: RestoreCopyIntent {
                    operation_id: OperationId("native-unadmitted-copy".into()),
                    expected_archive_sha256: "a".repeat(64),
                },
            },
        ] {
            let command = Command::RunProjectArchive {
                request: Box::new(ProjectArchiveRequest {
                    version: 1,
                    origin_root: "/synthetic/native".into(),
                    operation,
                }),
            };
            assert!(
                app.submit(command.clone(), Action::Refresh, Some(Action::Save))
                    .unwrap_err()
                    .to_string()
                    .contains("durable confirmation")
            );
            assert!(
                app.dispatch(Action::Performance(Box::new(command)), None)
                    .is_err()
            );
            assert!(peer.requests.try_recv().is_err());
            assert!(app.active.is_none());
            assert_eq!(app.document.as_ref().unwrap(), &document);
            assert_eq!(app.draft.name, name);
        }
        assert!(!frame(&ctx, &mut app, vec![]).shapes.is_empty());
        assert!(peer.requests.try_recv().is_err());
        assert_eq!(app.draft.name, name);
    }

    #[test]
    fn recording_save_completion_after_stop_cannot_start_audio_or_attach() {
        use audio_ui::test_support;
        use omniusong_session::PerformanceIntentReceipt;
        let fixture = Fixture::new();
        let (ctx, mut app, store) = manual(&fixture);
        let (audio, engine) = test_support::pair();
        app.audio = audio;
        app.audio
            .set_store_root(PathBuf::from("/synthetic/owned-store"));
        let _ = frame(&ctx, &mut app, vec![]);
        let list = store.requests.try_recv().unwrap();
        assert!(matches!(list.command, Command::ListPerformanceIntents));
        store
            .responses
            .try_send(Completion {
                id: list.id,
                kind: list.command.kind(),
                result: Ok(Reply::PerformanceIntents(vec![])),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        let original = app.document.as_ref().unwrap().clone();
        click(&ctx, &mut app, "Record keys/pads");
        let _ = frame(&ctx, &mut app, vec![]);
        let save = store.requests.try_recv().unwrap();
        let Command::SavePerformanceIntent {
            expected_revision: None,
            intent,
        } = &save.command
        else {
            panic!("persist Start before audio");
        };
        assert_eq!(intent.project_id.0, original.id());
        assert!(intent.finish.is_none() && intent.attachment.is_none());
        assert!(engine.command().is_none());
        app.audio.stop();
        app.request(Action::Close);
        assert!(app.audio_close_requested && !app.closing);
        assert!(app.audio.performance_guard());
        assert!(engine.command().is_none());
        assert!(
            engine.urgent().is_none(),
            "native Start has never been admitted"
        );
        // Even a successful late intent commit is not permission to resurrect
        // the continuation cancelled by Stop.
        store
            .responses
            .try_send(Completion {
                id: save.id,
                kind: save.command.kind(),
                result: Ok(Reply::PerformanceIntent(Box::new(
                    PerformanceIntentReceipt {
                        revision: 1,
                        intent: (**intent).clone(),
                        payload_sha256: "a".repeat(64),
                    },
                ))),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(engine.command().is_none());
        assert!(store.requests.try_recv().is_err());
        assert_eq!(app.document.as_ref().unwrap(), &original);
        assert!(
            app.audio.performance_guard(),
            "retained card survives Stop and the late reply"
        );
    }

    #[test]
    fn performance_service_bypass_cannot_submit_foreign_mutations_or_dirty_attachment() {
        let fixture = Fixture::new();
        let (_ctx, mut app, store) = manual(&fixture);
        let before = app.document.as_ref().unwrap().clone();
        assert!(
            app.dispatch(
                Action::Performance(Box::new(Command::Create {
                    name: "Foreign bypass".into()
                })),
                None
            )
            .is_err()
        );
        app.draft.name = "Unsaved musician work".into();
        let attach = Command::AttachPerformance {
            id: omniusong_engine::LiveId([7; 16]),
            revision: 1,
        };
        assert!(
            app.dispatch(Action::Performance(Box::new(attach)), None)
                .is_err()
        );
        assert!(store.requests.try_recv().is_err());
        assert_eq!(app.document.as_ref().unwrap(), &before);
        assert_eq!(app.draft.name, "Unsaved musician work");
    }

    #[test]
    fn creator_is_io_free_and_held_startup_keeps_paint_close_and_drop_responsive() {
        struct Held {
            entered: mpsc::SyncSender<std::thread::ThreadId>,
            gate: mpsc::Receiver<()>,
        }
        impl Backend for Held {
            fn handle(&mut self, _: &Command) -> Result<Reply, ServiceError> {
                self.entered.send(std::thread::current().id()).unwrap();
                self.gate.recv().unwrap();
                Ok(Reply::Library(Library::default()))
            }
        }
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let mut app = StudioApp::new(paths.clone());
        assert!(!paths.native.exists());
        assert!(!paths.legacy.exists());
        let (entered, started) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::channel();
        app.service = Some(SessionService::spawn_with_backend(Held { entered, gate }).unwrap());
        app.execute_initialize();
        assert_ne!(
            started.recv_timeout(Duration::from_secs(1)).unwrap(),
            std::thread::current().id()
        );
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        for _ in 0..12 {
            assert!(!frame(&ctx, &mut app, vec![]).shapes.is_empty());
            assert!(app.busy());
        }
        assert!(!paths.native.exists());
        assert!(!paths.legacy.exists());
        click(&ctx, &mut app, "Close");
        assert!(app.closing);
        assert!(app.busy());
        let before = Instant::now();
        drop(app);
        assert!(before.elapsed() < Duration::from_millis(200));
        release.send(()).unwrap();
    }

    #[test]
    fn accessible_create_atomic_save_undo_redo_and_reopen_use_persistent_worker() {
        let fixture = Fixture::new();
        let (ctx, mut app) = ready(&fixture);
        app.new_name = "Native worker creation".into();
        click(&ctx, &mut app, "Create session");
        assert!(app.busy());
        settle(&ctx, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        let id = app.document.as_ref().unwrap().id().to_owned();
        app.draft.name = "One atomic draft save".into();
        app.draft.tempo = "125.5".into();
        click(&ctx, &mut app, "Save changes");
        settle(&ctx, &mut app);
        let saved = app.document.as_ref().unwrap();
        assert_eq!(saved.revision(), 1);
        assert_eq!(saved.name(), "One atomic draft save");
        assert_eq!(saved.tempo(), 125.5);
        click(&ctx, &mut app, "Undo saved change");
        settle(&ctx, &mut app);
        assert_eq!(
            app.document.as_ref().unwrap().name(),
            "Native worker creation"
        );
        assert_eq!(app.document.as_ref().unwrap().tempo(), 104.0);
        click(&ctx, &mut app, "Redo saved change");
        settle(&ctx, &mut app);
        assert_eq!(
            app.document.as_ref().unwrap().name(),
            "One atomic draft save"
        );
        drop(app);
        let (_, mut reopened) = ready(&fixture);
        reopened.request(Action::Open(id));
        settle(&ctx, &mut reopened);
        assert_eq!(reopened.document.as_ref().unwrap().tempo(), 125.5);
    }

    #[test]
    fn invalid_draft_and_save_before_navigation_keep_the_same_pending_action() {
        let fixture = Fixture::new();
        let (ctx, mut app) = ready(&fixture);
        app.execute(Action::Create);
        settle(&ctx, &mut app);
        let id = app.document.as_ref().unwrap().id().to_owned();
        app.draft.name = "Pending saved name".into();
        app.draft.tempo = "invalid".into();
        app.request(Action::Open(id.clone()));
        assert!(app.pending.is_some());
        assert!(app.save().is_err());
        assert!(!app.busy());
        assert_eq!(app.document.as_ref().unwrap().revision(), 0);
        app.draft.tempo = "128".into();
        click(&ctx, &mut app, "Save and continue");
        assert!(app.pending.is_none());
        assert!(app.busy());
        settle(&ctx, &mut app);
        assert_eq!(app.document.as_ref().unwrap().id(), id);
        assert_eq!(app.document.as_ref().unwrap().revision(), 1);
        assert_eq!(app.document.as_ref().unwrap().name(), "Pending saved name");
    }

    #[test]
    fn lost_ack_retry_keeps_operation_id_uses_new_transport_id_and_does_not_duplicate_save() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.draft.name = "Stable intent".into();
        app.save().unwrap();
        let first = peer.requests.try_recv().unwrap();
        let operation = match &first.command {
            Command::Edit { intent } => intent.operation_id.clone(),
            _ => panic!(),
        };
        peer.responses
            .try_send(Completion {
                id: first.id,
                kind: CommandKind::Edit,
                result: Err(ServiceError::backend("Acknowledgement was lost", true)),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.busy());
        assert_eq!(app.draft.name, "Stable intent");
        click(&ctx, &mut app, "Retry last action");
        let retry = peer.requests.try_recv().unwrap();
        assert_ne!(first.id, retry.id);
        assert!(matches!(retry.command,Command::Edit{ref intent}if intent.operation_id==operation));
        assert!(app.busy());
        app.request(Action::Create);
        assert!(peer.requests.try_recv().is_err());
    }

    #[test]
    fn stale_context_completion_never_replaces_newer_draft() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.draft.name = "Accepted save".into();
        app.save().unwrap();
        let request = peer.requests.try_recv().unwrap();
        let mut result = ProjectDocument::new("Accepted save").unwrap();
        result.rename("Backend result").unwrap();
        app.draft.name = "A newer local draft".into();
        peer.responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::Edit,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(result).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert_eq!(app.draft.name, "A newer local draft");
        assert_eq!(app.document.as_ref().unwrap().name(), "Manual session");
        assert!(app.error.as_ref().unwrap().contains("draft changed"));
    }

    #[test]
    fn export_inspect_and_restore_copy_are_worker_actions_with_dirty_resolution() {
        let fixture = Fixture::new();
        let (ctx, mut app) = ready(&fixture);
        app.execute(Action::Create);
        settle(&ctx, &mut app);
        let source_id = app.document.as_ref().unwrap().id().to_owned();
        let backup = fixture.0.join("new-backup.omniusong");
        app.draft.name = "Saved before backup".into();
        app.request(Action::Export(backup.clone()));
        assert!(app.pending.is_some());
        assert!(!backup.exists());
        click(&ctx, &mut app, "Save and continue");
        settle(&ctx, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(backup.exists());
        let bytes = fs::read(&backup).unwrap();
        app.restore_path = backup.to_string_lossy().into_owned();
        app.request(Action::Inspect(backup.clone()));
        settle(&ctx, &mut app);
        let inspected = app.inspected.as_ref().unwrap();
        let intent = RestoreCopyIntent {
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            expected_archive_sha256: inspected.inspection.sha256.clone(),
        };
        app.draft.name = "Explicitly discarded draft".into();
        app.request(Action::Restore {
            path: backup.clone(),
            intent: intent.clone(),
        });
        assert!(app.pending.is_some());
        click(&ctx, &mut app, "Discard and continue");
        settle(&ctx, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_ne!(app.document.as_ref().unwrap().id(), source_id);
        assert_eq!(app.document.as_ref().unwrap().name(), "Saved before backup");
        assert_eq!(fs::read(&backup).unwrap(), bytes);
        assert_eq!(app.sessions.len(), 2);
    }

    #[test]
    fn recording_view_ignores_opaque_legacy_arrays_and_keeps_exact_metadata() {
        use omniusong_core::{
            contracts::TakeId,
            native::{TakeClip, TakeReference, TimelinePosition},
        };
        let fixture = Fixture::new();
        let (ctx, mut app, _peer) = manual(&fixture);
        let mut document = ProjectDocument::new("Declared original")
            .unwrap()
            .upgrade_native()
            .unwrap();
        document
            .append_take_clip(
                "recording-track",
                "Original recording",
                TakeClip {
                    edit_source: None,
                    id: "declared-clip".into(),
                    source: TakeReference {
                        take_id: TakeId("original-take".into()),
                        manifest_sha256: "a".repeat(64),
                        stream_id: "microphone".into(),
                    },
                    range: TakeRange::AudioFrames {
                        start: 100,
                        count: 4,
                    },
                    position: TimelinePosition {
                        frame: -48_000,
                        sample_rate: 48_000,
                    },
                    fades: None,
                },
            )
            .unwrap();
        let mut value = document.value().clone();
        let track = value["tracks"].as_array_mut().unwrap().last_mut().unwrap();
        track["notes"] =
            serde_json::json!([null,{"startTick":u64::MAX,"durationTicks":u64::MAX},["opaque"]]);
        track["audio"] = serde_json::json!(["opaque"]);
        track["instrument"] = serde_json::json!(["opaque"]);
        track["provenance"] = serde_json::json!(["opaque"]);
        let document = ProjectDocument::from_value(value).unwrap();
        let retained = document.to_json().unwrap();
        app.install_document(DocumentView::prepare(document).unwrap());
        app.selected_track = app.document.as_ref().unwrap().tracks().len() - 1;
        click(&ctx, &mut app, "Select audio clip declared-clip");
        click(&ctx, &mut app, "Saved clip source and original settings");
        let tree = frame(&ctx, &mut app, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        for expected in [
            "Audio clip editor",
            "1 declared clip(s) · select by exact identity",
            "Saved takeId: original-take",
            "Saved source start: 100",
            "Saved frame count: 4",
            "Saved placement frame: -48000",
            "Saved sample rate: 48000",
        ] {
            assert!(text.contains(&expected), "Missing {expected}: {text:?}");
        }
        assert!(!text.iter().any(|value| value.contains("Piano roll")
            || value.starts_with("Note ")
            || value.contains("Track provenance")));
        assert!(app.roll_needs_center);
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), retained);
    }

    #[test]
    fn path_overrides_remain_explicit_and_need_no_server() {
        let paths = Paths::parse([
            "--data-dir".into(),
            "/tmp/native-song".into(),
            "--legacy-dir".into(),
            "/tmp/legacy-song".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(paths.native, PathBuf::from("/tmp/native-song"));
        assert_eq!(paths.legacy, PathBuf::from("/tmp/legacy-song"));
        assert!(Paths::parse(["--data-dir".into()]).is_err());
        assert!(Paths::parse(["--unknown".into()]).is_err());
    }

    #[test]
    fn fixture_cleanup_never_removes_another_exclusively_owned_fixture() {
        let first = Fixture::new();
        let second = Fixture::new();
        assert_ne!(first.0, second.0);
        let marker = first.0.join("original-marker");
        fs::write(&marker, b"preserve").unwrap();
        drop(second);
        assert_eq!(fs::read(marker).unwrap(), b"preserve");
    }

    fn note_fixture_document() -> (ProjectDocument, NoteSelection) {
        let mut value = ProjectDocument::new("Synthetic note UI")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["id"] = serde_json::json!("鍵盤\0");
        value["tracks"][0]["notes"] = serde_json::json!([
            {"id":"","pitch":60,"startTick":37,"durationTicks":960,"velocity16":65001,"expression":[{"type":"pitch","points":[{"tick":0,"value32":2147483648u32},{"tick":960,"value32":4294967295u32}]}],"input":{"sourceId":"synthetic","sourceName":"No device","protocol":"midi2","group":3,"channel":5,"receivedTimeMs":1.25,"releaseVelocity16":61111,"keyDurationTicks":900},"opaque":{"keep":[null,"exact"]}},
            {"id":"same-pitch","pitch":60,"startTick":37,"durationTicks":960,"velocity16":123,"expression":[]}
        ]);
        let document = ProjectDocument::from_value(value).unwrap();
        let selection = NoteSelection {
            project_id: document.id().into(),
            address: omniusong_core::DocumentNoteAddress {
                track_id: "鍵盤\0".into(),
                note_id: "".into(),
            },
        };
        (document, selection)
    }
    fn revision(document: &ProjectDocument, revision: u64) -> ProjectDocument {
        let mut value = document.value().clone();
        value["revision"] = revision.into();
        ProjectDocument::from_value(value).unwrap()
    }
    fn note_completion(
        peer: &omniusong_session::TestPeer,
        id: RequestId,
        document: ProjectDocument,
        replayed: bool,
    ) {
        peer.responses
            .try_send(Completion {
                id,
                kind: CommandKind::Edit,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(document).unwrap(),
                    library: Library::default(),
                    replayed,
                }),
            })
            .unwrap();
    }
    #[test]
    fn accessible_note_save_and_metadata_continuation_keep_one_family_and_bind_revision_at_dispatch()
     {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        let (document, selection) = note_fixture_document();
        let source = document.to_json().unwrap();
        app.install_document(DocumentView::prepare(document.clone()).unwrap());
        app.request(Action::Note(NoteAction::Select(selection.clone())));
        app.notes.draft.as_mut().unwrap().velocity = "0".into();
        app.request(Action::Open(document.id().into()));
        click(&ctx, &mut app, "Cancel");
        assert!(app.pending.is_none());
        assert_eq!(app.notes.draft.as_ref().unwrap().velocity, "0");
        assert!(peer.requests.try_recv().is_err());
        app.request(Action::Note(NoteAction::BeginMetadata));
        click(&ctx, &mut app, "Save and continue");
        let first = peer.requests.try_recv().unwrap();
        let Command::Edit { intent } = first.command else {
            panic!()
        };
        assert_eq!(intent.expected_revision, 0);
        assert_eq!(
            intent.scope,
            EditScope::DocumentNote {
                address: selection.address.clone()
            }
        );
        let EditOperation::UpdateNote { values, timeline } = intent.edit else {
            panic!()
        };
        assert_eq!(values.velocity16, 0);
        let saved = revision(
            &document
                .update_note(&selection.address, values, timeline)
                .unwrap(),
            1,
        );
        note_completion(&peer, first.id, saved.clone(), false);
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.notes.dirty());
        assert_eq!(app.notes.selection, Some(selection.clone()));
        app.draft.name = "Metadata before authored note".into();
        let mut add = selection.clone();
        add.address.note_id = uuid::Uuid::new_v4().to_string();
        app.request(Action::Note(NoteAction::Add(add.clone())));
        click(&ctx, &mut app, "Save and continue");
        let metadata = peer.requests.try_recv().unwrap();
        let Command::Edit { intent } = metadata.command else {
            panic!()
        };
        assert_eq!(intent.expected_revision, 1);
        assert!(matches!(intent.edit, EditOperation::UpdateSession { .. }));
        let mut next = saved.clone();
        next.rename("Metadata before authored note").unwrap();
        note_completion(&peer, metadata.id, revision(&next, 2), false);
        let _ = frame(&ctx, &mut app, vec![]);
        assert_eq!(app.notes.draft.as_ref().unwrap().selection, add);
        assert!(app.notes.draft.as_ref().unwrap().is_new);
        assert!(app.notes.dirty());
        click(&ctx, &mut app, "Discard note");
        assert!(app.notes.draft.is_none());
        assert!(peer.requests.try_recv().is_err());
        assert_eq!(document.to_json().unwrap(), source);
    }
    #[test]
    fn uncertain_note_survives_navigation_and_exact_retry_uses_current_completion_context() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        let (document, selection) = note_fixture_document();
        app.install_document(DocumentView::prepare(document.clone()).unwrap());
        app.request(Action::Note(NoteAction::Select(selection.clone())));
        app.notes.draft.as_mut().unwrap().velocity = "0".into();
        app.dispatch(
            Action::Note(NoteAction::Save),
            Some(Action::Open("cancelled-continuation".into())),
        )
        .unwrap();
        let first = peer.requests.try_recv().unwrap();
        let exact = match &first.command {
            Command::Edit { intent } => intent.clone(),
            _ => panic!(),
        };
        peer.responses
            .try_send(Completion {
                id: first.id,
                kind: CommandKind::Edit,
                result: Err(ServiceError::backend(
                    "Synthetic lost note acknowledgement",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.request(Action::Note(NoteAction::Suspend));
        assert!(!app.dirty());
        assert!(app.retry.as_ref().unwrap().after.is_none());
        let other = ProjectDocument::new("Other synthetic session").unwrap();
        app.request(Action::Open(other.id().into()));
        let open = peer.requests.try_recv().unwrap();
        peer.responses
            .try_send(Completion {
                id: open.id,
                kind: CommandKind::Open,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(other).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert_eq!(app.notes.failure.as_ref().unwrap().intent, exact);
        app.request(Action::Open(document.id().into()));
        let open = peer.requests.try_recv().unwrap();
        let latest = revision(&document, 5);
        peer.responses
            .try_send(Completion {
                id: open.id,
                kind: CommandKind::Open,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(latest.clone()).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(peer.requests.try_recv().is_err());
        app.request(Action::Note(NoteAction::Retry));
        let retry = peer.requests.try_recv().unwrap();
        assert_ne!(first.id, retry.id);
        assert!(matches!(&retry.command,Command::Edit{intent}if *intent==exact));
        assert_eq!(
            app.active.as_ref().unwrap().context,
            Some((document.id().into(), 5))
        );
        note_completion(&peer, retry.id, latest, true);
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.notes.failure.is_none());
        assert_eq!(app.document.as_ref().unwrap().revision(), 5);
        assert!(peer.requests.try_recv().is_err());
    }
    #[test]
    fn known_note_conflict_reviews_latest_directly_then_requires_explicit_new_save() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        let (document, selection) = note_fixture_document();
        app.install_document(DocumentView::prepare(document.clone()).unwrap());
        app.request(Action::Note(NoteAction::Select(selection.clone())));
        app.notes.draft.as_mut().unwrap().pitch = "61".into();
        app.request(Action::Note(NoteAction::Save));
        let first = peer.requests.try_recv().unwrap();
        let old = match first.command {
            Command::Edit { intent } => intent,
            _ => panic!(),
        };
        peer.responses
            .try_send(Completion {
                id: first.id,
                kind: CommandKind::Edit,
                result: Err(ServiceError {
                    kind: FailureKind::RevisionConflict,
                    message: "Synthetic note conflict".into(),
                    outcome_unknown: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.request(Action::Note(NoteAction::ReviewLatest));
        assert!(app.pending.is_none());
        let open = peer.requests.try_recv().unwrap();
        assert!(matches!(open.command, Command::Open { .. }));
        peer.responses
            .try_send(Completion {
                id: open.id,
                kind: CommandKind::Open,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(revision(&document, 4)).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(peer.requests.try_recv().is_err());
        app.request(Action::Note(NoteAction::NewChoice));
        assert!(app.notes.failure.is_none());
        assert_eq!(app.notes.draft.as_ref().unwrap().pitch, "61");
        assert!(peer.requests.try_recv().is_err());
        click(&ctx, &mut app, "Save note");
        let next = peer.requests.try_recv().unwrap();
        let Command::Edit { intent } = next.command else {
            panic!()
        };
        assert_ne!(intent.operation_id, old.operation_id);
        assert_eq!(intent.expected_revision, 4);
    }
    #[test]
    fn busy_readonly_close_requires_explicit_note_discard_and_never_waits_for_worker() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        let (document, selection) = note_fixture_document();
        app.install_document(DocumentView::prepare(document).unwrap());
        app.request(Action::Note(NoteAction::Select(selection)));
        app.notes.draft.as_mut().unwrap().velocity = "invalid".into();
        app.execute(Action::Refresh);
        let _ = peer.requests.try_recv().unwrap();
        assert!(app.busy());
        click(&ctx, &mut app, "Close window");
        assert!(app.pending.is_some());
        assert!(!app.closing);
        click(&ctx, &mut app, "Cancel");
        assert!(app.pending.is_none());
        assert_eq!(app.notes.draft.as_ref().unwrap().velocity, "invalid");
        click(&ctx, &mut app, "Close window");
        click(&ctx, &mut app, "Discard and continue");
        assert!(app.closing);
        assert!(app.busy());
        assert!(peer.requests.try_recv().is_err());
    }

    #[test]
    fn accessible_note_widget_identity_tracks_exact_keys_across_reorder_and_project_copy() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        let (document, selection) = note_fixture_document();
        app.install_document(DocumentView::prepare(document.clone()).unwrap());
        app.request(Action::Note(NoteAction::Select(selection.clone())));
        let _ = frame(&ctx, &mut app, vec![]);
        let tree = frame(&ctx, &mut app, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let row = tree
            .nodes
            .iter()
            .find(|(_, node)| {
                node.label() == Some("Note 2 · pitch 60 · tick 37 · length 960 · velocity 123")
            })
            .expect("visible virtual note row")
            .0;
        let other_track = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Pocket drums"))
            .expect("track selector")
            .0;
        let click_old = |target| {
            egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: tree.tree_id,
                target_node: target,
                data: None,
            })
        };
        let mut value = document.value().clone();
        value["tracks"][0]["notes"]
            .as_array_mut()
            .unwrap()
            .reverse();
        let reordered = ProjectDocument::from_value(value).unwrap();
        app.install_document(DocumentView::prepare(reordered.clone()).unwrap());
        let _ = frame(&ctx, &mut app, vec![click_old(row)]);
        assert_eq!(
            app.notes.selection.as_ref().unwrap().address.note_id,
            "same-pitch",
            "Reordering must keep the old accessible target bound to its exact note key"
        );
        app.notes.draft.as_mut().unwrap().velocity = "0".into();
        let save_tree = frame(&ctx, &mut app, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let old_save = save_tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Save note"))
            .expect("save button")
            .0;
        let mut value = reordered.value().clone();
        value["id"] = uuid::Uuid::new_v4().to_string().into();
        let copy = ProjectDocument::from_value(value).unwrap();
        app.install_document(DocumentView::prepare(copy.clone()).unwrap());
        let _ = frame(&ctx, &mut app, vec![click_old(row), click_old(other_track)]);
        assert!(
            app.notes.selection.is_none(),
            "A copy legitimately reuses track/note keys but cannot accept the original project's accessible target"
        );
        assert_eq!(app.selected_track, 0);
        let mut target = selection;
        target.project_id = copy.id().into();
        app.request(Action::Note(NoteAction::Select(target)));
        app.notes.draft.as_mut().unwrap().velocity = "42".into();
        let _ = frame(
            &ctx,
            &mut app,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: save_tree.tree_id,
                    target_node: old_save,
                    data: None,
                },
            )],
        );
        assert!(
            peer.requests.try_recv().is_err(),
            "An old inspector Save must not submit a new project's draft"
        );
        assert_eq!(app.notes.draft.as_ref().unwrap().velocity, "42");
    }

    #[test]
    fn failed_initialization_stays_visible_and_only_explicit_action_retries() {
        let fixture = Fixture::new();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (mut service, peer) = SessionService::test_pair();
        let first = service.try_submit(Command::Initialize).unwrap();
        let request = peer.requests.try_recv().unwrap();
        let mut app = StudioApp::new(fixture.paths());
        app.service = Some(service);
        app.active = Some(Active {
            id: first,
            command: Command::Initialize,
            action: Action::Refresh,
            context: None,
            draft: Draft::default(),
            note_context: NoteEditor::default().context(),
            clip_context: ClipEditor::default().context(),
            after: None,
        });
        peer.responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::Initialize,
                result: Err(ServiceError::backend("Storage unavailable fixture", false)),
            })
            .unwrap();
        for _ in 0..8 {
            let output = frame(&ctx, &mut app, vec![]);
            assert!(!output.shapes.is_empty());
            assert!(!app.ready);
            assert!(!app.busy());
        }
        assert!(peer.requests.try_recv().is_err());
        assert!(app.error.as_ref().unwrap().contains("Storage unavailable"));
        click(&ctx, &mut app, "Try opening storage again");
        let retry = peer.requests.try_recv().unwrap();
        assert_ne!(retry.id, first);
        assert!(matches!(retry.command, Command::Initialize));
        assert!(app.busy());
        assert!(!fixture.paths().native.exists());
    }

    #[test]
    fn fatal_completion_keeps_error_draft_and_intent_until_explicit_fresh_reconnect_and_retry() {
        let fixture = Fixture::new();
        let (ctx, mut app) = ready(&fixture);
        app.execute(Action::Create);
        settle(&ctx, &mut app);
        let (service, peer) = SessionService::test_pair();
        let old_generation = service.generation();
        app.service = Some(service);
        app.draft.name = "Draft retained through storage quarantine".into();
        app.save().unwrap();
        let request = peer.requests.try_recv().unwrap();
        let operation = match &request.command {
            Command::Edit { intent } => intent.operation_id.clone(),
            _ => panic!(),
        };
        peer.responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::Edit,
                result: Err(ServiceError::storage_unavailable(
                    "Synthetic fatal storage evidence",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.disconnected());
        assert!(!app.busy());
        assert_eq!(app.draft.name, "Draft retained through storage quarantine");
        let known = app.error.clone().unwrap();
        assert!(known.contains("Synthetic fatal storage evidence"));
        assert!(known.contains("may have completed"));
        for _ in 0..5 {
            let _ = frame(&ctx, &mut app, vec![]);
            assert_eq!(app.error.as_deref(), Some(known.as_str()));
        }
        assert!(
            matches!(app.retry.as_ref().map(|active|&active.command),Some(Command::Edit{intent})if intent.operation_id==operation)
        );
        click(&ctx, &mut app, "Reconnect storage");
        settle(&ctx, &mut app);
        assert!(!app.disconnected());
        assert_ne!(app.service.as_ref().unwrap().generation(), old_generation);
        assert_eq!(app.document.as_ref().unwrap().revision(), 0);
        assert_eq!(app.draft.name, "Draft retained through storage quarantine");
        assert!(
            app.retry.is_some(),
            "Reconnection must not automatically resubmit the durable intent"
        );
        click(&ctx, &mut app, "Retry last action");
        assert!(
            matches!(app.active.as_ref().map(|active|&active.command),Some(Command::Edit{intent})if intent.operation_id==operation)
        );
        settle(&ctx, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.document.as_ref().unwrap().revision(), 1);
        assert_eq!(
            app.document.as_ref().unwrap().name(),
            "Draft retained through storage quarantine"
        );
    }

    #[test]
    fn disconnected_before_any_initialization_can_explicitly_retry_deliberate_first_run() {
        let fixture = Fixture::new();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (service, peer) = SessionService::test_pair();
        let mut app = StudioApp::new(fixture.paths());
        let generation = service.generation();
        app.service = Some(service);
        app.execute_initialize();
        let _ = peer.requests.try_recv().unwrap();
        drop(peer);
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.disconnected());
        assert!(!app.reconnect_existing);
        assert!(!fixture.paths().native.exists());
        click(&ctx, &mut app, "Try opening storage again");
        settle(&ctx, &mut app);
        assert!(app.ready);
        assert!(app.reconnect_existing);
        assert_ne!(app.service.as_ref().unwrap().generation(), generation);
        assert!(fixture.paths().native.join("studio.sqlite3").is_file());
    }

    #[test]
    fn operator_recovery_notice_requires_exact_root_evidence_and_retains_opaque_values() {
        let fixture = Fixture::new();
        let (ctx, mut app, _peer) = manual(&fixture);
        for (format, version, complete, expected) in [
            ("omniusong.operator-recovery-evidence", 1, false, true),
            ("future.operator-recovery", 1, false, false),
            ("omniusong.operator-recovery-evidence", 2, false, false),
            ("omniusong.operator-recovery-evidence", 1, true, false),
        ] {
            let mut value = ProjectDocument::new("Recovery disclosure fixture")
                .unwrap()
                .value()
                .clone();
            value["omniusongOperatorRecovery"] = serde_json::json!({"format":format,"version":version,"historicalLedgerComplete":complete,"opaqueEvidence":[null,{"takeId":"foreign"}]});
            value["vendor"] = serde_json::json!({"omniusongOperatorRecovery":{"format":"omniusong.operator-recovery-evidence","version":1,"historicalLedgerComplete":false}});
            let document = ProjectDocument::from_value(value).unwrap();
            let bytes = document.to_json().unwrap();
            app.install_document(DocumentView::prepare(document).unwrap());
            let tree = frame(&ctx, &mut app, vec![])
                .platform_output
                .accesskit_update
                .unwrap();
            let text: Vec<_> = tree
                .nodes
                .iter()
                .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
                .collect();
            assert_eq!(
                text.contains(&"Recovered snapshot · earlier edit history is incomplete"),
                expected
            );
            assert_eq!(text.contains(&"This session starts new edit history. The source checkpoint and recovery evidence are retained in the project."),expected);
            assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), bytes);
            assert!(
                app.document
                    .as_ref()
                    .unwrap()
                    .referenced_takes()
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn completed_ordinary_inspection_error_clears_background_work_status() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.restore_path = "/synthetic/malformed-backup".into();
        app.execute(Action::Inspect(PathBuf::from(&app.restore_path)));
        let request = peer.requests.try_recv().unwrap();
        peer.responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::InspectArchive,
                result: Err(ServiceError::backend("Synthetic invalid archive", false)),
            })
            .unwrap();
        let tree = frame(&ctx, &mut app, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        assert!(!app.busy());
        assert!(!app.disconnected());
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        assert!(text.contains(&"Synthetic invalid archive"));
        assert!(text.contains(
            &"Action finished with an error. Your saved session and local draft were kept."
        ));
        assert!(
            !text
                .iter()
                .any(|text| text.starts_with("Work is running in the background"))
        );
    }

    #[test]
    fn recovery_warning_meets_text_contrast_on_both_default_panel_themes() {
        fn luminance(color: egui::Color32) -> f64 {
            let linear = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        }
        for (dark, visuals) in [
            (true, egui::Visuals::dark()),
            (false, egui::Visuals::light()),
        ] {
            let fg = luminance(recovery_warning_color(dark));
            let bg = luminance(visuals.panel_fill);
            let contrast = (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05);
            assert!(
                contrast >= 4.5,
                "Theme dark={dark} recovery warning contrast {contrast}"
            );
        }
    }

    #[test]
    fn accessible_capture_guards_edits_navigation_and_close_but_stop_stays_queued() {
        use audio_ui::test_support;
        use omniusong_engine::{EngineCommand, EngineResult};
        let fixture = Fixture::new();
        let (ctx, mut app, store_peer) = manual(&fixture);
        let (audio, peer) = test_support::pair();
        app.audio = audio;
        click(&ctx, &mut app, "Advanced · solo input capture");
        click(&ctx, &mut app, "Record solo");
        assert!(matches!(
            peer.command(),
            Some(EngineCommand::StartCapture { input_device: None })
        ));
        assert!(app.audio.capture_guard());
        let original = app.document.as_ref().unwrap().to_json().unwrap();
        app.request(Action::Open("another-project".into()));
        assert!(store_peer.requests.try_recv().is_err());
        app.request(Action::Recovery(RecoveryAction::Scan));
        assert!(store_peer.requests.try_recv().is_err());
        assert!(app.save().is_err());
        app.request(Action::Export(PathBuf::from("/synthetic/new-backup")));
        assert!(store_peer.requests.try_recv().is_err());
        app.request(Action::Close);
        assert!(app.audio_close_requested);
        assert!(!app.closing);
        // Stop is admitted locally while the starting input still owns the one credit.
        app.audio.stop();
        assert!(peer.command().is_none());
        peer.reply(Ok(EngineResult::Capturing(test_support::status(
            "capturing",
        ))));
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(matches!(peer.command(), Some(EngineCommand::Stop)));
        peer.reply(Ok(EngineResult::Stopped));
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.audio.blocks_session());
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
        assert!(store_peer.requests.try_recv().is_err());
        click(&ctx, &mut app, "Keep window open");
        assert!(!app.closing);
    }

    #[test]
    fn sealed_capture_publication_failure_keeps_original_and_same_retry_intent() {
        use audio_ui::test_support;
        use omniusong_engine::{EngineCommand, EngineResult};
        let fixture = Fixture::new();
        let (ctx, mut app, store_peer) = manual(&fixture);
        let original_id = app.document.as_ref().unwrap().id().to_owned();
        let (audio, peer) = test_support::pair();
        app.audio = audio;
        click(&ctx, &mut app, "Advanced · solo input capture");
        click(&ctx, &mut app, "Record solo");
        let _ = peer.command();
        peer.reply(Ok(EngineResult::Capturing(test_support::status(
            "capturing",
        ))));
        let _ = frame(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, "Finish take");
        assert!(matches!(peer.command(), Some(EngineCommand::FinishCapture)));
        peer.reply(Ok(EngineResult::Captured(test_support::take())));
        let _ = frame(&ctx, &mut app, vec![]);
        let request = store_peer.requests.try_recv().unwrap();
        let (journal_path, publication) = match request.command {
            Command::AttachCapture {
                journal_path,
                publication,
            } => (journal_path, publication),
            _ => panic!("Capture must attach through the storage worker"),
        };
        assert_eq!(publication.project_id.0, original_id);
        assert_eq!(publication.expected_revision, 0);
        store_peer
            .responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::AttachCapture,
                result: Err(ServiceError::backend(
                    "Synthetic lost attachment acknowledgement",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.busy());
        assert!(app.audio.capture_guard());
        assert!(app.audio.blocks_session());
        assert_eq!(app.document.as_ref().unwrap().revision(), 0);
        click(&ctx, &mut app, "Retry attach sealed take");
        let retry = store_peer.requests.try_recv().unwrap();
        assert!(
            matches!(&retry.command,Command::AttachCapture{journal_path:path,publication:intent}if path==&journal_path&&intent==&publication)
        );
        assert_ne!(request.id, retry.id);
        let mut document = app.document.as_ref().unwrap().upgrade_native().unwrap();
        document
            .append_take_clip(
                &publication.track_id.0,
                &publication.track_name,
                publication.clip.clone(),
            )
            .unwrap();
        let mut value = document.value().clone();
        value["revision"] = 1.into();
        let document = ProjectDocument::from_value(value).unwrap();
        store_peer
            .responses
            .try_send(Completion {
                id: retry.id,
                kind: CommandKind::AttachCapture,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(document).unwrap(),
                    library: Library::default(),
                    replayed: true,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.audio.blocks_session());
        assert!(!app.audio.capture_guard());
        assert_eq!(app.document.as_ref().unwrap().revision(), 1);
        assert_eq!(
            app.document.as_ref().unwrap().take_clips().unwrap(),
            vec![publication.clip]
        );
        assert!(peer.command().is_none());
    }

    #[test]
    fn dirty_draft_never_starts_audio_and_close_with_held_capture_stays_explicit() {
        use audio_ui::test_support;
        use omniusong_engine::EngineCommand;
        let fixture = Fixture::new();
        let (ctx, mut app, _store_peer) = manual(&fixture);
        let (audio, peer) = test_support::pair();
        app.audio = audio;
        click(&ctx, &mut app, "Advanced · solo input capture");
        app.draft.name = "Unsaved local name".into();
        click(&ctx, &mut app, "Play saved session");
        click(&ctx, &mut app, "Record solo");
        click(&ctx, &mut app, "Start live audition");
        assert!(peer.command().is_none());
        app.draft = Draft::from_document(app.document.as_ref().unwrap());
        click(&ctx, &mut app, "Record solo");
        assert!(matches!(
            peer.command(),
            Some(EngineCommand::StartCapture { .. })
        ));
        app.request(Action::Close);
        assert!(!app.closing);
        assert!(app.audio_close_requested);
        click(&ctx, &mut app, "Close without waiting");
        assert!(app.closing);
        let before = Instant::now();
        drop(app);
        assert!(before.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn live_output_blocks_project_edits_and_has_no_capture_or_storage_side_effects() {
        use audio_ui::test_support;
        use omniusong_engine::{
            EngineCommand, EngineResult, EngineUpdate, LiveId, StopAck, UrgentProgress,
        };
        let fixture = Fixture::new();
        let (ctx, mut app, store_peer) = manual(&fixture);
        let (audio, peer) = test_support::pair();
        app.audio = audio;
        let original = app.document.as_ref().unwrap().to_json().unwrap();
        let draft = app.draft.clone();
        click(&ctx, &mut app, "Start live audition");
        let Some(EngineCommand::StartLive { gui_clock, .. }) = peer.command() else {
            panic!("Explicit live start")
        };
        peer.reply(Ok(EngineResult::LiveStarted(
            crate::live_ui::test_support::ready(gui_clock),
        )));
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.audio.live_guard());
        assert!(!app.audio.capture_guard());
        app.request(Action::Refresh);
        app.request(Action::Save);
        assert!(store_peer.requests.try_recv().is_err());
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
        assert!(app.draft == draft);
        app.request(Action::Close);
        let _ = frame(&ctx, &mut app, vec![]);
        let output = frame(&ctx, &mut app, vec![]);
        let tree = output.platform_output.accesskit_update.unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, n)| [n.label(), n.value()].into_iter().flatten())
            .collect();
        assert!(
            text.iter()
                .any(|t| t.contains("This performance is not recorded or saved"))
        );
        assert!(!text.iter().any(|t|t.contains("Closing without waiting can interrupt an unfinished original")));
        app.audio.stop();
        let mut stop = None;
        while let Some((ticket, scope)) = peer.urgent() {
            if scope.is_none() {
                stop = Some(ticket)
            }
        }
        let ticket = stop.expect("Urgent output Stop");
        peer.update(EngineUpdate {
            worker_generation: LiveId([1; 16]),
            revision: 1,
            live: None,
            urgent: UrgentProgress {
                stop: Some(StopAck {
                    ticket,
                    cancelled_through_control: 1,
                }),
                panic: None,
            },
            first_fault: None,
        });
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.audio.blocks_session());
        assert!(store_peer.requests.try_recv().is_err());
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
        assert!(app.draft == draft);
    }

    #[test]
    fn retained_scan_inspection_and_explicit_recovery_keep_music_and_retry_identity() {
        use recovery_ui::test_support as retained;
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        let original = app.document.as_ref().unwrap().to_json().unwrap();
        app.draft.name = "Unsaved draft survives read-only inspection".into();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(
            peer.requests.try_recv().is_err(),
            "Opening the panel must not automatically scan"
        );
        click(&ctx, &mut app, "Scan retained captures");
        let scan = peer.requests.try_recv().unwrap();
        assert!(matches!(scan.command, Command::ScanCaptures));
        assert!(app.pending.is_none());
        peer.responses
            .try_send(Completion {
                id: scan.id,
                kind: CommandKind::ScanCaptures,
                result: Ok(Reply::CaptureStages(retained::scan())),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, "Inspect selected capture");
        let inspect = peer.requests.try_recv().unwrap();
        assert!(
            matches!(inspect.command,Command::InspectCapture {stage_id}if stage_id==retained::STAGE)
        );
        peer.responses
            .try_send(Completion {
                id: inspect.id,
                kind: CommandKind::InspectCapture,
                result: Ok(Reply::CaptureInspected(retained::inspection(false))),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert_eq!(
            app.draft.name,
            "Unsaved draft survives read-only inspection"
        );
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
        assert!(peer.requests.try_recv().is_err());
        let action = retained::recover(&app.recovery);
        let operation = retained::operation(&action);
        app.request(Action::Recovery(action.clone()));
        assert!(app.pending.is_some());
        assert!(peer.requests.try_recv().is_err());
        click(&ctx, &mut app, "Discard and continue");
        let first = peer.requests.try_recv().unwrap();
        assert!(
            matches!(&first.command,Command::RecoverCapture {operation_id,..}if operation_id.0==operation)
        );
        peer.responses
            .try_send(Completion {
                id: first.id,
                kind: CommandKind::RecoverCapture,
                result: Err(ServiceError::backend(
                    "Synthetic recovery acknowledgement lost",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.retry_last();
        let retry = peer.requests.try_recv().unwrap();
        assert_ne!(first.id, retry.id);
        assert!(
            matches!(&retry.command,Command::RecoverCapture {inspection,operation_id}if operation_id.0==operation&&inspection.inspection_sha256=="b".repeat(64))
        );
        peer.responses
            .try_send(Completion {
                id: retry.id,
                kind: CommandKind::RecoverCapture,
                result: Ok(Reply::CaptureRecovered(retained::receipt(&action))),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
        assert!(!app.recovery.blocks_session());
        assert!(
            peer.requests.try_recv().is_err(),
            "A recovered receipt must not automatically attach or switch projects"
        );
    }

    #[test]
    fn retained_attachment_saves_first_then_keeps_bound_intent_through_fatal_reconnect() {
        use recovery_ui::test_support as retained;
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.recovery.scanned(retained::scan());
        app.recovery.inspected(retained::inspection(true)).unwrap();
        let action = retained::attach(&app.recovery, app.document.as_ref().unwrap().id());
        let operation = retained::operation(&action);
        app.draft.name = "Saved before retained attachment".into();
        app.request(Action::Recovery(action));
        assert!(app.pending.is_some());
        assert!(peer.requests.try_recv().is_err());
        click(&ctx, &mut app, "Save and continue");
        let save = peer.requests.try_recv().unwrap();
        assert!(matches!(save.command, Command::Edit { .. }));
        let mut value = app.document.as_ref().unwrap().value().clone();
        value["name"] = "Saved before retained attachment".into();
        value["revision"] = 1.into();
        let saved = ProjectDocument::from_value(value).unwrap();
        peer.responses
            .try_send(Completion {
                id: save.id,
                kind: CommandKind::Edit,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(saved).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        let attach = peer.requests.try_recv().unwrap();
        let Command::AttachCapture { publication, .. } = &attach.command else {
            panic!()
        };
        let publication = publication.clone();
        assert_eq!(publication.operation_id.0, operation);
        assert_eq!(publication.expected_revision, 1);
        assert_eq!(
            publication.clip.range,
            TakeRange::AudioFrames {
                start: 100,
                count: 4
            }
        );
        assert_eq!(publication.clip.position.frame, 0);
        peer.responses
            .try_send(Completion {
                id: attach.id,
                kind: CommandKind::AttachCapture,
                result: Err(ServiceError::storage_unavailable(
                    "Synthetic fatal attachment acknowledgement",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.disconnected());
        assert!(app.recovery.blocks_session());
        assert_eq!(app.document.as_ref().unwrap().revision(), 1);
        app.request(Action::Close);
        assert!(app.recovery_close_requested);
        assert!(!app.closing);
        click(&ctx, &mut app, "Keep recovery window open");
        let (service, fresh) = SessionService::test_pair();
        app.service = Some(service);
        app.execute_initialize();
        let initialize = fresh.requests.try_recv().unwrap();
        assert!(matches!(initialize.command, Command::Initialize));
        fresh
            .responses
            .try_send(Completion {
                id: initialize.id,
                kind: CommandKind::Initialize,
                result: Ok(Reply::Library(Library::default())),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(
            fresh.requests.try_recv().is_err(),
            "Initialization must not replay recovery or attachment"
        );
        app.request(Action::Recovery(retained::retry_attach(&app.recovery)));
        let retry = fresh.requests.try_recv().unwrap();
        assert!(
            matches!(&retry.command,Command::AttachCapture {publication:retried,..}if retried==&publication)
        );
        let mut document = app.document.as_ref().unwrap().upgrade_native().unwrap();
        document
            .append_take_clip(
                &publication.track_id.0,
                &publication.track_name,
                publication.clip.clone(),
            )
            .unwrap();
        let mut value = document.value().clone();
        value["revision"] = 2.into();
        let document = ProjectDocument::from_value(value).unwrap();
        fresh
            .responses
            .try_send(Completion {
                id: retry.id,
                kind: CommandKind::AttachCapture,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(document).unwrap(),
                    library: Library::default(),
                    replayed: true,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.recovery.blocks_session());
        assert_eq!(app.document.as_ref().unwrap().revision(), 2);
        assert_eq!(
            app.document.as_ref().unwrap().take_clips().unwrap(),
            vec![publication.clip]
        );
    }

    #[test]
    fn keep_local_releases_workflow_and_only_typed_conflict_allows_a_new_attachment_choice() {
        use recovery_ui::test_support as retained;
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.recovery.scanned(retained::scan());
        app.recovery.inspected(retained::inspection(true)).unwrap();
        let target = app.document.as_ref().unwrap().id().to_owned();
        app.request(Action::Recovery(retained::attach(&app.recovery, &target)));
        let first = peer.requests.try_recv().unwrap();
        let Command::AttachCapture { publication, .. } = &first.command else {
            panic!()
        };
        let original = publication.clone();
        peer.responses
            .try_send(Completion {
                id: first.id,
                kind: CommandKind::AttachCapture,
                result: Err(ServiceError::backend(
                    "Revision conflict words alone are not a known outcome",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.recovery.blocks_session());
        assert!(!retained::release_rejected(&mut app.recovery));
        assert!(retained::keep_local(&mut app.recovery));
        assert!(!app.recovery.blocks_session());
        let (service, fresh) = SessionService::test_pair();
        app.service = Some(service);
        app.execute_initialize();
        let initialize = fresh.requests.try_recv().unwrap();
        assert_ne!(first.id.generation, initialize.id.generation);
        fresh
            .responses
            .try_send(Completion {
                id: initialize.id,
                kind: CommandKind::Initialize,
                result: Ok(Reply::Library(Library::default())),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(
            fresh.requests.try_recv().is_err(),
            "Reconnect must not replay the suspended request"
        );
        app.request(Action::Refresh);
        let refresh = fresh.requests.try_recv().unwrap();
        assert!(matches!(refresh.command, Command::Refresh));
        fresh
            .responses
            .try_send(Completion {
                id: refresh.id,
                kind: CommandKind::Refresh,
                result: Ok(Reply::Library(Library::default())),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.request(Action::Open(target.clone()));
        let open = fresh.requests.try_recv().unwrap();
        assert!(matches!(open.command, Command::Open { .. }));
        let mut value = app.document.as_ref().unwrap().value().clone();
        value["name"] = "Latest external revision".into();
        value["revision"] = 1.into();
        let current = ProjectDocument::from_value(value).unwrap();
        fresh
            .responses
            .try_send(Completion {
                id: open.id,
                kind: CommandKind::Open,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(current).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.draft.name = "Work continues after keep-local".into();
        app.save().unwrap();
        let save = fresh.requests.try_recv().unwrap();
        assert!(matches!(&save.command,Command::Edit {intent}if intent.expected_revision==1));
        let mut value = app.document.as_ref().unwrap().value().clone();
        value["name"] = "Work continues after keep-local".into();
        value["revision"] = 2.into();
        fresh
            .responses
            .try_send(Completion {
                id: save.id,
                kind: CommandKind::Edit,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(ProjectDocument::from_value(value).unwrap())
                        .unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.request(Action::Recovery(retained::retry_attach(&app.recovery)));
        let retry = fresh.requests.try_recv().unwrap();
        assert!(
            matches!(&retry.command,Command::AttachCapture {publication,..}if publication==&original)
        );
        assert_eq!(
            original.expected_revision, 0,
            "Continuing to work must not rebase the suspended publication"
        );
        fresh
            .responses
            .try_send(Completion {
                id: retry.id,
                kind: CommandKind::AttachCapture,
                result: Err(ServiceError {
                    kind: FailureKind::RevisionConflict,
                    message: "Synthetic proven project conflict".into(),
                    outcome_unknown: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(retained::release_rejected(&mut app.recovery));
        assert!(!app.recovery.blocks_session());
        assert!(
            fresh.requests.try_recv().is_err(),
            "Release is a local decision, never a new publication"
        );
        app.request(Action::Recovery(retained::attach(&app.recovery, &target)));
        let next = fresh.requests.try_recv().unwrap();
        let Command::AttachCapture { publication, .. } = next.command else {
            panic!()
        };
        assert_eq!(publication.expected_revision, 2);
        assert_ne!(publication.operation_id, original.operation_id);
        assert_ne!(publication.track_id, original.track_id);
    }
}
