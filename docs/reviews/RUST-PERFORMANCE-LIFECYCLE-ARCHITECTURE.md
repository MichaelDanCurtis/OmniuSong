# Saved-performance lifecycle: independent architecture proposal

October 20, 2026. **Proposal and source review, not implementation or runtime acceptance.** The coordinator confirms that goal attachment `ac152ac5-7a5f-4aa0-bf71-edbd62deafb1` is synchronized to [GOAL.md](../GOAL.md); its new requirement is a beautiful, easy, crisp and functional interface, also tracked by H09. This investigation opens no native input or output device. The authorized eventual capture is the existing procedural keyboard/pad **output bus**, never a microphone, native input stream or software-loopback input.

The preceding [tap](RUST-PERFORMANCE-TAP-SOURCE-REVIEW.md) and [writer](RUST-PERFORMANCE-WRITER-SOURCE-REVIEW.md) qualify library paths and interrupted recovery. They do not yet implement a saved-performance worker lifecycle. The new prerequisite packet agreed with the coordinator is callback Finish evidence plus an explicitly versioned durable decoder/scanner. The coordinator's [frozen implementation contract](../contracts/NATIVE-PERFORMANCE-FINISH.md) adopts that boundary and the decisions below. Real worker ownership, private normal-seal authority, UI, attachment and native output acceptance remain required subsequent integration; this proposal must not be used to close those gates.

## Baseline integration points and gaps

- `engine/src/worker.rs::LiveOutput::start` creates `live_pair`, an actual output stream and one real `LiveErrorGuard` captured by its error closure. It has no performance stage, tap consumer or writer. The performance path must use `recording_live_pair` and the same output renderer, not reconstruct an event list afterward.
- `LiveOutput::stop` already pauses and drops the actual `cpal::Stream`, then checks separate renderer and error-closure destruction acknowledgments. Failure retains callback-owned backing and retires the child. Keep that conservative ownership policy. Successful Stop alone does not prove queued accepted batches were disposed or captured.
- `live.rs::LiveAdmission` owns the only producer and exact accepted sequence. Panic closes ordinary admission temporarily and resets sequence only after callback acknowledgment. `LiveRenderer` applies at most four batches per callback and alone owns voices. It currently has no irreversible Finish admission state or terminal recording cut.
- `LiveRenderer::drop` drops its complete recording wrapper and ingress consumer before publishing renderer destruction. The tap has a distinct producer-destroyed acknowledgment; admission retains its backing off callback. Preserve both proofs and retention order.
- `PerformanceWriter` incrementally commits complete six-stream checkpoints and can retain interrupted partial tails. It deliberately has no normal Finish API. `Capture::finish(bool)` is for the separate one-audio-stream input capture path; its caller boolean, random seal ID and wrapper are unsuitable for performance completion and must not be reused.
- Engine control uses one serialized private socket. Parent urgent lanes prevent stale local admission and preserve terminal results, but do not create a second IPC channel or instantaneous cancellation. Worker handlers that wait on callbacks or SQL can delay Stop/EOF processing. New lifecycle progress must expose that limitation rather than call a synchronous wait cancellable.

## Recommended first Finish policy

Adopt one explicit policy: **apply all worker-accepted batches through the captured fence, render the complete final native callback, then hard-cut every remaining voice and sustain state at that callback's end**. No natural release tail is inferred or padded, and no human NoteOff or release velocity is invented. Record the pre-cut active/release-tail counts and the explicit cut policy; original source identities remain in retained batches and source records. A future release-tail policy is a separately versioned choice, not a silent reinterpretation.

Closing admission does not immediately cut the current callback. If callbacks are already rendering, the request is observed at a later callback boundary. The final callback may be later still while the bounded ingress queue drains. This preserves the final accepted attack for its actual complete callback, including the truthful possibility of very short audible material. A Finish with no worker-accepted batch still records a complete callback and a positive audio extent. Reject malformed/overflowed/faulted final data rather than invent a zero-length successful take.

The fence describes **worker acceptance**, not parent enqueue, GUI observation or physical sound. Healthy parent Finish closes its local note gate immediately, resolves the in-flight exchange and drains the finite queue of already parent-enqueued batches through worker acceptance before requesting the immutable worker fence. Preserve those earlier GUI intentions; do not discard them merely because Finish arrived before their IPC exchange. Keep their original lease tokens and event clocks. Only interrupted Stop, fault or EOF may preempt that drain and explicitly report an unforwarded/uncaptured count. A rejected or uncertain exchange cannot be silently converted into a healthy captured original or a successful Finish.

## Proposed callback APIs and invariants

The names below match the coordinator's frozen shared types; the coordinator owns those types. The progress-query signature remains an implementation interface to verify. All callback-facing fields are fixed-size `Copy` values, including the operation identity. No String, serde, heap allocation, mutex, file access or waiting belongs in these methods.

```rust
struct LiveFinishFence {
    id: LiveId,
    scope: LiveScope,
    accepted_sequence: Option<u64>,
}

enum LiveFinishPolicy { CutAtCompleteCallbackEnd }

struct TapFinish {
    fence: LiveFinishFence,
    callback_index: u64,
    cut_frame: u64,
    voices_cut: u16,
    release_tails_cut: u16,
    policy: LiveFinishPolicy,
}

// Captures its own accepted watermark. The caller cannot provide one.
LiveAdmission::begin_finish(
    &mut self, scope: LiveScope, finish_id: LiveId
) -> Result<LiveFinishFence, LiveSubmitError>;

// Bounded observation of callback-owned acknowledgment, not sealing authority.
LiveAdmission::finish_ack(&self)
    -> Result<Option<TapFinish>, LiveSubmitError>;

PerformancePacketKind::Finish(TapFinish);
```

`begin_finish` requires the current recording scope, a nonzero stable ID, no pending Panic and a healthy, unexpired state. It captures the sole admission owner's `accepted` value and closes batch/Panic admission permanently before publishing the fixed request. Repeating the same ID/scope returns the exact fence; a different ID, stale scope or attempt to reopen admission rejects without rebasing. Finish is not supported on an ordinary audition-only pair.

The callback observes the immutable request at its boundary. It keeps the existing **four total batch dispositions per callback**, including discarded/rejected/empty packets. A healthy terminal Finish requires that the current scope's accepted sequence prefix has actually been Applied through the captured watermark, the ingress consumer is empty, and no first fault or pending fault publication exists. Missing/reordered/stale accepted material cannot be silently discarded to produce a healthy Finish. Prior acknowledged Panic generations and their discarded batches remain original evidence; Finish cannot race a new generation because pending Panic is rejected and later Panic admission stays closed.

The agreed separate `PerformancePacketKind::Finish` appears **after all PCM and before End**, reserving **71 maximum slots for each lifecycle-enabled callback**: Begin + at most four batch dispositions + at most 64 PCM packets + optional Finish + End. Ring capacity remains 256 with the existing fixed-memory ceiling rechecked at compile time. Reserving the complete callback precedes all voice mutation and event application; no partial-event capture fallback is allowed.

On the final callback, render the complete PCM extent, capture pre-cut counts, clear voices/sustain at `cut_frame`, emit Finish, then emit End with zero active/release-tail counts. The Finish callback index, scope and cut must match that complete End and its accepted/applied fence. A lone Finish packet, failed End push, tap overflow or partial tail never publishes a completion acknowledgment. Publish the acknowledgment only after End has been accepted into the tap. The writer and scanner require exactly this order, one Finish at most and no later callback group after the terminal Finish.

After the cut, subsequent native output buffers are explicitly silent and produce no further tap groups. The recorded frame endpoint stays fixed at `cut_frame`; a later native callback count must not extend it. Any post-cut device error still invalidates future normal completion. Do not label these unrecorded silent native buffers part of the retained take or claim that the physical device finished playing its internal buffer.

### Lease and fault races

The present one-second GUI lease cannot simply be renewed by Finish or ignored globally. There must be one atomic transition from a still-healthy, **not-yet-expired** admission lease to a bounded finishing mode. An expired or concurrently faulting state cannot be resurrected. This is a one-time control-owner transition after closing note admission; it is not a queued batch credit and cannot be renewed by Status, queue draining or repeated Finish.

Finishing mode has a bounded remaining callback budget derived from the frozen queue/fence, plus a real worker wall deadline. With a 16-batch queue and four dispositions per callback, no more than four drain callbacks are needed once the request is observed. The adopted contract allows a single checked frame deadline extension to **the maximum of the existing still-live deadline and observed frame plus six maximum callbacks**, including boundary slack. This never shortens a valid high-rate one-second lease. It uses the same deadline CAS as expiry and never clears EXPIRED. The native maximum of 16,384 frames at 8 kHz means a universal two-second whole-fence timeout does not cover every valid maximum-size sequence. Select and document an explicit finite worker timeout that covers the supported configuration, or reject that configuration before recording starts. Do not extend it repeatedly on progress. The worker must retire or retain interrupted material on timeout, never infer callback completion from elapsed time.

Use explicit `Finishing` and `FinishedCut` progress states, with no lease credit or fault in either healthy state. `FinishedCut` reports the immutable positive cut frame, zero voices and the equal accepted/applied fence. Acquire the acknowledgment before sampling or deriving terminal fields; mixing a fresh terminal state with pre-acknowledgment voice counts is invalid. These states describe library evidence only in this prerequisite. Version the actual worker wire profile before new lifecycle commands or states become reachable there; ordinary audition cannot enter them.

First faults remain sticky. `FaultPending` remains an interruption without a guessed cause. A fault, Stop or cancellation before the complete Finish acknowledgment supersedes healthy completion; preserve whatever complete/partial originals reached the writer. A fault after the acknowledgment but before real native teardown still vetoes normal sealing. Recheck the actual fault state after error-closure destruction: a callback Finish acknowledgment is only evidence of a cut, not evidence that no later error occurred.

## Versioned durable evidence

Adopt `omniusong.performance.v2` as the new owned namespace/profile. Keep v1 descriptor creation, exact envelopes, reading and explicit recovery intact. The new dispatcher must recognize exactly one supported owned profile; reject duplicate v1/v2 markers, malformed or null owned values and mixed-profile chunks. Old strict v1 readers fail visibly on v2, rather than interpreting a new Finish as an unknown extension. Never append v2 terminal evidence to a take created as v1.

Keep the SQLite journal, descriptor and generic manifest format versions unchanged unless implementation reveals a necessary storage change. This is owned performance-format evolution. Add the first-class v2 Finish control record with the exact callback fence/cut/policy, followed by the unchanged semantic End and final checkpoint. The checkpoint includes the Finish-containing control chunk in its normal ordered member list and all six predicted prefixes. A valid final checkpoint with Finish is still **unsealed interrupted evidence** until the later actual lifecycle proof exists.

The v2 writer/scanner must distinguish ordinary complete checkpoints from a terminal Finish checkpoint. Validate the source scope, accepted sequence accounting, callback number, exact end frame, one terminal record, pre/post-cut count relationships and absence of subsequent groups. Existing faults/pending faults cannot be hidden by a terminal record. Rehashed contradictions stop recovery at the preceding valid checkpoint; no skipped group or inferred silence.

Extend generic `Journal::seal`/generic recovery refusal to both owned markers, including malformed presence. V1 remains recoverable with unknown tail and has no retroactive normal-Finish eligibility. A v2 take with a valid Finish but no committed normal manifest after a crash still requires explicit interrupted recovery and keeps unknown tail. Persisted callback evidence cannot retroactively prove destruction of a vanished native error closure.

## Required real worker lifecycle

The eventual `PerformanceSession` should own one actual output stream, one `LiveAdmission`, one immutable stage/config identity, one tap control handle and one persistent off-callback writer/drainer. The writer alone owns `PerformanceTapOwner`; admission retains the tap backing until renderer destruction. Dropping the receiver must not free the ring on the callback. Writer failure cancels/faults the tap and closes live admission permanently; it cannot silently downgrade recording to unrecorded audition.

The worker creates and validates the descriptor/stage before allowing recorded sound. Its config binds actual output format, the full supplied GUI clock rational, fresh output epoch and four prepared local source endpoints. Once a stage exists, every start/progress/failure result retains that identity and path so a failed start is recoverable. Native stream construction/play failure retains the journal, including an empty one; it does not pretend the recording never existed. The recording UI must use a distinct output-only command and label, never route through `StartCapture`.

Proposed nonblocking state progression:

| State | Owner action and required evidence |
| --- | --- |
| Starting | Validate config; create stage and writer; construct the actual output stream with the real renderer/error closures. Publish stable identity before reporting ready. |
| Recording | Accept bounded live batches; persistent writer drains tap concurrently and exposes separate complete/durable positions. |
| FinishRequested | Close local/worker admission, store exact ID/fence, reply with pending state promptly; keep parent heartbeat and Stop routing alive. |
| CallbackCut | Observe exact callback Finish acknowledgment; no normal-seal authority yet. |
| StoppingNative | Call the actual stream's pause, then drop it only on success. Preserve/leak bounded callback ownership and retire the child on failed stop or unproven lifetime, as existing LiveOutput does. |
| Quiesced | Acquire actual renderer, error-closure and tap-producer destruction flags. Recheck first faults and tap cancellation/health. No synthetic guard or caller boolean is admissible. |
| Drained | Single writer drains until producer destruction **and** empty tap, verifies no partial callback remains, and commits the exact terminal checkpoint. Complete/durable callback/frame/index/fence must agree. |
| Sealing | Private lifecycle owner grants one normal-seal authority for the exact immutable finish request, descriptor, terminal checkpoint and final index. |
| Sealed | Return an authenticated manifest and exact operation identity. Attachment is a separate durable project operation with existing conflict/unknown-outcome handling. |
| Interrupted | Stop, EOF, cancellation, device/tap/validation/storage error or failed lifetime proof: retain the original and any complete prefix; no automatic normal seal or resume. |

A Finish request must not synchronously join the writer or wait for a slow SQL transaction inside the serialized IPC handler. Return a correlated pending result and drive progress through worker-owned state. Keep heartbeat/urgent Stop usable while waiting. Actual platform pause/drop itself may block; the existing watchdog/retirement boundary must stay explicit and bounded. If the implementation cannot keep the handler responsive during that OS call, state that limitation and preserve originals on retirement—do not describe Stop as instantaneous.

## Private normal-seal authority and replay

Normal sealing requires an internal, non-cloneable authority minted by the real worker lifecycle, not a public `finished: bool`, supplied `TapSnapshot`, manually instantiated `LiveErrorGuard` or arbitrary caller-provided fence. Its constructor validates the immutable Finish request against callback acknowledgment, real teardown and the drained writer checkpoint. Device-free tests may test the verifier with clearly identified fake adapters, but those tests cannot qualify native destruction proof.

The recorder primitive may expose a narrow checked full-prefix normal seal, analogous to explicit-prefix recovery, while engine code owns the lifecycle semantics. Keep generic normal seal blocked for owned performance markers. Seal exactly the final full index—no hidden selected tail—and use a stable operation ID supplied before Finish, with a derived request hash binding the descriptor, policy, captured fence and terminal checkpoint. The normal manifest has no interrupted receipt; recovered manifests always retain one with unknown tail. Exact replay returns the original manifest, conflicting IDs/requests reject, and a lost commit acknowledgment never causes another capture or new ending.

Define the irreversible boundary explicitly. Stop/EOF/cancellation before the worker grants the one-use normal-seal authority wins and leaves unsealed material. After the seal transaction has been authorized/in flight, a racing Stop cannot promise that no manifest committed. Output is already quiesced; retain the exact finish identity and report/replay the committed or unknown outcome. Use a serialized authority/cancel decision or an atomic gate with explicit linearization; never claim both successful cancellation and a normal sealed result for the same request. An asynchronous error that can still arrive means native quiescence was not proven, so authority must not yet exist.

On parent loss, close admission, attempt safe stop and bounded drain, and retain unsealed originals unless a previously authorized normal seal has already committed. No new success is fabricated from a clean queue or a persisted Finish record. The independent watchdog may terminate a stalled worker, leaving a recoverable consistent prefix and an unknown tail. The next process must inspect, not resume, that stage.

## Acceptance plan and concrete risks

The first prerequisite packet should have independent device-free tests on the production renderer/tap/writer, including:

1. Queue full of accepted packets, zero accepted packets, one/four/sixteen queued packets, accepted `None`, empty lease packets, exact final sequence and no post-fence admission. Keep all four-disposition and callback allocation/deallocation limits.
2. Finish observed before, during and after a callback; final full PCM extent; pre-cut voice/release-tail counts; exact cut at End; no synthetic human releases; later native buffers silent without extending the take.
3. Pending Panic rejection, stale scope/ID, repeated same request, conflicting ID, sequence exhaustion, lease expiry exactly at request/admission boundary and repeated Finish/Status unable to renew it. Abort/fault first-cause publication and `FaultPending` must never produce healthy completion.
4. Tap reservation failure before application, missing/reordered/duplicate Finish, Finish without End, fault between Finish and End, faulty/tampered End, and acknowledgment only after successful complete End. Acknowledgment followed by error must remain ineligible for normal seal.
5. V1 fixture byte compatibility, v2 strict decode, dual/malformed markers, rehashed finish/fence/checkpoint contradictions, partial terminal tails and terminal-record recovery remaining unknown without normal authority.

The real worker phase additionally needs private paired-process tests for Finish pending while Stop/EOF arrives, slow/failed writer, late reply, exact stable-ID replay, mandatory retention of failure identity and no second recording generation after unproven native teardown. Tests must prove actual lifetime relationships, not pass a forged success boolean. Normal-seal transaction before/after-commit faults must distinguish uncommitted, committed-but-unacknowledged and recovered outcomes.

Only later native output qualification can establish the actual backend's pause/drop/closure behavior. It must exercise the same installed binary and persistent workflow, using explicit output-only authorization. No native input is needed or allowed in this investigation. Software note response, physical latency, 30-minute capture, MIDI 2/MPE, plugins and Windows remain separate release requirements.

The main implementation risks requiring disposition are the atomic lease-to-finishing transition; bounded final-fence timing for all allowed callback configurations; serialized-worker responsiveness during native/SQL waits; fixed recording endpoint versus later native silent buffers; complete accepted-prefix accounting; version-compatible recovery; and an honest cancellation/normal-seal linearization point. None is satisfied merely by adding a Finish button or a new record enum.

## Initial implementation source review — October 20, 2026

The new library implementation now has the one-shot fence/acknowledgment, 71-slot callback reservation, complete final PCM followed by Finish and End, and explicit v2 writer/scanner dispatch described above. The baseline gap list records the starting point; these source changes do not yet add the native worker, normal sealing or UI. No build or test result is asserted by this initial review.

One concrete publication race was identified and corrected during review: `LiveAdmission::progress` sampled mutable voice counts before acquiring a newly published Finish acknowledgment. That could combine a fresh terminal state with old nonzero voices. The revised implementation acquires the immutable acknowledgment first and derives its terminal frame/applied fence and zero counts from it. `Finishing` and `FinishedCut` now have explicit strict DTO validity. A later fault still takes precedence and `finish_ack` rejects it. The existing audition worker cannot reach these states; introducing recorded worker commands requires an explicit wire-profile change.

The source audit found no further blocker in the one-time deadline CAS against sticky expiry; request snapshot before the callback boundary; four-packet drain; full-callback reservation before event application; Finish-before-End acknowledgment; or destruction of the producer wrapper before renderer acknowledgment. The writer/scanner reject v1 Finish, terminal ordering/fence/scope contradictions and post-cut callbacks. A partial or forged terminal group leaves the earlier checkpoint eligible for interrupted recovery and preserves all original rows. These are source conclusions awaiting executable gates, not proof of scheduling, native quiescence or normal completion.

Eight independent integration tests are saved in `crates/omniusong-engine/tests/performance_finish_review.rs`. They exercise the production renderer and v2 writer, all sixteen queued batches, exact source originals and F32 bytes, empty fence, retained Finish without End, five forged terminal-packet variants, v1 refusal, post-cut admission, four generically valid rehashed terminal contradictions, and late-device-fault acknowledgment veto. The additional allocator test measures only the calling test thread around an actual maximum-size final callback (64 held voices, four batches, eight channels, 16,384 frames and all 71 tap packets), then the following silent callback. All setup, requests, output buffers and packet inspection are outside the measured sections; allocation and deallocation counts must both remain zero. The test source is handed to the coordinator for formatting, compilation and execution; this review lane ran no Cargo command or device access.
