//! Real inherited-private-socket worker lifecycle, without device enumeration or streams.
#![cfg(target_os = "macos")]
use omniusong_core::contracts::{
    Architecture, ContractError, Platform, Validate, WirePayload, WorkerRole,
};
use omniusong_engine::{DeviceInventory, EngineClient, EngineCommand, EngineReply, EngineResult};
use omniusong_ipc::unix::{WorkerFacts, connect_inherited, default_worker_limits};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Deserialize, Serialize)]
struct ProbeRequest {
    request_id: u64,
    #[serde(flatten)]
    rest: std::collections::BTreeMap<String, serde_json::Value>,
}
impl Validate for ProbeRequest {
    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}
impl WirePayload for ProbeRequest {
    const PROTOCOL: &'static str = "omniusong.engine-control.v1";
    const MAX_BYTES: usize = 64 * 1024;
}
#[derive(Deserialize, Serialize)]
struct ProbeResponse {
    reply: EngineReply,
}
impl Validate for ProbeResponse {
    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}
impl WirePayload for ProbeResponse {
    const PROTOCOL: &'static str = "omniusong.engine-reply.v1";
    const MAX_BYTES: usize = 64 * 1024;
}

// This test is only a subprocess entrypoint, selected by the owned wrapper.
#[test]
#[ignore = "Invoked only by the device-free private-process tests"]
fn private_worker_helper() {
    let mode = std::env::var("OMNIUSONG_ENGINE_REVIEW_MODE").unwrap();
    let marker = PathBuf::from(std::env::var_os("OMNIUSONG_ENGINE_REVIEW_PID").unwrap());
    let pending_marker = marker.with_extension("pending");
    std::fs::write(&pending_marker, std::process::id().to_string()).unwrap();
    std::fs::rename(pending_marker, marker).unwrap();
    if mode == "actual" {
        omniusong_engine::run_worker().unwrap();
        return;
    }
    let mut peer = connect_inherited(
        WorkerFacts {
            role: WorkerRole::Engine,
            platform: Platform::Macos,
            architecture: if cfg!(target_arch = "aarch64") {
                Architecture::Aarch64
            } else {
                Architecture::X86_64
            },
            build_id: concat!("omniusong-engine-", env!("CARGO_PKG_VERSION")).into(),
            limits: default_worker_limits(),
            capabilities: vec!["native-audio-control-v1".into()],
        },
        Duration::from_secs(5),
    )
    .unwrap();
    let initial = peer
        .receive::<ProbeRequest>(Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .payload;
    assert_eq!(initial.request_id, 0);
    peer.send_payload(
        ProbeResponse {
            reply: EngineReply {
                request_id: 0,
                result: Ok(EngineResult::Stopped),
            },
        },
        Duration::from_secs(5),
    )
    .unwrap();
    let request = peer
        .receive::<ProbeRequest>(Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .payload;
    if mode == "eof" {
        return;
    }
    let result = if mode == "wrong-kind" {
        EngineResult::Devices(DeviceInventory {
            inputs: vec![],
            outputs: vec![],
        })
    } else {
        EngineResult::Stopped
    };
    let id = if mode == "wrong-id" {
        request.request_id + 1
    } else {
        request.request_id
    };
    peer.send_payload(
        ProbeResponse {
            reply: EngineReply {
                request_id: id,
                result: Ok(result),
            },
        },
        Duration::from_secs(5),
    )
    .unwrap();
    if mode == "complete-then-eof" {
        return;
    }
    // Keep the process present long enough for the parent to reject the response.
    // Parent retirement must close/terminate this inherited channel.
    let _ = peer.receive::<ProbeRequest>(Duration::from_secs(5));
}

struct Fixture {
    root: PathBuf,
    wrapper: PathBuf,
    pid: PathBuf,
    store: PathBuf,
}
fn shell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
impl Fixture {
    fn new(mode: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/engine-process-review")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let wrapper = root.join("private-worker.sh");
        let pid = root.join("worker.pid");
        let store = root.join("store");
        drop(omniusong_store::NativeStore::open(&store).unwrap());
        let current = std::env::current_exe().unwrap();
        let script = format!(
            "#!/bin/sh\nOMNIUSONG_ENGINE_REVIEW_MODE={} OMNIUSONG_ENGINE_REVIEW_PID={} exec {} --ignored --exact private_worker_helper --nocapture\n",
            shell_literal(mode),
            shell_literal(pid.to_str().unwrap()),
            shell_literal(current.to_str().unwrap())
        );
        std::fs::write(&wrapper, script).unwrap();
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
fn reply(client: &mut EngineClient) -> EngineReply {
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(reply) = client.try_poll() {
            return reply;
        }
        assert!(
            Instant::now() < end,
            "Private engine test did not receive a bounded terminal response"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn assert_child_retires(marker: &Path) {
    let pid = std::fs::read_to_string(marker).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while std::process::Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
    {
        assert!(
            Instant::now() < end,
            "Private audio child remains alive after client retirement"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn real_worker_status_stop_and_client_drop_never_need_a_device() {
    let fixture = Fixture::new("actual");
    let mut client = fixture.client();
    client.try_submit(EngineCommand::Status).unwrap();
    assert!(client.try_submit(EngineCommand::Stop).is_err());
    let first = reply(&mut client);
    assert_eq!(first.request_id, 1);
    match first.result.unwrap() {
        EngineResult::Status(s) => {
            assert_eq!(s.state, "idle");
            assert!(s.capture.is_none());
            assert_eq!(s.frames, 0);
        }
        other => panic!("Unexpected response: {other:?}"),
    }
    client.try_submit(EngineCommand::Stop).unwrap();
    let second = reply(&mut client);
    assert_eq!(second.request_id, 2);
    assert_eq!(second.result.unwrap(), EngineResult::Stopped);
    drop(client);
    assert_child_retires(&fixture.pid);
}
#[test]
fn authenticated_peer_eof_wrong_id_and_wrong_kind_retire_without_replay() {
    for mode in ["eof", "wrong-id", "wrong-kind"] {
        let fixture = Fixture::new(mode);
        let mut client = fixture.client();
        client.try_submit(EngineCommand::Stop).unwrap();
        let failure = reply(&mut client);
        assert_eq!(failure.request_id, 1);
        assert!(failure.result.is_err(), "{mode}");
        assert!(client.try_submit(EngineCommand::Stop).is_err(), "{mode}");
        assert!(
            client.try_poll().is_none(),
            "Terminal reply must occur only once"
        );
        drop(client);
        assert_child_retires(&fixture.pid);
    }
}

#[test]
fn queued_normal_completion_cannot_hide_later_terminal_child_failure() {
    let fixture = Fixture::new("complete-then-eof");
    let mut client = fixture.client();
    client.try_submit(EngineCommand::Stop).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.pid.exists() {
        assert!(Instant::now() < deadline, "Private helper did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Leave the one-slot reply queue occupied while the next heartbeat observes
    // EOF. Terminal delivery must survive behind the successful completion.
    assert_child_retires(&fixture.pid);
    let completed = reply(&mut client);
    assert_eq!(completed.request_id, 1);
    assert_eq!(completed.result.unwrap(), EngineResult::Stopped);
    let terminal = reply(&mut client);
    assert_eq!(terminal.request_id, 0);
    assert!(terminal.result.is_err());
    assert!(client.try_submit(EngineCommand::Stop).is_err());
    assert!(client.try_poll().is_none());
}
