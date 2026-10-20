//! Nonblocking native-audio UI state. Device/process/file work belongs to the engine actor.

use crate::live_ui::LiveUi;
use eframe::egui;
use omniusong_core::{
    contracts::{OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_engine::{
    CaptureIdentity, CapturedTake, DeviceInventory, EngineClient, EngineCommand, EngineReply,
    EngineResult, EngineStatus, EngineUpdate, LiveBatch, LiveQueued, LiveScope, LiveSubmitError,
    UrgentTicket,
};
use omniusong_store::TakePublication;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub(super) trait EnginePort {
    fn try_submit(&mut self, command: EngineCommand) -> Result<(), String>;
    fn try_poll(&mut self) -> Option<EngineReply>;
    fn is_busy(&self) -> bool;
    fn is_retired(&self) -> bool;
    fn try_push_live(&mut self, batch: LiveBatch) -> Result<LiveQueued, LiveSubmitError>;
    fn request_stop(&mut self) -> Result<UrgentTicket, String>;
    fn request_panic_live(&mut self, scope: LiveScope) -> Result<UrgentTicket, LiveSubmitError>;
    fn try_poll_update(&mut self) -> Option<EngineUpdate>;
}
impl EnginePort for EngineClient {
    fn try_submit(&mut self, command: EngineCommand) -> Result<(), String> {
        EngineClient::try_submit(self, command)
    }
    fn try_poll(&mut self) -> Option<EngineReply> {
        EngineClient::try_poll(self)
    }
    fn is_busy(&self) -> bool {
        EngineClient::is_busy(self)
    }
    fn is_retired(&self) -> bool {
        EngineClient::is_retired(self)
    }
    fn try_push_live(&mut self, batch: LiveBatch) -> Result<LiveQueued, LiveSubmitError> {
        EngineClient::try_push_live(self, batch)
    }
    fn request_stop(&mut self) -> Result<UrgentTicket, String> {
        EngineClient::request_stop(self)
    }
    fn request_panic_live(&mut self, scope: LiveScope) -> Result<UrgentTicket, LiveSubmitError> {
        EngineClient::request_panic_live(self, scope)
    }
    fn try_poll_update(&mut self) -> Option<EngineUpdate> {
        EngineClient::try_poll_update(self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Unavailable,
    StartingPlayback,
    StartingLive,
    Live,
    Playing,
    StartingCapture,
    Capturing,
    Finishing,
    Uncertain,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    Devices,
    Play,
    Live,
    Stop,
    Record,
    Finish,
    Status,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Stop,
    Finish,
}
struct CaptureTarget {
    project_id: String,
    revision: u64,
}
struct SealedCapture {
    take: CapturedTake,
    publication: TakePublication,
    publishing: bool,
    auto_attach: bool,
}

pub(super) struct AudioUi {
    port: Option<Box<dyn EnginePort>>,
    attempted: bool,
    phase: Phase,
    pending: Option<Pending>,
    deferred: Option<Control>,
    devices: DeviceInventory,
    input: Option<String>,
    output: Option<String>,
    status: Option<EngineStatus>,
    target: Option<CaptureTarget>,
    original: Option<CaptureIdentity>,
    sealed: Option<SealedCapture>,
    pub(super) error: Option<String>,
    message: String,
    last_status: Instant,
    live: LiveUi,
}
impl AudioUi {
    pub(super) fn new() -> Self {
        Self {
            port: None,
            attempted: false,
            phase: Phase::Idle,
            pending: None,
            deferred: None,
            devices: DeviceInventory {
                inputs: vec![],
                outputs: vec![],
            },
            input: None,
            output: None,
            status: None,
            target: None,
            original: None,
            sealed: None,
            error: None,
            message: "Native audio process has not started.".into(),
            last_status: Instant::now(),
            live: LiveUi::new(),
        }
    }
    pub(super) fn start(&mut self, executable: PathBuf, store: PathBuf) {
        if self.attempted {
            return;
        }
        self.attempted = true;
        match EngineClient::spawn(executable, store) {
            Ok(port) => {
                self.port = Some(Box::new(port));
                self.message =
                    "Audio ready for an explicit device scan or saved-session action.".into();
            }
            Err(error) => {
                self.error = Some(error);
                self.message = "Audio process could not start.".into();
            }
        }
    }
    pub(super) fn blocks_session(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Unavailable)
            || self.sealed.is_some()
            || self.live.engaged()
    }
    pub(super) fn live_guard(&self) -> bool {
        self.live.engaged() || matches!(self.phase, Phase::StartingLive | Phase::Live)
    }
    pub(super) fn cleanup_live_input(&mut self) {
        if let Some(port) = self.port.as_mut() {
            self.live.cleanup(port.as_mut());
        }
    }
    pub(super) fn live_input(&mut self, ctx: &egui::Context, blocked: bool) {
        if let Some(port) = self.port.as_mut() {
            self.live.input(ctx, blocked, port.as_mut());
        }
    }
    pub(super) fn capture_guard(&self) -> bool {
        self.target.is_some() || self.sealed.is_some()
    }
    pub(super) fn can_finish(&self) -> bool {
        self.port.is_some() && self.target.is_some() && self.sealed.is_none()
    }
    pub(super) fn needs_poll(&self) -> bool {
        self.pending.is_some()
            || self.deferred.is_some()
            || matches!(self.phase, Phase::Playing | Phase::Capturing)
            || self.live.engaged()
    }
    fn submit(&mut self, command: EngineCommand, pending: Pending) -> bool {
        let Some(port) = self.port.as_mut() else {
            self.error
                .get_or_insert_with(|| "Native audio process is unavailable.".into());
            return false;
        };
        if self.pending.is_some() || port.is_busy() {
            return false;
        }
        match port.try_submit(command) {
            Ok(()) => {
                self.pending = Some(pending);
                self.error = None;
                true
            }
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }
    fn play(&mut self, project_id: String) {
        if self.blocks_session() {
            return;
        }
        if self.submit(
            EngineCommand::StartPlayback {
                project_id,
                output_device: self.output.clone(),
            },
            Pending::Play,
        ) {
            self.phase = Phase::StartingPlayback;
            self.message = "Preparing the saved session for playback.".into();
        }
    }
    fn start_live(&mut self) {
        if self.blocks_session() {
            return;
        }
        let clock = self.live.begin();
        if self.submit(
            EngineCommand::StartLive {
                output_device: self.output.clone(),
                gui_clock: clock,
            },
            Pending::Live,
        ) {
            self.status = None;
            self.phase = Phase::StartingLive;
            self.message =
                "Preparing output-only live audition. This performance is not recorded or saved."
                    .into();
        } else {
            self.live.start_failed();
        }
    }
    fn record(&mut self, project_id: String, revision: u64) {
        if self.blocks_session() {
            return;
        }
        if self.submit(
            EngineCommand::StartCapture {
                input_device: self.input.clone(),
            },
            Pending::Record,
        ) {
            self.status = None;
            self.target = Some(CaptureTarget {
                project_id,
                revision,
            });
            self.phase = Phase::StartingCapture;
            self.message =
                "Opening the selected input for an explicitly requested solo take.".into();
        }
    }
    pub(super) fn stop(&mut self) {
        if self.live_guard() {
            if let Some(port) = self.port.as_mut() {
                self.live.request_stop(port.as_mut());
            }
            return;
        }
        self.control(Control::Stop);
    }
    pub(super) fn finish(&mut self) {
        self.control(Control::Finish);
    }
    fn control(&mut self, control: Control) {
        if control == Control::Finish && (self.target.is_none() || self.sealed.is_some()) {
            return;
        }
        if self.pending.is_some() || self.port.as_ref().is_some_and(|port| port.is_busy()) {
            // Emergency Stop supersedes a queued finish. There is never a second queue.
            if self.deferred != Some(Control::Stop) {
                self.deferred = Some(control);
            }
            self.message = if control == Control::Stop {
                "Stop requested; waiting for the current audio acknowledgement."
            } else {
                "Finish requested; waiting for the current audio acknowledgement."
            }
            .into();
            return;
        }
        let (command, pending) = match control {
            Control::Stop => (EngineCommand::Stop, Pending::Stop),
            Control::Finish => (EngineCommand::FinishCapture, Pending::Finish),
        };
        if self.submit(command, pending) && control == Control::Finish {
            self.phase = Phase::Finishing;
            self.message =
                "Finishing the original take. Publication starts only after a sealed result."
                    .into();
        }
    }
    pub(super) fn poll(&mut self) {
        if let Some(update) = self.port.as_mut().and_then(|port| port.try_poll_update())
            && self.live.update(update)
        {
            self.phase = Phase::Idle;
            self.status = None;
            self.message = "Live output stopped. No performance was recorded or saved.".into();
        }
        let reply = self.port.as_mut().and_then(|port| port.try_poll());
        if let Some(reply) = reply {
            let Some(pending) = self.pending.take() else {
                if let (0, Err(error)) = (reply.request_id, reply.result) {
                    self.retired(error);
                } else {
                    self.error = Some("Ignored an unsolicited audio completion.".into());
                }
                return;
            };
            if pending == Pending::Live && self.live.cancelled_start() {
                // An acknowledged or pending urgent Stop fences the ordinary
                // StartLive completion. It can never reopen note routing.
                self.live.start_failed();
                if let Err(error) = reply.result {
                    if self.port.as_ref().is_some_and(|port| port.is_retired()) {
                        self.retired(error);
                    } else {
                        self.error = Some(error);
                    }
                }
                return;
            }
            match reply.result {
                Err(error) => {
                    if self.port.as_ref().is_some_and(|port| port.is_retired()) {
                        self.retired(error);
                        return;
                    }
                    self.error = Some(error);
                    self.message = if pending==Pending::Live{"Live audition ended with an error. This performance was not recorded or saved; stop output before continuing."}else{"Audio action ended with an error. No complete final tail is claimed."}.into();
                    if matches!(
                        pending,
                        Pending::Play
                            | Pending::Live
                            | Pending::Record
                            | Pending::Finish
                            | Pending::Stop
                            | Pending::Status
                    ) && self.blocks_session()
                    {
                        self.phase = Phase::Uncertain;
                    }
                }
                Ok(result) => self.complete(pending, result),
            }
        }
        if self.pending.is_none()
            && let Some(control) = self.deferred.take()
        {
            self.control(control);
        }
        if self.pending.is_none()
            && self.deferred.is_none()
            && matches!(self.phase, Phase::Playing | Phase::Capturing)
            && self.last_status.elapsed() >= Duration::from_millis(250)
        {
            self.last_status = Instant::now();
            self.submit(EngineCommand::Status, Pending::Status);
        }
    }
    fn retired(&mut self, error: String) {
        let live = self.live_guard();
        self.error = Some(error);
        self.message=if live{"Live audio process is unavailable. This performance was not recorded or saved. No automatic restart or replay was attempted."}else{"Audio process is unavailable. Original journals remain retained; no automatic restart, resume, seal or replay was attempted."}.into();
        self.phase = if self.capture_guard() {
            Phase::Uncertain
        } else {
            Phase::Unavailable
        };
        self.status = None;
        self.deferred = None;
        self.live.retired();
        // Dropping the bounded client never waits for the retired child.
        self.port = None;
    }
    fn complete(&mut self, pending: Pending, result: EngineResult) {
        match (pending, result) {
            (Pending::Devices, EngineResult::Devices(devices)) => {
                self.devices = devices;
                self.message =
                    "Audio device list refreshed. Selecting a device does not open the microphone."
                        .into();
            }
            (Pending::Play, EngineResult::Playing(status)) => {
                self.phase = Phase::Playing;
                self.status = Some(status);
                self.message = "Playing the saved session.".into();
            }
            (Pending::Live, EngineResult::LiveStarted(ready)) => match self.live.activate(ready) {
                Ok(true) => {
                    self.phase = Phase::Live;
                    self.message = "Live audition · output only · not recording or saved.".into();
                }
                Ok(false) => {}
                Err(error) => {
                    self.error = Some(error);
                    self.phase = Phase::Uncertain;
                    self.stop();
                }
            },
            (Pending::Record, EngineResult::Capturing(status)) => {
                self.phase = Phase::Capturing;
                self.message =
                    "Recording a solo original. No monitoring or backing playback.".into();
                self.capture_status(status);
            }
            (Pending::Status, EngineResult::Status(status)) => {
                if self.target.is_some() {
                    self.capture_status(status);
                } else {
                    if status.state == "playback_finished" {
                        self.phase = Phase::Idle;
                        self.message="Playback consumed its prepared frames. Hardware latency and audible completion are not measured here.".into();
                    }
                    self.status = Some(status);
                }
            }
            (Pending::Stop, EngineResult::Stopped) => {
                let had_capture = self.target.is_some();
                self.phase = Phase::Idle;
                self.status = None;
                if self.sealed.is_none() {
                    self.target = None;
                }
                self.message=if self.sealed.is_some(){"Audio stopped. The sealed original is retained; its attachment state is unchanged."}else if had_capture&&self.original.is_some(){"Audio stopped. The original journal is retained unsealed; no successful final tail or attached take is claimed."}else{"Audio stopped. No new sealed capture result is claimed."}.into();
            }
            (Pending::Finish, EngineResult::Captured(take)) => self.captured(take),
            _ => {
                self.phase = Phase::Uncertain;
                self.error = Some(
                    "Audio completion did not match the requested action. Stop before continuing."
                        .into(),
                );
            }
        }
    }
    fn capture_status(&mut self, status: EngineStatus) {
        if let Some(identity) = &status.capture {
            if self.original.as_ref().is_some_and(|original| {
                original.take_id != identity.take_id
                    || original.journal_path != identity.journal_path
            }) && self.status.is_some()
            {
                self.phase = Phase::Uncertain;
                self.error=Some("Capture identity changed unexpectedly. The earlier original journal is retained; stop before continuing.".into());
                return;
            }
            self.original = Some(identity.clone());
        } else {
            self.phase = Phase::Uncertain;
            self.error=Some("Capture did not report its original journal identity. Stop audio; no recovery location or complete tail is assumed.".into());
        }
        if matches!(
            status.state.as_str(),
            "capture_stopped_transfer_overflow" | "capture_fault"
        ) {
            self.phase = Phase::Uncertain;
            self.message="Capture stopped after a transfer or device fault. Original journal retained for recovery; no automatic resume or seal was attempted.".into();
        }
        self.status = Some(status);
    }
    fn captured(&mut self, take: CapturedTake) {
        let Some(target) = &self.target else {
            self.phase = Phase::Uncertain;
            self.error = Some("A sealed take arrived without its original session target.".into());
            return;
        };
        if self.original.as_ref().is_some_and(|original| {
            original.take_id != take.take_id || original.journal_path != take.journal_path
        }) {
            self.phase = Phase::Uncertain;
            self.error = Some(format!(
                "Sealed reply identity did not match the recording. Original journal remains retained; unexpected result at {} was not attached.",
                take.journal_path.display()
            ));
            return;
        }
        if take.frames == 0 || take.channels == 0 || !(8_000..=768_000).contains(&take.sample_rate)
        {
            self.phase = Phase::Uncertain;
            self.error = Some(format!(
                "Capture returned an invalid source extent; original journal retained at {}",
                take.journal_path.display()
            ));
            return;
        }
        let publication = TakePublication {
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            project_id: ProjectId(target.project_id.clone()),
            expected_revision: target.revision,
            track_id: TrackId(uuid::Uuid::new_v4().to_string()),
            track_name: "Solo recorded original".into(),
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
            },
        };
        if let Err(error) = publication.validate() {
            self.phase = Phase::Uncertain;
            self.error = Some(format!(
                "Sealed original retained at {}; publication metadata rejected: {error}",
                take.journal_path.display()
            ));
            return;
        }
        self.phase = Phase::Idle;
        self.status = None;
        self.message =
            "Original sealed. Attaching at frame zero as an uncalibrated solo recording.".into();
        self.sealed = Some(SealedCapture {
            take,
            publication,
            publishing: false,
            auto_attach: true,
        });
    }
    pub(super) fn take_auto_publication(&mut self) -> Option<(PathBuf, TakePublication)> {
        let sealed = self.sealed.as_mut()?;
        if !sealed.auto_attach || sealed.publishing {
            return None;
        }
        sealed.auto_attach = false;
        Some((sealed.take.journal_path.clone(), sealed.publication.clone()))
    }
    pub(super) fn retry_publication(&self) -> Option<(PathBuf, TakePublication)> {
        let sealed = self.sealed.as_ref()?;
        if sealed.publishing {
            return None;
        }
        Some((sealed.take.journal_path.clone(), sealed.publication.clone()))
    }
    pub(super) fn publication_started(&mut self) {
        if let Some(sealed) = &mut self.sealed {
            sealed.publishing = true;
            sealed.auto_attach = false;
        }
        self.message = "Attaching the sealed original to its saved session.".into();
    }
    pub(super) fn publication_failed(&mut self) {
        if let Some(sealed) = &mut self.sealed {
            sealed.publishing = false;
            sealed.auto_attach = false;
        }
        self.message="Sealed original retained. Attachment was not confirmed; retry the same attachment explicitly.".into();
    }
    pub(super) fn publication_succeeded(&mut self) {
        self.sealed = None;
        self.target = None;
        self.original = None;
        self.phase = if self.port.is_some() {
            Phase::Idle
        } else {
            Phase::Unavailable
        };
        self.message="Solo original attached. Placement at frame zero is uncalibrated; source journal is retained.".into();
    }

    /// Returns true only for an explicit retry of the retained sealed publication.
    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        can_start: bool,
        context: Option<(String, u64)>,
    ) -> bool {
        let mut retry = false;
        let free = self.port.is_some() && self.pending.is_none() && !self.blocks_session();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(free, egui::Button::new("Audio devices"))
                .clicked()
            {
                self.submit(EngineCommand::Devices, Pending::Devices);
            }
            egui::ComboBox::from_id_salt("native-audio-input")
                .selected_text(
                    self.input
                        .as_ref()
                        .and_then(|id| self.devices.inputs.iter().find(|d| &d.id == id))
                        .map_or("System default input", |d| d.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    ui.add_enabled_ui(free, |ui| {
                        ui.selectable_value(&mut self.input, None, "System default input");
                        for device in &self.devices.inputs {
                            ui.selectable_value(
                                &mut self.input,
                                Some(device.id.clone()),
                                &device.name,
                            );
                        }
                    });
                })
                .response
                .accessible_name("Audio input device");
            egui::ComboBox::from_id_salt("native-audio-output")
                .selected_text(
                    self.output
                        .as_ref()
                        .and_then(|id| self.devices.outputs.iter().find(|d| &d.id == id))
                        .map_or("System default output", |d| d.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    ui.add_enabled_ui(free, |ui| {
                        ui.selectable_value(&mut self.output, None, "System default output");
                        for device in &self.devices.outputs {
                            ui.selectable_value(
                                &mut self.output,
                                Some(device.id.clone()),
                                &device.name,
                            );
                        }
                    });
                })
                .response
                .accessible_name("Audio output device");
            if ui
                .add_enabled(
                    free && can_start && context.is_some(),
                    egui::Button::new("Play saved session"),
                )
                .clicked()
                && let Some((id, _)) = &context
            {
                self.play(id.clone());
            }
            if ui
                .add_enabled(free && can_start, egui::Button::new("Start live audition"))
                .clicked()
            {
                self.start_live();
            }
            // Stop stays available independently of storage busy/dirty/navigation state.
            if ui
                .add_enabled(self.port.is_some(), egui::Button::new("Stop audio"))
                .clicked()
            {
                self.stop();
            }
            if ui
                .add_enabled(
                    free && can_start && context.is_some(),
                    egui::Button::new("Record solo"),
                )
                .clicked()
                && let Some((id, revision)) = &context
            {
                self.record(id.clone(), *revision);
            }
            if ui
                .add_enabled(self.can_finish(), egui::Button::new("Finish take"))
                .clicked()
            {
                self.finish();
            }
        });
        ui.small("Solo capture starts only when requested. No monitoring or overdub; placement is uncalibrated. Stop keeps an unfinished original without sealing it.");
        ui.small("Saved note playback uses a procedural audition instrument, not AI inference.");
        ui.label(&self.message);
        if let Some(status) = &self.status {
            ui.label(format!(
                "{} · {} Hz · {} channels · {} frames · peak {:.3} · {} xruns",
                status.state,
                status.sample_rate,
                status.channels,
                status.frames,
                status.peak,
                status.xruns
            ));
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(original) = &self.original {
            ui.small(format!("Original journal retained for recovery: {} · take {}. No automatic resume or seal.",original.journal_path.display(),original.take_id));
        }
        if let Some(sealed) = &self.sealed {
            ui.small(format!(
                "Retained sealed original: {} · {} frames · {} Hz · {} channels · {} gap frames",
                sealed.take.journal_path.display(),
                sealed.take.frames,
                sealed.take.sample_rate,
                sealed.take.channels,
                sealed.take.gap_frames
            ));
            ui.small(format!(
                "Take {} · manifest {}",
                sealed.take.take_id, sealed.take.manifest_sha256
            ));
            if ui
                .add_enabled(
                    !sealed.publishing && can_start,
                    egui::Button::new("Retry attach sealed take"),
                )
                .clicked()
            {
                retry = true;
            }
        }
        if let Some(port) = self.port.as_mut() {
            self.live.ui(ui, port.as_mut());
        }
        retry
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};
    #[derive(Default)]
    struct Shared {
        commands: VecDeque<EngineCommand>,
        replies: VecDeque<EngineReply>,
        busy: bool,
        updates: VecDeque<EngineUpdate>,
        batches: VecDeque<LiveBatch>,
        urgent: VecDeque<(UrgentTicket, Option<LiveScope>)>,
        next_ticket: u64,
        retired: bool,
    }
    struct Port(Rc<RefCell<Shared>>);
    pub(crate) struct Peer(Rc<RefCell<Shared>>);
    impl EnginePort for Port {
        fn try_submit(&mut self, command: EngineCommand) -> Result<(), String> {
            let mut s = self.0.borrow_mut();
            if s.busy {
                return Err("full".into());
            }
            s.busy = true;
            s.commands.push_back(command);
            Ok(())
        }
        fn try_poll(&mut self) -> Option<EngineReply> {
            let mut s = self.0.borrow_mut();
            let reply = s.replies.pop_front()?;
            s.busy = false;
            Some(reply)
        }
        fn is_busy(&self) -> bool {
            self.0.borrow().busy
        }
        fn is_retired(&self) -> bool {
            self.0.borrow().retired
        }
        fn try_push_live(&mut self, batch: LiveBatch) -> Result<LiveQueued, LiveSubmitError> {
            self.0.borrow_mut().batches.push_back(batch);
            Ok(LiveQueued {
                scope: batch.scope,
                sequence: batch.sequence,
            })
        }
        fn request_stop(&mut self) -> Result<UrgentTicket, String> {
            let mut s = self.0.borrow_mut();
            s.next_ticket += 1;
            let ticket = UrgentTicket {
                worker_generation: omniusong_engine::LiveId([1; 16]),
                id: s.next_ticket,
            };
            s.urgent.push_back((ticket, None));
            Ok(ticket)
        }
        fn request_panic_live(
            &mut self,
            scope: LiveScope,
        ) -> Result<UrgentTicket, LiveSubmitError> {
            let mut s = self.0.borrow_mut();
            s.next_ticket += 1;
            let ticket = UrgentTicket {
                worker_generation: scope.worker_generation,
                id: s.next_ticket,
            };
            s.urgent.push_back((ticket, Some(scope)));
            Ok(ticket)
        }
        fn try_poll_update(&mut self) -> Option<EngineUpdate> {
            self.0.borrow_mut().updates.pop_front()
        }
    }
    impl Peer {
        pub(crate) fn command(&self) -> Option<EngineCommand> {
            self.0.borrow_mut().commands.pop_front()
        }
        pub(crate) fn reply(&self, result: Result<EngineResult, String>) {
            self.0.borrow_mut().replies.push_back(EngineReply {
                request_id: 1,
                result,
            });
        }
        pub(crate) fn terminal(&self, error: &str) {
            self.0.borrow_mut().retired = true;
            self.0.borrow_mut().replies.push_back(EngineReply {
                request_id: 0,
                result: Err(error.into()),
            });
        }
        pub(crate) fn retire_pending(&self, error: &str) {
            self.0.borrow_mut().retired = true;
            self.reply(Err(error.into()));
        }
        pub(crate) fn update(&self, update: EngineUpdate) {
            self.0.borrow_mut().updates.push_back(update);
        }
        pub(crate) fn urgent(&self) -> Option<(UrgentTicket, Option<LiveScope>)> {
            self.0.borrow_mut().urgent.pop_front()
        }
    }
    pub(crate) fn pair() -> (AudioUi, Peer) {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let mut ui = AudioUi::new();
        ui.port = Some(Box::new(Port(shared.clone())));
        ui.attempted = true;
        ui.last_status = Instant::now() + Duration::from_secs(3600);
        (ui, Peer(shared))
    }
    pub(crate) fn status(state: &str) -> EngineStatus {
        EngineStatus {
            state: state.into(),
            sample_rate: 48_000,
            channels: 2,
            frames: 4,
            peak: 0.2,
            xruns: 0,
            capture: if state.starts_with("captur") {
                Some(CaptureIdentity {
                    journal_path: PathBuf::from("/synthetic/retained.sqlite3"),
                    take_id: "synthetic-take".into(),
                })
            } else {
                None
            },
        }
    }
    pub(crate) fn take() -> CapturedTake {
        CapturedTake {
            journal_path: PathBuf::from("/synthetic/retained.sqlite3"),
            take_id: "synthetic-take".into(),
            manifest_sha256: "a".repeat(64),
            frames: 4,
            sample_rate: 48_000,
            channels: 2,
            gap_frames: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};

    fn accessible_text(audio: &mut AudioUi) -> Vec<String> {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 1000.0),
                )),
                ..Default::default()
            },
            |ui| {
                audio.ui(ui, false, None);
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .map(str::to_owned)
            .collect()
    }
    #[test]
    fn pending_status_keeps_one_emergency_stop_and_never_floods_channel() {
        let (mut audio, peer) = pair();
        audio.record("project".into(), 7);
        assert!(matches!(
            peer.command(),
            Some(EngineCommand::StartCapture { .. })
        ));
        peer.reply(Ok(EngineResult::Capturing(status("capturing"))));
        audio.poll();
        audio.last_status = Instant::now() - Duration::from_secs(1);
        audio.poll();
        assert!(matches!(peer.command(), Some(EngineCommand::Status)));
        audio.finish();
        audio.stop();
        audio.finish();
        for _ in 0..8 {
            audio.poll();
        }
        assert!(peer.command().is_none());
        assert!(audio.capture_guard());
        peer.reply(Ok(EngineResult::Status(status("capturing"))));
        audio.poll();
        assert!(matches!(peer.command(), Some(EngineCommand::Stop)));
        peer.reply(Ok(EngineResult::Stopped));
        audio.poll();
        assert!(!audio.blocks_session());
        assert!(audio.take_auto_publication().is_none());
        assert!(audio.message.contains("unsealed"));
    }
    #[test]
    fn finish_pins_target_and_retains_exact_publication_across_failure() {
        let (mut audio, peer) = pair();
        audio.record("original-project".into(), 7);
        let _ = peer.command();
        peer.reply(Ok(EngineResult::Capturing(status("capturing"))));
        audio.poll();
        audio.finish();
        assert!(matches!(peer.command(), Some(EngineCommand::FinishCapture)));
        peer.reply(Ok(EngineResult::Captured(take())));
        audio.poll();
        let (path, publication) = audio.take_auto_publication().unwrap();
        assert_eq!(publication.project_id.0, "original-project");
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
        assert!(audio.take_auto_publication().is_none());
        audio.publication_started();
        assert!(audio.retry_publication().is_none());
        audio.publication_failed();
        assert_eq!(audio.retry_publication().unwrap(), (path, publication));
        assert!(audio.blocks_session());
        audio.publication_succeeded();
        assert!(!audio.blocks_session());
    }
    #[test]
    fn failed_finish_never_synthesizes_seal_or_releases_capture_guard() {
        let (mut audio, peer) = pair();
        audio.record("project".into(), 0);
        let _ = peer.command();
        peer.reply(Ok(EngineResult::Capturing(status("capturing"))));
        audio.poll();
        audio.finish();
        let _ = peer.command();
        peer.reply(Err(
            "synthetic worker EOF; unfinished journal retained".into()
        ));
        audio.poll();
        assert!(audio.capture_guard());
        assert!(audio.blocks_session());
        assert!(audio.take_auto_publication().is_none());
        assert!(audio.error.as_ref().unwrap().contains("EOF"));
        assert_eq!(
            audio.original.as_ref().unwrap().journal_path,
            PathBuf::from("/synthetic/retained.sqlite3")
        );
        audio.stop();
        assert!(matches!(peer.command(), Some(EngineCommand::Stop)));
    }

    #[test]
    fn natural_playback_completion_releases_guard_without_claiming_device_latency() {
        let (mut audio, peer) = pair();
        audio.play("project".into());
        let _ = peer.command();
        peer.reply(Ok(EngineResult::Playing(status("playing"))));
        audio.poll();
        assert!(audio.blocks_session());
        audio.last_status = Instant::now() - Duration::from_secs(1);
        audio.poll();
        assert!(matches!(peer.command(), Some(EngineCommand::Status)));
        peer.reply(Ok(EngineResult::Status(status("playback_finished"))));
        audio.poll();
        assert!(!audio.blocks_session());
        assert!(audio.message.contains("not measured"));
        assert!(peer.command().is_none());
    }

    #[test]
    fn capture_fault_retains_early_identity_and_does_not_automatically_seal() {
        let (mut audio, peer) = pair();
        audio.record("project".into(), 0);
        let _ = peer.command();
        peer.reply(Ok(EngineResult::Capturing(status("capturing"))));
        audio.poll();
        audio.last_status = Instant::now() - Duration::from_secs(1);
        audio.poll();
        let _ = peer.command();
        peer.reply(Ok(EngineResult::Status(status(
            "capture_stopped_transfer_overflow",
        ))));
        audio.poll();
        assert!(audio.capture_guard());
        assert_eq!(audio.original.as_ref().unwrap().take_id, "synthetic-take");
        assert!(audio.take_auto_publication().is_none());
        assert!(peer.command().is_none());
        audio.finish();
        assert!(matches!(peer.command(), Some(EngineCommand::FinishCapture)));
    }

    #[test]
    fn asynchronous_capture_terminal_failure_preserves_exact_cause_and_recovery_path() {
        let (mut audio, peer) = pair();
        audio.record("pinned-project".into(), 4);
        let _ = peer.command();
        peer.reply(Ok(EngineResult::Capturing(status("capturing"))));
        audio.poll();
        assert!(audio.pending.is_none());
        let cause = "Audio child heartbeat deadline exceeded: synthetic retained cause";
        peer.terminal(cause);
        audio.poll();
        assert_eq!(audio.error.as_deref(), Some(cause));
        assert!(matches!(audio.phase, Phase::Uncertain));
        assert!(audio.capture_guard());
        assert!(audio.blocks_session());
        assert!(!audio.can_finish());
        assert!(!audio.needs_poll());
        assert!(audio.port.is_none());
        assert!(audio.status.is_none());
        assert_eq!(audio.original.as_ref().unwrap().take_id, "synthetic-take");
        assert!(audio.take_auto_publication().is_none());
        let text = accessible_text(&mut audio);
        assert!(text.iter().any(|text| text == cause));
        assert!(text.iter().any(|text| {
            text.contains("Original journal retained for recovery: /synthetic/retained.sqlite3")
                && text.contains("synthetic-take")
        }));
        assert!(!text.iter().any(|text| text.starts_with("capturing ·")));
        for _ in 0..4 {
            audio.poll();
        }
        assert_eq!(audio.error.as_deref(), Some(cause));
        assert!(peer.command().is_none());
    }

    #[test]
    fn asynchronous_terminal_failure_retires_playback_or_idle_without_poisoning_session_guard() {
        for playing in [true, false] {
            let (mut audio, peer) = pair();
            if playing {
                audio.play("saved-project".into());
                let _ = peer.command();
                peer.reply(Ok(EngineResult::Playing(status("playing"))));
                audio.poll();
            }
            assert!(audio.pending.is_none());
            let cause = "Audio supervisor terminal failure: synthetic exact reason";
            peer.terminal(cause);
            audio.poll();
            assert_eq!(audio.error.as_deref(), Some(cause));
            assert!(matches!(audio.phase, Phase::Unavailable));
            assert!(!audio.blocks_session());
            assert!(!audio.capture_guard());
            assert!(audio.port.is_none());
            assert!(audio.status.is_none());
            assert!(!audio.needs_poll());
            let text = accessible_text(&mut audio);
            assert!(text.iter().any(|text| text == cause));
            assert!(!text.iter().any(|text| text.starts_with("playing ·")));
            audio.stop();
            audio.poll();
            assert_eq!(audio.error.as_deref(), Some(cause));
            assert!(peer.command().is_none());
        }
    }

    #[test]
    fn unsolicited_success_still_reports_protocol_error() {
        let (mut audio, peer) = pair();
        peer.reply(Ok(EngineResult::Stopped));
        audio.poll();
        assert_eq!(
            audio.error.as_deref(),
            Some("Ignored an unsolicited audio completion.")
        );
        assert!(audio.port.is_some());
        assert!(audio.pending.is_none());
        assert!(peer.command().is_none());
    }
    #[test]
    fn urgent_stop_during_live_start_fences_ordinary_reply_in_either_order() {
        use omniusong_engine::{EngineUpdate, LiveId, StopAck, UrgentProgress};
        for ack_first in [false, true] {
            let (mut audio, peer) = pair();
            audio.start_live();
            let Some(EngineCommand::StartLive { gui_clock, .. }) = peer.command() else {
                panic!("Live start command")
            };
            audio.stop();
            let (ticket, scope) = peer.urgent().expect("Stop bypasses held control admission");
            assert!(scope.is_none());
            assert!(audio.blocks_session());
            let ack = EngineUpdate {
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
            };
            if ack_first {
                peer.update(ack);
                audio.poll();
                assert!(!audio.blocks_session());
            }
            peer.reply(Ok(EngineResult::LiveStarted(
                crate::live_ui::test_support::ready(gui_clock),
            )));
            audio.poll();
            if !ack_first {
                assert!(audio.blocks_session());
                peer.update(ack);
                audio.poll();
            }
            assert!(!audio.blocks_session());
            assert!(!audio.live_guard());
            assert!(!audio.capture_guard());
            assert!(audio.take_auto_publication().is_none());
            assert!(!audio.live.running());
            assert!(peer.command().is_none());
        }
    }
    #[test]
    fn typed_retirement_on_pending_live_error_preserves_cause_without_pinning_session() {
        for stopped in [false, true] {
            let (mut audio, peer) = pair();
            audio.start_live();
            let _ = peer.command();
            if stopped {
                audio.stop();
            }
            let cause = "Synthetic retired child after pending live start";
            peer.retire_pending(cause);
            audio.poll();
            assert_eq!(audio.error.as_deref(), Some(cause));
            assert!(audio.port.is_none());
            assert!(!audio.blocks_session());
            assert!(!audio.capture_guard());
            assert!(audio.message.contains("not recorded or saved"));
            assert!(audio.take_auto_publication().is_none());
        }
    }
}
