//! Independent private-process supervisor tests. The helper uses CPU-only live
//! rendering; its protocol acknowledgments do not qualify native stream teardown.
#![cfg(target_os = "macos")]
use omniusong_core::contracts::{
    Architecture, ContractError, Platform, Validate, WirePayload, WorkerRole,
};
use omniusong_engine::*;
use omniusong_ipc::unix::{WorkerFacts, connect_inherited, default_worker_limits};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Deserialize, Serialize)]
struct Request {
    request_id: u64,
    action: serde_json::Value,
}
impl Validate for Request {
    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}
impl WirePayload for Request {
    const PROTOCOL: &'static str = "omniusong.engine-control.v1";
    const MAX_BYTES: usize = 64 * 1024;
}
#[derive(Deserialize, Serialize)]
struct Response {
    reply: EngineReply,
    update: Option<EngineUpdate>,
}
impl Validate for Response {
    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}
impl WirePayload for Response {
    const PROTOCOL: &'static str = "omniusong.engine-reply.v1";
    const MAX_BYTES: usize = 64 * 1024;
}
const REJECTION: &str = "Independent live peer rejected the exact admitted batch";
fn wait_until(mut condition: impl FnMut() -> bool, explanation: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "{explanation}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn ready(scope: LiveScope, gui_clock: LiveClock, progress: LiveProgress) -> LiveReady {
    LiveReady {
        scope,
        gui_clock,
        sources: [
            LiveSourceKind::ComputerKeys,
            LiveSourceKind::PointerKeys,
            LiveSourceKind::Pads,
            LiveSourceKind::AccessiblePulse,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| LiveSourceBinding {
            kind,
            handle: (i + 1) as u8,
            connection_generation: 1,
        })
        .collect(),
        output: DeviceInfo {
            id: "independent-synthetic-output".into(),
            name: "CPU-only private protocol fixture".into(),
        },
        sample_rate: 8000,
        channels: 1,
        progress,
    }
}
fn idle() -> EngineResult {
    EngineResult::Status(EngineStatus {
        state: "idle".into(),
        capture: None,
        sample_rate: 0,
        channels: 0,
        frames: 0,
        peak: 0.0,
        xruns: 0,
    })
}

#[test]
#[ignore = "Only the owned private-process wrapper invokes this device-free helper"]
fn live_private_helper() {
    let mode = std::env::var("OMNIUSONG_LIVE_REVIEW_MODE").unwrap();
    let marker = PathBuf::from(std::env::var_os("OMNIUSONG_LIVE_REVIEW_PID").unwrap());
    let pending = marker.with_extension("pending");
    std::fs::write(&pending, std::process::id().to_string()).unwrap();
    std::fs::rename(pending, &marker).unwrap();
    let mut peer = connect_inherited(
        WorkerFacts {
            role: WorkerRole::Engine,
            platform: Platform::Macos,
            architecture: if cfg!(target_arch = "aarch64") {
                Architecture::Aarch64
            } else {
                Architecture::X86_64
            },
            build_id: ENGINE_BUILD_ID.into(),
            limits: default_worker_limits(),
            capabilities: vec![LIVE_PROTOCOL.into()],
        },
        Duration::from_secs(5),
    )
    .unwrap();
    let initial = peer
        .receive::<Request>(Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .payload;
    assert_eq!(initial.request_id, 0);
    assert_eq!(initial.action["action"], "initialize");
    assert_eq!(initial.action["profile"], LIVE_PROTOCOL);
    let generation: LiveId =
        serde_json::from_value(initial.action["worker_generation"].clone()).unwrap();
    let mut update = EngineUpdate {
        worker_generation: generation,
        revision: 1,
        live: None,
        urgent: UrgentProgress::default(),
        first_fault: None,
    };
    peer.send_payload(
        Response {
            reply: EngineReply {
                request_id: 0,
                result: Ok(EngineResult::Initialized {
                    profile: LIVE_PROTOCOL.into(),
                    worker_generation: generation,
                }),
            },
            update: Some(update),
        },
        Duration::from_secs(5),
    )
    .unwrap();
    let mut live: Option<(
        live::LiveAdmission,
        live::LiveRenderer,
        live::LiveErrorGuard,
    )> = None;
    let mut rejected = false;
    let mut adversarial_ready = false;
    while let Ok(Some(request)) = peer.receive::<Request>(Duration::from_secs(8)) {
        let request = request.payload;
        if rejected {
            std::fs::write(
                marker.with_extension("unexpected-request"),
                b"request after one-shot rejection",
            )
            .unwrap();
        }
        let mut result = Ok(idle());
        match request.action["action"].as_str().unwrap() {
            "command" => {
                let command: EngineCommand =
                    serde_json::from_value(request.action["command"].clone()).unwrap();
                match command {
                    EngineCommand::StartLive { gui_clock, .. } => {
                        std::fs::write(marker.with_extension("start"), b"start received").unwrap();
                        if mode == "start-fence" {
                            wait_until(
                                || marker.with_extension("continue").exists(),
                                "Parent did not release in-flight Start fixture",
                            );
                        }
                        let scope = LiveScope {
                            worker_generation: generation,
                            session_id: LiveId([11; 16]),
                            panic_generation: 1,
                        };
                        let (admission, renderer) = live::live_pair(scope, 8000, 1).unwrap();
                        let guard = live::LiveErrorGuard::new(admission.control());
                        update.live = Some(admission.progress());
                        result = Ok(EngineResult::LiveStarted(ready(
                            scope,
                            gui_clock,
                            admission.progress(),
                        )));
                        if mode == "fault-before-ready" {
                            admission.control().fault(LiveFaultKind::Device, None);
                            update.live = Some(admission.progress());
                            update.first_fault = admission.progress().fault;
                        }
                        live = Some((admission, renderer, guard));
                    }
                    EngineCommand::Status => {
                        std::fs::write(marker.with_extension("status"), b"status received")
                            .unwrap();
                        if update.urgent.stop.is_some() {
                            std::fs::write(
                                marker.with_extension("after-stop"),
                                b"actor received stop acknowledgment",
                            )
                            .unwrap();
                        }
                        if let Some((admission, _, _)) = &live {
                            result = Ok(EngineResult::LiveStatus(admission.progress()));
                        }
                        if adversarial_ready && mode == "regress" {
                            let mut invalid = update.live.unwrap();
                            invalid.frames = 0;
                            invalid.worker_accepted_sequence = None;
                            invalid.callback_applied_sequence = None;
                            update.live = Some(invalid);
                            result = Ok(EngineResult::LiveStatus(invalid));
                        }
                        if adversarial_ready && mode == "change-fault" {
                            let mut changed = update.first_fault.unwrap();
                            changed.kind = LiveFaultKind::Clock;
                            update.first_fault = Some(changed);
                        }
                    }
                    EngineCommand::Stop => {
                        if let Some((admission, renderer, guard)) = live.take() {
                            drop(renderer);
                            drop(guard);
                            assert!(
                                admission.control().render_destroyed()
                                    && admission.control().error_destroyed()
                            );
                            let mut stopped = admission.progress();
                            stopped.state = LiveState::Stopped;
                            stopped.active_voices = 0;
                            stopped.release_tail_voices = 0;
                            stopped.lease_credit = None;
                            update.live = Some(stopped);
                        }
                        result = Ok(EngineResult::Stopped);
                    }
                    _ => panic!("Device or catalog command is forbidden in live fixture"),
                }
            }
            "live_batch" => {
                if mode == "batch-error" {
                    rejected = true;
                    result = Err(REJECTION.into());
                } else {
                    let batch: LiveBatch =
                        serde_json::from_value(request.action["batch"].clone()).unwrap();
                    let (admission, renderer, _) = live.as_mut().unwrap();
                    admission.admit(batch).unwrap();
                    renderer.render_into(&mut [0.0; 1]);
                    if mode == "change-fault" {
                        admission.control().fault(LiveFaultKind::Device, None);
                        update.first_fault = admission.progress().fault;
                    }
                    adversarial_ready = true;
                    update.live = Some(admission.progress());
                    result = Ok(EngineResult::LiveStatus(admission.progress()));
                }
            }
            "panic_live" => {
                let ticket: UrgentTicket =
                    serde_json::from_value(request.action["ticket"].clone()).unwrap();
                let scope: LiveScope =
                    serde_json::from_value(request.action["scope"].clone()).unwrap();
                let (admission, renderer, _) = live.as_mut().unwrap();
                admission.begin_panic(scope).unwrap();
                // Invoke the real CPU renderer, not a fabricated callback-clear flag.
                for _ in 0..4 {
                    renderer.render_into(&mut [0.0; 1]);
                }
                let progress = admission.finish_panic().unwrap().unwrap();
                update.live = Some(progress);
                update.urgent.panic = Some(PanicAck {
                    ticket,
                    requested_scope: scope,
                    outcome: PanicOutcome::Applied {
                        new_scope: progress.scope,
                    },
                });
                result = Ok(EngineResult::LiveStatus(progress));
                std::fs::write(
                    marker.with_extension("panic"),
                    b"CPU callback applied panic",
                )
                .unwrap();
            }
            "urgent_stop" => {
                let ticket: UrgentTicket =
                    serde_json::from_value(request.action["ticket"].clone()).unwrap();
                let cancelled_through_control = request.action["cancelled_through_control"]
                    .as_u64()
                    .unwrap();
                if let Some((admission, renderer, guard)) = live.take() {
                    drop(renderer);
                    drop(guard);
                    assert!(
                        admission.control().render_destroyed()
                            && admission.control().error_destroyed()
                    );
                    let mut stopped = admission.progress();
                    stopped.state = LiveState::Stopped;
                    stopped.active_voices = 0;
                    stopped.release_tail_voices = 0;
                    stopped.lease_credit = None;
                    update.live = Some(stopped);
                }
                update.urgent.stop = Some(StopAck {
                    ticket,
                    cancelled_through_control,
                });
                if let Some(superseded) = request
                    .action
                    .get("superseded_panic")
                    .filter(|v| !v.is_null())
                {
                    let (panic_ticket, scope): (UrgentTicket, LiveScope) =
                        serde_json::from_value(superseded.clone()).unwrap();
                    update.urgent.panic = Some(PanicAck {
                        ticket: panic_ticket,
                        requested_scope: scope,
                        outcome: PanicOutcome::SupersededByStop {
                            stop_ticket: ticket,
                        },
                    });
                }
                result = Ok(EngineResult::Stopped);
                std::fs::write(marker.with_extension("stop"), b"CPU owners dropped").unwrap();
            }
            other => panic!("Unexpected live fixture action {other}"),
        }
        update.revision += 1;
        assert!(update.is_valid());
        peer.send_payload(
            Response {
                reply: EngineReply {
                    request_id: request.request_id,
                    result,
                },
                update: Some(update),
            },
            Duration::from_secs(5),
        )
        .unwrap();
    }
    // Parent EOF exits the owned helper and drops its CPU callback owners.
}

struct Fixture {
    root: PathBuf,
    wrapper: PathBuf,
    pid: PathBuf,
    store: PathBuf,
}
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
impl Fixture {
    fn new(mode: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/live-process-review")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let wrapper = root.join("worker.sh");
        let pid = root.join("worker.pid");
        let store = root.join("store");
        drop(omniusong_store::NativeStore::open(&store).unwrap());
        let exe = std::env::current_exe().unwrap();
        std::fs::write(&wrapper,format!("#!/bin/sh\nOMNIUSONG_LIVE_REVIEW_MODE={} OMNIUSONG_LIVE_REVIEW_PID={} exec {} --ignored --exact live_private_helper --nocapture\n",literal(mode),literal(pid.to_str().unwrap()),literal(exe.to_str().unwrap()))).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            wrapper,
            pid,
            store,
        }
    }
    fn client(&self) -> EngineClient {
        EngineClient::spawn(self.wrapper.clone(), self.store.clone()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
fn start() -> EngineCommand {
    EngineCommand::StartLive {
        output_device: None,
        gui_clock: LiveClock {
            domain: "independent-gui".into(),
            epoch: "fixture".into(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 1,
        },
    }
}
fn update_where(
    client: &mut EngineClient,
    mut accept: impl FnMut(&EngineUpdate) -> bool,
) -> EngineUpdate {
    let mut found = None;
    wait_until(
        || {
            if let Some(value) = client.try_poll_update()
                && accept(&value)
            {
                found = Some(value);
            }
            found.is_some()
        },
        "Expected bounded cumulative live update",
    );
    found.unwrap()
}
fn reply(client: &mut EngineClient) -> EngineReply {
    let mut found = None;
    wait_until(
        || {
            found = client.try_poll();
            found.is_some()
        },
        "Expected bounded ordinary/terminal reply",
    );
    found.unwrap()
}
fn retired(path: &Path) {
    wait_until(|| path.exists(), "Private helper PID missing");
    let pid = std::fs::read_to_string(path).unwrap();
    wait_until(
        || {
            !std::process::Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        },
        "Private helper did not retire",
    );
}

#[test]
fn queued_normal_completion_cannot_block_panic_and_old_scope_stays_inert() {
    let fixture = Fixture::new("healthy");
    let mut client = fixture.client();
    client.try_submit(start()).unwrap();
    let started = update_where(&mut client, |u| {
        u.live.is_some_and(|p| p.state == LiveState::Running)
    });
    let scope = started.live.unwrap().scope;
    assert!(matches!(
        reply(&mut client).result.unwrap(),
        EngineResult::LiveStarted(_)
    ));
    client.try_submit(EngineCommand::Status).unwrap();
    wait_until(
        || fixture.pid.with_extension("status").exists(),
        "Status did not reach helper",
    );
    // Leave an ordinary Status completion unpolled while requesting urgent panic.
    let ticket = client.request_panic_live(scope).unwrap();
    assert_eq!(client.request_panic_live(scope).unwrap(), ticket);
    let completed = update_where(&mut client, |u| {
        u.urgent.panic.is_some_and(|ack| ack.ticket == ticket)
    });
    assert!(fixture.pid.with_extension("panic").exists());
    let new_scope = match completed.urgent.panic.unwrap().outcome {
        PanicOutcome::Applied { new_scope } => new_scope,
        _ => panic!("Panic was not applied"),
    };
    assert_eq!(new_scope.panic_generation, scope.panic_generation + 1);
    assert_eq!(
        client.try_push_live(LiveBatch::empty(scope, 1, None)),
        Err(LiveSubmitError::StaleScope)
    );
    assert!(matches!(
        reply(&mut client).result.unwrap(),
        EngineResult::LiveStatus(_)
    ));
    client
        .try_push_live(LiveBatch::empty(new_scope, 1, None))
        .unwrap();
    update_where(&mut client, |u| {
        u.live
            .is_some_and(|p| p.worker_accepted_sequence == Some(1))
    });
    drop(client);
    retired(&fixture.pid);
}

#[test]
fn stop_fences_an_inflight_start_and_pending_stop_rejects_panic_ticket() {
    let fixture = Fixture::new("start-fence");
    let mut client = fixture.client();
    client.try_submit(start()).unwrap();
    wait_until(
        || fixture.pid.with_extension("start").exists(),
        "Start did not reach private peer",
    );
    let ticket = client.request_stop().unwrap();
    assert_eq!(client.request_stop().unwrap(), ticket);
    let guessed_scope = LiveScope {
        worker_generation: ticket.worker_generation,
        session_id: LiveId([11; 16]),
        panic_generation: 1,
    };
    assert!(client.request_panic_live(guessed_scope).is_err());
    std::fs::write(
        fixture.pid.with_extension("continue"),
        b"finish start exchange",
    )
    .unwrap();
    let stopped = update_where(&mut client, |u| {
        u.urgent.stop.is_some_and(|ack| ack.ticket == ticket)
    });
    assert!(fixture.pid.with_extension("stop").exists());
    assert!(stopped.urgent.stop.unwrap().cancelled_through_control >= 1);
    assert!(
        client
            .try_push_live(LiveBatch::empty(guessed_scope, 1, None))
            .is_err()
    );
    // A delayed ordinary completion is not permission to reopen live admission.
    let _ = reply(&mut client);
    assert!(
        client
            .try_push_live(LiveBatch::empty(guessed_scope, 1, None))
            .is_err()
    );
    drop(client);
    retired(&fixture.pid);
}

#[test]
fn internal_live_rejection_is_retained_exactly_once_behind_queued_normal_completion() {
    let fixture = Fixture::new("batch-error");
    let mut client = fixture.client();
    client.try_submit(start()).unwrap();
    let started = update_where(&mut client, |u| {
        u.live.is_some_and(|p| p.state == LiveState::Running)
    });
    let scope = started.live.unwrap().scope;
    assert!(matches!(
        reply(&mut client).result.unwrap(),
        EngineResult::LiveStarted(_)
    ));
    client.try_submit(EngineCommand::Status).unwrap();
    wait_until(
        || fixture.pid.with_extension("status").exists(),
        "Status did not reach helper",
    );
    client
        .try_push_live(LiveBatch::empty(scope, 1, None))
        .unwrap();
    retired(&fixture.pid);
    wait_until(
        || client.is_retired(),
        "Client did not publish retired state after child exit",
    );
    assert!(matches!(
        reply(&mut client).result.unwrap(),
        EngineResult::LiveStatus(_)
    ));
    let terminal = reply(&mut client);
    assert_eq!(terminal.request_id, 0);
    assert_eq!(terminal.result.unwrap_err(), REJECTION);
    assert!(client.try_poll().is_none());
    assert!(
        client
            .try_push_live(LiveBatch::empty(scope, 2, None))
            .is_err()
    );
    assert!(client.request_stop().is_err());
    assert!(!fixture.pid.with_extension("unexpected-request").exists());
}

#[test]
fn completed_stop_keeps_parent_note_gate_closed_without_ui_update_polling() {
    let fixture = Fixture::new("healthy");
    let mut client = fixture.client();
    client.try_submit(start()).unwrap();
    let ready = match reply(&mut client).result.unwrap() {
        EngineResult::LiveStarted(ready) => ready,
        other => panic!("Unexpected start {other:?}"),
    };
    client.request_stop().unwrap();
    // This marker is emitted only after the peer receives a later heartbeat,
    // proving the actor consumed Stop without using the UI update consumer.
    wait_until(
        || fixture.pid.with_extension("after-stop").exists(),
        "Actor did not complete Stop and resume heartbeat",
    );
    assert!(!client.is_retired());
    assert!(
        client
            .try_push_live(LiveBatch::empty(ready.scope, 1, None))
            .is_err()
    );
    assert!(client.request_panic_live(ready.scope).is_err());
    assert!(
        !client.is_retired(),
        "Known stopped admission must not retire a healthy worker"
    );
    drop(client);
    retired(&fixture.pid);
}

#[test]
fn ordinary_stop_reply_closes_note_gate_without_requiring_update_poll() {
    let fixture = Fixture::new("healthy");
    let mut client = fixture.client();
    client.try_submit(start()).unwrap();
    let ready = match reply(&mut client).result.unwrap() {
        EngineResult::LiveStarted(ready) => ready,
        other => panic!("Unexpected start {other:?}"),
    };
    client.try_submit(EngineCommand::Stop).unwrap();
    assert_eq!(reply(&mut client).result.unwrap(), EngineResult::Stopped);
    assert!(
        client
            .try_push_live(LiveBatch::empty(ready.scope, 1, None))
            .is_err()
    );
    assert!(client.request_panic_live(ready.scope).is_err());
    assert!(!client.is_retired());
    drop(client);
    retired(&fixture.pid);
}

#[test]
fn authenticated_progress_regression_and_changed_first_fault_retire_the_peer() {
    for (mode, cause) in [
        ("regress", "Live progress regressed"),
        ("change-fault", "Live first fault changed or disappeared"),
    ] {
        let fixture = Fixture::new(mode);
        let mut client = fixture.client();
        client.try_submit(start()).unwrap();
        let ready = match reply(&mut client).result.unwrap() {
            EngineResult::LiveStarted(ready) => ready,
            other => panic!("Unexpected start {other:?}"),
        };
        client
            .try_push_live(LiveBatch::empty(ready.scope, 1, None))
            .unwrap();
        let terminal = reply(&mut client);
        assert_eq!(terminal.request_id, 0);
        assert_eq!(terminal.result.unwrap_err(), cause);
        assert!(client.is_retired());
        assert!(client.try_poll().is_none());
        retired(&fixture.pid);
    }
}

#[test]
fn fault_between_start_ready_and_authoritative_update_cannot_reopen_parent_admission() {
    let fixture = Fixture::new("fault-before-ready");
    let mut client = fixture.client();
    client.try_submit(start()).unwrap();
    let ready = match reply(&mut client).result.unwrap() {
        EngineResult::LiveStarted(ready) => ready,
        other => panic!("Unexpected start {other:?}"),
    };
    assert_eq!(ready.progress.state, LiveState::Running);
    // No update polling: the supervisor itself must retain the newer fault.
    assert_eq!(
        client.try_push_live(LiveBatch::empty(ready.scope, 1, None)),
        Err(LiveSubmitError::Faulted)
    );
    assert_eq!(
        client.request_panic_live(ready.scope),
        Err(LiveSubmitError::Faulted)
    );
    let faulted = update_where(&mut client, |u| u.first_fault.is_some());
    assert_eq!(faulted.first_fault.unwrap().kind, LiveFaultKind::Device);
    assert!(
        !client.is_retired(),
        "A known live fault permits explicit Stop"
    );
    drop(client);
    retired(&fixture.pid);
}
