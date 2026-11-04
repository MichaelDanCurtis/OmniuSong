//! Private-process protocol regression; no native device or normal seal.
//! The helper obtains Finish from the real CPU renderer, then supplies synthetic
//! playback status. Native worker behavior still requires separate qualification.
#![cfg(target_os = "macos")]
use omniusong_core::contracts::{
    Architecture, ContractError, Platform, Validate, WirePayload, WorkerRole,
};
use omniusong_engine::*;
use omniusong_ipc::unix::{WorkerFacts, connect_inherited, default_worker_limits};
use serde::{Deserialize, Serialize};
use std::{
    fs,
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
    const PROTOCOL: &'static str = "omniusong.engine-control.v2";
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
    const PROTOCOL: &'static str = "omniusong.engine-reply.v2";
    const MAX_BYTES: usize = 64 * 1024;
}
fn status(state: &str) -> EngineStatus {
    EngineStatus {
        state: state.into(),
        capture: None,
        sample_rate: 48_000,
        channels: 2,
        frames: 256,
        peak: 0.0,
        xruns: 0,
    }
}

#[test]
#[ignore = "Invoked only as an owned private IPC helper; no devices"]
fn performance_playback_private_helper() {
    let pid = PathBuf::from(std::env::var_os("OMNIUSONG_PERFORMANCE_LIFECYCLE_PID").unwrap());
    let omit = std::env::var("OMNIUSONG_PERFORMANCE_LIFECYCLE_OMIT").unwrap() == "1";
    fs::write(&pid, std::process::id().to_string()).unwrap();
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
    assert_eq!(initial.action["action"], "initialize");
    assert_eq!(initial.action["profile"], LIVE_PROTOCOL);
    let generation: LiveId =
        serde_json::from_value(initial.action["worker_generation"].clone()).unwrap();
    let root: PathBuf = serde_json::from_value(initial.action["store_root"].clone()).unwrap();
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
    let mut live = None;
    let mut performance: Option<PerformanceStatus> = None;
    let mut playing = false;
    while let Ok(Some(request)) = peer.receive::<Request>(Duration::from_secs(8)) {
        let request = request.payload;
        let result = match request.action["action"].as_str().unwrap() {
            "urgent_stop" => {
                let ticket: UrgentTicket =
                    serde_json::from_value(request.action["ticket"].clone()).unwrap();
                let through = request.action["cancelled_through_control"]
                    .as_u64()
                    .unwrap();
                update.urgent.stop = Some(StopAck {
                    ticket,
                    cancelled_through_control: through,
                });
                Ok(EngineResult::Stopped)
            }
            "command" => {
                let command: EngineCommand =
                    serde_json::from_value(request.action["command"].clone()).unwrap();
                match command {
                    EngineCommand::StartPerformance { request: start } => {
                        let scope = LiveScope {
                            worker_generation: generation,
                            session_id: LiveId([22; 16]),
                            panic_generation: 1,
                        };
                        let (admission, renderer, owner) =
                            live::recording_live_pair(scope, 48_000, 2).unwrap();
                        let progress = admission.progress();
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
                            .map(|(i, kind)| LiveSourceBinding {
                                kind,
                                handle: i as u8 + 1,
                                connection_generation: 1,
                            })
                            .collect(),
                            output: DeviceInfo {
                                id: "synthetic-output".into(),
                                name: "CPU protocol fixture; no device".into(),
                            },
                            sample_rate: 48_000,
                            channels: 2,
                            progress,
                        };
                        let value = PerformanceStatus {
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
                                descriptor_sha256: Some("a".repeat(64)),
                            },
                            phase: PerformancePhase::Recording,
                            ready: Some(ready),
                            native_sample_format: Some("f32".into()),
                            live: Some(progress),
                            writer: None,
                            finish: None,
                            cut: None,
                            native: NativeRetirementStatus::default(),
                            sealed: None,
                            error: None,
                        };
                        assert!(value.is_valid());
                        update.live = Some(progress);
                        performance = Some(value.clone());
                        live = Some((admission, renderer, owner));
                        Ok(EngineResult::Performance(Box::new(value)))
                    }
                    EngineCommand::FinishPerformance {
                        scope, finish_id, ..
                    } => {
                        let (admission, renderer, owner) = live.as_mut().unwrap();
                        let fence = admission.begin_finish(scope, finish_id).unwrap();
                        renderer.render_into(&mut [0.0; 512]);
                        while owner.try_pop().unwrap().is_some() {}
                        let cut = admission.finish_ack().unwrap().unwrap();
                        let value = performance.as_mut().unwrap();
                        value.revision += 1;
                        value.phase = PerformancePhase::Interrupted;
                        value.finish = Some(fence);
                        value.cut = Some(cut);
                        value.live = Some(admission.progress());
                        value.error = Some(
                            "CPU-only fixture has no native lifetime or normal seal authority"
                                .into(),
                        );
                        assert!(value.is_valid());
                        update.live = value.live;
                        Ok(EngineResult::Performance(Box::new(value.clone())))
                    }
                    EngineCommand::StartPlayback { .. } => {
                        playing = true;
                        if omit {
                            update.live = None;
                        }
                        Ok(EngineResult::Playing(status("playing")))
                    }
                    EngineCommand::Status if playing => {
                        Ok(EngineResult::Status(status("playback_finished")))
                    }
                    EngineCommand::Status => Ok(performance.as_ref().map_or_else(
                        || EngineResult::Status(status("idle")),
                        |p| EngineResult::Performance(Box::new(p.clone())),
                    )),
                    EngineCommand::Stop => Ok(EngineResult::Stopped),
                    other => panic!("Unexpected fixture command: {other:?}"),
                }
            }
            other => panic!("Unexpected fixture action {other}"),
        };
        update.revision += 1;
        if peer
            .send_payload(
                Response {
                    reply: EngineReply {
                        request_id: request.request_id,
                        result,
                    },
                    update: Some(update),
                },
                Duration::from_secs(5),
            )
            .is_err()
        {
            break;
        }
    }
    if let Some((admission, renderer, owner)) = live.take() {
        drop(renderer);
        drop(owner);
        drop(admission);
    }
}

struct Fixture {
    root: PathBuf,
    wrapper: PathBuf,
    pid: PathBuf,
    store: PathBuf,
}
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}
impl Fixture {
    fn new(omit: bool) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-playback-process-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let wrapper = root.join("worker.sh");
        let pid = root.join("worker.pid");
        let store = root.join("store");
        drop(omniusong_store::NativeStore::open(&store).unwrap());
        fs::write(&wrapper,format!("#!/bin/sh\nOMNIUSONG_PERFORMANCE_LIFECYCLE_OMIT={} OMNIUSONG_PERFORMANCE_LIFECYCLE_PID={} exec {} --ignored --exact performance_playback_private_helper --nocapture\n",if omit {"1"}else{"0"},literal(pid.to_str().unwrap()),literal(std::env::current_exe().unwrap().to_str().unwrap()))).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
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
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn until<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "Private protocol fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn reply(client: &mut EngineClient) -> EngineReply {
    until(|| client.try_poll())
}
fn retire(pid: &Path) {
    until(|| pid.exists().then_some(()));
    let pid = fs::read_to_string(pid).unwrap();
    until(|| {
        (!std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success())
        .then_some(())
    });
}
fn finished(client: &mut EngineClient) -> LiveProgress {
    let start = PerformanceStart {
        id: LiveId::random(),
        output_device: None,
        gui_clock: LiveClock {
            domain: "independent-gui".into(),
            epoch: "fixture".into(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 1,
        },
    };
    client
        .try_submit(EngineCommand::StartPerformance {
            request: start.clone(),
        })
        .unwrap();
    assert!(matches!(
        reply(client).result.unwrap(),
        EngineResult::Performance(_)
    ));
    let recording = until(|| {
        client
            .try_performance_update()
            .filter(|s| s.phase == PerformancePhase::Recording)
    });
    client
        .try_submit(EngineCommand::FinishPerformance {
            start_id: start.id,
            scope: recording.live.unwrap().scope,
            finish_id: LiveId::random(),
        })
        .unwrap();
    let result = reply(client).result.unwrap();
    let EngineResult::Performance(result) = result else {
        panic!("Missing CPU Finish result")
    };
    let terminal = result.live.unwrap();
    assert_eq!(terminal.state, LiveState::FinishedCut);
    assert_eq!(terminal.frames, 256);
    let ticket = client.request_stop().unwrap();
    until(|| {
        client
            .try_poll_update()
            .filter(|u| u.urgent.stop.is_some_and(|ack| ack.ticket == ticket))
    });
    terminal
}

#[test]
fn retained_terminal_performance_scope_allows_playback_status_and_stop() {
    let f = Fixture::new(false);
    let mut client = f.client();
    let terminal = finished(&mut client);
    client
        .try_submit(EngineCommand::StartPlayback {
            project_id: "synthetic-project".into(),
            output_device: None,
        })
        .unwrap();
    assert!(matches!(
        reply(&mut client).result.unwrap(),
        EngineResult::Playing(_)
    ));
    assert_eq!(until(|| client.try_poll_update()).live, Some(terminal));
    client.try_submit(EngineCommand::Status).unwrap();
    assert!(
        matches!(reply(&mut client).result.unwrap(),EngineResult::Status(s) if s.state=="playback_finished")
    );
    client.try_submit(EngineCommand::Stop).unwrap();
    assert!(matches!(
        reply(&mut client).result.unwrap(),
        EngineResult::Stopped
    ));
    assert_eq!(until(|| client.try_poll_update()).live, Some(terminal));
    assert!(!client.is_retired());
    drop(client);
    retire(&f.pid);
}

#[test]
fn disappearing_performance_scope_still_retires_the_peer_with_exact_cause_once() {
    let f = Fixture::new(true);
    let mut client = f.client();
    let _ = finished(&mut client);
    client
        .try_submit(EngineCommand::StartPlayback {
            project_id: "synthetic-project".into(),
            output_device: None,
        })
        .unwrap();
    assert_eq!(
        reply(&mut client).result.unwrap_err(),
        "Ended live scope summary disappeared"
    );
    assert!(client.is_retired());
    assert!(client.try_submit(EngineCommand::Status).is_err());
    retire(&f.pid);
    assert!(client.try_poll().is_none());
}
