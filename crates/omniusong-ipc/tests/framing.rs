use omniusong_core::contracts::{
    AudioAction, AudioCommand, CONTROL_MAX_BYTES, Envelope, Extensions, MessageId, ProjectId,
    SessionId, WORKER_MAX_BYTES,
};
use omniusong_ipc::{Admission, FrameCeiling, FrameReader, FrameWriter, IpcError, ReadStage};
use std::io::{self, Cursor, Read, Write};

fn session() -> SessionId {
    SessionId("session-1".into())
}
fn message(sequence: u64) -> Envelope<AudioCommand> {
    Envelope::new(
        session(),
        MessageId(format!("message-{sequence}")),
        sequence,
        AudioCommand {
            project_id: ProjectId("project-1".into()),
            action: AudioAction::Panic,
            extensions: Extensions::new(),
        },
    )
}
fn bytes(message: &Envelope<AudioCommand>) -> Vec<u8> {
    let payload = message.encode().unwrap();
    let mut framed = (payload.len() as u32).to_le_bytes().to_vec();
    framed.extend(payload);
    framed
}
fn ceiling() -> FrameCeiling {
    FrameCeiling::new(WORKER_MAX_BYTES).unwrap()
}

#[test]
fn admission_allows_gaps_but_rejects_replay_stale_first_and_wrap_without_advancing() {
    let mut admission = Admission::new(session()).unwrap();
    assert!(matches!(
        admission.admit(&message(1)),
        Err(IpcError::FirstSequence { .. })
    ));
    assert_eq!(admission.last_sequence(), None);
    admission.admit(&message(0)).unwrap();
    admission.admit(&message(7)).unwrap();
    for seq in [7, 6, 0] {
        assert!(matches!(
            admission.admit(&message(seq)),
            Err(IpcError::Replay { .. })
        ));
        assert_eq!(admission.last_sequence(), Some(7));
    }
    let mut stale = message(8);
    stale.session_id = SessionId("different-session".into());
    assert!(matches!(
        admission.admit(&stale),
        Err(IpcError::StaleSession)
    ));
    assert_eq!(admission.last_sequence(), Some(7));
    let mut invalid = message(8);
    invalid.version = 2;
    assert!(matches!(
        admission.admit(&invalid),
        Err(IpcError::InvalidEnvelope(_))
    ));
    assert_eq!(admission.last_sequence(), Some(7));
    admission.admit(&message(u64::MAX)).unwrap();
    assert!(matches!(
        admission.admit(&message(0)),
        Err(IpcError::SequenceWrap)
    ));
    assert!(matches!(
        admission.next_sequence(),
        Err(IpcError::SequenceWrap)
    ));
    assert_eq!(admission.last_sequence(), Some(u64::MAX));
}

#[test]
fn clean_eof_is_distinct_from_header_payload_and_zero_length_failures() {
    let mut clean = FrameReader::new(Cursor::new(Vec::<u8>::new()), ceiling(), session()).unwrap();
    assert!(clean.receive::<AudioCommand>().unwrap().is_none());
    assert!(clean.receive::<AudioCommand>().unwrap().is_none());
    assert!(!clean.is_poisoned());
    for count in 1..4 {
        let mut reader =
            FrameReader::new(Cursor::new(vec![1; count]), ceiling(), session()).unwrap();
        assert!(matches!(reader.receive::<AudioCommand>(),
            Err(IpcError::Truncated { stage: ReadStage::Header, received, .. }) if received == count));
        assert!(reader.is_poisoned());
        assert!(matches!(
            reader.receive::<AudioCommand>(),
            Err(IpcError::Poisoned)
        ));
    }
    let mut payload = 5_u32.to_le_bytes().to_vec();
    payload.extend(b"abc");
    let mut reader = FrameReader::new(Cursor::new(payload), ceiling(), session()).unwrap();
    assert!(matches!(
        reader.receive::<AudioCommand>(),
        Err(IpcError::Truncated {
            stage: ReadStage::Payload,
            expected: 5,
            received: 3
        })
    ));
    let mut zero = FrameReader::new(Cursor::new([0_u8; 4]), ceiling(), session()).unwrap();
    assert!(matches!(
        zero.receive::<AudioCommand>(),
        Err(IpcError::ZeroLength)
    ));
}

#[test]
fn typed_decode_or_session_failure_closes_admission_instead_of_resynchronizing() {
    for problem in ["version", "session", "replay"] {
        let mut stream = bytes(&message(0));
        let mut next = message(if problem == "replay" { 0 } else { 1 });
        if problem == "session" {
            next.session_id = SessionId("old-session".into());
        }
        if problem == "version" {
            let mut value = serde_json::to_value(&next).unwrap();
            value["version"] = 2.into();
            let payload = serde_json::to_vec(&value).unwrap();
            stream.extend((payload.len() as u32).to_le_bytes());
            stream.extend(payload);
        } else {
            stream.extend(bytes(&next));
        }
        stream.extend(bytes(&message(2)));
        let mut reader = FrameReader::new(Cursor::new(stream), ceiling(), session()).unwrap();
        assert_eq!(
            reader.receive::<AudioCommand>().unwrap().unwrap().sequence,
            0
        );
        assert!(reader.receive::<AudioCommand>().is_err());
        assert_eq!(reader.last_sequence(), Some(0));
        assert!(matches!(
            reader.receive::<AudioCommand>(),
            Err(IpcError::Poisoned)
        ));
    }
}

#[test]
fn outgoing_validation_does_not_emit_bytes_and_partial_write_failure_poisoned() {
    let mut output = Vec::new();
    {
        let mut writer = FrameWriter::new(&mut output, ceiling(), session()).unwrap();
        let mut invalid = message(0);
        invalid.version = 9;
        assert!(writer.send(&invalid).is_err());
        assert_eq!(writer.next_sequence().unwrap(), 0);
        assert!(!writer.is_poisoned());
        writer.send(&message(0)).unwrap();
        assert!(matches!(
            writer.send(&message(0)),
            Err(IpcError::Replay { .. })
        ));
    }
    assert_eq!(output, bytes(&message(0)));
    struct FailAfterHeader(usize);
    impl Write for FailAfterHeader {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 >= 6 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture"));
            }
            let n = bytes.len().min(6 - self.0);
            self.0 += n;
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = FrameWriter::new(FailAfterHeader(0), ceiling(), session()).unwrap();
    assert!(matches!(writer.send(&message(0)), Err(IpcError::Io(_))));
    assert!(writer.is_poisoned());
    assert_eq!(writer.next_sequence().unwrap(), 0);
    assert!(matches!(writer.send(&message(0)), Err(IpcError::Poisoned)));
}

#[test]
fn announced_length_is_rejected_after_four_bytes_before_body_read_or_allocation() {
    struct HeaderOnly {
        header: Cursor<[u8; 4]>,
    }
    impl Read for HeaderOnly {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            assert!(
                self.header.position() < 4,
                "attempted to read an oversized body"
            );
            self.header.read(bytes)
        }
    }
    for announced in [u32::MAX, (CONTROL_MAX_BYTES + 1) as u32] {
        let mut reader = FrameReader::new(
            HeaderOnly {
                header: Cursor::new(announced.to_le_bytes()),
            },
            ceiling(),
            session(),
        )
        .unwrap();
        assert!(matches!(
            reader.receive::<AudioCommand>(),
            Err(IpcError::Oversized {
                ceiling: CONTROL_MAX_BYTES,
                ..
            })
        ));
        assert_eq!(reader.last_sequence(), None);
    }
    assert!(FrameCeiling::new(0).is_err());
    assert!(FrameCeiling::new(WORKER_MAX_BYTES + 1).is_err());
    let mut reader =
        FrameReader::new(Cursor::new(bytes(&message(0))), ceiling(), session()).unwrap();
    assert!(matches!(
        reader.receive_with_ceiling::<AudioCommand>(FrameCeiling::new(8).unwrap()),
        Err(IpcError::Oversized { ceiling: 8, .. })
    ));
}

#[cfg(unix)]
#[test]
fn real_socket_pair_handles_one_byte_reads_and_peer_payload_truncation() {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    struct OneByte<R>(R);
    impl<R: Read> Read for OneByte<R> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let limit = bytes.len().min(1);
            self.0.read(&mut bytes[..limit])
        }
    }
    let (read, mut write) = UnixStream::pair().unwrap();
    read.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let expected = message(0);
    let stream = bytes(&expected);
    let worker = std::thread::spawn(move || {
        for byte in stream {
            write.write_all(&[byte]).unwrap();
        }
    });
    let mut reader = FrameReader::new(OneByte(read), ceiling(), session()).unwrap();
    assert_eq!(reader.receive::<AudioCommand>().unwrap().unwrap(), expected);
    worker.join().unwrap();
    assert!(reader.receive::<AudioCommand>().unwrap().is_none());
    let (read, mut write) = UnixStream::pair().unwrap();
    read.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    write.write_all(&100_u32.to_le_bytes()).unwrap();
    write.write_all(b"partial").unwrap();
    drop(write);
    let mut reader = FrameReader::new(read, ceiling(), session()).unwrap();
    assert!(matches!(
        reader.receive::<AudioCommand>(),
        Err(IpcError::Truncated {
            stage: ReadStage::Payload,
            expected: 100,
            received: 7
        })
    ));
}
