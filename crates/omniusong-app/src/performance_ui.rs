//! Durable recorded-output choices. This module performs no filesystem or device work.
use eframe::egui;
use omniusong_core::{
    contracts::{OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_engine::{
    EngineCommand, LiveClock, LiveId, LiveReady, LiveScope, PerformancePhase, PerformanceStart,
    PerformanceStatus,
};
use omniusong_session::{
    Command, PerformanceFinishIntent, PerformanceIntent, PerformanceIntentReceipt, Reply,
    ServiceError,
};
use omniusong_store::TakePublication;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    List,
    Start,
    PrepareRelease,
    Finish,
    Attach,
    Release,
}
struct Task {
    command: Command,
    effect: Effect,
}
struct Card {
    intent: PerformanceIntent,
    receipt: Option<PerformanceIntentReceipt>,
    status: Option<PerformanceStatus>,
    activated: bool,
    started_here: bool,
    cancelled: bool,
    stopped: bool,
    engine_retry: Option<EngineCommand>,
    attached: bool,
    first_error: Option<String>,
    prior_unknown: bool,
    unstarted_rejected: bool,
}

pub(super) struct PerformanceUi {
    root: Option<PathBuf>,
    listed: bool,
    retained: Vec<PerformanceIntentReceipt>,
    card: Option<Card>,
    queued: Option<Task>,
    active: Option<Task>,
    retry: Option<Task>,
    engine: Option<EngineCommand>,
    pub(super) error: Option<String>,
    message: String,
    local_cancelled: bool,
}
impl PerformanceUi {
    pub(super) fn new() -> Self {
        Self { root: None, listed: false, retained: vec![], card: None, queued: None, active: None, retry: None, engine: None, error: None, local_cancelled:false, message: "Record keys/pads saves the procedural output and callback originals locally. No microphone is opened.".into() }
    }
    pub(super) fn set_root(&mut self, root: PathBuf) {
        self.root = Some(root);
    }
    pub(super) fn busy(&self) -> bool {
        self.active.is_some() || self.queued.is_some()
    }
    pub(super) fn has_card(&self) -> bool {
        self.card.is_some()
    }
    pub(super) fn blocks(&self) -> bool {
        self.card.is_some() || self.busy() && !self.only_list()
    }
    fn only_list(&self) -> bool {
        self.active
            .as_ref()
            .or(self.queued.as_ref())
            .is_some_and(|t| t.effect == Effect::List)
    }
    pub(super) fn engaged(&self) -> bool {
        self.card
            .as_ref()
            .is_some_and(|c| c.started_here && !c.stopped)
    }
    pub(super) fn pending_status(&self) -> bool {
        self.engaged()
    }
    fn queue_save(&mut self, effect: Effect) -> bool {
        if self.busy() || self.retry.is_some() {
            return false;
        }
        let Some(card) = &self.card else {
            return false;
        };
        self.queued = Some(Task {
            command: Command::SavePerformanceIntent {
                expected_revision: card.receipt.as_ref().map(|r| r.revision),
                intent: Box::new(card.intent.clone()),
            },
            effect,
        });
        true
    }
    pub(super) fn begin(
        &mut self,
        project: String,
        output: Option<String>,
        clock: LiveClock,
    ) -> Result<(), String> {
        if self.blocks() || self.busy() || self.retry.is_some() {
            return Err("Resolve the retained recording choice first.".into());
        }
        let root = self
            .root
            .as_ref()
            .filter(|p| p.is_absolute())
            .ok_or("The recording store path is not ready.")?;
        let start = PerformanceStart {
            id: LiveId::random(),
            output_device: output,
            gui_clock: clock,
        };
        let journal_path = root
            .join("captures")
            .join(start.directory_name())
            .join("take.sqlite3");
        let intent = PerformanceIntent {
            version: 1,
            start,
            project_id: ProjectId(project),
            journal_path,
            descriptor_sha256: None,
            finish: None,
            attachment: None,
            released: false,
        };
        self.card = Some(Card {
            intent,
            receipt: None,
            status: None,
            activated: false,
            started_here: false,
            cancelled: false,
            stopped: true,
            engine_retry: None,
            attached: false,
            first_error: None,
            prior_unknown: false,
            unstarted_rejected: false,
        });
        self.error = None;
        self.message = "Saving the recording choice before opening output. Keys remain closed until readiness is confirmed.".into();
        self.queue_save(Effect::Start);
        Ok(())
    }
    pub(super) fn take_command(&mut self) -> Option<Command> {
        if self.active.is_some() {
            return None;
        }
        if self.queued.is_none()
            && self.root.is_some()
            && !self.listed
            && self.card.is_none()
            && self.retry.is_none()
        {
            self.listed = true;
            self.queued = Some(Task {
                command: Command::ListPerformanceIntents,
                effect: Effect::List,
            });
        }
        let task = self.queued.take()?;
        let command = task.command.clone();
        self.active = Some(task);
        Some(command)
    }
    fn matches(a: &Command, b: &Command) -> bool {
        match (a, b) {
            (Command::ListPerformanceIntents, Command::ListPerformanceIntents) => true,
            (
                Command::SavePerformanceIntent {
                    expected_revision: a,
                    intent: ai,
                },
                Command::SavePerformanceIntent {
                    expected_revision: b,
                    intent: bi,
                },
            ) => a == b && ai == bi,
            (
                Command::AttachPerformance {
                    id: a,
                    revision: ar,
                },
                Command::AttachPerformance {
                    id: b,
                    revision: br,
                },
            ) => a == b && ar == br,
            _ => false,
        }
    }
    pub(super) fn command_target(&self, command: &Command) -> Option<&str> {
        let Command::AttachPerformance { id, revision } = command else {
            return None;
        };
        let task = self.active.as_ref()?;
        if !Self::matches(&task.command, command) {
            return None;
        }
        let card = self.card.as_ref()?;
        let receipt = card.receipt.as_ref()?;
        let attachment = receipt.intent.attachment.as_ref()?;
        (receipt.intent.start.id == *id
            && receipt.revision == *revision
            && attachment.project_id == card.intent.project_id)
            .then_some(attachment.project_id.0.as_str())
    }
    fn target_matches(intent: &PerformanceIntent, context: Option<&(String, u64)>) -> bool {
        context.is_some_and(|(id, _)| *id == intent.project_id.0)
    }
    fn review_matches(receipt: &PerformanceIntentReceipt, context: Option<&(String, u64)>) -> bool {
        receipt.intent.released
            || receipt.intent.attachment.is_none()
            || Self::target_matches(&receipt.intent, context)
    }
    fn retry_attachment(&mut self, can_attach: bool, context: Option<&(String, u64)>) {
        if self.busy() || self.retry.is_some() || !can_attach {
            return;
        }
        let Some(card) = &self.card else {
            return;
        };
        if !card.stopped || card.intent.released || !Self::target_matches(&card.intent, context) {
            return;
        }
        let Some(receipt) = &card.receipt else {
            return;
        };
        if receipt.intent.attachment.is_none() {
            return;
        }
        self.queued = Some(Task {
            command: Command::AttachPerformance {
                id: receipt.intent.start.id,
                revision: receipt.revision,
            },
            effect: Effect::Attach,
        });
    }
    pub(super) fn submission_failed(&mut self, command: &Command, error: String) {
        if !self
            .active
            .as_ref()
            .is_some_and(|t| Self::matches(&t.command, command))
        {
            return;
        }
        self.retry = self.active.take();
        self.error = Some(error);
        self.message = "This submission was not admitted. The exact recording choice is retained; an earlier uncertain attempt is not cancelled.".into();
    }
    pub(super) fn completed(&mut self, command: &Command, result: Result<&Reply, &ServiceError>) {
        if !self
            .active
            .as_ref()
            .is_some_and(|t| Self::matches(&t.command, command))
        {
            return;
        }
        let task = self.active.take().expect("matched task");
        match result {
            Err(error) => {
                if let Some(card) = &mut self.card {
                    card.prior_unknown |= error.outcome_unknown;
                    card.unstarted_rejected = task.effect == Effect::Start
                        && error.kind == omniusong_session::FailureKind::IntentRejected
                        && !error.outcome_unknown
                        && !card.prior_unknown
                        && card.receipt.is_none()
                        && !card.started_here;
                }
                self.error = Some(error.message.clone());
                self.message = if error.outcome_unknown { "Storage outcome is uncertain. Original and exact recording intent are retained; retry only by an explicit choice." } else { "The recording choice was rejected. Its exact intent and original are retained." }.into();
                self.retry = Some(task);
            }
            Ok(Reply::PerformanceIntents(receipts)) if task.effect == Effect::List => {
                self.retained = receipts.iter().take(32).cloned().collect();
                self.error = None;
            }
            Ok(Reply::PerformanceIntent(receipt)) => {
                let Command::SavePerformanceIntent {
                    intent,
                    expected_revision,
                } = &task.command
                else {
                    self.protocol(task);
                    return;
                };
                let revision_matches = match expected_revision {
                    None => receipt.revision == 1,
                    Some(previous) => {
                        receipt.revision == *previous
                            || previous.checked_add(1) == Some(receipt.revision)
                    }
                };
                if receipt.intent != **intent
                    || !(1..=16).contains(&receipt.revision)
                    || !revision_matches
                    || receipt.payload_sha256.len() != 64
                    || !receipt
                        .payload_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    self.protocol(task);
                    return;
                }
                let Some(card) = &mut self.card else {
                    self.protocol(task);
                    return;
                };
                if card.intent.start.id != receipt.intent.start.id {
                    self.protocol(task);
                    return;
                }
                if card.receipt.as_ref().is_some_and(|prior| {
                    receipt.revision < prior.revision
                        || receipt.revision == prior.revision && receipt.intent != prior.intent
                }) {
                    self.protocol(task);
                    return;
                }
                card.receipt = Some((**receipt).clone());
                self.retained
                    .retain(|r| r.intent.start.id != receipt.intent.start.id);
                self.retained.insert(0, (**receipt).clone());
                self.retained.truncate(32);
                self.error = None;
                match task.effect {
                    Effect::Start if !card.cancelled => {
                        let command = EngineCommand::StartPerformance {
                            request: card.intent.start.clone(),
                        };
                        card.started_here = true;
                        card.stopped = false;
                        card.engine_retry = Some(command.clone());
                        self.engine = Some(command);
                    }
                    Effect::Finish if !card.cancelled => {
                        if let Some(finish) = &card.intent.finish {
                            let command = EngineCommand::FinishPerformance {
                                start_id: card.intent.start.id,
                                scope: finish.scope,
                                finish_id: finish.id,
                            };
                            card.engine_retry = Some(command.clone());
                            self.engine = Some(command);
                        }
                    }
                    Effect::Attach => {
                        self.queued = Some(Task {
                            command: Command::AttachPerformance {
                                id: card.intent.start.id,
                                revision: receipt.revision,
                            },
                            effect: Effect::Attach,
                        });
                    }
                    Effect::Release => {
                        if card.stopped {
                            self.card = None;
                            self.message = "Recording choice retained locally. No original was deleted; session editing is available.".into();
                        }
                    }
                    Effect::PrepareRelease => {
                        card.intent.released = true;
                        self.queue_save(Effect::Release);
                    }
                    _ => {}
                }
            }
            Ok(Reply::Document { .. }) if task.effect == Effect::Attach => {
                if let Some(card) = &mut self.card {
                    card.attached = true;
                    card.intent.released = true;
                }
                self.message = "Recorded audio attached. Its complete original streams are retained; frame-zero placement is uncalibrated.".into();
                self.queue_save(Effect::Release);
            }
            Ok(_) => self.protocol(task),
        }
    }
    fn protocol(&mut self, task: Task) {
        if let Some(card) = &mut self.card {
            card.prior_unknown = true;
            card.unstarted_rejected = false;
        }
        self.error = Some("Recording storage completion did not match its exact choice. No audio action was inferred.".into());
        self.retry = Some(task);
    }
    fn dismiss_unstarted(&mut self) {
        if self.busy()
            || !self
                .retry
                .as_ref()
                .is_some_and(|t| t.effect == Effect::Start)
            || !self.card.as_ref().is_some_and(|c| {
                c.unstarted_rejected && !c.prior_unknown && c.receipt.is_none() && !c.started_here
            })
        {
            return;
        }
        self.card = None;
        self.retry = None;
        self.engine = None;
        self.local_cancelled = true;
        self.message="Unstarted local recording choice dismissed. This does not delete or cancel a stored intent or source; no engine Start was sent.".into();
    }
    pub(super) fn take_local_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.local_cancelled)
    }
    pub(super) fn take_engine(&mut self) -> Option<EngineCommand> {
        self.engine.take()
    }
    pub(super) fn engine_failed(&mut self, error: String) {
        if let Some(card) = &mut self.card {
            card.first_error.get_or_insert_with(|| error.clone());
        }
        self.error.get_or_insert(error);
        self.message = "Audio outcome is not confirmed. Keep the original stage; there is no automatic restart or rerender.".into();
    }
    pub(super) fn observe(
        &mut self,
        status: PerformanceStatus,
    ) -> Result<Option<LiveReady>, String> {
        let Some(card) = &mut self.card else {
            return Ok(None);
        };
        if !status.is_valid()
            || status.identity.start_id != card.intent.start.id
            || status.identity.capture.journal_path != card.intent.journal_path
            || status.identity.capture.take_id != card.intent.start.take_id()
        {
            return Err("Recorded output identity did not match the saved choice. Stop; the original stage remains retained.".into());
        }
        if let Some(prior) = &card.status {
            if status.revision < prior.revision {
                return Ok(None);
            }
            if status.revision == prior.revision {
                if &status != prior {
                    return Err("Recorded output changed at the same status revision. Stop before continuing.".into());
                }
                return Ok(None);
            }
        }
        if card
            .intent
            .descriptor_sha256
            .as_ref()
            .is_some_and(|s| status.identity.descriptor_sha256.as_ref() != Some(s))
            || status.ready.as_ref().is_some_and(|r| {
                r.gui_clock != card.intent.start.gui_clock
                    || r.scope.session_id != card.intent.start.id
                    || status.identity.descriptor_sha256.is_none()
                    || status.native_sample_format.is_none()
                    || card
                        .intent
                        .start
                        .output_device
                        .as_ref()
                        .is_some_and(|id| *id != r.output.id)
            })
            || status.finish.as_ref().is_some_and(|f| {
                card.intent
                    .finish
                    .as_ref()
                    .is_none_or(|saved| saved.id != f.id || saved.scope != f.scope)
            })
        {
            return Err("Recorded output identity did not match the saved choice. Stop; the original stage remains retained.".into());
        }
        if let Some(prior) = &card.status {
            if prior.ready.as_ref().is_some_and(|r| {
                status.ready.as_ref().is_none_or(|n| {
                    r.scope.worker_generation != n.scope.worker_generation
                        || r.scope.session_id != n.scope.session_id
                })
            }) {
                return Err("Recorded output changed worker or session identity.".into());
            }
            if prior
                .sealed
                .as_ref()
                .is_some_and(|sealed| status.sealed.as_ref() != Some(sealed))
            {
                return Err("A confirmed sealed original changed in a later status.".into());
            }
        }
        let ready = if !card.activated
            && !card.cancelled
            && status.phase == PerformancePhase::Recording
            && status.error.is_none()
        {
            status
                .ready
                .clone()
                .filter(|r| r.progress.state == omniusong_engine::LiveState::Running)
        } else {
            None
        };
        if ready.is_some() {
            card.activated = true;
            card.engine_retry = None;
        }
        if status.phase == PerformancePhase::Sealed {
            card.engine_retry = None;
        }
        if let Some(error) = &status.error {
            self.error.get_or_insert_with(|| error.clone());
            card.first_error.get_or_insert_with(|| error.clone());
        }
        if card.intent.descriptor_sha256.is_none() {
            card.intent.descriptor_sha256 = status.identity.descriptor_sha256.clone();
        }
        card.status = Some(status);
        Ok(ready)
    }
    pub(super) fn finish(&mut self, scope: LiveScope) {
        if self.busy() || self.retry.is_some() {
            self.error = Some(
                "Wait for the recording choice to finish saving before choosing Finish.".into(),
            );
            return;
        }
        let Some(card) = &mut self.card else {
            return;
        };
        if card.cancelled || !card.activated || card.intent.finish.is_some() {
            return;
        }
        card.intent.finish = Some(PerformanceFinishIntent {
            id: LiveId::random(),
            scope,
        });
        self.queue_save(Effect::Finish);
        self.message = "Saving the exact Finish choice. Human input and lease renewal are closed; held voices are cut only by the callback Finish.".into();
    }
    pub(super) fn can_finish(&self) -> bool {
        !self.busy()
            && self.retry.is_none()
            && self.card.as_ref().is_some_and(|c| {
                c.activated
                    && !c.cancelled
                    && !c.stopped
                    && c.intent.finish.is_none()
                    && c.status.as_ref().is_some_and(|s| {
                        s.phase == PerformancePhase::Recording && s.error.is_none()
                    })
            })
    }
    pub(super) fn cancel(&mut self) -> bool {
        let Some(card) = &mut self.card else {
            return false;
        };
        card.cancelled = true;
        card.engine_retry = None;
        self.engine = None;
        self.message = "Stop requested. Saving an intent does not reopen output; original evidence is retained without a promised final tail.".into();
        card.started_here && !card.stopped
    }
    pub(super) fn stopped(&mut self) {
        if let Some(card) = &mut self.card {
            card.stopped = true;
            card.engine_retry = None;
        }
    }
    pub(super) fn retired(&mut self, error: String) {
        self.cancel();
        self.stopped();
        self.engine_failed(error);
    }
    pub(super) fn wants_release_stop(&self) -> bool {
        self.card.as_ref().is_some_and(|c| {
            !c.stopped
                && c.status
                    .as_ref()
                    .is_some_and(|s| s.phase == PerformancePhase::Sealed)
        })
    }
    pub(super) fn status_request(&self) -> Option<EngineCommand> {
        let c = self.card.as_ref()?;
        (c.started_here && !c.stopped).then_some(EngineCommand::PerformanceStatus {
            start_id: c.intent.start.id,
        })
    }
    fn release(&mut self) {
        if self.busy() || self.retry.is_some() {
            return;
        }
        let Some(card) = &mut self.card else {
            return;
        };
        if !card.stopped || card.intent.attachment.is_some() {
            return;
        }
        if card
            .receipt
            .as_ref()
            .is_some_and(|r| r.intent == card.intent)
        {
            card.intent.released = true;
            self.queue_save(Effect::Release);
        } else {
            self.queue_save(Effect::PrepareRelease);
        }
    }
    fn attach(&mut self, project: &str, revision: u64) {
        if self.busy() || self.retry.is_some() {
            return;
        }
        let Some(card) = &mut self.card else {
            return;
        };
        if !card.stopped || card.intent.attachment.is_some() || card.intent.project_id.0 != project
        {
            return;
        }
        let Some(take) = card.status.as_ref().and_then(|s| s.sealed.as_ref()) else {
            return;
        };
        card.intent.attachment = Some(TakePublication {
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            project_id: card.intent.project_id.clone(),
            expected_revision: revision,
            track_id: TrackId(uuid::Uuid::new_v4().to_string()),
            track_name: "Recorded keys and pads".into(),
            clip: TakeClip {
                id: uuid::Uuid::new_v4().to_string(),
                source: TakeReference {
                    take_id: TakeId(take.take_id.clone()),
                    manifest_sha256: take.manifest_sha256.clone(),
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 0,
                    count: take.frames,
                },
                position: TimelinePosition {
                    frame: 0,
                    sample_rate: take.sample_rate,
                },
                fades: None,
                edit_source: None,
            },
        });
        self.queue_save(Effect::Attach);
    }
    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        can_attach: bool,
        allow_review: bool,
        context: Option<&(String, u64)>,
    ) {
        ui.strong("Recorded keys and pads · output only");
        ui.small(&self.message);
        if let Some(card) = &self.card {
            if let Some(error) = &card.first_error {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("First audio failure: {error}"),
                );
            }
            if let Some(status) = &card.status {
                ui.label(format!("{:?}", status.phase));
                if let Some(ready) = &status.ready {
                    ui.label(format!(
                        "{} · {} Hz · {} channels · {}",
                        ready.output.name,
                        ready.sample_rate,
                        ready.channels,
                        status
                            .native_sample_format
                            .as_deref()
                            .unwrap_or("format pending")
                    ));
                }
                if let Some(w) = &status.writer {
                    ui.label(format!(
                        "Rendered {} frames · durable {} frames · {} complete callbacks",
                        w.complete_frames, w.durable_frames, w.complete_callbacks
                    ));
                }
                if status.sealed.is_some() {
                    ui.strong(if card.attached {
                        "Original attached"
                    } else {
                        "Original saved locally · not attached"
                    });
                } else if matches!(
                    status.phase,
                    PerformancePhase::Failed
                        | PerformancePhase::Interrupted
                        | PerformancePhase::SealUncertain
                ) {
                    ui.label("Original retained · final tail unknown. Performance-aware inspection/recovery is not available here yet.");
                }
            } else {
                ui.label("Saved recording choice · source has not been inspected. No recording is resumed.");
            }
            ui.small("Parent transfer-loss count: unavailable / unknown. Durable frames do not prove that every GUI event was saved.");
            let attach = card.stopped
                && card.intent.attachment.is_none()
                && card.status.as_ref().is_some_and(|s| s.sealed.is_some())
                && context.is_some_and(|(id, _)| *id == card.intent.project_id.0);
            let release = card.stopped && card.intent.attachment.is_none();
            let retry_engine =
                card.engine_retry.is_some() && !card.cancelled && self.error.is_some();
            let retry_attachment = card.stopped
                && card.receipt.is_some()
                && card.intent.attachment.is_some()
                && !card.intent.released
                && Self::target_matches(&card.intent, context);
            let close_review = card.intent.released && !card.started_here;
            let dismiss_unstarted = card.unstarted_rejected
                && !card.prior_unknown
                && card.receipt.is_none()
                && !card.started_here
                && !self.busy();
            let idle = !self.busy() && self.retry.is_none();
            let mut chosen_attach = false;
            let mut chosen_release = false;
            let mut chosen_engine = false;
            let mut chosen_saved_attachment = false;
            let mut chosen_close = false;
            let mut chosen_dismiss = false;
            ui.horizontal_wrapped(|ui| {
                chosen_attach = ui
                    .add_enabled(
                        idle && can_attach && attach,
                        egui::Button::new("Attach recording to session"),
                    )
                    .clicked();
                chosen_release = ui
                    .add_enabled(
                        idle && release,
                        egui::Button::new("Keep recording locally and return to session"),
                    )
                    .clicked();
                chosen_engine = ui
                    .add_enabled(
                        idle && retry_engine,
                        egui::Button::new("Retry exact audio choice"),
                    )
                    .clicked();
                chosen_saved_attachment = ui
                    .add_enabled(
                        idle && can_attach && retry_attachment,
                        egui::Button::new("Retry stored recording attachment"),
                    )
                    .clicked();
                chosen_close = ui
                    .add_enabled(
                        idle && close_review,
                        egui::Button::new("Close recording review"),
                    )
                    .clicked();
                chosen_dismiss = ui
                    .add_enabled(
                        dismiss_unstarted,
                        egui::Button::new("Cancel unstarted recording choice"),
                    )
                    .clicked();
            });
            ui.collapsing("Recording source and intent",|ui| {
                ui.monospace(card.intent.journal_path.display().to_string());
                ui.monospace(card.intent.start.take_id());
                if let Some(sha)=&card.intent.descriptor_sha256 { ui.monospace(format!("Descriptor {sha}")); }
                if let Some(s)=&card.status && let Some(t)=&s.sealed { ui.monospace(format!("Manifest {}",t.manifest_sha256)); }
                if let Some(a)=&card.intent.attachment { ui.monospace(format!("Attachment {} · session {} r{}",a.operation_id.0,a.project_id.0,a.expected_revision)); }
                if card.intent.attachment.is_some() && !card.attached { ui.small("The exact attachment choice is retained. Conflicting or uncertain choices cannot be superseded here; source/conflict review is a later workflow."); }
                ui.small("Original GUI observations and callback frames are uncalibrated. Attachment selects audio at frame zero; original event streams remain preserved. Editable-note derivation is not implemented.");
            });
            if chosen_attach && let Some((id, revision)) = context {
                self.attach(id, *revision);
            }
            if chosen_release {
                self.release();
            }
            if chosen_engine && let Some(card) = &self.card {
                self.engine = card.engine_retry.clone();
            }
            if chosen_saved_attachment {
                self.retry_attachment(can_attach, context);
            }
            if chosen_close {
                self.card = None;
            }
            if chosen_dismiss {
                self.dismiss_unstarted();
            }
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let retry_target_ok = self.retry.as_ref().is_none_or(|task| {
            !matches!(task.command, Command::AttachPerformance { .. })
                || can_attach
                    && self
                        .card
                        .as_ref()
                        .is_some_and(|card| Self::target_matches(&card.intent, context))
        });
        if self.retry.is_some()
            && retry_target_ok
            && !self.busy()
            && ui.button("Retry exact recording storage choice").clicked()
        {
            self.queued = self.retry.take();
        }
        if self.card.is_none() {
            let mut selected = None;
            if self
                .retained
                .iter()
                .any(|receipt| !Self::review_matches(receipt, context))
            {
                ui.small("Open the original target session before reviewing or retrying its pending recording attachment. Choosing a different target never changes the saved request.");
            }
            egui::CollapsingHeader::new("Retained recording choices").show(ui,|ui| {
                ui.small("Review only. Listing an intent does not verify, start, resume, finish or recover its source.");
                egui::ScrollArea::vertical().max_height(150.0).show_rows(ui,24.0,self.retained.len(),|ui,range| {
                    for i in range { let r=&self.retained[i]; if ui.add_enabled(allow_review && Self::review_matches(r,context) && !self.busy() && self.retry.is_none(),egui::Button::new(format!("Review {} · choice r{}{}",r.intent.start.take_id(),r.revision,if r.intent.released {" · released"}else{""}))).clicked(){selected=Some(i);} }
                });
            });
            if let Some(i) = selected {
                let receipt = self.retained[i].clone();
                self.card = Some(Card {
                    intent: receipt.intent.clone(),
                    receipt: Some(receipt),
                    status: None,
                    activated: false,
                    started_here: false,
                    cancelled: true,
                    stopped: true,
                    engine_retry: None,
                    attached: false,
                    first_error: None,
                    prior_unknown: false,
                    unstarted_rejected: false,
                });
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    pub(crate) fn receipt(command: &Command, revision: u64) -> Reply {
        let Command::SavePerformanceIntent { intent, .. } = command else {
            panic!("expected saved intent")
        };
        Reply::PerformanceIntent(Box::new(PerformanceIntentReceipt {
            revision,
            intent: (**intent).clone(),
            payload_sha256: "a".repeat(64),
        }))
    }
    pub(crate) fn retained_attachment() -> PerformanceIntentReceipt {
        let start = PerformanceStart {
            id: LiveId([8; 16]),
            output_device: None,
            gui_clock: LiveClock {
                domain: "retained-gui".into(),
                epoch: "retained-clock".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
        };
        let target = ProjectId("synthetic-project".into());
        let intent = PerformanceIntent {
            version: 1,
            journal_path: PathBuf::from("/synthetic/performance-store/captures")
                .join(start.directory_name())
                .join("take.sqlite3"),
            descriptor_sha256: Some("b".repeat(64)),
            finish: Some(PerformanceFinishIntent {
                id: LiveId([9; 16]),
                scope: LiveScope {
                    worker_generation: LiveId([1; 16]),
                    session_id: start.id,
                    panic_generation: 1,
                },
            }),
            attachment: Some(TakePublication {
                operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
                project_id: target.clone(),
                expected_revision: 7,
                track_id: TrackId(uuid::Uuid::new_v4().to_string()),
                track_name: "Recorded keys and pads".into(),
                clip: TakeClip {
                    id: uuid::Uuid::new_v4().to_string(),
                    source: TakeReference {
                        take_id: TakeId(start.take_id()),
                        manifest_sha256: "d".repeat(64),
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames { start: 0, count: 4 },
                    position: TimelinePosition {
                        frame: 0,
                        sample_rate: 48_000,
                    },
                    fades: None,
                    edit_source: None,
                },
            }),
            start,
            project_id: target,
            released: false,
        };
        intent.validate().unwrap();
        PerformanceIntentReceipt {
            revision: 3,
            intent,
            payload_sha256: "a".repeat(64),
        }
    }
    pub(crate) fn status(intent: &PerformanceIntent, revision: u64) -> PerformanceStatus {
        let mut ready = crate::live_ui::test_support::ready(intent.start.gui_clock.clone());
        ready.scope.session_id = intent.start.id;
        // Rebind the whole progress fixture, including its one-use credit.
        // Merely changing progress.scope leaves credit.scope on the old session.
        ready.progress = crate::live_ui::test_support::progress(ready.scope);
        if let Some(id) = &intent.start.output_device {
            ready.output.id = id.clone();
        }
        assert!(
            ready.is_valid(),
            "performance fixture readiness and its lease share one exact scope"
        );
        PerformanceStatus {
            revision,
            identity: omniusong_engine::PerformanceIdentity {
                start_id: intent.start.id,
                capture: omniusong_engine::CaptureIdentity {
                    journal_path: intent.journal_path.clone(),
                    take_id: intent.start.take_id(),
                },
                descriptor_sha256: Some("b".repeat(64)),
            },
            phase: PerformancePhase::Recording,
            ready: Some(ready.clone()),
            native_sample_format: Some("f32".into()),
            live: Some(ready.progress),
            writer: None,
            finish: None,
            cut: None,
            native: Default::default(),
            sealed: None,
            error: None,
        }
    }
    pub(crate) fn sealed(intent: &PerformanceIntent, revision: u64) -> PerformanceStatus {
        use omniusong_engine::{LiveFinishFence, LiveFinishPolicy, NativeRetirementStatus};
        let mut s = status(intent, revision);
        let choice = intent.finish.as_ref().expect("saved Finish");
        let fence = LiveFinishFence {
            id: choice.id,
            scope: choice.scope,
            accepted_sequence: None,
        };
        let cut = omniusong_engine::performance_tap::TapFinish {
            fence,
            callback_index: 1,
            cut_frame: 4,
            voices_cut: 0,
            release_tails_cut: 0,
            policy: LiveFinishPolicy::CutAtCompleteCallbackEnd,
        };
        s.phase = PerformancePhase::Sealed;
        s.finish = Some(fence);
        s.cut = Some(cut);
        s.native = NativeRetirementStatus {
            pause_succeeded: true,
            renderer_destroyed: true,
            error_destroyed: true,
            producer_destroyed: true,
        };
        s.writer=Some(omniusong_engine::performance_writer::WriterProgress {
            received_packets:4,complete_callbacks:1,complete_frames:4,durable_callbacks:1,durable_frames:4,
            durable_prefixes:serde_json::from_value(serde_json::Value::Array(omniusong_engine::performance_records::STREAM_IDS.iter().enumerate().map(|(i,id)|serde_json::json!({"stream_id":id,"next_sequence":if i==0 {1}else{0},"source_end":{"unit":if i==0 {"audio_frames"}else{"events"},"end":if i==0 {4}else{0}},"recorded_units":if i==0 {4}else{0},"gap_units":0,"gap_chunks":0})).collect())).unwrap(),
            checkpoint_sha256:Some("c".repeat(64)),first_error:None,capture_fault:None,capture_fault_pending:false,finish_cut:Some(cut),
        });
        s.sealed = Some(omniusong_engine::CapturedTake {
            journal_path: intent.journal_path.clone(),
            take_id: intent.start.take_id(),
            manifest_sha256: "d".repeat(64),
            frames: 4,
            sample_rate: 48_000,
            channels: 2,
            gap_frames: 0,
        });
        assert!(s.is_valid());
        s
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};
    fn frame(
        p: &mut PerformanceUi,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        context: &(String, u64),
    ) -> egui::FullOutput {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1200.0),
                )),
                time: Some(ctx.cumulative_frame_nr() as f64 / 30.0),
                events,
                ..Default::default()
            },
            |ui| p.ui(ui, true, true, Some(context)),
        );
        out.textures_delta.clear();
        out
    }
    fn click(p: &mut PerformanceUi, ctx: &egui::Context, label: &str, context: &(String, u64)) {
        let mut out = frame(p, ctx, vec![], context);
        for _ in 0..8 {
            out = frame(p, ctx, vec![], context);
        }
        let tree = out.platform_output.accesskit_update.expect("AX tree");
        let target = tree
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some(label))
            .unwrap_or_else(|| panic!("Missing {label}"))
            .0;
        frame(
            p,
            ctx,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: tree.tree_id,
                    target_node: target,
                    data: None,
                },
            )],
            context,
        );
    }
    fn restarted(receipt: PerformanceIntentReceipt) -> PerformanceUi {
        let mut p = PerformanceUi::new();
        p.set_root(PathBuf::from("/synthetic/performance-store"));
        let list = p.take_command().unwrap();
        p.completed(&list, Ok(&Reply::PerformanceIntents(vec![receipt])));
        p
    }
    #[test]
    fn restarted_pending_attachment_review_requires_original_target_and_never_starts_engine() {
        let receipt = retained_attachment();
        let original = receipt.clone();
        let mut p = restarted(receipt);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let wrong = ("foreign-project".into(), 99);
        click(&mut p, &ctx, "Retained recording choices", &wrong);
        let label = format!("Review {} · choice r3", original.intent.start.take_id());
        click(&mut p, &ctx, &label, &wrong);
        assert!(p.card.is_none());
        assert!(!p.blocks());
        assert!(p.take_engine().is_none());
        let target = (original.intent.project_id.0.clone(), 42);
        click(&mut p, &ctx, &label, &target);
        assert!(p.has_card());
        assert!(p.take_engine().is_none());
        assert!(p.take_command().is_none());
        click(&mut p, &ctx, "Retry stored recording attachment", &target);
        let command = p.take_command().expect("explicit stored attachment");
        assert!(
            matches!(&command,Command::AttachPerformance {id,revision} if *id==original.intent.start.id && *revision==original.revision)
        );
        assert_eq!(
            p.command_target(&command),
            Some(original.intent.project_id.0.as_str())
        );
        assert_eq!(
            p.card.as_ref().unwrap().intent.attachment,
            original.intent.attachment
        );
        assert!(
            p.take_engine().is_none(),
            "no Start, Finish, rerender or new publication"
        );
    }
    #[test]
    fn stored_attachment_target_helper_rejects_stale_or_unmatched_commands() {
        let receipt = retained_attachment();
        let mut p = restarted(receipt.clone());
        let command = Command::AttachPerformance {
            id: receipt.intent.start.id,
            revision: receipt.revision,
        };
        assert!(
            p.command_target(&command).is_none(),
            "listing is not admission"
        );
        p.card = Some(Card {
            intent: receipt.intent.clone(),
            receipt: Some(receipt.clone()),
            status: None,
            activated: false,
            started_here: false,
            cancelled: true,
            stopped: true,
            engine_retry: None,
            attached: false,
            first_error: None,
            prior_unknown: false,
            unstarted_rejected: false,
        });
        p.retry_attachment(true, Some(&("foreign-project".into(), 10)));
        assert!(p.take_command().is_none());
        p.retry_attachment(true, Some(&(receipt.intent.project_id.0.clone(), 10)));
        let admitted = p.take_command().unwrap();
        assert_eq!(
            p.command_target(&admitted),
            Some(receipt.intent.project_id.0.as_str())
        );
        assert!(
            p.command_target(&Command::AttachPerformance {
                id: receipt.intent.start.id,
                revision: receipt.revision + 1
            })
            .is_none()
        );
        assert!(p.command_target(&Command::ListPerformanceIntents).is_none());
    }
    fn prepared() -> PerformanceUi {
        let mut p = PerformanceUi::new();
        p.set_root(PathBuf::from("/synthetic/performance-store"));
        p.begin(
            "synthetic-project".into(),
            None,
            LiveClock {
                domain: "gui-test".into(),
                epoch: "original-clock".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
        )
        .unwrap();
        p
    }
    #[test]
    fn fresh_known_prewrite_start_rejection_can_be_dismissed_but_prior_unknown_cannot() {
        for prior_unknown in [false, true] {
            let mut p = prepared();
            let first = p.take_command().unwrap();
            if prior_unknown {
                p.completed(
                    &first,
                    Err(&ServiceError::backend("first outcome unknown", true)),
                );
                p.queued = p.retry.take();
                assert!(PerformanceUi::matches(&p.take_command().unwrap(), &first));
            }
            let rejection = ServiceError {
                kind: omniusong_session::FailureKind::IntentRejected,
                message: "All32 recording choices are retained".into(),
                outcome_unknown: false,
            };
            p.completed(&first, Err(&rejection));
            p.dismiss_unstarted();
            if prior_unknown {
                assert!(p.has_card());
                assert!(p.blocks());
                assert!(!p.take_local_cancelled());
                assert!(PerformanceUi::matches(
                    &first,
                    &p.retry.as_ref().unwrap().command
                ));
            } else {
                assert!(!p.has_card());
                assert!(!p.blocks());
                assert!(p.take_local_cancelled());
                assert!(p.take_engine().is_none());
                assert!(p.retry.is_none());
                p.begin(
                    "synthetic-project".into(),
                    None,
                    LiveClock {
                        domain: "fresh-gui".into(),
                        epoch: "fresh-clock".into(),
                        ticks_per_second_numerator: 1,
                        ticks_per_second_denominator: 1,
                    },
                )
                .unwrap();
                let second = p.take_command().unwrap();
                assert!(
                    !PerformanceUi::matches(&first, &second),
                    "a later new choice is explicit"
                );
                assert!(p.take_engine().is_none());
            }
        }
    }
    fn save(p: &mut PerformanceUi, revision: u64) -> Command {
        let command = p.take_command().expect("queued save");
        p.completed(&command, Ok(&receipt(&command, revision)));
        command
    }
    #[test]
    fn durable_start_before_audio_and_stop_cancels_late_save_continuation() {
        let mut p = prepared();
        assert!(p.take_engine().is_none());
        let command = p.take_command().unwrap();
        let Command::SavePerformanceIntent {
            expected_revision,
            intent,
        } = &command
        else {
            panic!()
        };
        assert_eq!(*expected_revision, None);
        assert_eq!(
            intent.journal_path,
            PathBuf::from("/synthetic/performance-store/captures")
                .join(intent.start.directory_name())
                .join("take.sqlite3")
        );
        assert!(!p.cancel(), "no engine Start has been admitted");
        p.completed(&command, Ok(&receipt(&command, 1)));
        assert!(p.take_engine().is_none());
        assert!(!p.engaged());
        assert!(p.blocks(), "the original choice remains reviewable");
        p.release();
        save(&mut p, 2);
        assert!(!p.has_card());
    }
    #[test]
    fn unknown_storage_outcome_retries_exact_bytes_and_never_generates_another_start() {
        let mut p = prepared();
        let first = p.take_command().unwrap();
        p.completed(
            &first,
            Err(&ServiceError::backend("lost storage acknowledgment", true)),
        );
        assert!(p.take_engine().is_none());
        assert!(p.take_command().is_none());
        let retry = p.retry.take().unwrap();
        assert!(PerformanceUi::matches(&first, &retry.command));
        p.queued = Some(retry);
        let again = p.take_command().unwrap();
        p.submission_failed(&again, "channel full; retry not admitted".into());
        assert!(PerformanceUi::matches(
            &first,
            &p.retry.as_ref().unwrap().command
        ));
        p.queued = p.retry.take();
        let last = p.take_command().unwrap();
        p.completed(&last, Ok(&receipt(&last, 1)));
        assert!(matches!(
            p.take_engine(),
            Some(EngineCommand::StartPerformance { .. })
        ));
        assert!(p.take_engine().is_none());
    }
    #[test]
    fn readiness_activates_once_and_newer_fault_cannot_be_replaced_by_old_ready() {
        let mut p = prepared();
        save(&mut p, 1);
        p.take_engine();
        let intent = p.card.as_ref().unwrap().intent.clone();
        let ready = status(&intent, 2);
        assert!(ready.is_valid());
        assert!(p.observe(ready.clone()).unwrap().is_some());
        assert!(p.observe(ready.clone()).unwrap().is_none());
        let mut fault = ready.clone();
        fault.revision = 4;
        fault.phase = PerformancePhase::Failed;
        fault.error = Some("actual earlier callback failure".into());
        p.observe(fault.clone()).unwrap();
        assert!(p.observe(ready).unwrap().is_none());
        assert_eq!(p.card.as_ref().unwrap().status.as_ref(), Some(&fault));
        let mut before_ready = prepared();
        save(&mut before_ready, 1);
        before_ready.take_engine();
        let intent = before_ready.card.as_ref().unwrap().intent.clone();
        let mut failed = status(&intent, 3);
        failed.phase = PerformancePhase::Failed;
        failed.error = Some("fault before handoff".into());
        assert!(before_ready.observe(failed).unwrap().is_none());
        assert!(before_ready.observe(status(&intent, 2)).unwrap().is_none());
        assert!(!before_ready.card.as_ref().unwrap().activated);
    }
    #[test]
    fn finish_is_saved_before_engine_and_stop_preempts_pending_finish_save() {
        let mut p = prepared();
        save(&mut p, 1);
        p.take_engine();
        let intent = p.card.as_ref().unwrap().intent.clone();
        let ready = p.observe(status(&intent, 2)).unwrap().unwrap();
        p.finish(ready.scope);
        assert!(p.take_engine().is_none());
        let command = p.take_command().unwrap();
        let Command::SavePerformanceIntent { intent, .. } = &command else {
            panic!()
        };
        let original = intent.finish.clone().unwrap();
        assert!(p.cancel());
        p.completed(&command, Ok(&receipt(&command, 2)));
        assert!(p.take_engine().is_none());
        assert_eq!(p.card.as_ref().unwrap().intent.finish, Some(original));
        assert!(
            p.engaged(),
            "only a matching Stop acknowledgment releases lifetime"
        );
        p.stopped();
        assert!(!p.engaged());
    }
    #[test]
    fn stopped_original_metadata_is_saved_before_release_and_card_clears_only_on_ack() {
        let mut p = prepared();
        save(&mut p, 1);
        p.take_engine();
        let intent = p.card.as_ref().unwrap().intent.clone();
        p.observe(status(&intent, 2)).unwrap();
        p.cancel();
        p.stopped();
        p.release();
        let metadata = p.take_command().unwrap();
        let Command::SavePerformanceIntent { intent, .. } = &metadata else {
            panic!()
        };
        assert!(!intent.released);
        assert!(intent.descriptor_sha256.is_some());
        p.completed(&metadata, Ok(&receipt(&metadata, 2)));
        let release = p.take_command().unwrap();
        let Command::SavePerformanceIntent { intent, .. } = &release else {
            panic!()
        };
        assert!(intent.released);
        assert!(p.has_card());
        p.completed(&release, Ok(&receipt(&release, 3)));
        assert!(!p.has_card());
        assert!(p.take_engine().is_none());
    }
    #[test]
    fn restart_lists_once_without_start_finish_source_verification_or_automatic_attachment() {
        let mut p = PerformanceUi::new();
        assert!(
            p.take_command().is_none(),
            "unconfigured test/audio owners must not start storage work"
        );
        p.set_root(PathBuf::from("/synthetic/performance-store"));
        let list = p.take_command().unwrap();
        assert!(matches!(list, Command::ListPerformanceIntents));
        assert!(p.take_command().is_none());
        let source = prepared();
        let intent = source.card.unwrap().intent;
        p.completed(
            &list,
            Ok(&Reply::PerformanceIntents(vec![PerformanceIntentReceipt {
                revision: 1,
                intent,
                payload_sha256: "c".repeat(64),
            }])),
        );
        assert_eq!(p.retained.len(), 1);
        assert!(p.card.is_none());
        assert!(p.take_engine().is_none());
        assert!(p.take_command().is_none());
        assert!(!p.blocks());
    }
    #[test]
    fn wrong_source_or_same_revision_changed_status_never_activates_recording() {
        let mut p = prepared();
        save(&mut p, 1);
        p.take_engine();
        let intent = p.card.as_ref().unwrap().intent.clone();
        let mut wrong = status(&intent, 2);
        wrong.identity.capture.journal_path.push("foreign");
        assert!(p.observe(wrong).is_err());
        assert!(!p.card.as_ref().unwrap().activated);
        let good = status(&intent, 2);
        p.observe(good.clone()).unwrap();
        let mut changed = good;
        changed.error = Some("different payload".into());
        changed.phase = PerformancePhase::Failed;
        assert!(p.observe(changed).is_err());
    }
    #[test]
    fn sealing_only_retains_original_explicit_attachment_is_persisted_and_failure_cannot_release_it()
     {
        let mut p = prepared();
        save(&mut p, 1);
        p.take_engine();
        let initial = p.card.as_ref().unwrap().intent.clone();
        let ready = p.observe(status(&initial, 2)).unwrap().unwrap();
        p.finish(ready.scope);
        save(&mut p, 2);
        p.take_engine();
        let intent = p.card.as_ref().unwrap().intent.clone();
        p.observe(sealed(&intent, 3)).unwrap();
        assert!(
            p.take_command().is_none(),
            "sealing never attaches a project"
        );
        p.attach("synthetic-project", 7);
        assert!(p.take_command().is_none(), "must wait for exact Stop ACK");
        p.stopped();
        p.attach("synthetic-project", 7);
        let persist = p.take_command().unwrap();
        let Command::SavePerformanceIntent { intent, .. } = &persist else {
            panic!()
        };
        let publication = intent.attachment.clone().unwrap();
        assert_eq!(publication.expected_revision, 7);
        assert_eq!(
            publication.clip.range,
            TakeRange::AudioFrames { start: 0, count: 4 }
        );
        assert_eq!(
            publication.clip.position,
            TimelinePosition {
                frame: 0,
                sample_rate: 48_000
            }
        );
        assert_eq!(publication.clip.source.stream_id, "audio");
        assert!(publication.clip.edit_source.is_none());
        p.completed(&persist, Ok(&receipt(&persist, 3)));
        let attach = p.take_command().unwrap();
        assert!(matches!(
            attach,
            Command::AttachPerformance { revision: 3, .. }
        ));
        p.completed(
            &attach,
            Err(&ServiceError::backend("attachment outcome unknown", true)),
        );
        p.release();
        assert!(p.take_command().is_none());
        assert_eq!(
            p.card.as_ref().unwrap().intent.attachment.as_ref(),
            Some(&publication)
        );
        assert!(PerformanceUi::matches(
            &attach,
            &p.retry.as_ref().unwrap().command
        ));
        assert!(p.blocks());
    }
}
