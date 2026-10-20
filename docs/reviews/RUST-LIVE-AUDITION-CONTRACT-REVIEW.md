# Independent live-audition contract review

October 20, 2026. Read-only review of [NATIVE-LIVE-AUDITION.md](../contracts/NATIVE-LIVE-AUDITION.md), draft SHA-256 `691c43d1c67ed2b4a4ec657d6972d8c20fe87fed411436b5c3763e8125ab14f1`, at local base `190878c`. This report alone is owned by the reviewer. No production/contract edits, Cargo, devices, UI or new subagents were used. Recommendations below require coordinator adoption; they are not implemented interfaces.

The proposed procedural, output-only keys/pads slice is defensible. It explicitly excludes recording, hardware MIDI/MPE/MIDI 2 transport and physical latency claims. The draft correctly says separate parent ingress still shares serialized private IPC, and panic can wait for that exchange or the callback-frame lease. **The coordinator has accepted the required corrections below; exact public API and contract freeze remain prerequisites to implementation.** The accepted design uses cumulative `EngineUpdate`, urgent tickets, a nonblocking supervisor outbox/wake, contiguous sequences, callback-confirmed panic, admission-time lease deadlines and explicit live-output destruction acknowledgements. No live implementation or qualification is claimed.

## Required contract edits

### 1. Define contiguous admission, stale rejection and panic acknowledgement

Draft lines 27–41 say both monotonically increasing sequences and fail-on-sequence-loss. Strictly increasing sequences alone allow gaps. Choose exactly contiguous batch admission: first sequence 1, then checked `last_accepted + 1`, including empty batches. Validate the entire packet before advancing the watermark. Rejection consumes no sequence. Parent-accepted, worker-accepted and callback-applied counters must remain distinct and scoped to the same session/panic generation.

An old session/panic/source generation is rejected and counted **without panicking a newer generation**. A missing/duplicate sequence or invalid packet in the currently admitted generation instead closes its note admission and latches a visible fault/panic. This distinction is necessary to satisfy line 37: delayed old packets cannot be allowed to silence new voices either.

Give panic a session-scoped checked monotonic ticket. A dedicated parent latch stores the latest requested ticket; an acknowledgement clears it only if no newer ticket arrived. Stop dominates panic. A repeated ticket may return its already established acknowledgement without incrementing the generation twice. A new panic advances the panic generation, invalidates queued older batches, resets per-generation accepted/applied sequence watermarks to zero, and returns next sequence 1. The **callback exclusively owns and clears voices/sustain**. The worker must wait for the callback's matching generation/ticket acknowledgement before returning PanicLive success or opening admission to fresh presses. A control-side acknowledgement before that point can permit old sound or new notes before cleanup. Bound the wait and retire on failure; do not simulate callback acknowledgement when callbacks stop.

### 2. Make lease renewal independent of delayed packet application

The one-use GUI token rule in lines 45–49 is necessary but does not alone prevent backlog renewal. If the renderer renews on each dequeued packet, several distinct tokens already accepted while the GUI was alive could extend its lease after it freezes. Define renewal at **worker admission time**, using the callback's published session-relative frame counter, and publish a single latest absolute deadline. Event packets may retain their original token for evidence but must never refresh the timer when eventually applied. Ordinary Status, dequeue activity and supervisor forwarding must never manufacture or rotate GUI credit.

Use an opaque worker-created token scoped to session and panic generation. Only one outstanding unconsumed credit exists; the GUI learns the next one through a polled receipt. Consuming a credit and issuing its successor happen once for a wholly admitted packet. Rejected/full/malformed packets do not extend a lease. A previously issued token can accompany ordered events but cannot renew again. Parent/supervisor code forwards the exact original credit and never substitutes one from a newer reply. Panic rotates it only as part of the acknowledged new generation.

Specify a linearization rule between expiry and renewal. A minimal representation is a checked `AtomicU64` absolute output-frame deadline with an expired sentinel; renewal and expiry compare/exchange the same state, and expired is terminal until a new session. The worker checks current frame against the existing deadline before attempting renewal. Callback expiry checks must happen before rendering beyond the deadline, not merely once per 2,048-frame sub-block. Render at most the valid prefix then silence the rest. Use bounded atomic attempts; contention must fail conservatively without a callback spin loop. A renewal that loses to expiry cannot rearm the session. An exact algorithm still needs source review; independent atomics for a renewable counter and expired flag without an ordering proof are insufficient.

Retain the draft's honest allowance: one unused GUI-issued token may arrive after the last GUI action and renew once. Multiple queued packets carrying it renew once in total. Frames may not advance when the native callback itself is stalled, so this is not a wall-clock or physical silence guarantee. A late packet after expiry requires explicit Stop/new Start even if its token was once valid.

### 3. Freeze one callback work budget and whole-packet application semantics

Line 55 alternates callback/sub-block and leaves four packets as a suggestion. Freeze **four packets total per native callback**, at most 32 events each, including empty, stale and rejected packets. Do not reset that budget for every 2,048-frame sub-block or keep draining while the producer refills. Panic/lease checks have their own bounded fixed-state path and cannot wait for this packet budget. Define a supported native callback frame ceiling, preferably the existing 16,384-frame policy; above it, fill silence and latch a device/buffer fault. Silence filling is necessarily linear in the supplied output slice, but voice/event DSP must remain bounded and disabled for invalid shapes.

State the first slice's scheduling policy: ordered admitted events apply at a defined next callback/sub-block boundary; source timestamps remain observations and are not claimed to be calibrated sample offsets. If sample scheduling is not implemented, do not imply it through the `RealtimeBatch` type. Define same-boundary on/off ordering explicitly.

Stage one packet against preallocated 64-voice/source state, validate every transition, then commit before producing its sound. A late invalid event or 65th occupied voice cannot leave the preceding events audible. Envelope release tails still occupy slots until they end; a NoteOff is not automatically a free slot. This admits a straightforward fixed-state copy/preflight without allocation or unbounded rollback. With four packets and 32 events, even linear 64-slot lookups have a concrete upper bound; measure it later rather than calling it low latency now.

### 4. Freeze source/voice ownership and bounded release rules

Lines 29/33 leave a voice identifier insufficiently bound. A live voice must retain `(session, panic_generation, source_handle, source_connection_generation, note_handle, voice_handle, patch)`. NoteOff and Sustain may mutate only that source/connection's state. Reject another source's attempt to release a matching numeric handle. Prepare at most 16 sources at Start; for this first local-only slice, make their connection generations immutable during the session. Source loss can request the same global panic protocol; hot reconnection/source-table mutation need not be invented here.

Use checked, monotonically issued, nonzero note/voice ordinals per source and never reuse them within a session/panic generation. This lets ended one-shots have bounded late-release handling: a release for an issued, no-longer-active voice is a counted no-op; a never-issued future handle is invalid. It avoids an ever-growing tombstone table. Sustained voices remain source-owned, and velocity zero is a distinct admitted silent NoteOn, not MIDI 1 NoteOff reinterpretation. Preserve release velocity and decide its audible envelope behavior explicitly.

A discrete accessibility activation needs a callback-owned finite pulse/auto-release mode. A delayed GUI timer that later sends NoteOff is not an independent reliable release. A small fixed `Held | Pulse` gate on NoteOn, with a prepared bounded pulse duration, is enough. Drums can remain bounded one-shots; patch selection at NoteOn stays immutable for that voice. Define focus/pointer cancel cleanup, modifier/text-field suppression, key-repeat rejection, and whether losing one surface triggers source cleanup or global panic; none may replay held notes after acknowledgement.

### 5. Freeze public polling and saturation semantics

Lines 15–23 and 71 defer the interface that engine and UI must share. The smallest proposed additions are:

```text
StartLive -> LiveReady { handle, output_config, sources, next_sequence: 1, lease_token }
LiveHandle = { worker_generation, session_uuid, panic_generation }
try_push_live(FixedLiveBatch) -> AcceptedToParent { sequence }
                           | Stale | Saturated { panic_ticket } | Retired | Invalid
request_live_panic(session_uuid) -> PanicTicket | Stale | Retired
try_poll_update() -> Option<EngineUpdate>
EngineUpdate = { ordinary_reply?, live_snapshot?, terminal? }
LiveSnapshot = { handle, accepted_sequence, applied_sequence, active_voices,
                 latest_lease_token, lease_deadline_frame, fault, panic_ack }
```

The sketch follows the coordinator's cumulative `EngineUpdate` choice; final field/method spelling remains a shared API freeze decision. Freeze one UI polling path for ordinary control completions, live progress and terminal state; do not introduce competing consumers of the same completion queue. Its live snapshot retains the latest outstanding token, cumulative accepted/applied watermarks and sticky fault; coalescing cannot erase a token, pretend an unapplied packet rendered, or hide a fault behind idle. Token/receipt updates from an older handle cannot overwrite the current handle. A panic acknowledgement explains which old accepted-but-unapplied sequences were invalidated. Existing completed control replies and exact terminal causes must remain distinguishable and observable once; if one update contains both, the UI must process both.

`request_live_panic` must be independent of the busy capacity-one RPC and the full live queue. Current-session parent saturation closes admission and latches panic **inside `try_push_live` before returning**; UI cleanup alone is not sufficient if the GUI freezes immediately afterward. The supervisor selects Stop, then panic, then bounded ordinary control/live work at exchange boundaries; one stalled exchange still has its existing deadline. The worker has the equivalent bounded panic/lease path. The accepted supervisor outbox/wake must remain nonblocking even when an ordinary completion is unread while the child is active; a blocking reply send would prevent urgent work and heartbeat progress. Retain the completed reply and exact terminal cause in bounded state without hiding either behind newer live status. No automatic event retries after uncertain admission.

### 6. Give live output an explicit closure-lifetime proof

Draft line 43 describes existing acknowledged destruction rules, but current `Playback::stop` is `pause()` then `drop(self)` (`crates/omniusong-engine/src/worker.rs:160–168`); the renderer/error-handler destruction acknowledgements exist on Capture, not that playback struct. Require the new live renderer closure and its error closure to each publish a destruction acknowledgement, with control retaining all backing allocations/refcounted owners until both are observed after successful pause/drop. Reuse the bounded wait/worker-retirement policy. A failed pause must keep ownership until process retirement, and an uncertain callback lifetime must reject subsequent sessions. Control must never directly mutate callback-owned voices to make panic appear complete.

## Independent acceptance cases required before native UI evidence

| Area | Required deterministic proof |
| --- | --- |
| Work and ownership | Native callbacks of 1, 2,047, 2,048, 2,049 and 16,384 frames; malformed channel shape/oversize; concurrently refilled queue still drains at most four total packets; no callback allocations/deallocations or final-owner drop. |
| Admission | Exact contiguous sequences including empty batches; replay/decrease/gap reject without rebasing; stale session/generation has no effect on newer sounding voices; whole-packet failure after valid leading events produces none of that packet's sound. |
| Panic races | Saturated parent and callback queues; panic while a live RPC is held; a second ticket arrives during the first acknowledgement; older reply cannot clear newer latch; acknowledgement occurs only after callback clear; post-panic old packets never retrigger. |
| Lease credits | Freeze GUI with 16 parent and 16 callback packets queued; repeated old credit renews once; several previously accepted credits do not renew on dequeue; supervisor heartbeats and later idle/status do not renew; unread successor credit cannot be inserted by the supervisor. |
| Expiry race | Renewal just before/at/after deadline, concurrent expiry/renewal and deadline within a callback; no sample beyond the allowed live prefix; terminal expiry cannot be rearmed by queued tokens or focus return; no counter wraps. |
| Voice ownership | Repeated same-pitch independent voices, same numeric handle on different sources, per-source sustain, held releases under sustain, release-tail polyphony, zero/max velocities, late one-shot release, invalid future handle and accessibility pulse ending without GUI callbacks. |
| Process/terminal | Helper peer stalls live acknowledgement, errors on a heartbeat, or exits while normal completion and live status are occupied; exact terminal cause retained once, new admission stops, child retires, no replay. |
| UI/preservation | Pointer up/cancel/leave, focus loss, text/modifier/repeat protection, full UI buffer, Stop during start, mandatory fresh press after panic, explicit source/patch/velocity/not-recording display; exact project/journal/asset bytes unchanged. |

After these contract decisions and software gates, actual Mac testing may qualify explicit procedural output audition and its visible controls under the existing output-only authorization. It must identify the tested binary/device/configuration. No input device, performance recording, native MIDI delivery, MPE, audible-quality, physical latency, 30-minute stability or Windows claim is part of this contract review.

## Coordinator incorporation

The coordinator incorporated the six accepted corrections into the draft after this initial review, plus the engine/UI API feasibility feedback: contiguous per-scope sequences, inert stale packets, scoped callback-confirmed Panic, admission-time one-use lease credits with sticky sample-exact expiry, four packets total per callback, source-owned nonreused voices and callback-owned pulses, nonblocking actor outbox/wake, cumulative urgent ticket status and both live output closure acknowledgements. Initial line references above refer to the draft at review start. Exact Rust DTOs/private response variants, Stop/start fencing and the bounded lease algorithm still need coordinator freeze/source review before implementation. No live implementation or runtime acceptance follows from this document.
