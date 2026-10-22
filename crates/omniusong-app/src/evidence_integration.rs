//! Native admission only. Presentation owns no filesystem, worker or engine.
use super::*;
use evidence_ui::{EvidenceAction, EvidenceAvailability};
use omniusong_session::ServiceError;

pub(super) fn evidence_command(command: &Command) -> bool {
    matches!(
        command,
        Command::RunEvidenceJob { .. }
            | Command::InspectRetainedEvidence { .. }
            | Command::ListEvidenceJobs
            | Command::ReadEvidenceJob { .. }
            | Command::ReconcileEvidenceJob { .. }
    )
}
pub(super) fn completion_matches(
    request: &EvidenceUiRequest,
    result: &Result<Reply, ServiceError>,
) -> bool {
    let Ok(reply) = result else {
        return true;
    };
    match (request.command(), reply) {
        (Command::RunEvidenceJob { request }, Reply::EvidenceJobOutcome(result)) => {
            result.job_id == request.job_id
                && request
                    .fingerprint()
                    .is_ok_and(|sha| sha == result.request_sha256)
        }
        (
            Command::ReconcileEvidenceJob {
                job_id,
                expected_request_sha256,
                ..
            },
            Reply::EvidenceJobOutcome(result),
        ) => {
            result.job_id == job_id
                && result.request_sha256 == expected_request_sha256
                && result.catalog_retirement.is_none()
        }
        (Command::ReadEvidenceJob { job_id }, Reply::EvidenceJob(view)) => view.job_id == job_id,
        (Command::ListEvidenceJobs, Reply::EvidenceJobs(_)) => true,
        (
            Command::InspectRetainedEvidence { path },
            Reply::RetainedEvidenceInspected(inspection),
        ) => inspection.receipt.path.as_os_str() == path.as_os_str(),
        _ => false,
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct EvidenceContext {
    document: Option<(String, u64)>,
    metadata: Draft,
    note: NoteContext,
    clip: ClipContext,
    track: usize,
    origin: Option<PathBuf>,
    ready: bool,
    barriers: [bool; 10],
}
impl StudioApp {
    pub(super) fn current_evidence_context(&self) -> EvidenceContext {
        EvidenceContext {
            document: self.context(),
            metadata: self.draft.clone(),
            note: self.notes.context(),
            clip: self.clips.context(),
            track: self.selected_track,
            origin: self.native_origin.clone(),
            ready: self.ready,
            barriers: [
                self.audio.blocks_session(),
                self.recovery.blocks_session(),
                self.pending.is_some(),
                self.retry.is_some(),
                self.musical_unresolved(),
                self.closing,
                self.audio_close_requested,
                self.recovery_close_requested,
                self.note_close_requested,
                self.clip_close_requested,
            ],
        }
    }
    pub(super) fn sync_evidence_context(&mut self) {
        let context = self.current_evidence_context();
        if self.evidence_context.as_ref() != Some(&context) {
            self.evidence.context_changed();
            self.evidence_context = Some(context);
        }
    }
    pub(super) fn evidence_mutation_allowed(&self) -> bool {
        self.ready
            && self.native_origin.is_some()
            && self.service.is_some()
            && !self.busy()
            && !self.disconnected()
            && self.active.is_none()
            && self.dirty_family().is_ok_and(|family| family.is_none())
            && !self.musical_unresolved()
            && self.retry.is_none()
            && self.pending.is_none()
            && !self.audio.blocks_session()
            && !self.recovery.blocks_session()
            && !self.closing
            && !self.audio_close_requested
            && !self.recovery_close_requested
            && !self.note_close_requested
            && !self.clip_close_requested
    }
    pub(super) fn evidence_availability(&self) -> EvidenceAvailability {
        let can_submit = self.service.is_some()
            && !self.busy()
            && !self.disconnected()
            && self.active.is_none()
            && !self.closing;
        EvidenceAvailability {
            can_submit,
            can_read_artifacts: can_submit && !self.audio.blocks_session(),
            can_run: self.evidence_mutation_allowed(),
            generation: self.service.as_ref().map(SessionService::generation),
            origin: self.native_origin.clone(),
            unavailable_reason: if self.closing {
                Some("The window is closing.".into())
            } else if self.busy() {
                Some("The session worker is busy. Exact cancellation remains available.".into())
            } else if self.service.is_none() || self.disconnected() {
                Some("Connect evidence history explicitly to read saved jobs.".into())
            } else if self.audio.blocks_session() {
                Some("Finish or stop audio and resolve retained recordings before reading artifacts or starting a new job. Job metadata remains available.".into())
            } else if !self.evidence_mutation_allowed() {
                Some("New exports and restores require an open studio, resolved drafts and choices, and released audio. History reads keep your drafts.".into())
            } else {
                None
            },
        }
    }
    pub(super) fn validate_evidence_submission(
        &self,
        command: &Command,
        action: &Action,
        after: Option<&Action>,
    ) -> AppResult<()> {
        let Action::Evidence(request) = action else {
            return Err("Studio-evidence commands remain backend-only outside their exact explicit review route".into());
        };
        let availability = self.evidence_availability();
        let generation = availability
            .generation
            .ok_or("Connect the evidence history worker first")?;
        if after.is_some()
            || !request.matches_command(command)
            || !self.evidence.authorizes(request, generation)
        {
            return Err("Evidence requires its exact current review and command without an action continuation".into());
        }
        if !availability.can_submit {
            return Err(
                "Evidence storage is unavailable or busy; choose the action explicitly when ready"
                    .into(),
            );
        }
        if request.is_run()
            && (!availability.can_run
                || self.evidence_context.as_ref() != Some(&self.current_evidence_context()))
        {
            return Err("Resolve drafts, retained choices and audio before reviewing a new export or restore".into());
        }
        if request.reads_artifact() && !availability.can_read_artifacts {
            return Err(
                "Finish or stop audio and resolve retained recordings before inspecting artifacts"
                    .into(),
            );
        }
        command.validate()?;
        Ok(())
    }
    pub(super) fn handle_evidence_action(&mut self, action: EvidenceAction) {
        match action {
            EvidenceAction::Submit(request) => self.request(Action::Evidence(request)),
            EvidenceAction::Cancel(id) => {
                let exact = self.evidence.pending_cancel() == Some(id)
                    && self.active.as_ref().is_some_and(|active| {
                        active.id == id
                            && matches!(active.action, Action::Evidence(_))
                            && matches!(
                                active.command,
                                Command::RunEvidenceJob { .. }
                                    | Command::InspectRetainedEvidence { .. }
                                    | Command::ReconcileEvidenceJob { .. }
                            )
                    });
                let accepted = exact
                    && self
                        .service
                        .as_ref()
                        .is_some_and(|service| service.cancel_evidence_job(id));
                self.evidence.cancel_requested(id, accepted);
            }
            EvidenceAction::MetadataConnect => {
                if let Err(error) = self.connect_evidence_history() {
                    self.error = Some(error.to_string());
                }
            }
        }
    }
    pub(super) fn connect_evidence_history(&mut self) -> AppResult<()> {
        // A fresh service owns this same sole slot; it does not initialize a
        // catalog, inspect an artifact or replay a job. Keep every musical draft.
        if self.busy()
            || self.active.is_some()
            || self.closing
            || (self.service.is_some() && !self.disconnected())
        {
            return Err(
                "Finish the current request before explicitly reconnecting evidence history".into(),
            );
        }
        let origin = &self.paths.native;
        if !origin.is_absolute()
            || origin
                .to_str()
                .is_none_or(|text| text.len() > 4096 || text.chars().any(char::is_control))
            || origin.components().collect::<PathBuf>().as_os_str() != origin.as_os_str()
            || origin
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("Evidence history needs the exact original absolute studio folder. Reopen with that canonical --data-dir path; no replacement folder was created.".into());
        }
        self.service = None;
        self.start_service_mode(true, false);
        if self.service.is_none() {
            return Err("Could not connect evidence history".into());
        }
        self.reconnect_existing = true;
        self.error = None;
        self.status = "Evidence history connected. Choose List jobs; no studio catalog or artifact was opened.".into();
        Ok(())
    }
    pub(super) fn complete_evidence(
        &mut self,
        request: &EvidenceUiRequest,
        id: RequestId,
        result: Result<&Reply, &ServiceError>,
    ) {
        self.sync_evidence_context();
        self.evidence.completed(request, id, result);
        let retirement = match result {
            Err(error) if error.kind == FailureKind::StorageUnavailable => Some(error),
            Ok(Reply::EvidenceJobOutcome(reply)) => reply.catalog_retirement.as_ref(),
            _ => None,
        };
        if let Some(error) = retirement {
            self.ready = false;
            self.reconnect_existing = true;
            self.evidence.worker_lost(id.generation, &error.message);
            self.source_recovery
                .worker_lost(id.generation, &error.message);
            self.performance_sources
                .worker_lost(id.generation, &error.message);
            self.status = "Evidence result retained. Storage worker stopped; reconnect explicitly before another action.".into();
            self.error = Some(error.message.chars().take(512).collect());
        } else {
            self.status = "Evidence action finished. Review its result or retained uncertainty in Studio evidence; musical drafts were kept.".into();
            self.error = result
                .err()
                .map(|error| error.message.chars().take(512).collect());
        }
    }
    pub(super) fn show_evidence_window(&mut self, ctx: &egui::Context) {
        self.sync_evidence_context();
        if !self.evidence.open {
            return;
        }
        let availability = self.evidence_availability();
        let mut open = self.evidence.open;
        let mut action = None;
        egui::Window::new("Studio evidence")
            .id(egui::Id::unique("studio-evidence-window"))
            .open(&mut open).default_width(680.0).default_height(620.0)
            .min_width(340.0)
            .show(ctx, |ui| {
                if self.evidence_metadata_only {
                    ui.small("History uses the exact configured studio location. An alias or a new location does not identify the original journal.");
                    if self.paths.native.as_os_str().len() <= 4096 { ui.label(self.paths.native.display().to_string()); }
                    else { ui.colored_label(egui::Color32::LIGHT_RED, "Configured studio location exceeds the path limit."); }
                }
                action = self.evidence.ui(ui, &availability);
            });
        self.evidence.open = open;
        if let Some(action) = action {
            self.handle_evidence_action(action);
        }
    }
}
