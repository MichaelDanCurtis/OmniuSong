# Saved-performance Finish and native lifecycle

October 20, 2026. Implementation contract; this document is not qualification evidence. Extends the [tap](NATIVE-PERFORMANCE-TAP.md) and [writer](NATIVE-PERFORMANCE-WRITER.md) while preserving every existing version-1 take. The full workstation goal and milestone 2 remain incomplete.

## Delivery sequence and authority

Implement the callback-owned Finish fence/cut and version-2 durable evidence first. Then integrate the actual native output worker, one draining writer, early stage identity and private normal-seal authority. Finally connect explicit recording/recovery/attachment controls and separately derived editable notes. A callback acknowledgment, codec test or device-free renderer is not a completed native recording workflow.

This path records the procedural post-mix F32 output bus and original keyboard/pad batches. It opens no native input stream. It does not convert observations into MIDI, rescale GUI ticks, invent physical alignment, create model inference, synthesize human releases or promise exact editable replay through the different saved-session instrument.

## Immutable admission fence

Add these fixed, strict serde DTOs to the live types:

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
```

`LiveAdmission::begin_finish(scope, id)` captures the sole worker admission owner's actual last accepted sequence; the caller cannot supply a watermark. Only a recorded pair may finish. Reject an invalid ID, foreign scope, pending Panic, sticky fault or expired lease. The first successful request permanently closes new batch admission, credits and Panic. Repeating the identical scope/ID returns the same immutable fence without extending the finishing window; a different request cannot replace it. Use a distinct `FinishPending` admission error. Existing non-recording live audition remains unchanged.

The Finish lease transition must be bounded and never revive an expired/faulted performance. Before publishing the request, use the existing deadline CAS protocol to extend a still-live lease once, to a checked frame bound sufficient for the maximum 16 queued batches at four per callback, with bounded boundary slack. Adopt `max(existing live deadline, observed frame + 6 * LIVE_MAX_CALLBACK_FRAMES)`; this preserves a longer already-live one-second lease at high sample rates. Reject arithmetic exhaustion and an existing EXPIRED latch; never clear that latch or a first fault. The callback's lease-expiry operation and this extension compete through the same atomic deadline. A request observed after a fault cannot turn it healthy. The worker later imposes a separate wall-clock Finish deadline; the frame allowance is not native responsiveness evidence.

The fence publication is release/acquire ordered after its fixed ID, scope and sequence data are written, and callback acknowledgment is a separate one-shot publication. Avoid allocation, blocking locks, serialization and filesystem operations in the callback. Keep the request immutable after publication. A snapshot returning no acknowledgment while publication is incomplete must not fabricate a cut or resolve a pending fault.

`LiveProgress` uses explicit `Finishing` and `FinishedCut` states rather than reporting running audition without a valid credit. Both require no published fault, no credit and a positive deadline. `FinishedCut` also requires zero voices/tails, matching accepted/applied sequences and a positive fixed recorded frame. Neither state claims native shutdown or a sealed take; `finish_ack` remains the fault-vetoing callback acknowledgment query. Ordinary audition states and behavior remain unchanged. Version the actual worker protocol when these new recording commands/states are exposed through IPC.

## Callback-owned hard cut

The existing four-batch whole-callback limit stays unchanged. Starting Finish does not discard accepted batches. Subsequent callback boundaries apply/retain queued batches normally until the request's same-scope accepted sequence is fully applied and the queue is empty. Empty fence means no accepted batch, not sequence zero. A rejection, mismatch, fault, queue/tap overflow or unsupported generation prevents a healthy Finish.

Render the entire final native callback, including all logical F32 PCM. At its exclusive end, count active voices and release tails, explicitly clear remaining voices/sustain, and emit `PerformancePacketKind::Finish(TapFinish)` **after all PCM and before End**. Finish is a separate control action, not a NoteOff, Panic, automatic voice end or fault. Preserve the cut counts even though End reports zero active/tail voices. A held or sustained human note remains held in its original event stream; the cut only explains the actual bus boundary. The first policy is explicitly hard cut, without extra natural-release audio or an inferred missing tail.

Reserve one additional tap slot for every recorded callback before changing renderer state: at most 71 packets (Begin, four batches, 64 PCM, Finish, End), with the existing 256-slot bounded ring and 16-KiB packet ceiling. No Finish record may precede the full PCM span, appear twice, be followed by another batch/PCM or disagree with its callback/scope/end/fence. End must carry no live/tap fault and must be retained completely before the callback publishes the Finish acknowledgment. A request that arrives too late for a callback is handled by the next callback; do not label an earlier boundary retroactively.

After the cut, later native buffers contain silence and create no further tap callbacks or recorded frames. The final recorded frame watermark remains fixed. A real asynchronous device fault remains sticky even after acknowledgment and can still veto normal sealing. Stop/cancellation remains an interruption; Finish acknowledgment alone is not authority to seal. Any malformed callback after the cut is still a fault, even though it cannot extend the original.

## Explicit owned profile version 2

Retain `omniusong.performance.v1` and all version-1 descriptor, envelope, checkpoint bytes and read/recovery behavior. Add `PerformanceProfile::{OriginalV1, FinishV2}`, with distinct owned namespace `omniusong.performance.v2`, provenance version 2 and `LocalRecord.version == 2` for **every** version-2 record. A descriptor/envelope must contain exactly its one matching owned namespace; reject mixed markers, mismatched versions, unknown owned fields and unsupported future versions.

Keep `create_descriptor` and `PerformanceWriter::create` as version-1 APIs for existing callers/fixtures. Add explicit profile creation and `PerformanceWriter::create_finishable` for version 2. `from_descriptor` accepts both exact profiles and regenerates the descriptor with its actual profile. Preserve all exact source clocks, prepared source bindings, original values and six-stream structure. No implicit migration or rewrite of existing objects occurs.

Add `PerformanceRecord::Finish { callback_index, finish: TapFinish }` only in version 2. It belongs on the control stream with observed ticks equal to the cut frame. The generic envelope remains Local/Uninterpreted with no alignment. Encoding/decoding binds the owned namespace to the actual record version. Checkpoints hash under `OmniuSong.performance-checkpoint.v1\0` or `OmniuSong.performance-checkpoint.v2\0` according to their strict profile; retain the existing member/prefix semantics.

The incremental writer and semantic scanner require at most one Finish, after complete PCM and before its matching End. Validate identical callback index, exact scope/current accepted applied fence, cut frame equal to callback exclusive end, valid ID/policy and bounded counts; End after the cut reports zero active/release-tail voices. No following callback or new source/control originals may extend a finished performance. The final complete checkpoint includes the Finish record. A version-2 take with no Finish can still be explicitly recovered to a valid interrupted checkpoint; a Finish-bearing interrupted take still has unknown tail and is not automatically normally sealed.

Generic `Journal::seal`, generic recovered sealing and `CaptureReader::recover` must refuse either performance marker, including unsupported/null values and both markers. Format-aware recovery binds the selected checkpoint, descriptor/profile and all original rows. No new normal-seal convenience API is introduced in this prerequisite packet.

## Native worker integration requirements

Before admitting any note, a distinct recorded-live Start must create the private stage and writer from the actual selected output configuration, retain a stable take/descriptor/path identity and publish that identity to the parent. Build only the actual output stream. Move the real renderer and one real error guard into its data/error closures; never construct/drop a substitute guard to satisfy a check. Retain the admission backing and unique tap owner until closure destruction. A setup failure retains the stage and identifies it; there is no silent plain-audition fallback.

A single bounded off-callback owner drains the tap into `PerformanceWriter`. Source, callback-complete and durable progress remain distinct. Storage validation/error/ambiguous commit immediately closes recording admission and retains the exact pending write; successful exact retry does not resume capture. A stalled SQL call is not interruptible merely because a worker thread exists; process retirement/deadlines remain necessary, and must not be reported as healthy Finish.

The parent actor must close its own live admission before requesting Finish and distinguish parent-queued from worker-accepted batches. The worker captures its immutable fence only after the protocol's preceding accepted requests are accounted for. Finish processing must not block the serialized request loop waiting for callback/SQL completion: urgent Stop, fault, parent EOF and watchdog retirement must remain actionable. Poll an explicit finishing state; preserve the stable Finish ID and original stage on deadline/channel failure.

Only after callback Finish acknowledgment may the worker pause and drop the **actual** output stream. It must then observe actual renderer destruction, actual error-closure destruction and tap producer destruction, drain through the final End with an empty queue, and validate the final checkpoint matches the cut and all six original prefixes. Check final live/tap faults and cancellation after those real closures are gone. Missing acknowledgment, pause/drop failure, cancellation, Stop/EOF or a device fault retains an unsealed original and unknown tail.

A private lifecycle-owned capability, created only after that proof, may authorize a normal manifest transaction. Public caller booleans, caller-authored counters, an empty queue, flush success or offline guard drops are insufficient. Define the seal linearization point explicitly: Stop/EOF observed before authority wins leaves unsealed. Once the actual transaction is authorized/in flight, a racing Stop cannot promise that no manifest committed. Preserve/replay the exact stable Finish intent and report committed or uncertain state truthfully. A retry must authenticate descriptor, original rows, cut/fence/checkpoint and operation identity without rerendering. Recovered manifests remain separately labeled with unknown tail.

## Acceptance gates

Device-free source/tests must cover more than four and all sixteen accepted batches; empty fence; exact complete PCM/cut/End ordering; held/sustained/release-tail voices; no synthetic human offs; silent subsequent buffers; original ticks/velocities; identical/different ID retry; foreign scope; pending/completed Panic; expired/near-expiry lease; faults before/during/after callback acknowledgment; callback/tap capacity; cancellation and queue overflow; no finish on ordinary audition; bad/missing/duplicate/early Finish; malformed post-cut buffer; explicit profile/namespace mismatch; old version-1 byte identities; version-2 semantic recovery retaining originals and unknown tail.

Then separately qualify actual worker lanes/draining and early identity, independently held data/error closures, native pause/drop/failure/retirement, Stop/EOF races, storage stall/failure, exact normal-seal lost-reply replay, native UI controls/retained recovery/attachment, Undo/Redo and complete archive restoration. No source-only result can close these actual native/UI gates. Preserve the user's software-loopback-only restriction: this entire path can be qualified through output without opening a native input stream. Physical/listening, thirty-minute stability, MIDI 2/MPE, native inference/plugins, installers, Windows and full-song acceptance remain independent.
