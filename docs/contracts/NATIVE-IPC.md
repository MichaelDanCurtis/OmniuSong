# Native IPC implementation, revision 1

`omniusong-ipc` implements the non-realtime control framing and private Unix worker launch boundary from [NATIVE-PROTOCOLS.md](NATIVE-PROTOCOLS.md). Its wire payloads remain [`omniusong-core::contracts`](../../crates/omniusong-core/src/contracts.rs). This crate does not implement an audio engine, callback queues, plugin processing, model execution, object streaming, or a recording journal.

## Public API and ownership

| API | Responsibility |
| --- | --- |
| `FrameCeiling::new(bytes)` | Validate a positive ceiling no greater than 65,536 bytes. A ceiling can only reduce a payload's protocol maximum. |
| `Admission::new(session)` / `admit(envelope)` | Validate the typed envelope and admit its session and sequence without modifying state on rejection. |
| `FrameReader::new(reader, ceiling, session)` / `receive::<T>()` | Read one bounded frame and return a fully validated, admitted `Envelope<T>`, clean EOF, or a terminal error. |
| `FrameWriter::new(writer, ceiling, session)` / `send(&envelope)` | Validate before writing and advance the sending sequence only after a complete write and flush. |
| `receive_with_ceiling` / `send_with_ceiling` | Apply an additional negotiated ceiling without enlarging the original ceiling. |
| `unix::spawn_private(Command, LaunchConfig)` | Launch an owned worker with a private socket pair; validate its hello; return a `PrivateChild`. |
| `unix::connect_inherited(WorkerFacts, timeout)` | Connect a worker through the inherited anonymous Unix socket; validate the bootstrap and welcome. |
| `PrivateChild` / `WorkerConnection` | Own the connection, expose typed send/receive operations with absolute deadlines, and retain negotiated limits. |
| `private_transport_support()` / `require_private_transport()` | Report Unix launch-bound support, or explicit unsupported status on Windows and other non-Unix targets. |

Every operation may allocate, block, perform system calls, serialize JSON, or reap a process. Construct, use, and drop these owners on non-realtime control or supervisor threads. The audio callback must receive prepared fixed-capacity records through the separately specified queues. No JSON, socket, subprocess handle, or these owners belong in a callback.

The generic reader/writer accept a caller-owned `Read`/`Write` implementation. They implement framing and admission, not authentication or a timeout policy for arbitrary streams. Callers must supply blocking streams with a bounded I/O policy. The private Unix wrappers supply that policy. `WouldBlock` and `TimedOut` I/O results map to `DeadlineExceeded`; do not use the generic API as a nonblocking polling interface.

## Framing, limits and admission

A normal frame contains a four-byte unsigned little-endian payload length followed by exactly that many UTF-8 JSON bytes. Zero-length frames are invalid. Before allocating or reading the body, the reader checks the announced length against:

```
min(connection ceiling, additional negotiated ceiling, T::MAX_BYTES)
```

Initial maxima are 4,096 bytes for audio command/event, job cancellation, and candidate decision; 16,384 bytes for captured events; and 65,536 bytes for worker handshake, project edit, and other job records. The four framing bytes are additional. No arbitrary inbound allocation is based on an unchecked length. The bounded byte buffer and JSON decoding allocate off realtime. Outbound typed envelope encoding validates the core contract and serialized size before emitting any bytes; this is not a general memory quota for locally constructed JSON values.

After handshake, payloads whose protocol ceiling is at most 4,096 use negotiated `control_bytes`; larger payload types use negotiated `worker_bytes`. The connection ceiling and individual protocol ceiling still apply. `realtime_events` and `block_frames` are negotiated metadata for later engine preparation; this crate does not create those queues or buffers. Hello and welcome use the connection and handshake protocol ceilings before negotiated limits take effect, permitting small application-message limits.

`receive::<T>()` selects an expected protocol. The core decoder checks the exact version, protocol discriminator, identifiers, semantic values, addresses and extension policy. Unknown extension metadata stays opaque, including exact unfamiliar JSON numbers. The receiver exposes the typed message only after all validation and admission checks pass. It does not attempt to infer a message type from untrusted fields or dispatch arbitrary operations.

Each direction has its own sequence history. The first envelope must have sequence zero. Subsequent sequences must strictly increase; gaps are allowed. A duplicate or decrease returns `Replay`, and another session returns `StaleSession`. After accepting `u64::MAX`, the sender cannot obtain another sequence; a lower incoming sequence returns `SequenceWrap`. Establish a fresh launch/session rather than resetting a live channel. None of these rejections changes the last admitted sequence.

Private handshake consumes sequence zero in both directions. Application messages therefore begin at sequence one. `send_payload` supplies a fresh random message ID and the next sequence. A caller using `send` supplies a complete envelope and gets the same validation. A message ID correlates replies; durable `operation_id` and expected project revision remain the store's atomic idempotency boundary. Successful IPC admission is not durable commit, queue admission, graph activation, or audible output.

## Failure behavior

- Clean EOF before any byte of a new header returns `None`; repeated reads remain `None`.
- EOF partway through a header or body returns `Truncated` with stage and observed count.
- Oversize, empty frame, malformed/unsupported JSON, invalid envelope, stale session, replay, read error or deadline expiry permanently poisons the incoming reader. It never scans for another header or rebases its sequence.
- Locally invalid or oversized output is rejected before any bytes are written. It does not poison the writer or consume a sequence, so the caller may correct it.
- A write or flush failure poisons the writer because the peer may have observed a partial or complete frame. Retry on that byte stream is forbidden.
- Private-channel read faults and poisoned writes shut down both directions. Later receives report `Poisoned`; clean peer EOF remains distinct. The generic framing owners cannot close an arbitrary borrowed stream; their owner must close/drop it after a terminal error.

Do not automatically resend a durable operation after an uncertain write. Reconnect through a new session and resolve the durable operation ledger using the same operation ID and canonical request content. This crate deliberately cannot decide whether a peer committed a received edit.

## Unix launch boundary

The supervisor creates `UnixStream::pair()` and passes one endpoint to the exact `Command` it launches, using `OwnedFd` and `Stdio` to inherit that duplex socket as fd 0. No shell, filesystem socket name, TCP listener, port reservation, or permission prompt is introduced. The parent closes its configured source descriptor by dropping `Command` immediately after spawning. The other endpoint is retained by the supervisor. stdout/stderr retain the caller's explicit `Command` policy; protocol bytes use neither stream.

The worker duplicates fd 0 and verifies that both socket addresses are unnamed. Ordinary pipes, `/dev/null`, files, terminals and pathname-bound Unix sockets are rejected. This check restricts the transport shape; it does not independently attest the launcher or peer process.

The authentication boundary is possession of an endpoint supplied by a trusted supervisor to its selected executable, combined with a fresh launch challenge and the owned child handle. Before returning a connection, the supervisor checks:

1. The hello is a valid versioned `WorkerHello` in the fresh session, at sequence zero.
2. Worker ID and the 256-bit launch nonce exactly match the per-launch bootstrap.
3. `omniusong:process_id` in hello extensions equals `Child::id()`.
4. Role, platform and architecture match `LaunchConfig`, and the build label matches when configured.
5. `Child::try_wait()` reports the launched process still running at the hello and welcome checkpoints.
6. Limits are valid; the welcome selects the per-field minimum of the worker offer and supervisor maximum.

The worker verifies that the bootstrap names its actual `process::id()`, sends the hello, then checks the welcome's session, sequence, version, worker identity and limits. Initial object grants are empty. A valid welcome must not enlarge its offered limits.

This is **launch-bound capability authentication**, not kernel peer-PID authentication. The reported PID and platform/build fields are claims checked against launch expectations; there is no `LOCAL_PEERPID`/`SO_PEERCRED`, executable signature or architecture attestation. A worker deliberately delegating its socket can delegate its authority. The crate does not sandbox plugins, protect against a compromised supervisor/OS, or assert that the process remains alive after a checked instant. Callers must launch an intended executable and supervise its subsequent lifetime.

Only the descriptors this implementation owns have an explicit lifetime here. It does **not** enumerate and close every unrelated inheritable descriptor in the parent, and makes no such claim. General descriptor-hygiene enforcement, OS sandboxing and kernel credentials remain integration hardening work. These limits narrow the current implementation relative to the complete production topology specified in NATIVE-PROTOCOLS.md.

## Private bootstrap and deadlines

The first supervisor-to-worker frame is an internal binary bootstrap, not an ordinary envelope. Its length must be exactly 80 bytes, checked against an 80-byte allocation ceiling:

| Offset | Bytes | Encoding |
| --- | ---: | --- |
| 0 | 8 | ASCII `ONSIPC01`, identifying bootstrap revision 1 |
| 8 | 16 | Session UUID bytes |
| 24 | 16 | Worker UUID bytes |
| 40 | 32 | Fresh OS-random launch nonce |
| 72 | 4 | Expected child PID, unsigned little endian |
| 76 | 4 | Connection frame ceiling, unsigned little endian |

The session/worker UUIDs and each generated message ID use fresh OS entropy. The full nonce contains 256 random bits; it is not derived from a UUID, time, process ID, counter or fallback PRNG. `getrandom = 0.4.3` failures return `EntropyUnavailable`; there is no insecure fallback. Nonces stay inside the private socket handshake and are not placed in arguments, environment variables or error messages. Owners and bootstrap state have no debug formatter that exposes the nonce.

Each handshake or public private-channel send/receive operation accepts a timeout greater than zero and at most 60 seconds. The supervisor uses one absolute deadline across bootstrap write, hello read and welcome write. The worker uses one across bootstrap read, hello write and welcome read. Each partial socket read/write refreshes the OS timeout from the remaining absolute budget and checks completion time. Slow partial traffic cannot restart the budget.

The handshake I/O deadline begins after process spawn and entropy acquisition; it is not a hard bound on OS process creation, entropy system calls, scheduling, or process reaping. Launch failure/handshake rejection drops a guard that kills and reaps the owned child. `PrivateChild::terminate` and `Drop` shut down the channel and terminate/reap that direct child; they do not manage arbitrary descendant process groups. A caller must not run these operations on the callback thread. Production supervision still needs process exit/status reporting, restart policy and the specified engine grace period.

## Windows and integration status

The Unix module is compiled only on Unix. `require_private_transport()` explicitly returns `UnsupportedPlatform` elsewhere. There is no generic stdio or permissive local-pipe fallback for Windows. A Windows implementation must create private named pipes with suitable ACLs, restricted handle inheritance, verified expected peer identity, bounded I/O and the same version/session/admission semantics. That implementation and Windows qualification remain pending.

The crate supplies one synchronous, typed control connection. It does not implement the 128-request application queue, callback rings, role-specific command authorization, independent telemetry channels, shared-memory plugin slots, 1 MiB object transfer, worker executable packaging or app-to-engine integration. Consumers must select the allowed payload type and validate current project/revision/permissions before submitting work. Passing a valid `Envelope<ProjectEditIntent>` does not authorize an engine or model worker to mutate the project store.

## Verification

Owned tests in `tests/framing.rs` exercise first sequence, gaps, replay/decrease/wrap, stale sessions, typed decode failure, header-before-body size rejection, EOF/truncation, permanent poison, local rejection, partial writes and real paired sockets with one-byte reads. `tests/process_transport.rs` launches an actual helper subprocess to exchange typed messages, check fresh session/worker identities, reject wrong role/nonce/PID, reject arbitrary stdin, enforce negotiated control limits, and terminate silent or slow partial handshakes. The helper is the test executable; it is not an engine implementation.

Independent tests live in `tests/review_regressions.rs`; they include exact extension number preservation and forged wrong-worker/stale-session child handshakes. Test commands:

```sh
cargo test -p omniusong-ipc --locked
cargo clippy -p omniusong-ipc --all-targets --locked -- -D warnings
```

Record the actual command outcomes and host in the migration evidence after running them. Source presence and a passing process fixture do not establish audio latency, device access, plugin safety, production packaging or the 30-minute recording acceptance result.

Observed October 20, 2026 on Darwin 27.0.0 arm64: the coordinator's fresh workspace all-features test run completed the IPC suites with 6 framing, 6 process-transport and 9 independent tests passing. The process count includes the helper entry, which is a no-op when invoked without its fixture mode. Evidence: `.runtime/rust-migration/recording-ipc-workspace-tests.log`. This observation covers IPC test results; the coordinator records the full workspace and Clippy disposition separately.
