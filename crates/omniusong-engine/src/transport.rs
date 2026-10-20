use crate::live_supervisor::{self, ClientLanes, FaultRequest, PanicRequest};
use crate::*;
use omniusong_core::contracts::{Architecture, ContractError, Platform, Validate, WirePayload};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
};
const WIRE_BYTES: usize = 64 * 1024;
pub(crate) const BUILD: &str = ENGINE_BUILD_ID;
#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
// The fixed 32-event packet stays inline (currently 1,864 bytes). The private
// actor has one in-flight request; boxing would add an unnecessary allocation.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Action {
    Initialize {
        store_root: PathBuf,
        worker_generation: LiveId,
        profile: String,
    },
    LiveBatch {
        batch: LiveBatch,
    },
    LiveFault {
        scope: LiveScope,
        kind: LiveFaultKind,
        sequence: Option<u64>,
    },
    PanicLive {
        ticket: UrgentTicket,
        scope: LiveScope,
    },
    UrgentStop {
        ticket: UrgentTicket,
        cancelled_through_control: u64,
        superseded_panic: Option<(UrgentTicket, LiveScope)>,
    },
    Command {
        command: EngineCommand,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub request_id: u64,
    pub action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Response {
    pub reply: EngineReply,
    #[serde(default)]
    pub update: Option<EngineUpdate>,
}
fn require(ok: bool, s: &str) -> Result<(), ContractError> {
    if ok {
        Ok(())
    } else {
        Err(ContractError(s.into()))
    }
}
fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.contains('\0')
}
pub(crate) fn command_valid(command: &EngineCommand) -> Result<(), String> {
    let device = match command {
        EngineCommand::StartPlayback {
            project_id,
            output_device,
        } => {
            if !text(project_id, 200) {
                return Err("Invalid project identity".into());
            }
            output_device
        }
        EngineCommand::StartCapture { input_device } => input_device,
        EngineCommand::StartLive {
            output_device,
            gui_clock,
        } => {
            if !gui_clock.is_valid() {
                return Err("Invalid live GUI clock".into());
            }
            output_device
        }
        _ => return Ok(()),
    };
    if device.as_ref().is_some_and(|s| !text(s, 1024)) {
        return Err("Invalid device identity".into());
    }
    Ok(())
}
impl Validate for Request {
    fn validate(&self) -> Result<(), ContractError> {
        match &self.action {
            Action::Initialize {
                store_root,
                worker_generation,
                profile,
            } => require(
                store_root.is_absolute()
                    && store_root.to_string_lossy().len() <= 4096
                    && worker_generation.is_valid()
                    && profile == LIVE_PROTOCOL,
                "Invalid engine store path",
            ),
            Action::Command { command } => command_valid(command).map_err(ContractError),
            Action::LiveBatch { batch } => require(
                batch.validate_shape(768000).is_ok()
                    && serde_json::to_vec(batch).is_ok_and(|v| v.len() <= LIVE_WIRE_BYTES),
                "Invalid or oversized live batch",
            ),
            Action::LiveFault {
                scope, sequence, ..
            } => require(
                scope.is_valid() && *sequence != Some(0),
                "Invalid scoped live fault",
            ),
            Action::PanicLive { ticket, scope } => require(
                scope.is_valid()
                    && ticket.worker_generation == scope.worker_generation
                    && ticket.id > 0,
                "Invalid panic ticket",
            ),
            Action::UrgentStop {
                ticket,
                superseded_panic,
                ..
            } => require(
                ticket.worker_generation.is_valid()
                    && ticket.id > 0
                    && superseded_panic.is_none_or(|(p, s)| {
                        s.is_valid()
                            && s.worker_generation == ticket.worker_generation
                            && p.worker_generation == ticket.worker_generation
                            && p.id > 0
                            && p.id <= ticket.id
                    }),
                "Invalid Stop ticket",
            ),
        }
    }
}
impl WirePayload for Request {
    const PROTOCOL: &'static str = "omniusong.engine-control.v1";
    const MAX_BYTES: usize = WIRE_BYTES;
}
impl Validate for Response {
    fn validate(&self) -> Result<(), ContractError> {
        if self.update.is_some_and(|u| !u.is_valid()) {
            return Err(ContractError("Invalid engine update".into()));
        }
        match &self.reply.result {
            Err(s) => require(text(s, 16 * 1024), "Invalid engine error"),
            Ok(EngineResult::Devices(d)) => {
                require(
                    d.inputs.len() <= 64 && d.outputs.len() <= 64,
                    "Too many devices",
                )?;
                for item in d.inputs.iter().chain(&d.outputs) {
                    require(
                        text(&item.id, 1024) && text(&item.name, 1024),
                        "Invalid device descriptor",
                    )?;
                }
                Ok(())
            }
            Ok(EngineResult::Status(s) | EngineResult::Playing(s) | EngineResult::Capturing(s)) => {
                require(
                    matches!(
                        s.state.as_str(),
                        "idle"
                            | "playing"
                            | "playback_finished"
                            | "capturing"
                            | "capture_stopped_transfer_overflow"
                            | "capture_fault"
                    ),
                    "Invalid engine state",
                )?;
                require(
                    s.peak.is_finite() && s.peak >= 0.0 && s.frames <= i64::MAX as u64,
                    "Invalid engine meter",
                )?;
                if s.state == "idle" {
                    require(
                        s.sample_rate == 0
                            && s.channels == 0
                            && s.frames == 0
                            && s.peak == 0.0
                            && s.xruns == 0,
                        "Idle status contains active configuration or meters",
                    )?;
                } else {
                    require(
                        (8000..=768000).contains(&s.sample_rate) && (1..=8).contains(&s.channels),
                        "Active status has invalid audio configuration",
                    )?;
                }
                match &self.reply.result {
                    Ok(EngineResult::Playing(_)) => require(
                        matches!(s.state.as_str(), "playing" | "playback_finished"),
                        "Playing result contains a different lifecycle state",
                    )?,
                    Ok(EngineResult::Capturing(_)) => require(
                        s.state.starts_with("captur"),
                        "Capturing result contains a different lifecycle state",
                    )?,
                    _ => {}
                }
                if let Some(c) = &s.capture {
                    require(
                        text(&c.take_id, 96)
                            && c.journal_path.is_absolute()
                            && c.journal_path.to_string_lossy().len() <= 4096,
                        "Invalid capture identity",
                    )?;
                }
                require(
                    s.state.starts_with("captur") == s.capture.is_some(),
                    "Missing or unexpected capture identity",
                )
            }
            Ok(EngineResult::Captured(c)) => require(
                c.journal_path.is_absolute()
                    && c.journal_path.to_string_lossy().len() <= 4096
                    && text(&c.take_id, 96)
                    && c.manifest_sha256.len() == 64
                    && c.manifest_sha256
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                    && c.frames > 0
                    && c.frames <= i64::MAX as u64
                    && c.gap_frames <= c.frames
                    && (8000..=768000).contains(&c.sample_rate)
                    && (1..=8).contains(&c.channels),
                "Invalid captured take receipt",
            ),
            Ok(EngineResult::Initialized {
                profile,
                worker_generation,
            }) => require(
                profile == LIVE_PROTOCOL && worker_generation.is_valid(),
                "Invalid initialized profile",
            ),
            Ok(EngineResult::LiveStarted(ready)) => require(ready.is_valid(), "Invalid live ready"),
            Ok(EngineResult::LiveStatus(progress)) => {
                require(progress.is_valid(), "Invalid live progress")
            }
            Ok(EngineResult::Stopped) => Ok(()),
        }
    }
}
impl WirePayload for Response {
    const PROTOCOL: &'static str = "omniusong.engine-reply.v1";
    const MAX_BYTES: usize = WIRE_BYTES;
}
pub(crate) fn matches_result(
    command: &EngineCommand,
    result: &Result<EngineResult, String>,
) -> bool {
    match result {
        Err(_) => true,
        Ok(result) => matches!(
            (command, result),
            (
                EngineCommand::StartLive { .. },
                EngineResult::LiveStarted(_)
            ) | (EngineCommand::Status, EngineResult::LiveStatus(_))
                | (EngineCommand::Devices, EngineResult::Devices(_))
                | (
                    EngineCommand::StartPlayback { .. },
                    EngineResult::Playing(_)
                )
                | (EngineCommand::Stop, EngineResult::Stopped)
                | (
                    EngineCommand::StartCapture { .. },
                    EngineResult::Capturing(_)
                )
                | (EngineCommand::FinishCapture, EngineResult::Captured(_))
                | (EngineCommand::Status, EngineResult::Status(_))
        ),
    }
}
pub(crate) struct Pending {
    pub id: u64,
    pub command: EngineCommand,
}
// Capacity-one acknowledged replies plus one actor outbox bound this inline
// value (currently 464 bytes); avoid an extra heap allocation for each completion.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Event {
    Completed(EngineReply),
    Failed(String),
}
/// One admitted request until its checked response is consumed. Drop never joins.
pub struct EngineClient {
    requests: SyncSender<Pending>,
    replies: Receiver<Event>,
    pending: Option<Pending>,
    next: u64,
    dead: Arc<AtomicBool>,
    live_lanes: ClientLanes,
    ready: Option<LiveReady>,
    parent_next: u64,
    next_urgent: u64,
    pending_panic: Option<PanicRequest>,
    seen_update: u64,
}
impl EngineClient {
    pub fn spawn(executable: PathBuf, store_root: PathBuf) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let dead = Arc::new(AtomicBool::new(false));
        let actor_dead = dead.clone();
        let (live_lanes, actor_lanes) = live_supervisor::lanes(dead.clone());
        thread::Builder::new()
            .name("audio-supervisor".into())
            .spawn(move || {
                // Returning retires the owned child before any wait for UI credit.
                let result =
                    live_supervisor::actor(executable, store_root, rx, &result_tx, actor_lanes);
                actor_dead.store(true, Ordering::Release);
                if let Err(error) = result {
                    // Capacity stays one; retain the terminal cause behind an
                    // already-queued completion. Dropping the UI receiver wakes
                    // this off-UI sender. The UI never joins this thread.
                    let _ = result_tx.send(Event::Failed(error));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            requests: tx,
            replies: result_rx,
            pending: None,
            next: 1,
            dead,
            live_lanes,
            ready: None,
            parent_next: 1,
            next_urgent: 1,
            pending_panic: None,
            seen_update: 0,
        })
    }
    pub fn try_submit(&mut self, command: EngineCommand) -> Result<(), String> {
        command_valid(&command)?;
        if matches!(
            command,
            EngineCommand::StartLive { .. }
                | EngineCommand::StartPlayback { .. }
                | EngineCommand::StartCapture { .. }
        ) && self.live_lanes.shared.stop_pending()
        {
            return Err("Waiting for reliable Stop acknowledgment".into());
        }
        if self.pending.is_some() {
            return Err("Audio engine is busy".into());
        }
        if self.dead.load(Ordering::Acquire) {
            return Err("Audio worker is disconnected; unsealed originals are retained".into());
        }
        let next = self
            .next
            .checked_add(1)
            .ok_or("Engine request sequence exhausted")?;
        let pending = Pending {
            id: self.next,
            command,
        };
        match self.requests.try_send(Pending {
            id: pending.id,
            command: pending.command.clone(),
        }) {
            Ok(()) => {
                self.pending = Some(pending);
                self.next = next;
                self.live_lanes.shared.notify();
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err("Audio request queue is full".into()),
            Err(TrySendError::Disconnected(_)) => Err("Audio worker disconnected".into()),
        }
    }
    pub fn try_poll(&mut self) -> Option<EngineReply> {
        match self.replies.try_recv() {
            Ok(Event::Completed(reply)) => {
                let valid = self.pending.as_ref().is_some_and(|p| {
                    p.id == reply.request_id && matches_result(&p.command, &reply.result)
                });
                if valid {
                    self.pending = None;
                    if matches!(&reply.result, Ok(EngineResult::Stopped)) {
                        self.ready = None;
                    }
                    if let Ok(EngineResult::LiveStarted(ready)) = &reply.result
                        && reply.request_id
                            > self.live_lanes.shared.stop_barrier.load(Ordering::Acquire)
                        && !self.live_lanes.shared.stop_pending()
                    {
                        self.ready = Some(ready.clone());
                        self.parent_next = ready.progress.next_sequence().unwrap_or(u64::MAX);
                    }
                    self.live_lanes.shared.notify();
                    Some(reply)
                } else {
                    self.dead.store(true, Ordering::Release);
                    Some(EngineReply {
                        request_id: self.pending.take().map_or(0, |p| p.id),
                        result: Err("Audio response correlation failed".into()),
                    })
                }
            }
            Ok(Event::Failed(error)) => {
                self.dead.store(true, Ordering::Release);
                Some(EngineReply {
                    request_id: self.pending.take().map_or(0, |p| p.id),
                    result: Err(error),
                })
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if let Some(p) = self.pending.take() {
                    Some(EngineReply {
                        request_id: p.id,
                        result: Err(
                            "Audio supervisor disconnected; unsealed originals are retained".into(),
                        ),
                    })
                } else {
                    None
                }
            }
        }
    }
    fn urgent_ticket(&mut self) -> Result<UrgentTicket, String> {
        let id = self.next_urgent;
        self.next_urgent = id
            .checked_add(1)
            .ok_or("Urgent ticket sequence exhausted")?;
        Ok(UrgentTicket {
            worker_generation: self.live_lanes.shared.generation,
            id,
        })
    }
    pub fn request_stop(&mut self) -> Result<UrgentTicket, String> {
        if self.dead.load(Ordering::Acquire) {
            return Err("Audio worker is retired".into());
        }
        let shared = &self.live_lanes.shared;
        let id = shared.stop_requested.load(Ordering::Acquire);
        if id > shared.stop_completed.load(Ordering::Acquire) {
            return Ok(UrgentTicket {
                worker_generation: shared.generation,
                id,
            });
        }
        let ticket = self.urgent_ticket()?;
        self.ready = None;
        self.live_lanes
            .shared
            .live_active
            .store(false, Ordering::Release);
        self.live_lanes
            .shared
            .stop_barrier
            .store(self.next.saturating_sub(1), Ordering::Release);
        self.live_lanes
            .shared
            .stop_requested
            .store(ticket.id, Ordering::Release);
        self.live_lanes.shared.notify();
        Ok(ticket)
    }
    pub fn request_panic_live(
        &mut self,
        scope: LiveScope,
    ) -> Result<UrgentTicket, LiveSubmitError> {
        if self.dead.load(Ordering::Acquire) {
            return Err(LiveSubmitError::Retired);
        }
        if self.live_lanes.shared.stop_pending() {
            return Err(LiveSubmitError::PanicPending);
        }
        if !self.live_lanes.shared.live_active.load(Ordering::Acquire) {
            return Err(LiveSubmitError::NotLive);
        }
        let ready = self.ready.as_ref().ok_or(LiveSubmitError::NotLive)?;
        if ready.scope != scope {
            return Err(LiveSubmitError::StaleScope);
        }
        if self.live_lanes.shared.faulted.load(Ordering::Acquire) {
            return Err(LiveSubmitError::Faulted);
        }
        if let Some(request) = self.pending_panic {
            return if request.scope == scope {
                Ok(request.ticket)
            } else {
                Err(LiveSubmitError::PanicPending)
            };
        }
        let ticket = self.urgent_ticket().map_err(|_| LiveSubmitError::Retired)?;
        let request = PanicRequest { ticket, scope };
        match self.live_lanes.panic.try_send(request) {
            Ok(()) => {
                self.pending_panic = Some(request);
                self.live_lanes.shared.notify();
                Ok(ticket)
            }
            Err(TrySendError::Full(_)) => Err(LiveSubmitError::PanicPending),
            Err(TrySendError::Disconnected(_)) => Err(LiveSubmitError::Retired),
        }
    }
    fn live_rejection(&self, scope: LiveScope, kind: LiveFaultKind, sequence: Option<u64>) {
        self.live_lanes
            .shared
            .faulted
            .store(true, Ordering::Release);
        let _ = self.live_lanes.fault.try_send(FaultRequest {
            scope,
            kind,
            sequence,
        });
        self.live_lanes.shared.notify();
    }
    pub fn try_push_live(&mut self, batch: LiveBatch) -> Result<LiveQueued, LiveSubmitError> {
        if self.dead.load(Ordering::Acquire) {
            return Err(LiveSubmitError::Retired);
        }
        if self.live_lanes.shared.stop_pending() || self.pending_panic.is_some() {
            return Err(LiveSubmitError::PanicPending);
        }
        if !self.live_lanes.shared.live_active.load(Ordering::Acquire) {
            return Err(LiveSubmitError::NotLive);
        }
        let ready = self.ready.as_ref().ok_or(LiveSubmitError::NotLive)?;
        if ready.progress.state != LiveState::Running {
            return Err(if ready.progress.state == LiveState::Faulted {
                LiveSubmitError::Faulted
            } else {
                LiveSubmitError::PanicPending
            });
        }
        if batch.scope != ready.scope {
            return Err(LiveSubmitError::StaleScope);
        }
        if self.live_lanes.shared.faulted.load(Ordering::Acquire) {
            return Err(LiveSubmitError::Faulted);
        }
        if batch.validate_shape(ready.sample_rate).is_err() {
            self.live_rejection(
                batch.scope,
                LiveFaultKind::InvalidBatch,
                Some(batch.sequence),
            );
            return Err(LiveSubmitError::Invalid);
        }
        if batch.sequence != self.parent_next {
            self.live_rejection(batch.scope, LiveFaultKind::Sequence, Some(batch.sequence));
            return Err(LiveSubmitError::Invalid);
        }
        let Some(next) = self.parent_next.checked_add(1) else {
            self.live_rejection(
                batch.scope,
                LiveFaultKind::CounterExhausted,
                Some(batch.sequence),
            );
            return Err(LiveSubmitError::Faulted);
        };
        match self.live_lanes.live.try_send(batch) {
            Ok(()) => {
                self.parent_next = next;
                self.live_lanes.shared.notify();
                Ok(LiveQueued {
                    scope: batch.scope,
                    sequence: batch.sequence,
                })
            }
            Err(TrySendError::Full(_)) => {
                self.live_rejection(
                    batch.scope,
                    LiveFaultKind::QueueOverflow,
                    Some(batch.sequence),
                );
                Err(LiveSubmitError::Saturated)
            }
            Err(TrySendError::Disconnected(_)) => Err(LiveSubmitError::Retired),
        }
    }
    pub fn try_poll_update(&mut self) -> Option<EngineUpdate> {
        let update = {
            let slot = self.live_lanes.shared.latest.try_lock().ok()?;
            let value = (*slot)?;
            if value.revision <= self.seen_update {
                return None;
            }
            value
        };
        self.seen_update = update.revision;
        if let Some(pending) = self.pending_panic
            && update
                .urgent
                .panic
                .is_some_and(|ack| ack.ticket == pending.ticket)
        {
            self.pending_panic = None;
        }
        if let Some(progress) = update.live {
            if let Some(ready) = &mut self.ready
                && ready.scope.session_id == progress.scope.session_id
            {
                if ready.scope != progress.scope {
                    ready.scope = progress.scope;
                    self.parent_next = progress.next_sequence().unwrap_or(u64::MAX);
                }
                ready.progress = progress;
            }
            if progress.state == LiveState::Stopped {
                self.ready = None;
            }
        } else {
            self.ready = None;
        }
        self.live_lanes.shared.notify();
        Some(update)
    }

    pub fn is_retired(&self) -> bool {
        self.dead.load(Ordering::Acquire)
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
}
pub(crate) fn architecture() -> Architecture {
    if cfg!(target_arch = "aarch64") {
        Architecture::Aarch64
    } else {
        Architecture::X86_64
    }
}
pub(crate) fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Windows
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_and_results_are_strict() {
        assert!(
            serde_json::from_str::<Request>(
                r#"{"request_id":1,"action":"command","command":{"command":"stop","extra":1}}"#
            )
            .is_err()
        );
        assert!(!matches_result(
            &EngineCommand::FinishCapture,
            &Ok(EngineResult::Stopped)
        ));
    }
    fn peer() -> (EngineClient, Receiver<Pending>, SyncSender<Event>) {
        let (tx, rx) = mpsc::sync_channel(1);
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        (
            EngineClient {
                requests: tx,
                replies: reply_rx,
                pending: None,
                next: 1,
                dead: Arc::new(AtomicBool::new(false)),
                live_lanes: live_supervisor::lanes(Arc::new(AtomicBool::new(false))).0,
                ready: None,
                parent_next: 1,
                next_urgent: 1,
                pending_panic: None,
                seen_update: 0,
            },
            rx,
            reply_tx,
        )
    }
    #[test]
    fn live_admission_and_reliable_stop_do_not_depend_on_control_credit() {
        let (mut client, _controls, _replies) = peer();
        let (lanes, _held) = live_supervisor::lanes(client.dead.clone());
        let scope = LiveScope {
            worker_generation: lanes.shared.generation,
            session_id: LiveId([7; 16]),
            panic_generation: 1,
        };
        let (admission, _renderer) = crate::live::live_pair(scope, 48000, 1).unwrap();
        client.ready = Some(LiveReady {
            scope,
            gui_clock: LiveClock {
                domain: "gui".into(),
                epoch: "one".into(),
                ticks_per_second_numerator: 1000,
                ticks_per_second_denominator: 1,
            },
            sources: vec![],
            output: DeviceInfo {
                id: "synthetic".into(),
                name: "No device".into(),
            },
            sample_rate: 48000,
            channels: 1,
            progress: admission.progress(),
        });
        lanes.shared.live_active.store(true, Ordering::Release);
        client.live_lanes = lanes;
        client.try_submit(EngineCommand::Devices).unwrap();
        for sequence in 1..=LIVE_QUEUE_BATCHES as u64 {
            client
                .try_push_live(LiveBatch::empty(scope, sequence, None))
                .unwrap();
        }
        assert_eq!(
            client.try_push_live(LiveBatch::empty(scope, 17, None)),
            Err(LiveSubmitError::Saturated)
        );
        assert!(client.live_lanes.shared.faulted.load(Ordering::Acquire));
        assert!(client.is_busy());
        let ticket = client.request_stop().unwrap();
        assert_eq!(client.request_stop().unwrap(), ticket);
        assert_eq!(
            client.request_panic_live(scope),
            Err(LiveSubmitError::PanicPending)
        );
        assert_eq!(
            client.try_push_live(LiveBatch::empty(scope, 17, None)),
            Err(LiveSubmitError::PanicPending)
        );
        assert!(client.ready.is_none());
    }
    #[test]
    fn admission_remains_busy_after_actor_consumes_request() {
        let (mut client, requests, replies) = peer();
        client.try_submit(EngineCommand::Stop).unwrap();
        let request = requests.try_recv().unwrap();
        assert!(client.try_submit(EngineCommand::Devices).is_err());
        replies
            .send(Event::Completed(EngineReply {
                request_id: request.id,
                result: Ok(EngineResult::Stopped),
            }))
            .unwrap();
        assert!(client.is_busy());
        assert!(client.try_poll().unwrap().result.is_ok());
        assert!(!client.is_busy());
        client.try_submit(EngineCommand::Devices).unwrap();
    }
    #[test]
    fn mismatched_result_retires_client_before_new_admission() {
        let (mut client, requests, replies) = peer();
        client.try_submit(EngineCommand::FinishCapture).unwrap();
        let request = requests.try_recv().unwrap();
        replies
            .send(Event::Completed(EngineReply {
                request_id: request.id,
                result: Ok(EngineResult::Stopped),
            }))
            .unwrap();
        assert!(client.try_poll().unwrap().result.is_err());
        assert!(client.try_submit(EngineCommand::Stop).is_err());
    }
    #[test]
    fn child_failure_returns_pending_identity_once_without_retry() {
        let (mut client, requests, replies) = peer();
        client
            .try_submit(EngineCommand::StartCapture { input_device: None })
            .unwrap();
        let request = requests.try_recv().unwrap();
        replies
            .send(Event::Failed("unsealed original retained".into()))
            .unwrap();
        let reply = client.try_poll().unwrap();
        assert_eq!(reply.request_id, request.id);
        assert!(reply.result.unwrap_err().contains("retained"));
        assert!(client.try_poll().is_none());
        assert!(client.is_retired());
        assert!(client.try_submit(EngineCommand::FinishCapture).is_err());
    }
    #[test]
    fn status_configuration_and_result_lifecycle_must_agree() {
        let mut status = crate::EngineStatus {
            state: "idle".into(),
            capture: None,
            sample_rate: 0,
            channels: 0,
            frames: 0,
            peak: 0.0,
            xruns: 0,
        };
        let response = |result| Response {
            update: None,
            reply: EngineReply {
                request_id: 1,
                result: Ok(result),
            },
        };
        assert!(
            response(EngineResult::Status(status.clone()))
                .validate()
                .is_ok()
        );
        assert!(
            response(EngineResult::Playing(status.clone()))
                .validate()
                .is_err()
        );
        status.state = "playing".into();
        assert!(
            response(EngineResult::Playing(status.clone()))
                .validate()
                .is_err()
        );
        status.sample_rate = 48000;
        status.channels = 2;
        assert!(
            response(EngineResult::Playing(status.clone()))
                .validate()
                .is_ok()
        );
        assert!(
            response(EngineResult::Capturing(status.clone()))
                .validate()
                .is_err()
        );
        status.state = "capture_fault".into();
        status.capture = Some(crate::CaptureIdentity {
            journal_path: PathBuf::from("/retained/take.sqlite3"),
            take_id: "take".into(),
        });
        assert!(
            response(EngineResult::Capturing(status.clone()))
                .validate()
                .is_ok()
        );
        assert!(response(EngineResult::Playing(status)).validate().is_err());
    }
}
