//! Independent adversarial tests: bounded framing and per-direction admission.
use omniusong_core::contracts::{
    AudioAction, AudioCommand, CONTROL_MAX_BYTES, Envelope, Extensions, MessageId, ProjectId,
    SessionId, WORKER_MAX_BYTES,
};
use omniusong_ipc::{Admission, FrameCeiling, FrameReader, FrameWriter, IpcError, ReadStage};
use serde_json::json;
use std::{
    io::{self, Cursor, Read, Write},
    sync::{Arc, Mutex},
};

fn session() -> SessionId {
    SessionId("review-session".into())
}
fn message(sequence: u64) -> Envelope<AudioCommand> {
    Envelope::new(
        session(),
        MessageId(format!("message-{sequence}")),
        sequence,
        AudioCommand {
            project_id: ProjectId("review-project".into()),
            action: AudioAction::Stop,
            extensions: Extensions::new(),
        },
    )
}
fn ceiling() -> FrameCeiling {
    FrameCeiling::new(WORKER_MAX_BYTES).unwrap()
}
fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut framed = (bytes.len() as u32).to_le_bytes().to_vec();
    framed.extend_from_slice(bytes);
    framed
}
fn reader(bytes: Vec<u8>) -> FrameReader<Cursor<Vec<u8>>> {
    FrameReader::new(Cursor::new(bytes), ceiling(), session()).unwrap()
}

#[test]
fn admission_rejections_preserve_state_and_gaps_are_monotonic_not_contiguous() {
    let mut admission = Admission::new(session()).unwrap();
    assert!(matches!(
        admission.admit(&message(1)),
        Err(IpcError::FirstSequence { received: 1 })
    ));
    assert_eq!(admission.last_sequence(), None);
    admission.admit(&message(0)).unwrap();
    let mut stale = message(1);
    stale.session_id = SessionId("old-session".into());
    assert!(matches!(
        admission.admit(&stale),
        Err(IpcError::StaleSession)
    ));
    assert!(matches!(
        admission.admit(&message(0)),
        Err(IpcError::Replay { .. })
    ));
    assert_eq!(admission.last_sequence(), Some(0));
    admission.admit(&message(7)).unwrap();
    assert!(matches!(
        admission.admit(&message(6)),
        Err(IpcError::Replay { .. })
    ));
    assert_eq!(admission.last_sequence(), Some(7));
    admission.admit(&message(u64::MAX)).unwrap();
    assert!(matches!(
        admission.next_sequence(),
        Err(IpcError::SequenceWrap)
    ));
    assert!(matches!(
        admission.admit(&message(0)),
        Err(IpcError::SequenceWrap)
    ));
    assert_eq!(admission.last_sequence(), Some(u64::MAX));
}

struct HeaderOnly {
    header: Cursor<[u8; 4]>,
    calls: Arc<Mutex<usize>>,
}
impl Read for HeaderOnly {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        *self.calls.lock().unwrap() += 1;
        if self.header.position() == 4 {
            panic!("Oversized frame tried to read payload");
        }
        self.header.read(bytes)
    }
}

#[test]
fn control_channel_ceiling_is_checked_before_any_payload_read() {
    for announced in [CONTROL_MAX_BYTES as u32 + 1, u32::MAX] {
        let calls = Arc::new(Mutex::new(0));
        let input = HeaderOnly {
            header: Cursor::new(announced.to_le_bytes()),
            calls: calls.clone(),
        };
        let mut reader = FrameReader::new(input, ceiling(), session()).unwrap();
        assert!(matches!(
            reader.receive::<AudioCommand>(),
            Err(IpcError::Oversized {
                ceiling: CONTROL_MAX_BYTES,
                ..
            })
        ));
        assert_eq!(*calls.lock().unwrap(), 1);
        assert!(reader.is_poisoned());
        assert!(matches!(
            reader.receive::<AudioCommand>(),
            Err(IpcError::Poisoned)
        ));
        assert_eq!(*calls.lock().unwrap(), 1);
    }
}

#[test]
fn eof_zero_length_and_truncation_have_distinct_terminal_outcomes() {
    let mut clean = reader(Vec::new());
    assert_eq!(clean.receive::<AudioCommand>().unwrap(), None);
    assert!(!clean.is_poisoned());
    assert_eq!(clean.receive::<AudioCommand>().unwrap(), None);
    for count in 1..4 {
        let mut partial = reader(vec![1; count]);
        assert!(
            matches!(partial.receive::<AudioCommand>(), Err(IpcError::Truncated { stage: ReadStage::Header, expected: 4, received }) if received == count)
        );
        assert!(partial.is_poisoned());
    }
    let mut partial = reader([10_u32.to_le_bytes().as_slice(), b"abc"].concat());
    assert!(matches!(
        partial.receive::<AudioCommand>(),
        Err(IpcError::Truncated {
            stage: ReadStage::Payload,
            expected: 10,
            received: 3
        })
    ));
    assert!(matches!(
        partial.receive::<AudioCommand>(),
        Err(IpcError::Poisoned)
    ));
    let mut empty = reader(0_u32.to_le_bytes().to_vec());
    assert!(matches!(
        empty.receive::<AudioCommand>(),
        Err(IpcError::ZeroLength)
    ));
}

#[test]
fn malformed_channel_version_and_replay_never_resynchronize_to_a_later_good_frame() {
    for invalid in [
        b"{".to_vec(),
        {
            let mut value = serde_json::to_value(message(0)).unwrap();
            value["protocol"] = json!("omniusong.job-result");
            serde_json::to_vec(&value).unwrap()
        },
        {
            let mut value = serde_json::to_value(message(0)).unwrap();
            value["version"] = json!(2);
            serde_json::to_vec(&value).unwrap()
        },
        {
            let mut value = serde_json::to_value(message(0)).unwrap();
            value["payload"]["project_id"] = json!("");
            serde_json::to_vec(&value).unwrap()
        },
    ] {
        let mut stream = frame(&invalid);
        stream.extend(frame(&message(0).encode().unwrap()));
        let mut input = reader(stream);
        assert!(matches!(
            input.receive::<AudioCommand>(),
            Err(IpcError::InvalidEnvelope(_))
        ));
        assert_eq!(input.last_sequence(), None);
        assert!(matches!(
            input.receive::<AudioCommand>(),
            Err(IpcError::Poisoned)
        ));
    }
    let good = frame(&message(0).encode().unwrap());
    let mut input = reader([good.clone(), good, frame(&message(1).encode().unwrap())].concat());
    assert!(input.receive::<AudioCommand>().unwrap().is_some());
    assert!(matches!(
        input.receive::<AudioCommand>(),
        Err(IpcError::Replay { .. })
    ));
    assert_eq!(input.last_sequence(), Some(0));
    assert!(matches!(
        input.receive::<AudioCommand>(),
        Err(IpcError::Poisoned)
    ));
}

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);
impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn local_rejection_emits_no_bytes_and_does_not_consume_sequence() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut output = FrameWriter::new(SharedWriter(bytes.clone()), ceiling(), session()).unwrap();
    let mut invalid = message(0);
    invalid.payload.project_id = ProjectId(String::new());
    assert!(output.send(&invalid).is_err());
    assert!(output.send(&message(1)).is_err());
    assert!(bytes.lock().unwrap().is_empty());
    assert_eq!(output.next_sequence().unwrap(), 0);
    assert!(!output.is_poisoned());
    output.send(&message(0)).unwrap();
    assert_eq!(output.next_sequence().unwrap(), 1);
    let before = bytes.lock().unwrap().clone();
    assert!(output.send(&message(0)).is_err());
    assert_eq!(*bytes.lock().unwrap(), before);
}

struct FailsAfter {
    bytes: Arc<Mutex<Vec<u8>>>,
    remaining: usize,
}
impl Write for FailsAfter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "isolated review fault",
            ));
        }
        let count = self.remaining.min(bytes.len());
        self.bytes
            .lock()
            .unwrap()
            .extend_from_slice(&bytes[..count]);
        self.remaining -= count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn partial_header_or_payload_write_poisoning_prevents_retry_bytes() {
    for allowed in [2, 4, 9] {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let mut output = FrameWriter::new(
            FailsAfter {
                bytes: bytes.clone(),
                remaining: allowed,
            },
            ceiling(),
            session(),
        )
        .unwrap();
        assert!(output.send(&message(0)).is_err());
        assert!(output.is_poisoned());
        assert_eq!(bytes.lock().unwrap().len(), allowed);
        let before = bytes.lock().unwrap().clone();
        assert!(matches!(output.send(&message(0)), Err(IpcError::Poisoned)));
        assert_eq!(*bytes.lock().unwrap(), before);
    }
}

#[test]
fn fragmented_frames_preserve_unknown_large_integers_and_long_decimals() {
    let mut original = message(0);
    let opaque = serde_json::from_str::<serde_json::Value>(r#"{"integer":1234567890123456789012345678901234567890,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
    original.extensions.insert("vendor".into(), opaque.clone());
    original.payload.extensions.insert("vendor".into(), opaque);
    struct Fragmented(Cursor<Vec<u8>>);
    impl Read for Fragmented {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let count = out.len().min(1);
            self.0.read(&mut out[..count])
        }
    }
    let mut output = Vec::new();
    FrameWriter::new(&mut output, ceiling(), session())
        .unwrap()
        .send(&original)
        .unwrap();
    let mut input =
        FrameReader::new(Fragmented(Cursor::new(output)), ceiling(), session()).unwrap();
    let actual = input.receive::<AudioCommand>().unwrap().unwrap();
    assert_eq!(actual, original);
    let encoded = String::from_utf8(actual.encode().unwrap()).unwrap();
    assert!(encoded.contains("1234567890123456789012345678901234567890"));
    assert!(encoded.contains("0.123456789012345678901234567890123456789"));
}

#[cfg(unix)]
#[test]
fn paired_socket_fragmentation_and_peer_death_preserve_committed_frame_boundary() {
    use std::{os::unix::net::UnixStream, time::Duration};
    let (mut sender, receiver) = UnixStream::pair().unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let encoded = frame(&message(0).encode().unwrap());
    let producer = std::thread::spawn(move || {
        for chunk in encoded.chunks(3) {
            sender.write_all(chunk).unwrap();
        }
        sender.write_all(&100_u32.to_le_bytes()).unwrap();
        sender.write_all(b"incomplete").unwrap();
        // Closing this private peer is the meaningful half-frame failure.
    });
    let mut input = FrameReader::new(receiver, ceiling(), session()).unwrap();
    assert_eq!(input.receive::<AudioCommand>().unwrap(), Some(message(0)));
    assert!(matches!(
        input.receive::<AudioCommand>(),
        Err(IpcError::Truncated {
            stage: ReadStage::Payload,
            expected: 100,
            received: 10
        })
    ));
    assert_eq!(input.last_sequence(), Some(0));
    assert!(matches!(
        input.receive::<AudioCommand>(),
        Err(IpcError::Poisoned)
    ));
    producer.join().unwrap();
}

#[cfg(unix)]
#[test]
fn launch_rejects_correct_challenge_with_wrong_worker_identity_or_stale_session() {
    use omniusong_core::contracts::{
        Architecture, PROTOCOL_VERSION, Platform, WorkerHello, WorkerId, WorkerRole,
    };
    use omniusong_ipc::unix::{LaunchConfig, default_worker_limits, spawn_private};
    use std::{
        os::{fd::AsFd, unix::net::UnixStream},
        process::{Command, Stdio},
        time::Duration,
    };
    const MODE: &str = "OMNIUSONG_REVIEW_BAD_HELLO";
    if let Ok(mode) = std::env::var(MODE) {
        let fd = std::io::stdin().as_fd().try_clone_to_owned().unwrap();
        let mut socket = UnixStream::from(fd);
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut length = [0; 4];
        socket.read_exact(&mut length).unwrap();
        assert_eq!(u32::from_le_bytes(length), 80);
        let mut bootstrap = [0; 80];
        socket.read_exact(&mut bootstrap).unwrap();
        assert_eq!(&bootstrap[..8], b"ONSIPC01");
        let expected_session = SessionId(
            uuid::Uuid::from_slice(&bootstrap[8..24])
                .unwrap()
                .to_string(),
        );
        let expected_worker = WorkerId(
            uuid::Uuid::from_slice(&bootstrap[24..40])
                .unwrap()
                .to_string(),
        );
        let session = if mode == "session" {
            SessionId("previous-launch-session".into())
        } else {
            expected_session
        };
        let worker = if mode == "worker" {
            WorkerId("different-worker".into())
        } else {
            expected_worker
        };
        // Retain the actual challenge exclusively in the inherited private stream.
        let launch_nonce: String = bootstrap[40..72]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut extensions = Extensions::new();
        extensions.insert("omniusong:process_id".into(), json!(std::process::id()));
        let hello = Envelope::new(
            session.clone(),
            MessageId("review-hello".into()),
            0,
            WorkerHello {
                worker_id: worker,
                launch_nonce,
                role: WorkerRole::Engine,
                platform: Platform::Macos,
                architecture: Architecture::Aarch64,
                build_id: "independent-review".into(),
                min_protocol: PROTOCOL_VERSION,
                max_protocol: PROTOCOL_VERSION,
                limits: default_worker_limits(),
                capabilities: vec![],
                extensions,
            },
        );
        FrameWriter::new(socket, ceiling(), session)
            .unwrap()
            .send(&hello)
            .unwrap();
        // Supervisor rejection must close, kill and reap this owned fixture.
        loop {
            std::thread::park();
        }
    }
    for mode in ["worker", "session"] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "launch_rejects_correct_challenge_with_wrong_worker_identity_or_stale_session",
                "--nocapture",
            ])
            .env(MODE, mode)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let result = spawn_private(
            command,
            LaunchConfig {
                expected_role: WorkerRole::Engine,
                expected_platform: Platform::Macos,
                expected_architecture: Architecture::Aarch64,
                expected_build_id: Some("independent-review".into()),
                limits: default_worker_limits(),
                frame_ceiling: ceiling(),
                handshake_timeout: Duration::from_secs(2),
            },
        );
        if mode == "worker" {
            assert!(matches!(
                result,
                Err(IpcError::Handshake("worker identity mismatch"))
            ));
        } else {
            assert!(matches!(result, Err(IpcError::StaleSession)));
        }
    }
}
