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
pub(crate) struct Shared {
    pub generation: LiveId,
    pub stop_requested: AtomicU64,
    pub stop_completed: AtomicU64,
    pub stop_barrier: AtomicU64,
    pub faulted: AtomicBool,
    pub live_active: AtomicBool,
    pub latest: Mutex<Option<EngineUpdate>>,
    pub wake: SyncSender<()>,
}
impl Shared {
    pub fn notify(&self) {
        let _ = self.wake.try_send(());
    }
    pub fn stop_pending(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire) > self.stop_completed.load(Ordering::Acquire)
    }
    fn publish(&self, update: EngineUpdate, allow_new_session: bool) -> Result<(), String> {
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
                    if matches!(previous.state, LiveState::Faulted | LiveState::Stopped)
                        && next.state == LiveState::Running
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
            | EngineCommand::StartPlayback { .. }
            | EngineCommand::StartCapture { .. }
    )
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
    loop {
        if let Some((error, deadline, _)) = &deferred_rejection
            && Instant::now() >= *deadline
        {
            return Err(error.clone());
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
        let stop_id = shared.stop_requested.load(Ordering::Acquire);
        let mut normal = None;
        let action = if stop_id > shared.stop_completed.load(Ordering::Acquire) {
            let superseded = lanes.panic.try_recv().ok().map(|p| (p.ticket, p.scope));
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
            if starts(&pending.command) && pending.id <= shared.stop_barrier.load(Ordering::Acquire)
            {
                outbox = Some(Event::Completed(EngineReply {
                    request_id: pending.id,
                    result: Err("Audio start cancelled by an acknowledged Stop fence".into()),
                }));
                continue;
            }
            let action = Action::Command {
                command: pending.command.clone(),
            };
            normal = Some(pending);
            Some(action)
        } else if let Ok(batch) = lanes.live.try_recv() {
            Some(Action::LiveBatch { batch })
        } else if last_status.elapsed() >= Duration::from_millis(250) {
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
        let starts_live = matches!(&response.reply.result, Ok(EngineResult::LiveStarted(_)));
        if let Some(update) = response.update {
            shared.publish(update, starts_live)?;
            if deferred_rejection.as_ref().is_some_and(|(_, _, scope)| {
                update.live.is_some_and(|p| {
                    p.scope.session_id == scope.session_id
                        && p.state == LiveState::Faulted
                        && p.fault.is_some()
                })
            }) {
                deferred_rejection = None;
            }
        } else if internal_live || urgent_stop || starts_live {
            return Err("Missing live authoritative update".into());
        }
        if let Some((ticket, scope)) = expected_panic
            && response.reply.result.is_ok()
            && !response.update.is_some_and(|u| {
                u.urgent.panic.is_some_and(|ack| {
                    ack.ticket == ticket
                        && ack.requested_scope == scope
                        && matches!(ack.outcome, PanicOutcome::Applied { .. })
                })
            })
        {
            return Err("Panic callback acknowledgment mismatch".into());
        }
        if urgent_stop {
            if !matches!(response.reply.result, Ok(EngineResult::Stopped)) {
                return Err(response
                    .reply
                    .result
                    .err()
                    .unwrap_or("Unexpected urgent Stop result".into()));
            }
            let latest = shared
                .latest
                .lock()
                .map_err(|_| "Live update mailbox poisoned")?;
            if !latest
                .as_ref()
                .and_then(|u| u.urgent.stop)
                .is_some_and(|ack| {
                    ack.ticket.id == stop_id
                        && ack.cancelled_through_control
                            == shared.stop_barrier.load(Ordering::Acquire)
                })
            {
                return Err("Urgent Stop acknowledgment mismatch".into());
            }
            shared.live_active.store(false, Ordering::Release);
            shared.stop_completed.store(stop_id, Ordering::Release);
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
}
