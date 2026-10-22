//! Sole-worker admission and native lifetime for explicit copied recovery.
use super::*;
use omniusong_session::ServiceError;
use source_recovery_ui::{
    SourceRecoveryAction, SourceRecoveryAvailability, SourceRecoveryOffer, SourceRecoveryOperation,
    SourceRecoveryTarget,
};
use std::io::Write;

pub(super) fn source_recovery_command(command: &Command) -> bool {
    matches!(
        command,
        Command::SavePreparedPerformanceSourceAction { .. }
            | Command::ListPreparedPerformanceSourceActions
            | Command::ListPerformanceSourceActionStates
            | Command::ReadPerformanceSourceAction { .. }
            | Command::ExecutePerformanceSourceAction { .. }
            | Command::ReconcilePerformanceSourceAction { .. }
            | Command::SavePerformanceSourceDecision { .. }
            | Command::AttachPerformanceSource { .. }
            | Command::ReconcilePerformanceSourceAttachment { .. }
    )
}

/// Identity only; correctly correlated but invalid bodies are terminal protocol
/// observations, never reasons to hold an already-consumed service response.
pub(super) fn reference_matches(
    request: &SourceRecoveryRequest,
    result: &Result<Reply, ServiceError>,
) -> bool {
    let Ok(reply) = result else {
        return true;
    };
    let command = request.command();
    let identity = |r: &omniusong_session::PerformanceSourceActionReference,
                    state: &omniusong_session::PerformanceSourceActionState| {
        let triple = match &command {
            Command::ReadPerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            }
            | Command::ExecutePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            }
            | Command::ReconcilePerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            }
            | Command::SavePerformanceSourceDecision {
                id,
                revision,
                payload_sha256,
                ..
            }
            | Command::AttachPerformanceSource {
                id,
                revision,
                payload_sha256,
            }
            | Command::ReconcilePerformanceSourceAttachment {
                id,
                revision,
                payload_sha256,
            } => Some((id, revision, payload_sha256)),
            _ => None,
        };
        triple.is_some_and(|(id, revision, sha)| {
            r.matches(id, *revision, sha)
                && state.prepared.action.id == *id
                && state.revision >= *revision
        })
    };
    match (&command, reply) {
        (Command::ListPerformanceSourceActionStates, Reply::PerformanceSourceActionStates(_)) => {
            true
        }
        (
            Command::ReadPerformanceSourceAction {
                id,
                revision,
                payload_sha256,
            },
            Reply::PerformanceSourceActionDetail(detail),
        ) => {
            detail.state.prepared.action.id == *id
                && detail.state.revision == *revision
                && detail.state.payload_sha256 == *payload_sha256
        }
        (
            Command::SavePreparedPerformanceSourceAction { action, .. },
            Reply::PreparedPerformanceSourceAction(receipt),
        ) => receipt.revision == 1 && receipt.action == **action,
        (
            Command::ExecutePerformanceSourceAction { .. }
            | Command::ReconcilePerformanceSourceAction { .. },
            Reply::PerformanceSourceExecution(receipt),
        ) => identity(&receipt.requested, &receipt.state),
        (
            Command::SavePerformanceSourceDecision { .. },
            Reply::PerformanceSourceDecision(receipt),
        ) => identity(&receipt.requested, &receipt.state),
        (
            Command::AttachPerformanceSource { .. }
            | Command::ReconcilePerformanceSourceAttachment { .. },
            Reply::PerformanceSourceAttachment(receipt),
        ) => identity(&receipt.requested, &receipt.state),
        _ => false,
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct SourceRecoveryContext {
    music: evidence_integration::EvidenceContext,
    source: (
        Option<u64>,
        Option<u64>,
        u64,
        u64,
        Option<String>,
        bool,
        bool,
    ),
}
struct ReviewBytes(Vec<u8>);
impl Write for ReviewBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len()
            > omniusong_store::MAX_PERFORMANCE_SOURCE_REVIEW_BYTES.saturating_sub(self.0.len())
        {
            return Err(std::io::Error::other(
                "Source review exceeds its retained byte limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl StudioApp {
    fn current_source_recovery_context(&self) -> SourceRecoveryContext {
        SourceRecoveryContext {
            music: self.current_evidence_context(),
            source: self.performance_sources.recovery_context_key(),
        }
    }
    pub(super) fn sync_source_recovery_context(&mut self) {
        let context = self.current_source_recovery_context();
        if self.source_recovery_context.as_ref() != Some(&context) {
            self.source_recovery.context_changed();
            self.source_recovery_context = Some(context);
        }
    }
    pub(super) fn source_recovery_availability(&self) -> SourceRecoveryAvailability {
        let evidence = self.evidence_availability();
        let can_submit = evidence.can_submit
            && self.ready
            && self.native_origin.is_some()
            && !self.evidence_metadata_only
            && !self.audio_close_requested
            && !self.recovery_close_requested
            && !self.note_close_requested
            && !self.clip_close_requested;
        let review = self.performance_sources.recovery_offer().and_then(
            |(generation, epoch, serial, review)| {
                let mut bytes = ReviewBytes(vec![]);
                serde_json::to_writer(&mut bytes, review).ok()?;
                Some(SourceRecoveryOffer {
                    generation,
                    epoch,
                    serial,
                    review: bytes.0,
                })
            },
        );
        SourceRecoveryAvailability {
            can_submit,
            can_mutate: can_submit && self.evidence_mutation_allowed(),
            can_read_artifacts: can_submit
                && !self.audio.blocks_session()
                && !self.recovery.blocks_session(),
            generation: evidence.generation,
            origin: self.native_origin.clone(),
            review,
            target: self.document.as_ref().map(|doc| SourceRecoveryTarget {
                project_id: ProjectId(doc.id().into()),
                revision: doc.revision(),
                requires_native_audio_upgrade: doc.schema_version() == 1,
            }),
            unavailable_reason: if !self.ready || self.evidence_metadata_only {
                Some("Open the original studio catalog before reading recovery choices. Evidence history alone does not restore those choices.".into())
            } else {
                evidence.unavailable_reason
            },
        }
    }
    pub(super) fn validate_source_recovery_submission(
        &self,
        command: &Command,
        action: &Action,
        after: Option<&Action>,
    ) -> AppResult<()> {
        let Action::SourceRecovery(request) = action else {
            return Err("Copied recovery commands remain backend-only outside their exact explicit review route".into());
        };
        let availability = self.source_recovery_availability();
        let generation = availability
            .generation
            .ok_or("Open the original studio first")?;
        if after.is_some()
            || !request.matches_command(command)
            || !self.source_recovery.authorizes(request, generation)
        {
            return Err("Recovery needs its exact current request without a continuation".into());
        }
        if !availability.can_submit {
            return Err(
                "Recovery storage is unavailable or busy; no replacement catalog was opened".into(),
            );
        }
        if request.is_artifact() && !availability.can_read_artifacts {
            return Err(
                "Stop audio and resolve retained recordings before checking recovery artifacts"
                    .into(),
            );
        }
        if request.is_mutation()
            && (!availability.can_mutate
                || self.source_recovery_context.as_ref()
                    != Some(&self.current_source_recovery_context()))
        {
            return Err("Resolve musical drafts, failed choices and audio, then explicitly review recovery again".into());
        }
        if request.is_mutation()
            && let Some(publication) = request.attachment_publication()
        {
            let target = availability
                .target
                .as_ref()
                .ok_or("Open the exact saved attachment target first")?;
            let approved = request
                .approved_target()
                .ok_or("Review the attachment target and placement explicitly")?;
            if target.project_id != publication.project_id
                || approved.project_id != target.project_id
            {
                return Err("The saved publication belongs to a different session; no target was substituted".into());
            }
            if request.is_mutation()
                && target.requires_native_audio_upgrade
                && !request.permits_native_audio_upgrade()
            {
                return Err(
                    "Review and consent to the native-audio format change before attaching".into(),
                );
            }
            if matches!(request.operation, SourceRecoveryOperation::Decision { .. })
                && (approved != target || publication.expected_revision != target.revision)
            {
                return Err("The attachment target changed before saving its choice; review the current session again".into());
            }
        }
        command.validate()?;
        Ok(())
    }
    pub(super) fn handle_source_recovery_action(&mut self, action: SourceRecoveryAction) {
        match action {
            SourceRecoveryAction::Submit(request) => self.request(Action::SourceRecovery(request)),
            SourceRecoveryAction::Cancel(id) => {
                let exact = self.source_recovery.pending_cancel() == Some(id)
                    && self.active.as_ref().is_some_and(|active| {
                        active.id == id
                            && matches!(active.action, Action::SourceRecovery(_))
                            && matches!(
                                active.command,
                                Command::ExecutePerformanceSourceAction { .. }
                                    | Command::ReconcilePerformanceSourceAction { .. }
                                    | Command::AttachPerformanceSource { .. }
                                    | Command::ReconcilePerformanceSourceAttachment { .. }
                            )
                    });
                let accepted = exact
                    && self
                        .service
                        .as_ref()
                        .is_some_and(|service| service.cancel_source_action(id));
                self.source_recovery.cancel_requested(id, accepted);
            }
        }
    }
    pub(super) fn complete_source_recovery(
        &mut self,
        request: &SourceRecoveryRequest,
        id: RequestId,
        mut result: Result<Reply, ServiceError>,
    ) {
        let unchanged =
            self.source_recovery_context.as_ref() == Some(&self.current_source_recovery_context());
        self.sync_source_recovery_context();
        let retirement = match &result {
            Err(error)
                if matches!(
                    error.kind,
                    FailureKind::StorageUnavailable | FailureKind::Disconnected
                ) =>
            {
                Some(error.message.chars().take(512).collect::<String>())
            }
            Ok(Reply::PerformanceSourceAttachment(receipt)) => receipt
                .current_document_error
                .as_ref()
                .filter(|error| error.kind == FailureKind::StorageUnavailable)
                .map(|error| error.message.chars().take(512).collect::<String>()),
            _ => None,
        };
        let invalid = result
            .as_ref()
            .is_ok_and(|reply| !request.matches_reply(reply));
        let protocol = invalid.then(|| ServiceError { kind: FailureKind::Protocol,
            message: "The correlated recovery response did not match its reviewed body. Exact identities were kept; completion remains uncertain.".into(),
            outcome_unknown: request.is_mutation() || request.is_artifact(),
        });
        let observed = protocol.as_ref().map_or_else(|| result.as_ref(), Err);
        let accepted = self.source_recovery.completed(request, id, observed);
        self.error = observed
            .err()
            .map(|error| error.message.chars().take(512).collect());
        let can_install = accepted
            && !invalid
            && unchanged
            && matches!(request.operation, SourceRecoveryOperation::Attach { .. })
            && self.dirty_family().is_ok_and(|family| family.is_none())
            && !self.musical_unresolved()
            && self.retry.is_none()
            && self.pending.is_none()
            && !self.closing;
        if can_install
            && let Ok(Reply::PerformanceSourceAttachment(receipt)) = &mut result
            && receipt.committed
            && let Some(view) = receipt.current_document.take()
            && self
                .document
                .as_ref()
                .is_some_and(|doc| doc.id() == view.document.id())
        {
            // This is the separately loaded current view, never a historical postimage.
            if let Some(summary) = self
                .sessions
                .iter_mut()
                .find(|summary| summary.id == view.document.id())
            {
                summary.name = view.document.name().into();
                summary.revision = view.document.revision();
                summary.track_count = view.document.tracks().len();
                summary.note_count = view
                    .document
                    .tracks()
                    .iter()
                    .filter(|track| track["kind"] != "recording")
                    .filter_map(|track| track["notes"].as_array())
                    .map(Vec::len)
                    .sum();
            }
            self.install_document(view);
        }
        if let Some(message) = retirement {
            self.ready = false;
            self.reconnect_existing = true;
            self.source_recovery.worker_lost(id.generation, &message);
            self.performance_sources
                .worker_lost(id.generation, &message);
            self.evidence.worker_lost(id.generation, &message);
            self.error = Some(message);
            self.status = "Recovery result retained. Storage stopped; reconnect the original studio explicitly.".into();
        } else {
            self.status = "Recovery action finished. Review its recorded state and current observation separately; musical drafts were kept.".into();
        }
    }
    pub(super) fn show_source_recovery_window(&mut self, ctx: &egui::Context) {
        self.sync_source_recovery_context();
        if !self.source_recovery.open {
            return;
        }
        let availability = self.source_recovery_availability();
        let mut open = self.source_recovery.open;
        let mut action = None;
        egui::Window::new("Recording recovery")
            .id(egui::Id::unique("recording-recovery-window"))
            .open(&mut open)
            .default_width(700.0)
            .default_height(620.0)
            .min_width(340.0)
            .show(ctx, |ui| {
                action = self.source_recovery.ui(ui, &availability);
            });
        self.source_recovery.open = open;
        if let Some(action) = action {
            self.handle_source_recovery_action(action);
        }
    }
}
