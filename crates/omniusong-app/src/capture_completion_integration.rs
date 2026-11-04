//! Compact recording commitment followed by a separately guarded current read.
use super::*;
use capture_completion_ui::CaptureCompletionDecision;
use omniusong_session::{CaptureAttachmentRequest, ServiceError};

#[derive(Clone)]
pub(super) struct CaptureViewRequest {
    pub request_sha256: String,
    pub origin_root: PathBuf,
    pub worker_generation: u64,
    pub project_id: ProjectId,
}

impl StudioApp {
    pub(super) fn admit_capture_submission(
        &mut self,
        command: &Command,
        action: &Action,
        after: Option<&Action>,
    ) -> AppResult<()> {
        let Command::AttachCapture { request } = command else {
            return Err("Choose the exact recording attachment".into());
        };
        request.validate()?;
        let sha = request.fingerprint()?;
        let action_matches = match action {
            Action::AttachCapture { request: expected } => expected.fingerprint()? == sha,
            Action::Recovery(recovery @ RecoveryAction::Attach { .. }) => {
                matches!(recovery.command()?, Command::AttachCapture { request: expected }
                    if expected.fingerprint().is_ok_and(|value| value == sha))
            }
            _ => false,
        };
        if !action_matches || after.is_some() {
            return Err("Recording attachment requires its exact retained choice without a follow-up action".into());
        }
        if !self.ready || self.busy() || self.disconnected() || self.service.is_none()
            || self.closing || self.dirty() || self.musical_unresolved()
            || self.native_origin.as_ref().is_none_or(|origin|
                origin.as_os_str() != request.origin_root.as_os_str())
            || self.document.as_ref().map(ProjectDocument::id)
                != Some(request.publication.project_id.0.as_str())
        {
            return Err("Open the original studio and target session, then resolve local edits before attaching this exact recording".into());
        }
        self.capture_completion.begin(request.as_ref().clone())?;
        Ok(())
    }

    fn capture_context_matches(&self, active: &Active, view: &CaptureViewRequest) -> bool {
        self.ready && !self.closing && !self.disconnected()
            && self.service.as_ref().is_some_and(|service|
                service.generation() == view.worker_generation)
            && active.id.generation == view.worker_generation
            && self.native_origin.as_ref().is_some_and(|origin|
                origin.as_os_str() == view.origin_root.as_os_str())
            && self.document.as_ref().map(ProjectDocument::id) == Some(view.project_id.0.as_str())
            && self.context() == active.context && self.draft == active.draft
            && self.notes.context() == active.note_context && self.clips.context() == active.clip_context
            && self.capture_completion.request_sha256() == Some(view.request_sha256.as_str())
    }

    fn retire_capture_worker(&mut self, generation: u64, error: &ServiceError) {
        if error.kind != FailureKind::StorageUnavailable {
            return;
        }
        if self.service.as_ref().is_none_or(|service| service.generation() != generation) {
            return;
        }
        self.reconnect_existing = true;
        self.performance_sources.worker_lost(generation, &error.message);
        self.evidence.worker_lost(generation, &error.message);
        self.source_recovery.worker_lost(generation, &error.message);
        // The service itself retires on its independently validated fatal core.
        // Never replace a new generation because an old reply carried a fault.
    }

    pub(super) fn complete_capture(&mut self, active: Active, result: Result<Reply, ServiceError>) {
        let Command::AttachCapture { request } = &active.command else { return; };
        let mut retirement = None;
        let decision = match &result {
            Ok(Reply::CaptureAttachment(reply)) => {
                retirement = reply.retirement_error();
                self.capture_completion.completed(request, reply).map_err(|message| ServiceError {
                    kind: FailureKind::Protocol, message, outcome_unknown: true,
                })
            }
            Err(error) => Err(error.clone()),
            Ok(_) => Err(ServiceError {kind: FailureKind::Protocol,
                message: "The recording response did not contain its save receipt".into(), outcome_unknown: true}),
        };
        let confirmed = decision == Ok(CaptureCompletionDecision::Committed);
        if confirmed {
            // Commit fact first. Neither a view failure nor changed draft may
            // put this publication back into the retry slot.
            if matches!(active.action, Action::AttachCapture { .. }) {
                self.audio.publication_succeeded();
            }
            if matches!(active.action, Action::Recovery(RecoveryAction::Attach { .. })) {
                self.recovery.attached();
            }
            if self.retry.as_ref().is_some_and(|retry| matches!(&retry.command,
                Command::AttachCapture { request: previous } if previous.fingerprint().ok() == request.fingerprint().ok())) {
                self.retry = None;
            }
            let view = CaptureViewRequest {
                request_sha256: self.capture_completion.request_sha256().expect("confirmed request").to_owned(),
                origin_root: request.origin_root.clone(),
                worker_generation: active.id.generation,
                project_id: request.publication.project_id.clone(),
            };
            self.status = "Recording saved. Opening the current session separately.".into();
            self.error = None;
            if let Some(error) = retirement {
                self.retire_capture_worker(active.id.generation, &error);
                self.capture_completion.view_failed(&view.request_sha256, &error.message);
                self.error = Some(error.message);
            } else if !self.capture_context_matches(&active, &view) {
                self.capture_completion.view_context_changed(&view.request_sha256);
                self.status = "Recording saved. Your newer editor work was kept.".into();
            } else {
                let sha = view.request_sha256.clone();
                let command = Command::Open {project_id: view.project_id.clone()};
                match self.submit(command, Action::CaptureView(view), None) {
                    Ok(()) => { self.capture_completion.view_loading(&sha); }
                    Err(error) => {
                        self.capture_completion.view_failed(&sha, error.to_string());
                        self.error = Some(format!("Recording saved; editor could not refresh: {error}"));
                    }
                }
            }
            return;
        }
        let error = match decision {
            Err(error) => {
                let _ = self.capture_completion.transport_failed(request, &error);
                error
            }
            Ok(_) => match &result {
                Ok(Reply::CaptureAttachment(reply)) => reply.failure_error().expect("unconfirmed outcome"),
                _ => unreachable!("typed unconfirmed capture reply"),
            },
        };
        if matches!(active.action, Action::AttachCapture { .. }) { self.audio.publication_failed(); }
        if let Action::Recovery(recovery) = &active.action {
            self.recovery.failed(recovery);
            if error.kind == FailureKind::RevisionConflict && !error.outcome_unknown
                && !self.capture_completion.uncertain() {
                self.recovery.revision_conflict(recovery);
            }
        }
        if let Some(fatal) = retirement.as_ref().or_else(||
            (error.kind == FailureKind::StorageUnavailable).then_some(&error)) {
            self.retire_capture_worker(active.id.generation, fatal);
        }
        self.status = if self.capture_completion.uncertain() {
            "Recording save remains unconfirmed. The exact request and original were kept."
        } else { "Recording was not added. The original and exact request were kept." }.into();
        self.error = Some(error.message);
        self.retry = Some(active);
    }

    pub(super) fn complete_capture_view(&mut self, active: Active, result: Result<Reply, ServiceError>) {
        let Action::CaptureView(view) = &active.action else { return; };
        if let Err(error) = &result {
            self.retire_capture_worker(active.id.generation, error);
            self.capture_completion.view_failed(&view.request_sha256, &error.message);
            self.status = "Recording saved; the editor could not refresh. Your current work was kept.".into();
            self.error = Some(error.message.clone());
            return;
        }
        if !self.capture_context_matches(&active, view) {
            self.capture_completion.view_context_changed(&view.request_sha256);
            self.status = "Recording saved. Your newer editor work was kept.".into();
            return;
        }
        match result {
            Ok(Reply::Document { view: current, library, .. })
                if current.document.id() == view.project_id.0
                && library.native_root.as_ref().is_some_and(|origin|
                    origin.as_os_str() == view.origin_root.as_os_str()) => {
                // Validate context, exact project and origin before changing
                // either the displayed library or musical editor.
                self.library(library);
                self.install_document(current);
                self.capture_completion.view_ready(&view.request_sha256);
                self.status = "Recording saved. The current saved session is open.".into();
                self.error = None;
            }
            _ => {
                self.capture_completion.view_failed(&view.request_sha256, "The editor response did not match the saved session");
                self.error = Some("Recording saved; its editor response did not match. Your current work was kept.".into());
            }
        }
    }
}
