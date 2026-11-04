//! Bounded non-realtime IPC framing and launch-bound private transport.
//!
//! These APIs perform blocking I/O and allocate bounded control frames. They
//! must never be called from a device/render callback.

use omniusong_core::contracts::{
    ContractError, Envelope, SessionId, Validate, WORKER_MAX_BYTES, WirePayload,
};
use std::{
    fmt,
    io::{self, Read, Write},
};

#[cfg(unix)]
pub mod unix;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStage {
    Header,
    Payload,
}

#[derive(Debug)]
pub enum IpcError {
    Io(io::Error),
    InvalidEnvelope(ContractError),
    InvalidCeiling,
    ZeroLength,
    Oversized {
        announced: u64,
        ceiling: usize,
    },
    Truncated {
        stage: ReadStage,
        expected: usize,
        received: usize,
    },
    AllocationFailed,
    Poisoned,
    StaleSession,
    FirstSequence {
        received: u64,
    },
    Replay {
        previous: u64,
        received: u64,
    },
    SequenceWrap,
    UnsupportedPlatform,
    DeadlineExceeded,
    Handshake(&'static str),
    EntropyUnavailable,
}
impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IPC I/O failed: {e}"),
            Self::InvalidEnvelope(e) => write!(f, "Invalid IPC envelope: {e}"),
            Self::InvalidCeiling => f.write_str("IPC frame ceiling is outside the supported range"),
            Self::ZeroLength => f.write_str("An IPC frame cannot be empty"),
            Self::Oversized { announced, ceiling } => write!(f, "IPC frame length {announced} exceeds {ceiling} bytes"),
            Self::Truncated { stage, expected, received } => write!(f, "Truncated {stage:?}: received {received} of {expected} bytes"),
            Self::AllocationFailed => f.write_str("Unable to allocate bounded IPC frame"),
            Self::Poisoned => f.write_str("IPC stream is unusable after an earlier framing/admission failure"),
            Self::StaleSession => f.write_str("IPC envelope belongs to another session"),
            Self::FirstSequence { received } => write!(f, "A new IPC direction must start at sequence zero, received {received}"),
            Self::Replay { previous, received } => write!(f, "IPC sequence {received} does not follow {previous}"),
            Self::SequenceWrap => f.write_str("IPC sequence exhausted; establish a new session"),
            Self::UnsupportedPlatform => f.write_str("Private Windows IPC is unavailable until named-pipe ACL and peer checks are implemented"),
            Self::DeadlineExceeded => f.write_str("IPC deadline expired"),
            Self::Handshake(reason) => write!(f, "Private IPC handshake rejected: {reason}"),
            Self::EntropyUnavailable => f.write_str("OS entropy for the launch challenge is unavailable"),
        }
    }
}
impl std::error::Error for IpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::InvalidEnvelope(e) => Some(e),
            _ => None,
        }
    }
}
impl From<io::Error> for IpcError {
    fn from(e: io::Error) -> Self {
        if matches!(
            e.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ) {
            Self::DeadlineExceeded
        } else {
            Self::Io(e)
        }
    }
}
impl From<ContractError> for IpcError {
    fn from(e: ContractError) -> Self {
        Self::InvalidEnvelope(e)
    }
}
pub type Result<T> = std::result::Result<T, IpcError>;

/// A negotiated ceiling may reduce, never enlarge, the protocol maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameCeiling(usize);
impl FrameCeiling {
    pub fn new(bytes: usize) -> Result<Self> {
        if bytes == 0 || bytes > WORKER_MAX_BYTES {
            return Err(IpcError::InvalidCeiling);
        }
        Ok(Self(bytes))
    }
    pub fn bytes(self) -> usize {
        self.0
    }
}

/// Per-direction admission. A failed check never advances its state.
#[derive(Debug, Clone)]
pub struct Admission {
    session: SessionId,
    last_sequence: Option<u64>,
}
impl Admission {
    pub fn new(session: SessionId) -> Result<Self> {
        session.validate()?;
        Ok(Self {
            session,
            last_sequence: None,
        })
    }
    pub fn session_id(&self) -> &SessionId {
        &self.session
    }
    pub fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }
    pub fn next_sequence(&self) -> Result<u64> {
        self.last_sequence
            .map_or(Ok(0), |n| n.checked_add(1).ok_or(IpcError::SequenceWrap))
    }
    pub fn check<T: WirePayload>(&self, message: &Envelope<T>) -> Result<()> {
        if message.session_id != self.session {
            return Err(IpcError::StaleSession);
        }
        match self.last_sequence {
            None if message.sequence != 0 => Err(IpcError::FirstSequence {
                received: message.sequence,
            }),
            Some(u64::MAX) if message.sequence != u64::MAX => Err(IpcError::SequenceWrap),
            Some(previous) if message.sequence <= previous => Err(IpcError::Replay {
                previous,
                received: message.sequence,
            }),
            _ => Ok(()),
        }
    }
    pub fn admit<T: WirePayload>(&mut self, message: &Envelope<T>) -> Result<()> {
        message.validate()?;
        self.check(message)?;
        self.last_sequence = Some(message.sequence);
        Ok(())
    }
}

fn read_part<R: Read>(
    reader: &mut R,
    target: &mut [u8],
    stage: ReadStage,
    clean_eof: bool,
) -> Result<bool> {
    let mut received = 0;
    while received < target.len() {
        match reader.read(&mut target[received..]) {
            Ok(0) if received == 0 && clean_eof => return Ok(false),
            Ok(0) => {
                return Err(IpcError::Truncated {
                    stage,
                    expected: target.len(),
                    received,
                });
            }
            Ok(n) => received += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

/// Internal bootstrap and typed readers share this one length-before-allocation path.
pub(crate) fn read_frame<R: Read>(reader: &mut R, ceiling: usize) -> Result<Option<Vec<u8>>> {
    let mut header = [0_u8; 4];
    if !read_part(reader, &mut header, ReadStage::Header, true)? {
        return Ok(None);
    }
    let announced = u32::from_le_bytes(header) as u64;
    if announced == 0 {
        return Err(IpcError::ZeroLength);
    }
    if announced > ceiling as u64 {
        return Err(IpcError::Oversized { announced, ceiling });
    }
    // The checked conversion follows the ceiling comparison on every platform.
    let length =
        usize::try_from(announced).map_err(|_| IpcError::Oversized { announced, ceiling })?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(length)
        .map_err(|_| IpcError::AllocationFailed)?;
    payload.resize(length, 0);
    read_part(reader, &mut payload, ReadStage::Payload, false)?;
    Ok(Some(payload))
}
pub(crate) fn write_frame<W: Write>(writer: &mut W, payload: &[u8], ceiling: usize) -> Result<()> {
    if payload.is_empty() {
        return Err(IpcError::ZeroLength);
    }
    if payload.len() > ceiling || payload.len() > u32::MAX as usize {
        return Err(IpcError::Oversized {
            announced: payload.len() as u64,
            ceiling,
        });
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

/// Incoming errors poison the stream; never scan for a new header after failure.
pub struct FrameReader<R> {
    pub(crate) inner: R,
    ceiling: FrameCeiling,
    admission: Admission,
    poisoned: bool,
    ended: bool,
}
impl<R: Read> FrameReader<R> {
    pub fn new(inner: R, ceiling: FrameCeiling, session: SessionId) -> Result<Self> {
        Ok(Self {
            inner,
            ceiling,
            admission: Admission::new(session)?,
            poisoned: false,
            ended: false,
        })
    }
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    pub fn last_sequence(&self) -> Option<u64> {
        self.admission.last_sequence()
    }
    pub fn receive<T: WirePayload>(&mut self) -> Result<Option<Envelope<T>>> {
        self.receive_with_ceiling(self.ceiling)
    }
    /// An additional negotiated ceiling can only reduce this reader's limit.
    pub fn receive_with_ceiling<T: WirePayload>(
        &mut self,
        additional: FrameCeiling,
    ) -> Result<Option<Envelope<T>>> {
        if self.poisoned {
            return Err(IpcError::Poisoned);
        }
        if self.ended {
            return Ok(None);
        }
        let result = (|| {
            let Some(payload) = read_frame(
                &mut self.inner,
                self.ceiling.0.min(additional.0).min(T::MAX_BYTES),
            )?
            else {
                return Ok(None);
            };
            let message = Envelope::<T>::decode(&payload)?;
            // Admission occurs before exposing an operation to any durable consumer.
            self.admission.admit(&message)?;
            Ok(Some(message))
        })();
        match &result {
            Ok(None) => self.ended = true,
            Err(_) => self.poisoned = true,
            _ => {}
        }
        result
    }
}

/// Local validation failure emits no bytes; partial I/O failure poisons the stream.
pub struct FrameWriter<W> {
    pub(crate) inner: W,
    ceiling: FrameCeiling,
    admission: Admission,
    poisoned: bool,
}
impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W, ceiling: FrameCeiling, session: SessionId) -> Result<Self> {
        Ok(Self {
            inner,
            ceiling,
            admission: Admission::new(session)?,
            poisoned: false,
        })
    }
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    pub fn next_sequence(&self) -> Result<u64> {
        self.admission.next_sequence()
    }
    pub fn send<T: WirePayload>(&mut self, message: &Envelope<T>) -> Result<()> {
        self.send_with_ceiling(message, self.ceiling)
    }
    /// An additional negotiated ceiling can only reduce this writer's limit.
    pub fn send_with_ceiling<T: WirePayload>(
        &mut self,
        message: &Envelope<T>,
        additional: FrameCeiling,
    ) -> Result<()> {
        if self.poisoned {
            return Err(IpcError::Poisoned);
        }
        let bytes = message.encode()?;
        self.admission.check(message)?;
        let ceiling = self.ceiling.0.min(additional.0).min(T::MAX_BYTES);
        if bytes.len() > ceiling {
            return Err(IpcError::Oversized {
                announced: bytes.len() as u64,
                ceiling,
            });
        }
        if let Err(error) = write_frame(&mut self.inner, &bytes, ceiling) {
            self.poisoned = true;
            return Err(error);
        }
        self.admission.admit(message)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateTransportSupport {
    UnixLaunchBound,
    Unsupported,
}
pub fn private_transport_support() -> PrivateTransportSupport {
    if cfg!(unix) {
        PrivateTransportSupport::UnixLaunchBound
    } else {
        PrivateTransportSupport::Unsupported
    }
}
/// Windows must not fall back to arbitrary stdio or an unprotected pipe.
pub fn require_private_transport() -> Result<()> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(IpcError::UnsupportedPlatform)
    }
}
