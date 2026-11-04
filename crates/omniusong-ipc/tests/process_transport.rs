#![cfg(unix)]

use omniusong_core::contracts::{
    Architecture, AudioAction, AudioCommand, AudioEvent, AudioNotice, Envelope, Extensions,
    MessageId, PROTOCOL_VERSION, Platform, ProjectId, SessionId, WORKER_MAX_BYTES, WorkerHello,
    WorkerId, WorkerRole,
};
use omniusong_ipc::{
    FrameCeiling, FrameWriter, IpcError,
    unix::{LaunchConfig, WorkerFacts, connect_inherited, default_worker_limits, spawn_private},
};
use std::{
    io::{self, Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const MODE: &str = "OMNIUSONG_IPC_FIXTURE_MODE";
fn facts() -> WorkerFacts {
    WorkerFacts {
        role: WorkerRole::Engine,
        platform: Platform::Macos,
        architecture: Architecture::Aarch64,
        build_id: "ipc-process-fixture-v1".into(),
        limits: default_worker_limits(),
        capabilities: vec!["fixture.echo".into()],
    }
}
fn config(timeout: Duration) -> LaunchConfig {
    LaunchConfig {
        expected_role: WorkerRole::Engine,
        expected_platform: Platform::Macos,
        expected_architecture: Architecture::Aarch64,
        expected_build_id: Some("ipc-process-fixture-v1".into()),
        limits: default_worker_limits(),
        frame_ceiling: FrameCeiling::new(WORKER_MAX_BYTES).unwrap(),
        handshake_timeout: timeout,
    }
}
fn child_command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process_worker_entry", "--nocapture"])
        .env(MODE, mode)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Child-process fixture entry. In an ordinary test invocation it does no work.
#[test]
fn process_worker_entry() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    if mode == "silent" {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    if mode == "reject-stdio" {
        assert!(connect_inherited(facts(), Duration::from_secs(1)).is_err());
        return;
    }
    if matches!(mode.as_str(), "slow-hello" | "wrong-nonce" | "wrong-pid") {
        // Independent wire fixture reads the documented fixed bootstrap layout.
        let owned = io::stdin().as_fd().try_clone_to_owned().unwrap();
        let mut socket = UnixStream::from(owned);
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut header = [0; 4];
        socket.read_exact(&mut header).unwrap();
        assert_eq!(u32::from_le_bytes(header), 80);
        let mut bootstrap = [0; 80];
        socket.read_exact(&mut bootstrap).unwrap();
        assert_eq!(&bootstrap[..8], b"ONSIPC01");
        if mode == "slow-hello" {
            socket.write_all(&1_000_u32.to_le_bytes()).unwrap();
            for _ in 0..100 {
                if socket.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            return;
        }
        let session = SessionId(
            uuid::Uuid::from_slice(&bootstrap[8..24])
                .unwrap()
                .to_string(),
        );
        let worker_id = WorkerId(
            uuid::Uuid::from_slice(&bootstrap[24..40])
                .unwrap()
                .to_string(),
        );
        let nonce = if mode == "wrong-nonce" {
            "0".repeat(64)
        } else {
            bootstrap[40..72]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let mut extensions = Extensions::new();
        extensions.insert(
            "omniusong:process_id".into(),
            serde_json::json!(if mode == "wrong-pid" {
                u64::from(std::process::id()) + 1
            } else {
                u64::from(std::process::id())
            }),
        );
        let facts = facts();
        let hello = Envelope::new(
            session.clone(),
            MessageId("fixture-hello".into()),
            0,
            WorkerHello {
                worker_id,
                launch_nonce: nonce,
                role: facts.role,
                platform: facts.platform,
                architecture: facts.architecture,
                build_id: facts.build_id,
                min_protocol: PROTOCOL_VERSION,
                max_protocol: PROTOCOL_VERSION,
                limits: facts.limits,
                capabilities: facts.capabilities,
                extensions,
            },
        );
        let mut writer = FrameWriter::new(
            socket,
            FrameCeiling::new(WORKER_MAX_BYTES).unwrap(),
            session,
        )
        .unwrap();
        writer.send(&hello).unwrap();
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let mut facts = facts();
    if mode == "wrong-role" {
        facts.role = WorkerRole::Model {
            provider: "different-worker".into(),
        };
    }
    let mut connection = connect_inherited(facts, Duration::from_secs(3)).unwrap();
    let request = connection
        .receive::<AudioCommand>(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    assert!(matches!(request.payload.action, AudioAction::Panic));
    connection
        .send_payload(
            AudioEvent {
                in_reply_to: Some(request.message_id),
                notice: AudioNotice::Queued,
                extensions: Extensions::new(),
            },
            Duration::from_secs(3),
        )
        .unwrap();
}

#[test]
fn real_spawned_process_exchanges_typed_messages_and_fresh_launch_identities() {
    let mut old_session = None;
    let mut old_worker = None;
    for _ in 0..2 {
        let mut child =
            spawn_private(child_command("echo"), config(Duration::from_secs(3))).unwrap();
        assert_ne!(child.identity().process_id, std::process::id());
        assert_ne!(old_session.as_ref(), Some(&child.identity().session_id));
        assert_ne!(old_worker.as_ref(), Some(&child.identity().worker_id));
        old_session = Some(child.identity().session_id.clone());
        old_worker = Some(child.identity().worker_id.clone());
        assert_eq!(child.next_outgoing_sequence().unwrap(), 1);
        let sent = child
            .send_payload(
                AudioCommand {
                    project_id: ProjectId("project-1".into()),
                    action: AudioAction::Panic,
                    extensions: Extensions::new(),
                },
                Duration::from_secs(2),
            )
            .unwrap();
        let response = child
            .receive::<AudioEvent>(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(response.sequence, 1);
        assert_eq!(response.payload.in_reply_to, Some(sent));
        assert!(matches!(response.payload.notice, AudioNotice::Queued));
        // Wait with an explicit test deadline; PrivateChild drop kills/reaps on failure.
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < end, "child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn role_nonce_and_claimed_pid_mismatch_are_rejected_before_returning_a_channel() {
    for mode in ["wrong-role", "wrong-nonce", "wrong-pid"] {
        assert!(
            matches!(
                spawn_private(child_command(mode), config(Duration::from_secs(2))),
                Err(IpcError::Handshake(_))
            ),
            "mode {mode}"
        );
    }
}

#[test]
fn silent_and_slow_partial_hello_cannot_extend_absolute_handshake_deadline() {
    for mode in ["silent", "slow-hello"] {
        let start = Instant::now();
        assert!(
            matches!(
                spawn_private(child_command(mode), config(Duration::from_millis(150))),
                Err(IpcError::DeadlineExceeded)
            ),
            "mode {mode}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "partial traffic reset the deadline"
        );
    }
}

#[test]
fn arbitrary_stdio_is_rejected_instead_of_becoming_a_private_transport() {
    let status = child_command("reject-stdio")
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn negotiated_control_limit_is_enforced_before_sending() {
    let mut configuration = config(Duration::from_secs(2));
    configuration.limits.control_bytes = 64;
    let mut child = spawn_private(child_command("echo"), configuration).unwrap();
    assert!(matches!(
        child.send_payload(
            AudioCommand {
                project_id: ProjectId("project-1".into()),
                action: AudioAction::Panic,
                extensions: Extensions::new(),
            },
            Duration::from_secs(1)
        ),
        Err(IpcError::Oversized { ceiling: 64, .. })
    ));
    assert_eq!(child.next_outgoing_sequence().unwrap(), 1);
    child.terminate().unwrap();
}
