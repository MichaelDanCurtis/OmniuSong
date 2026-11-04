# Saved keyboard/pad performance: independent architecture recommendation

October 20, 2026. Source inspection against `e32ba95b0bc24be09b918a5b434355f9f2964ae2`, with coordinator and engine/UI investigator review. This document proposes the next implementation; it is not recording, device or UI acceptance evidence. The packaged **09789029…** arrangement application remains unchanged and its actual arrangement walkthrough is pending an external-drive SQLite open. The earlier 562-entry source gate does not test this proposed capture path.

The recommended first code packet is a device-free recording tap on the **actual `LiveRenderer`**, followed by a multi-stream incremental writer, explicit worker/UI capture lifecycle, retained-stage recovery and project attachment. Editable notes remain a required subsequent, explicitly derived publication. Neither an offline fixture that buffers all PCM nor a list collected after playing satisfies saved performance capture.

This is keyboard/pad **output-bus recording**. It opens no native input stream, including no virtual loopback input. It does not establish MIDI input, MIDI 2.0 transport, MPE, physical sound, latency, 30-minute reliability or Windows qualification. Those goal requirements remain open.

## Findings that determine the boundary

| Current source | Consequence |
| --- | --- |
| `engine/src/live.rs`: `apply` stages a complete batch before committing voices; `boundary` consumes at most four batches per native invocation. | Capture committed originals at this boundary. Parent admission and a coalesced applied watermark are insufficient evidence of which events sounded. Preserve disposition for packets that the callback discards or rejects. |
| `LiveEvent` retains full `u16` attack/release velocity, source/connection, note/voice ordinals, observed ticks, patch and Pulse duration. | Retain these exact fields, full live scope, batch sequence and event index. Repeated pitch is not an identity; Pulse completion is not a human key release. |
| `LiveOutput` discards `OutputCallbackInfo`; renderer output is a logical F32 mix converted to F32/I16/U16. | Name the recording **post-mix F32 before device conversion**. The callback frame axis is exact for this bus, not measured speaker time. Do not invent a host-clock anchor from GUI timestamps. |
| `recording::Journal` supports multiple immutable audio/raw streams, but `CaptureJournal` requires one zero-based F32 audio stream. | Use a new performance reducer around journal primitives. Do not repurpose the existing single-input reducer without an explicit new contract. |
| Raw chunks contain `Envelope<CapturedEvent>`; local `MusicalEvent::NoteOn/Off` currently require a MIDI channel. | Use `RawPacket::Local`, `MusicalEvent::Uninterpreted` and an owned, strictly validated local-performance extension. Do not fabricate MIDI channel zero, UMP or calibration. |
| `Journal::append` commits one stream chunk per transaction. | A later multi-stream writer needs an explicit consistent-prefix mechanism. Independent audio and event commits are not an atomic performance checkpoint. |
| Active raw-event `TakeClip`s have no implemented renderer; the saved-note renderer differs from live synthesis/envelopes. | Retained PCM is the initial exact logical-bus replay. A raw clip is read-only evidence. Derived editable notes cannot be labeled an identical rendering. |
| A typed take reference retains the complete sealed SQLite object through history/receipts/archive. | One authenticated PCM attachment can retain all original event streams. Opaque metadata that merely resembles a take ID must not become an inferred dependency. |

## First packet: callback tap interface

Keep existing `live_pair` unchanged. Add a new constructor in the live module:

```rust
pub fn recording_live_pair(
    scope: LiveScope, sample_rate: u32, channels: u16,
) -> Result<(LiveAdmission, LiveRenderer, PerformanceTapOwner), String>;
```

The new tap module owns its fixed packets, queue and non-realtime receiver. Root integration owns the renderer changes and fault mapping. This packet adds no `EngineCommand`, IPC shape, journal, device open, recording UI or automatic publication.

`TapInfo { scope, sample_rate, channels }` binds the initial live identity and output format. It deliberately does not pretend to contain the GUI clock: the existing `LiveOutput` owns that descriptor separately. Until the later worker binds the actual immutable `LiveClock` and `LiveSourceBinding`s, tap `observed_ticks` are opaque scope-bound values, not self-describing timestamps.

Proposed construction and producer boundary:

```rust
// Created off the audio thread. Producer and retention guard are crate-private.
tap_pair(info: TapInfo)
    -> Result<(PerformanceTapProducer, PerformanceTapOwner,
               PerformanceTapRetention), TapError>;

// Producer: callback-only after construction.
begin_callback(begin: TapBegin) -> Result<u64, TapError>;
push_batch(batch: LiveBatch, disposition: BatchDisposition) -> Result<(), TapError>;
push_pcm(pcm: TapPcm) -> Result<(), TapError>;
end_callback(end: TapEnd) -> Result<(), TapError>;
fault(kind: TapFaultKind, frame: u64);

// Sole receiver: off callback. Faults never prohibit draining retained packets.
PerformanceTapOwner::try_pop(&mut self) -> Result<Option<PerformancePacket>, TapError>;
PerformanceTapOwner::info(&self) -> TapInfo;
PerformanceTapOwner::control(&self) -> Arc<TapControl>;
```

All callback packets and nested values are fixed-size `Copy` data, without `String`, `Vec`, JSON, borrowed GUI state or allocation-owning fields. The exact field proposal is:

| Type | Fields / meaning |
| --- | --- |
| `PerformancePacket` | `callback_index: u64`, `ordinal: u16`, `kind`. Index starts at 1 and never wraps; ordinal starts at 0 for Begin and is contiguous. |
| `PerformancePacketKind` | `Begin(TapBegin)`, `Batch { batch: LiveBatch, disposition }`, `Pcm(TapPcm)`, `End(TapEnd)`. |
| `TapBegin` | `scope`, `start_frame: u64`, `frames: u16`, `sample_rate: u32`, `channels: u16`. First frame is zero; each next complete callback begins at the previous End. |
| `BatchDisposition` | `Applied`, `DiscardedStale`, `DiscardedPanic`, `Rejected { fault: LiveFaultKind }`. Retain the whole original fixed batch, including empty batches and unused slots. |
| `TapPcm` | `start_frame: u64`, `frames: u16`, `channels: u16`, `samples: [f32; 2048]`. At most 256 frames × 8 channels; finite interleaved valid prefix, zero unused cells. No fabricated nanosecond fields. |
| `TapEnd` | `end_frame`, `scope_after`, `applied_sequence: Option<u64>`, `active_voices`, `release_tail_voices`, `clears: [Option<TapClear>; 2]`, `transitions: [Option<TapTransition>; 128]`, `transition_len`, `first_fault: Option<LiveFault>`. |
| `TapClear` | `scope`, exact `frame`, reason `Panic { next_generation }`, `Fault { kind }` or `FaultPending`. This is an engine action clearing all voices/sustain in that scope, not a collection of invented human NoteOffs. |
| `TapTransition` | `scope`, `source`, `voice`, exact `frame`, kind `AutoRelease { release_velocity16 }` or `VoiceEnded`. Pulse versus patch-driven release is resolved against that voice's retained original event. |
| `TapFaultKind` | At least `Overflow`, `ReceiverAbandoned`, `MalformedPacket`, `CounterExhausted`, `Aborted`. Store the first cause/frame in fixed atomics. Subsequent symptoms cannot replace it. |

Begin validates the immutable base worker/session, rate/channels, contiguous frame range and checked end at or below `i64::MAX`. Scope follows the previous End. Applied batches require valid fixed shape and the current scope; the renderer remains responsible for musical validation and truth of the disposition. Rejected/discarded originals must survive even with invalid length, source, key or scope: copy them without slicing or interpreting unsafe indices. A second tap voice allocator would risk rejecting the evidence of the renderer's own rejection.

End validates packed arrays, bounds, coherent voice counts, unchanged base worker/session and the bounded panic-generation transition. PCM packets cover the entire Begin interval exactly, without overlap or holes. Automatic transitions are engine observations; they neither add GUI events nor modify the original batch. A normal callback has at most 64 resident voices, each with at most one automatic release and one end, hence the 128 bound. Explicit human releases and source sustain changes are already preserved in the batch. A Panic plus a fault in the same invocation fits two aggregate clear records.

`FaultPending` covers the existing atomic first-fault publication sentinel: the renderer may already know it must clear while the winning thread has not yet published the cause. Record that actual clear frame without spinning or inventing a kind; `End.first_fault` can contain the later resolved cause. Resolving the cause is not another clear and does not consume a third slot. A pending marker itself proves interruption. Control snapshot APIs may return Busy when a coherent snapshot is temporarily unavailable; callback execution never waits for one.

### Reservation, ordering and ownership

Use **one SPSC queue with 256 slots**. Before consuming any ingress or mutating voices, reserve by checking producer free slots for:

`2 + 4 + ceil(callback_frames / 256)`

That is Begin, up to four batch dispositions, PCM packets and End: **70 slots maximum** at 16,384 frames. No other producer can consume these slots; the consumer can only free additional slots. Validate the actual packet size at compilation, initially `<= 16 KiB`, so the ring allocation is `<= 4 MiB`. These are chosen ceilings, not performance measurements.

On insufficient space, latch recording overflow at the callback-start frame and silence without applying or consuming another event batch. Do not resume that take automatically after the writer catches up. Root should use a distinct live `RecordingTap` fault so the existing fail-closed voice and terminal paths retain the exact cause. The queue is still drainable. A partial Begin without End is incomplete evidence, not a complete callback or sealed take. A valid End is only a packet-group fence; a callback that records a lease/device fault still leaves an interrupted performance.

PCM is copied directly from the same finite post-mix F32 values supplied to output conversion. It is not rerendered later. I16/U16 device quantization, driver buffering and physical speaker output remain outside its exactness claim. The maximum four-packet ingress budget remains per whole native callback, including stale/empty/rejected packets, not per 2,048-frame subdivision.

The receiver is not publicly clonable. A private backing retention guard is held by the off-callback admission/supervisor owner until render **and error-closure destruction** are acknowledged. An off-callback mutex may guard the consumer; the callback never acquires it. Dropping the sole writer/receiver latches abandonment, rather than silently allowing audition to continue. The producer destruction Release acknowledgment occurs after its producer handle is dropped; final inspection uses Acquire. An exceptional premature loss of every off-callback guard must retain bounded backing storage for process retirement rather than free the last ring allocation on the callback. Constructor failure, failed play/pause and quiescence timeout must preserve this obligation. The existing failed-stop policy of retiring the child instead of retrying unsafe teardown remains applicable.

## Required next stage: immutable local originals

Create a fresh cleared live session for each performance. Arming an already sounding session would require a complete held-voice, sustain, patch and clock snapshot; that is not the first contract. Prepare the stage, descriptor, queue and writer before starting output or admitting notes. Return the stage identity early enough that start/fault/worker-loss UI can retain it.

Declare **six streams** at take creation: one F32 audio bus, four prepared GUI-source raw streams, and one engine-control raw stream. Source handles 1–4 and their connection generations are bound to immutable endpoint IDs in the descriptor. The complete worker/session/panic scope and original integer ordinals remain in each owned local record. Do not infer endpoints from filenames. Raw event range units are event ordinals; PCM range units are source audio frames. Neither may be substituted for the other.

Use the existing length-prefixed capture envelope with `raw: Local`, `musical: Uninterpreted` and a strict versioned extension, for example `omniusong.local_performance_v1`. Interpret that owned namespace only when the take's explicit performance provenance declares it. Its versioned typed payload retains full original `LiveEvent`, live scope, batch sequence, event index, callback index, exact applied bus epoch/rate/frame and disposition. Reject unsupported versions for performance replay/projection; retain unfamiliar data as original evidence. Do not generically reinterpret legacy unknown keys.

The GUI-source envelopes use their **actual original GUI clock** and `CaptureTime.alignment = None`. Applied-frame evidence is separate scheduling provenance, not a calibrated GUI-to-audio `ClockAnchor`. Capture must reject an unrepresentable GUI clock before start, or exactly reduce its ratio to fit the journal's numerator/denominator bounds; silent rounding is forbidden. In particular `LiveClock` presently permits a larger denominator than journal `HostClock`.

Engine-control records use a separate render-frame clock with rate `sample_rate / 1`, an explicit local output epoch and exact frame observations. They retain full callback Begin/End, dequeued batch disposition, empty lease checkpoints, generation clears and automatic transitions. Invalid/discarded packets whose source/clock cannot be authenticated belong here with their original fixed payload; they must not be represented as valid events on a declared GUI source. Applied event envelopes may be split by prepared source, with exact links back to the complete original callback/batch record.

The recorded-original claim is precise: **callback-received packets and applied events that reached this tap**. The parent queue and worker ingress can contain other accepted presses when a process dies. Expose parent/worker/applied/durable watermarks separately, plus known discarded identities and an unknown-tail flag. Never claim every parent-admitted press was captured merely because a later watermark exists. Recoverable records should include the pending/discarded fixed batches when they are actually available; absence remains an explicit limitation.

## Incremental writer and consistent recovery prefix

The writer drains and validates a complete Begin…End group before treating it as a coherent callback. It performs all serialization, UUID/string construction, hashing, chunking and SQLite work. It may buffer a bounded callback group and bounded chunk accumulators; it must not retain the entire performance. Keep existing audio payload ceilings of 1 MiB and raw chunk ceilings of 64 KiB. Flush on the first size threshold or approximately 250 ms of produced audio; tiny callbacks and event storms remain bounded by bytes/packet counts rather than wall-clock assumptions.

Recommend a bounded **atomic append-group** primitive for the later recording stage: validate all headers/payloads, insert the audio/source/control chunks and update their prefixes in one journal transaction, with the final owned performance checkpoint in that same transaction. This is an explicit API addition; calling existing single-chunk `append` repeatedly does not provide it. Limit one group to a fixed chunk count and byte budget (initial proposal: 64 chunks / 8 MiB). Split larger accumulated work at complete callback boundaries. Preserve immutable retry checks for each stream sequence and the group/checkpoint identity.

The checkpoint binds callback index, exclusive audio end, per-stream event ends, actual chunk identities and live scope. Recovery validates this cross-stream prefix as well as generic journal hashes. A crash cannot turn audio-only commits into a claim that matching human event originals survived. If an atomic group is not implemented, the alternative must explicitly retain all partial rows, expose per-stream prefixes and identify the last proven cross-stream checkpoint; it may not call the longest independent audio prefix a complete performance.

Queue saturation, receiver loss or writer/disk failure is terminal for the current capture. Do not silently drop events while continuing audio, silently substitute silence for unknown source loss, or fill an unknown tail. Known frame loss can be recorded only when both its exact range and reason are proved; such a gap is not a successful uninterrupted performance. The bus has no native input-clock gap estimator. After parent/child loss, recovered material remains an explicitly reviewed verified prefix, with all bad/unselected original rows and an unknown tail retained.

Thirty-minute capture is a streaming/disk requirement, not the existing playback preparation cap of 120 seconds/64 MiB. The proposed tap has no 120-second duration limit. Journal/source counters remain checked below signed storage limits; disk capacity and sustained writer throughput must be measured later. Current bounded playback/waveform preparation can still reject long recorded objects visibly; removing those rendering limitations is separate required work, not justification to truncate originals.

## Worker, Finish, Stop and UI lifecycle

The next worker command should be distinct from native input capture, for example `StartPerformance { output_device, gui_clock }`. The UI holds the target project/revision and chosen placement separately. Initial scope is a new solo performance at frame zero: no simultaneous backing playback, overdub, count-in or automatic device input. Return `PerformanceStarted { live, identity }`, with take/stage path, descriptor digest and output epoch, and show that identity during every active/fault status. Status never renews the existing GUI lease.

Recommended states are local Arm → Starting → Recording → Finishing → Sealed → explicitly Attaching → Attached, with Interrupted and retained uncertain-attachment branches. Starting has no note admission until both output and writer are ready. Recording retains normal source-aware sustain, repeated voices, accessibility Pulse, focus guards and one-use lease rules. Panic remains an explicit recorded clear/generation transition and can continue a healthy take only after the real callback acknowledgment. Lease/device/capture faults cannot use healthy Panic to resume the take.

`FinishPerformance` must close new live admission and fence the exact worker-accepted scope/sequence. The callback drains at most four packets per native invocation until every fenced packet is either applied or explicitly disposed; a control-thread watermark alone cannot satisfy this fence. For the first complete writer/worker packet, choose and document a simple **explicit finish cut** at the fenced callback boundary: record engine Finish clear/end, silence remaining voices, then stop output. This preserves the exact captured bus and truthfully identifies any truncated held/release sound. A future release-tail finish may instead render a bounded tail, but it needs its own finite callback-owned state/deadline and must not synthesize human NoteOff events. Do not let an unresolved tail policy block the first tap, which adds neither Finish command nor seal.

Only an explicit healthy Finish can produce a normal sealed manifest, after: callback finish fence, successful stop, both closure-destruction acknowledgments, complete queue drain, final cross-stream durable checkpoint, and normal journal seal with a stable finalize operation ID. A timeout or failed pause retires the child and retains the stage; never manufacture an acknowledgment. Normal completion must be retained even behind a queued reply, and the exact first terminal cause must be delivered once.

Stop, Cancel after start, parent EOF, device failure and queue/writer fault retain unsealed originals. Stop is always reachable, outranks Finish/start, and does not mean a successful take. Start and a lost ephemeral Finish reply cannot be blindly repeated; inspect the retained stage and stable finalization identity. No automatic seal, resume or project attachment occurs after reconnect.

The performance target and retained original must outlive live-process retirement. Existing audition cleanup may release an audition guard; it must not erase a recording target. Guard project mutation/navigation/archive/close consistently during active/finalizing/attaching states. After interruption or a completed unattached take, an explicit keep-local choice can release navigation while retaining stage identity and any exact unresolved publication request. Unattached stages remain outside ordinary project archive closure and must be labeled accordingly.

## Attachment, derived editing and archive fidelity

An initial explicit audio `TakePublication` may attach the sealed PCM stream at a reviewed arrangement position using its exact source rate. This retains the full physical journal, including all GUI/control streams, through the existing immutable object registry, history, operation results and complete archives. Do not add active raw-event clips to audible playback: the current renderer correctly rejects them. A dedicated read-only event summary or explicitly muted raw lane can expose their presence without pretending it is editable or playable MIDI.

A later **separate typed atomic operation** must publish the editable projection together with its source reference/projection receipt. Do not simulate one atomic performance acceptance using a sequence of independent AddNote operations or unsupported `ApplyPreparedEdit`. The request binds operation ID, expected project revision, source take/manifest, exact stream/checkpoint selection, projection version, tempo/time mapping and explicit rounding/loss policy. Archive request validation and copy remapping must recognize the owned shape; opaque original fields and take bytes remain unchanged. Replay authenticates the same immutable source and original result before external stage access. Changed requests cannot reuse an operation ID.

The unquantized originals remain authoritative. Derive identities from full take/worker/session/panic/source/connection/note-or-voice identity, preserving original integer ordinals. Sustain/key-up, auto release and audible end remain different observations. Preserve exact attack/release velocity, patch, Pulse and timing provenance even where the current editable schema has no corresponding rendered field. A raw `takeId` in a note extension is not by itself an owned dependency: retain an explicit typed source reference in the publication's project structure.

Existing notes have integer 960-PPQ positions and positive durations; zero-frame notes and two distinct frame times that round to one tick cannot be silently padded to a tick or discarded. Offer a bounded explicit projection result that reports unrepresentable/collapsed observations, keeps them in originals, and requires a chosen conversion policy before editable publication. Preserve residual/exact source-frame provenance and do not label tick conversion as exact timing. Existing note limits and the 8-MiB schema-2 metadata cap constrain the derived view, not the length of the original journal; large projections need explicit bounded regions/chunks, never silent truncation.

The saved-note renderer currently uses a different procedural path from `LiveRenderer`, including patch, sustain and envelope behavior. Until shared playback semantics are implemented and independently compared, auditioning derived notes is a **derived interpretation**, while immutable PCM is the exact recorded logical mix. Avoid default simultaneous PCM+derived playback, which doubles the performance. Original and edited interpretations need explicit audition selection and source-preserving, reversible project operations. High-resolution expression and MIDI 2/MPE remain future mandatory lanes; this local extension must not pre-quantize their originals.

## Independent acceptance plan

| Gate | Required observations |
| --- | --- |
| First tap, no devices | Apply actual `LiveRenderer` packets and compare PCM bit-for-bit with ordinary live F32 output; verify I16/U16 conversions separately without calling stored F32 physical-exact audio. Test channels 1–8, callback lengths 1/256/2047/2048/2049/16384, malformed/oversized callbacks, zero-on/release velocities, repeated pitch and mixed patches. |
| Fixed work and ownership | Count at most four total ingress dispositions and at most 70 records per invocation. Test queue exactly sufficient/one slot short, consumer stalled/dropped, control guard held, allocation/deallocation flags including renderer destruction, counter exhaustion and complete-group/partial-group behavior. No queue mutation or voice application after failed reservation. |
| Musical originals | Verify exact scope/sequence/source ticks and u16 values; whole-batch rejection; source sustain ownership; late key-up after Pulse/drum end; 64 voices including release tails; <=128 exact auto-release/end observations; healthy Panic disposal/reset plus same-epoch tick high-water marks. |
| Fault and clocks | Lease expires at the exact bus frame, queued lease credits cannot renew forever, Status never renews, Panic is not a fault reset, foreign scope stays inert, and first cause cannot be overwritten. No fabricated GUI/audio alignment or host timestamps. |
| Writer and interruption | Inject failure before/after each multi-stream transaction boundary; recover a consistent complete checkpoint with partial/bad tails retained. Exceed old 30-MiB/4,096-event bridge limits using streamed fixtures. Assert all original payloads and dispositions, unknown tail, per-source clock binding, and no production cap inherited from offline playback. |
| Lifecycle/private process | Stop during Start/Finish, parent EOF, writer failure, output pause/play failure, both closure acknowledgments, late normal completion behind a terminal event, lost Finish reply and stable finalization inspection. Device-free helper tests cannot assert a simulated callback acknowledgment proves a native backend stop. |
| Publication/edit/archive | Exact attachment retry after stage deletion and later edits; conflict and changed operation rejection; SQL rollback; Undo/Redo and operation-only source closure after redo truncation; identity restore and restore-as-copy retain whole journal bytes, original source/clock/patch data and projection receipts. |
| Actual later package | Explicit Record keys/pads, genuine repeated key/pad and sustain gestures, Panic continuation, Finish/Stop distinction, visible early identity, recovery after restart, attach/edit/Undo/Redo/archive/copy/reopen, exact original comparisons and output-only replay. Preserve the current 0978 package until its separate pending walkthrough is complete. |

The first tap can be accepted as a backend data-path increment only after its independent fixed-work, original-event, PCM, ownership and fault checks pass. The full saved-performance workflow additionally requires the writer, worker/UI lifecycle, recovery, attachment and editable-derived publication gates. Source/build tests alone do not close those gates or any release measurement.
