//! Explicit retained-stage review. All source access stays on the session worker.

use eframe::egui;
use omniusong_core::{
    contracts::{OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_session::{CaptureAttachmentRequest, CaptureInspection, CaptureRecoveryReceipt, CaptureStageScan, Command};
use omniusong_store::{CaptureAudio, CaptureStageState, TakePublication};
use std::path::{Path, PathBuf};

const MAX_RELEASED_ATTACHMENTS: usize = 8;

#[derive(Clone)]
struct AttachmentEvidence {
    inspection: Option<Box<CaptureInspection>>,
    recovery: Option<Box<CaptureRecoveryReceipt>>,
}
struct ReleasedAttachment {
    action: RecoveryAction,
    evidence: Option<AttachmentEvidence>,
}
#[derive(Clone)]
pub(super) struct AttachmentChoice {
    journal_path: PathBuf,
    take_id: String,
    manifest_sha256: String,
    audio: CaptureAudio,
    project_id: String,
    operation_id: OperationId,
    track_id: TrackId,
    clip_id: String,
    evidence: AttachmentEvidence,
}
#[derive(Clone)]
pub(super) enum RecoveryAction {
    Scan,
    Inspect(String),
    Recover {
        inspection: Box<CaptureInspection>,
        operation_id: OperationId,
    },
    ChooseAttachment(Box<AttachmentChoice>),
    Attach {
        origin_root: PathBuf,
        journal_path: PathBuf,
        publication: Box<TakePublication>,
    },
}
impl RecoveryAction {
    pub(super) fn read_only(&self) -> bool {
        matches!(self, Self::Scan | Self::Inspect(_))
    }
    pub(super) fn command(&self) -> Result<Command, String> {
        Ok(match self {
            Self::Scan => Command::ScanCaptures,
            Self::Inspect(stage_id) => Command::InspectCapture {
                stage_id: stage_id.clone(),
            },
            Self::Recover {
                inspection,
                operation_id,
            } => Command::RecoverCapture {
                inspection: inspection.clone(),
                operation_id: operation_id.clone(),
            },
            Self::Attach {
                origin_root,
                journal_path,
                publication,
            } => Command::AttachCapture {
                request: Box::new(CaptureAttachmentRequest {
                    version: 1,
                    origin_root: origin_root.clone(),
                    journal_path: journal_path.clone(),
                    publication: publication.as_ref().clone(),
                }),
            },
            Self::ChooseAttachment(_) => {
                return Err("Choose a saved session revision before attaching".into());
            }
        })
    }
}

pub(super) struct RecoveryUi {
    scan: Option<CaptureStageScan>,
    selected: Option<String>,
    inspection: Option<Box<CaptureInspection>>,
    recovered: Option<Box<CaptureRecoveryReceipt>>,
    stream: usize,
    recover_retry: Option<RecoveryAction>,
    recover_inflight: bool,
    attach_retry: Option<RecoveryAction>,
    attach_inflight: bool,
    attach_suspended: bool,
    attach_known_rejected: bool,
    attach_evidence: Option<AttachmentEvidence>,
    released: Vec<ReleasedAttachment>,
}
impl RecoveryUi {
    pub(super) fn new() -> Self {
        Self {
            scan: None,
            selected: None,
            inspection: None,
            recovered: None,
            stream: 0,
            recover_retry: None,
            recover_inflight: false,
            attach_retry: None,
            attach_inflight: false,
            attach_suspended: false,
            attach_known_rejected: false,
            attach_evidence: None,
            released: vec![],
        }
    }
    pub(super) fn blocks_session(&self) -> bool {
        self.recover_inflight
            || self.attach_inflight
            || (self.attach_retry.is_some() && !self.attach_suspended)
    }
    pub(super) fn scanned(&mut self, scan: CaptureStageScan) {
        if !scan
            .stages
            .iter()
            .any(|stage| self.selected.as_deref() == Some(stage.stage_id.as_str()))
        {
            self.selected = scan.stages.first().map(|stage| stage.stage_id.clone());
        }
        self.inspection = None;
        self.stream = 0;
        self.scan = Some(scan);
    }
    pub(super) fn inspected(&mut self, inspection: Box<CaptureInspection>) -> Result<(), String> {
        if self.selected.as_deref() != Some(&inspection.stage_id) {
            return Err(
                "The retained-stage selection changed. Inspect the chosen stage again.".into(),
            );
        }
        if self.recovered.as_ref().is_some_and(|receipt| {
            receipt.stage_id == inspection.stage_id
                && (receipt.take_id != inspection.take_id
                    || inspection.manifest_sha256.as_ref() != Some(&receipt.manifest_sha256))
        }) {
            self.recovered = None;
        }
        self.inspection = Some(inspection);
        self.stream = 0;
        Ok(())
    }
    pub(super) fn recovered(&mut self, receipt: Box<CaptureRecoveryReceipt>) -> Result<(), String> {
        self.recover_inflight = false;
        let matches = matches!(&self.recover_retry,Some(RecoveryAction::Recover {inspection,operation_id})if inspection.stage_id==receipt.stage_id&&inspection.inspection_sha256==receipt.reviewed_inspection_sha256&&operation_id.0==receipt.operation_id);
        if !matches {
            return Err("Recovery receipt did not match the retained reviewed request; no source was selected for attachment.".into());
        }
        self.recover_retry = None;
        self.recovered = Some(receipt);
        self.stream = 0;
        Ok(())
    }
    pub(super) fn started(&mut self, action: &RecoveryAction) {
        match action {
            RecoveryAction::Recover { .. } => {
                self.recover_retry = Some(action.clone());
                self.recover_inflight = true;
            }
            RecoveryAction::Attach { .. } => {
                self.attach_retry = Some(action.clone());
                self.attach_inflight = true;
                self.attach_known_rejected = false;
            }
            _ => {}
        }
    }
    pub(super) fn failed(&mut self, action: &RecoveryAction) {
        match action {
            RecoveryAction::Recover { .. } => {
                self.recover_retry = Some(action.clone());
                self.recover_inflight = false;
            }
            RecoveryAction::Attach { .. } => {
                self.attach_retry = Some(action.clone());
                self.attach_inflight = false;
            }
            _ => {}
        }
    }
    pub(super) fn attached(&mut self) {
        self.attach_retry = None;
        self.attach_inflight = false;
        self.attach_suspended = false;
        self.attach_known_rejected = false;
        self.attach_evidence = None;
    }
    pub(super) fn revision_conflict(&mut self, action: &RecoveryAction) {
        if let (
            Some(RecoveryAction::Attach {
                publication: stored,
                ..
            }),
            RecoveryAction::Attach { publication, .. },
        ) = (&self.attach_retry, action)
            && stored == publication
            && !self.attach_inflight
        {
            self.attach_known_rejected = true;
        }
    }
    fn keep_local(&mut self) -> bool {
        if self.attach_inflight || self.attach_retry.is_none() {
            return false;
        }
        self.attach_suspended = true;
        true
    }
    fn release_rejected(&mut self) -> bool {
        if self.attach_inflight
            || !self.attach_known_rejected
            || self.released.len() >= MAX_RELEASED_ATTACHMENTS
        {
            return false;
        }
        let Some(action) = self.attach_retry.take() else {
            return false;
        };
        self.released.push(ReleasedAttachment {
            action,
            evidence: self.attach_evidence.take(),
        });
        self.attach_suspended = false;
        self.attach_known_rejected = false;
        true
    }
    pub(super) fn prepare_attachment(
        &mut self,
        choice: &AttachmentChoice,
        project_id: &str,
        revision: u64,
        origin_root: &Path,
    ) -> Result<RecoveryAction, String> {
        if self.attach_retry.is_some() {
            return Err("An earlier attachment is unresolved. Retry that exact request, or explicitly release a known rejected choice first.".into());
        }
        if choice.project_id != project_id {
            return Err(
                "The current session changed. Choose its retained original attachment again."
                    .into(),
            );
        }
        if choice.audio.frames == 0 || choice.audio.recorded_frames == 0 {
            return Err("A playable attachment requires nonempty verified recorded audio.".into());
        }
        let publication = TakePublication {
            operation_id: choice.operation_id.clone(),
            project_id: ProjectId(project_id.into()),
            expected_revision: revision,
            track_id: choice.track_id.clone(),
            track_name: format!("Recovered original · {}", choice.audio.stream_id),
            clip: TakeClip {
                edit_source: None,
                id: choice.clip_id.clone(),
                source: TakeReference {
                    take_id: TakeId(choice.take_id.clone()),
                    manifest_sha256: choice.manifest_sha256.clone(),
                    stream_id: choice.audio.stream_id.clone(),
                },
                range: TakeRange::AudioFrames {
                    start: choice.audio.start_frame,
                    count: choice.audio.frames,
                },
                position: TimelinePosition {
                    frame: 0,
                    sample_rate: choice.audio.sample_rate,
                },
                fades: None,
            },
        };
        CaptureAttachmentRequest { version: 1, origin_root: origin_root.to_owned(),
            journal_path: choice.journal_path.clone(), publication: publication.clone()
        }.validate().map_err(|error| error.message)?;
        self.attach_evidence = Some(choice.evidence.clone());
        self.attach_suspended = false;
        self.attach_known_rejected = false;
        Ok(RecoveryAction::Attach {
            origin_root: origin_root.to_owned(),
            journal_path: choice.journal_path.clone(),
            publication: Box::new(publication),
        })
    }
    fn recovery_action(&self) -> Option<RecoveryAction> {
        let inspection = self.inspection.as_ref()?;
        if !self.can_recover() {
            return None;
        }
        if let Some(RecoveryAction::Recover {
            inspection: previous,
            ..
        }) = &self.recover_retry
            && previous.inspection_sha256 == inspection.inspection_sha256
        {
            return self.recover_retry.clone();
        }
        Some(RecoveryAction::Recover {
            inspection: inspection.clone(),
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
        })
    }
    fn can_recover(&self) -> bool {
        self.inspection.as_ref().is_some_and(|inspection| {
            matches!(
                inspection.state,
                CaptureStageState::Recording | CaptureStageState::Interrupted
            ) && !self
                .recovered
                .as_ref()
                .is_some_and(|receipt| receipt.stage_id == inspection.stage_id)
                && inspection.manifest_sha256.is_none()
                && inspection
                    .audio
                    .iter()
                    .any(|audio| audio.frames > 0 && audio.recorded_frames > 0)
        })
    }
    fn can_attach(&self) -> bool {
        let Some(selected) = self.selected.as_deref() else {
            return false;
        };
        if let Some(receipt) = self
            .recovered
            .as_ref()
            .filter(|receipt| receipt.stage_id == selected)
        {
            return receipt
                .audio
                .get(self.stream)
                .is_some_and(|audio| audio.frames > 0 && audio.recorded_frames > 0);
        }
        self.inspection
            .as_ref()
            .filter(|inspection| {
                inspection.stage_id == selected
                    && matches!(inspection.state, CaptureStageState::Sealed)
                    && inspection.manifest_sha256.is_some()
            })
            .and_then(|inspection| inspection.audio.get(self.stream))
            .is_some_and(|audio| audio.frames > 0 && audio.recorded_frames > 0)
    }
    fn attachment_choice(&self, project_id: &str) -> Option<AttachmentChoice> {
        let selected = self.selected.as_deref()?;
        let (path, take, manifest, audio) = if let Some(receipt) = self
            .recovered
            .as_ref()
            .filter(|receipt| receipt.stage_id == selected)
        {
            (
                &receipt.journal_path,
                &receipt.take_id,
                &receipt.manifest_sha256,
                receipt.audio.as_slice(),
            )
        } else {
            let inspection = self
                .inspection
                .as_ref()
                .filter(|inspection| inspection.stage_id == selected)?;
            (
                &inspection.journal_path,
                &inspection.take_id,
                inspection.manifest_sha256.as_ref()?,
                inspection.audio.as_slice(),
            )
        };
        let audio = audio.get(self.stream)?;
        if audio.frames == 0 || audio.recorded_frames == 0 {
            return None;
        }
        Some(AttachmentChoice {
            journal_path: path.clone(),
            take_id: take.clone(),
            manifest_sha256: manifest.clone(),
            audio: audio.clone(),
            project_id: project_id.into(),
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            track_id: TrackId(uuid::Uuid::new_v4().to_string()),
            clip_id: uuid::Uuid::new_v4().to_string(),
            evidence: AttachmentEvidence {
                inspection: self
                    .inspection
                    .as_ref()
                    .filter(|inspection| inspection.stage_id == selected)
                    .cloned(),
                recovery: self
                    .recovered
                    .as_ref()
                    .filter(|receipt| receipt.stage_id == selected)
                    .cloned(),
            },
        })
    }
    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        enabled: bool,
        project_id: Option<&str>,
        draft_dirty: bool,
    ) -> Option<RecoveryAction> {
        let mut action = None;
        ui.heading("Retained captures");
        ui.small("Unattached stages stay local. Project archives exclude them until explicitly attached. Scanning never opens a microphone.");
        ui.small("Scanning, inspection and recovery do not start recording.");
        if ui
            .add_enabled(enabled, egui::Button::new("Scan retained captures"))
            .clicked()
        {
            action = Some(RecoveryAction::Scan);
        }
        if let Some(scan) = &self.scan {
            ui.label(format!("{} retained stage(s)", scan.stages.len()));
            if scan.truncated {
                ui.strong("Scan limit reached; additional entries were not inspected.");
            }
            for problem in scan.problems.iter().take(16) {
                ui.label(format!("Scan problem: {problem}"));
            }
            if scan.problems.len() > 16 {
                ui.small(format!(
                    "{} more scan diagnostics retained.",
                    scan.problems.len() - 16
                ));
            }
            egui::ScrollArea::vertical()
                .id_salt("capture-stage-list")
                .max_height(140.0)
                .show_rows(ui, 22.0, scan.stages.len(), |ui, range| {
                    for index in range {
                        let stage = &scan.stages[index];
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::selectable(
                                    self.selected.as_deref() == Some(&stage.stage_id),
                                    format!("{} · {}", stage.stage_id, state_text(&stage.state)),
                                ),
                            )
                            .clicked()
                        {
                            self.selected = Some(stage.stage_id.clone());
                            self.inspection = None;
                            self.stream = 0;
                        }
                    }
                });
            if let Some(stage) = scan
                .stages
                .iter()
                .find(|stage| self.selected.as_deref() == Some(&stage.stage_id))
            {
                ui.small(format!(
                    "Retained source path: {}",
                    stage.journal_path.display()
                ));
                if let Some(problem) = &stage.problem {
                    ui.label(format!("Stage problem: {problem}"));
                }
                let inspectable = stage.problem.is_none()
                    && !matches!(
                        stage.state,
                        CaptureStageState::Owned | CaptureStageState::Invalid
                    );
                if ui
                    .add_enabled(
                        enabled && inspectable,
                        egui::Button::new("Inspect selected capture"),
                    )
                    .clicked()
                {
                    action = Some(RecoveryAction::Inspect(stage.stage_id.clone()));
                }
            }
        } else {
            ui.label("Scan explicitly to discover local capture stages.");
        }
        if let Some(RecoveryAction::Recover {
            inspection,
            operation_id,
        }) = &self.recover_retry
        {
            ui.small(format!(
                "Retained recovery request: stage {} · operation {}",
                inspection.stage_id, operation_id.0
            ));
            if ui
                .add_enabled(
                    enabled && !self.recover_inflight,
                    egui::Button::new("Retry reviewed recovery"),
                )
                .clicked()
            {
                action = self.recover_retry.clone();
            }
        }
        if let Some(inspection) = &self.inspection {
            let prior_snapshot = self.recovered.as_ref().is_some_and(|receipt| {
                self.selected.as_deref() == Some(receipt.stage_id.as_str())
                    && receipt.stage_id == inspection.stage_id
                    && receipt.reviewed_inspection_sha256 == inspection.inspection_sha256
            });
            ui.separator();
            ui.strong(if prior_snapshot {
                "Reviewed snapshot before recovery"
            } else {
                "Read-only journal inspection"
            });
            if prior_snapshot {
                ui.small("This inspection is retained as evidence from before recovery. The confirmed sealed result is shown separately below.");
            }
            ui.label(format!(
                "{}{} · take {}",
                if prior_snapshot {
                    "Before recovery: "
                } else {
                    ""
                },
                state_text(&inspection.state),
                inspection.take_id
            ));
            ui.small(format!(
                "Original journal: {}",
                inspection.journal_path.display()
            ));
            ui.label(if prior_snapshot&&inspection.unknown_tail {
                "Before recovery: unknown tail was present. Material after the reviewed prefix was unproven."
            }else if prior_snapshot {
                "Before recovery: the reviewed snapshot reported no unknown tail."
            }else if inspection.unknown_tail {
                "Unknown tail: yes. Material after the verified prefix is unproven."
            } else {
                "Unknown tail: no, according to the verified sealed manifest."
            });
            ui.small(format!(
                "{} verified chunks · {} verified payload bytes · {} source bytes",
                inspection.verified_chunks,
                inspection.verified_payload_bytes,
                inspection.byte_length
            ));
            ui.small(format!(
                "Reviewed fingerprint: {}",
                inspection.inspection_sha256
            ));
            ui.small(format!(
                "Descriptor: {} · raw file: {}",
                inspection.descriptor_sha256, inspection.journal_sha256
            ));
            ui.small(format!(
                "Retained original rows: {}",
                inspection.original_rows_sha256
            ));
            for problem in inspection.problems.iter().take(16) {
                ui.label(format!(
                    "Source problem: {} · stream {:?} · sequence {:?}",
                    problem.message, problem.stream_id, problem.sequence
                ));
            }
            if inspection.problems.len() > 16 {
                ui.small(format!(
                    "{} more source problems retained.",
                    inspection.problems.len() - 16
                ));
            }
            ui.small(match &inspection.manifest_sha256 {
                Some(hash) => format!("{}verified sealed manifest: {hash}",if prior_snapshot {"Before recovery: "}else{""}),
                None if prior_snapshot => "Before recovery: no sealed manifest. The confirmed sealed manifest appears in the recovery result below.".into(),
                None => {
                    "No sealed manifest. Recovering requires a separate explicit action.".into()
                }
            });
            if ui
                .add_enabled(
                    enabled && self.attach_retry.is_none() && self.can_recover(),
                    egui::Button::new("Recover verified prefix"),
                )
                .clicked()
            {
                action = self.recovery_action();
            }
        }
        let recovered = self
            .recovered
            .as_ref()
            .filter(|receipt| self.selected.as_deref() == Some(&receipt.stage_id));
        let audio = if let Some(receipt) = recovered {
            ui.strong("Confirmed recovery result · current sealed original");
            ui.label(if receipt.replayed {
                "Recovered original confirmed by its existing receipt."
            } else {
                "Verified prefix sealed. The original journal and all raw rows are retained."
            });
            ui.small(format!(
                "Sealed original: {} · take {}",
                receipt.journal_path.display(),
                receipt.take_id
            ));
            ui.small(format!("Manifest: {}", receipt.manifest_sha256));
            ui.label(if receipt.unknown_tail {
                "Unknown tail remains: yes. Recovery did not invent the missing final tail."
            } else {
                "Unknown tail: no."
            });
            receipt.audio.as_slice()
        } else {
            self.inspection
                .as_ref()
                .map_or(&[][..], |inspection| inspection.audio.as_slice())
        };
        if !audio.is_empty() {
            self.stream = self.stream.min(audio.len() - 1);
            let mut selected = self.stream;
            egui::ComboBox::from_id_salt("retained-audio-stream")
                .selected_text(&audio[selected].stream_id)
                .show_ui(ui, |ui| {
                    for (index, stream) in audio.iter().enumerate() {
                        ui.selectable_value(&mut selected, index, &stream.stream_id);
                    }
                })
                .response
                .accessible_name("Retained audio stream");
            self.stream = selected;
            let stream = &audio[selected];
            ui.label(format!(
                "Verified source frames {}–{} · {} recorded · {} gap frames",
                stream.start_frame,
                stream.start_frame.saturating_add(stream.frames),
                stream.recorded_frames,
                stream.gap_frames
            ));
            ui.small(format!(
                "{} Hz · {} channels · {:?} · {:?}",
                stream.sample_rate, stream.channels, stream.encoding, stream.layout
            ));
            ui.small("Attachment uses only this verified window at frame zero. Solo placement is uncalibrated; source timing is not inferred.");
        }
        if ui
            .add_enabled(
                enabled && self.attach_retry.is_none() && project_id.is_some() && self.can_attach(),
                egui::Button::new("Attach selected audio to current session"),
            )
            .clicked()
            && let Some(project_id) = project_id
        {
            action = self
                .attachment_choice(project_id)
                .map(|choice| RecoveryAction::ChooseAttachment(Box::new(choice)));
        }
        let mut keep_local = false;
        let mut release = false;
        if let Some(retained @ RecoveryAction::Attach { publication, .. }) = &self.attach_retry {
            ui.separator();
            ui.strong(if self.attach_known_rejected {
                "Original kept local · attachment rejected"
            } else if self.attach_suspended {
                "Original kept local · attachment outcome unresolved"
            } else {
                "Retained attachment request"
            });
            attachment_details(ui, retained, self.attach_evidence.as_ref());
            if self.attach_suspended && self.attach_known_rejected {
                ui.label("The session guard is released. The original and rejected attachment request remain retained.");
            } else if self.attach_suspended {
                ui.label("Returning to the session did not cancel or undo this request. It may already have attached. A fresh attachment choice stays disabled while this outcome is unresolved.");
            }
            if self.attach_known_rejected {
                ui.label("Attachment was not published because its session revision changed. The original is retained and may already have been ingested.");
            }
            let target_matches = project_id == Some(publication.project_id.0.as_str());
            if !target_matches {
                ui.small("Reopen the original target session to retry this exact attachment.");
            }
            if draft_dirty {
                ui.small("Save or discard local edits before retrying the retained attachment. Its original expected revision will stay unchanged.");
            }
            if ui
                .add_enabled(
                    enabled && !self.attach_inflight && target_matches && !draft_dirty,
                    egui::Button::new("Retry retained attachment"),
                )
                .clicked()
            {
                action = self.attach_retry.clone();
            }
            if ui
                .add_enabled(
                    enabled && !self.attach_inflight && !self.attach_suspended,
                    egui::Button::new("Keep original local and return to session"),
                )
                .clicked()
            {
                keep_local = true;
            }
            if self.attach_known_rejected {
                ui.small("Releasing this rejected choice keeps its exact request and evidence below. A later attachment requires another explicit choice and a new operation; nothing is rebased automatically.");
                if ui
                    .add_enabled(
                        enabled
                            && !self.attach_inflight
                            && self.released.len() < MAX_RELEASED_ATTACHMENTS,
                        egui::Button::new("Release rejected attachment choice"),
                    )
                    .clicked()
                {
                    release = true;
                }
                if self.released.len() >= MAX_RELEASED_ATTACHMENTS {
                    ui.label("Rejected-choice history is full in this window. Keep the original local to continue without another attachment choice.");
                }
            }
        }
        if keep_local {
            self.keep_local();
        }
        if release {
            self.release_rejected();
        }
        for (index, released) in self.released.iter().enumerate() {
            ui.collapsing(format!("Released rejected attachment {} · retained evidence",index+1),|ui|{ui.label("This request was rejected before project publication. Its original and exact request are retained here. Releasing this choice did not cancel or undo work and did not delete the original.");attachment_details(ui,&released.action,released.evidence.as_ref());});
        }
        action
    }
}

fn attachment_details(
    ui: &mut egui::Ui,
    action: &RecoveryAction,
    evidence: Option<&AttachmentEvidence>,
) {
    let RecoveryAction::Attach {
        journal_path,
        publication,
        ..
    } = action
    else {
        return;
    };
    ui.small(format!("Retained original: {}", journal_path.display()));
    ui.small(format!(
        "Target session {} · expected revision {} · operation {}",
        publication.project_id.0, publication.expected_revision, publication.operation_id.0
    ));
    ui.small(format!(
        "Take {} · stream {} · manifest {}",
        publication.clip.source.take_id.0,
        publication.clip.source.stream_id,
        publication.clip.source.manifest_sha256
    ));
    ui.small(format!(
        "Source window {} + {} frames · placement {} at {} Hz · track {} · clip {}",
        publication.clip.range.start(),
        publication.clip.range.count(),
        publication.clip.position.frame,
        publication.clip.position.sample_rate,
        publication.track_id.0,
        publication.clip.id
    ));
    if let Some(evidence) = evidence {
        if let Some(inspection) = &evidence.inspection {
            ui.small(format!(
                "Retained review {} · original rows {} · unknown tail {}",
                inspection.inspection_sha256,
                inspection.original_rows_sha256,
                inspection.unknown_tail
            ));
        }
        if let Some(recovery) = &evidence.recovery {
            ui.small(format!(
                "Retained recovery receipt {} · reviewed {} · unknown tail {}",
                recovery.operation_id, recovery.reviewed_inspection_sha256, recovery.unknown_tail
            ));
        }
    }
}

fn state_text(state: &CaptureStageState) -> &'static str {
    match state {
        CaptureStageState::Uninspected => "Not inspected",
        CaptureStageState::Recording => "Unsealed recording",
        CaptureStageState::Interrupted => "Interrupted original",
        CaptureStageState::Sealed => "Sealed original",
        CaptureStageState::Owned => "Busy · live owner",
        CaptureStageState::Invalid => "Unavailable stage",
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    pub(crate) const STAGE: &str = "00000000-0000-4000-8000-000000000001";
    pub(crate) fn inspection(sealed: bool) -> Box<CaptureInspection> {
        Box::new(CaptureInspection {stage_id:STAGE.into(),journal_path:PathBuf::from("/synthetic/captures/original/take.sqlite3"),take_id:"retained-source".into(),descriptor_sha256:"a".repeat(64),inspection_sha256:"b".repeat(64),journal_sha256:"c".repeat(64),original_rows_sha256:"d".repeat(64),byte_length:40960,state:if sealed {CaptureStageState::Sealed}else{CaptureStageState::Interrupted},verified_chunks:2,verified_payload_bytes:6,chunk_index_sha256:"e".repeat(64),problems:serde_json::from_value(serde_json::json!([{"stream_id":"audio","sequence":1,"message":"Synthetic damaged tail retained"}])).unwrap(),unknown_tail:true,audio:vec![CaptureAudio {stream_id:"audio".into(),start_frame:100,frames:4,recorded_frames:3,gap_frames:1,sample_rate:48000,channels:1,encoding:serde_json::from_str("\"s16_le\"").unwrap(),layout:serde_json::from_str("\"interleaved\"").unwrap()}],manifest_sha256:sealed.then(||"f".repeat(64))})
    }
    pub(crate) fn scan() -> CaptureStageScan {
        CaptureStageScan {
            stages: vec![omniusong_store::CaptureStage {
                stage_id: STAGE.into(),
                journal_path: PathBuf::from("/synthetic/captures/original/take.sqlite3"),
                state: CaptureStageState::Uninspected,
                problem: None,
            }],
            problems: vec!["Synthetic redirected neighbor was ignored".into()],
            truncated: false,
        }
    }
    pub(crate) fn receipt(action: &RecoveryAction) -> Box<CaptureRecoveryReceipt> {
        let RecoveryAction::Recover {
            inspection,
            operation_id,
        } = action
        else {
            panic!("Expected a reviewed recovery");
        };
        Box::new(CaptureRecoveryReceipt {
            stage_id: inspection.stage_id.clone(),
            journal_path: inspection.journal_path.clone(),
            take_id: inspection.take_id.clone(),
            operation_id: operation_id.0.clone(),
            reviewed_inspection_sha256: inspection.inspection_sha256.clone(),
            manifest_sha256: "f".repeat(64),
            unknown_tail: true,
            audio: inspection.audio.clone(),
            replayed: false,
        })
    }
    pub(crate) fn recover(ui: &RecoveryUi) -> RecoveryAction {
        ui.recovery_action().unwrap()
    }
    pub(crate) fn attach(ui: &RecoveryUi, project_id: &str) -> RecoveryAction {
        RecoveryAction::ChooseAttachment(Box::new(ui.attachment_choice(project_id).unwrap()))
    }
    pub(crate) fn retry_attach(ui: &RecoveryUi) -> RecoveryAction {
        ui.attach_retry.clone().unwrap()
    }
    pub(crate) fn keep_local(ui: &mut RecoveryUi) -> bool {
        ui.keep_local()
    }
    pub(crate) fn release_rejected(ui: &mut RecoveryUi) -> bool {
        ui.release_rejected()
    }
    pub(crate) fn operation(action: &RecoveryAction) -> String {
        match action {
            RecoveryAction::Recover { operation_id, .. } => operation_id.0.clone(),
            RecoveryAction::ChooseAttachment(choice) => choice.operation_id.0.clone(),
            RecoveryAction::Attach { publication, .. } => publication.operation_id.0.clone(),
            _ => panic!("No durable operation"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};
    fn frame(
        ctx: &egui::Context,
        recovery: &mut RecoveryUi,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<RecoveryAction>) {
        let mut action = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 2400.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                action = recovery.ui(ui, true, Some("target-project"), false);
            },
        );
        output.textures_delta.clear();
        (output, action)
    }
    fn click(
        ctx: &egui::Context,
        recovery: &mut RecoveryUi,
        label: &str,
    ) -> Option<RecoveryAction> {
        let (output, none) = frame(ctx, recovery, vec![]);
        assert!(none.is_none());
        let tree = output.platform_output.accesskit_update.unwrap();
        let target = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .unwrap_or_else(|| panic!("Missing retained-capture action {label}"))
            .0;
        frame(
            ctx,
            recovery,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: tree.tree_id,
                    target_node: target,
                    data: None,
                },
            )],
        )
        .1
    }
    #[test]
    fn accessible_recovery_is_explicit_and_retries_preserve_review_and_publication() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut recovery = RecoveryUi::new();
        assert!(frame(&ctx, &mut recovery, vec![]).1.is_none());
        assert!(matches!(
            click(&ctx, &mut recovery, "Scan retained captures"),
            Some(RecoveryAction::Scan)
        ));
        recovery.scanned(scan());
        assert!(
            matches!(click(&ctx,&mut recovery,"Inspect selected capture"),Some(RecoveryAction::Inspect(id))if id==STAGE)
        );
        recovery.inspected(inspection(false)).unwrap();
        let original_review = serde_json::to_vec(&recovery.inspection).unwrap();
        let action = click(&ctx, &mut recovery, "Recover verified prefix").unwrap();
        let operation_id = operation(&action);
        recovery.started(&action);
        recovery.failed(&action);
        let retry = click(&ctx, &mut recovery, "Retry reviewed recovery").unwrap();
        assert_eq!(operation(&retry), operation_id);
        let Command::RecoverCapture {
            inspection: review, ..
        } = retry.command().unwrap()
        else {
            panic!()
        };
        assert_eq!(review.inspection_sha256, "b".repeat(64));
        recovery.recovered(receipt(&action)).unwrap();
        let (output, none) = frame(&ctx, &mut recovery, vec![]);
        assert!(none.is_none());
        let tree = output.platform_output.accesskit_update.unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        assert!(text.contains(&"Reviewed snapshot before recovery"));
        assert!(text.contains(&"Before recovery: no sealed manifest. The confirmed sealed manifest appears in the recovery result below."));
        assert!(text.contains(&"Confirmed recovery result · current sealed original"));
        assert!(
            !text.contains(&"No sealed manifest. Recovering requires a separate explicit action.")
        );
        assert_eq!(
            serde_json::to_vec(&recovery.inspection).unwrap(),
            original_review,
            "Relabelling must preserve every reviewed snapshot value"
        );
        let choice = click(
            &ctx,
            &mut recovery,
            "Attach selected audio to current session",
        )
        .unwrap();
        let id = operation(&choice);
        let RecoveryAction::ChooseAttachment(choice) = choice else {
            panic!()
        };
        let action = recovery
            .prepare_attachment(&choice, "target-project", 7, Path::new("/synthetic/studio"))
            .unwrap();
        let Command::AttachCapture { publication, .. } = action.command().unwrap() else {
            panic!()
        };
        assert_eq!(publication.operation_id.0, id);
        assert_eq!(publication.expected_revision, 7);
        assert_eq!(
            publication.clip.range,
            TakeRange::AudioFrames {
                start: 100,
                count: 4
            }
        );
        assert_eq!(
            publication.clip.position,
            TimelinePosition {
                frame: 0,
                sample_rate: 48000
            }
        );
        recovery.started(&action);
        recovery.failed(&action);
        assert!(recovery.blocks_session());
        let retry = click(&ctx, &mut recovery, "Retry retained attachment").unwrap();
        assert_eq!(
            retry.command().unwrap().kind(),
            action.command().unwrap().kind()
        );
        assert_eq!(operation(&retry), id);
        let Command::AttachCapture {
            publication: retried,
            ..
        } = retry.command().unwrap()
        else {
            panic!()
        };
        assert_eq!(retried, publication);
        recovery.attached();
        assert!(!recovery.blocks_session());
    }
    #[test]
    fn unknown_tail_gaps_raw_path_and_archive_exclusion_remain_visible() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut recovery = RecoveryUi::new();
        recovery.scanned(scan());
        recovery.inspected(inspection(false)).unwrap();
        let tree = frame(&ctx, &mut recovery, vec![])
            .0
            .platform_output
            .accesskit_update
            .unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        for needle in [
            "Project archives exclude them",
            "Unknown tail: yes",
            "/synthetic/captures/original/take.sqlite3",
            "100–104",
            "3 recorded · 1 gap frames",
            "Synthetic damaged tail retained",
            "No sealed manifest",
        ] {
            assert!(
                text.iter().any(|text| text.contains(needle)),
                "Missing disclosure {needle}"
            );
        }
    }
    #[test]
    fn mismatched_receipt_and_empty_recorded_audio_never_become_attachment_sources() {
        let mut recovery = RecoveryUi::new();
        recovery.scanned(scan());
        recovery.inspected(inspection(false)).unwrap();
        let action = recover(&recovery);
        recovery.started(&action);
        let mut wrong = receipt(&action);
        wrong.operation_id = "different-operation".into();
        assert!(recovery.recovered(wrong).is_err());
        assert!(!recovery.can_attach());
        assert!(recovery.recover_retry.is_some());
        let mut empty = inspection(true);
        empty.audio[0].recorded_frames = 0;
        recovery.inspected(empty).unwrap();
        assert!(!recovery.can_recover());
        assert!(!recovery.can_attach());
        assert!(recovery.attachment_choice("target-project").is_none());
    }

    #[test]
    fn keep_local_unlocks_session_but_preserves_exact_unresolved_intent_and_source_evidence() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut recovery = RecoveryUi::new();
        recovery.scanned(scan());
        recovery.inspected(inspection(false)).unwrap();
        let reviewed = recover(&recovery);
        recovery.started(&reviewed);
        recovery.recovered(receipt(&reviewed)).unwrap();
        let RecoveryAction::ChooseAttachment(choice) = attach(&recovery, "target-project") else {
            panic!()
        };
        let action = recovery
            .prepare_attachment(&choice, "target-project", 7, Path::new("/synthetic/studio"))
            .unwrap();
        let operation_id = operation(&action);
        recovery.started(&action);
        assert!(
            !recovery.keep_local(),
            "An active request cannot be suspended"
        );
        recovery.failed(&action);
        assert!(recovery.blocks_session());
        assert!(
            click(
                &ctx,
                &mut recovery,
                "Keep original local and return to session"
            )
            .is_none()
        );
        assert!(!recovery.blocks_session());
        assert!(recovery.attach_suspended);
        assert!(
            !recovery.release_rejected(),
            "An uncertain result is not a proven revision conflict"
        );
        recovery.scanned(scan());
        assert!(recovery.inspection.is_none());
        let evidence = recovery.attach_evidence.as_ref().unwrap();
        assert_eq!(
            evidence.inspection.as_ref().unwrap().original_rows_sha256,
            "d".repeat(64)
        );
        assert_eq!(
            evidence
                .recovery
                .as_ref()
                .unwrap()
                .reviewed_inspection_sha256,
            "b".repeat(64)
        );
        assert!(
            click(
                &ctx,
                &mut recovery,
                "Attach selected audio to current session"
            )
            .is_none()
        );
        let retry = click(&ctx, &mut recovery, "Retry retained attachment").unwrap();
        assert_eq!(operation(&retry), operation_id);
        let Command::AttachCapture { publication, .. } = retry.command().unwrap() else {
            panic!()
        };
        let Command::AttachCapture {
            publication: original,
            ..
        } = action.command().unwrap()
        else {
            panic!()
        };
        assert_eq!(publication, original);
        assert!(frame(&ctx, &mut recovery, vec![]).1.is_none());
    }

    #[test]
    fn explicit_known_conflict_release_retains_old_request_and_allows_only_new_user_choice() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut recovery = RecoveryUi::new();
        recovery.scanned(scan());
        recovery.inspected(inspection(true)).unwrap();
        let RecoveryAction::ChooseAttachment(choice) = attach(&recovery, "target-project") else {
            panic!()
        };
        let old = recovery
            .prepare_attachment(&choice, "target-project", 7, Path::new("/synthetic/studio"))
            .unwrap();
        recovery.started(&old);
        recovery.failed(&old);
        recovery.revision_conflict(&old);
        assert!(click(&ctx, &mut recovery, "Release rejected attachment choice").is_none());
        assert!(!recovery.blocks_session());
        assert!(recovery.attach_retry.is_none());
        assert_eq!(recovery.released.len(), 1);
        assert_eq!(operation(&recovery.released[0].action), operation(&old));
        assert!(
            recovery.released[0]
                .evidence
                .as_ref()
                .unwrap()
                .inspection
                .is_some()
        );
        assert!(frame(&ctx, &mut recovery, vec![]).1.is_none());
        let next = click(
            &ctx,
            &mut recovery,
            "Attach selected audio to current session",
        )
        .unwrap();
        assert_ne!(operation(&next), operation(&old));
        let RecoveryAction::ChooseAttachment(choice) = next else {
            panic!()
        };
        let next = recovery
            .prepare_attachment(&choice, "target-project", 8, Path::new("/synthetic/studio"))
            .unwrap();
        let Command::AttachCapture { publication, .. } = next.command().unwrap() else {
            panic!()
        };
        assert_eq!(publication.expected_revision, 8);
        let Command::AttachCapture {
            publication: old_publication,
            ..
        } = recovery.released[0].action.command().unwrap()
        else {
            panic!()
        };
        assert_eq!(old_publication.expected_revision, 7);
        assert_ne!(publication.operation_id, old_publication.operation_id);
    }
}
