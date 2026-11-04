//! Unix socket-pair transport bound to the child launched by this supervisor.
//!
//! This does not inspect kernel peer credentials or sandbox the child. A worker
//! which deliberately delegates its inherited socket can delegate its authority.
//! See docs/contracts/NATIVE-IPC.md for the exact identity boundary.

use crate::{FrameCeiling, FrameReader, FrameWriter, IpcError, Result, read_frame, write_frame};
use omniusong_core::contracts::{
    Architecture, CONTROL_MAX_BYTES, Envelope, Extensions, MAX_BLOCK_FRAMES, MessageId,
    PROTOCOL_VERSION, Platform, REALTIME_EVENT_CAPACITY, SessionId, Validate, WORKER_MAX_BYTES,
    WirePayload, WorkerHello, WorkerId, WorkerLimits, WorkerRole, WorkerWelcome,
};
use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::fd::{AsFd, OwnedFd},
    os::unix::net::UnixStream,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

const BOOTSTRAP_MAGIC: &[u8; 8] = b"ONSIPC01";
const BOOTSTRAP_BYTES: usize = 80;
const MAX_IO_TIMEOUT: Duration = Duration::from_secs(60);
const PROCESS_ID_FIELD: &str = "omniusong:process_id";

fn random_uuid() -> Result<Uuid> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| IpcError::EntropyUnavailable)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(Uuid::from_bytes(bytes))
}

fn deadline(timeout: Duration) -> Result<Instant> {
    if timeout.is_zero() || timeout > MAX_IO_TIMEOUT {
        return Err(IpcError::Handshake(
            "timeout must be greater than zero and at most 60 seconds",
        ));
    }
    Instant::now()
        .checked_add(timeout)
        .ok_or(IpcError::DeadlineExceeded)
}

/// OS timeouts are refreshed with the remaining *absolute* operation deadline.
/// A peer cannot extend the budget by sending one byte before each socket timeout.
struct DeadlineSocket {
    socket: UnixStream,
    deadline: Instant,
}
impl DeadlineSocket {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "absolute IPC deadline"))
    }
    fn ensure_completed_in_time(&self) -> io::Result<()> {
        self.remaining().map(|_| ())
    }
}
impl Read for DeadlineSocket {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.socket.set_read_timeout(Some(self.remaining()?))?;
        let count = self.socket.read(bytes)?;
        self.ensure_completed_in_time()?;
        Ok(count)
    }
}
impl Write for DeadlineSocket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket.set_write_timeout(Some(self.remaining()?))?;
        let count = self.socket.write(bytes)?;
        self.ensure_completed_in_time()?;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.ensure_completed_in_time()
    }
}

/// A private challenge, never put into command arguments, environment or logs.
struct Bootstrap {
    session: Uuid,
    worker: Uuid,
    nonce: [u8; 32],
    child_pid: u32,
    ceiling: FrameCeiling,
}
impl Bootstrap {
    fn new(child_pid: u32, ceiling: FrameCeiling) -> Result<Self> {
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| IpcError::EntropyUnavailable)?;
        Ok(Self {
            session: random_uuid()?,
            worker: random_uuid()?,
            nonce,
            child_pid,
            ceiling,
        })
    }
    fn session_id(&self) -> SessionId {
        SessionId(self.session.to_string())
    }
    fn worker_id(&self) -> WorkerId {
        WorkerId(self.worker.to_string())
    }
    fn nonce_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut result = String::with_capacity(64);
        for byte in self.nonce {
            result.push(DIGITS[(byte >> 4) as usize] as char);
            result.push(DIGITS[(byte & 15) as usize] as char);
        }
        result
    }
    fn encode(&self) -> [u8; BOOTSTRAP_BYTES] {
        let mut bytes = [0; BOOTSTRAP_BYTES];
        bytes[..8].copy_from_slice(BOOTSTRAP_MAGIC);
        bytes[8..24].copy_from_slice(self.session.as_bytes());
        bytes[24..40].copy_from_slice(self.worker.as_bytes());
        bytes[40..72].copy_from_slice(&self.nonce);
        bytes[72..76].copy_from_slice(&self.child_pid.to_le_bytes());
        bytes[76..80].copy_from_slice(&(self.ceiling.bytes() as u32).to_le_bytes());
        bytes
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != BOOTSTRAP_BYTES || &bytes[..8] != BOOTSTRAP_MAGIC {
            return Err(IpcError::Handshake("invalid private bootstrap format"));
        }
        let session = Uuid::from_slice(&bytes[8..24])
            .map_err(|_| IpcError::Handshake("invalid bootstrap session"))?;
        let worker = Uuid::from_slice(&bytes[24..40])
            .map_err(|_| IpcError::Handshake("invalid bootstrap worker"))?;
        if session.is_nil() || worker.is_nil() {
            return Err(IpcError::Handshake("empty bootstrap identity"));
        }
        let mut nonce = [0; 32];
        nonce.copy_from_slice(&bytes[40..72]);
        let child_pid = u32::from_le_bytes(bytes[72..76].try_into().expect("fixed slice"));
        if child_pid == 0 {
            return Err(IpcError::Handshake("empty expected process identity"));
        }
        let ceiling = FrameCeiling::new(u32::from_le_bytes(
            bytes[76..80].try_into().expect("fixed slice"),
        ) as usize)?;
        Ok(Self {
            session,
            worker,
            nonce,
            child_pid,
            ceiling,
        })
    }
}

pub fn default_worker_limits() -> WorkerLimits {
    WorkerLimits {
        control_bytes: CONTROL_MAX_BYTES as u32,
        worker_bytes: WORKER_MAX_BYTES as u32,
        realtime_events: REALTIME_EVENT_CAPACITY as u16,
        block_frames: MAX_BLOCK_FRAMES,
    }
}

#[derive(Debug, Clone)]
pub struct LaunchConfig {
    pub expected_role: WorkerRole,
    pub expected_platform: Platform,
    pub expected_architecture: Architecture,
    /// Optional exact build label; this is not executable signature attestation.
    pub expected_build_id: Option<String>,
    pub limits: WorkerLimits,
    pub frame_ceiling: FrameCeiling,
    pub handshake_timeout: Duration,
}
impl LaunchConfig {
    fn validate(&self) -> Result<()> {
        self.expected_role.validate()?;
        self.limits.validate()?;
        deadline(self.handshake_timeout)?;
        if self
            .expected_build_id
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 256)
        {
            return Err(IpcError::Handshake("invalid expected build label"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct WorkerFacts {
    pub role: WorkerRole,
    pub platform: Platform,
    pub architecture: Architecture,
    pub build_id: String,
    pub limits: WorkerLimits,
    pub capabilities: Vec<String>,
}

/// Evidence of a checked launch/hello, not a kernel per-message peer PID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchIdentity {
    pub process_id: u32,
    pub session_id: SessionId,
    pub worker_id: WorkerId,
    pub role: WorkerRole,
    pub platform: Platform,
    pub architecture: Architecture,
    pub build_id: String,
}

struct Channel {
    reader: FrameReader<DeadlineSocket>,
    writer: FrameWriter<DeadlineSocket>,
    session: SessionId,
    closed: bool,
    failed: bool,
    negotiated: Option<WorkerLimits>,
}
impl Channel {
    fn new(
        socket: UnixStream,
        session: SessionId,
        ceiling: FrameCeiling,
        until: Instant,
    ) -> Result<Self> {
        let writer = socket.try_clone()?;
        Ok(Self {
            reader: FrameReader::new(
                DeadlineSocket {
                    socket,
                    deadline: until,
                },
                ceiling,
                session.clone(),
            )?,
            writer: FrameWriter::new(
                DeadlineSocket {
                    socket: writer,
                    deadline: until,
                },
                ceiling,
                session.clone(),
            )?,
            session,
            closed: false,
            failed: false,
            negotiated: None,
        })
    }
    fn close(&mut self) {
        self.closed = true;
        let _ = self.reader.inner.socket.shutdown(Shutdown::Both);
    }
    fn typed_ceiling<T: WirePayload>(&self) -> Result<FrameCeiling> {
        let maximum = match &self.negotiated {
            Some(limits) if T::MAX_BYTES <= CONTROL_MAX_BYTES => limits.control_bytes,
            Some(limits) => limits.worker_bytes,
            None => WORKER_MAX_BYTES as u32,
        };
        FrameCeiling::new(maximum as usize)
    }
    fn send_until<T: WirePayload>(&mut self, message: &Envelope<T>, until: Instant) -> Result<()> {
        if self.closed {
            return Err(IpcError::Poisoned);
        }
        self.writer.inner.deadline = until;
        let ceiling = self.typed_ceiling::<T>()?;
        let result = self.writer.send_with_ceiling(message, ceiling);
        if self.writer.is_poisoned() {
            self.failed = true;
            self.close();
        }
        result
    }
    fn receive_until<T: WirePayload>(&mut self, until: Instant) -> Result<Option<Envelope<T>>> {
        if self.failed {
            return Err(IpcError::Poisoned);
        }
        if self.closed {
            return Ok(None);
        }
        self.reader.inner.deadline = until;
        let ceiling = self.typed_ceiling::<T>()?;
        let result = self.reader.receive_with_ceiling(ceiling);
        if result.is_err() {
            self.failed = true;
            self.close();
        } else if matches!(result, Ok(None)) {
            self.close();
        }
        result
    }
    fn envelope<T: WirePayload>(&self, payload: T) -> Result<Envelope<T>> {
        Ok(Envelope::new(
            self.session.clone(),
            MessageId(random_uuid()?.to_string()),
            self.writer.next_sequence()?,
            payload,
        ))
    }
}

struct ChildGuard(Option<Child>);
impl ChildGuard {
    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("guard owns child")
    }
    fn take(&mut self) -> Child {
        self.0.take().expect("guard owns child")
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct PrivateChild {
    child: Child,
    channel: Channel,
    identity: LaunchIdentity,
    limits: WorkerLimits,
}
impl PrivateChild {
    pub fn identity(&self) -> &LaunchIdentity {
        &self.identity
    }
    pub fn limits(&self) -> &WorkerLimits {
        &self.limits
    }
    pub fn next_outgoing_sequence(&self) -> Result<u64> {
        self.channel.writer.next_sequence()
    }
    pub fn send<T: WirePayload>(&mut self, message: &Envelope<T>, timeout: Duration) -> Result<()> {
        self.channel.send_until(message, deadline(timeout)?)
    }
    pub fn send_payload<T: WirePayload>(
        &mut self,
        payload: T,
        timeout: Duration,
    ) -> Result<MessageId> {
        let message = self.channel.envelope(payload)?;
        self.send(&message, timeout)?;
        Ok(message.message_id)
    }
    pub fn receive<T: WirePayload>(&mut self, timeout: Duration) -> Result<Option<Envelope<T>>> {
        self.channel.receive_until(deadline(timeout)?)
    }
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        Ok(self.child.try_wait()?)
    }
    pub fn terminate(&mut self) -> Result<ExitStatus> {
        self.channel.close();
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        Ok(self.child.wait()?)
    }
}
impl Drop for PrivateChild {
    fn drop(&mut self) {
        self.channel.close();
        // This owner must be dropped on a control thread, never in a callback.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn negotiate(offered: &WorkerLimits, maximum: &WorkerLimits) -> WorkerLimits {
    WorkerLimits {
        control_bytes: offered.control_bytes.min(maximum.control_bytes),
        worker_bytes: offered.worker_bytes.min(maximum.worker_bytes),
        realtime_events: offered.realtime_events.min(maximum.realtime_events),
        block_frames: offered.block_frames.min(maximum.block_frames),
    }
}
fn limits_within(selected: &WorkerLimits, offered: &WorkerLimits) -> bool {
    selected.control_bytes <= offered.control_bytes
        && selected.worker_bytes <= offered.worker_bytes
        && selected.realtime_events <= offered.realtime_events
        && selected.block_frames <= offered.block_frames
}

/// Launch a known executable with one private duplex socket inherited as fd 0.
/// The caller's stdout/stderr policy remains unchanged. No shell is introduced.
/// No endpoint is registered in the filesystem or bound to a network port.
pub fn spawn_private(mut command: Command, config: LaunchConfig) -> Result<PrivateChild> {
    config.validate()?;
    let (parent, child_socket) = UnixStream::pair()?;
    let owned: OwnedFd = child_socket.into();
    command.stdin(Stdio::from(owned));
    let child = command.spawn()?;
    // Drop Command promptly: it can retain its configured source descriptor.
    drop(command);
    let pid = child.id();
    let mut guard = ChildGuard(Some(child));
    let bootstrap = Bootstrap::new(pid, config.frame_ceiling)?;
    let until = deadline(config.handshake_timeout)?;
    let mut parent = DeadlineSocket {
        socket: parent,
        deadline: until,
    };
    write_frame(&mut parent, &bootstrap.encode(), BOOTSTRAP_BYTES)?;
    let mut channel = Channel::new(
        parent.socket,
        bootstrap.session_id(),
        config.frame_ceiling,
        until,
    )?;
    let hello = channel
        .receive_until::<WorkerHello>(until)?
        .ok_or(IpcError::Handshake("child closed before hello"))?;
    if guard.child_mut().try_wait()?.is_some() {
        return Err(IpcError::Handshake("child exited during handshake"));
    }
    if hello.payload.worker_id != bootstrap.worker_id() {
        return Err(IpcError::Handshake("worker identity mismatch"));
    }
    if hello.payload.launch_nonce != bootstrap.nonce_hex() {
        return Err(IpcError::Handshake("launch challenge mismatch"));
    }
    if hello
        .payload
        .extensions
        .get(PROCESS_ID_FIELD)
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(pid))
    {
        return Err(IpcError::Handshake(
            "worker process claim differs from the launched Child handle",
        ));
    }
    if hello.payload.role != config.expected_role
        || hello.payload.platform != config.expected_platform
        || hello.payload.architecture != config.expected_architecture
        || config
            .expected_build_id
            .as_ref()
            .is_some_and(|build| *build != hello.payload.build_id)
    {
        return Err(IpcError::Handshake(
            "worker role, platform, architecture or build mismatch",
        ));
    }
    let limits = negotiate(&hello.payload.limits, &config.limits);
    let welcome = channel.envelope(WorkerWelcome {
        worker_id: bootstrap.worker_id(),
        negotiated_protocol: PROTOCOL_VERSION,
        limits: limits.clone(),
        grants: Vec::new(),
        extensions: Extensions::new(),
    })?;
    channel.send_until(&welcome, until)?;
    if guard.child_mut().try_wait()?.is_some() {
        return Err(IpcError::Handshake(
            "child exited before handshake completed",
        ));
    }
    let identity = LaunchIdentity {
        process_id: pid,
        session_id: bootstrap.session_id(),
        worker_id: bootstrap.worker_id(),
        role: hello.payload.role,
        platform: hello.payload.platform,
        architecture: hello.payload.architecture,
        build_id: hello.payload.build_id,
    };
    channel.negotiated = Some(limits.clone());
    Ok(PrivateChild {
        child: guard.take(),
        channel,
        identity,
        limits,
    })
}

pub struct WorkerConnection {
    channel: Channel,
    worker_id: WorkerId,
    limits: WorkerLimits,
}
impl WorkerConnection {
    pub fn session_id(&self) -> &SessionId {
        &self.channel.session
    }
    pub fn worker_id(&self) -> &WorkerId {
        &self.worker_id
    }
    pub fn limits(&self) -> &WorkerLimits {
        &self.limits
    }
    pub fn next_outgoing_sequence(&self) -> Result<u64> {
        self.channel.writer.next_sequence()
    }
    pub fn send<T: WirePayload>(&mut self, message: &Envelope<T>, timeout: Duration) -> Result<()> {
        self.channel.send_until(message, deadline(timeout)?)
    }
    pub fn send_payload<T: WirePayload>(
        &mut self,
        payload: T,
        timeout: Duration,
    ) -> Result<MessageId> {
        let message = self.channel.envelope(payload)?;
        self.send(&message, timeout)?;
        Ok(message.message_id)
    }
    pub fn receive<T: WirePayload>(&mut self, timeout: Duration) -> Result<Option<Envelope<T>>> {
        self.channel.receive_until(deadline(timeout)?)
    }
}
impl Drop for WorkerConnection {
    fn drop(&mut self) {
        self.channel.close();
    }
}

/// Reject pipes/files/terminals/named listeners: fd 0 must be an anonymous Unix
/// socket pair supplied by the launcher. This does not attest the parent's PID.
pub fn connect_inherited(facts: WorkerFacts, timeout: Duration) -> Result<WorkerConnection> {
    let until = deadline(timeout)?;
    let owned = io::stdin().as_fd().try_clone_to_owned()?;
    let socket = UnixStream::from(owned);
    if !socket.local_addr()?.is_unnamed() || !socket.peer_addr()?.is_unnamed() {
        return Err(IpcError::Handshake(
            "stdin is not an anonymous private Unix socket",
        ));
    }
    let mut socket = DeadlineSocket {
        socket,
        deadline: until,
    };
    let bytes = read_frame(&mut socket, BOOTSTRAP_BYTES)?.ok_or(IpcError::Handshake(
        "launcher closed before private bootstrap",
    ))?;
    let bootstrap = Bootstrap::decode(&bytes)?;
    if bootstrap.child_pid != std::process::id() {
        return Err(IpcError::Handshake(
            "private bootstrap belongs to a different process",
        ));
    }
    let worker_id = bootstrap.worker_id();
    let mut channel = Channel::new(
        socket.socket,
        bootstrap.session_id(),
        bootstrap.ceiling,
        until,
    )?;
    let mut extensions = Extensions::new();
    extensions.insert(
        PROCESS_ID_FIELD.into(),
        serde_json::json!(std::process::id()),
    );
    let offered = facts.limits.clone();
    let hello = channel.envelope(WorkerHello {
        worker_id: worker_id.clone(),
        launch_nonce: bootstrap.nonce_hex(),
        role: facts.role,
        platform: facts.platform,
        architecture: facts.architecture,
        build_id: facts.build_id,
        min_protocol: PROTOCOL_VERSION,
        max_protocol: PROTOCOL_VERSION,
        limits: facts.limits,
        capabilities: facts.capabilities,
        extensions,
    })?;
    channel.send_until(&hello, until)?;
    let welcome = channel
        .receive_until::<WorkerWelcome>(until)?
        .ok_or(IpcError::Handshake("launcher closed before welcome"))?;
    if welcome.payload.worker_id != worker_id || !limits_within(&welcome.payload.limits, &offered) {
        return Err(IpcError::Handshake(
            "welcome identity or negotiated limits mismatch",
        ));
    }
    channel.negotiated = Some(welcome.payload.limits.clone());
    Ok(WorkerConnection {
        channel,
        worker_id,
        limits: welcome.payload.limits,
    })
}
