//! Nonblocking application lanes and the one private-socket owner.
use crate::transport::{
    Action, BUILD, Event, Pending, Request, Response, architecture, matches_result, platform,
};
use crate::*;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy)]
pub(crate) struct PanicRequest {
    pub ticket: UrgentTicket,
    pub scope: LiveScope,
}
#[derive(Clone, Copy)]
pub(crate) struct FaultRequest {
    pub scope: LiveScope,
    pub kind: LiveFaultKind,
    pub sequence: Option<u64>,
}
#[derive(Default)]
struct PerformanceMailbox {
    start: Option<PerformanceStart>,
    root: PathBuf,
    status: Option<PerformanceStatus>,
    finish: Option<(LiveScope, LiveId, Option<u64>)>,
}
pub(crate) struct Shared {
    pub generation: LiveId,
    pub stop_requested: AtomicU64,
    pub stop_completed: AtomicU64,
    pub stop_barrier: AtomicU64,
    pub faulted: AtomicBool,
    pub live_active: AtomicBool,
    pub performance_active: AtomicBool,
    pub finishing: AtomicBool,
    performance: Mutex<PerformanceMailbox>,
    pub latest: Mutex<Option<EngineUpdate>>,
    pub wake: SyncSender<()>,
}
impl Shared {
    pub fn performance_snapshot(&self) -> Option<PerformanceStatus> {
        self.performance.try_lock().ok()?.status.clone()
    }
    pub fn check_start(&self, request: &PerformanceStart) -> Result<(), String> {
        let slot = self
            .performance
            .try_lock()
            .map_err(|_| "Performance mailbox is busy")?;
        if slot
            .start
            .as_ref()
            .is_some_and(|old| old.id == request.id && old != request)
        {
            return Err("Recorded-performance Start ID has different immutable content".into());
        }
        if self.performance_active.load(Ordering::Acquire) && slot.start.as_ref() != Some(request) {
            return Err(
                "Wait for real Stop acknowledgment before another performance Start".into(),
            );
        }
        Ok(())
    }
    pub fn check_finish(
        &self,
        start_id: LiveId,
        scope: LiveScope,
        id: LiveId,
    ) -> Result<(), String> {
        let slot = self
            .performance
            .try_lock()
            .map_err(|_| "Performance mailbox is busy")?;
        let status = slot
            .status
            .as_ref()
            .ok_or("Performance has not published readiness")?;
        if status.identity.start_id != start_id
            || status.ready.as_ref().is_none_or(|ready| {
                ready.scope.worker_generation != scope.worker_generation
                    || ready.scope.session_id != scope.session_id
            })
            || status.live.is_none_or(|live| live.scope != scope)
        {
            return Err("Stale recorded-performance Finish scope".into());
        }
        if let Some((old_scope, old_id, _)) = slot.finish {
            return if old_scope == scope && old_id == id {
                Ok(())
            } else {
                Err("Performance already has a different immutable Finish intent".into())
            };
        }
        if !matches!(status.phase, PerformancePhase::Recording)
            || self.faulted.load(Ordering::Acquire)
        {
            return Err("Recorded performance is not healthy for Finish".into());
        }
        Ok(())
    }
    pub(crate) fn register_start(
        &self,
        request: &PerformanceStart,
        root: &std::path::Path,
    ) -> Result<(), String> {
        self.check_start(request)?;
        let mut slot = self
            .performance
            .lock()
            .map_err(|_| "Performance mailbox poisoned")?;
        if slot.start.as_ref() != Some(request) {
            *slot = PerformanceMailbox {
                start: Some(request.clone()),
                root: root.to_owned(),
                status: None,
                finish: None,
            };
            self.finishing.store(false, Ordering::Release);
        }
        self.performance_active.store(true, Ordering::Release);
        Ok(())
    }
    fn register_finish(
        &self,
        start_id: LiveId,
        scope: LiveScope,
        id: LiveId,
    ) -> Result<(), String> {
        self.check_finish(start_id, scope, id)?;
        let mut slot = self
            .performance
            .lock()
            .map_err(|_| "Performance mailbox poisoned")?;
        if slot.finish.is_none() {
            let accepted = self
                .latest
                .lock()
                .map_err(|_| "Live update mailbox poisoned")?
                .as_ref()
                .and_then(|update| update.live)
                .filter(|p| p.scope == scope)
                .ok_or("Missing authoritative Finish progress")?
                .worker_accepted_sequence;
            slot.finish = Some((scope, id, accepted));
        }
        Ok(())
    }
    /// Authenticate stage identity BEFORE an update can introduce its native
    /// live scope. The initial readiness remains immutable after first delivery.
    #[cfg(test)]
    pub(crate) fn publish_performance(&self, status: &PerformanceStatus) -> Result<bool, String> {
        self.publish_performance_inner(status, None)
    }
    fn publish_performance_response(
        &self,
        status: &PerformanceStatus,
        update: EngineUpdate,
    ) -> Result<bool, String> {
        self.publish_performance_inner(status, Some(update))
    }
    fn publish_performance_inner(
        &self,
        status: &PerformanceStatus,
        update: Option<EngineUpdate>,
    ) -> Result<bool, String> {
        if !status.is_valid() {
            return Err("Invalid performance status".into());
        }
        if status.phase == PerformancePhase::Starting
            && (status.ready.is_some()
                || status.live.is_some()
                || status.finish.is_some()
                || status.cut.is_some())
        {
            return Err(
                "Starting performance supplied premature live readiness or Finish evidence".into(),
            );
        }
        let mut slot = self
            .performance
            .lock()
            .map_err(|_| "Performance mailbox poisoned")?;
        let start = slot
            .start
            .as_ref()
            .ok_or("Unsolicited recorded-performance status")?;
        let expected_path = slot
            .root
            .join("captures")
            .join(start.directory_name())
            .join("take.sqlite3");
        if status.identity.start_id != start.id
            || status.identity.capture.take_id != start.take_id()
            || status.identity.capture.journal_path != expected_path
        {
            return Err("Recorded-performance stage identity changed".into());
        }
        if let Some(ready) = &status.ready {
            if ready.scope.worker_generation != self.generation
                || ready.gui_clock != start.gui_clock
                || start
                    .output_device
                    .as_ref()
                    .is_some_and(|id| id != &ready.output.id)
            {
                return Err(
                    "Recorded-performance native readiness differs from immutable Start".into(),
                );
            }
            if status.identity.descriptor_sha256.is_none() || status.native_sample_format.is_none()
            {
                return Err("Native readiness lacks its original descriptor or actual sample-format identity".into());
            }
            if status.finish.is_some_and(|fence| {
                fence.scope.worker_generation != ready.scope.worker_generation
                    || fence.scope.session_id != ready.scope.session_id
            }) {
                return Err("Recorded-performance Finish has a foreign native scope".into());
            }
        }
        if let Some(fence) = status.finish
            && slot.finish != Some((fence.scope, fence.id, fence.accepted_sequence))
        {
            return Err(
                "Recorded-performance Finish fence differs from actual parent drain/admission"
                    .into(),
            );
        }
        let first_ready =
            slot.status.as_ref().is_none_or(|old| old.ready.is_none()) && status.ready.is_some();
        if let Some(old) = &slot.status {
            if status.revision < old.revision || (status.revision == old.revision && status != old)
            {
                return Err("Recorded-performance status revision regressed or changed".into());
            }
            if old.identity.descriptor_sha256.is_some()
                && old.identity.descriptor_sha256 != status.identity.descriptor_sha256
                || old.ready.is_some() && old.ready != status.ready
                || old.native_sample_format.is_some()
                    && old.native_sample_format != status.native_sample_format
                || old.finish.is_some() && old.finish != status.finish
                || old.cut.is_some() && old.cut != status.cut
                || old.sealed.is_some() && old.sealed != status.sealed
                || old.error.is_some() && old.error != status.error
            {
                return Err(
                    "Recorded-performance immutable evidence changed or disappeared".into(),
                );
            }
            if old.phase != PerformancePhase::Starting && status.phase == PerformancePhase::Starting
                || !matches!(
                    old.phase,
                    PerformancePhase::Starting | PerformancePhase::Recording
                ) && status.phase == PerformancePhase::Recording
                || old.phase == PerformancePhase::Sealed && status.phase != PerformancePhase::Sealed
            {
                return Err("Recorded-performance lifecycle rearmed".into());
            }
            if old.native.pause_succeeded && !status.native.pause_succeeded
                || old.native.renderer_destroyed && !status.native.renderer_destroyed
                || old.native.error_destroyed && !status.native.error_destroyed
                || old.native.producer_destroyed && !status.native.producer_destroyed
            {
                return Err("Recorded-performance retirement observation regressed".into());
            }
            if let (Some(previous), Some(next)) = (&old.live, &status.live) {
                if previous.scope.worker_generation != next.scope.worker_generation
                    || previous.scope.session_id != next.scope.session_id
                    || next.scope.panic_generation < previous.scope.panic_generation
                    || next.scope.panic_generation
                        > previous.scope.panic_generation.saturating_add(1)
                    || next.frames < previous.frames
                    || previous.fault.is_some() && previous.fault != next.fault
                    || previous.scope == next.scope
                        && (next.worker_accepted_sequence < previous.worker_accepted_sequence
                            || next.callback_applied_sequence < previous.callback_applied_sequence)
                {
                    return Err("Recorded-performance live progress regressed".into());
                }
            } else if old.live.is_some() && status.live.is_none() {
                return Err("Recorded-performance live evidence disappeared".into());
            }
            if let (Some(previous), Some(next)) = (&old.writer, &status.writer) {
                if next.received_packets < previous.received_packets
                    || next.complete_callbacks < previous.complete_callbacks
                    || next.complete_frames < previous.complete_frames
                    || next.durable_callbacks < previous.durable_callbacks
                    || next.durable_frames < previous.durable_frames
                    || previous.first_error.is_some() && previous.first_error != next.first_error
                    || previous.capture_fault.is_some()
                        && previous.capture_fault != next.capture_fault
                    || previous.finish_cut.is_some() && previous.finish_cut != next.finish_cut
                    || previous
                        .durable_prefixes
                        .iter()
                        .zip(&next.durable_prefixes)
                        .any(|(old, new)| {
                            old.stream_id != new.stream_id
                                || new.next_sequence < old.next_sequence
                                || new.source_end.value() < old.source_end.value()
                                || new.recorded_units < old.recorded_units
                                || new.gap_units < old.gap_units
                                || new.gap_chunks < old.gap_chunks
                        })
                {
                    return Err("Recorded-performance durable progress regressed".into());
                }
            } else if old.writer.is_some() && status.writer.is_none() {
                return Err("Recorded-performance writer evidence disappeared".into());
            }
        }
        // Commit the authoritative update while this mailbox is still held.
        // A mismatched/regressing update must never expose a new GUI Ready.
        if let Some(update) = update {
            self.publish(update, first_ready)?;
        }
        if !matches!(
            status.phase,
            PerformancePhase::Starting | PerformancePhase::Recording
        ) {
            self.finishing.store(true, Ordering::Release);
        }
        slot.status = Some(status.clone());
        Ok(first_ready)
    }
    pub fn notify(&self) {
        let _ = self.wake.try_send(());
    }
    pub fn stop_pending(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire) > self.stop_completed.load(Ordering::Acquire)
    }
    pub(crate) fn publish(
        &self,
        update: EngineUpdate,
        allow_new_session: bool,
    ) -> Result<(), String> {
        if !update.is_valid() || update.worker_generation != self.generation {
            return Err("Invalid live worker update".into());
        }
        let mut slot = self
            .latest
            .lock()
            .map_err(|_| "Live update mailbox poisoned")?;
        if let Some(old) = *slot {
            if update.revision <= old.revision {
                return Err("Live update revision regressed".into());
            }
            if old.urgent.stop.is_some_and(|ack| {
                update
                    .urgent
                    .stop
                    .is_none_or(|next| next.ticket.id < ack.ticket.id)
            }) || old.urgent.panic.is_some_and(|ack| {
                update
                    .urgent
                    .panic
                    .is_none_or(|next| next.ticket.id < ack.ticket.id)
            }) {
                return Err("Urgent acknowledgment regressed".into());
            }
            let new_session = update.live.is_some_and(|p| {
                old.live
                    .is_none_or(|o| o.scope.session_id != p.scope.session_id)
            });
            if new_session && !allow_new_session {
                return Err("Unsolicited new live session".into());
            }
            if old.first_fault.is_some() && old.first_fault != update.first_fault && !new_session {
                return Err("Live first fault changed or disappeared".into());
            }
            if let (Some(previous), Some(next)) = (old.live, update.live) {
                if previous.scope == next.scope {
                    if next.frames < previous.frames
                        || next.worker_accepted_sequence < previous.worker_accepted_sequence
                        || next.callback_applied_sequence < previous.callback_applied_sequence
                        || next.rejected_batches < previous.rejected_batches
                        || next.overflow_batches < previous.overflow_batches
                        || next.late_releases < previous.late_releases
                        || next.lease_deadline_frame < previous.lease_deadline_frame
                    {
                        return Err("Live progress regressed".into());
                    }
                    if previous.fault.is_some() && next.fault != previous.fault {
                        return Err("Live fault changed or disappeared".into());
                    }
                    if matches!(
                        previous.state,
                        LiveState::Faulted
                            | LiveState::Stopped
                            | LiveState::FinishedCut
                            | LiveState::Finishing
                    ) && next.state == LiveState::Running
                    {
                        return Err("Ended live scope rearmed".into());
                    }
                } else if previous.scope.session_id == next.scope.session_id
                    && previous.scope.panic_generation.checked_add(1)
                        != Some(next.scope.panic_generation)
                {
                    return Err("Live panic generation skipped or regressed".into());
                }
            } else if old.live.is_some() && update.live.is_none() {
                return Err("Ended live scope summary disappeared".into());
            }
        }
        // Reset only the previous session's latch, then apply the NEW snapshot.
        // A callback may fault after LiveReady was made but before this update.
        if allow_new_session {
            self.faulted.store(false, Ordering::Release);
        }
        if update
            .live
            .is_some_and(|p| matches!(p.state, LiveState::Faulted | LiveState::Panicking))
        {
            self.faulted.store(true, Ordering::Release);
        }
        *slot = Some(update);
        Ok(())
    }
}
pub(crate) struct ClientLanes {
    pub live: SyncSender<LiveBatch>,
    pub panic: SyncSender<PanicRequest>,
    pub fault: SyncSender<FaultRequest>,
    pub shared: Arc<Shared>,
}
pub(crate) struct ActorLanes {
    live: Receiver<LiveBatch>,
    panic: Receiver<PanicRequest>,
    fault: Receiver<FaultRequest>,
    wake: Receiver<()>,
    shared: Arc<Shared>,
}
pub(crate) fn lanes(_dead: Arc<AtomicBool>) -> (ClientLanes, ActorLanes) {
    let (lt, lr) = mpsc::sync_channel(LIVE_QUEUE_BATCHES);
    let (pt, pr) = mpsc::sync_channel(1);
    let (ft, fr) = mpsc::sync_channel(1);
    let (wt, wr) = mpsc::sync_channel(1);
    let shared = Arc::new(Shared {
        generation: LiveId::random(),
        stop_requested: AtomicU64::new(0),
        stop_completed: AtomicU64::new(0),
        stop_barrier: AtomicU64::new(0),
        faulted: AtomicBool::new(false),
        live_active: AtomicBool::new(false),
        performance_active: AtomicBool::new(false),
        finishing: AtomicBool::new(false),
        performance: Mutex::new(PerformanceMailbox::default()),
        latest: Mutex::new(None),
        wake: wt,
    });
    (
        ClientLanes {
            live: lt,
            panic: pt,
            fault: ft,
            shared: shared.clone(),
        },
        ActorLanes {
            live: lr,
            panic: pr,
            fault: fr,
            wake: wr,
            shared,
        },
    )
}
fn starts(command: &EngineCommand) -> bool {
    matches!(
        command,
        EngineCommand::StartLive { .. }
            | EngineCommand::StartPerformance { .. }
            | EngineCommand::StartPlayback { .. }
            | EngineCommand::StartCapture { .. }
    )
}
// All values stay within the actor's fixed one-control/16-batch budget. Keeping
// the original batch inline avoids another heap owner for queued musical data.
#[allow(clippy::large_enum_variant)]
enum ControlDispatch {
    Preempted(Pending),
    Batch { pending: Pending, batch: LiveBatch },
    Command(Pending),
}
fn dispatch_control(
    pending: Pending,
    live: &Receiver<LiveBatch>,
    stop_barrier: u64,
) -> ControlDispatch {
    if (starts(&pending.command)
        || matches!(pending.command, EngineCommand::FinishPerformance { .. }))
        && pending.id <= stop_barrier
    {
        return ControlDispatch::Preempted(pending);
    }
    if matches!(pending.command, EngineCommand::FinishPerformance { .. })
        && let Ok(batch) = live.try_recv()
    {
        return ControlDispatch::Batch { pending, batch };
    }
    ControlDispatch::Command(pending)
}
fn stop_ack_matches(update: &EngineUpdate, ticket: UrgentTicket, barrier: u64) -> bool {
    update.is_valid()
        && update.worker_generation == ticket.worker_generation
        && update
            .urgent
            .stop
            .is_some_and(|ack| ack.ticket == ticket && ack.cancelled_through_control == barrier)
}
#[cfg(unix)]
pub(crate) fn actor(
    executable: PathBuf,
    store_root: PathBuf,
    requests: Receiver<Pending>,
    replies: &SyncSender<Event>,
    lanes: ActorLanes,
) -> Result<(), String> {
    use omniusong_core::contracts::WorkerRole;
    use omniusong_ipc::{
        FrameCeiling,
        unix::{LaunchConfig, default_worker_limits, spawn_private},
    };
    let shared = &lanes.shared;
    let mut command = std::process::Command::new(executable);
    command
        .arg("--audio-worker")
        .stdout(std::process::Stdio::null());
    let mut child = spawn_private(
        command,
        LaunchConfig {
            expected_role: WorkerRole::Engine,
            expected_platform: platform(),
            expected_architecture: architecture(),
            expected_build_id: Some(BUILD.into()),
            limits: default_worker_limits(),
            frame_ceiling: FrameCeiling::new(64 * 1024).map_err(|e| e.to_string())?,
            handshake_timeout: Duration::from_secs(5),
        },
    )
    .map_err(|e| e.to_string())?;
    fn exchange(
        child: &mut omniusong_ipc::unix::PrivateChild,
        id: u64,
        action: Action,
    ) -> Result<Response, String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        child
            .send_payload(
                Request {
                    request_id: id,
                    action,
                },
                Duration::from_secs(15),
            )
            .map_err(|e| e.to_string())?;
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or("Audio request deadline exhausted")?;
        let response = child
            .receive::<Response>(left)
            .map_err(|e| {
                format!(
                    "Audio worker deadline/channel failure: {e}; unsealed originals are retained"
                )
            })?
            .ok_or("Audio worker closed its private channel")?
            .payload;
        if response.reply.request_id != id {
            return Err("Audio private response identity mismatch".into());
        }
        Ok(response)
    }
    let stage_root = store_root.clone();
    let init = exchange(
        &mut child,
        0,
        Action::Initialize {
            store_root,
            worker_generation: shared.generation,
            profile: LIVE_PROTOCOL.into(),
        },
    )?;
    match init.reply.result {
        Ok(EngineResult::Initialized {
            profile,
            worker_generation,
        }) if profile == LIVE_PROTOCOL && worker_generation == shared.generation => {}
        Err(e) => return Err(e),
        _ => return Err("Unexpected audio initialization/profile result".into()),
    }
    if let Some(update) = init.update {
        if update.live.is_some()
            || update.first_fault.is_some()
            || update.urgent != UrgentProgress::default()
        {
            return Err("Initialization supplied unexpected live state".into());
        }
        shared.publish(update, false)?;
    }
    let mut wire_id = 1u64;
    let mut outbox = None;
    let mut incoming = None;
    let mut last_status = Instant::now();
    let mut deferred_rejection: Option<(String, Instant, LiveScope)> = None;
    let mut stop_sent: Option<(u64, Instant)> = None;
    let mut panic_sent: Option<(UrgentTicket, LiveScope, Instant)> = None;
    loop {
        if let Some((error, deadline, _)) = &deferred_rejection
            && Instant::now() >= *deadline
        {
            return Err(error.clone());
        }
        if stop_sent.is_some_and(|(_, deadline)| Instant::now() >= deadline) {
            return Err("Native performance Stop did not prove quiescence before its supervisor deadline; original retained".into());
        }
        if panic_sent.is_some_and(|(_, _, deadline)| Instant::now() >= deadline) {
            return Err("Live Panic did not produce its callback acknowledgment before the supervisor deadline".into());
        }
        // Never block a live child waiting for GUI reply credit.
        if let Some(event) = outbox.take() {
            match replies.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => outbox = Some(event),
                Err(TrySendError::Disconnected(_)) => return Ok(()),
            }
        }
        if incoming.is_none() {
            match requests.try_recv() {
                Ok(p) => incoming = Some(p),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let _ = exchange(
                        &mut child,
                        wire_id,
                        Action::Command {
                            command: EngineCommand::Stop,
                        },
                    );
                    return Ok(());
                }
            }
        }
        // Stop may preempt an admitted-but-not-yet-sent Finish. Preserve the
        // control completion without ever sending that normal-seal intent.
        if outbox.is_none()
            && incoming.as_ref().is_some_and(|pending: &Pending| {
                matches!(pending.command, EngineCommand::FinishPerformance { .. })
                    && pending.id <= shared.stop_barrier.load(Ordering::Acquire)
            })
        {
            let cancelled = incoming.take().expect("checked pending Finish");
            outbox = Some(Event::Completed(EngineReply {
                request_id: cancelled.id,
                result: Err(
                    "Performance Finish was preempted by reliable Stop; original retained".into(),
                ),
            }));
        }
        let stop_id = shared.stop_requested.load(Ordering::Acquire);
        let mut normal = None;
        let stopping = stop_id > shared.stop_completed.load(Ordering::Acquire);
        let performance = shared.performance_active.load(Ordering::Acquire);
        let status_interval = if performance || stopping || panic_sent.is_some() {
            Duration::from_millis(40)
        } else {
            Duration::from_millis(250)
        };
        let action = if stopping && stop_sent.is_none_or(|(sent, _)| sent != stop_id) {
            let superseded = lanes
                .panic
                .try_recv()
                .ok()
                .map(|p| (p.ticket, p.scope))
                .or(panic_sent.map(|(ticket, scope, _)| (ticket, scope)));
            for _ in 0..LIVE_QUEUE_BATCHES {
                if lanes.live.try_recv().is_err() {
                    break;
                }
            }
            Some(Action::UrgentStop {
                ticket: UrgentTicket {
                    worker_generation: shared.generation,
                    id: stop_id,
                },
                cancelled_through_control: shared.stop_barrier.load(Ordering::Acquire),
                superseded_panic: superseded,
            })
        } else if stopping {
            (last_status.elapsed() >= status_interval).then_some(Action::Command {
                command: EngineCommand::Status,
            })
        } else if let Ok(fault) = lanes.fault.try_recv() {
            Some(Action::LiveFault {
                scope: fault.scope,
                kind: fault.kind,
                sequence: fault.sequence,
            })
        } else if let Ok(panic) = lanes.panic.try_recv() {
            for _ in 0..LIVE_QUEUE_BATCHES {
                if lanes.live.try_recv().is_err() {
                    break;
                }
            }
            Some(Action::PanicLive {
                ticket: panic.ticket,
                scope: panic.scope,
            })
        } else if outbox.is_none() && incoming.is_some() {
            let pending = incoming.take().expect("pending control");
            match dispatch_control(
                pending,
                &lanes.live,
                shared.stop_barrier.load(Ordering::Acquire),
            ) {
                ControlDispatch::Preempted(pending) => {
                    outbox = Some(Event::Completed(EngineReply { request_id: pending.id, result: Err("Audio start/Finish cancelled by the reliable Stop fence; original retained".into()) }));
                    continue;
                }
                ControlDispatch::Batch { pending, batch } => {
                    incoming = Some(pending);
                    Some(Action::LiveBatch { batch })
                }
                ControlDispatch::Command(pending) => {
                    let command = pending.command.clone();
                    normal = Some(pending);
                    Some(Action::Command { command })
                }
            }
        } else if let Ok(batch) = lanes.live.try_recv() {
            Some(Action::LiveBatch { batch })
        } else if last_status.elapsed() >= status_interval {
            Some(Action::Command {
                command: EngineCommand::Status,
            })
        } else {
            None
        };
        let Some(action) = action else {
            let _ = lanes.wake.recv_timeout(Duration::from_millis(10));
            continue;
        };
        if let Some(pending) = &normal {
            if matches!(pending.command, EngineCommand::FinishPerformance { .. })
                && shared.stop_pending()
            {
                outbox = Some(Event::Completed(EngineReply {
                    request_id: pending.id,
                    result: Err(
                        "Performance Finish was preempted by reliable Stop; original retained"
                            .into(),
                    ),
                }));
                continue;
            }
            let validation = match &pending.command {
                EngineCommand::StartPerformance { request } => {
                    shared.register_start(request, &stage_root)
                }
                EngineCommand::FinishPerformance {
                    start_id,
                    scope,
                    finish_id,
                } => shared.register_finish(*start_id, *scope, *finish_id),
                EngineCommand::StartLive { .. }
                | EngineCommand::StartPlayback { .. }
                | EngineCommand::StartCapture { .. }
                    if shared.performance_active.load(Ordering::Acquire) =>
                {
                    Err("Stop the retained performance before another session".into())
                }
                _ => Ok(()),
            };
            if let Err(error) = validation {
                outbox = Some(Event::Completed(EngineReply {
                    request_id: pending.id,
                    result: Err(error),
                }));
                continue;
            }
        }
        let urgent_stop = matches!(&action, Action::UrgentStop { .. });
        let scoped = match &action {
            Action::LiveBatch { batch } => Some(batch.scope),
            Action::LiveFault { scope, .. } | Action::PanicLive { scope, .. } => Some(*scope),
            _ => None,
        };
        if let Some(scope) = scoped {
            let latest = shared
                .latest
                .lock()
                .map_err(|_| "Live update mailbox poisoned")?;
            if latest
                .as_ref()
                .and_then(|u| u.live)
                .is_none_or(|p| p.scope != scope)
            {
                continue;
            }
        }
        let expected_panic = match &action {
            Action::PanicLive { ticket, scope } => Some((*ticket, *scope)),
            _ => None,
        };
        let internal_live = matches!(
            &action,
            Action::LiveBatch { .. } | Action::PanicLive { .. } | Action::LiveFault { .. }
        );
        let expected = normal
            .as_ref()
            .map(|p| p.command.clone())
            .unwrap_or(EngineCommand::Status);
        let mut response = exchange(&mut child, wire_id, action)?;
        wire_id = wire_id
            .checked_add(1)
            .ok_or("Audio wire sequence exhausted")?;
        last_status = Instant::now();
        // Reject a mismatched reply kind before publishing ANY new readiness or
        // stage observation to the GUI's cumulative mailboxes.
        if normal.is_some() && !matches_result(&expected, &response.reply.result)
            || internal_live
                && !matches!(
                    &response.reply.result,
                    Ok(EngineResult::LiveStatus(_)) | Err(_)
                )
            || urgent_stop
                && !matches!(
                    &response.reply.result,
                    Ok(EngineResult::Stopped | EngineResult::Performance(_)) | Err(_)
                )
            || normal.is_none()
                && !internal_live
                && !urgent_stop
                && !matches_result(&EngineCommand::Status, &response.reply.result)
        {
            return Err("Audio result kind mismatch".into());
        }
        let performance_response =
            matches!(&response.reply.result, Ok(EngineResult::Performance(_)));
        let performance_ready =
            if let Ok(EngineResult::Performance(status)) = &response.reply.result {
                if status.ready.as_ref().is_some_and(|ready| {
                    response.update.is_none_or(|update| {
                        update.live.is_none_or(|live| {
                            live.scope.worker_generation != ready.scope.worker_generation
                                || live.scope.session_id != ready.scope.session_id
                        })
                    })
                }) {
                    return Err(
                        "Native performance readiness lacks its matching authoritative live update"
                            .into(),
                    );
                }
                shared.publish_performance_response(
                    status,
                    response
                        .update
                        .ok_or("Missing authoritative performance update")?,
                )?
            } else {
                false
            };
        let starts_live =
            performance_ready || matches!(&response.reply.result, Ok(EngineResult::LiveStarted(_)));
        if let Some(update) = response.update {
            if !performance_response {
                shared.publish(update, starts_live)?;
            }
            if deferred_rejection.as_ref().is_some_and(|(_, _, scope)| {
                update.live.is_some_and(|p| {
                    p.scope.session_id == scope.session_id
                        && p.state == LiveState::Faulted
                        && p.fault.is_some()
                })
            }) {
                deferred_rejection = None;
            }
        } else if internal_live
            || urgent_stop
            || starts_live
            || matches!(&response.reply.result, Ok(EngineResult::Performance(_)))
        {
            return Err("Missing live authoritative update".into());
        }
        if let Some((ticket, scope)) = expected_panic
            && response.reply.result.is_ok()
        {
            let acknowledged = response.update.is_some_and(|u| {
                u.urgent.panic.is_some_and(|ack| {
                    ack.ticket == ticket
                        && ack.requested_scope == scope
                        && matches!(ack.outcome, PanicOutcome::Applied { new_scope } if u.live.is_some_and(|live| live.scope == new_scope))
                })
            });
            if !acknowledged {
                if !matches!(&response.reply.result, Ok(EngineResult::LiveStatus(progress)) if progress.scope == scope && progress.state == LiveState::Panicking)
                {
                    return Err("Panic callback acknowledgment mismatch".into());
                }
                panic_sent = Some((ticket, scope, Instant::now() + Duration::from_secs(15)));
            } else if response.update.is_some_and(|update| {
                update.first_fault.is_none()
                    && update.live.is_some_and(|live| {
                        live.state == LiveState::Running && live.fault.is_none()
                    })
            }) {
                shared.faulted.store(false, Ordering::Release);
            }
        }
        if urgent_stop {
            if !matches!(
                &response.reply.result,
                Ok(EngineResult::Stopped | EngineResult::Performance(_))
            ) {
                return Err(response
                    .reply
                    .result
                    .err()
                    .unwrap_or("Unexpected urgent Stop result".into()));
            }
            stop_sent = Some((stop_id, Instant::now() + Duration::from_secs(15)));
        }
        if let Some((ticket, scope, _)) = panic_sent
            && response.update.is_some_and(|update| {
                update
                    .urgent
                    .panic
                    .is_some_and(|ack| ack.ticket == ticket && ack.requested_scope == scope)
            })
        {
            if response.update.is_some_and(|update| update.first_fault.is_none() && update.live.is_some_and(|live| live.state == LiveState::Running && live.fault.is_none()) && update.urgent.panic.is_some_and(|ack| matches!(ack.outcome, PanicOutcome::Applied { new_scope } if update.live.is_some_and(|live| live.scope == new_scope)))) {
                shared.faulted.store(false, Ordering::Release);
            }
            panic_sent = None;
        }
        if shared.stop_pending() {
            let latest = shared
                .latest
                .lock()
                .map_err(|_| "Live update mailbox poisoned")?;
            let acknowledged = latest.as_ref().is_some_and(|update| {
                stop_ack_matches(
                    update,
                    UrgentTicket {
                        worker_generation: shared.generation,
                        id: stop_id,
                    },
                    shared.stop_barrier.load(Ordering::Acquire),
                )
            });
            if acknowledged {
                shared.live_active.store(false, Ordering::Release);
                shared.performance_active.store(false, Ordering::Release);
                shared.stop_completed.store(stop_id, Ordering::Release);
                stop_sent = None;
            } else if urgent_stop && matches!(&response.reply.result, Ok(EngineResult::Stopped)) {
                return Err("Urgent Stop acknowledgment mismatch".into());
            }
        }
        if urgent_stop {
            // A pending performance status is an observation, never a Stop ACK.
        } else if let Some(pending) = normal {
            if !matches_result(&expected, &response.reply.result) {
                return Err("Audio result kind mismatch".into());
            }
            if matches!(&response.reply.result, Ok(EngineResult::LiveStarted(_))) {
                shared
                    .live_active
                    .store(!shared.stop_pending(), Ordering::Release);
            }
            if matches!(&response.reply.result, Ok(EngineResult::Stopped)) {
                shared.live_active.store(false, Ordering::Release);
                shared.performance_active.store(false, Ordering::Release);
            }
            response.reply.request_id = pending.id;
            outbox = Some(Event::Completed(response.reply));
        } else if internal_live {
            if let Err(error) = &response.reply.result {
                let known = response.update.is_some_and(|u| {
                    u.live
                        .is_some_and(|p| p.state == LiveState::Faulted && p.fault.is_some())
                });
                if !known {
                    if let Some(progress) = response
                        .update
                        .and_then(|u| u.live)
                        .filter(|p| p.state == LiveState::Panicking && p.fault.is_none())
                    {
                        if deferred_rejection.is_none() {
                            deferred_rejection = Some((
                                error.clone(),
                                Instant::now() + Duration::from_secs(15),
                                progress.scope,
                            ));
                        }
                    } else {
                        return Err(error.clone());
                    }
                }
            }
            // Only authenticated sticky faults may represent a known live rejection.
            if !matches!(
                &response.reply.result,
                Ok(EngineResult::LiveStatus(_)) | Err(_)
            ) {
                return Err("Unexpected live admission result".into());
            }
        } else {
            if !matches_result(&EngineCommand::Status, &response.reply.result) {
                return Err("Audio heartbeat result kind mismatch".into());
            }
            response.reply.result?;
        }
        if performance_ready && !shared.stop_pending() && !shared.finishing.load(Ordering::Acquire)
        {
            shared.live_active.store(true, Ordering::Release);
        }
    }
}
#[cfg(not(unix))]
pub(crate) fn actor(
    _: PathBuf,
    _: PathBuf,
    _: Receiver<Pending>,
    _: &SyncSender<Event>,
    _: ActorLanes,
) -> Result<(), String> {
    Err("Private audio worker launch is not implemented on this platform".into())
}

#[cfg(test)]
pub(crate) fn performance_fixture(
    generation: LiveId,
    root: &std::path::Path,
) -> (PerformanceStart, PerformanceStatus, PerformanceStatus) {
    let start = PerformanceStart {
        id: LiveId([11; 16]),
        output_device: Some("synthetic-output".into()),
        gui_clock: LiveClock {
            domain: "gui".into(),
            epoch: "fixture".into(),
            ticks_per_second_numerator: 1000,
            ticks_per_second_denominator: 1,
        },
    };
    let scope = LiveScope {
        worker_generation: generation,
        session_id: LiveId([12; 16]),
        panic_generation: 1,
    };
    let (admission, _renderer) = crate::live::live_pair(scope, 48000, 1).unwrap();
    let ready = LiveReady {
        scope,
        gui_clock: start.gui_clock.clone(),
        sources: [
            LiveSourceKind::ComputerKeys,
            LiveSourceKind::PointerKeys,
            LiveSourceKind::Pads,
            LiveSourceKind::AccessiblePulse,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, kind)| LiveSourceBinding {
            kind,
            handle: (index + 1) as u8,
            connection_generation: 1,
        })
        .collect(),
        output: DeviceInfo {
            id: "synthetic-output".into(),
            name: "Software protocol fixture; no native device".into(),
        },
        sample_rate: 48000,
        channels: 1,
        progress: admission.progress(),
    };
    let starting = PerformanceStatus {
        revision: 1,
        identity: PerformanceIdentity {
            start_id: start.id,
            capture: CaptureIdentity {
                journal_path: root
                    .join("captures")
                    .join(start.directory_name())
                    .join("take.sqlite3"),
                take_id: start.take_id(),
            },
            descriptor_sha256: None,
        },
        phase: PerformancePhase::Starting,
        ready: None,
        native_sample_format: None,
        live: None,
        writer: None,
        finish: None,
        cut: None,
        native: NativeRetirementStatus::default(),
        sealed: None,
        error: None,
    };
    let mut recording = starting.clone();
    recording.revision = 2;
    recording.identity.descriptor_sha256 = Some("a".repeat(64));
    recording.phase = PerformancePhase::Recording;
    recording.live = Some(ready.progress);
    recording.ready = Some(ready);
    recording.native_sample_format = Some("f32".into());
    (start, starting, recording)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn update(shared: &Shared, revision: u64) -> EngineUpdate {
        let scope = LiveScope {
            worker_generation: shared.generation,
            session_id: LiveId([2; 16]),
            panic_generation: 1,
        };
        let (admission, _renderer) = crate::live::live_pair(scope, 48000, 1).unwrap();
        EngineUpdate {
            worker_generation: shared.generation,
            revision,
            live: Some(admission.progress()),
            urgent: UrgentProgress::default(),
            first_fault: None,
        }
    }
    #[test]
    fn cumulative_mailbox_rejects_regression_and_changed_first_fault() {
        let (client, _actor) = lanes(Arc::new(AtomicBool::new(false)));
        let shared = &client.shared;
        let mut first = update(shared, 1);
        first.live.as_mut().unwrap().frames = 100;
        shared.publish(first, true).unwrap();
        let mut lower = first;
        lower.revision = 2;
        lower.live.as_mut().unwrap().frames = 99;
        assert!(
            shared
                .publish(lower, false)
                .unwrap_err()
                .contains("regressed")
        );
        let mut fault = first;
        fault.revision = 2;
        let p = fault.live.as_mut().unwrap();
        p.state = LiveState::Faulted;
        p.lease_credit = None;
        let evidence = LiveFault {
            scope: p.scope,
            kind: LiveFaultKind::Clock,
            frame: 100,
            sequence: None,
        };
        p.fault = Some(evidence);
        fault.first_fault = Some(evidence);
        shared.publish(fault, false).unwrap();
        let mut changed = fault;
        changed.revision = 3;
        changed.first_fault.as_mut().unwrap().kind = LiveFaultKind::Device;
        assert!(shared.publish(changed, false).is_err());
    }
    #[test]
    fn callback_fault_in_new_update_wins_stale_healthy_ready() {
        let (client, _actor) = lanes(Arc::new(AtomicBool::new(false)));
        let shared = &client.shared;
        let first = update(shared, 1);
        shared.publish(first, true).unwrap();
        shared.faulted.store(true, Ordering::Release);
        let mut next = update(shared, 2);
        let progress = next.live.as_mut().unwrap();
        progress.scope.session_id = LiveId([3; 16]);
        progress.state = LiveState::Faulted;
        progress.lease_credit = None;
        let fault = LiveFault {
            scope: progress.scope,
            kind: LiveFaultKind::Device,
            frame: 0,
            sequence: None,
        };
        progress.fault = Some(fault);
        next.first_fault = Some(fault);
        shared.publish(next, true).unwrap();
        assert!(shared.faulted.load(Ordering::Acquire));
    }
    #[test]
    fn stopped_summary_cannot_rearm_or_disappear() {
        let (client, _actor) = lanes(Arc::new(AtomicBool::new(false)));
        let shared = &client.shared;
        let mut first = update(shared, 1);
        let p = first.live.as_mut().unwrap();
        p.state = LiveState::Stopped;
        p.lease_credit = None;
        shared.publish(first, true).unwrap();
        let mut next = update(shared, 2);
        next.live.as_mut().unwrap().scope = first.live.unwrap().scope;
        assert!(shared.publish(next, false).is_err());
        next.live = None;
        assert!(shared.publish(next, false).is_err());
    }
    #[test]
    fn performance_ready_is_authenticated_once_and_immutable_stage_fields_cannot_change() {
        let (client, _actor) = lanes(Arc::new(AtomicBool::new(false)));
        let shared = &client.shared;
        let root = std::path::Path::new("/synthetic-native");
        let (start, starting, recording) = performance_fixture(shared.generation, root);
        shared.register_start(&start, root).unwrap();
        assert!(!shared.publish_performance(&starting).unwrap());
        assert!(shared.performance_snapshot().unwrap().ready.is_none());
        assert!(shared.publish_performance(&recording).unwrap());
        assert!(!shared.publish_performance(&recording).unwrap());
        for case in 0..9 {
            let mut changed = recording.clone();
            changed.revision += 1;
            match case {
                0 => changed.identity.capture.journal_path = root.join("different.sqlite3"),
                1 => changed.identity.capture.take_id.push('x'),
                2 => changed.identity.descriptor_sha256 = Some("b".repeat(64)),
                3 => changed.ready.as_mut().unwrap().gui_clock.epoch = "changed".into(),
                4 => changed.ready.as_mut().unwrap().output.id = "other".into(),
                5 => changed.ready.as_mut().unwrap().scope.worker_generation = LiveId([99; 16]),
                6 => changed.ready.as_mut().unwrap().scope.session_id = LiveId([99; 16]),
                7 => changed.native_sample_format = Some("i16".into()),
                _ => changed.identity.descriptor_sha256 = None,
            }
            assert!(shared.publish_performance(&changed).is_err(), "case {case}");
            assert_eq!(shared.performance_snapshot().unwrap(), recording);
        }
        let mut changed = start.clone();
        changed.gui_clock.epoch = "different".into();
        assert!(shared.check_start(&changed).is_err());
        assert!(shared.check_start(&start).is_ok());
    }
    #[test]
    fn finish_fence_binds_actual_worker_admission_after_the_parent_queue_drain() {
        let (client, _actor) = lanes(Arc::new(AtomicBool::new(false)));
        let shared = &client.shared;
        let root = std::path::Path::new("/synthetic-native");
        let (start, _, mut status) = performance_fixture(shared.generation, root);
        shared.register_start(&start, root).unwrap();
        shared.publish_performance(&status).unwrap();
        let mut actual = status.live.unwrap();
        actual.worker_accepted_sequence = Some(16);
        actual.callback_applied_sequence = Some(4);
        shared
            .publish(
                EngineUpdate {
                    worker_generation: shared.generation,
                    revision: 1,
                    live: Some(actual),
                    urgent: UrgentProgress::default(),
                    first_fault: None,
                },
                true,
            )
            .unwrap();
        let finish_id = LiveId([13; 16]);
        shared
            .register_finish(start.id, actual.scope, finish_id)
            .unwrap();
        status.revision += 1;
        status.phase = PerformancePhase::Finishing;
        actual.state = LiveState::Finishing;
        actual.lease_credit = None;
        status.live = Some(actual);
        status.finish = Some(LiveFinishFence {
            id: finish_id,
            scope: actual.scope,
            accepted_sequence: Some(16),
        });
        assert!(!shared.publish_performance(&status).unwrap());
        let mut forged = status.clone();
        forged.revision += 1;
        forged.finish.as_mut().unwrap().accepted_sequence = Some(15);
        forged.live.as_mut().unwrap().worker_accepted_sequence = Some(15);
        assert!(shared.publish_performance(&forged).is_err());
        assert!(
            shared
                .check_finish(start.id, actual.scope, finish_id)
                .is_ok()
        );
        assert!(
            shared
                .check_finish(start.id, actual.scope, LiveId([14; 16]))
                .is_err()
        );
    }
    #[test]
    fn actor_finish_dispatch_drains_all_sixteen_original_batches_before_control_and_stop_preempts()
    {
        let (sender, receiver) = mpsc::sync_channel(LIVE_QUEUE_BATCHES);
        let scope = LiveScope {
            worker_generation: LiveId([1; 16]),
            session_id: LiveId([2; 16]),
            panic_generation: 1,
        };
        let mut originals = Vec::new();
        for sequence in 1..=16 {
            let mut batch = LiveBatch::empty(scope, sequence, None);
            batch
                .try_push(LiveEvent {
                    source: 1,
                    connection_generation: 1,
                    observed_ticks: u64::MAX - 16 + sequence,
                    event: LiveEventKind::NoteOn {
                        note: sequence,
                        voice: sequence,
                        key: 60,
                        velocity16: sequence as u16,
                        patch: LivePatch::Keys,
                    },
                })
                .unwrap();
            sender.try_send(batch).unwrap();
            originals.push(batch);
        }
        let command = EngineCommand::FinishPerformance {
            start_id: LiveId([3; 16]),
            scope,
            finish_id: LiveId([4; 16]),
        };
        let mut pending = Pending {
            id: 9,
            command: command.clone(),
        };
        for original in originals {
            let ControlDispatch::Batch {
                pending: retained,
                batch,
            } = dispatch_control(pending, &receiver, 0)
            else {
                panic!("Finish overtook an admitted parent batch")
            };
            assert_eq!(batch, original);
            assert_eq!(retained.id, 9);
            assert_eq!(retained.command, command);
            pending = retained;
        }
        assert!(matches!(
            dispatch_control(pending, &receiver, 0),
            ControlDispatch::Command(Pending { id: 9, .. })
        ));
        sender.try_send(LiveBatch::empty(scope, 17, None)).unwrap();
        assert!(matches!(
            dispatch_control(Pending { id: 9, command }, &receiver, 9),
            ControlDispatch::Preempted(Pending { id: 9, .. })
        ));
        assert_eq!(
            receiver.try_recv().unwrap().sequence,
            17,
            "Stop fence must win before any Finish drain/dispatch"
        );
    }
    #[test]
    fn only_exact_cumulative_stop_ticket_and_control_fence_can_complete_stop() {
        let (client, _actor) = lanes(Arc::new(AtomicBool::new(false)));
        let shared = &client.shared;
        let ticket = UrgentTicket {
            worker_generation: shared.generation,
            id: 4,
        };
        let mut actual = update(shared, 1);
        assert!(!stop_ack_matches(&actual, ticket, 7));
        actual.urgent.stop = Some(StopAck {
            ticket,
            cancelled_through_control: 7,
        });
        assert!(stop_ack_matches(&actual, ticket, 7));
        assert!(!stop_ack_matches(&actual, ticket, 8));
        assert!(!stop_ack_matches(
            &actual,
            UrgentTicket { id: 5, ..ticket },
            7
        ));
        assert!(!stop_ack_matches(
            &actual,
            UrgentTicket {
                worker_generation: LiveId([99; 16]),
                ..ticket
            },
            7
        ));
    }
    #[test]
    fn rejected_authoritative_update_cannot_publish_async_ready_to_the_gui_mailbox() {
        let (client, _actor) = lanes(Arc::new(AtomicBool::new(false)));
        let shared = &client.shared;
        let root = std::path::Path::new("/synthetic-native");
        let (start, starting, recording) = performance_fixture(shared.generation, root);
        shared.register_start(&start, root).unwrap();
        shared.publish_performance(&starting).unwrap();
        shared
            .publish(
                EngineUpdate {
                    worker_generation: shared.generation,
                    revision: 1,
                    live: None,
                    urgent: UrgentProgress::default(),
                    first_fault: None,
                },
                false,
            )
            .unwrap();
        let mut update = EngineUpdate {
            worker_generation: shared.generation,
            revision: 1,
            live: recording.live,
            urgent: UrgentProgress::default(),
            first_fault: None,
        };
        assert!(
            shared
                .publish_performance_response(&recording, update)
                .is_err()
        );
        assert_eq!(shared.performance_snapshot().unwrap(), starting);
        assert!(shared.latest.lock().unwrap().unwrap().live.is_none());
        update.revision = 2;
        assert!(
            shared
                .publish_performance_response(&recording, update)
                .unwrap()
        );
        assert_eq!(shared.performance_snapshot().unwrap(), recording);
    }
}
