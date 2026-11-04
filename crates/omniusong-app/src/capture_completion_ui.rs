//! Passive recording-save facts. The coordinator owns submission and editor context guards.

use eframe::egui;
use omniusong_session::{
    CaptureAttachmentCommitment, CaptureAttachmentOutcome, CaptureAttachmentReply,
    CaptureAttachmentRequest, ServiceError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CaptureCompletionDecision {
    Committed,
    Rejected,
    Uncertain,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum CaptureViewState {
    #[default]
    NotRequested,
    Loading,
    Ready,
    Failed(String),
    ContextChanged,
}

#[derive(Default)]
pub(super) struct CaptureCompletionUi {
    request: Option<CaptureAttachmentRequest>,
    request_sha256: Option<String>,
    commitment: Option<CaptureAttachmentCommitment>,
    last_reply: Option<CaptureAttachmentReply>,
    uncertain_reply: Option<CaptureAttachmentReply>,
    client_uncertainty: Option<ServiceError>,
    inflight: bool,
    view: CaptureViewState,
}

impl CaptureCompletionUi {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Freeze the complete request before submitting it. Retrying does not
    /// replace an earlier unconfirmed attempt with a fresh save identity.
    pub(super) fn begin(&mut self, request: CaptureAttachmentRequest) -> Result<(), String> {
        let sha = request.fingerprint().map_err(|error| error.message)?;
        if self.request_sha256.as_deref() == Some(sha.as_str()) {
            if self.commitment.is_some() {
                return Err("This recording is already saved.".into());
            }
            if self.inflight {
                return Err("This recording is still being saved.".into());
            }
            self.inflight = true;
            return Ok(());
        }
        if self.inflight || self.uncertain() {
            return Err("Confirm the previous recording save before saving another recording.".into());
        }
        *self = Self {
            request: Some(request),
            request_sha256: Some(sha),
            inflight: true,
            ..Self::default()
        };
        Ok(())
    }

    /// Invalid references/bodies change no facts. The coordinator separately
    /// handles protocol uncertainty and independently valid worker retirement.
    pub(super) fn completed(
        &mut self,
        request: &CaptureAttachmentRequest,
        reply: &CaptureAttachmentReply,
    ) -> Result<CaptureCompletionDecision, String> {
        self.require_request(request)?;
        if !reply.matches_request_reference(request) {
            return Err("The recording save response belongs to a different request.".into());
        }
        reply.validate_for(request).map_err(|error| error.message)?;

        let decision = match &reply.outcome {
            CaptureAttachmentOutcome::Committed { commitment } => {
                // Keep the first confirmed fact across a recorded replay. The
                // latest reply still describes this attempt's completion kind.
                if self.commitment.is_none() {
                    self.commitment = Some(commitment.clone());
                }
                self.uncertain_reply = None;
                self.client_uncertainty = None;
                CaptureCompletionDecision::Committed
            }
            CaptureAttachmentOutcome::Rejected { .. } => {
                // A refusal describes this attempt only. It cannot disprove
                // the project disposition of a previous uncertain attempt.
                CaptureCompletionDecision::Rejected
            }
            CaptureAttachmentOutcome::Uncertain { .. } => {
                if self.commitment.is_none() {
                    self.uncertain_reply = Some(reply.clone());
                }
                CaptureCompletionDecision::Uncertain
            }
        };
        self.last_reply = Some(reply.clone());
        self.inflight = false;
        Ok(decision)
    }

    /// A lost response is client uncertainty; it supplies no invocation fact.
    pub(super) fn transport_failed(
        &mut self,
        request: &CaptureAttachmentRequest,
        error: &ServiceError,
    ) -> Result<(), String> {
        self.require_request(request)?;
        self.inflight = false;
        if self.commitment.is_none() {
            self.client_uncertainty = Some(error.clone());
        }
        Ok(())
    }

    fn require_request(&self, request: &CaptureAttachmentRequest) -> Result<(), String> {
        let sha = request.fingerprint().map_err(|error| error.message)?;
        if self.request_sha256.as_deref() == Some(sha.as_str()) {
            Ok(())
        } else {
            Err("The recording save request no longer matches the retained recording.".into())
        }
    }

    pub(super) fn request(&self) -> Option<&CaptureAttachmentRequest> {
        self.request.as_ref()
    }

    pub(super) fn request_sha256(&self) -> Option<&str> {
        self.request_sha256.as_deref()
    }

    pub(super) fn commitment(&self) -> Option<&CaptureAttachmentCommitment> {
        self.commitment.as_ref()
    }

    /// Actual store facts for the latest validated reply, never a fabricated
    /// reply for EOF or a failed editor read.
    pub(super) fn last_reply(&self) -> Option<&CaptureAttachmentReply> {
        self.last_reply.as_ref()
    }

    pub(super) fn uncertain_reply(&self) -> Option<&CaptureAttachmentReply> {
        self.uncertain_reply.as_ref()
    }

    pub(super) fn client_uncertainty(&self) -> Option<&ServiceError> {
        self.client_uncertainty.as_ref()
    }

    pub(super) fn uncertain(&self) -> bool {
        self.commitment.is_none()
            && (self.uncertain_reply.is_some() || self.client_uncertainty.is_some())
    }

    pub(super) fn is_inflight(&self) -> bool {
        self.inflight
    }

    pub(super) fn can_retry(&self) -> bool {
        self.request.is_some() && self.commitment.is_none() && !self.inflight
    }

    pub(super) fn view_state(&self) -> &CaptureViewState {
        &self.view
    }

    fn set_view(&mut self, request_sha256: &str, state: CaptureViewState) -> bool {
        if self.request_sha256.as_deref() != Some(request_sha256) || self.commitment.is_none() {
            return false;
        }
        self.view = state;
        true
    }

    pub(super) fn view_loading(&mut self, request_sha256: &str) -> bool {
        self.set_view(request_sha256, CaptureViewState::Loading)
    }

    pub(super) fn view_ready(&mut self, request_sha256: &str) -> bool {
        self.set_view(request_sha256, CaptureViewState::Ready)
    }

    pub(super) fn view_failed(
        &mut self,
        request_sha256: &str,
        message: impl Into<String>,
    ) -> bool {
        self.set_view(request_sha256, CaptureViewState::Failed(message.into()))
    }

    pub(super) fn view_context_changed(&mut self, request_sha256: &str) -> bool {
        self.set_view(request_sha256, CaptureViewState::ContextChanged)
    }

    /// Paint reads only stored state; it performs no request encoding or work.
    pub(super) fn show(&self, ui: &mut egui::Ui) {
        if self.request.is_none() {
            return;
        }
        ui.group(|ui| {
            if self.commitment.is_some() {
                ui.strong("Recording saved");
                ui.label("Your recording is saved in the session.");
                ui.small(match &self.view {
                    CaptureViewState::NotRequested => "Editor has not refreshed yet.",
                    CaptureViewState::Loading => "Editor: opening the saved session…",
                    CaptureViewState::Ready => "Editor: saved session opened.",
                    CaptureViewState::Failed(_) => {
                        "Editor could not refresh. Your recording is still saved."
                    }
                    CaptureViewState::ContextChanged => {
                        "Editor kept your current work. Open the saved session when ready."
                    }
                });
            } else if self.uncertain() {
                ui.strong("Recording save unconfirmed");
                ui.label(if self.inflight {
                    "Checking whether this recording was saved…"
                } else {
                    "Retry this recording to check whether it was saved."
                });
            } else if self.inflight {
                ui.strong("Saving recording…");
            } else {
                ui.strong("Recording was not added");
                ui.label("Review the save error before trying again.");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::{
        contracts::{OperationId, ProjectId, TakeId, TrackId},
        native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
    };
    use omniusong_session::{
        CaptureAttachmentCompletion, CaptureAttachmentMessage, CaptureAttachmentProblem,
        CaptureAttachmentProblemKind, FailureKind,
    };
    use omniusong_store::TakePublication;
    use std::path::PathBuf;

    fn request() -> CaptureAttachmentRequest {
        let root = PathBuf::from(if cfg!(windows) {
            r"C:\capture-ui-fixture"
        } else {
            "/capture-ui-fixture"
        });
        CaptureAttachmentRequest {
            version: 1,
            origin_root: root.join("studio"),
            journal_path: root.join("recording.sqlite3"),
            publication: TakePublication {
                operation_id: OperationId("recording-save".into()),
                project_id: ProjectId("recording-project".into()),
                expected_revision: 7,
                track_id: TrackId("recording-track".into()),
                track_name: "Recorded performance".into(),
                clip: TakeClip {
                    id: "recording-clip".into(),
                    source: TakeReference {
                        take_id: TakeId("recording-take".into()),
                        manifest_sha256: "a".repeat(64),
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames { start: 0, count: 8 },
                    position: TimelinePosition {
                        frame: 0,
                        sample_rate: 48_000,
                    },
                    fades: None,
                    edit_source: None,
                },
            },
        }
    }

    fn committed(
        request: &CaptureAttachmentRequest,
        completion: CaptureAttachmentCompletion,
    ) -> CaptureAttachmentReply {
        CaptureAttachmentReply {
            version: 1,
            request_sha256: request.fingerprint().unwrap(),
            store_invoked: true,
            outcome: CaptureAttachmentOutcome::Committed {
                commitment: CaptureAttachmentCommitment {
                    operation_id: request.publication.operation_id.clone(),
                    project_id: request.publication.project_id.clone(),
                    previous_revision: request.publication.expected_revision,
                    committed_revision: request.publication.expected_revision + 1,
                    completion,
                },
            },
            catalog_retirement: None,
        }
    }

    fn failure(
        request: &CaptureAttachmentRequest,
        uncertain: bool,
    ) -> CaptureAttachmentReply {
        let problem = CaptureAttachmentProblem {
            kind: if uncertain {
                CaptureAttachmentProblemKind::Io
            } else {
                CaptureAttachmentProblemKind::Rejected
            },
            detail: CaptureAttachmentMessage {
                text: "Synthetic save interruption".into(),
                truncated: false,
            },
            sqlite: None,
            governor_stop: None,
        };
        CaptureAttachmentReply {
            version: 1,
            request_sha256: request.fingerprint().unwrap(),
            store_invoked: uncertain,
            outcome: if uncertain {
                CaptureAttachmentOutcome::Uncertain { problem }
            } else {
                CaptureAttachmentOutcome::Rejected { problem }
            },
            catalog_retirement: None,
        }
    }

    fn eof() -> ServiceError {
        ServiceError {
            kind: FailureKind::Disconnected,
            message: "Synthetic lost response".into(),
            outcome_unknown: true,
        }
    }

    #[test]
    fn editor_failure_and_later_transport_loss_keep_confirmed_save() {
        let request = request();
        let reply = committed(&request, CaptureAttachmentCompletion::FreshCommit);
        let mut ui = CaptureCompletionUi::new();
        ui.begin(request.clone()).unwrap();
        assert_eq!(
            ui.completed(&request, &reply).unwrap(),
            CaptureCompletionDecision::Committed
        );
        assert!(!ui.can_retry());
        let saved = ui.commitment().cloned().unwrap();
        let sha = ui.request_sha256().unwrap().to_owned();
        assert!(ui.view_loading(&sha));
        assert!(ui.view_failed(&sha, "Synthetic editor read failure"));
        ui.transport_failed(&request, &eof()).unwrap();
        assert_eq!(ui.commitment(), Some(&saved));
        assert!(matches!(ui.view_state(), CaptureViewState::Failed(_)));
        assert!(!ui.can_retry());
        assert!(!ui.uncertain());
        assert!(ui.client_uncertainty().is_none());
        assert!(ui.begin(request).is_err());
        assert_eq!(ui.commitment(), Some(&saved));
    }

    #[test]
    fn replay_is_idempotent_and_does_not_claim_an_editor_view() {
        let request = request();
        let fresh = committed(&request, CaptureAttachmentCompletion::FreshCommit);
        let replay = committed(&request, CaptureAttachmentCompletion::RecordedReplay);
        let mut ui = CaptureCompletionUi::new();
        ui.begin(request.clone()).unwrap();
        ui.completed(&request, &fresh).unwrap();
        let saved = ui.commitment().cloned().unwrap();
        ui.completed(&request, &replay).unwrap();
        ui.completed(&request, &replay).unwrap();
        assert_eq!(ui.commitment(), Some(&saved));
        assert_eq!(ui.last_reply(), Some(&replay));
        assert_eq!(ui.view_state(), &CaptureViewState::NotRequested);
        assert!(!ui.can_retry());
    }

    #[test]
    fn lost_response_then_rejected_retry_stays_unconfirmed_until_exact_commitment() {
        let request = request();
        let mut ui = CaptureCompletionUi::new();
        ui.begin(request.clone()).unwrap();
        ui.transport_failed(&request, &eof()).unwrap();
        assert!(ui.uncertain());
        assert!(ui.last_reply().is_none());
        assert!(ui.can_retry());
        ui.begin(request.clone()).unwrap();
        assert!(ui.uncertain());
        let rejected = failure(&request, false);
        assert_eq!(
            ui.completed(&request, &rejected).unwrap(),
            CaptureCompletionDecision::Rejected
        );
        assert!(ui.uncertain());
        assert!(ui.client_uncertainty().is_some());
        assert_eq!(ui.last_reply().unwrap().store_invoked, false);
        assert!(ui.can_retry());
        ui.begin(request.clone()).unwrap();
        let replay = committed(&request, CaptureAttachmentCompletion::RecordedReplay);
        ui.completed(&request, &replay).unwrap();
        assert!(ui.commitment().is_some());
        assert!(!ui.uncertain());
        assert!(ui.client_uncertainty().is_none());
        assert!(!ui.can_retry());
        assert_eq!(ui.request(), Some(&request));
    }

    #[test]
    fn rejected_retry_preserves_earlier_invoked_uncertainty_facts() {
        let request = request();
        let uncertain = failure(&request, true);
        let mut ui = CaptureCompletionUi::new();
        ui.begin(request.clone()).unwrap();
        ui.completed(&request, &uncertain).unwrap();
        ui.begin(request.clone()).unwrap();
        ui.completed(&request, &failure(&request, false)).unwrap();
        assert!(ui.uncertain());
        assert_eq!(ui.uncertain_reply(), Some(&uncertain));
        assert!(ui.uncertain_reply().unwrap().store_invoked);
        assert!(!ui.last_reply().unwrap().store_invoked);
    }

    #[test]
    fn foreign_requests_references_and_invalid_bodies_do_not_replace_saved_facts() {
        let request = request();
        let reply = committed(&request, CaptureAttachmentCompletion::FreshCommit);
        let mut ui = CaptureCompletionUi::new();
        ui.begin(request.clone()).unwrap();
        ui.completed(&request, &reply).unwrap();
        let saved = ui.commitment().cloned().unwrap();
        let mut foreign = request.clone();
        foreign.journal_path = foreign.origin_root.join("other-recording.sqlite3");
        assert!(ui.completed(&foreign, &reply).is_err());
        let foreign_reply = committed(&foreign, CaptureAttachmentCompletion::FreshCommit);
        assert!(ui.completed(&request, &foreign_reply).is_err());
        let mut malformed = reply.clone();
        if let CaptureAttachmentOutcome::Committed { commitment } = &mut malformed.outcome {
            commitment.committed_revision += 1;
        }
        assert!(ui.completed(&request, &malformed).is_err());
        malformed = reply.clone();
        malformed.version = 2;
        assert!(ui.completed(&request, &malformed).is_err());
        assert!(ui.transport_failed(&foreign, &eof()).is_err());
        assert_eq!(ui.request(), Some(&request));
        assert_eq!(ui.commitment(), Some(&saved));
        assert_eq!(ui.last_reply(), Some(&reply));
        assert!(!ui.uncertain());
        assert!(!ui.can_retry());
    }

    #[test]
    fn editor_updates_require_the_exact_saved_request_fingerprint() {
        let request = request();
        let sha = request.fingerprint().unwrap();
        let wrong_sha = "0".repeat(64);
        let mut ui = CaptureCompletionUi::new();
        ui.begin(request.clone()).unwrap();
        assert!(!ui.view_loading(&sha));
        ui.completed(
            &request,
            &committed(&request, CaptureAttachmentCompletion::FreshCommit),
        )
        .unwrap();
        assert!(!ui.view_loading(&wrong_sha));
        assert!(!ui.view_ready(&wrong_sha));
        assert!(!ui.view_failed(&wrong_sha, "Foreign failure"));
        assert!(!ui.view_context_changed(&wrong_sha));
        assert_eq!(ui.view_state(), &CaptureViewState::NotRequested);
        assert!(ui.view_loading(&sha));
        assert!(ui.view_ready(&sha));
        assert_eq!(ui.view_state(), &CaptureViewState::Ready);
        assert!(ui.view_context_changed(&sha));
        assert_eq!(ui.view_state(), &CaptureViewState::ContextChanged);
        assert!(!ui.can_retry());
    }

    #[test]
    fn a_different_request_cannot_replace_inflight_or_uncertain_recording() {
        let request = request();
        let mut different_origin = request.clone();
        different_origin.origin_root = different_origin.origin_root.join("other-studio");
        let mut different_locator = request.clone();
        different_locator.journal_path = different_locator.origin_root.join("other.sqlite3");
        let mut ui = CaptureCompletionUi::new();
        ui.begin(request.clone()).unwrap();
        assert!(ui.begin(different_origin.clone()).is_err());
        assert!(ui.is_inflight());
        ui.transport_failed(&request, &eof()).unwrap();
        assert!(ui.begin(different_origin).is_err());
        assert!(ui.begin(different_locator).is_err());
        assert_eq!(ui.request(), Some(&request));
        assert!(ui.uncertain());
        assert!(ui.can_retry());
    }

    #[test]
    fn a_proven_first_refusal_allows_a_different_recording() {
        let request = request();
        let mut next = request.clone();
        next.publication.operation_id = OperationId("next-recording-save".into());
        let mut ui = CaptureCompletionUi::new();
        ui.begin(request.clone()).unwrap();
        ui.completed(&request, &failure(&request, false)).unwrap();
        assert!(!ui.uncertain());
        ui.begin(next.clone()).unwrap();
        assert_eq!(ui.request(), Some(&next));
        assert!(ui.is_inflight());
        assert!(ui.last_reply().is_none());
    }
}
