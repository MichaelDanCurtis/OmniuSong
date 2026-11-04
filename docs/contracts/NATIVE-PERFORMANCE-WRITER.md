# Native performance writer and checkpoint recovery

October 20, 2026. Milestone 2 implementation contract. This extends the qualified library-only [performance tap](../reviews/RUST-PERFORMANCE-TAP-SOURCE-REVIEW.md) with an off-callback incremental writer and explicit recovery of a consistent six-stream prefix. The frozen [capture architecture](../reviews/RUST-PERFORMANCE-CAPTURE-ARCHITECTURE.md) remains the broader plan. This document specifies the next device-free packet; it is not evidence that its implementation or tests have passed.

This packet opens no native input or output stream. It adds no worker recording command, GUI recording control, automatic project attachment, editable-note projection or normal Finish/seal claim. Those remain required work. The tap's F32 samples are the actual logical post-mix bus before device-format conversion; they are not physical sound or the exact integer samples submitted to an I16/U16 device.

## Ownership and API boundaries

The audio callback remains limited to the existing fixed `PerformancePacket` transfer. It does not construct any type in this document, serialize, hash, allocate, lock the receiver, access SQLite or wait for storage.

| Module | Responsibility |
| --- | --- |
| `omniusong-recording` journal primitives | Bounded atomic groups of existing immutable chunks; exact retry; read-only ordinal access; explicit selected-prefix recovered sealing. No knowledge of live DSP or performance-record semantics. |
| `engine::performance_records` | Versioned owned descriptor/record codec, six-stream binding, exact clock representability and deterministic capture envelopes. |
| `engine::performance_writer` | Validate actual tap groups, reduce them to chunks, commit checkpoints, retain interrupted originals and validate/recover the last complete performance checkpoint. |
| Later worker/supervisor integration | Own the draining thread, callback/error-closure lifetime, early stage identity, stop/finish fence and retained terminal delivery. |

The writer is moved to one non-realtime owner. There is one consumer of `PerformanceTapOwner`; neither a second receiver nor a second writer is permitted. The admission/supervisor retains the private tap backing until renderer **and native error closure** destruction are acknowledged. Writer failure cancels the tap and closes note admission through the later supervisor; it cannot silently revert to unrecorded audition. Dropping a writer does not assert that callbacks stopped.

The initial reducer entry points are:

```rust
PerformanceWriter::create(path: &Path, config: PerformanceConfig) -> Result<Self>;
PerformanceWriter::push_packet(&mut self, packet: PerformancePacket)
    -> Result<WriterProgress>;
PerformanceWriter::flush(&mut self) -> Result<WriterProgress>;
PerformanceWriter::progress(&self) -> WriterProgress;
PerformanceWriter::retry_pending(&mut self) -> Result<WriterProgress>;
PerformanceWriter::close_interrupted(self) -> Result<RetainedPerformance>;
```

`flush` commits only completed callbacks. `close_interrupted` leaves the original unsealed; there is deliberately no `finish(bool)`, public normal-seal helper or caller-supplied quiescence boolean. An adapter may drain a bounded number of packets per iteration, but must retain the same receiver and writer between iterations. An empty queue is not EOF. The writer can distinguish producer destruction from an empty queue without treating producer destruction alone as complete native shutdown.

`WriterProgress` separates `received_packets`, `complete_callbacks`, `complete_frames`, `durable_callbacks`, `durable_frames`, `checkpoint_sha256`, `durable_prefixes` and the first writer error. Capture-side first fault is a separate field: a valid fault-bearing End is original evidence, not malformed syntax or an SQLite error. All counters have explicit units. A packet received, a callback completed, a transaction committed and a take recovered are different facts. `RetainedPerformance` identifies the path/take/descriptor and last committed checkpoint plus interruption/unknown-tail status; it is not a sealed manifest or an attachment receipt. Read-only `path()` and `descriptor()` accessors expose early stage identity without implying publication.

## Journal v1: atomic groups without migration

Keep the existing SQLite journal version, six tables, immutable chunk triggers and descriptor/manifest version 1. A performance checkpoint is an original raw control record in the **last chunk of its transaction**, not a new mutable cache or side table. Existing non-performance journals retain their interpretation.

The generic addition is:

```rust
pub struct AppendChunk<'a> {
    pub header: &'a ChunkHeader,
    pub payload: &'a [u8],
}
pub struct AppendGroupReceipt {
    pub group_sha256: String,
    pub first_ordinal: u64,
    pub last_ordinal: u64,
    pub chunks: Vec<ChunkReceipt>,
    pub replayed: bool,
}
Journal::append_group(&mut self, chunks: &[AppendChunk<'_>])
    -> recording::Result<AppendGroupReceipt>;
```

The group has 1–64 chunks. The sum of canonical header bytes and payload bytes is at most 8 MiB, checked with overflow-safe arithmetic before mutation. Existing lower limits still apply: audio payload at most 1 MiB, raw payload at most 64 KiB, header at most 64 KiB, each capture envelope at most 16 KiB. Reject duplicate `(stream_id, sequence)` addresses inside one request. Validate every header, payload, range, descriptor/clock/format and checked signed-storage counter before committing any member.

For a new group, one `BEGIN IMMEDIATE` transaction checks recording state, reads affected stream prefixes, simulates all advances in supplied order, inserts every chunk and updates every affected prefix, then commits. Global ordinals are consecutive in that supplied order. Prefix changes and rows roll back together on failure before commit. A failure reported after commit is an uncertain reply, not proof of rollback. Existing single-chunk `append` remains compatible; it is not a replacement for this transaction.

For a retry, either every requested immutable row exists or none exists. A mixed existing/new group is `Conflict`; do not append its missing suffix. All-existing replay verifies exact canonical headers, payload bytes and hashes, consecutive global ordinals in request order, and membership in the proven original prefix. Changed order, changed bytes, a duplicate address or a row outside that prefix fails. A group retry may succeed after later groups without advancing or rebasing any prefix. Its receipt identifies the original group boundary; any reported prefix must clearly identify whether it is that boundary or the current durable prefix.

`group_sha256` is SHA-256 of `b"OmniuSong.recording-append-group.v1\0"`, the 64-byte ASCII descriptor SHA and each 64-byte ASCII chunk SHA in request order. Fixed-width digests make the sequence unambiguous. It is deterministic content identity. An all-old generic replay does **not** prove those rows were originally committed together: the performance checkpoint's owned semantics supply that assurance for this writer. No unordered hash set, partial replay or reconstructed successful suffix is acceptable.

Fault hooks must allow failures after a chosen member insert, before commit and after commit, as well as existing seal boundaries. Tests must distinguish rollback from committed-but-unacknowledged outcomes. No hook runs on the audio callback.

## Exact configuration and six streams

`PerformanceConfig` contains `take_id`, `info: TapInfo`, `gui_clock: LiveClock`, `output_epoch: ClockEpochId` and `[PerformanceSource; 4]`. Each `PerformanceSource` contains the exact `LiveSourceBinding` and a stable `EndpointId`. Validate a fresh initial scope with a cleared session; first-packet capture does not support arming a previously sounding renderer. This profile binds the actual prepared source handles/kinds 1–4 and connection generation 1. A later source set requires a new explicit profile, not inferred endpoints.

`create_descriptor(&PerformanceConfig) -> Result<TakeDescriptor>` validates the whole configuration before any stage creation. Store strict `{version: 1, config: ...}` provenance under the owned key `omniusong.performance.v1`. This retains the complete original GUI clock representation. Do not infer performance data from unrelated provenance keys, filenames or arbitrary IDs.

| Stream | Source position | Clock | Contents |
| --- | --- | --- | --- |
| `audio` | Audio frames, initially 0 | Render-frame domain, declared output epoch, rate/1 | Interleaved F32LE, actual tap rate/channels, logical post-mix bus |
| `gui-1` | Event ordinals, initially 0 | Exact GUI domain/epoch and rational | Applied ComputerKeys originals |
| `gui-2` | Event ordinals, initially 0 | Same actual GUI clock | Applied PointerKeys originals |
| `gui-3` | Event ordinals, initially 0 | Same actual GUI clock | Applied Pads originals |
| `gui-4` | Event ordinals, initially 0 | Same actual GUI clock | Applied AccessiblePulse originals |
| `control` | Event ordinals, initially 0 | Render-frame domain, declared output epoch, rate/1 | Begin, full dequeued batches/dispositions, clears, automatic transitions, End and checkpoints |

The audio/control endpoint is deterministic and bounded, derived with a domain-separated hash of take identity rather than appending text to a potentially 96-byte ID. GUI endpoints must be unique and distinct from the control endpoint. Use a fixed bounded render-domain identifier such as `omniusong.render-frames`; its epoch is the explicit configuration epoch. Sample rate, channels and scope must match every tap Begin. Initial source positions remain zero; Panic changes the live generation but does not reset the take's audio or event positions.

The GUI `LiveClock` accepts a denominator up to 10^12, whereas journal `HostClock` accepts at most 10^9. Compute `gcd(numerator, denominator)` in integers and divide both sides exactly. Require the reduced numerator ≤10^12 and denominator ≤10^9 and core-valid domain/epoch IDs. Reject an unrepresentable clock **before** creating the journal. Do not round the ratio, rescale ticks, convert ticks through float, substitute nanoseconds or guess an epoch. Preserve the original ratio in provenance even when reduced in stream descriptors. Applied GUI event timestamps are the original `u64` observed ticks. The separately recorded applied frame is scheduling evidence; `CaptureTime.alignment` stays `None` and `clock_anchors` stays empty.

## Owned record codec

The durable codec is distinct from the process-local `PerformancePacket` representation. Small tap DTOs may derive serde to avoid copying field definitions, but the journal accepts them only inside this versioned, strictly validated profile:

```rust
struct LocalRecord { version: u16, record: PerformanceRecord }

create_descriptor(config: &PerformanceConfig) -> Result<TakeDescriptor>;
encode_record(config: &PerformanceConfig, stream_index: usize,
              record: &LocalRecord, observed_ticks: u64,
              event_ordinal: u64) -> Result<Vec<u8>>;
decode_record(bytes: &[u8]) -> Result<LocalRecord>;
checkpoint_sha256(encoded_envelope: &[u8]) -> Result<String>;
```

`decode_record` validates both the generic capture envelope and owned payload. The scanner must also verify that the actual envelope session/source/clock/sequence/event identity matches the descriptor and current stream ordinal. A contextual decode helper returning both `Envelope<CapturedEvent>` and `LocalRecord` is preferable to reparsing or dropping those fields.

The owned enum contains:

- `Begin { callback_index, begin: TapBegin }`.
- `Batch { callback_index, packet_ordinal, batch: LiveBatch, disposition }`, preserving **all** fixed batch fields, including lease token, invalid/rejected lengths and unused slots. Rejected or discarded originals are evidence, not valid applied messages.
- `Clear { callback_index, clear: TapClear }` and `Transition { callback_index, transition: TapTransition }`.
- `End { callback_index, end_frame, scope_after, applied_sequence, active_voices, release_tail_voices, clear_count, transition_count, first_fault }`.
- `AppliedEvent { callback_index, batch_ordinal, event_index, scope, batch_sequence, applied_frame, original: LiveEvent }`.
- `Checkpoint { checkpoint: PerformanceCheckpoint }`.

All owned structs/variants reject unknown fields and unsupported versions. Unit variants must use strict empty-struct deserialization where ordinary serde tagged-unit handling would ignore extra fields. The raw generic extension maps remain available for unrelated profiles, but this owned namespace is parsed as an exact versioned contract.

Each durable record is inside a normal `Envelope<CapturedEvent>` with `RawPacket::Local`, `MusicalEvent::Uninterpreted`, no channel and no alignment. `group: 0` is a structural local placeholder required by the current generic address DTO, explicitly not a MIDI group/channel claim. The owned `LocalRecord` is the sole value under `CapturedEvent.extensions["omniusong.performance.v1"]`. Use deterministic domain-separated message/event identities and the exact per-stream ordinal, so retry reconstructs identical bytes. Do not generate fresh random event IDs during serialization or group retry.

The full batch is retained on `control`. Only successfully applied, descriptor-bound events additionally produce GUI-stream `AppliedEvent` records. Each such record links back to exactly one event index in its original applied batch. No record is created on a GUI stream for a rejected/discarded packet, a wrong endpoint/generation, a malformed array or an empty lease batch. The control record remains exact even when its contents cannot be interpreted as a valid source event. Queued parent-admitted batches never dequeued by the renderer are not claimed as captured originals.

Begin/Batch use the callback-start render frame. Clear/Transition retain their actual render frame; End uses its exclusive end. A checkpoint uses its audio end. Raw chunk observed bounds are the minimum/maximum exact timestamps of its records; this is needed because original GUI event order is not a clock conversion and split control observations may not be emitted in timestamp order. Event ordinals retain sequence independently of timestamp bounds. No synthetic human NoteOff is created for Pulse completion, envelope end, Panic, fault or interruption.

The existing 16-KiB envelope ceiling is retained. A full 128-transition `TapEnd` can exceed it, so each clear/transition is encoded separately and End carries bounded counts. Maximum-value 32-event batches and 63-member checkpoints must be tested against both the byte ceiling and the generic 2,048-node/depth-16 extension shape limit. Oversize is an explicit writer error; no record truncation, reduced velocity, omitted unused batch slot or raised generic limit is permitted to make it fit.

## Callback validation, buffering and commits

The reducer validates real `recording_live_pair` output. It enforces consecutive callback indices and packet ordinals, Begin/End scope continuity, exact rate/channels, four total batch records, at most 64 PCM packets and exact contiguous PCM coverage of the declared callback interval. PCM samples must be finite and unused packet slots zero. It retains and validates packed clear/transition counts and bounds. An Applied batch must be shape-valid and belong to the current scope; rejected/discarded batch contents are copied faithfully without indexing invalid lengths. End must match the accumulated callback, voice-count bounds and permitted generation change.

Before the complete End is validated, no part of that callback joins a complete performance checkpoint. A fault-bearing End can be completely retained, but it is interrupted evidence and cannot authorize healthy Finish. `FaultPending` retains the actual clear frame and must not be replaced by an invented cause or a later timestamp. Healthy Panic may continue the same take only through the tap's existing callback acknowledgment and scope transition; source-clock high-water marks are not reset.

Use bounded accumulators, never a performance-length vector. The adopted ceilings are:

| Item | Bound |
| --- | ---: |
| Actual callback transfer | Existing 70 packets / 16,384 frames / 8 channels |
| Encoded pending callback, including chunk headers | 2 MiB |
| One atomic group, including all canonical headers and checkpoint | 64 chunks / 8 MiB |
| PCM chunk payload | 1 MiB |
| Raw chunk payload | 64 KiB |
| Capture envelope / singleton checkpoint envelope | 16 KiB |
| Complete callbacks buffered before flush | 64 |
| Produced audio between ordinary flushes | Approximately 250 ms, rounded in integer frames |

A maximum supported callback must fit the callback/group limits in a deterministic worst-case codec test. Before adding a complete callback that would exceed a group limit, flush the prior complete callbacks and then add it to a fresh group. A single callback is never split between checkpoint transactions. At a size/count/time threshold, finalize chunk packing and append the checkpoint as one dedicated final control chunk. Time is measured from produced frames, not a wall-clock sleep; tiny callbacks or event storms still trigger the independent byte/count limits. Empty stream contributions do not create empty chunks. No upper duration, 120-second playback cap, 64-MiB object cap, 30-MiB bridge cap or 4,096-event cap applies to the original writer.

For each complete callback, PCM is encoded bit-exact F32LE and may be concatenated with neighboring PCM from the same stream. Raw records use the existing four-byte little-endian length-prefix codec. Do not reinterpret control frame counts as source event ordinals. All six prefixes participate in the checkpoint, including unchanged empty GUI streams.

An append failure is sticky. Retain the exact pending group request and its identities; after an uncertain post-commit error, `retry_pending` may authenticate/retry only that same request, never mint a new group or continue capture past it. Successful retry can clarify which original prefix committed; it does not erase the first failure or reopen packet admission. The later supervisor must stop audio admission and retain the original stage. This packet does not promise an interruptible blocked filesystem call or a cross-volume durability guarantee merely because SQLite uses FULL synchronization.

On `close_interrupted`, commit the complete pending callbacks first. If storage and packet validation have not failed, append the bounded, already-validated current Begin/Batch/PCM prefix as an **unselected tail group without a checkpoint**. This preserves fragments that reached the writer without manufacturing the missing End, audio or events. Their source records remain exact, but they do not advance the complete performance checkpoint. If a SQL outcome is ambiguous or packet validation has failed, issue no new tail request: retain the stage and report the buffered/queue tail as unknown. Capture faults alone do not prohibit bounded draining of well-formed originals; writer corruption/storage failure does. Neither path seals normally or resumes the take.

## Checkpoint and consistent-prefix validation

`PerformanceCheckpoint` has `group_sequence: u64`, `previous_checkpoint_sha256: Option<String>`, `first_callback_index`, `last_callback_index`, `audio_end`, `scope_after`, `chunks: Vec<PerformanceChunkIdentity>` and `prefixes: Vec<StreamPrefix>`. Each member identity contains `stream_id`, `sequence`, `sha256`. Group sequence begins at 1; the first predecessor is absent. Members list every chunk since the previous checkpoint in exact global order, excluding the checkpoint's own chunk. There are at most 63 members. Prefixes contain all six streams in descriptor order, including the predicted final control sequence/event end after this singleton checkpoint record.

`checkpoint_sha256` is SHA-256 of `b"OmniuSong.performance-checkpoint.v1\0"` followed by the exact validated checkpoint envelope bytes. This becomes the next checkpoint's predecessor. It does not recursively include its own digest. The ordinary chunk digest still authenticates its header and payload, and the generic journal index authenticates the complete ordered chunk sequence.

The scanner must establish all of the following before selecting a checkpoint:

1. Generic chunk/header/payload/index validation succeeds through that ordinal.
2. Every member after the preceding checkpoint is present once, in order, with matching stream/sequence/digest. No chunk or event is silently skipped.
3. Audio, source events and control observations reproduce complete valid callback groups. GUI applied-event links match the original full applied batches exactly, without missing or duplicate observations. Rejected/discarded originals are not promoted to GUI events.
4. The callback index range, exclusive audio end, ending scope and all six prefixes match the scanner's independently accumulated state; no audio-only or event-only extension is accepted.
5. The checkpoint is the sole record in its final control chunk; its predecessor and group sequence continue the same descriptor-bound chain.

A cryptographically self-consistent but semantically contradictory checkpoint is rejected. The scanner chooses the **last fully validated checkpoint before the first generic or semantic failure**. It never skips a broken group to find a later plausible checkpoint. No checkpoint means no recoverable complete performance; earlier raw rows remain original evidence. Scan chunk-by-chunk and keep only bounded current-group/callback state, the last accepted checkpoint and bounded diagnostics. Do not collect the whole take in RAM.

## Explicit recovery, unchanged originals

The recording-layer additions are a generic `RecoveryPrefix { chunk_count, chunk_index_sha256 }`, `CaptureReader::read_ordinal`, and `CaptureReader::recover_prefix(self, operation_id, &RecoveryPrefix)`. Read access is bounded by the existing inspected verified chunk prefix and uses its held read transaction and exclusive owner guard. The recovery method permits a selected prefix shorter than the longest generic prefix; it verifies the selected index hash and retained interruption index, rechecks original rows inside the seal transaction and compares the selection on replay. All later rows—including valid chunks from an incomplete group and corrupt tails—remain untouched in the physical journal.

The engine supplies `inspect_performance(&CaptureReader) -> Result<PerformanceInspection>` and `recover_performance(CaptureReader, &PerformanceInspection, operation_id)`. The inspection binds the complete generic `JournalInspection`, owned descriptor interpretation, last valid checkpoint/hash, selected `RecoveryPrefix`, six selected prefixes and bounded problems/unknown-tail state. The canonical review token binds **all** fields. Recovery recomputes that token and review, rather than trusting a caller-provided hash attached to changed fields.

On first recovery, the current complete physical/normalized source inspection must match the reviewed source before takeover. Transfer the existing owner guard directly into the recovery writer; never unlock and reacquire between review and mutation. Derive the immutable seal-operation ID from the caller's operation ID and exact reviewed source/checkpoint. Reopening a completed recovery uses the same derived identity, selected count/index and unchanged original-row digest; physical bytes may differ because sealing created controlled metadata. A changed review, different operation, changed original tail or changed selection cannot replay that manifest.

Existing generic `CaptureReader::recover`, `Journal::seal` and `Journal::seal_recovered` must not silently select the longest prefix for a descriptor containing the owned `omniusong.performance.v1` marker. Reject that convenience path and require the performance-specific verifier plus explicit-prefix primitive. Generic read-only inspection remains useful; opaque provenance outside the owned marker is unaffected. This prevents the current generic retained-capture UI from accidentally sealing an audio prefix whose corresponding performance checkpoint is missing. It is an API correctness boundary, not a sandbox against arbitrary in-process Rust callers.

An unsealed/interrupted performance always has an unknown tail. Do not infer missing duration from GUI timestamps, invent silence or gap ranges, resume capture into the old take, or rewrite source rows. A recovered manifest selects the consistent checkpoint while retaining the complete original file and interruption receipt. Existing sealed-object ingestion and complete project archives can then preserve the whole journal when attachment is explicitly implemented. Recovery itself does not attach anything.

Normal Finish remains unavailable in this packet. A future Finish must close new admission, fence every worker-accepted batch to an actual callback disposition, record the callback-owned finish cut/tail policy, successfully stop output, acquire real renderer/error-closure destruction acknowledgments, drain the tap, commit the final checkpoint and then seal under a stable operation ID. Neither a clean queue, a successful writer flush, a synthetic `LiveErrorGuard` nor a caller boolean substitutes for those proofs.

## Required device-free acceptance

- Atomic groups: 1/64-chunk bounds, exact header-plus-payload budget, multiple chunks per stream, six prefix updates, duplicate/order conflicts, all-new/all-old/mixed retries, later-history replay, last-member failure rollback, pre-commit failure, committed-but-lost-reply replay and recovered/sealed state guards.
- Codec: exact ticks above 2^53 and near `u64::MAX`, reducible and unrepresentable rational clocks, core-invalid clock IDs, distinct source endpoints, all exact `u16` attack/release values, malformed original batch retention, strict versions/unknown fields, worst-case batch/checkpoint size and split 128-transition End. No fabricated MIDI or calibration.
- Actual tap integration: finite bit-exact F32 reconstruction, every applied/empty/rejected/discarded disposition, repeated voices, source sustain, Pulse/automatic transitions, healthy Panic continuation, exact-end lease clear, pending-fault evidence, incomplete callback and producer/receiver failure. Validate real production renderer output rather than a manufactured event list.
- Incremental behavior: stream past 30 MiB and 4,096 events with bounded memory, many tiny callbacks, maximum callbacks/event storms, flush thresholds and callback-boundary group splitting. This is software streaming evidence, not the later measured 30-minute device gate.
- Recovery: live-owner refusal, unchanged read-only bytes/sidecars, corrupt member/End/checkpoint, valid audio followed by corrupt control, rehashed contradictory source links, previous-checkpoint mismatch, selected-prefix rollback, stale review, changed operation/tail replay rejection and all bad/unselected rows retained. Check a recovered snapshot/archive keeps the full physical original when the responsible integration lane supplies that test.
- Lifecycle boundary: no normal seal API, explicit recovered/unknown-tail status, no fake native quiescence, no catalog mutation or implicit attachment, and no claim that parent-admitted-but-never-dequeued batches were captured.

Independent source review and final central test/build evidence are required before calling this writer packet qualified. Worker/UI integration, normal Finish, project publication, editable derivation and the full MIDI 2/MPE/plugin/Windows/reliability goals remain open.
