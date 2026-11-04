# Native protocol and recording decisions, revision 1

Adopted October 20, 2026 for the Rust migration. The coordinator owns changes to these shared contracts. This document resolves the next implementation boundaries; it does not claim an engine, process transport, recorder, plugin host, or model adapter already implements them. The corresponding checked data types are in [`omniusong-core::contracts`](../../crates/omniusong-core/src/contracts.rs). The [full goal](../GOAL.md) and [release matrix](../RELEASE-ACCEPTANCE.md) remain mandatory.

## Ownership and process topology

The production topology is a Rust application process, a supervised Rust engine process, isolated Rust plugin workers, and isolated Rust model workers. These are bundled executables started automatically by the application. No browser, Node, Python service, or user-managed server is introduced.

| Owner | Responsibilities | Forbidden ownership |
| --- | --- | --- |
| Application/control process | Native UI; project/edit/candidate transactions; main project store; job and scanner supervision; config; permission requests; worker status; immutable object grants | Device callback, arbitrary plugin library loading, inference in a paint callback |
| Engine process | Native audio/MIDI device lifetime; control-side graph preparation; scheduler; DSP; playback voice handles; plugin-processing supervision; recorder writer thread and per-take journal | UI-owned mutable project references, model execution, project database queries in audio callback |
| One plugin scanner process per scan job | Load/enumerate one candidate binary, bounded metadata response, validation and exit | Playback graph, project store, access to original recording mutation |
| One plugin worker per active instance | Format host adapter; plugin code/state/editor event loop; bounded prepared audio/event slots | Main project database, model workers, engine device ownership |
| One model worker per admitted job initially | Pinned native model/runtime; input grants; progress; bounded outputs; cancellation | Project commits, candidate acceptance, device callback, unrestricted project-directory access |

The engine owns plugin worker lifetime so that UI failure does not tear down the render callback mid-write. The general control-process-loss policy permits a **5-second configurable grace period**, then stops playback at a block boundary and exits; this is an initial policy, not a measured guarantee. **Capture amendment, October 20, 2026:** the first native audio implementation stops input on parent/IPC loss, drains only provably accepted data when possible, and retains the journal **unsealed**. Parent loss, stream error and forced termination must never manufacture a successful final tail. Only an explicit `FinishCapture` request may enter normal sealing, after callback-producer quiescence and verified drain; interrupted-prefix recovery remains a separate explicit action. The general five-second policy does not authorize continued microphone capture or automatic sealing after control-process loss. See [the engine contract](NATIVE-AUDIO-ENGINE.md). Restart discovers retained takes and interrupted jobs. Do not automatically continue a paid request, restart a microphone recording, accept a candidate, or replay user note-ons after restart.

The application supervises workers through private inherited IPC handles. The initial Mac transport is local Unix-domain sockets for control; Windows uses private named pipes with an ACL for the current user and expected child process. The supervisor supplies a fresh 256-bit launch nonce, validates child identity and nonce, and closes inherited unrelated handles. A nonce field alone is not authentication. Local IPC is not a listening TCP service.

The implemented [Unix launcher](NATIVE-IPC.md) currently binds a private inherited anonymous socket pair to an owned child handle, fresh nonce/session, expected role/platform/build label and checked hello. It does not yet inspect kernel peer credentials or close every ambient inheritable descriptor. Those production hardening checks and Windows named-pipe ACL/identity enforcement remain pending; no stronger authentication or sandbox claim follows from the current launcher.

Plugins remain untrusted native code with access bounded by process placement and OS facilities. A subprocess is crash isolation, not a claim of a fully sandboxed plugin. Actual OS sandbox compatibility, editor hosting, signing and entitlement requirements need qualification. AUv3 system-managed hosting is additional platform behavior, not a substitute for our AUv2/worker boundary.

## Versions, framing, extensions and object grants

Every control record is an `Envelope<T>`:

```json
{
  "protocol": "omniusong.audio-command",
  "version": 1,
  "session_id": "engine-session-17",
  "message_id": "command-42",
  "sequence": 42,
  "payload": {
    "project_id": "project-7",
    "action": {"command": "panic"}
  },
  "vendor:trace": {"receipt": "retained"}
}
```

IPC framing is a **4-byte unsigned little-endian payload byte length**, followed by that many UTF-8 JSON bytes. A receiver reads and validates the length before allocating a frame, applies its channel-specific maximum, and calls `Envelope::<T>::decode`. JSON transport is entirely non-realtime. No line-delimited parser, compiler-dependent Rust memory layout, or unchecked generic serde call is the process admission boundary.

Before control envelopes, the current private Unix launcher sends one fixed 80-byte bootstrap, itself length-prefixed and checked before allocation. Its independently versioned magic, byte layout, nonce handling and absolute deadline are defined in [NATIVE-IPC.md](NATIVE-IPC.md). This bootstrap is a private launch capability exchange, not an alternate unvalidated command channel. `WorkerHello` debug formatting redacts its nonce field; arbitrary peer-supplied text still must not be logged as trusted diagnostic data.

| Protocol | Payload type | Initial maximum serialized JSON bytes |
| --- | --- | ---: |
| `omniusong.audio-command` | `AudioCommand` | 4,096 |
| `omniusong.audio-event` | `AudioEvent` | 4,096 |
| `omniusong.capture` | `CapturedEvent` | 16,384 |
| `omniusong.project-edit` | `ProjectEditIntent` | 65,536 |
| `omniusong.job-request` | `JobRequest` | 65,536 |
| `omniusong.job-cancel` | `JobCancel` | 4,096 |
| `omniusong.job-update` | `JobUpdate` | 65,536 |
| `omniusong.job-result` | `JobResult` | 65,536 |
| `omniusong.candidate-decision` | `CandidateDecision` | 4,096 |
| `omniusong.worker-hello` / `omniusong.worker-welcome` | `WorkerHello` / `WorkerWelcome` | 65,536 |

Limits are initial configuration and protocol ceilings, not measured performance. Negotiation may reduce them. Raising a hard ceiling requires a reviewed contract/version decision and capacity tests. Big plans, event sequences, model artifacts, plugin state, and candidate proposals travel as `BlobRef` capabilities through a separate bounded streaming/object channel, not inline in a 4 KiB command. The initial object chunk transfer is at most 1 MiB per message with backpressure on non-realtime producer threads.

A `BlobRef` contains SHA-256, byte length, and media type. It is not a path, URL, network-download request, or authorization to read arbitrary files. The supervisor resolves only immutable objects explicitly granted to the worker, streams or maps them read-only, validates digest and byte count, and applies a job-specific limit lower than the absolute 1 TiB protocol ceiling. Model manifests pin weights, tokenizer, preprocessing/configuration and runtime dependencies; a weight hash alone is not a complete identity.

Version 1 is exact for a selected protocol. Unknown versions, wrong protocol channels, unknown semantic variants, invalid sizes/addresses, nonfinite numbers, and extension keys that shadow known fields fail before admission. A worker hello offers a supported range, but `WorkerWelcome` selects exactly one supported version. Unsupported peers receive an actionable error and no plugin/model load request.

`#[serde(flatten)]` maps preserve unknown extension metadata at declared envelope/record extension points, including exact unfamiliar JSON numbers through `serde_json`'s `arbitrary_precision`. Opaque fields never become local asset references because they contain the word `assetId`. Strict inner address/operation structs reject unknown fields rather than silently consuming an unrecognized semantic layout; extension data belongs at the explicit enclosing extension point. This rule is for new protocol records, not a license to discard unknown legacy project fields.

Wire IDs are application-issued opaque strings of at most 96 ASCII characters. Native endpoint names/UIDs remain exact source metadata; adapters assign a stable application identity instead of forcing arbitrary OS strings into the wire-ID grammar. Sequence numbers are monotonic per sending channel and session; message IDs identify replies. Reject stale session messages, duplicate/out-of-order commands, and sequence wrap. Duplicate durable operation IDs use the idempotency rules below rather than blindly executing a replayed command.

## Control-to-engine admission and realtime queues

The application sends `AudioCommand` to the engine's non-realtime control thread. That thread resolves object grants, validates project/render revision and clock epoch, prepares graph/buffers/plugin endpoints, interns IDs into numeric handles, and compiles timestamped events. Only then can it publish `RealtimeControl` or `RealtimeBatch` to the callback.

`PreparePlan` does not activate a graph. `Prepared` means resources are ready on the control side. `ActivatePlan` requests adoption at a block boundary in a specific audio clock epoch. `Queued` means admitted to a bounded queue; `Applied` includes the render revision and actual adoption frame. Neither means sound reached a speaker. `ChunkCommitted` means the writer completed a durable journal transaction; `RecordingFinalized` means a sealed manifest exists. Neither is implied by `StopRecording` being queued.

Initial limits:

| Boundary | Capacity / policy |
| --- | --- |
| Application→engine pending control requests | 128 frames × maximum 4 KiB; queue-full rejects admission before `Queued` |
| Engine control→callback prepared control ring | 128 fixed `RealtimeControl` records; consume at most 32 per callback |
| Scheduler→callback event ring | 64 fixed `RealtimeBatch` records; each at most 256 events and 2,048 audio frames |
| Native MIDI ingress | 16,384 fixed cells containing timestamp, source handle, packet/fragment fields; assembly/JSON conversion off callback |
| Audio capture transfer | 256 preallocated slots; each holds at most 2,048 frames × 16 channels of f32 plus fixed metadata |
| Plugin processing slots | Three fixed shared-memory slots per instance; one-quantum initial bridge latency |
| Active inference | One admitted CPU job initially; explicit measured admission policy required before concurrent jobs |

These capacities are negotiated/configured before stream activation. The device adapter reports its actual buffer size, channel count and rate. Unsupported configurations fail or trigger a stopped-stream reconfiguration; no callback grows buffers. A stream buffer change starts a new audio epoch and invalidates queued blocks prepared for the previous configuration.

`RealtimeBatch` is fixed-capacity and `Copy`, with a bounded event slice, ordered offsets within a block, signed start frame and numeric epoch handle. `RealtimeControl` contains only fixed-size commands and prepared handles. Neither owns a `String`, `Vec`, JSON value, `Arc` destructor, project reference, or database handle. They are in-process types; their Rust layout is not the worker/shared-memory ABI.

Prepared plans have explicit generation handles. The callback announces its completed generation through an atomic/return ring; retired resources are destroyed on the control thread only after the callback can no longer access them. No callback-side refcount finalizer, `Drop` of a plugin, allocator call, logging formatter, mutex wait, database/file/network call, subprocess call, or model execution is permitted. Panic/unwind must not cross FFI; callback adapters fail to silence and publish fixed telemetry.

Queue overload is explicit:

1. Reject new control requests with `queue_full`; preserve operation ID so the UI can retry intentionally. Do not acknowledge a queued edit or graph change that was dropped.
2. Never overwrite a captured original packet/audio slot. Increment fixed overflow counters, retain the earliest affected sequence/frame, publish a visible fault, and journal a gap when the writer catches up.
3. If an event block cannot retain note-off/pedal/panic semantics, mark the run degraded and request a panic at the next block through a dedicated atomic emergency flag that cannot be crowded out by the ordinary ring. The callback clears its active voice state with bounded work. Do not silently drop releases and leave orphaned voices.
4. Coalesce expendable UI meters/progress only. Do not coalesce original capture, final job outcomes, durable commit receipts, or accepted edits.
5. A wrong-epoch or late realtime batch is rejected and counted. It is never replayed at an unrelated new clock origin. The non-realtime scheduler resolves restart/seek and supplies a fresh plan.

The initial built-in DSP path need not add a whole plugin quantum. The isolated plugin bridge does, and its latency is part of graph compensation and the software response budget. Any under-10-ms claim must name the actual graph, plugin bridge, buffer/rate and input/output timestamp endpoints.

## Plugin audio transport and failure policy

Adopt one isolated process per instance initially. A scanner never becomes the engine process. AU on Mac is implemented first, followed by CLAP and VST3 on their applicable platforms. Mac native adapters use the investigated CoreAudio/CoreMIDI bindings; Windows uses a qualified WASAPI path and independently qualified MIDI backend. The [native audio research](../research/RUST-AUDIO-MIDI-PLUGINS.md) remains the source for API/version/license constraints; the Windows MIDI Services shipping target is not established merely by this protocol.

Control IPC carries plugin identity, format, bus/parameter/capability metadata, object references, activation state, latency and editor lifecycle. Audio/event processing uses the fixed slot mapping established before activation. Shared memory is granted only to the intended engine/instance pair. Slots have checked bounds, monotonically increasing sequence, stream epoch, signed block start, sample rate, frame count, bus layout revision and event count. Every field has an explicitly encoded fixed integer representation; PCM is planar IEEE-754 f32, little endian. Events are translated to format-specific native calls inside the worker. Native pointers, Rust enums, trait objects and plugin pointers never cross the mapping.

The implementing plugin transport must define and test the exact byte offsets/alignment in its separately versioned slot codec before activation; that codec is not implemented by the current control contract. The negotiated maxima above and the sequence/epoch/ownership rules are fixed for the first implementation. A producer owns a free slot until publishing it with release ordering; a consumer acquires publication, validates bounds/sequence, consumes it, and returns ownership. No side may reuse an in-flight slot or wait for a peer on the audio callback.

Block N is submitted for a future compensated block. If the worker output is not ready at its scheduled block, the engine supplies silence for an instrument or latency-aligned dry bypass for an effect where the graph declares such a path. It never replays old audio or waits for IPC. Every miss is counted. Three consecutive misses fault/bypass the instance and request non-realtime termination; this threshold is initial policy, not proof of a safe deadline on any machine.

Plugin state changes/checkpoints happen off realtime at an explicit quiescent/format-approved boundary. A checkpoint is stored as an immutable object with plugin binary identity/version and parameter schema. After a crash, preserve the project and last good state, mark the instance unavailable, and require an explicit restart. Restart does not replay note-ons until a new transport/voice plan is prepared. Freeze/bounce records source revision, plugin identities, latency, tails and output assets; it never replaces the only source state.

CLAP note IDs, VST3 note-expression IDs and AU/MIDI paths are adapter-local translations of `VoiceId`, not raw project UUID truncations. UMP input does not prove arbitrary MIDI 2 messages can pass through every plugin API. Supported dialects and any expression loss must be reported before rendering; unsupported expression remains preserved in the project.

## Clock, capture, identity and expression

`AudioFrame` carries a **signed i64 timeline frame**, sample rate, and audio epoch. `SourceFrame` in core remains an unsigned offset into an immutable recording. Musical ticks retain their own type and schema-1 PPQ 960. The first native tempo-map schema will convert ticks to frames explicitly; this protocol does not reinterpret legacy tick integers or assert that all musical positions are samples.

`HostTimestamp` carries source clock domain, boot/connection epoch and exact u64 ticks. A `ClockAnchor` couples a host timestamp to an audio frame, a rational host-ticks-per-second rate, and uncertainty. `CaptureTime` retains the observed timestamp even if no calibrated alignment is available. If alignment is present, validation requires matching host domain/epoch, matching audio epoch/rate, and a converted frame within the retained uncertainty plus one final rounding frame. It rejects apparent agreement achieved by silently treating host ticks as nanoseconds.

Native adapters own clock calibration and refresh anchors when drift requires it. Device restart creates a new audio epoch; native clock reset creates a new host epoch. Clock uncertainty and discontinuity remain stored evidence. Recording compensation is a reversible alignment transform on an arrangement clip, not a rewrite of original ticks, frames or packet timestamps.

`CapturedEvent` contains:

- An immutable `EventId`, endpoint identity and connection generation, UMP group and optional zero-based channel.
- Original UMP words or MIDI 1 bytes, with original resolution. Short/raw packets become structured envelopes off the callback. Long MIDI 1/SysEx streams are retained as ordered bounded fragments in the raw journal and reassembled only on the control/protocol path with declared limits. Malformed/unsupported source data can be retained as a `Quarantined` immutable object reference with a reason and cannot directly trigger a musical event.
- Optional interpreted musical data with stable `NoteId` and a distinct `VoiceId`. Capturing creates a voice occurrence; replay/audition/each loop allocates a new playback occurrence. The source note ID remains unchanged.
- Unknown metadata at explicit extension points. Captured measured expression refers back to its own event; edited and estimated expression use their operation/job origin in prepared musical objects.

The decoder still needs independent MIDI fixtures. The contract's packet-length/address checks do not constitute a complete MIDI parser or validate every possible semantic interpretation. MIDI 1 and MIDI 2 Channel Voice messages do not carry a general DAW UUID for simultaneous identical pitches. Preserve ambiguity/matching policy and raw data; do not promise independent per-note control when the wire cannot express it.

Expression values declare pitch cents, normalized values, or exact protocol integers with 7/14/16/32-bit precision. Channel and MPE-zone scopes remain distinct from per-note targets. Manager/member configuration, bend ranges, pre-note gestures, sustain, physical release, release velocity, voice tails and channel reuse belong to the MPE resolver and raw-capture history. Extending note duration alone is not faithful sustain replay.

MIDI-CI, standard Clip File encoding, MIDI 1 fallback and native UMP endpoint delivery remain separate modules and gates. A failed Windows UMP backend may expose an honestly labeled MIDI 1 mode, but that fallback cannot close the mandatory Windows MIDI 2 requirement.

## Project edits and candidate transactions

Every `ProjectEditIntent` carries `operation_id`, `project_id`, `expected_revision`, explicit scope and an operation. IDs are generated once by the initiating action and reused on retry. Selection references track/note identities, not current row numbers. A prepared edit object is a typed versioned semantic proposal, not an unrestricted JSON merge patch.

The store/control layer must atomically:

1. Resolve the current project, validate the expected revision and selected identities, inspect current locks and source-preservation rules, and decode/validate any referenced edit/candidate object.
2. Check an operation ledger. A repeated operation ID with identical canonical request content returns its original durable result. The same ID with different content is rejected.
3. Validate the resulting document and all new object grants; publish project revision, undo/redo history/cursor, candidate decision and operation receipt in one main-store transaction.
4. Return the committed revision only after commit. Audio activation is a separate request/acknowledgement.

A worker cannot commit a project. `CandidateDecision::Accept` and `EditOperation::AcceptCandidate` route through the same acceptance transaction and operation ledger; they are not two acceptance implementations. Acceptance checks both the candidate's base revision and current expected revision, exact scope and locks, and object validation. A stale result remains available for comparison but is not silently rebased. Explicit rebasing is a new user-visible operation/candidate.

Candidate states are `Ready → Auditioning → Ready`, with acceptance/discard/invalidation allowed from ready or auditioning. Accepted/discarded/invalidated are terminal candidate decisions. Undoing acceptance is a new project revision that reverses its musical effect; it does not rewrite history to claim the candidate was never accepted. Source assets and the acceptance receipt remain retained.

## Model jobs and worker readiness

`JobRequest` declares one of the eight required roles, exact model/runtime identity, project/base revision, scope, immutable inputs, parameter object, wall-time/memory/thread/output budget, and operation ID. `JobResult` names validated output objects and a candidate proposal; it is not an accepted edit. Artifact validation includes byte count/hash, format decoding, finite values, duration/channel/rate/placement bounds, allowed scope and provenance. A syntactically valid envelope alone cannot mark inference successful.

The adopted job lifecycle is:

```text
queued → loading → running → succeeded
   │         │        │
   └─────────┴────────┴→ cancelling → cancelled
   └─────────┴────────┴─────────────→ failed / interrupted
```

`JobCancel` supplies job and operation IDs. The supervisor serializes cancellation and completion decisions. Once cancellation is admitted as `Cancelling`, a later worker success cannot become an accepted/succeeded job; retained output may be recorded as unaccepted diagnostic material. A completion committed before cancellation admission is already terminal, and cancellation reports that outcome. Cancellation is cooperative first, followed by a bounded timeout and worker termination; only the supervisor commits `Cancelled`. Progress can be coalesced; terminal outcomes cannot.

Restart changes an incomplete running/loading job to `Interrupted`; it does not pretend to resume the same computation. Retry creates a new job/attempt with explicit intent and retained provenance. This also prevents automatic paid resubmission. Model install, verified artifact, runtime loading, ready, running inference, valid result and candidate acceptance remain separate states. No mock/procedural output may pass as inference.

`WorkerHello` reports role, platform/architecture, build ID, protocol range, limits and capabilities. The supervisor verifies expected role/identity, nonce and compatible versions, then sends `WorkerWelcome` with a selected version, limits and read-only object grants. Hello does not mean a plugin/model is loaded. A subsequent role-specific preparation receipt identifies actual loaded artifacts and supported capabilities; the initial model implementation must add its tested loader/control messages before advertising readiness.

## Long recording: immutable chunks and durable publication

The 30 MiB asset, 48 MiB legacy-import total, 1.9 MB JSON and 4,096 embedded capture-event caps are **schema-1 bridge limits only**. New native recording must not embed an entire take or raw event stream in project JSON, or lift those caps while retaining whole-take buffering. Thirty minutes of stereo 48 kHz 24-bit PCM is 518,400,000 bytes before headers; f32 storage is larger. Thirty minutes is an acceptance minimum, not a new maximum.

Adopt this storage boundary for milestone 2:

1. Each take has an engine-writer-owned SQLite journal under the native store, identified by `TakeId`. It is independent of the main editable project database so a long project/candidate transaction cannot block original capture. Only the recorder writer thread performs journal I/O. `synchronous=FULL`, rollback-journal atomic transactions, checked schema versions and explicit commit receipts apply initially.
2. Audio arrives through the preallocated transfer ring. The writer forms an immutable chunk of at most **1 MiB PCM**, flushing at **250 ms of accumulated audio** or the byte limit, whichever comes first. The policy bounds normal uncommitted work; it is not a promised fsync deadline. Raw-event chunks are at most **64 KiB** with a **50 ms** accumulation target. Large packets can span chunks with explicit stream/sequence references.
3. A single journal transaction inserts chunk bytes, digest, stream/segment ID, monotonic chunk sequence, source-frame/event range, audio format, clock-anchor references and gap records, and advances the durable prefix. Chunk payloads are append-only. A hash includes the versioned canonical chunk header and content; metadata needed to interpret PCM is not left unauthenticated.
4. `ChunkCommitted` is emitted only after that transaction commits. It identifies the take, stream and chunk sequence and reports the latest durable source-frame and/or event prefix; a MIDI-only take does not require fictitious audio frames. “Captured into RAM,” “queued for disk,” “written,” and “durably committed” remain distinguishable. If the disk stalls, bounded buffers eventually fill; report loss and stop recording cleanly rather than overwrite original data or freeze the callback.
5. At stop, drain accepted slots, commit remaining chunks, validate sequence/ranges/hashes, seal a manifest and mark the take sealed in one journal transaction. A sealed manifest records audio/MIDI streams, segments, clocks, gaps, source provenance and format. It refers to chunk hashes and does not copy PCM into project JSON.
6. The application publishes a reference to the durable sealed take and arrangement metadata in its main project transaction. Publication may include history and operation receipt. A crash between take sealing and project publication leaves an orphaned **recoverable take**, not a partially visible track. Recovery enumerates journals; it never deletes them because project publication did not finish.

On process death, SQLite recovers the journal's committed transactions. The recovery reader verifies integrity, sequence/range continuity and hashes, marks the unfinished take `Interrupted`, and exposes the proven committed prefix with explicit gaps. The uncommitted tail is reported as lost or of unknown extent according to retained evidence; it is never represented as successfully recorded silence. Original chunks survive recovery. Continuing capture starts a new segment/epoch or take with a visible relationship, not fabricated continuity.

Main-store states refer only to sealed immutable manifests; an actively recording take is a recovery/in-progress reference, not a playable “complete” original. Auditioning a growing take requires a separate committed-prefix reader. User-approved recovery of a partial take creates a new sealed manifest over existing verified chunks, preserving the interrupted source receipt.

Archives export canonical manifests and every referenced chunk, including original/derived relationships, undo/history references, accepted candidate provenance and selected pending-recovery material. Export uses a consistent SQLite read/backup mechanism or a sealed journal, never a raw copy of an active SQLite file. Restore verifies all objects in staging before publishing a new project. Garbage collection is out of the first recording milestone; eventual collection must account for undo, candidates, archives-in-progress and recovery roots and must never infer orphaned capture means disposable data.

This deliberately introduces staged immutable recording objects before their project transaction. The schema-1 bridge's single-database imports remain unchanged. [Musical schema 2](NATIVE-TAKES.md) now introduces explicit take/manifest references without reinterpreting existing `assetId` strings or discarding legacy source JSON. [Owned take storage](NATIVE-TAKE-STORE.md) separates durable ingestion/registration from atomic project/history/operation publication.

The implemented [recording journal](NATIVE-RECORDING.md) supplies bounded chunk storage, live writer/recovery ownership, immutable interruption/index receipts, explicit recovered sealing, fixed-view sealed readers and consistent backups. Callback transfer rings, timed aggregation and device capture remain pending. An OS owner lock prevents ordinary/symlink aliases from taking over a live journal; Unix hard-linked journals are rejected. Equivalent Windows alias protection still requires platform work and qualification.

The main store's [edit ledger](NATIVE-EDITS.md) implements atomic idempotent rename, tempo, paired session updates, undo, redo and sealed-take publication. Its SQLite schema version 4 is independent of musical schema 2. [Native archives](NATIVE-ARCHIVE.md) retain complete current/history/operation/import/copy-proof reference closures and full originals. [Typed copy restore](NATIVE-ARCHIVE-COPY.md) retains immutable lineage; OSARv2 exports and strict v1 reading are supported. The [background session service](NATIVE-SESSION-SERVICE.md) owns store/UI operations with stable durable action IDs and bounded transport. Actual Mac archive UI acceptance, selected unassigned-recovery export/management, prepared edits and candidate acceptance remain pending. Library publication is not an operating recorder/engine workflow.

Required pre-device tests: capture larger than 30 MiB and more than 4,096 raw events with bounded memory; every chunk/commit/finalization kill boundary; disk-full/stall/queue-overflow cases; no partial project publication; restore/hash/gap checks; concurrent project edits while recording; repeated finalization/operation IDs; source/undo retention. Only subsequent actual device evidence can pass the 30-minute no-dropout requirement.

## Verification and next implementation owners

The current contract tests cover exact extension-number round trips, unsupported/wrong protocol and size rejection, clock domain/epoch/rate/alignment integrity, address/value validation, fixed-batch ordering/capacity, worker negotiation, model-result round trips, and terminal lifecycle rules. They do not test OS IPC, audio scheduling, lock freedom, durable journal behavior, native MIDI semantics, plugin crashes, real inference, latency or long recording.

Reproduce the contract check with:

```sh
cargo test --locked -p omniusong-core contracts::tests
```

Before independent implementations begin, owners must keep these dependencies:

- **Store/recording owner:** native take/manifest schema, operation ledger and transaction receipts, per-take journal and recovery/archives. Must not silently reuse legacy asset limits.
- **Engine/device owner:** private IPC framing/handshake, native clocks, bounded queue implementation, prepared plan lifetime, fixed callback buffers, recorder producer and telemetry.
- **Plugin owner:** stable slot codec, loader/editor/lifecycle adapters, one-quantum bridge, latency compensation and failure fixtures. Implement only against the engine's adopted epoch/buffer/voice contracts.
- **MIDI owner:** raw capture/resolver contracts, codecs and MPE state, independent CI/file/native-transport modules and format loss reports.
- **Model owner:** role-specific load/cancel protocol additions, supervisor admission, bounded input/output grants and actual inference. Job/candidate decisions remain main-store-owned.

Any added wire message must define its protocol name/version, byte limits, extension points, state transition and independent fixtures. Engine, plugin, recorder and expressive-MIDI implementations still require independent review before their respective acceptance milestones.
